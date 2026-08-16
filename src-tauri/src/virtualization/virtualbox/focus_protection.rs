use std::sync::Mutex;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedVmWindowIdentity {
    pub vm_uuid: String,
    pub vm_name: String,
    pub session_pid: Option<u32>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FocusProtectionStatus {
    pub active: bool,
    pub locked: bool,
    pub managed_window_found: bool,
    pub restoration_count: u64,
    pub last_error: Option<String>,
}

pub trait FocusProtectionBackend: Send + Sync {
    fn start(&self, target: ManagedVmWindowIdentity, anchor: Option<isize>) -> Result<(), String>;
    fn set_locked(&self, locked: bool) -> Result<(), String>;
    fn stop(&self) -> Result<(), String>;
    fn status(&self) -> FocusProtectionStatus;
}

#[derive(Default)]
pub struct SimulatedFocusProtection {
    status: Mutex<FocusProtectionStatus>,
}

impl FocusProtectionBackend for SimulatedFocusProtection {
    fn start(
        &self,
        _target: ManagedVmWindowIdentity,
        _anchor: Option<isize>,
    ) -> Result<(), String> {
        *self
            .status
            .lock()
            .map_err(|_| "focus status lock is poisoned")? = FocusProtectionStatus {
            active: true,
            locked: true,
            ..FocusProtectionStatus::default()
        };
        Ok(())
    }

    fn set_locked(&self, locked: bool) -> Result<(), String> {
        let mut status = self
            .status
            .lock()
            .map_err(|_| "focus status lock is poisoned")?;
        status.locked = locked;
        status.active = locked;
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        *self
            .status
            .lock()
            .map_err(|_| "focus status lock is poisoned")? = FocusProtectionStatus::default();
        Ok(())
    }

    fn status(&self) -> FocusProtectionStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_else(|_| FocusProtectionStatus {
                last_error: Some("focus status lock is poisoned".to_owned()),
                ..FocusProtectionStatus::default()
            })
    }
}

#[cfg(target_os = "windows")]
mod windows_backend {
    use std::{
        cell::RefCell,
        path::Path,
        sync::{mpsc, Arc, Mutex},
        thread::{self, JoinHandle},
    };

    use windows::{
        core::PWSTR,
        Win32::{
            Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM},
            System::Threading::{
                GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
            UI::{
                Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
                WindowsAndMessaging::{
                    DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowTextLengthW,
                    GetWindowTextW, GetWindowThreadProcessId, IsWindow, PostThreadMessageW,
                    SetForegroundWindow, TranslateMessage, EVENT_SYSTEM_FOREGROUND, MSG,
                    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_QUIT,
                },
            },
        },
    };

    use super::{FocusProtectionBackend, FocusProtectionStatus, ManagedVmWindowIdentity};

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct WindowDescriptor {
        process_id: u32,
        executable_name: String,
        title: String,
    }

    fn should_restore_focus(
        target: &ManagedVmWindowIdentity,
        window: Option<&WindowDescriptor>,
        locked: bool,
    ) -> bool {
        if !locked {
            return false;
        }
        let Some(window) = window else { return false };
        if !window
            .executable_name
            .eq_ignore_ascii_case("VirtualBoxVM.exe")
        {
            return false;
        }
        if target
            .session_pid
            .is_some_and(|pid| pid == window.process_id)
        {
            return true;
        }
        target.session_pid.is_none()
            && !target.vm_name.trim().is_empty()
            && window
                .title
                .to_ascii_lowercase()
                .contains(&target.vm_name.to_ascii_lowercase())
    }

    #[derive(Clone, Debug)]
    struct NativeState {
        target: ManagedVmWindowIdentity,
        anchor: Option<isize>,
        last_safe_window: Option<isize>,
        status: FocusProtectionStatus,
    }

    struct Listener {
        thread_id: u32,
        handle: JoinHandle<()>,
    }

    #[derive(Default)]
    pub struct WindowsFocusProtection {
        shared: Arc<Mutex<Option<NativeState>>>,
        listener: Mutex<Option<Listener>>,
    }

    thread_local! {
        static CALLBACK_STATE: RefCell<Option<Arc<Mutex<Option<NativeState>>>>> = const { RefCell::new(None) };
    }

