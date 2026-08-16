use std::sync::Mutex;

use serde::Serialize;

use crate::displays::{DisplayBounds, DisplayDevice};

use super::focus_protection::ManagedVmWindowIdentity;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PresentationState {
    #[default]
    NotPresented,
    WaitingForVmWindow,
    ResolvingDisplay,
    Presenting,
    Presented,
    PresentationUnavailable,
    Error,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PresentationMode {
    #[default]
    Borderless,
    ConventionalWindow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PresentationFailureReason {
    NoMatchingVmWindow,
    ManagedVmPidUnknown,
    DisplayNotResolved,
    WindowStateReadFailed,
    SetWindowStyleFailed,
    SetWindowPosFailed,
    PostconditionVerificationFailed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowCandidateDiagnostic {
    pub process_id: u32,
    pub window_handle: isize,
    pub class_name: String,
    pub title: String,
    pub visible: bool,
    pub owner_window_handle: Option<isize>,
    pub bounds: Option<DisplayBounds>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationTraceEntry {
    pub stage: String,
    pub timestamp_ms: u64,
    pub elapsed_ms: u64,
    pub duration_ms: Option<u64>,
    pub detail: Option<String>,
    pub process_id: Option<u32>,
    pub window_handle: Option<isize>,
    pub target_display_id: Option<String>,
    pub target_bounds: Option<DisplayBounds>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayPresentationStatus {
    pub state: PresentationState,
    pub mode: PresentationMode,
    pub assigned_display_id: Option<String>,
    pub display_name: Option<String>,
    pub bounds: Option<DisplayBounds>,
    pub managed_window_found: bool,
    pub managed_window_process_id: Option<u32>,
    pub failure_reason: Option<PresentationFailureReason>,
    pub window_candidates: Vec<WindowCandidateDiagnostic>,
    pub trace: Vec<PresentationTraceEntry>,
    pub last_error: Option<String>,
}

pub trait PresentationBackend: Send + Sync {
    fn start(
        &self,
        target: ManagedVmWindowIdentity,
        assigned_display_id: String,
    ) -> Result<(), String>;
    fn set_locked(&self, locked: bool) -> Result<(), String>;
    fn mark_unavailable(
        &self,
        assigned_display_id: Option<String>,
        message: String,
    ) -> Result<(), String>;
    fn stop(&self) -> Result<(), String>;
    fn status(&self) -> DisplayPresentationStatus;
}

pub fn resolve_assigned_display<'a>(
    displays: &'a [DisplayDevice],
    assigned_display_id: &str,
) -> Option<&'a DisplayDevice> {
    displays.iter().find(|display| {
        display
            .pnp_instance_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case(assigned_display_id))
            || display.id.eq_ignore_ascii_case(assigned_display_id)
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WindowDescriptor {
    handle: isize,
    process_id: u32,
    executable_name: String,
    title: String,
    class_name: String,
    visible: bool,
    owner_handle: Option<isize>,
    bounds: Option<DisplayBounds>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BorderlessPlacement {
    handle: isize,
    bounds: DisplayBounds,
    original_style: isize,
    borderless_style: isize,
}

fn borderless_placement(
    handle: isize,
    bounds: DisplayBounds,
    original_style: isize,
    chrome_mask: isize,
) -> BorderlessPlacement {
    BorderlessPlacement {
        handle,
        bounds,
        original_style,
        borderless_style: original_style & !chrome_mask,
    }
}

fn classify_operation_result(
    result: Result<(), String>,
    reason: PresentationFailureReason,
) -> Result<(), (PresentationFailureReason, String)> {
    result.map_err(|error| (reason, error))
}

fn borderless_postconditions_match(
    actual_style: isize,
    expected_style: isize,
    chrome_mask: isize,
    actual_bounds: DisplayBounds,
    expected_bounds: DisplayBounds,
    monitor_matches: bool,
) -> bool {
    actual_style & chrome_mask == 0
        && actual_style == expected_style
        && actual_bounds == expected_bounds
        && monitor_matches
}

fn select_managed_window<'a>(
    target: &ManagedVmWindowIdentity,
    windows: &'a [WindowDescriptor],
) -> Option<&'a WindowDescriptor> {
    let candidates = windows.iter().filter(|window| {
        window.visible
            && window
                .executable_name
                .eq_ignore_ascii_case("VirtualBoxVM.exe")
    });
    if let Some(process_id) = target.session_pid {
        if let Some(exact) = candidates
            .clone()
            .filter(|window| window.process_id == process_id)
            .max_by_key(|window| window_selection_score(window, &target.vm_name))
        {
            return Some(exact);
        }
    }
    let vm_name = target.vm_name.trim();
    if vm_name.is_empty() {
        return None;
    }
    let matches: Vec<_> = candidates
        .filter(|window| title_matches(&window.title, vm_name))
        .collect();
    let mut process_ids = matches.iter().map(|window| window.process_id);
    let first_process = process_ids.next()?;
    if process_ids.any(|process_id| process_id != first_process) {
        return None;
    }
    matches
        .into_iter()
        .max_by_key(|window| window_selection_score(window, vm_name))
}

fn title_matches(title: &str, vm_name: &str) -> bool {
    title
        .to_ascii_lowercase()
        .contains(&vm_name.to_ascii_lowercase())
}

fn window_selection_score(window: &WindowDescriptor, vm_name: &str) -> u8 {
    u8::from(window.owner_handle.is_none()) * 4
        + u8::from(title_matches(&window.title, vm_name)) * 2
        + u8::from(!window.class_name.is_empty())
}

#[derive(Default)]
pub struct SimulatedPresentation {
    status: Mutex<DisplayPresentationStatus>,
}

impl PresentationBackend for SimulatedPresentation {
    fn start(
        &self,
        target: ManagedVmWindowIdentity,
        assigned_display_id: String,
    ) -> Result<(), String> {
        *self
            .status
            .lock()
            .map_err(|_| "presentation status lock is poisoned")? = DisplayPresentationStatus {
            state: PresentationState::Presented,
            mode: PresentationMode::Borderless,
            assigned_display_id: Some(assigned_display_id),
            managed_window_found: true,
            managed_window_process_id: target.session_pid,
            ..DisplayPresentationStatus::default()
        };
        Ok(())
    }

    fn set_locked(&self, locked: bool) -> Result<(), String> {
        let mut status = self
            .status
            .lock()
            .map_err(|_| "presentation status lock is poisoned")?;
        status.state = if locked {
            PresentationState::Presented
        } else {
            PresentationState::NotPresented
        };
        status.mode = if locked {
            PresentationMode::Borderless
        } else {
            PresentationMode::ConventionalWindow
        };
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        *self
            .status
            .lock()
            .map_err(|_| "presentation status lock is poisoned")? =
            DisplayPresentationStatus::default();
        Ok(())
    }

    fn mark_unavailable(
        &self,
        assigned_display_id: Option<String>,
        message: String,
    ) -> Result<(), String> {
        *self
            .status
            .lock()
            .map_err(|_| "presentation status lock is poisoned")? = DisplayPresentationStatus {
            state: PresentationState::PresentationUnavailable,
            assigned_display_id,
            last_error: Some(message),
            ..DisplayPresentationStatus::default()
        };
        Ok(())
    }

    fn status(&self) -> DisplayPresentationStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_else(|_| DisplayPresentationStatus {
                state: PresentationState::Error,
                last_error: Some("presentation status lock is poisoned".to_owned()),
                ..DisplayPresentationStatus::default()
            })
    }
}

#[cfg(test)]
impl SimulatedPresentation {
    fn simulate_display_topology(&self, bounds: Option<DisplayBounds>) {
        let mut status = self.status.lock().unwrap();
        status.bounds = bounds;
        status.state = if bounds.is_some() {
            PresentationState::Presented
        } else {
            PresentationState::PresentationUnavailable
        };
    }
}

#[cfg(target_os = "windows")]
mod windows_backend {
    use std::{
        path::Path,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        thread::{self, JoinHandle},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    use windows::{
        core::{BOOL, PWSTR},
        Win32::{
            Foundation::{
                CloseHandle, GetLastError, SetLastError, HANDLE, HWND, LPARAM, RECT, WIN32_ERROR,
            },
            Graphics::Gdi::{MonitorFromRect, MonitorFromWindow, MONITOR_DEFAULTTONULL},
            System::Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
            UI::WindowsAndMessaging::{
                EnumWindows, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowPlacement,
                GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
                IsWindow, IsWindowVisible, SetWindowLongPtrW, SetWindowPlacement, SetWindowPos,
                GWL_EXSTYLE, GWL_STYLE, GW_OWNER, HWND_TOP, SWP_FRAMECHANGED, SWP_NOACTIVATE,
                SWP_SHOWWINDOW, WINDOWPLACEMENT, WINDOW_LONG_PTR_INDEX, WS_CAPTION, WS_MAXIMIZEBOX,
                WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
            },
        },
    };

    use crate::displays;

    use super::{
        resolve_assigned_display, DisplayPresentationStatus, ManagedVmWindowIdentity,
        PresentationBackend, PresentationFailureReason, PresentationMode, PresentationState,
        PresentationTraceEntry, WindowCandidateDiagnostic, WindowDescriptor,
    };

    #[derive(Clone, Copy, Debug)]
    struct OriginalWindow {
        handle: isize,
        style: isize,
        extended_style: isize,
        bounds: RECT,
        placement: WINDOWPLACEMENT,
    }

    struct SharedState {
        target: ManagedVmWindowIdentity,
        assigned_display_id: String,
        locked: bool,
        status: DisplayPresentationStatus,
        original: Option<OriginalWindow>,
        started_at: Instant,
    }

    struct Worker {
        cancel: Arc<AtomicBool>,
        handle: JoinHandle<()>,
    }

    #[derive(Default)]
    pub struct WindowsPresentation {
        shared: Arc<Mutex<Option<SharedState>>>,
        worker: Mutex<Option<Worker>>,
    }

    impl PresentationBackend for WindowsPresentation {
        fn start(
            &self,
            target: ManagedVmWindowIdentity,
            assigned_display_id: String,
        ) -> Result<(), String> {
            self.stop()?;
            *self
                .shared
                .lock()
                .map_err(|_| "presentation state lock is poisoned")? = Some(SharedState {
                target,
                assigned_display_id: assigned_display_id.clone(),
                locked: true,
                status: DisplayPresentationStatus {
                    state: PresentationState::WaitingForVmWindow,
                    mode: PresentationMode::Borderless,
                    assigned_display_id: Some(assigned_display_id),
                    ..DisplayPresentationStatus::default()
                },
                original: None,
                started_at: Instant::now(),
            });
            let cancel = Arc::new(AtomicBool::new(false));
            let thread_cancel = cancel.clone();
            let shared = self.shared.clone();
            let handle = thread::Builder::new()
                .name("multiseat-vm-presentation".to_owned())
                .spawn(move || presentation_loop(shared, thread_cancel))
                .map_err(|error| format!("could not start presentation worker: {error}"))?;
            *self
                .worker
                .lock()
                .map_err(|_| "presentation worker lock is poisoned")? =
                Some(Worker { cancel, handle });
            Ok(())
        }

        fn set_locked(&self, locked: bool) -> Result<(), String> {
            let mut shared = self
                .shared
                .lock()
                .map_err(|_| "presentation state lock is poisoned")?;
            let state = shared
                .as_mut()
                .ok_or_else(|| "presentation is not initialized".to_owned())?;
            state.locked = locked;
            if !locked {
                restore_original(state);
                state.status.state = PresentationState::NotPresented;
                state.status.mode = PresentationMode::ConventionalWindow;
            } else {
                state.status.state = PresentationState::ResolvingDisplay;
                state.status.mode = PresentationMode::Borderless;
            }
            Ok(())
        }

        fn stop(&self) -> Result<(), String> {
            if let Some(worker) = self
                .worker
                .lock()
                .map_err(|_| "presentation worker lock is poisoned")?
                .take()
            {
                worker.cancel.store(true, Ordering::Release);
                worker
                    .handle
                    .join()
                    .map_err(|_| "presentation worker panicked".to_owned())?;
            }
            let mut shared = self
                .shared
                .lock()
                .map_err(|_| "presentation state lock is poisoned")?;
            if let Some(state) = shared.as_mut() {
                restore_original(state);
            }
            *shared = None;
            Ok(())
        }

        fn mark_unavailable(
            &self,
            assigned_display_id: Option<String>,
            message: String,
        ) -> Result<(), String> {
            self.stop()?;
            *self
                .shared
                .lock()
                .map_err(|_| "presentation state lock is poisoned")? = Some(SharedState {
                target: ManagedVmWindowIdentity {
                    vm_uuid: String::new(),
                    vm_name: String::new(),
                    session_pid: None,
                },
                assigned_display_id: assigned_display_id.clone().unwrap_or_default(),
                locked: false,
                status: DisplayPresentationStatus {
                    state: PresentationState::PresentationUnavailable,
                    assigned_display_id,
                    last_error: Some(message),
                    ..DisplayPresentationStatus::default()
                },
                original: None,
                started_at: Instant::now(),
            });
            Ok(())
        }

        fn status(&self) -> DisplayPresentationStatus {
            self.shared
                .lock()
                .ok()
                .and_then(|state| state.as_ref().map(|state| state.status.clone()))
                .unwrap_or_default()
        }
    }

    impl Drop for WindowsPresentation {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }

    fn presentation_loop(shared: Arc<Mutex<Option<SharedState>>>, cancel: Arc<AtomicBool>) {
        let window_deadline = Instant::now() + Duration::from_secs(20);
        let mut managed_window_seen = false;
        while !cancel.load(Ordering::Acquire) {
            if let Ok(mut guard) = shared.lock() {
                if let Some(state) = guard.as_mut() {
                    update_presentation(state, window_deadline);
                    managed_window_seen |= state.status.managed_window_found;
                    if !managed_window_seen && Instant::now() >= window_deadline {
                        return;
                    }
                } else {
                    return;
                }
            }
            let interval = if Instant::now() < window_deadline {
                Duration::from_millis(250)
            } else {
                Duration::from_secs(2)
            };
            let until = Instant::now() + interval;
            while Instant::now() < until {
                if cancel.load(Ordering::Acquire) {
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }

    fn update_presentation(state: &mut SharedState, window_deadline: Instant) {
        if !state.locked {
            return;
        }
        let prior_state = state.status.state;
        let prior_window_found = state.status.managed_window_found;
        let prior_bounds = state.status.bounds;
        let windows = enumerate_windows();
        state.status.window_candidates = windows
            .iter()
            .filter(|window| {
                window
                    .executable_name
                    .eq_ignore_ascii_case("VirtualBoxVM.exe")
            })
            .take(32)
            .map(|window| WindowCandidateDiagnostic {
                process_id: window.process_id,
                window_handle: window.handle,
                class_name: window.class_name.clone(),
                title: window.title.clone(),
                visible: window.visible,
                owner_window_handle: window.owner_handle,
                bounds: window.bounds,
            })
            .collect();
        if prior_state != PresentationState::Presented
            && prior_state != PresentationState::PresentationUnavailable
        {
            push_trace(
                state,
                "WindowDiscoveryAttempt",
                Some(format!(
                    "{} VirtualBoxVM.exe top-level candidate(s)",
                    state.status.window_candidates.len()
                )),
                None,
                None,
                None,
            );
        }
        let selected = super::select_managed_window(&state.target, &windows).cloned();
        let Some(window) = selected else {
            state.status.managed_window_found = false;
            state.status.managed_window_process_id = None;
            state.status.state = if Instant::now() < window_deadline {
                PresentationState::WaitingForVmWindow
            } else {
                PresentationState::PresentationUnavailable
            };
            state.status.failure_reason = (Instant::now() >= window_deadline).then_some(
                if state.target.session_pid.is_none() {
                    PresentationFailureReason::ManagedVmPidUnknown
                } else {
                    PresentationFailureReason::NoMatchingVmWindow
                },
            );
            state.status.last_error = (Instant::now() >= window_deadline).then(|| {
                "managed VirtualBoxVM window was not found within 20 seconds; VM remains running"
                    .to_owned()
            });
            if prior_state != PresentationState::PresentationUnavailable {
                if let Some(error) = state.status.last_error.clone() {
                    push_error_trace(state, "PresentationUnavailable", error, None, None, None);
                }
            }
            return;
        };
        state.status.managed_window_found = true;
        state.status.managed_window_process_id = Some(window.process_id);
        state.status.failure_reason = None;
        if !prior_window_found {
            push_trace(
                state,
                "VmWindowResolved",
                Some(format!(
                    "class='{}', title='{}', owner={:?}",
                    window.class_name, window.title, window.owner_handle
                )),
                Some(window.process_id),
                Some(window.handle),
                None,
            );
        }
        state.status.state = PresentationState::ResolvingDisplay;

        let displays = match displays::enumerate_session_visible() {
            Ok(displays) => displays,
            Err(error) => {
                state.status.state = PresentationState::Error;
                state.status.failure_reason = Some(PresentationFailureReason::DisplayNotResolved);
                state.status.last_error = Some(error);
                return;
            }
        };
        let Some(display) = resolve_assigned_display(&displays, &state.assigned_display_id) else {
            state.status.state = PresentationState::PresentationUnavailable;
            state.status.failure_reason = Some(PresentationFailureReason::DisplayNotResolved);
            state.status.display_name = None;
            state.status.bounds = None;
            state.status.last_error = Some(
                "assigned physical display is not visible to the current Windows session; VM remains running"
                    .to_owned(),
            );
            if prior_state != PresentationState::PresentationUnavailable {
                push_error_trace(
                    state,
                    "DisplayNotResolved",
                    state.status.last_error.clone().unwrap_or_default(),
                    Some(window.process_id),
                    Some(window.handle),
                    None,
                );
            }
            return;
        };
        let Some(bounds) = display.bounds else {
            state.status.state = PresentationState::PresentationUnavailable;
            state.status.failure_reason = Some(PresentationFailureReason::DisplayNotResolved);
            state.status.display_name = display.friendly_name.clone();
            state.status.bounds = None;
            state.status.last_error =
                Some("assigned display has no current HMONITOR bounds".to_owned());
            return;
        };
        state.status.display_name = display
            .friendly_name
            .clone()
            .or_else(|| Some(display.desktop_device_name.clone()));
        state.status.bounds = Some(bounds);
        let same_window = state
            .original
            .is_some_and(|original| original.handle == window.handle);
        if prior_state == PresentationState::Presented
            && prior_bounds == Some(bounds)
            && same_window
        {
            state.status.state = PresentationState::Presented;
            state.status.mode = PresentationMode::Borderless;
            state.status.last_error = None;
            state.status.failure_reason = None;
            return;
        }
        push_trace(
            state,
            "DisplayResolved",
            display.friendly_name.clone(),
            Some(window.process_id),
            Some(window.handle),
            Some(bounds),
        );
        state.status.state = PresentationState::Presenting;
        match apply_borderless(state, window.handle, bounds) {
            Ok(()) => {
                state.status.state = PresentationState::Presented;
                state.status.mode = PresentationMode::Borderless;
                state.status.last_error = None;
                state.status.failure_reason = None;
                push_trace(
                    state,
                    "Presented",
                    None,
                    Some(window.process_id),
                    Some(window.handle),
                    Some(bounds),
                );
            }
            Err((reason, error)) => {
                state.status.state = PresentationState::Error;
                state.status.failure_reason = Some(reason);
                state.status.last_error = Some(error.clone());
                push_error_trace(
                    state,
                    "PresentationFailed",
                    error,
                    Some(window.process_id),
                    Some(window.handle),
                    Some(bounds),
                );
            }
        }
    }

    fn apply_borderless(
        state: &mut SharedState,
        handle: isize,
        bounds: crate::displays::DisplayBounds,
    ) -> Result<(), (PresentationFailureReason, String)> {
        let hwnd = HWND(handle as *mut _);
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return Err((
                PresentationFailureReason::WindowStateReadFailed,
                "managed VirtualBoxVM window is no longer valid".to_owned(),
            ));
        }
        if state.original.is_none() || state.original.is_some_and(|value| value.handle != handle) {
            let mut original_bounds = RECT::default();
            unsafe { GetWindowRect(hwnd, &mut original_bounds) }.map_err(|error| {
                (
                    PresentationFailureReason::WindowStateReadFailed,
                    format!("GetWindowRect failed: {error}"),
                )
            })?;
            let mut original_placement = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            unsafe { GetWindowPlacement(hwnd, &mut original_placement) }.map_err(|error| {
                (
                    PresentationFailureReason::WindowStateReadFailed,
                    format!("GetWindowPlacement failed: {error}"),
                )
            })?;
            let original_style = get_window_long_ptr(hwnd, GWL_STYLE, "GWL_STYLE")
                .map_err(|error| (PresentationFailureReason::WindowStateReadFailed, error))?;
            let original_extended_style = get_window_long_ptr(hwnd, GWL_EXSTYLE, "GWL_EXSTYLE")
                .map_err(|error| (PresentationFailureReason::WindowStateReadFailed, error))?;
            state.original = Some(OriginalWindow {
                handle,
                style: original_style,
                extended_style: original_extended_style,
                bounds: original_bounds,
                placement: original_placement,
            });
        }
        let Some(original) = state.original else {
            return Err((
                PresentationFailureReason::WindowStateReadFailed,
                "could not preserve the original managed window state".to_owned(),
            ));
        };
        let chrome =
            (WS_CAPTION | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU).0 as isize;
        let placement = super::borderless_placement(handle, bounds, original.style, chrome);
        push_trace(state, "StyleRead", None, None, Some(handle), Some(bounds));
        super::classify_operation_result(
            set_window_long_ptr(hwnd, GWL_STYLE, placement.borderless_style, "GWL_STYLE"),
            PresentationFailureReason::SetWindowStyleFailed,
        )?;
        push_trace(
            state,
            "BorderlessApplied",
            None,
            None,
            Some(handle),
            Some(bounds),
        );
        unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                placement.bounds.x,
                placement.bounds.y,
                placement.bounds.width,
                placement.bounds.height,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        }
        .map_err(|error| {
            (
                PresentationFailureReason::SetWindowPosFailed,
                format!("SetWindowPos(borderless presentation) failed: {error}"),
            )
        })?;
        push_trace(state, "WindowMoved", None, None, Some(handle), Some(bounds));
        verify_borderless_postconditions(hwnd, placement.borderless_style, bounds, chrome)
    }

    fn verify_borderless_postconditions(
        hwnd: HWND,
        expected_style: isize,
        expected_bounds: crate::displays::DisplayBounds,
        chrome_mask: isize,
    ) -> Result<(), (PresentationFailureReason, String)> {
        let actual_style = get_window_long_ptr(hwnd, GWL_STYLE, "GWL_STYLE postcondition")
            .map_err(|error| {
                (
                    PresentationFailureReason::PostconditionVerificationFailed,
                    error,
                )
            })?;
        let mut actual_rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut actual_rect) }.map_err(|error| {
            (
                PresentationFailureReason::PostconditionVerificationFailed,
                format!("postcondition GetWindowRect failed: {error}"),
            )
        })?;
        let expected_rect = RECT {
            left: expected_bounds.x,
            top: expected_bounds.y,
            right: expected_bounds.x + expected_bounds.width,
            bottom: expected_bounds.y + expected_bounds.height,
        };
        let actual_monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL) };
        let expected_monitor = unsafe { MonitorFromRect(&expected_rect, MONITOR_DEFAULTTONULL) };
        let monitor_valid = !expected_monitor.is_invalid() && actual_monitor == expected_monitor;
        let actual_bounds = crate::displays::DisplayBounds {
            x: actual_rect.left,
            y: actual_rect.top,
            width: actual_rect.right - actual_rect.left,
            height: actual_rect.bottom - actual_rect.top,
        };
        if super::borderless_postconditions_match(
            actual_style,
            expected_style,
            chrome_mask,
            actual_bounds,
            expected_bounds,
            monitor_valid,
        ) {
            Ok(())
        } else {
            Err((
                PresentationFailureReason::PostconditionVerificationFailed,
                format!(
                    "borderless postcondition failed: style={actual_style:#x} expected={expected_style:#x}, rect={actual_rect:?} expected={expected_rect:?}, target_monitor_match={monitor_valid}"
                ),
            ))
        }
    }

    fn set_window_long_ptr(
        hwnd: HWND,
        index: WINDOW_LONG_PTR_INDEX,
        value: isize,
        label: &str,
    ) -> Result<(), String> {
        // SetWindowLongPtr uses zero both as a valid previous value and as its
        // failure sentinel. Clear and inspect the thread's last-error value to
        // distinguish those cases as required by the Win32 contract.
        unsafe { SetLastError(WIN32_ERROR(0)) };
        let previous = unsafe { SetWindowLongPtrW(hwnd, index, value) };
        let error = unsafe { GetLastError() };
        if previous == 0 && error.0 != 0 {
            Err(format!(
                "SetWindowLongPtrW({label}) failed with Windows error {}",
                error.0
            ))
        } else {
            Ok(())
        }
    }

    fn get_window_long_ptr(
        hwnd: HWND,
        index: WINDOW_LONG_PTR_INDEX,
        label: &str,
    ) -> Result<isize, String> {
        unsafe { SetLastError(WIN32_ERROR(0)) };
        let value = unsafe { GetWindowLongPtrW(hwnd, index) };
        let error = unsafe { GetLastError() };
        if value == 0 && error.0 != 0 {
            Err(format!(
                "GetWindowLongPtrW({label}) failed with Windows error {}",
                error.0
            ))
        } else {
            Ok(value)
        }
    }

    fn restore_original(state: &mut SharedState) {
        let Some(original) = state.original.take() else {
            return;
        };
        let hwnd = HWND(original.handle as *mut _);
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return;
        }
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_STYLE, original.style);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, original.extended_style);
        }
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                None,
                original.bounds.left,
                original.bounds.top,
                original.bounds.right - original.bounds.left,
                original.bounds.bottom - original.bounds.top,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        };
        let _ = unsafe { SetWindowPlacement(hwnd, &original.placement) };
    }

    fn push_trace(
        state: &mut SharedState,
        stage: &str,
        detail: Option<String>,
        process_id: Option<u32>,
        window_handle: Option<isize>,
        target_bounds: Option<crate::displays::DisplayBounds>,
    ) {
        if state.status.trace.len() >= 128 {
            state.status.trace.remove(0);
        }
        state.status.trace.push(PresentationTraceEntry {
            stage: stage.to_owned(),
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|value| u64::try_from(value.as_millis()).ok())
                .unwrap_or_default(),
            elapsed_ms: u64::try_from(state.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            duration_ms: None,
            detail,
            process_id,
            window_handle,
            target_display_id: Some(state.assigned_display_id.clone()),
            target_bounds,
            error: None,
        });
    }

    fn push_error_trace(
        state: &mut SharedState,
        stage: &str,
        error: String,
        process_id: Option<u32>,
        window_handle: Option<isize>,
        target_bounds: Option<crate::displays::DisplayBounds>,
    ) {
        push_trace(state, stage, None, process_id, window_handle, target_bounds);
        if let Some(entry) = state.status.trace.last_mut() {
            entry.error = Some(error);
        }
    }

    fn enumerate_windows() -> Vec<WindowDescriptor> {
        let mut result = Vec::new();
        let _ = unsafe {
            EnumWindows(
                Some(collect_window),
                LPARAM((&mut result as *mut Vec<WindowDescriptor>) as isize),
            )
        };
        result
    }

    unsafe extern "system" fn collect_window(hwnd: HWND, data: LPARAM) -> BOOL {
        let result = unsafe { &mut *(data.0 as *mut Vec<WindowDescriptor>) };
        let mut process_id = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
        if process_id != 0 {
            if let Some(executable_name) = process_name(process_id) {
                let title_length = unsafe { GetWindowTextLengthW(hwnd) };
                let mut title = vec![0_u16; usize::try_from(title_length).unwrap_or(0) + 1];
                let copied = unsafe { GetWindowTextW(hwnd, &mut title) };
                title.truncate(usize::try_from(copied).unwrap_or(0));
                let owner_handle = unsafe { GetWindow(hwnd, GW_OWNER) }
                    .ok()
                    .map(|owner| owner.0 as isize);
                let mut class_name = vec![0_u16; 256];
                let class_length = unsafe { GetClassNameW(hwnd, &mut class_name) };
                class_name.truncate(usize::try_from(class_length).unwrap_or(0));
                let mut rect = RECT::default();
                let bounds = unsafe { GetWindowRect(hwnd, &mut rect) }.ok().map(|_| {
                    crate::displays::DisplayBounds {
                        x: rect.left,
                        y: rect.top,
                        width: rect.right - rect.left,
                        height: rect.bottom - rect.top,
                    }
                });
                result.push(WindowDescriptor {
                    handle: hwnd.0 as isize,
                    process_id,
                    executable_name,
                    title: String::from_utf16_lossy(&title),
                    class_name: String::from_utf16_lossy(&class_name),
                    visible: unsafe { IsWindowVisible(hwnd) }.as_bool(),
                    owner_handle,
                    bounds,
                });
            }
        }
        BOOL(1)
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

    pub fn backend() -> Box<dyn PresentationBackend> {
        Box::new(WindowsPresentation::default())
    }
}

