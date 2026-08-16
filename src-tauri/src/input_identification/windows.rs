use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    mem::size_of,
    sync::{mpsc, Mutex, OnceLock},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use tauri::Emitter;
use windows::{
    core::w,
    Win32::{
        Foundation::{
            GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
        },
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{
            Input::{
                GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE,
                RIDEV_DEVNOTIFY, RIDEV_INPUTSINK, RIDEV_REMOVE, RID_INPUT, RIM_TYPEKEYBOARD,
                RIM_TYPEMOUSE,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
                PostQuitMessage, PostThreadMessageW, RegisterClassW, TranslateMessage,
                GIDC_REMOVAL, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY,
                WM_INPUT, WM_INPUT_DEVICE_CHANGE, WM_QUIT, WNDCLASSW,
            },
        },
    },
};

use super::{IdentificationStatus, IdentifiedInputDevice};
use crate::{
    devices, environment,
    keyboard_routing::{
        normalize_raw_keyboard_event, GuestKeyboardSink, KeyboardDiagnosticAction, KeyboardRouter,
        KeyboardRoutingDiagnosticEvent, KeyboardRoutingDiagnosticStatus, KeyboardRoutingTransport,
        NormalizedKeyEvent,
    },
};

const EVENT_NAME: &str = "input-device-identified";
const ROUTING_EVENT_NAME: &str = "keyboard-routing-diagnostic";
const EVENT_DEBOUNCE: Duration = Duration::from_millis(900);
const API_ERROR: u32 = u32::MAX;

thread_local! {
    static LISTENER_CONTEXT: RefCell<Option<ListenerContext>> = const { RefCell::new(None) };
}

static WINDOW_CLASS_RESULT: OnceLock<Result<(), String>> = OnceLock::new();

#[derive(Default)]
pub struct InputIdentificationService {
    listener: Mutex<Option<ListenerHandle>>,
}

struct ListenerHandle {
    thread_id: u32,
    join: JoinHandle<()>,
    mode: ListenerModeStatus,
}

enum ListenerModeStatus {
    Identification(IdentificationStatus),
    KeyboardRouting(KeyboardRoutingDiagnosticStatus),
}

struct ListenerContext {
    app: tauri::AppHandle,
    correlations: HashMap<String, IdentifiedInputDevice>,
    throttle: EventThrottle,
    keyboard_router: Option<KeyboardRouter<Box<dyn GuestKeyboardSink + Send>>>,
}

struct ReportingKeyboardSink {
    app: tauri::AppHandle,
    inner: Box<dyn GuestKeyboardSink + Send>,
    detail: &'static str,
}

impl GuestKeyboardSink for ReportingKeyboardSink {
    fn key_down(&mut self, event: &NormalizedKeyEvent) -> Result<(), String> {
        self.inner.key_down(event)?;
        self.app
            .emit(
                ROUTING_EVENT_NAME,
                KeyboardRoutingDiagnosticEvent {
                    action: KeyboardDiagnosticAction::KeyDown,
                    event: Some(event.clone()),
                    detail: self.detail.to_owned(),
                },
            )
            .map_err(|error| format!("could not emit keyboard diagnostic: {error}"))
    }

    fn key_up(&mut self, event: &NormalizedKeyEvent) -> Result<(), String> {
        self.inner.key_up(event)?;
        self.app
            .emit(
                ROUTING_EVENT_NAME,
                KeyboardRoutingDiagnosticEvent {
                    action: KeyboardDiagnosticAction::KeyUp,
                    event: Some(event.clone()),
                    detail: self.detail.to_owned(),
                },
            )
            .map_err(|error| format!("could not emit keyboard diagnostic: {error}"))
    }

    fn release_all(&mut self) -> Result<(), String> {
        self.inner.release_all()?;
        self.app
            .emit(
                ROUTING_EVENT_NAME,
                KeyboardRoutingDiagnosticEvent {
                    action: KeyboardDiagnosticAction::ReleaseAll,
                    event: None,
                    detail: "all tracked key state released at the diagnostic backend boundary"
                        .to_owned(),
                },
            )
            .map_err(|error| format!("could not emit keyboard release diagnostic: {error}"))
    }
}