    impl FocusProtectionBackend for WindowsFocusProtection {
        fn start(
            &self,
            target: ManagedVmWindowIdentity,
            anchor: Option<isize>,
        ) -> Result<(), String> {
            self.stop()?;
            let foreground = unsafe { GetForegroundWindow() };
            let foreground = (!foreground.is_invalid()).then_some(foreground.0 as isize);
            *self
                .shared
                .lock()
                .map_err(|_| "focus state lock is poisoned")? = Some(NativeState {
                target,
                anchor,
                last_safe_window: anchor.or(foreground),
                status: FocusProtectionStatus {
                    active: true,
                    locked: true,
                    ..FocusProtectionStatus::default()
                },
            });

            let shared = self.shared.clone();
            let (sender, receiver) = mpsc::sync_channel(1);
            let handle = thread::Builder::new()
                .name("multiseat-focus-protection".to_owned())
                .spawn(move || run_listener(shared, sender))
                .map_err(|error| format!("could not start focus protection thread: {error}"))?;
            let initialized = receiver
                .recv()
                .map_err(|_| "focus protection thread exited before initialization".to_owned());
            let thread_id = match initialized {
                Ok(Ok(thread_id)) => thread_id,
                Ok(Err(error)) | Err(error) => {
                    let _ = handle.join();
                    *self
                        .shared
                        .lock()
                        .map_err(|_| "focus state lock is poisoned")? = None;
                    return Err(error);
                }
            };
            *self
                .listener
                .lock()
                .map_err(|_| "focus listener lock is poisoned")? =
                Some(Listener { thread_id, handle });
            Ok(())
        }

        fn set_locked(&self, locked: bool) -> Result<(), String> {
            let mut shared = self
                .shared
                .lock()
                .map_err(|_| "focus state lock is poisoned")?;
            let state = shared
                .as_mut()
                .ok_or_else(|| "focus protection is not initialized".to_owned())?;
            state.status.locked = locked;
            state.status.active = locked;
            Ok(())
        }