#[cfg(target_os = "windows")]
pub fn native_backend() -> Box<dyn PresentationBackend> {
    windows_backend::backend()
}

#[cfg(not(target_os = "windows"))]
pub fn native_backend() -> Box<dyn PresentationBackend> {
    Box::new(SimulatedPresentation::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(id: &str, bounds: DisplayBounds) -> DisplayDevice {
        DisplayDevice {
            id: id.into(),
            pnp_instance_id: Some(id.into()),
            device_name: format!("{id}\\Monitor0"),
            desktop_device_name: format!("\\\\.\\{id}"),
            friendly_name: Some(id.into()),
            device_path: None,
            adapter_name: "Adapter".into(),
            adapter_device_id: None,
            primary: id == "Dennis",
            bounds: Some(bounds),
        }
    }

    fn target(pid: Option<u32>) -> ManagedVmWindowIdentity {
        ManagedVmWindowIdentity {
            vm_uuid: "managed-uuid".into(),
            vm_name: "Windows - Barnen".into(),
            session_pid: pid,
        }
    }

    fn window(pid: u32, executable_name: &str, title: &str) -> WindowDescriptor {
        WindowDescriptor {
            handle: pid as isize,
            process_id: pid,
            executable_name: executable_name.into(),
            title: title.into(),
            class_name: "Qt663QWindowIcon".into(),
            visible: true,
            owner_handle: None,
            bounds: None,
        }
    }

    #[test]
    fn stable_identity_resolves_negative_coordinates_and_distinct_resolutions() {
        let mut barnet = display(
            "Barnen",
            DisplayBounds {
                x: -2560,
                y: -1440,
                width: 2560,
                height: 1440,
            },
        );
        barnet.id = r"MONITOR\BNQ7840\CLASS\0001".into();
        barnet.pnp_instance_id = Some(r"DISPLAY\BNQ7840\INSTANCE&UID41217".into());
        let displays = vec![
            display(
                "Dennis",
                DisplayBounds {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
            ),
            barnet,
        ];
        let resolved =
            resolve_assigned_display(&displays, r"display\bnq7840\instance&uid41217").unwrap();
        assert_eq!(resolved.bounds.unwrap().x, -2560);
        assert_eq!(resolved.bounds.unwrap().y, -1440);
        assert_eq!(resolved.bounds.unwrap().width, 2560);
        assert_eq!(resolved.bounds.unwrap().height, 1440);
    }

    #[test]
    fn pid_and_executable_select_only_the_managed_vm_window() {
        let windows = vec![
            window(41, "VirtualBox.exe", "VirtualBox Manager"),
            window(42, "VirtualBoxVM.exe", "Windows - Barnen [Running]"),
            window(43, "VirtualBoxVM.exe", "Unrelated VM [Running]"),
        ];
        assert_eq!(
            select_managed_window(&target(Some(42)), &windows)
                .unwrap()
                .process_id,
            42
        );
        assert_eq!(
            select_managed_window(&target(Some(99)), &windows)
                .unwrap()
                .process_id,
            42,
            "a stale SessionPID may fall back only to the uniquely named managed VirtualBoxVM process"
        );
        let unrelated = ManagedVmWindowIdentity {
            vm_uuid: "other".into(),
            vm_name: "Missing managed VM".into(),
            session_pid: Some(99),
        };
        assert!(select_managed_window(&unrelated, &windows).is_none());
    }

    #[test]
    fn title_fallback_is_bounded_to_virtualboxvm_when_pid_is_unavailable() {
        let windows = vec![
            window(41, "notepad.exe", "Windows - Barnen"),
            window(42, "VirtualBoxVM.exe", "Windows - Barnen [Running]"),
        ];
        assert_eq!(
            select_managed_window(&target(None), &windows)
                .unwrap()
                .process_id,
            42
        );
        assert_eq!(
            select_managed_window(&target(Some(42)), &windows)
                .unwrap()
                .process_id,
            42,
            "the same candidate remains selected when SessionPID becomes available"
        );
    }

    #[test]
    fn owned_virtualbox_window_is_allowed_only_with_strong_managed_identity() {
        let mut managed = window(42, "VirtualBoxVM.exe", "Windows - Barnen [Running]");
        managed.owner_handle = Some(900);
        let unrelated = window(43, "VirtualBoxVM.exe", "Other VM [Running]");
        assert_eq!(
            select_managed_window(&target(Some(42)), &[managed.clone(), unrelated.clone()])
                .unwrap()
                .handle,
            managed.handle
        );
        assert!(select_managed_window(
            &ManagedVmWindowIdentity {
                vm_uuid: "missing".into(),
                vm_name: "Missing VM".into(),
                session_pid: None,
            },
            &[managed, unrelated],
        )
        .is_none());
    }

    #[test]
    fn simulated_admin_unlock_and_relock_restore_presentation_state() {
        let backend = SimulatedPresentation::default();
        backend.start(target(Some(42)), "Barnen".into()).unwrap();
        assert_eq!(backend.status().state, PresentationState::Presented);
        backend.set_locked(false).unwrap();
        assert_eq!(backend.status().mode, PresentationMode::ConventionalWindow);
        backend.set_locked(true).unwrap();
        assert_eq!(backend.status().state, PresentationState::Presented);
        assert_eq!(backend.status().mode, PresentationMode::Borderless);
        backend.stop().unwrap();
        assert_eq!(backend.status().state, PresentationState::NotPresented);
    }

    #[test]
    fn missing_assigned_display_never_falls_back_to_dennis() {
        let displays = vec![display(
            "Dennis",
            DisplayBounds {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
        )];
        assert!(resolve_assigned_display(&displays, "Barnen").is_none());
    }

    #[test]
    fn disconnect_becomes_unavailable_and_same_display_can_recover() {
        let backend = SimulatedPresentation::default();
        backend.start(target(Some(42)), "Barnen".into()).unwrap();
        backend.simulate_display_topology(None);
        assert_eq!(
            backend.status().state,
            PresentationState::PresentationUnavailable
        );
        let returned = DisplayBounds {
            x: 1920,
            y: 0,
            width: 1600,
            height: 900,
        };
        backend.simulate_display_topology(Some(returned));
        assert_eq!(backend.status().state, PresentationState::Presented);
        assert_eq!(backend.status().bounds, Some(returned));
    }

    #[test]
    fn stop_cancels_pending_window_discovery_state() {
        let backend = SimulatedPresentation::default();
        *backend.status.lock().unwrap() = DisplayPresentationStatus {
            state: PresentationState::WaitingForVmWindow,
            assigned_display_id: Some("Barnen".into()),
            ..DisplayPresentationStatus::default()
        };
        backend.stop().unwrap();
        assert_eq!(backend.status(), DisplayPresentationStatus::default());
    }

    #[test]
    fn borderless_plan_uses_exact_bounds_and_preserves_original_style() {
        let bounds = DisplayBounds {
            x: -1920,
            y: 120,
            width: 1920,
            height: 1080,
        };
        let plan = borderless_placement(42, bounds, 0b1111, 0b0110);
        assert_eq!(plan.handle, 42);
        assert_eq!(plan.bounds, bounds);
        assert_eq!(plan.original_style, 0b1111);
        assert_eq!(plan.borderless_style, 0b1001);
    }

    #[test]
    fn window_operation_failures_remain_typed() {
        assert_eq!(
            classify_operation_result(
                Err("access denied".into()),
                PresentationFailureReason::SetWindowStyleFailed,
            )
            .unwrap_err(),
            (
                PresentationFailureReason::SetWindowStyleFailed,
                "access denied".into()
            )
        );
        assert_eq!(
            classify_operation_result(
                Err("invalid HWND".into()),
                PresentationFailureReason::SetWindowPosFailed,
            )
            .unwrap_err()
            .0,
            PresentationFailureReason::SetWindowPosFailed
        );
    }

    #[test]
    fn postcondition_requires_style_bounds_and_target_monitor() {
        let expected = DisplayBounds {
            x: 1920,
            y: 9,
            width: 1920,
            height: 1080,
        };
        assert!(borderless_postconditions_match(
            0b1001, 0b1001, 0b0110, expected, expected, true
        ));
        assert!(!borderless_postconditions_match(
            0b1111, 0b1001, 0b0110, expected, expected, true
        ));
        assert!(!borderless_postconditions_match(
            0b1001,
            0b1001,
            0b0110,
            DisplayBounds { x: 0, ..expected },
            expected,
            true,
        ));
        assert!(!borderless_postconditions_match(
            0b1001, 0b1001, 0b0110, expected, expected, false
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn native_worker_start_and_status_are_non_blocking_and_cancellable() {
        use std::time::{Duration, Instant};

        let backend = native_backend();
        let started = Instant::now();
        backend
            .start(target(Some(u32::MAX)), "missing-display".into())
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        let status_started = Instant::now();
        let _ = backend.status();
        assert!(status_started.elapsed() < Duration::from_secs(1));
        backend.stop().unwrap();
        assert_eq!(backend.status().state, PresentationState::NotPresented);
    }
}