struct DiagnosticKeyboardSink;

impl GuestKeyboardSink for DiagnosticKeyboardSink {
    fn key_down(&mut self, _event: &NormalizedKeyEvent) -> Result<(), String> {
        Ok(())
    }

    fn key_up(&mut self, _event: &NormalizedKeyEvent) -> Result<(), String> {
        Ok(())
    }

    fn release_all(&mut self) -> Result<(), String> {
        Ok(())
    }
}

struct EventThrottle {
    last_source: Option<String>,
    last_emitted: Option<Instant>,
    interval: Duration,
}

impl EventThrottle {
    fn new(interval: Duration) -> Self {
        Self {
            last_source: None,
            last_emitted: None,
            interval,
        }
    }

    fn should_emit(&mut self, source: &str, now: Instant) -> bool {
        let changed = self.last_source.as_deref() != Some(source);
        let elapsed = self
            .last_emitted
            .is_none_or(|last| now.duration_since(last) >= self.interval);
        if changed || elapsed {
            self.last_source = Some(source.to_owned());
            self.last_emitted = Some(now);
            true
        } else {
            false
        }
    }
}

impl InputIdentificationService {
    pub fn start(&self, app: tauri::AppHandle) -> Result<IdentificationStatus, String> {
        let mut listener = self
            .listener
            .lock()
            .map_err(|_| "input identification lock is poisoned".to_owned())?;
        if let Some(existing) = listener.as_ref() {
            return match &existing.mode {
                ListenerModeStatus::Identification(status) => Ok(status.clone()),
                ListenerModeStatus::KeyboardRouting(_) => {
                    Err("keyboard routing diagnostic is already active".to_owned())
                }
            };
        }

        let discovery = devices::enumerate()?;
        let environment = environment::detect(discovery.aster_device_evidence())?;
        let correlations = build_correlations(&discovery, environment.current_session_id);
        let status = IdentificationStatus {
            active: true,
            session_raw_input_devices: discovery.session_raw_input_devices.len(),
            correlated_physical_devices: correlations
                .values()
                .map(|device| device.stable_physical_device_id.to_ascii_lowercase())
                .collect::<HashSet<_>>()
                .len(),
        };

        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("raw-input-identification".to_owned())
            .spawn(move || listener_thread(app, correlations, None, ready_sender))
            .map_err(|error| format!("could not start input identification thread: {error}"))?;
        let thread_id = match ready_receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(thread_id)) => thread_id,
            Ok(Err(error)) => {
                let _ = join.join();
                return Err(error);
            }
            Err(error) => {
                return Err(format!(
                    "input identification listener did not initialize: {error}"
                ))
            }
        };

        *listener = Some(ListenerHandle {
            thread_id,
            join,
            mode: ListenerModeStatus::Identification(status.clone()),
        });
        Ok(status)
    }

    pub fn start_keyboard_routing(
        &self,
        app: tauri::AppHandle,
        target_physical_device_id: String,
    ) -> Result<KeyboardRoutingDiagnosticStatus, String> {
        self.start_keyboard_listener(
            app,
            target_physical_device_id,
            Box::new(DiagnosticKeyboardSink),
            KeyboardRoutingTransport::DiagnosticOnly,
        )
    }

    pub fn start_keyboard_injection(
        &self,
        app: tauri::AppHandle,
        target_physical_device_id: String,
        sink: Box<dyn GuestKeyboardSink + Send>,
    ) -> Result<KeyboardRoutingDiagnosticStatus, String> {
        self.start_keyboard_listener(
            app,
            target_physical_device_id,
            sink,
            KeyboardRoutingTransport::VirtualBoxScancodePrototype,
        )
    }

    fn start_keyboard_listener(
        &self,
        app: tauri::AppHandle,
        target_physical_device_id: String,
        sink: Box<dyn GuestKeyboardSink + Send>,
        transport: KeyboardRoutingTransport,
    ) -> Result<KeyboardRoutingDiagnosticStatus, String> {
        let mut listener = self
            .listener
            .lock()
            .map_err(|_| "input listener lock is poisoned".to_owned())?;
        if let Some(existing) = listener.as_ref() {
            return match &existing.mode {
                ListenerModeStatus::KeyboardRouting(status) => Ok(status.clone()),
                ListenerModeStatus::Identification(_) => {
                    Err("input identification is already active".to_owned())
                }
            };
        }
        let discovery = devices::enumerate()?;
        let environment = environment::detect(discovery.aster_device_evidence())?;
        let target = discovery
            .physical_devices
            .iter()
            .find(|device| {
                device.capabilities.keyboard
                    && device.id.eq_ignore_ascii_case(&target_physical_device_id)
            })
            .ok_or_else(|| {
                format!(
                    "configured keyboard '{}' is not present",
                    target_physical_device_id
                )
            })?;
        let target_id = target.id.clone();
        let correlations = build_correlations(&discovery, environment.current_session_id);
        if !correlations.values().any(|device| {
            device.device_type == devices::InputDeviceType::Keyboard
                && device
                    .stable_physical_device_id
                    .eq_ignore_ascii_case(&target_id)
        }) {
            return Err(
                "configured keyboard is not visible through Raw Input in this session".into(),
            );
        }
        let status = KeyboardRoutingDiagnosticStatus {
            active: true,
            target_physical_device_id: Some(target_id.clone()),
            current_session_id: environment.current_session_id,
            real_guest_injection_enabled: transport
                == KeyboardRoutingTransport::VirtualBoxScancodePrototype,
            host_input_suppression: "notImplemented".to_owned(),
            transport,
        };
        let detail = match transport {
            KeyboardRoutingTransport::DiagnosticOnly => {
                "diagnostic sink only; no input was injected into the VM"
            }
            KeyboardRoutingTransport::VirtualBoxScancodePrototype => {
                "scan code forwarded to the managed VM through VBoxManage keyboardputscancode"
            }
        };
        let reporting_sink: Box<dyn GuestKeyboardSink + Send> = Box::new(ReportingKeyboardSink {
            app: app.clone(),
            inner: sink,
            detail,
        });
        let router = KeyboardRouter::new(target_id, reporting_sink);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("raw-input-keyboard-routing-diagnostic".to_owned())
            .spawn(move || listener_thread(app, correlations, Some(router), ready_sender))
            .map_err(|error| format!("could not start keyboard diagnostic thread: {error}"))?;
        let thread_id = match ready_receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(thread_id)) => thread_id,
            Ok(Err(error)) => {
                let _ = join.join();
                return Err(error);
            }
            Err(error) => {
                return Err(format!(
                    "keyboard diagnostic listener did not initialize: {error}"
                ))
            }
        };
        *listener = Some(ListenerHandle {
            thread_id,
            join,
            mode: ListenerModeStatus::KeyboardRouting(status.clone()),
        });
        Ok(status)
    }

    pub fn stop_keyboard_routing(&self) -> Result<KeyboardRoutingDiagnosticStatus, String> {
        self.stop()?;
        let discovery = devices::enumerate()?;
        let session_id = environment::detect(discovery.aster_device_evidence())?.current_session_id;
        Ok(KeyboardRoutingDiagnosticStatus {
            active: false,
            target_physical_device_id: None,
            current_session_id: session_id,
            real_guest_injection_enabled: false,
            host_input_suppression: "notImplemented".to_owned(),
            transport: KeyboardRoutingTransport::DiagnosticOnly,
        })
    }

    pub fn stop(&self) -> Result<IdentificationStatus, String> {
        let handle = self
            .listener
            .lock()
            .map_err(|_| "input identification lock is poisoned".to_owned())?
            .take();
        if let Some(handle) = handle {
            stop_listener(handle)?;
        }
        Ok(IdentificationStatus {
            active: false,
            session_raw_input_devices: 0,
            correlated_physical_devices: 0,
        })
    }
}