        fn stop(&self) -> Result<(), String> {
            let listener = self
                .listener
                .lock()
                .map_err(|_| "focus listener lock is poisoned")?
                .take();
            if let Some(listener) = listener {
                unsafe { PostThreadMessageW(listener.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
                    .map_err(|error| format!("could not stop focus protection thread: {error}"))?;
                listener
                    .handle
                    .join()
                    .map_err(|_| "focus protection thread panicked".to_owned())?;
            }
            *self
                .shared
                .lock()
                .map_err(|_| "focus state lock is poisoned")? = None;
            Ok(())
        }

        fn status(&self) -> FocusProtectionStatus {
            self.shared
                .lock()
                .ok()
                .and_then(|state| state.as_ref().map(|state| state.status.clone()))
                .unwrap_or_default()
        }
    }

    impl Drop for WindowsFocusProtection {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }

    fn run_listener(
        shared: Arc<Mutex<Option<NativeState>>>,
        sender: mpsc::SyncSender<Result<u32, String>>,
    ) {
        CALLBACK_STATE.with(|slot| *slot.borrow_mut() = Some(shared));
        let hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(win_event_callback),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if hook.is_invalid() {
            let _ = sender.send(Err(
                "SetWinEventHook(EVENT_SYSTEM_FOREGROUND) failed".to_owned()
            ));
            CALLBACK_STATE.with(|slot| *slot.borrow_mut() = None);
            return;
        }
        let thread_id = unsafe { GetCurrentThreadId() };
        if sender.send(Ok(thread_id)).is_err() {
            let _ = unsafe { UnhookWinEvent(hook) };
            CALLBACK_STATE.with(|slot| *slot.borrow_mut() = None);
            return;
        }
        let mut message = MSG::default();
        loop {
            let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
            if result.0 <= 0 {
                break;
            }
            let _ = unsafe { TranslateMessage(&message) };
            unsafe { DispatchMessageW(&message) };
        }
        let _ = unsafe { UnhookWinEvent(hook) };
        CALLBACK_STATE.with(|slot| *slot.borrow_mut() = None);
    }

    unsafe extern "system" fn win_event_callback(
        _hook: HWINEVENTHOOK,
        event: u32,
        hwnd: HWND,
        _object_id: i32,
        _child_id: i32,
        _thread_id: u32,
        _event_time: u32,
    ) {
        if event != EVENT_SYSTEM_FOREGROUND || hwnd.is_invalid() {
            return;
        }
        CALLBACK_STATE.with(|slot| {
            if let Some(shared) = slot.borrow().as_ref() {
                handle_foreground_window(shared, hwnd);
            }
        });
    }

    fn handle_foreground_window(shared: &Arc<Mutex<Option<NativeState>>>, hwnd: HWND) {
        let Ok(mut shared) = shared.lock() else {
            return;
        };
        let Some(state) = shared.as_mut() else {
            return;
        };
        let descriptor = describe_window(hwnd);
        let managed_window = should_restore_focus(&state.target, descriptor.as_ref(), true);
        if !managed_window {
            state.last_safe_window = Some(hwnd.0 as isize);
            return;
        }
        state.status.managed_window_found = true;
        if !state.status.locked {
            return;
        }
        let destination = state
            .last_safe_window
            .or(state.anchor)
            .map(|value| HWND(value as *mut _))
            .filter(|window| unsafe { IsWindow(Some(*window)) }.as_bool());
        let Some(destination) = destination else {
            state.status.last_error =
                Some("no valid MultiSeat Lite focus anchor is available".to_owned());
            return;
        };
        if unsafe { SetForegroundWindow(destination) }.as_bool() {
            state.status.restoration_count += 1;
            state.status.last_error = None;
        } else {
            state.status.last_error =
                Some("SetForegroundWindow could not restore host focus".to_owned());
        }
    }

    fn describe_window(hwnd: HWND) -> Option<WindowDescriptor> {
        let mut process_id = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
        if process_id == 0 {
            return None;
        }
        let executable_name = process_name(process_id)?;
        let title_length = unsafe { GetWindowTextLengthW(hwnd) };
        let mut title = vec![0_u16; usize::try_from(title_length).ok()?.saturating_add(1)];
        let copied = unsafe { GetWindowTextW(hwnd, &mut title) };
        title.truncate(usize::try_from(copied).ok()?);
        Some(WindowDescriptor {
            process_id,
            executable_name,
            title: String::from_utf16_lossy(&title),
        })
    }

    fn process_name(process_id: u32) -> Option<String> {
        let process =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }.ok()?;
        struct OwnedHandle(HANDLE);
        impl Drop for OwnedHandle {
            fn drop(&mut self) {
                let _ = unsafe { CloseHandle(self.0) };
            }
        }
        let process = OwnedHandle(process);
        let mut buffer = vec![0_u16; 32_768];
        let mut length = u32::try_from(buffer.len()).ok()?;
        unsafe {
            QueryFullProcessImageNameW(
                process.0,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        }
        .ok()?;
        buffer.truncate(usize::try_from(length).ok()?);
        Path::new(&String::from_utf16_lossy(&buffer))
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn target() -> ManagedVmWindowIdentity {
            ManagedVmWindowIdentity {
                vm_uuid: "managed-uuid".into(),
                vm_name: "Windows - Barnen".into(),
                session_pid: Some(42),
            }
        }

        fn managed_window() -> WindowDescriptor {
            WindowDescriptor {
                process_id: 42,
                executable_name: "VirtualBoxVM.exe".into(),
                title: "Windows - Barnen [Running] - Oracle VirtualBox".into(),
            }
        }

        #[test]
        fn managed_focus_triggers_restoration_but_unrelated_focus_does_not() {
            assert!(should_restore_focus(
                &target(),
                Some(&managed_window()),
                true
            ));
            assert!(!should_restore_focus(
                &target(),
                Some(&WindowDescriptor {
                    process_id: 43,
                    ..managed_window()
                }),
                true
            ));
            assert!(!should_restore_focus(
                &target(),
                Some(&WindowDescriptor {
                    executable_name: "notepad.exe".into(),
                    ..managed_window()
                }),
                true
            ));
        }

        #[test]
        fn admin_unlock_suppresses_focus_restoration() {
            assert!(!should_restore_focus(
                &target(),
                Some(&managed_window()),
                false
            ));
        }

        #[test]
        fn title_fallback_is_limited_to_named_virtualboxvm_window() {
            let mut target = target();
            target.session_pid = None;
            assert!(should_restore_focus(&target, Some(&managed_window()), true));
        }
    }

    pub fn backend() -> Box<dyn FocusProtectionBackend> {
        Box::new(WindowsFocusProtection::default())
    }
}

#[cfg(target_os = "windows")]
pub fn native_backend() -> Box<dyn FocusProtectionBackend> {
    windows_backend::backend()
}

#[cfg(not(target_os = "windows"))]
pub fn native_backend() -> Box<dyn FocusProtectionBackend> {
    Box::new(SimulatedFocusProtection::default())
}
