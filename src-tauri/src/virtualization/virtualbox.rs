use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    process::{Command, Output},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

mod creation;
mod focus_protection;
mod installation;
mod keyboard_sink;
mod mouse_integration;
mod presentation;
pub use creation::*;
use focus_protection::{FocusProtectionBackend, ManagedVmWindowIdentity};
pub use installation::*;
pub use keyboard_sink::VirtualBoxGuestKeyboardSink;
use mouse_integration::MouseIntegrationController;
pub use mouse_integration::{
    MouseIntegrationControl, MouseIntegrationState, MouseIntegrationStatus,
};
use presentation::PresentationBackend;
pub use presentation::{
    DisplayPresentationStatus, PresentationFailureReason, PresentationMode, PresentationState,
    PresentationTraceEntry, WindowCandidateDiagnostic,
};

use crate::{
    devices::PhysicalInputDevice,
    seats::{
        ApplicationConfig, DeviceRoutingBackend, InputRoutingRuntimeStatus, InputRoutingStrategy,
        SeatId, UsbPassthroughSafety,
    },
};

use super::{
    correlate_usb_devices, parse_virtualbox_usbhost, SeatRuntimeStatus, UsbCorrelationState,
    VirtualBoxStatus,
};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VBoxOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub trait VBoxManageExecutor: Send + Sync {
    fn execute(&self, arguments: &[String]) -> Result<VBoxOutput, String>;

    fn wait(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

pub struct ProcessVBoxManageExecutor {
    path: PathBuf,
}

impl ProcessVBoxManageExecutor {
    pub fn discover() -> Result<Self, String> {
        let path = super::windows::vbox_manage_path()
            .ok_or_else(|| "VBoxManage.exe was not found".to_owned())?;
        Ok(Self { path })
    }

    fn run(&self, arguments: &[String]) -> Result<Output, String> {
        let mut command = Command::new(&self.path);
        command.args(arguments);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command.output().map_err(|error| {
            format!(
                "could not execute '{}' with arguments {:?}: {error}",
                self.path.display(),
                arguments
            )
        })
    }
}

impl VBoxManageExecutor for ProcessVBoxManageExecutor {
    fn execute(&self, arguments: &[String]) -> Result<VBoxOutput, String> {
        let output = self.run(arguments)?;
        Ok(VBoxOutput {
            success: output.status.success(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualMachine {
    pub name: String,
    pub uuid: String,
    pub state: String,
    pub guest_os_description: Option<String>,
    pub usb_xhci_enabled: Option<bool>,
    pub session_pid: Option<u32>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualMachineList {
    pub machines: Vec<VirtualMachine>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeComponentStatus {
    pub configured: bool,
    pub attached: bool,
    pub physical_device_id: Option<String>,
    pub runtime_uuid: Option<String>,
    pub mapping: Option<UsbCorrelationState>,
    pub usb: RuntimeUsbDiagnostic,
    pub routing_strategy: InputRoutingStrategy,
    pub routing_status: InputRoutingRuntimeStatus,
    pub successful_guest_sends: Option<u64>,
    pub usb_passthrough_safety: UsbPassthroughSafety,
    pub safety_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EvidencePresence {
    Present,
    Missing,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UsbAttachCommandStatus {
    #[default]
    NotRequested,
    Success,
    Failure,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GuestUsbVisibility {
    Verified,
    Missing,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UsbRuntimeHealth {
    #[default]
    NotResolved,
    HostBusy,
    AttachRequested,
    Captured,
    DisappearedAfterAttach,
    PhysicalDeviceDisconnected,
    Released,
    Ambiguous,
    Failed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeUsbDiagnostic {
    pub windows_physical_device: EvidencePresence,
    pub usb_parent: EvidencePresence,
    pub virtual_box_host_device: EvidencePresence,
    pub resolved_usb_parent: Option<String>,
    pub runtime_uuid: Option<String>,
    pub runtime_address: Option<String>,
    pub state_before_attach: Option<String>,
    pub attach_command: UsbAttachCommandStatus,
    pub state_immediately_after_attach: Option<String>,
    pub state_after_retry: Option<String>,
    pub current_virtual_box_state: Option<String>,
    pub guest_visibility: GuestUsbVisibility,
    pub health: UsbRuntimeHealth,
    pub probe_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatRuntimeSnapshot {
    pub seat_id: String,
    pub status: SeatRuntimeStatus,
    pub vm_id: Option<String>,
    pub vm_state: Option<String>,
    pub vm_started_by_multiseat: bool,
    pub keyboard: RuntimeComponentStatus,
    pub mouse: RuntimeComponentStatus,
    pub display_presentation: DisplayPresentationStatus,
    pub startup_trace: Vec<StartupTraceEntry>,
    pub input_isolation: ManagedInputIsolationStatus,
    pub message: Option<String>,
    pub logs: Vec<RuntimeLogEntry>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ManagedSeatDisplayMode {
    #[default]
    NormalVirtualBox,
    SeatDisplayLocked,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedInputIsolationStatus {
    pub display_mode: ManagedSeatDisplayMode,
    pub mouse_capture_policy: Option<String>,
    pub mouse_capture_disabled: bool,
    pub mouse_capture_policy_owned: bool,
    pub mouse_capture_policy_error: Option<String>,
    pub mouse_integration: MouseIntegrationStatus,
    pub mouse_isolation: MouseIsolationStatus,
    pub focus_protection_active: bool,
    pub focus_protection_locked: bool,
    pub focus_restoration_count: u64,
    pub focus_protection_error: Option<String>,
    pub dennis_keyboard_attached: bool,
    pub dennis_mouse_attached: bool,
    pub host_gui_keyboard_isolation: HostGuiKeyboardIsolation,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseIsolationStatus {
    Active,
    Inactive,
    Error,
    #[default]
    Unverified,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HostGuiKeyboardIsolation {
    #[default]
    BestEffort,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatOperationResult {
    pub success: bool,
    pub runtime: SeatRuntimeSnapshot,
    pub errors: Vec<String>,
    pub rollback_attempted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogEntry {
    pub action: String,
    pub outcome: String,
    pub physical_device_id: Option<String>,
    pub runtime_uuid: Option<String>,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupTraceEntry {
    pub stage: String,
    pub timestamp_ms: u64,
    pub elapsed_ms: u64,
    pub duration_ms: Option<u64>,
    pub detail: Option<String>,
    pub error: Option<String>,
    pub process_id: Option<u32>,
    pub window_handle: Option<isize>,
    pub target_display_id: Option<String>,
    pub target_bounds: Option<crate::displays::DisplayBounds>,
    pub exit_status: Option<i32>,
}

#[derive(Clone, Debug)]
struct RuntimeRecord {
    vm_started_by_multiseat: bool,
    attached: Vec<ManagedAttachment>,
    logs: Vec<RuntimeLogEntry>,
    last_error: Option<String>,
    transitional: Option<SeatRuntimeStatus>,
    display_mode: ManagedSeatDisplayMode,
    mouse_integration: MouseIntegrationStatus,
    keyboard_routing_status: InputRoutingRuntimeStatus,
    startup_started: Option<Instant>,
    startup_trace: Vec<StartupTraceEntry>,
}

impl Default for RuntimeRecord {
    fn default() -> Self {
        Self {
            vm_started_by_multiseat: false,
            attached: Vec::new(),
            logs: Vec::new(),
            last_error: None,
            transitional: None,
            display_mode: ManagedSeatDisplayMode::default(),
            mouse_integration: MouseIntegrationStatus::default(),
            keyboard_routing_status: InputRoutingRuntimeStatus::PrototypeInactive,
            startup_started: None,
            startup_trace: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct ManagedAttachment {
    physical_device_id: String,
    runtime_uuid: String,
    kind: InputKind,
    mapping: UsbCorrelationState,
    diagnostic: RuntimeUsbDiagnostic,
}

#[derive(Clone, Debug)]
struct ResolvedUsb {
    uuid: String,
    address: Option<String>,
    state: Option<String>,
    mapping: UsbCorrelationState,
}

#[derive(Clone, Debug)]
struct UsbObservation {
    windows_physical_device: EvidencePresence,
    usb_parent: EvidencePresence,
    virtual_box_host_device: EvidencePresence,
    resolved_usb_parent: Option<String>,
    runtime_uuid: Option<String>,
    runtime_address: Option<String>,
    current_state: Option<String>,
    mapping: Option<UsbCorrelationState>,
    health: UsbRuntimeHealth,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputKind {
    Keyboard,
    Mouse,
}

pub struct VirtualBoxRuntimeService {
    executor: Box<dyn VBoxManageExecutor>,
    records: Mutex<HashMap<String, RuntimeRecord>>,
    focus_protection: Box<dyn FocusProtectionBackend>,
    mouse_integration: Box<dyn MouseIntegrationController>,
    presentation: Box<dyn PresentationBackend>,
    refresh_physical_hardware: bool,
    asynchronous_starts: Mutex<HashSet<String>>,
    start_cancellations: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl VirtualBoxRuntimeService {
    pub fn from_environment() -> Self {
        let mut service = match ProcessVBoxManageExecutor::discover() {
            Ok(executor) => Self::with_components(
                Box::new(executor),
                focus_protection::native_backend(),
                mouse_integration::documented_backend(),
                presentation::native_backend(),
            ),
            Err(error) => Self::with_components(
                Box::new(UnavailableExecutor(error)),
                focus_protection::native_backend(),
                mouse_integration::documented_backend(),
                presentation::native_backend(),
            ),
        };
        service.refresh_physical_hardware = true;
        service
    }

    pub fn discover() -> Result<Self, String> {
        let mut service = Self::with_components(
            Box::new(ProcessVBoxManageExecutor::discover()?),
            focus_protection::native_backend(),
            mouse_integration::documented_backend(),
            presentation::native_backend(),
        );
        service.refresh_physical_hardware = true;
        Ok(service)
    }

    pub fn new(executor: Box<dyn VBoxManageExecutor>) -> Self {
        Self::with_components(
            executor,
            Box::new(focus_protection::SimulatedFocusProtection::default()),
            mouse_integration::simulated_backend(),
            Box::new(presentation::SimulatedPresentation::default()),
        )
    }

    fn with_components(
        executor: Box<dyn VBoxManageExecutor>,
        focus_protection: Box<dyn FocusProtectionBackend>,
        mouse_integration: Box<dyn MouseIntegrationController>,
        presentation: Box<dyn PresentationBackend>,
    ) -> Self {
        Self {
            executor,
            records: Mutex::new(HashMap::new()),
            focus_protection,
            mouse_integration,
            presentation,
            refresh_physical_hardware: false,
            asynchronous_starts: Mutex::new(HashSet::new()),
            start_cancellations: Mutex::new(HashMap::new()),
        }
    }

    pub fn record_error(&self, seat_id: &SeatId, error: &str) -> Result<(), String> {
        self.with_record(seat_id, |record| {
            record.last_error = Some(error.to_owned());
            record.transitional = Some(SeatRuntimeStatus::Error);
        })
    }

    pub fn begin_asynchronous_start(
        &self,
        seat_id: &SeatId,
        configuration: &ApplicationConfig,
    ) -> Result<(Arc<AtomicBool>, SeatRuntimeSnapshot), String> {
        {
            let mut starts = self
                .asynchronous_starts
                .lock()
                .map_err(|_| "VirtualBox asynchronous-start lock is poisoned")?;
            if !starts.insert(seat_id.0.clone()) {
                return Err(format!("seat '{}' is already starting", seat_id.0));
            }
        }
        let cancellation = Arc::new(AtomicBool::new(false));
        self.start_cancellations
            .lock()
            .map_err(|_| "VirtualBox start-cancellation lock is poisoned")?
            .insert(seat_id.0.clone(), cancellation.clone());
        self.replace_record(
            seat_id,
            RuntimeRecord {
                transitional: Some(SeatRuntimeStatus::Starting),
                startup_started: Some(Instant::now()),
                ..RuntimeRecord::default()
            },
        )?;
        self.trace(seat_id, "StartRequested", None, None)?;
        self.trace(
            seat_id,
            "StartCommandReturned",
            Some("startup accepted; activation continues on a background worker"),
            None,
        )?;
        Ok((
            cancellation,
            self.accepted_start_snapshot(seat_id, configuration)?,
        ))
    }

    pub fn finish_asynchronous_start(&self, seat_id: &SeatId) {
        if let Ok(mut starts) = self.asynchronous_starts.lock() {
            starts.remove(&seat_id.0);
        }
        if let Ok(mut cancellations) = self.start_cancellations.lock() {
            cancellations.remove(&seat_id.0);
        }
    }

    pub fn cancel_asynchronous_start(&self, seat_id: &SeatId) {
        if let Ok(cancellations) = self.start_cancellations.lock() {
            if let Some(cancellation) = cancellations.get(&seat_id.0) {
                cancellation.store(true, Ordering::Release);
            }
        }
    }

    pub fn set_keyboard_routing_runtime(
        &self,
        seat_id: &SeatId,
        status: InputRoutingRuntimeStatus,
        detail: &str,
    ) -> Result<(), String> {
        self.with_record(seat_id, |record| record.keyboard_routing_status = status)?;
        self.log(
            seat_id,
            "keyboard-routing",
            if matches!(
                status,
                InputRoutingRuntimeStatus::Active | InputRoutingRuntimeStatus::PrototypeActive
            ) {
                "active"
            } else {
                "error"
            },
            None,
            None,
            detail,
        )
    }

    pub fn record_startup_stage(
        &self,
        seat_id: &SeatId,
        stage: &str,
        detail: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), String> {
        self.trace(seat_id, stage, detail, error)
    }

    pub fn record_startup_duration(
        &self,
        seat_id: &SeatId,
        stage: &str,
        started: Instant,
        detail: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), String> {
        self.trace_detailed(seat_id, stage, detail, error, Some(started.elapsed()), None)
    }

    fn accepted_start_snapshot(
        &self,
        seat_id: &SeatId,
        configuration: &ApplicationConfig,
    ) -> Result<SeatRuntimeSnapshot, String> {
        let routing = configuration.input_routing(seat_id);
        let keyboard_id = target_device_id(seat_id, configuration, InputKind::Keyboard);
        let mouse_id = target_device_id(seat_id, configuration, InputKind::Mouse);
        let record = self.record(seat_id)?;
        Ok(SeatRuntimeSnapshot {
            seat_id: seat_id.0.clone(),
            status: SeatRuntimeStatus::Starting,
            vm_id: configured_vm_id(seat_id, configuration)
                .ok()
                .map(str::to_owned),
            vm_state: Some("start-requested".to_owned()),
            vm_started_by_multiseat: false,
            keyboard: pending_component(keyboard_id, routing.keyboard, &record),
            mouse: pending_component(mouse_id, routing.mouse, &record),
            display_presentation: DisplayPresentationStatus {
                state: PresentationState::WaitingForVmWindow,
                assigned_display_id: configured_display_id(seat_id, configuration)
                    .map(str::to_owned),
                ..DisplayPresentationStatus::default()
            },
            startup_trace: record.startup_trace.clone(),
            input_isolation: ManagedInputIsolationStatus::default(),
            message: Some("Start accepted; VM and seat components are starting".to_owned()),
            logs: record.logs,
        })
    }

    pub fn list_vms(&self) -> Result<VirtualMachineList, String> {
        let output = self.checked(&["list", "vms"])?;
        let mut result = VirtualMachineList::default();
        for line in output
            .stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            let Some((name, uuid)) = parse_vm_list_line(line) else {
                result
                    .warnings
                    .push(format!("ignored malformed VM list line: {line}"));
                continue;
            };
            match self.inspect_vm(&uuid) {
                Ok(mut vm) => {
                    vm.name = name;
                    result.machines.push(vm);
                }
                Err(error) => result
                    .warnings
                    .push(format!("could not inspect VM {uuid}: {error}")),
            }
        }
        Ok(result)
    }

    pub fn inspect_vm(&self, vm_id: &str) -> Result<VirtualMachine, String> {
        let output = self.checked(&["showvminfo", vm_id, "--machinereadable"])?;
        let fields = parse_machine_readable(&output.stdout);
        Ok(VirtualMachine {
            name: fields
                .get("name")
                .cloned()
                .unwrap_or_else(|| vm_id.to_owned()),
            uuid: fields
                .get("UUID")
                .cloned()
                .unwrap_or_else(|| vm_id.to_owned()),
            state: fields
                .get("VMState")
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned()),
            guest_os_description: fields.get("ostype").cloned(),
            usb_xhci_enabled: fields
                .get("xhci")
                .map(|value| value.eq_ignore_ascii_case("on")),
            session_pid: fields
                .get("SessionPID")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|pid| *pid != 0),
        })
    }

    /// Applies and verifies the documented per-VM VirtualBox GUI policy. This
    /// deliberately refuses unmanaged VMs, so an unrelated VM can never be
    /// modified through the managed-seat path.
    pub fn ensure_managed_mouse_capture_disabled(
        &self,
        seat_id: &SeatId,
        configuration: &ApplicationConfig,
    ) -> Result<bool, String> {
        let backend = configuration
            .backends
            .virtual_box
            .seats
            .get(&seat_id.0)
            .ok_or_else(|| format!("seat '{}' has no VirtualBox VM selected", seat_id.0))?;
        if !backend.managed_by_multiseat {
            return Ok(false);
        }
        self.set_mouse_capture_policy(&backend.vm_id, "Disabled")?;
        Ok(true)
    }

    pub fn allow_host_input_temporarily(
        &self,
        seat_id: &SeatId,
        configuration: &ApplicationConfig,
        physical_devices: &[PhysicalInputDevice],
    ) -> Result<SeatRuntimeSnapshot, String> {
        let backend = managed_backend(seat_id, configuration)?;
        require_running(&self.inspect_vm(&backend.vm_id)?)?;
        self.set_mouse_capture_policy(&backend.vm_id, "Default")?;
        let integration = self
            .mouse_integration
            .request(&backend.vm_id, MouseIntegrationState::Enabled)?;
        self.presentation.set_locked(false)?;
        self.focus_protection.set_locked(false)?;
        self.with_record(seat_id, |record| {
            record.display_mode = ManagedSeatDisplayMode::NormalVirtualBox;
            record.mouse_integration = integration;
        })?;
        self.log(
            seat_id,
            "input-isolation",
            "temporarily-unlocked",
            None,
            None,
            "administrator temporarily allowed VirtualBox GUI host input",
        )?;
        self.status(seat_id, configuration, physical_devices)
    }

    pub fn lock_seat_input(
        &self,
        seat_id: &SeatId,
        configuration: &ApplicationConfig,
        physical_devices: &[PhysicalInputDevice],
        focus_anchor: Option<isize>,
    ) -> Result<SeatRuntimeSnapshot, String> {
        let backend = managed_backend(seat_id, configuration)?;
        let vm = self.inspect_vm(&backend.vm_id)?;
        require_running(&vm)?;
        self.set_mouse_capture_policy(&backend.vm_id, "Disabled")?;
        let integration = self
            .mouse_integration
            .request(&backend.vm_id, MouseIntegrationState::Disabled)?;
        self.start_managed_presentation(seat_id, configuration, &vm);
        self.focus_protection.start(
            ManagedVmWindowIdentity {
                vm_uuid: vm.uuid,
                vm_name: vm.name,
                session_pid: vm.session_pid,
            },
            focus_anchor,
        )?;
        self.with_record(seat_id, |record| {
            record.display_mode = ManagedSeatDisplayMode::SeatDisplayLocked;
            record.mouse_integration = integration;
        })?;
        self.log(
            seat_id,
            "input-isolation",
            "locked",
            None,
            None,
            "VirtualBox GUI host input locked for managed seat",
        )?;
        self.status(seat_id, configuration, physical_devices)
    }

    pub fn shutdown_focus_protection(&self) -> Result<(), String> {
        self.presentation.stop()?;
        self.focus_protection.stop()
    }

    fn start_managed_presentation(
        &self,
        seat_id: &SeatId,
        configuration: &ApplicationConfig,
        vm: &VirtualMachine,
    ) {
        let Some(display_id) = configured_display_id(seat_id, configuration) else {
            let _ = self.presentation.mark_unavailable(
                None,
                "seat has no assigned physical display; VM remains running".to_owned(),
            );
            return;
        };
        if let Err(error) = self.presentation.start(
            ManagedVmWindowIdentity {
                vm_uuid: vm.uuid.clone(),
                vm_name: vm.name.clone(),
                session_pid: vm.session_pid,
            },
            display_id.to_owned(),
        ) {
            let _ = self
                .presentation
                .mark_unavailable(Some(display_id.to_owned()), error);
        }
    }

    fn set_mouse_capture_policy(&self, vm_id: &str, policy: &str) -> Result<(), String> {
        self.checked(&["setextradata", vm_id, "GUI/MouseCapturePolicy", policy])?;
        let observed = self.mouse_capture_policy(vm_id)?.ok_or_else(|| {
            format!("VirtualBox did not return GUI/MouseCapturePolicy after setting {policy}")
        })?;
        if observed.eq_ignore_ascii_case(policy) {
            Ok(())
        } else {
            Err(format!(
                "VirtualBox GUI/MouseCapturePolicy verification failed: expected {policy}, observed {observed}"
            ))
        }
    }

    fn mouse_capture_policy(&self, vm_id: &str) -> Result<Option<String>, String> {
        let output = self.checked(&["getextradata", vm_id, "GUI/MouseCapturePolicy"])?;
        Ok(output
            .stdout
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix("Value:").map(str::trim))
            .filter(|value| !value.is_empty())
            .map(str::to_owned))
    }

    pub fn start(
        &self,
        seat_id: &SeatId,
        configuration: &ApplicationConfig,
        physical_devices: &[PhysicalInputDevice],
        focus_anchor: Option<isize>,
    ) -> Result<SeatRuntimeSnapshot, String> {
        let cancellation = Arc::new(AtomicBool::new(false));
        let result = self.start_inner(
            seat_id,
            configuration,
            physical_devices,
            focus_anchor,
            &cancellation,
        );
        if result.is_err() {
            let _ = self.presentation.stop();
            let _ = self.focus_protection.stop();
        }
        result
    }

    pub fn start_with_cancellation(
        &self,
        seat_id: &SeatId,
        configuration: &ApplicationConfig,
        physical_devices: &[PhysicalInputDevice],
        focus_anchor: Option<isize>,
        cancellation: &Arc<AtomicBool>,
    ) -> Result<SeatRuntimeSnapshot, String> {
        let result = self.start_inner(
            seat_id,
            configuration,
            physical_devices,
            focus_anchor,
            cancellation,
        );
        if result.is_err() {
            let _ = self.presentation.stop();
            let _ = self.focus_protection.stop();
        }
        result
    }

    fn start_inner(
        &self,
        seat_id: &SeatId,
        configuration: &ApplicationConfig,
        physical_devices: &[PhysicalInputDevice],
        focus_anchor: Option<isize>,
        cancellation: &Arc<AtomicBool>,
    ) -> Result<SeatRuntimeSnapshot, String> {
        self.check_start_cancelled(cancellation)?;
        let inputs = validate_activation(seat_id, configuration, physical_devices)?;
        let vm_id = configured_vm_id(seat_id, configuration)?.to_owned();
        let vm = self.inspect_vm(&vm_id)?;
        let vm_errors = validate_vm(&vm);
        if !vm_errors.is_empty() {
            return Err(vm_errors.join("; "));
        }
        let vm_already_running = vm.state.eq_ignore_ascii_case("running");

        self.with_record(seat_id, |record| {
            record.transitional = Some(SeatRuntimeStatus::Starting);
            record.last_error = None;
            record.attached.clear();
            record.logs.clear();
            record.keyboard_routing_status = InputRoutingRuntimeStatus::PrototypeInactive;
            if record.startup_started.is_none() {
                record.startup_started = Some(Instant::now());
                record.startup_trace.clear();
            }
        })?;
        if self.record(seat_id)?.startup_trace.is_empty() {
            self.trace(seat_id, "StartRequested", None, None)?;
        }
        self.log(
            seat_id,
            "activation",
            "started",
            None,
            None,
            "activation validation succeeded",
        )?;
        self.log(seat_id, "vm-state", "observed", None, None, &vm.state)?;

        let managed = configuration
            .backends
            .virtual_box
            .seats
            .get(&seat_id.0)
            .is_some_and(|backend| backend.managed_by_multiseat);
        if managed {
            self.set_mouse_capture_policy(&vm_id, "Disabled")?;
            self.with_record(seat_id, |record| {
                record.display_mode = ManagedSeatDisplayMode::SeatDisplayLocked
            })?;
        }

        let routing = configuration.input_routing(seat_id);
        // Preflight is deliberately fresh for every activation. Its UUIDs are validation-only.
        self.resolve_required_usb(&inputs, &routing, configuration)?;
        self.check_start_cancelled(cancellation)?;
        if !vm_already_running {
            let vm_start_started = Instant::now();
            let output = self.checked(&["startvm", &vm_id, "--type", "gui"])?;
            self.trace_detailed(
                seat_id,
                "VMStartRequested",
                Some(&vm_id),
                None,
                Some(vm_start_started.elapsed()),
                output.exit_code,
            )?;
            self.with_record(seat_id, |record| record.vm_started_by_multiseat = true)?;
            self.log(
                seat_id,
                "vm-start",
                "success",
                None,
                None,
                "VirtualBox VM start requested",
            )?;
        } else {
            self.log(
                seat_id,
                "vm-start",
                "skipped",
                None,
                None,
                "VM was already running",
            )?;
        }
        let vm_wait_started = Instant::now();
        self.wait_for_vm_state_cancellable(&vm_id, "running", 40, cancellation)?;
        self.trace_detailed(
            seat_id,
            "VMRunningObserved",
            Some(&vm_id),
            None,
            Some(vm_wait_started.elapsed()),
            None,
        )?;
        self.check_start_cancelled(cancellation)?;
        if managed {
            let running_vm = self.inspect_vm(&vm_id)?;
            self.start_managed_presentation(seat_id, configuration, &running_vm);
            self.trace(
                seat_id,
                "PresentationWorkerStarted",
                configured_display_id(seat_id, configuration),
                None,
            )?;
            self.focus_protection.start(
                ManagedVmWindowIdentity {
                    vm_uuid: running_vm.uuid,
                    vm_name: running_vm.name,
                    session_pid: running_vm.session_pid,
                },
                focus_anchor,
            )?;
            self.trace(seat_id, "FocusProtectionActive", None, None)?;
            let integration = self
                .mouse_integration
                .request(&vm_id, MouseIntegrationState::Disabled)?;
            self.with_record(seat_id, |record| {
                record.mouse_integration = integration;
                record.display_mode = ManagedSeatDisplayMode::SeatDisplayLocked;
            })?;
        }

        // Resolve again immediately before each attachment. UUIDs from preflight
        // are validation-only and are never reused for a controlvm operation.
        for (kind, physical) in [
            (InputKind::Keyboard, inputs.keyboard),
            (InputKind::Mouse, inputs.mouse),
        ] {
            let strategy = match kind {
                InputKind::Keyboard => routing.keyboard,
                InputKind::Mouse => routing.mouse,
            };
            if strategy != InputRoutingStrategy::VirtualBoxUsbPassthrough {
                self.log(
                    seat_id,
                    "input-routing",
                    "usb-skipped",
                    Some(&physical.id),
                    None,
                    match strategy {
                        InputRoutingStrategy::NativeKeyboardRouting => {
                            "physical keyboard USB passthrough disabled; native diagnostic routing selected"
                        }
                        InputRoutingStrategy::Disabled => "input routing is disabled",
                        InputRoutingStrategy::VirtualBoxUsbPassthrough => {
                            "VirtualBox USB passthrough selected"
                        }
                    },
                )?;
                continue;
            }
            let attach_started = (kind == InputKind::Mouse).then(Instant::now);
            if kind == InputKind::Mouse {
                self.trace(seat_id, "MouseAttachStarted", Some(&physical.id), None)?;
            }
            self.check_start_cancelled(cancellation)?;
            let resolved = self.resolve_single_usb(physical)?;
            if let Err(error) =
                self.attach(seat_id, &vm_id, kind, physical, &resolved, physical_devices)
            {
                let rollback_errors = self.rollback(
                    seat_id,
                    &vm_id,
                    &[inputs.keyboard.clone(), inputs.mouse.clone()],
                );
                let message = if rollback_errors.is_empty() {
                    error
                } else {
                    format!("{error}; rollback errors: {}", rollback_errors.join("; "))
                };
                self.with_record(seat_id, |record| {
                    record.last_error = Some(message.clone());
                    record.transitional = Some(SeatRuntimeStatus::Error);
                })?;
                return Err(message);
            }
            if let Some(attach_started) = attach_started {
                self.trace_detailed(
                    seat_id,
                    "MouseCaptured",
                    Some(&physical.id),
                    None,
                    Some(attach_started.elapsed()),
                    None,
                )?;
            }
        }
        self.with_record(seat_id, |record| record.transitional = None)?;
        self.status(seat_id, configuration, physical_devices)
    }

    pub fn stop(
        &self,
        seat_id: &SeatId,
        configuration: &ApplicationConfig,
        physical_devices: &[PhysicalInputDevice],
    ) -> Result<SeatRuntimeSnapshot, String> {
        self.cancel_asynchronous_start(seat_id);
        self.presentation.stop()?;
        self.focus_protection.stop()?;
        let vm_id = configured_vm_id(seat_id, configuration)?.to_owned();
        let restored_integration = self.mouse_integration.restore_if_owned(&vm_id)?;
        self.with_record(seat_id, |record| {
            record.transitional = Some(SeatRuntimeStatus::Stopping);
            record.mouse_integration = restored_integration;
        })?;
        self.detach_managed(seat_id, &vm_id, physical_devices)?;
        let vm = self.inspect_vm(&vm_id)?;
        if vm.state.eq_ignore_ascii_case("running") {
            match self.checked(&["controlvm", &vm_id, "shutdown"]) {
                Ok(_) => self.log(
                    seat_id,
                    "vm-stop",
                    "requested",
                    None,
                    None,
                    "graceful Guest Additions shutdown requested",
                )?,
                Err(guest_error) => {
                    self.log(
                        seat_id,
                        "vm-stop",
                        "fallback",
                        None,
                        None,
                        &format!("Guest shutdown unavailable: {guest_error}"),
                    )?;
                    self.checked(&["controlvm", &vm_id, "acpipowerbutton"])?;
                    self.log(
                        seat_id,
                        "vm-stop",
                        "requested",
                        None,
                        None,
                        "ACPI power button requested",
                    )?;
                }
            }
            let _ = self.wait_for_vm_state(&vm_id, "poweroff", 30);
        }
        self.with_record(seat_id, |record| {
            record.transitional = None;
            record.vm_started_by_multiseat = false;
            record.last_error = None;
            record.display_mode = ManagedSeatDisplayMode::NormalVirtualBox;
        })?;
        self.status(seat_id, configuration, physical_devices)
    }

    pub fn release(
        &self,
        seat_id: &SeatId,
        configuration: &ApplicationConfig,
        physical_devices: &[PhysicalInputDevice],
    ) -> Result<SeatRuntimeSnapshot, String> {
        let inputs = validate_target_inputs_only(seat_id, configuration, physical_devices)?;
        let vm_id = configured_vm_id(seat_id, configuration)?.to_owned();
        let routing = configuration.input_routing(seat_id);
        for (kind, physical, strategy) in [
            (InputKind::Keyboard, inputs.keyboard, routing.keyboard),
            (InputKind::Mouse, inputs.mouse, routing.mouse),
        ] {
            if strategy != InputRoutingStrategy::VirtualBoxUsbPassthrough {
                self.with_record(seat_id, |record| {
                    record.attached.retain(|item| item.kind != kind)
                })?;
                continue;
            }
            let current = self.usb_status()?;
            let mapping = correlate_usb_devices(&[physical.clone()], &current).remove(0);
            if matches!(
                mapping.state,
                UsbCorrelationState::Exact | UsbCorrelationState::Unambiguous
            ) {
                if let Some(uuid) = mapping.matched_uuid {
                    let is_captured = current
                        .usb_devices
                        .iter()
                        .find(|usb| usb.uuid == uuid)
                        .and_then(|usb| usb.current_state.as_deref())
                        .is_some_and(|state| {
                            state.eq_ignore_ascii_case("captured")
                                || state.eq_ignore_ascii_case("held")
                        });
                    if is_captured {
                        match self.checked(&["controlvm", &vm_id, "usbdetach", &uuid]) {
                            Ok(_) => self.log(
                                seat_id,
                                "usb-detach",
                                "success",
                                Some(&physical.id),
                                Some(&uuid),
                                "configured Barnen device released",
                            )?,
                            Err(error) => self.log(
                                seat_id,
                                "usb-detach",
                                "error",
                                Some(&physical.id),
                                Some(&uuid),
                                &error,
                            )?,
                        }
                    }
                }
            }
            self.with_record(seat_id, |record| {
                record.attached.retain(|item| item.kind != kind)
            })?;
        }
        self.with_record(seat_id, |record| {
            record.last_error = None;
            record.transitional = None;
        })?;
        self.status(seat_id, configuration, physical_devices)
    }

    pub fn status(
        &self,
        seat_id: &SeatId,
        configuration: &ApplicationConfig,
        physical_devices: &[PhysicalInputDevice],
    ) -> Result<SeatRuntimeSnapshot, String> {
        let backend = configuration.backends.virtual_box.seats.get(&seat_id.0);
        let vm_id = backend.map(|item| item.vm_id.clone());
        let record = self.record(seat_id)?;
        let vm_state = vm_id
            .as_deref()
            .and_then(|id| self.inspect_vm(id).ok())
            .map(|vm| vm.state);
        let keyboard_id = target_device_id(seat_id, configuration, InputKind::Keyboard);
        let mouse_id = target_device_id(seat_id, configuration, InputKind::Mouse);
        let routing = configuration.input_routing(seat_id);
        let current_usb = self.usb_status();
        let observation = |id: Option<&str>, kind: InputKind, strategy: InputRoutingStrategy| match (
            &current_usb,
            id,
        ) {
            (_, Some(_)) if strategy != InputRoutingStrategy::VirtualBoxUsbPassthrough => Ok(None),
            (Ok(usb), Some(id)) => Ok(Some(observe_usb_in_snapshot(
                id,
                kind,
                physical_devices,
                usb,
                record.attached.iter().any(|item| {
                    item.kind == kind
                        && item.diagnostic.attach_command == UsbAttachCommandStatus::Success
                }),
            ))),
            (Err(error), Some(_)) => Err(error.clone()),
            (_, None) => Ok(None),
        };
        let mut keyboard = component_status(
            keyboard_id,
            InputKind::Keyboard,
            &record,
            physical_devices,
            observation(keyboard_id, InputKind::Keyboard, routing.keyboard),
            routing.keyboard,
            keyboard_id.and_then(|id| {
                configuration.device_safety(id, DeviceRoutingBackend::VirtualBoxUsbPassthrough)
            }),
        );
        if routing.keyboard == InputRoutingStrategy::NativeKeyboardRouting {
            keyboard.routing_status = record.keyboard_routing_status;
        }
        let mouse = component_status(
            mouse_id,
            InputKind::Mouse,
            &record,
            physical_devices,
            observation(mouse_id, InputKind::Mouse, routing.mouse),
            routing.mouse,
            mouse_id.and_then(|id| {
                configuration.device_safety(id, DeviceRoutingBackend::VirtualBoxUsbPassthrough)
            }),
        );
        let (mouse_capture_policy, mouse_capture_policy_error) = match backend
            .filter(|item| item.managed_by_multiseat)
            .map(|item| self.mouse_capture_policy(&item.vm_id))
        {
            Some(Ok(value)) => (value, None),
            Some(Err(error)) => (None, Some(error)),
            None => (None, None),
        };
        let focus = self.focus_protection.status();
        let display_presentation = self.presentation.status();
        let host = configuration.seats.first();
        let dennis_keyboard_attached = host
            .and_then(|seat| seat.devices.keyboard.as_ref())
            .is_some_and(|id| {
                current_usb.as_ref().ok().is_some_and(|usb| {
                    observe_usb_in_snapshot(
                        &id.0,
                        InputKind::Keyboard,
                        physical_devices,
                        usb,
                        false,
                    )
                    .health
                        == UsbRuntimeHealth::Captured
                })
            });
        let dennis_mouse_attached = host
            .and_then(|seat| seat.devices.mouse.as_ref())
            .is_some_and(|id| {
                current_usb.as_ref().ok().is_some_and(|usb| {
                    observe_usb_in_snapshot(&id.0, InputKind::Mouse, physical_devices, usb, false)
                        .health
                        == UsbRuntimeHealth::Captured
                })
            });
        let mouse_capture_disabled = mouse_capture_policy
            .as_deref()
            .is_some_and(|policy| policy.eq_ignore_ascii_case("Disabled"));
        let managed = backend.is_some_and(|item| item.managed_by_multiseat);
        let mouse_integration = record.mouse_integration.clone();
        let presentation_healthy =
            !managed || display_presentation.state == PresentationState::Presented;
        let mouse_isolation =
            if !managed || record.display_mode != ManagedSeatDisplayMode::SeatDisplayLocked {
                MouseIsolationStatus::Inactive
            } else if mouse_capture_policy_error.is_some() {
                MouseIsolationStatus::Error
            } else if mouse.attached
                && mouse_capture_disabled
                && mouse_integration.observed == MouseIntegrationState::Disabled
                && focus.active
                && focus.locked
                && !dennis_mouse_attached
            {
                MouseIsolationStatus::Active
            } else if mouse_integration.observed == MouseIntegrationState::Unknown {
                MouseIsolationStatus::Unverified
            } else {
                MouseIsolationStatus::Inactive
            };
        let isolation_healthy = !managed || mouse_isolation == MouseIsolationStatus::Active;
        let status = record.transitional.unwrap_or_else(|| {
            if vm_id.is_none() {
                SeatRuntimeStatus::NotConfigured
            } else if record.last_error.is_some() {
                SeatRuntimeStatus::Error
            } else if vm_state
                .as_deref()
                .is_some_and(|state| state.eq_ignore_ascii_case("running"))
            {
                if keyboard.routing_status == InputRoutingRuntimeStatus::Active
                    && mouse.routing_status == InputRoutingRuntimeStatus::Active
                    && !dennis_keyboard_attached
                    && !dennis_mouse_attached
                    && isolation_healthy
                    && presentation_healthy
                {
                    SeatRuntimeStatus::Running
                } else {
                    SeatRuntimeStatus::PartiallyRunning
                }
            } else {
                SeatRuntimeStatus::Stopped
            }
        });
        Ok(SeatRuntimeSnapshot {
            seat_id: seat_id.0.clone(),
            status,
            vm_id,
            vm_state,
            vm_started_by_multiseat: record.vm_started_by_multiseat,
            keyboard,
            mouse,
            display_presentation,
            startup_trace: record.startup_trace.clone(),
            input_isolation: ManagedInputIsolationStatus {
                display_mode: record.display_mode,
                mouse_capture_policy,
                mouse_capture_disabled,
                mouse_capture_policy_owned: backend
                    .is_some_and(|item| item.mouse_capture_policy_owned),
                mouse_capture_policy_error,
                mouse_integration,
                mouse_isolation,
                focus_protection_active: focus.active,
                focus_protection_locked: focus.locked,
                focus_restoration_count: focus.restoration_count,
                focus_protection_error: focus.last_error,
                dennis_keyboard_attached,
                dennis_mouse_attached,
                host_gui_keyboard_isolation: HostGuiKeyboardIsolation::BestEffort,
            },
            message: record.last_error,
            logs: record.logs,
        })
    }

    fn attach(
        &self,
        seat_id: &SeatId,
        vm_id: &str,
        kind: InputKind,
        physical: &PhysicalInputDevice,
        resolved: &ResolvedUsb,
        physical_devices: &[PhysicalInputDevice],
    ) -> Result<(), String> {
        self.log(
            seat_id,
            "usb-resolve",
            "success",
            Some(&physical.id),
            Some(&resolved.uuid),
            "current VirtualBox runtime identity resolved",
        )?;
        self.with_record(seat_id, |record| {
            record.attached.push(ManagedAttachment {
                physical_device_id: physical.id.clone(),
                runtime_uuid: resolved.uuid.clone(),
                kind,
                mapping: resolved.mapping,
                diagnostic: RuntimeUsbDiagnostic {
                    windows_physical_device: EvidencePresence::Present,
                    usb_parent: physical
                        .usb_parent_instance_id
                        .as_ref()
                        .map_or(EvidencePresence::Unknown, |_| EvidencePresence::Present),
                    virtual_box_host_device: EvidencePresence::Present,
                    resolved_usb_parent: physical.usb_parent_instance_id.clone(),
                    runtime_uuid: Some(resolved.uuid.clone()),
                    runtime_address: resolved.address.clone(),
                    state_before_attach: resolved.state.clone(),
                    attach_command: UsbAttachCommandStatus::NotRequested,
                    health: UsbRuntimeHealth::AttachRequested,
                    ..RuntimeUsbDiagnostic::default()
                },
            })
        })?;
        let attach_result = self.checked(&["controlvm", vm_id, "usbattach", &resolved.uuid]);
        self.with_record(seat_id, |record| {
            if let Some(item) = record.attached.iter_mut().find(|item| item.kind == kind) {
                item.diagnostic.attach_command = if attach_result.is_ok() {
                    UsbAttachCommandStatus::Success
                } else {
                    UsbAttachCommandStatus::Failure
                };
                if attach_result.is_err() {
                    item.diagnostic.health = UsbRuntimeHealth::Failed;
                }
            }
        })?;
        attach_result?;
        self.log(
            seat_id,
            "usb-attach",
            "success",
            Some(&physical.id),
            Some(&resolved.uuid),
            "VBoxManage accepted dynamic attachment",
        )?;
        let immediate = self.observe_usb(&physical.id, kind, physical_devices, true)?;
        self.update_attachment_observation(seat_id, kind, &immediate, true)?;
        self.executor.wait(Duration::from_millis(500));
        let retry = self.observe_usb(&physical.id, kind, physical_devices, true)?;
        self.update_attachment_observation(seat_id, kind, &retry, false)?;
        let verified = retry.health == UsbRuntimeHealth::Captured;
        self.log(
            seat_id,
            "usb-verify",
            if verified { "success" } else { "unhealthy" },
            Some(&physical.id),
            retry.runtime_uuid.as_deref(),
            if verified {
                "VirtualBox positively reports the device as captured/held"
            } else {
                "attach command succeeded without positive captured evidence"
            },
        )?;
        Ok(())
    }

    fn detach_managed(
        &self,
        seat_id: &SeatId,
        vm_id: &str,
        physical_devices: &[PhysicalInputDevice],
    ) -> Result<(), String> {
        let attachments = self.record(seat_id)?.attached;
        let mut errors = Vec::new();
        for attachment in attachments.into_iter().rev().filter(|attachment| {
            attachment.diagnostic.attach_command == UsbAttachCommandStatus::Success
        }) {
            if matches!(
                attachment.diagnostic.health,
                UsbRuntimeHealth::DisappearedAfterAttach
                    | UsbRuntimeHealth::PhysicalDeviceDisconnected
            ) {
                self.log(
                    seat_id,
                    "usb-detach",
                    "skipped",
                    Some(&attachment.physical_device_id),
                    attachment.diagnostic.runtime_uuid.as_deref(),
                    "device is no longer resolvable; VM shutdown will release any remaining ownership",
                )?;
                continue;
            }
            let Some(physical) = find_physical(
                physical_devices,
                &attachment.physical_device_id,
                attachment.kind,
            ) else {
                errors.push(format!(
                    "managed device '{}' is no longer present",
                    attachment.physical_device_id
                ));
                continue;
            };
            let current_uuid = match self.resolve_single_usb(physical) {
                Ok(resolved) => resolved.uuid,
                Err(error) => {
                    errors.push(error);
                    continue;
                }
            };
            match self.checked(&["controlvm", vm_id, "usbdetach", &current_uuid]) {
                Ok(_) => self.log(
                    seat_id,
                    "usb-detach",
                    "success",
                    Some(&attachment.physical_device_id),
                    Some(&current_uuid),
                    "detached MultiSeat Lite managed device",
                )?,
                Err(error) => errors.push(error),
            }
        }
        if errors.is_empty() {
            self.with_record(seat_id, |record| record.attached.clear())?;
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    fn rollback(
        &self,
        seat_id: &SeatId,
        vm_id: &str,
        physical_devices: &[PhysicalInputDevice],
    ) -> Vec<String> {
        let attachments = self
            .record(seat_id)
            .map(|record| record.attached)
            .unwrap_or_default();
        let mut errors = Vec::new();
        for attachment in attachments.into_iter().rev().filter(|attachment| {
            attachment.diagnostic.attach_command == UsbAttachCommandStatus::Success
        }) {
            if matches!(
                attachment.diagnostic.health,
                UsbRuntimeHealth::DisappearedAfterAttach
                    | UsbRuntimeHealth::PhysicalDeviceDisconnected
            ) {
                let _ = self.log(
                    seat_id,
                    "rollback-detach",
                    "skipped",
                    Some(&attachment.physical_device_id),
                    attachment.diagnostic.runtime_uuid.as_deref(),
                    "device disappeared after attach and has no current safe runtime identity",
                );
                continue;
            }
            let Some(physical) = find_physical(
                physical_devices,
                &attachment.physical_device_id,
                attachment.kind,
            ) else {
                errors.push(format!(
                    "rollback device '{}' is no longer present",
                    attachment.physical_device_id
                ));
                continue;
            };
            let current_uuid = match self.resolve_single_usb(physical) {
                Ok(resolved) => resolved.uuid,
                Err(error) => {
                    errors.push(error);
                    continue;
                }
            };
            match self.checked(&["controlvm", vm_id, "usbdetach", &current_uuid]) {
                Ok(_) => {
                    let _ = self.log(
                        seat_id,
                        "rollback-detach",
                        "success",
                        Some(&attachment.physical_device_id),
                        Some(&current_uuid),
                        "rolled back attachment",
                    );
                }
                Err(error) => errors.push(error),
            }
        }
        let _ = self.with_record(seat_id, |record| record.attached.clear());
        errors
    }

    fn resolve_required_usb(
        &self,
        inputs: &ActivationInputs<'_>,
        routing: &crate::seats::SeatInputRouting,
        configuration: &ApplicationConfig,
    ) -> Result<(), String> {
        let status = self.usb_status()?;
        for (label, physical, strategy, keyboard_capable) in [
            ("Barnen keyboard", inputs.keyboard, routing.keyboard, true),
            ("Barnen mouse", inputs.mouse, routing.mouse, false),
        ] {
            if strategy != InputRoutingStrategy::VirtualBoxUsbPassthrough {
                continue;
            }
            ensure_usb_passthrough_allowed(configuration, physical, keyboard_capable)?;
            let mapping = correlate_usb_devices(&[physical.clone()], &status).remove(0);
            acceptable_uuid(&mapping, label)?;
        }
        Ok(())
    }

    fn resolve_single_usb(&self, physical: &PhysicalInputDevice) -> Result<ResolvedUsb, String> {
        let status = self.usb_status()?;
        let mapping = correlate_usb_devices(&[physical.clone()], &status).remove(0);
        let uuid = acceptable_uuid(&mapping, &physical.id)?;
        let device = status.usb_devices.iter().find(|device| device.uuid == uuid);
        Ok(ResolvedUsb {
            uuid,
            address: device.and_then(|device| device.address.clone()),
            state: device.and_then(|device| device.current_state.clone()),
            mapping: mapping.state,
        })
    }

    fn observe_usb(
        &self,
        physical_id: &str,
        kind: InputKind,
        fallback_devices: &[PhysicalInputDevice],
        after_attach: bool,
    ) -> Result<UsbObservation, String> {
        let refreshed_devices = if self.refresh_physical_hardware {
            crate::enumerate_hardware()?.input_devices
        } else {
            fallback_devices.to_vec()
        };
        let status = self.usb_status()?;
        Ok(observe_usb_in_snapshot(
            physical_id,
            kind,
            &refreshed_devices,
            &status,
            after_attach,
        ))
    }

    fn update_attachment_observation(
        &self,
        seat_id: &SeatId,
        kind: InputKind,
        observation: &UsbObservation,
        immediate: bool,
    ) -> Result<(), String> {
        self.with_record(seat_id, |record| {
            if let Some(item) = record.attached.iter_mut().find(|item| item.kind == kind) {
                if let Some(uuid) = &observation.runtime_uuid {
                    item.runtime_uuid = uuid.clone();
                }
                if let Some(mapping) = observation.mapping {
                    item.mapping = mapping;
                }
                item.diagnostic.windows_physical_device = observation.windows_physical_device;
                item.diagnostic.usb_parent = observation.usb_parent;
                item.diagnostic.virtual_box_host_device = observation.virtual_box_host_device;
                item.diagnostic.resolved_usb_parent = observation.resolved_usb_parent.clone();
                item.diagnostic.runtime_uuid = observation.runtime_uuid.clone();
                item.diagnostic.runtime_address = observation.runtime_address.clone();
                item.diagnostic.current_virtual_box_state = observation.current_state.clone();
                if immediate {
                    item.diagnostic.state_immediately_after_attach =
                        observation.current_state.clone();
                } else {
                    item.diagnostic.state_after_retry = observation.current_state.clone();
                }
                item.diagnostic.health = observation.health;
            }
        })
    }

    fn usb_status(&self) -> Result<VirtualBoxStatus, String> {
        let output = self.checked(&["list", "usbhost"])?;
        let parsed = parse_virtualbox_usbhost(&output.stdout);
        Ok(VirtualBoxStatus {
            installed: true,
            vbox_manage_path: None,
            version: None,
            operational: true,
            probe_error: None,
            parser_warnings: parsed.warnings,
            usb_devices: parsed.devices,
        })
    }

    fn wait_for_vm_state(&self, vm_id: &str, desired: &str, attempts: usize) -> Result<(), String> {
        self.wait_for_vm_state_cancellable(
            vm_id,
            desired,
            attempts,
            &Arc::new(AtomicBool::new(false)),
        )
    }

    fn wait_for_vm_state_cancellable(
        &self,
        vm_id: &str,
        desired: &str,
        attempts: usize,
        cancellation: &Arc<AtomicBool>,
    ) -> Result<(), String> {
        for _ in 0..attempts {
            self.check_start_cancelled(cancellation)?;
            if self.inspect_vm(vm_id)?.state.eq_ignore_ascii_case(desired) {
                return Ok(());
            }
            self.executor.wait(Duration::from_millis(500));
        }
        Err(format!(
            "VM {vm_id} did not reach state '{desired}' in time"
        ))
    }

    fn check_start_cancelled(&self, cancellation: &Arc<AtomicBool>) -> Result<(), String> {
        if cancellation.load(Ordering::Acquire) {
            Err("managed seat startup was cancelled".to_owned())
        } else {
            Ok(())
        }
    }

    fn checked(&self, arguments: &[&str]) -> Result<VBoxOutput, String> {
        self.checked_redacted(arguments, &[])
    }

    /// Executes VBoxManage while ensuring credentials and product keys never
    /// appear in diagnostic errors. `sensitive_arguments` contains complete
    /// argument values (for example `--key=...`) that must be redacted.
    fn checked_redacted(
        &self,
        arguments: &[&str],
        sensitive_arguments: &[&str],
    ) -> Result<VBoxOutput, String> {
        let arguments: Vec<String> = arguments.iter().map(|value| (*value).to_owned()).collect();
        let output = self.executor.execute(&arguments).map_err(|message| {
            sensitive_arguments
                .iter()
                .fold(message, |sanitized, sensitive| {
                    if sensitive.is_empty() {
                        sanitized
                    } else {
                        sanitized.replace(sensitive, "<redacted>")
                    }
                })
        })?;
        if output.success {
            Ok(output)
        } else {
            let redacted_arguments: Vec<&str> = arguments
                .iter()
                .map(|argument| {
                    if sensitive_arguments.contains(&argument.as_str()) {
                        "<redacted>"
                    } else {
                        argument.as_str()
                    }
                })
                .collect();
            let mut stderr = output.stderr.trim().to_owned();
            for sensitive in sensitive_arguments {
                if !sensitive.is_empty() {
                    stderr = stderr.replace(sensitive, "<redacted>");
                }
            }
            Err(format!(
                "VBoxManage {:?} failed with exit code {:?}: {}",
                redacted_arguments, output.exit_code, stderr
            ))
        }
    }

    fn replace_record(&self, seat_id: &SeatId, record: RuntimeRecord) -> Result<(), String> {
        self.records
            .lock()
            .map_err(|_| "VirtualBox runtime lock is poisoned".to_owned())?
            .insert(seat_id.0.clone(), record);
        Ok(())
    }

    fn with_record(
        &self,
        seat_id: &SeatId,
        operation: impl FnOnce(&mut RuntimeRecord),
    ) -> Result<(), String> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| "VirtualBox runtime lock is poisoned".to_owned())?;
        operation(records.entry(seat_id.0.clone()).or_default());
        Ok(())
    }

    fn record(&self, seat_id: &SeatId) -> Result<RuntimeRecord, String> {
        Ok(self
            .records
            .lock()
            .map_err(|_| "VirtualBox runtime lock is poisoned".to_owned())?
            .get(&seat_id.0)
            .cloned()
            .unwrap_or_default())
    }

    fn log(
        &self,
        seat_id: &SeatId,
        action: &str,
        outcome: &str,
        physical: Option<&str>,
        uuid: Option<&str>,
        detail: &str,
    ) -> Result<(), String> {
        self.with_record(seat_id, |record| {
            record.logs.push(RuntimeLogEntry {
                action: action.to_owned(),
                outcome: outcome.to_owned(),
                physical_device_id: physical.map(str::to_owned),
                runtime_uuid: uuid.map(str::to_owned),
                detail: detail.to_owned(),
            })
        })
    }

    fn trace(
        &self,
        seat_id: &SeatId,
        stage: &str,
        detail: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), String> {
        self.trace_detailed(seat_id, stage, detail, error, None, None)
    }

    fn trace_detailed(
        &self,
        seat_id: &SeatId,
        stage: &str,
        detail: Option<&str>,
        error: Option<&str>,
        duration: Option<Duration>,
        exit_status: Option<i32>,
    ) -> Result<(), String> {
        self.with_record(seat_id, |record| {
            let started = record.startup_started.get_or_insert_with(Instant::now);
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let timestamp_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|value| u64::try_from(value.as_millis()).ok())
                .unwrap_or_default();
            if record.startup_trace.len() >= 128 {
                record.startup_trace.remove(0);
            }
            record.startup_trace.push(StartupTraceEntry {
                stage: stage.to_owned(),
                timestamp_ms,
                elapsed_ms,
                duration_ms: duration.and_then(|value| u64::try_from(value.as_millis()).ok()),
                detail: detail.map(str::to_owned),
                error: error.map(str::to_owned),
                process_id: None,
                window_handle: None,
                target_display_id: None,
                target_bounds: None,
                exit_status,
            });
        })
    }
}

#[cfg(test)]
mod runtime_tests {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use std::time::{Duration, Instant};

    use crate::{
        devices::InputCapabilities,
        seats::{
            DeviceBackendSafetyRecord, DeviceRoutingBackend, DisplayId, PhysicalDeviceId,
            SeatInputRouting, UsbPassthroughSafety, VirtualBoxSeatConfig,
        },
    };

    use super::*;

    #[test]
    fn asynchronous_start_is_accepted_immediately_and_duplicate_is_refused() {
        let fake = FakeExecutor::default();
        let service = service(&fake);
        let seat_id = SeatId("seat-2".into());
        let started = Instant::now();
        let (cancellation, snapshot) = service
            .begin_asynchronous_start(&seat_id, &configuration())
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(snapshot.status, SeatRuntimeStatus::Starting);
        assert_eq!(snapshot.vm_state.as_deref(), Some("start-requested"));
        assert!(snapshot
            .startup_trace
            .iter()
            .any(|entry| entry.stage == "StartCommandReturned"));
        assert!(service
            .begin_asynchronous_start(&seat_id, &configuration())
            .unwrap_err()
            .contains("already starting"));
        service.cancel_asynchronous_start(&seat_id);
        assert!(cancellation.load(Ordering::Acquire));
        service.finish_asynchronous_start(&seat_id);
    }

    #[test]
    fn startup_trace_is_bounded() {
        let fake = FakeExecutor::default();
        let service = service(&fake);
        let seat_id = SeatId("seat-2".into());
        for index in 0..140 {
            service
                .record_startup_stage(
                    &seat_id,
                    "WindowDiscoveryAttempt",
                    Some(&index.to_string()),
                    None,
                )
                .unwrap();
        }
        let record = service.record(&seat_id).unwrap();
        assert_eq!(record.startup_trace.len(), 128);
        assert_eq!(
            record.startup_trace.first().unwrap().detail.as_deref(),
            Some("12")
        );
    }

    #[test]
    fn keyboard_and_mouse_can_be_active_while_presentation_is_still_waiting() {
        let fake = FakeExecutor::default();
        let service = VirtualBoxRuntimeService::with_components(
            Box::new(fake.clone()),
            Box::new(focus_protection::SimulatedFocusProtection::default()),
            mouse_integration::simulated_backend(),
            Box::new(WaitingPresentation::default()),
        );
        let config = configuration();
        service
            .start(&SeatId("seat-2".into()), &config, &devices(), None)
            .unwrap();
        service
            .set_keyboard_routing_runtime(
                &SeatId("seat-2".into()),
                InputRoutingRuntimeStatus::Active,
                "test router active",
            )
            .unwrap();
        let status = service
            .status(&SeatId("seat-2".into()), &config, &devices())
            .unwrap();
        assert_eq!(
            status.display_presentation.state,
            PresentationState::WaitingForVmWindow
        );
        assert_eq!(
            status.keyboard.routing_status,
            InputRoutingRuntimeStatus::Active
        );
        assert_eq!(
            status.mouse.routing_status,
            InputRoutingRuntimeStatus::Active
        );
        assert_eq!(status.status, SeatRuntimeStatus::PartiallyRunning);
    }

    #[derive(Default)]
    struct FakeState {
        calls: Mutex<Vec<Vec<String>>>,
        usb_calls: AtomicUsize,
        attach_calls: AtomicUsize,
        running: AtomicBool,
        fail_mouse_attach: AtomicBool,
        ambiguous_keyboard: AtomicBool,
        keyboard_disappears_after_attach: AtomicBool,
        all_captured: AtomicBool,
        mouse_capture_policy: Mutex<String>,
    }

    #[derive(Clone, Default)]
    struct FakeExecutor(Arc<FakeState>);

    #[derive(Default)]
    struct WaitingPresentation(Mutex<DisplayPresentationStatus>);

    impl PresentationBackend for WaitingPresentation {
        fn start(
            &self,
            _target: ManagedVmWindowIdentity,
            assigned_display_id: String,
        ) -> Result<(), String> {
            *self.0.lock().unwrap() = DisplayPresentationStatus {
                state: PresentationState::WaitingForVmWindow,
                assigned_display_id: Some(assigned_display_id),
                ..DisplayPresentationStatus::default()
            };
            Ok(())
        }

        fn set_locked(&self, locked: bool) -> Result<(), String> {
            self.0.lock().unwrap().state = if locked {
                PresentationState::WaitingForVmWindow
            } else {
                PresentationState::NotPresented
            };
            Ok(())
        }

        fn mark_unavailable(
            &self,
            assigned_display_id: Option<String>,
            message: String,
        ) -> Result<(), String> {
            *self.0.lock().unwrap() = DisplayPresentationStatus {
                state: PresentationState::PresentationUnavailable,
                assigned_display_id,
                last_error: Some(message),
                ..DisplayPresentationStatus::default()
            };
            Ok(())
        }

        fn stop(&self) -> Result<(), String> {
            *self.0.lock().unwrap() = DisplayPresentationStatus::default();
            Ok(())
        }

        fn status(&self) -> DisplayPresentationStatus {
            self.0.lock().unwrap().clone()
        }
    }

    impl VBoxManageExecutor for FakeExecutor {
        fn execute(&self, arguments: &[String]) -> Result<VBoxOutput, String> {
            self.0.calls.lock().unwrap().push(arguments.to_vec());
            let args: Vec<&str> = arguments.iter().map(String::as_str).collect();
            let output = match args.as_slice() {
                ["showvminfo", _, "--machinereadable"] => format!(
                    "name=\"Windows - Barnen\"\nUUID=\"vm-1\"\nostype=\"Windows11_64\"\nVMState=\"{}\"\n",
                    if self.0.running.load(Ordering::SeqCst) { "running" } else { "poweroff" }
                ),
                ["list", "usbhost"] => {
                    let generation = self.0.usb_calls.fetch_add(1, Ordering::SeqCst) + 1;
                    usb_output(
                        generation,
                        self.0.attach_calls.load(Ordering::SeqCst),
                        self.0.ambiguous_keyboard.load(Ordering::SeqCst),
                        self.0.all_captured.load(Ordering::SeqCst),
                        self.0.keyboard_disappears_after_attach.load(Ordering::SeqCst),
                    )
                }
                ["startvm", ..] => {
                    self.0.running.store(true, Ordering::SeqCst);
                    String::new()
                }
                ["controlvm", _, "usbattach", uuid] => {
                    if uuid.starts_with("mouse-") && self.0.fail_mouse_attach.load(Ordering::SeqCst) {
                        return Ok(VBoxOutput { success: false, exit_code: Some(1), stdout: String::new(), stderr: "mouse attach failed".into() });
                    }
                    self.0.attach_calls.fetch_add(1, Ordering::SeqCst);
                    String::new()
                }
                ["controlvm", _, "acpipowerbutton"] => {
                    self.0.running.store(false, Ordering::SeqCst);
                    String::new()
                }
                ["controlvm", _, "shutdown"] => {
                    self.0.running.store(false, Ordering::SeqCst);
                    String::new()
                }
                ["controlvm", _, "usbdetach", _] => String::new(),
                ["setextradata", _, "GUI/MouseCapturePolicy", policy] => {
                    *self.0.mouse_capture_policy.lock().unwrap() = (*policy).to_owned();
                    String::new()
                }
                ["getextradata", _, "GUI/MouseCapturePolicy"] => format!(
                    "Value: {}\n",
                    self.0.mouse_capture_policy.lock().unwrap()
                ),
                _ => String::new(),
            };
            Ok(VBoxOutput {
                success: true,
                exit_code: Some(0),
                stdout: output,
                stderr: String::new(),
            })
        }

        fn wait(&self, _duration: Duration) {}
    }

    fn usb_output(
        generation: usize,
        attached: usize,
        ambiguous: bool,
        all_captured: bool,
        keyboard_disappears_after_attach: bool,
    ) -> String {
        let keyboard_state = if all_captured || attached >= 1 {
            "Captured"
        } else {
            "Busy"
        };
        let mouse_state = if all_captured || attached >= 1 {
            "Captured"
        } else {
            "Busy"
        };
        let duplicate = if ambiguous {
            format!("\nUUID: keyboard-duplicate-{generation}\nVendorId: 0x1a2c (1A2C)\nProductId: 0x4c5e (4C5E)\nCurrent State: Busy\n")
        } else {
            String::new()
        };
        let keyboard = if keyboard_disappears_after_attach && attached >= 1 {
            String::new()
        } else {
            format!("UUID: keyboard-{generation}\nVendorId: 0x1a2c (1A2C)\nProductId: 0x4c5e (4C5E)\nCurrent State: {keyboard_state}\n{duplicate}\n")
        };
        format!(
            "Host USB Devices:\n\n{keyboard}\nUUID: mouse-{generation}\nVendorId: 0x30fa (30FA)\nProductId: 0x1440 (1440)\nCurrent State: {mouse_state}\n\nUUID: dennis-{generation}\nVendorId: 0x1b1c (1B1C)\nProductId: 0x1b31 (1B31)\nCurrent State: Busy\n"
        )
    }

    fn device(
        id: &str,
        vendor: &str,
        product: &str,
        keyboard: bool,
        mouse: bool,
    ) -> PhysicalInputDevice {
        PhysicalInputDevice {
            id: id.into(),
            container_id: Some(id.into()),
            friendly_name: Some(id.into()),
            manufacturer: None,
            vendor_id: Some(vendor.into()),
            product_id: Some(product.into()),
            usb_parent_instance_id: None,
            serial_number: None,
            capabilities: InputCapabilities { keyboard, mouse },
            logical_nodes: Vec::new(),
            raw_input_paths: Vec::new(),
            present: true,
        }
    }

    fn devices() -> Vec<PhysicalInputDevice> {
        vec![
            device("dennis-keyboard", "1532", "021F", true, false),
            device("dennis-mouse", "1B1C", "1B31", false, true),
            device("barnen-keyboard", "1A2C", "4C5E", true, false),
            device("barnen-mouse", "30FA", "1440", false, true),
        ]
    }

    fn configuration() -> ApplicationConfig {
        let mut config = ApplicationConfig::default();
        config.seats[0].devices.keyboard = Some(PhysicalDeviceId("dennis-keyboard".into()));
        config.seats[0].devices.mouse = Some(PhysicalDeviceId("dennis-mouse".into()));
        config.seats[1].devices.keyboard = Some(PhysicalDeviceId("barnen-keyboard".into()));
        config.seats[1].devices.mouse = Some(PhysicalDeviceId("barnen-mouse".into()));
        config.seats[1].devices.display = Some(DisplayId("barnen-display".into()));
        config.backends.virtual_box.seats.insert(
            "seat-2".into(),
            VirtualBoxSeatConfig {
                vm_id: "vm-1".into(),
                managed_by_multiseat: true,
                ..Default::default()
            },
        );
        config
    }

    fn configuration_with_cleared_keyboard_usb() -> ApplicationConfig {
        let mut config = configuration();
        config
            .set_input_routing(
                &SeatId("seat-2".into()),
                SeatInputRouting {
                    keyboard: InputRoutingStrategy::VirtualBoxUsbPassthrough,
                    mouse: InputRoutingStrategy::VirtualBoxUsbPassthrough,
                },
            )
            .unwrap();
        config
            .record_device_safety(DeviceBackendSafetyRecord {
                physical_device_id: PhysicalDeviceId("barnen-keyboard".into()),
                vendor_id: Some("1A2C".into()),
                product_id: Some("4C5E".into()),
                backend: DeviceRoutingBackend::VirtualBoxUsbPassthrough,
                safety: UsbPassthroughSafety::Supported,
                reason: "test fixture explicitly cleared".into(),
                observed: None,
            })
            .unwrap();
        config
    }

    fn service(fake: &FakeExecutor) -> VirtualBoxRuntimeService {
        VirtualBoxRuntimeService::new(Box::new(fake.clone()))
    }

    fn calls(fake: &FakeExecutor, operation: &str) -> Vec<Vec<String>> {
        fake.0
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|args| args.iter().any(|arg| arg == operation))
            .cloned()
            .collect()
    }

    #[test]
    fn current_mouse_usb_uuid_is_resolved_each_activation_and_stale_uuid_is_not_reused() {
        let fake = FakeExecutor::default();
        let service = service(&fake);
        let config = configuration();
        service
            .start(&SeatId("seat-2".into()), &config, &devices(), None)
            .unwrap();
        service
            .start(&SeatId("seat-2".into()), &config, &devices(), None)
            .unwrap();
        let attaches = calls(&fake, "usbattach");
        let mouse_uuids: Vec<_> = attaches
            .iter()
            .filter_map(|args| args.last())
            .filter(|uuid| uuid.starts_with("mouse-"))
            .collect();
        assert_eq!(mouse_uuids.len(), 2);
        assert_ne!(mouse_uuids[0], mouse_uuids[1]);
        assert!(!attaches
            .iter()
            .any(|args| args.last().unwrap() == "mouse-1"));
    }

    #[test]
    fn missing_barnen_keyboard_or_mouse_blocks_start() {
        for clear_keyboard in [true, false] {
            let fake = FakeExecutor::default();
            let service = service(&fake);
            let mut config = configuration();
            if clear_keyboard {
                config.seats[1].devices.keyboard = None;
            } else {
                config.seats[1].devices.mouse = None;
            }
            assert!(service
                .start(&SeatId("seat-2".into()), &config, &devices(), None)
                .is_err());
            assert!(calls(&fake, "startvm").is_empty());
        }
    }

    #[test]
    fn ambiguous_usb_mapping_blocks_start() {
        let fake = FakeExecutor::default();
        fake.0.ambiguous_keyboard.store(true, Ordering::SeqCst);
        let service = service(&fake);
        let config = configuration_with_cleared_keyboard_usb();
        assert!(service
            .start(&SeatId("seat-2".into()), &config, &devices(), None)
            .unwrap_err()
            .contains("safely"));
        assert!(calls(&fake, "startvm").is_empty());
    }

    #[test]
    fn dennis_input_cannot_be_attached() {
        let fake = FakeExecutor::default();
        let service = service(&fake);
        let mut config = configuration();
        config.seats[1].devices.keyboard = Some(PhysicalDeviceId("dennis-keyboard".into()));
        assert!(service
            .start(&SeatId("seat-2".into()), &config, &devices(), None)
            .unwrap_err()
            .contains("Dennis"));
        assert!(calls(&fake, "usbattach").is_empty());
    }

    #[test]
    fn native_keyboard_prototype_and_captured_mouse_are_partially_running() {
        let fake = FakeExecutor::default();
        let service = service(&fake);
        let status = service
            .start(&SeatId("seat-2".into()), &configuration(), &devices(), None)
            .unwrap();
        assert_eq!(status.status, SeatRuntimeStatus::PartiallyRunning);
        assert!(!status.keyboard.attached && status.mouse.attached);
        assert_eq!(
            status.keyboard.routing_strategy,
            InputRoutingStrategy::NativeKeyboardRouting
        );
        assert_eq!(
            status.keyboard.routing_status,
            InputRoutingRuntimeStatus::PrototypeInactive
        );
        assert!(calls(&fake, "usbattach").iter().all(|args| !args
            .last()
            .is_some_and(|uuid| uuid.starts_with("keyboard-"))));
        assert!(status.input_isolation.mouse_capture_disabled);
        assert_eq!(
            status.input_isolation.mouse_integration.observed,
            MouseIntegrationState::Disabled
        );
        assert_eq!(
            status.input_isolation.mouse_isolation,
            MouseIsolationStatus::Active
        );
        assert!(status.input_isolation.focus_protection_active);
        assert_eq!(
            status.input_isolation.display_mode,
            ManagedSeatDisplayMode::SeatDisplayLocked
        );
        assert!(!status.input_isolation.dennis_keyboard_attached);
        assert!(!status.input_isolation.dennis_mouse_attached);
    }

    #[test]
    fn successful_attach_without_vbox_device_is_disappeared_not_attached() {
        let fake = FakeExecutor::default();
        fake.0
            .keyboard_disappears_after_attach
            .store(true, Ordering::SeqCst);
        let service = service(&fake);
        let status = service
            .start(
                &SeatId("seat-2".into()),
                &configuration_with_cleared_keyboard_usb(),
                &devices(),
                None,
            )
            .unwrap();
        assert_eq!(status.status, SeatRuntimeStatus::PartiallyRunning);
        assert!(!status.keyboard.attached);
        assert_eq!(
            status.keyboard.usb.health,
            UsbRuntimeHealth::DisappearedAfterAttach
        );
        assert_eq!(
            status.keyboard.usb.windows_physical_device,
            EvidencePresence::Present
        );
        assert_eq!(
            status.keyboard.usb.virtual_box_host_device,
            EvidencePresence::Missing
        );
        assert_eq!(
            status.keyboard.usb.attach_command,
            UsbAttachCommandStatus::Success
        );
        assert_eq!(
            status.keyboard.usb.guest_visibility,
            GuestUsbVisibility::Unknown
        );
        assert!(status.mouse.attached);
        assert_eq!(status.mouse.usb.health, UsbRuntimeHealth::Captured);
    }

    #[test]
    fn known_problematic_keyboard_usb_is_blocked_before_vm_start_or_attach() {
        let fake = FakeExecutor::default();
        let service = service(&fake);
        let mut config = configuration();
        config
            .set_input_routing(
                &SeatId("seat-2".into()),
                SeatInputRouting {
                    keyboard: InputRoutingStrategy::VirtualBoxUsbPassthrough,
                    mouse: InputRoutingStrategy::VirtualBoxUsbPassthrough,
                },
            )
            .unwrap();
        config
            .record_device_safety(DeviceBackendSafetyRecord {
                physical_device_id: PhysicalDeviceId("barnen-keyboard".into()),
                vendor_id: Some("1A2C".into()),
                product_id: Some("4C5E".into()),
                backend: DeviceRoutingBackend::VirtualBoxUsbPassthrough,
                safety: UsbPassthroughSafety::KnownProblematic,
                reason: "host PNP_DETECTED_FATAL_ERROR during VirtualBox USB capture".into(),
                observed: Some("0xCA / Arg1 0x2 / VBoxUSBMon.sys".into()),
            })
            .unwrap();

        let error = service
            .start(&SeatId("seat-2".into()), &config, &devices(), None)
            .unwrap_err();
        assert!(error.contains("disabled"));
        assert!(error.contains("Automatic retry is disabled"));
        assert!(calls(&fake, "startvm").is_empty());
        assert!(calls(&fake, "usbattach").is_empty());
    }

    #[test]
    fn keyboard_usb_requires_explicit_supported_clearance() {
        let fake = FakeExecutor::default();
        let service = service(&fake);
        let mut config = configuration();
        config
            .set_input_routing(
                &SeatId("seat-2".into()),
                SeatInputRouting {
                    keyboard: InputRoutingStrategy::VirtualBoxUsbPassthrough,
                    mouse: InputRoutingStrategy::VirtualBoxUsbPassthrough,
                },
            )
            .unwrap();

        let error = service
            .start(&SeatId("seat-2".into()), &config, &devices(), None)
            .unwrap_err();
        assert!(error.contains("not explicitly cleared"));
        assert!(calls(&fake, "startvm").is_empty());
        assert!(calls(&fake, "usbattach").is_empty());
    }

    #[test]
    fn missing_windows_device_is_distinct_from_vbox_enumeration_loss() {
        let status = VirtualBoxStatus {
            installed: true,
            vbox_manage_path: None,
            version: None,
            operational: true,
            probe_error: None,
            parser_warnings: Vec::new(),
            usb_devices: Vec::new(),
        };
        let observation =
            observe_usb_in_snapshot("barnen-keyboard", InputKind::Keyboard, &[], &status, true);
        assert_eq!(
            observation.health,
            UsbRuntimeHealth::PhysicalDeviceDisconnected
        );
        assert_eq!(
            observation.windows_physical_device,
            EvidencePresence::Missing
        );
    }

    #[test]
    fn managed_start_sets_and_verifies_mouse_capture_policy_only_on_managed_vm() {
        let fake = FakeExecutor::default();
        let service = service(&fake);
        service
            .start(&SeatId("seat-2".into()), &configuration(), &devices(), None)
            .unwrap();
        let settings = calls(&fake, "setextradata");
        assert_eq!(settings.len(), 1);
        assert_eq!(settings[0][1], "vm-1");
        assert_eq!(settings[0].last().map(String::as_str), Some("Disabled"));

        let mut unmanaged = configuration();
        unmanaged
            .backends
            .virtual_box
            .seats
            .get_mut("seat-2")
            .unwrap()
            .managed_by_multiseat = false;
        assert!(!service
            .ensure_managed_mouse_capture_disabled(&SeatId("seat-2".into()), &unmanaged)
            .unwrap());
        assert_eq!(calls(&fake, "setextradata").len(), 1);
    }

    #[test]
    fn admin_unlock_is_temporary_and_does_not_change_usb_health() {
        let fake = FakeExecutor::default();
        let service = service(&fake);
        let config = configuration_with_cleared_keyboard_usb();
        service
            .start(&SeatId("seat-2".into()), &config, &devices(), None)
            .unwrap();
        let unlocked = service
            .allow_host_input_temporarily(&SeatId("seat-2".into()), &config, &devices())
            .unwrap();
        assert_eq!(
            unlocked.input_isolation.display_mode,
            ManagedSeatDisplayMode::NormalVirtualBox
        );
        assert_eq!(
            unlocked.input_isolation.mouse_integration.observed,
            MouseIntegrationState::Enabled
        );
        assert_eq!(
            unlocked.input_isolation.mouse_isolation,
            MouseIsolationStatus::Inactive
        );
        assert!(!unlocked.input_isolation.focus_protection_locked);
        assert_eq!(
            unlocked.display_presentation.mode,
            PresentationMode::ConventionalWindow
        );
        assert_eq!(
            unlocked.display_presentation.state,
            PresentationState::NotPresented
        );
        assert!(unlocked.keyboard.attached && unlocked.mouse.attached);

        let locked = service
            .lock_seat_input(&SeatId("seat-2".into()), &config, &devices(), None)
            .unwrap();
        assert!(locked.input_isolation.mouse_capture_disabled);
        assert!(locked.input_isolation.focus_protection_locked);
        assert_eq!(
            locked.input_isolation.mouse_integration.observed,
            MouseIntegrationState::Disabled
        );
        assert_eq!(
            locked.input_isolation.mouse_isolation,
            MouseIsolationStatus::Active
        );
        assert_eq!(
            locked.display_presentation.mode,
            PresentationMode::Borderless
        );
        assert_eq!(
            locked.display_presentation.state,
            PresentationState::Presented
        );

        service
            .allow_host_input_temporarily(&SeatId("seat-2".into()), &config, &devices())
            .unwrap();
        let restarted = service
            .start(&SeatId("seat-2".into()), &config, &devices(), None)
            .unwrap();
        assert!(restarted.input_isolation.mouse_capture_disabled);
        assert!(restarted.input_isolation.focus_protection_locked);
        assert_eq!(
            restarted.display_presentation.state,
            PresentationState::Presented
        );
    }

    #[test]
    fn second_attachment_failure_rolls_back_first() {
        let fake = FakeExecutor::default();
        fake.0.fail_mouse_attach.store(true, Ordering::SeqCst);
        let service = service(&fake);
        assert!(service
            .start(
                &SeatId("seat-2".into()),
                &configuration_with_cleared_keyboard_usb(),
                &devices(),
                None,
            )
            .is_err());
        let detaches = calls(&fake, "usbdetach");
        assert_eq!(detaches.len(), 1);
        assert!(detaches[0].last().unwrap().starts_with("keyboard-"));
    }

    #[test]
    fn already_running_vm_is_not_claimed_by_activation() {
        let fake = FakeExecutor::default();
        fake.0.running.store(true, Ordering::SeqCst);
        let service = service(&fake);
        let status = service
            .start(&SeatId("seat-2".into()), &configuration(), &devices(), None)
            .unwrap();
        assert!(!status.vm_started_by_multiseat);
        assert!(calls(&fake, "startvm").is_empty());
    }

    #[test]
    fn stop_detaches_only_multiseat_managed_devices() {
        let fake = FakeExecutor::default();
        let service = service(&fake);
        let config = configuration_with_cleared_keyboard_usb();
        service
            .start(&SeatId("seat-2".into()), &config, &devices(), None)
            .unwrap();
        service
            .stop(&SeatId("seat-2".into()), &config, &devices())
            .unwrap();
        let detached: Vec<_> = calls(&fake, "usbdetach")
            .into_iter()
            .map(|args| args.last().unwrap().clone())
            .collect();
        assert_eq!(detached.len(), 2);
        assert!(detached.iter().any(|uuid| uuid.starts_with("keyboard-")));
        assert!(detached.iter().any(|uuid| uuid.starts_with("mouse-")));
        assert!(!detached.iter().any(|uuid| uuid.starts_with("dennis-")));
    }

    #[test]
    fn stop_remains_possible_when_keyboard_disappeared_after_attach() {
        let fake = FakeExecutor::default();
        fake.0
            .keyboard_disappears_after_attach
            .store(true, Ordering::SeqCst);
        let service = service(&fake);
        let config = configuration();
        service
            .start(&SeatId("seat-2".into()), &config, &devices(), None)
            .unwrap();
        let stopped = service
            .stop(&SeatId("seat-2".into()), &config, &devices())
            .unwrap();
        assert_eq!(stopped.status, SeatRuntimeStatus::Stopped);
        let detached = calls(&fake, "usbdetach");
        assert_eq!(detached.len(), 1);
        assert!(detached[0].last().unwrap().starts_with("mouse-"));
    }

    #[test]
    fn release_reenumerates_and_targets_only_configured_barnen_devices() {
        let fake = FakeExecutor::default();
        fake.0.running.store(true, Ordering::SeqCst);
        fake.0.all_captured.store(true, Ordering::SeqCst);
        let service = service(&fake);
        service
            .release(&SeatId("seat-2".into()), &configuration(), &devices())
            .unwrap();
        let detached: Vec<_> = calls(&fake, "usbdetach")
            .into_iter()
            .map(|args| args.last().unwrap().clone())
            .collect();
        assert_eq!(detached, vec!["mouse-1"]);
        assert!(!detached.iter().any(|uuid| uuid.starts_with("dennis-")));
        assert!(!detached.iter().any(|uuid| uuid.starts_with("keyboard-")));
    }

    #[test]
    fn parses_vm_identity_state_and_guest_type() {
        assert_eq!(
            parse_vm_list_line("\"Windows - Barnen\" {vm-1}"),
            Some(("Windows - Barnen".into(), "vm-1".into()))
        );
        let fields = parse_machine_readable("name=\"Windows - Barnen\"\nUUID=\"vm-1\"\nVMState=\"poweroff\"\nostype=\"Windows11_64\"\n");
        assert_eq!(fields.get("UUID").map(String::as_str), Some("vm-1"));
        assert_eq!(fields.get("VMState").map(String::as_str), Some("poweroff"));
    }
}

struct UnavailableExecutor(String);

impl VBoxManageExecutor for UnavailableExecutor {
    fn execute(&self, _arguments: &[String]) -> Result<VBoxOutput, String> {
        Err(self.0.clone())
    }
}

struct ActivationInputs<'a> {
    keyboard: &'a PhysicalInputDevice,
    mouse: &'a PhysicalInputDevice,
}
fn configured_vm_id<'a>(
    seat_id: &SeatId,
    configuration: &'a ApplicationConfig,
) -> Result<&'a str, String> {
    configuration
        .backends
        .virtual_box
        .seats
        .get(&seat_id.0)
        .map(|item| item.vm_id.as_str())
        .ok_or_else(|| format!("seat '{}' has no VirtualBox VM selected", seat_id.0))
}

fn configured_display_id<'a>(
    seat_id: &SeatId,
    configuration: &'a ApplicationConfig,
) -> Option<&'a str> {
    configuration
        .seats
        .iter()
        .find(|seat| &seat.id == seat_id)
        .and_then(|seat| seat.devices.display.as_ref())
        .map(|display| display.0.as_str())
}

fn managed_backend<'a>(
    seat_id: &SeatId,
    configuration: &'a ApplicationConfig,
) -> Result<&'a crate::seats::VirtualBoxSeatConfig, String> {
    let backend = configuration
        .backends
        .virtual_box
        .seats
        .get(&seat_id.0)
        .ok_or_else(|| format!("seat '{}' has no VirtualBox VM selected", seat_id.0))?;
    if !backend.managed_by_multiseat {
        return Err("input isolation is available only for a MultiSeat Lite managed VM".to_owned());
    }
    Ok(backend)
}

fn require_running(vm: &VirtualMachine) -> Result<(), String> {
    if vm.state.eq_ignore_ascii_case("running") {
        Ok(())
    } else {
        Err(format!(
            "managed VM '{}' is not running (state: {})",
            vm.name, vm.state
        ))
    }
}

fn usb_health_from_vbox_state(state: Option<&str>) -> UsbRuntimeHealth {
    match state.map(str::to_ascii_lowercase).as_deref() {
        Some("captured") | Some("held") => UsbRuntimeHealth::Captured,
        Some("busy") => UsbRuntimeHealth::HostBusy,
        Some("available") | Some("unavailable") => UsbRuntimeHealth::Released,
        Some(_) | None => UsbRuntimeHealth::Failed,
    }
}

fn ensure_usb_passthrough_allowed(
    configuration: &ApplicationConfig,
    physical: &PhysicalInputDevice,
    keyboard_capable: bool,
) -> Result<(), String> {
    let safety =
        configuration.device_safety(&physical.id, DeviceRoutingBackend::VirtualBoxUsbPassthrough);
    if let Some(record) = safety {
        if matches!(
            record.safety,
            UsbPassthroughSafety::KnownProblematic | UsbPassthroughSafety::Disabled
        ) {
            return Err(format!(
                "VirtualBox USB passthrough is disabled for '{}': {}. Automatic retry is disabled",
                physical.id, record.reason
            ));
        }
    }
    if keyboard_capable
        && !safety.is_some_and(|record| record.safety == UsbPassthroughSafety::Supported)
    {
        return Err(format!(
            "keyboard '{}' is not explicitly cleared for VirtualBox USB passthrough; use native keyboard routing",
            physical.id
        ));
    }
    Ok(())
}

fn observe_usb_in_snapshot(
    physical_id: &str,
    kind: InputKind,
    devices: &[PhysicalInputDevice],
    status: &VirtualBoxStatus,
    after_attach: bool,
) -> UsbObservation {
    let Some(physical) = find_physical(devices, physical_id, kind) else {
        return UsbObservation {
            windows_physical_device: EvidencePresence::Missing,
            usb_parent: EvidencePresence::Unknown,
            virtual_box_host_device: EvidencePresence::Unknown,
            resolved_usb_parent: None,
            runtime_uuid: None,
            runtime_address: None,
            current_state: None,
            mapping: None,
            health: UsbRuntimeHealth::PhysicalDeviceDisconnected,
        };
    };
    let mapping = correlate_usb_devices(&[physical.clone()], status).remove(0);
    let matched = mapping
        .matched_uuid
        .as_deref()
        .and_then(|uuid| status.usb_devices.iter().find(|device| device.uuid == uuid));
    let current_state = matched.and_then(|device| device.current_state.clone());
    let health = match mapping.state {
        UsbCorrelationState::Ambiguous => UsbRuntimeHealth::Ambiguous,
        UsbCorrelationState::Unavailable => {
            if after_attach {
                UsbRuntimeHealth::DisappearedAfterAttach
            } else {
                UsbRuntimeHealth::NotResolved
            }
        }
        UsbCorrelationState::Exact | UsbCorrelationState::Unambiguous => {
            usb_health_from_vbox_state(current_state.as_deref())
        }
    };
    UsbObservation {
        windows_physical_device: EvidencePresence::Present,
        usb_parent: physical
            .usb_parent_instance_id
            .as_ref()
            .map_or(EvidencePresence::Unknown, |_| EvidencePresence::Present),
        virtual_box_host_device: if matched.is_some() {
            EvidencePresence::Present
        } else {
            EvidencePresence::Missing
        },
        resolved_usb_parent: physical.usb_parent_instance_id.clone(),
        runtime_uuid: matched.map(|device| device.uuid.clone()),
        runtime_address: matched.and_then(|device| device.address.clone()),
        current_state,
        mapping: Some(mapping.state),
        health,
    }
}

fn validate_activation<'a>(
    seat_id: &SeatId,
    configuration: &ApplicationConfig,
    devices: &'a [PhysicalInputDevice],
) -> Result<ActivationInputs<'a>, String> {
    let target_index = configuration
        .seats
        .iter()
        .position(|seat| &seat.id == seat_id)
        .ok_or_else(|| format!("unknown seat '{}'", seat_id.0))?;
    if target_index == 0 {
        return Err(
            "the primary host seat cannot be started as the VirtualBox second seat".to_owned(),
        );
    }
    configured_vm_id(seat_id, configuration)?;
    let inputs = validate_target_inputs_only(seat_id, configuration, devices)?;
    let host = &configuration.seats[0];
    let host_keyboard = host
        .devices
        .keyboard
        .as_ref()
        .ok_or_else(|| "Dennis must retain a configured keyboard".to_owned())?;
    let host_mouse = host
        .devices
        .mouse
        .as_ref()
        .ok_or_else(|| "Dennis must retain a configured mouse".to_owned())?;
    if host_keyboard.0.eq_ignore_ascii_case(&inputs.keyboard.id)
        || host_mouse.0.eq_ignore_ascii_case(&inputs.mouse.id)
        || host_keyboard.0.eq_ignore_ascii_case(&inputs.mouse.id)
        || host_mouse.0.eq_ignore_ascii_case(&inputs.keyboard.id)
    {
        return Err("a Barnen input device is also assigned to Dennis".to_owned());
    }
    Ok(inputs)
}

fn validate_target_inputs_only<'a>(
    seat_id: &SeatId,
    configuration: &ApplicationConfig,
    devices: &'a [PhysicalInputDevice],
) -> Result<ActivationInputs<'a>, String> {
    let seat = configuration
        .seats
        .iter()
        .find(|seat| &seat.id == seat_id)
        .ok_or_else(|| format!("unknown seat '{}'", seat_id.0))?;
    let keyboard_id = seat
        .devices
        .keyboard
        .as_ref()
        .ok_or_else(|| "Barnen has no configured keyboard".to_owned())?;
    let mouse_id = seat
        .devices
        .mouse
        .as_ref()
        .ok_or_else(|| "Barnen has no configured mouse".to_owned())?;
    let keyboard = find_physical(devices, &keyboard_id.0, InputKind::Keyboard)
        .ok_or_else(|| format!("Barnen keyboard '{}' is not present", keyboard_id.0))?;
    let mouse = find_physical(devices, &mouse_id.0, InputKind::Mouse)
        .ok_or_else(|| format!("Barnen mouse '{}' is not present", mouse_id.0))?;
    Ok(ActivationInputs { keyboard, mouse })
}

fn find_physical<'a>(
    devices: &'a [PhysicalInputDevice],
    id: &str,
    kind: InputKind,
) -> Option<&'a PhysicalInputDevice> {
    devices.iter().find(|device| {
        (device.id.eq_ignore_ascii_case(id) || device.contains_logical_id(id))
            && match kind {
                InputKind::Keyboard => device.capabilities.keyboard,
                InputKind::Mouse => device.capabilities.mouse,
            }
    })
}

fn target_device_id<'a>(
    seat_id: &SeatId,
    configuration: &'a ApplicationConfig,
    kind: InputKind,
) -> Option<&'a str> {
    configuration
        .seats
        .iter()
        .find(|seat| &seat.id == seat_id)
        .and_then(|seat| match kind {
            InputKind::Keyboard => seat.devices.keyboard.as_ref(),
            InputKind::Mouse => seat.devices.mouse.as_ref(),
        })
        .map(|id| id.0.as_str())
}

fn pending_component(
    id: Option<&str>,
    routing_strategy: InputRoutingStrategy,
    record: &RuntimeRecord,
) -> RuntimeComponentStatus {
    RuntimeComponentStatus {
        configured: id.is_some(),
        attached: false,
        physical_device_id: id.map(str::to_owned),
        runtime_uuid: None,
        mapping: None,
        usb: RuntimeUsbDiagnostic::default(),
        routing_strategy,
        routing_status: match routing_strategy {
            InputRoutingStrategy::NativeKeyboardRouting => record.keyboard_routing_status,
            InputRoutingStrategy::VirtualBoxUsbPassthrough => {
                InputRoutingRuntimeStatus::NotImplemented
            }
            InputRoutingStrategy::Disabled => InputRoutingRuntimeStatus::Disabled,
        },
        successful_guest_sends: None,
        usb_passthrough_safety: if routing_strategy
            == InputRoutingStrategy::VirtualBoxUsbPassthrough
        {
            UsbPassthroughSafety::Unverified
        } else {
            UsbPassthroughSafety::Disabled
        },
        safety_reason: None,
    }
}

fn component_status(
    id: Option<&str>,
    kind: InputKind,
    record: &RuntimeRecord,
    devices: &[PhysicalInputDevice],
    observation: Result<Option<UsbObservation>, String>,
    routing_strategy: InputRoutingStrategy,
    safety_record: Option<&crate::seats::DeviceBackendSafetyRecord>,
) -> RuntimeComponentStatus {
    let item = record.attached.iter().find(|item| item.kind == kind);
    let mut usb =
        item.map(|item| item.diagnostic.clone())
            .unwrap_or_else(|| RuntimeUsbDiagnostic {
                windows_physical_device: if id
                    .is_some_and(|id| find_physical(devices, id, kind).is_some())
                {
                    EvidencePresence::Present
                } else if id.is_some() {
                    EvidencePresence::Missing
                } else {
                    EvidencePresence::Unknown
                },
                ..RuntimeUsbDiagnostic::default()
            });
    let mut mapping = item.map(|item| item.mapping);
    match observation {
        Ok(Some(observation)) => {
            usb.windows_physical_device = observation.windows_physical_device;
            usb.usb_parent = observation.usb_parent;
            usb.virtual_box_host_device = observation.virtual_box_host_device;
            usb.resolved_usb_parent = observation.resolved_usb_parent;
            usb.runtime_uuid = observation.runtime_uuid;
            usb.runtime_address = observation.runtime_address;
            usb.current_virtual_box_state = observation.current_state;
            usb.health = observation.health;
            usb.probe_error = None;
            mapping = observation.mapping.or(mapping);
        }
        Ok(None) => {}
        Err(error) => {
            usb.health = UsbRuntimeHealth::Failed;
            usb.probe_error = Some(error);
        }
    }
    let attached = usb.health == UsbRuntimeHealth::Captured;
    let routing_status = match routing_strategy {
        InputRoutingStrategy::VirtualBoxUsbPassthrough if attached => {
            InputRoutingRuntimeStatus::Active
        }
        InputRoutingStrategy::VirtualBoxUsbPassthrough => InputRoutingRuntimeStatus::Error,
        InputRoutingStrategy::NativeKeyboardRouting => InputRoutingRuntimeStatus::PrototypeInactive,
        InputRoutingStrategy::Disabled => InputRoutingRuntimeStatus::Disabled,
    };
    let usb_passthrough_safety = safety_record.map_or_else(
        || {
            if routing_strategy == InputRoutingStrategy::VirtualBoxUsbPassthrough {
                UsbPassthroughSafety::Unverified
            } else {
                UsbPassthroughSafety::Disabled
            }
        },
        |record| record.safety,
    );
    RuntimeComponentStatus {
        configured: id.is_some_and(|id| find_physical(devices, id, kind).is_some()),
        attached,
        physical_device_id: id.map(str::to_owned),
        runtime_uuid: usb
            .runtime_uuid
            .clone()
            .or_else(|| item.map(|item| item.runtime_uuid.clone())),
        mapping,
        usb,
        routing_strategy,
        routing_status,
        successful_guest_sends: None,
        usb_passthrough_safety,
        safety_reason: safety_record.map(|record| record.reason.clone()),
    }
}

fn acceptable_uuid(mapping: &super::UsbCorrelation, label: &str) -> Result<String, String> {
    if !matches!(
        mapping.state,
        UsbCorrelationState::Exact | UsbCorrelationState::Unambiguous
    ) {
        return Err(format!(
            "{label} could not be resolved safely: {:?}: {}",
            mapping.state, mapping.reason
        ));
    }
    mapping
        .matched_uuid
        .clone()
        .ok_or_else(|| format!("{label} mapping has no runtime UUID"))
}

fn parse_vm_list_line(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix('"')?;
    let end = rest.rfind("\" {")?;
    let name = &rest[..end];
    let uuid = rest[end + 3..].strip_suffix('}')?;
    (!name.is_empty() && !uuid.is_empty()).then(|| (name.to_owned(), uuid.to_owned()))
}

pub fn parse_machine_readable(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| {
            (
                key.trim().trim_matches('"').to_owned(),
                value
                    .trim()
                    .trim_matches('"')
                    .replace("\\\"", "\"")
                    .replace("\\\\", "\\"),
            )
        })
        .collect()
}

pub fn validate_vm(vm: &VirtualMachine) -> Vec<String> {
    let mut errors = Vec::new();
    if !vm
        .guest_os_description
        .as_deref()
        .is_some_and(|kind| kind.to_ascii_lowercase().contains("windows"))
    {
        errors.push("selected VM is not configured with a Windows guest OS type".to_owned());
    }
    if vm.usb_xhci_enabled == Some(false) {
        errors.push("selected VM does not have its xHCI USB controller enabled".to_owned());
    }
    errors
}