impl Drop for InputIdentificationService {
    fn drop(&mut self) {
        if let Ok(listener) = self.listener.get_mut() {
            if let Some(handle) = listener.take() {
                let _ = stop_listener(handle);
            }
        }
    }
}

fn stop_listener(handle: ListenerHandle) -> Result<(), String> {
    if !handle.join.is_finished() {
        // SAFETY: thread_id belongs to the listener and its message queue exists after readiness.
        unsafe { PostThreadMessageW(handle.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
            .map_err(|error| format!("could not stop input identification listener: {error}"))?;
    }
    handle
        .join
        .join()
        .map_err(|_| "input identification thread panicked during shutdown".to_owned())
}

fn build_correlations(
    discovery: &devices::InputDiscovery,
    session_id: u32,
) -> HashMap<String, IdentifiedInputDevice> {
    discovery
        .session_raw_input_devices
        .iter()
        .filter(|raw| {
            !raw.aster_related
                && matches!(
                    raw.device_type,
                    devices::InputDeviceType::Keyboard | devices::InputDeviceType::Mouse
                )
        })
        .filter_map(|raw| {
            let instance_id = raw.instance_id.as_ref()?;
            let physical = discovery.physical_devices.iter().find(|physical| {
                physical.capabilities.supports(raw.device_type)
                    && physical.contains_logical_id(instance_id)
            })?;
            Some((
                raw.device_path.to_ascii_lowercase(),
                IdentifiedInputDevice {
                    device_type: raw.device_type,
                    stable_physical_device_id: physical.id.clone(),
                    container_id: physical.container_id.clone(),
                    raw_input_device_path: raw.device_path.clone(),
                    friendly_name: physical.friendly_name.clone(),
                    vendor_id: physical.vendor_id.clone(),
                    product_id: physical.product_id.clone(),
                    current_session_id: session_id,
                },
            ))
        })
        .collect()
}

fn listener_thread(
    app: tauri::AppHandle,
    correlations: HashMap<String, IdentifiedInputDevice>,
    keyboard_router: Option<KeyboardRouter<Box<dyn GuestKeyboardSink + Send>>>,
    ready: mpsc::SyncSender<Result<u32, String>>,
) {
    let initialized = initialize_listener(app, correlations, keyboard_router);
    let (window, thread_id) = match initialized {
        Ok(value) => value,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if ready.send(Ok(thread_id)).is_err() {
        cleanup_listener(window);
        return;
    }

    let mut message = MSG::default();
    loop {
        // SAFETY: message is valid output storage; this thread owns the message loop.
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 <= 0 {
            break;
        }
        // SAFETY: message was populated successfully by GetMessageW.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    cleanup_listener(window);
}

fn initialize_listener(
    app: tauri::AppHandle,
    correlations: HashMap<String, IdentifiedInputDevice>,
    keyboard_router: Option<KeyboardRouter<Box<dyn GuestKeyboardSink + Send>>>,
) -> Result<(HWND, u32), String> {
    // SAFETY: A null module name requests the current process module.
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|error| format!("GetModuleHandleW failed: {error}"))?;
    let instance = HINSTANCE(module.0);
    ensure_window_class(instance)?;

    // SAFETY: The class is registered, all string pointers are static, and HWND_MESSAGE
    // creates a non-visible message-only window owned by this listener thread.
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("MultiseatLiteInputIdentification"),
            w!("Multiseat Lite Raw Input"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| format!("could not create Raw Input message window: {error}"))?;

    LISTENER_CONTEXT.with(|context| {
        *context.borrow_mut() = Some(ListenerContext {
            app,
            correlations,
            throttle: EventThrottle::new(EVENT_DEBOUNCE),
            keyboard_router,
        });
    });
    register_raw_input(window)?;
    // SAFETY: Called on the listener thread itself.
    let thread_id = unsafe { GetCurrentThreadId() };
    Ok((window, thread_id))
}

fn ensure_window_class(instance: HINSTANCE) -> Result<(), String> {
    WINDOW_CLASS_RESULT
        .get_or_init(|| {
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: w!("MultiseatLiteInputIdentification"),
                ..Default::default()
            };
            // SAFETY: class contains a valid callback, instance, and static class name.
            let atom = unsafe { RegisterClassW(&class) };
            if atom != 0 {
                return Ok(());
            }
            // SAFETY: GetLastError is read immediately after RegisterClassW failed.
            let error = unsafe { GetLastError() };
            if error == ERROR_CLASS_ALREADY_EXISTS {
                Ok(())
            } else {
                Err(format!(
                    "RegisterClassW failed with Windows error {}",
                    error.0
                ))
            }
        })
        .clone()
}

fn register_raw_input(window: HWND) -> Result<(), String> {
    let devices = [
        RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x02,
            dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
            hwndTarget: window,
        },
        RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x06,
            dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
            hwndTarget: window,
        },
    ];
    // SAFETY: devices is a valid array and window belongs to this thread.
    unsafe { RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32) }
        .map_err(|error| format!("RegisterRawInputDevices failed: {error}"))
}

fn cleanup_listener(window: HWND) {
    let removals = [
        RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x02,
            dwFlags: RIDEV_REMOVE,
            hwndTarget: HWND::default(),
        },
        RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x06,
            dwFlags: RIDEV_REMOVE,
            hwndTarget: HWND::default(),
        },
    ];
    // SAFETY: RIDEV_REMOVE with a null target unregisters these usages for this process.
    let _ = unsafe { RegisterRawInputDevices(&removals, size_of::<RAWINPUTDEVICE>() as u32) };
    LISTENER_CONTEXT.with(|context| {
        context.borrow_mut().take();
    });
    // SAFETY: window was created and is owned by this listener thread.
    let _ = unsafe { DestroyWindow(window) };
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_INPUT => {
            process_raw_input(HRAWINPUT(lparam.0 as *mut _));
        }
        WM_INPUT_DEVICE_CHANGE if wparam.0 == GIDC_REMOVAL as usize => {
            LISTENER_CONTEXT.with(|context| {
                if let Some(router) = context
                    .borrow_mut()
                    .as_mut()
                    .and_then(|context| context.keyboard_router.as_mut())
                {
                    let _ = router.release_all();
                    PostQuitMessage(0);
                }
            });
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(window, message, wparam, lparam)
}

unsafe fn process_raw_input(raw_input: HRAWINPUT) {
    let mut input = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    let result = GetRawInputData(
        raw_input,
        RID_INPUT,
        Some((&mut input as *mut RAWINPUT).cast()),
        &mut size,
        size_of::<windows::Win32::UI::Input::RAWINPUTHEADER>() as u32,
    );
    if result == API_ERROR
        || (input.header.dwType != RIM_TYPEMOUSE.0 && input.header.dwType != RIM_TYPEKEYBOARD.0)
    {
        return;
    }
    let Ok(path) = devices::raw_input_device_path(input.header.hDevice) else {
        return;
    };
    let key = path.to_ascii_lowercase();
    LISTENER_CONTEXT.with(|context| {
        let mut context = context.borrow_mut();
        let Some(context) = context.as_mut() else {
            return;
        };
        let Some(device) = context.correlations.get(&key).cloned() else {
            return;
        };
        if let Some(router) = context.keyboard_router.as_mut() {
            if input.header.dwType != RIM_TYPEKEYBOARD.0
                || device.device_type != devices::InputDeviceType::Keyboard
            {
                return;
            }
            let keyboard = input.data.keyboard;
            let event = normalize_raw_keyboard_event(
                device.stable_physical_device_id,
                path,
                keyboard.MakeCode,
                keyboard.Flags,
                keyboard.VKey,
            );
            if let Err(error) = router.route(event) {
                let _ = context.app.emit(
                    ROUTING_EVENT_NAME,
                    KeyboardRoutingDiagnosticEvent {
                        action: KeyboardDiagnosticAction::BackendError,
                        event: None,
                        detail: error,
                    },
                );
                PostQuitMessage(0);
            }
            return;
        }
        if context.throttle.should_emit(&key, Instant::now()) {
            let _ = context.app.emit(EVENT_NAME, device);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{
        aggregate_physical_devices, AsterRelatedPnpDevice, InputDevice, InputDiscovery,
        RawInputDevice,
    };

    #[test]
    fn raw_input_from_each_child_maps_to_the_same_physical_device() {
        let logical_nodes: Vec<_> = ["ABC", "DEF"]
            .into_iter()
            .map(|suffix| InputDevice {
                id: format!(r"HID\VID_1234&PID_5678\{suffix}"),
                device_path: None,
                instance_id: format!(r"HID\VID_1234&PID_5678\{suffix}"),
                container_id: Some("container-a".into()),
                friendly_name: Some("Identical keyboard name".into()),
                manufacturer: None,
                hardware_ids: Vec::new(),
                enumerator: Some("HID".into()),
                device_type: devices::InputDeviceType::Keyboard,
                vendor_id: Some("1234".into()),
                product_id: Some("5678".into()),
                usb_parent_instance_id: None,
                serial_number: None,
            })
            .collect();
        let raw_devices = vec![
            RawInputDevice {
                id: r"\\?\HID#VID_1234&PID_5678#ABC".into(),
                device_path: r"\\?\HID#VID_1234&PID_5678#ABC".into(),
                instance_id: Some(r"HID\VID_1234&PID_5678\ABC".into()),
                device_type: devices::InputDeviceType::Keyboard,
                aster_related: false,
            },
            RawInputDevice {
                id: r"\\?\HID#VID_1234&PID_5678#DEF".into(),
                device_path: r"\\?\HID#VID_1234&PID_5678#DEF".into(),
                instance_id: Some(r"HID\VID_1234&PID_5678\DEF".into()),
                device_type: devices::InputDeviceType::Keyboard,
                aster_related: false,
            },
            RawInputDevice {
                id: r"\\?\ROOT#MUTENX_DEVICE#0000#KEYBOARD".into(),
                device_path: r"\\?\ROOT#MUTENX_DEVICE#0000#KEYBOARD".into(),
                instance_id: Some(r"ROOT\MUTENX_DEVICE\0000".into()),
                device_type: devices::InputDeviceType::Keyboard,
                aster_related: true,
            },
        ];
        let discovery = InputDiscovery {
            physical_devices: aggregate_physical_devices(&logical_nodes, &raw_devices),
            logical_pnp_devices: logical_nodes,
            session_raw_input_devices: raw_devices,
            aster_related_pnp_devices: vec![AsterRelatedPnpDevice {
                instance_id: r"ROOT\MUTENX_DEVICE\0000".into(),
                friendly_name: None,
                service: Some("MUTENX_SERVICE".into()),
                class_guid: "legacy".into(),
            }],
        };
        let correlations = build_correlations(&discovery, 7);
        assert_eq!(correlations.len(), 2);
        assert!(correlations.values().all(|matched| {
            matched.stable_physical_device_id == discovery.physical_devices[0].id
                && matched.container_id.as_deref() == Some("container-a")
                && matched.current_session_id == 7
        }));
    }

    #[test]
    fn throttle_emits_on_source_change_and_after_interval() {
        let start = Instant::now();
        let mut throttle = EventThrottle::new(Duration::from_secs(1));
        assert!(throttle.should_emit("mouse-a", start));
        assert!(!throttle.should_emit("mouse-a", start + Duration::from_millis(100)));
        assert!(throttle.should_emit("mouse-b", start + Duration::from_millis(200)));
        assert!(throttle.should_emit("mouse-b", start + Duration::from_millis(1200)));
    }
}
