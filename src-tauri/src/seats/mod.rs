//! Logical seat configuration, validation, persistence, and hardware reconciliation.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

use crate::{
    devices::{InputCapabilities, InputDeviceType, PhysicalInputDevice},
    HardwareSnapshot,
};

pub const CONFIG_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SeatId(pub String);

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct PhysicalDeviceId(pub String);

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DisplayId(pub String);

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatDeviceAssignments {
    pub display: Option<DisplayId>,
    pub keyboard: Option<PhysicalDeviceId>,
    pub mouse: Option<PhysicalDeviceId>,
    pub audio_output: Option<PhysicalDeviceId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatConfig {
    pub id: SeatId,
    pub name: String,
    pub devices: SeatDeviceAssignments,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationConfig {
    pub version: u32,
    pub seats: Vec<SeatConfig>,
    #[serde(default)]
    pub backends: BackendConfiguration,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendConfiguration {
    pub virtual_box: VirtualBoxBackendConfiguration,
    /// Routing is keyed by seat ID, independently of stable hardware IDs.
    #[serde(default)]
    pub input_routing: BTreeMap<String, SeatInputRouting>,
    /// Explicit per-physical-device/backend safety decisions. These are not a
    /// global VID/PID blacklist.
    #[serde(default)]
    pub device_safety: Vec<DeviceBackendSafetyRecord>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InputRoutingStrategy {
    VirtualBoxUsbPassthrough,
    NativeKeyboardRouting,
    Disabled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InputRoutingRuntimeStatus {
    NotImplemented,
    PrototypeInactive,
    PrototypeActive,
    Active,
    Error,
    Disabled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceRoutingBackend {
    VirtualBoxUsbPassthrough,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UsbPassthroughSafety {
    Supported,
    Unverified,
    KnownProblematic,
    Disabled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceBackendSafetyRecord {
    pub physical_device_id: PhysicalDeviceId,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub backend: DeviceRoutingBackend,
    pub safety: UsbPassthroughSafety,
    pub reason: String,
    pub observed: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatInputRouting {
    pub keyboard: InputRoutingStrategy,
    pub mouse: InputRoutingStrategy,
}

impl Default for SeatInputRouting {
    fn default() -> Self {
        Self {
            keyboard: InputRoutingStrategy::NativeKeyboardRouting,
            mouse: InputRoutingStrategy::VirtualBoxUsbPassthrough,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualBoxBackendConfiguration {
    /// Backend-only configuration keyed by stable MultiSeat Lite seat ID.
    pub seats: BTreeMap<String, VirtualBoxSeatConfig>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualBoxSeatConfig {
    /// Stable VirtualBox VM UUID. Host USB UUIDs must never be persisted here.
    pub vm_id: String,
    /// True only for a VM created transactionally by MultiSeat Lite.
    #[serde(default)]
    pub managed_by_multiseat: bool,
    /// MultiSeat Lite owns the per-VM GUI/MouseCapturePolicy setting and may
    /// switch it only for this managed VM during an explicit admin unlock.
    #[serde(default)]
    pub mouse_capture_policy_owned: bool,
    /// Non-sensitive, restart-safe installation journal. Credentials and
    /// product keys are deliberately not represented by this model.
    #[serde(default)]
    pub installation: Option<ManagedVmInstallation>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ManagedWindowsVersion {
    Windows10,
    Windows11,
}

fn legacy_managed_windows_version() -> ManagedWindowsVersion {
    // Managed installation records written before selectable profiles were
    // introduced could only have been created by the Windows 11 path.
    ManagedWindowsVersion::Windows11
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ManagedInstallState {
    NotCreated,
    Creating,
    InstallingWindows,
    Rebooting,
    InstallingGuestAdditions,
    WaitingForGuest,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GuestAdditionsInstallState {
    NotInstalled,
    Installing,
    Installed,
    Failed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowsInstallationStatus {
    #[default]
    Unknown,
    Installing,
    Installed,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GuestAdditionsStatus {
    #[default]
    Unknown,
    Unavailable,
    /// A host-side property such as HostVerLastChecked exists, but no guest
    /// runlevel or installed Guest Additions version has been verified.
    PartialEvidence,
    Communicating,
    InstalledVersionKnown,
    RepairRequired,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GuestRunLevel {
    #[default]
    Unknown,
    System,
    Userland,
    Desktop,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ManagedVmStatus {
    #[default]
    Installing,
    WindowsInstalled,
    WaitingForGuestAdditions,
    OperationalUnverified,
    Degraded,
    Stopped,
    Saved,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestRunLevelProbe {
    pub level: GuestRunLevel,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedInstallationEvidence {
    pub last_vm_state: Option<String>,
    pub runlevel_probes: Vec<GuestRunLevelProbe>,
    pub guest_os_product: Option<String>,
    pub guest_additions_version: Option<String>,
    pub guest_add_host_version_last_checked: Option<String>,
    pub reset_counter: Option<u64>,
    pub reconciliation_timestamp_unix_ms: Option<u64>,
    pub reconciliation_event: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedInstallationObservation {
    pub windows: WindowsInstallationStatus,
    pub guest_additions: GuestAdditionsStatus,
    pub run_level: GuestRunLevel,
    pub managed: ManagedVmStatus,
    pub evidence: ManagedInstallationEvidence,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedVmInstallation {
    #[serde(default = "legacy_managed_windows_version")]
    pub windows_version: ManagedWindowsVersion,
    #[serde(default, alias = "virtualBoxOsType")]
    pub virtual_box_os_type_id: String,
    #[serde(default)]
    pub computer_name: String,
    #[serde(default)]
    pub domain_name: String,
    pub state: ManagedInstallState,
    pub guest_additions: GuestAdditionsInstallState,
    #[serde(default)]
    pub guest_additions_version: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub observed: ManagedInstallationObservation,
}

impl Default for ApplicationConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            seats: vec![
                SeatConfig {
                    id: SeatId("seat-1".to_owned()),
                    name: "Dennis".to_owned(),
                    devices: SeatDeviceAssignments::default(),
                },
                SeatConfig {
                    id: SeatId("seat-2".to_owned()),
                    name: "Barnen".to_owned(),
                    devices: SeatDeviceAssignments::default(),
                },
            ],
            backends: BackendConfiguration::default(),
        }
    }
}

impl ApplicationConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != CONFIG_VERSION {
            return Err(format!(
                "unsupported configuration version {}; expected {CONFIG_VERSION}",
                self.version
            ));
        }
        if self.seats.len() != 2 {
            return Err("configuration must contain exactly two seats".to_owned());
        }

        let mut seat_ids = HashSet::new();
        let mut displays = HashSet::new();
        let mut keyboards = HashSet::new();
        let mut mice = HashSet::new();
        let mut audio_outputs = HashSet::new();

        for seat in &self.seats {
            if seat.id.0.trim().is_empty() || !seat_ids.insert(&seat.id.0) {
                return Err(format!("seat ID '{}' is empty or duplicated", seat.id.0));
            }
            if seat.name.trim().is_empty() {
                return Err(format!("seat '{}' has an empty name", seat.id.0));
            }
            validate_unique(
                &mut displays,
                seat.devices.display.as_ref().map(|id| &id.0),
                "display",
            )?;
            validate_unique(
                &mut keyboards,
                seat.devices.keyboard.as_ref().map(|id| &id.0),
                "keyboard",
            )?;
            validate_unique(
                &mut mice,
                seat.devices.mouse.as_ref().map(|id| &id.0),
                "mouse",
            )?;
            validate_unique(
                &mut audio_outputs,
                seat.devices.audio_output.as_ref().map(|id| &id.0),
                "audio output",
            )?;
        }
        for (seat_id, backend) in &self.backends.virtual_box.seats {
            if !seat_ids.contains(seat_id) {
                return Err(format!(
                    "VirtualBox configuration references unknown seat '{seat_id}'"
                ));
            }
            if backend.vm_id.trim().is_empty() {
                return Err(format!("VirtualBox VM ID for seat '{seat_id}' is empty"));
            }
        }
        for (index, seat) in self.seats.iter().enumerate() {
            for input_id in input_assignment_ids(&seat.devices) {
                if self.seats.iter().skip(index + 1).any(|other| {
                    input_assignment_ids(&other.devices)
                        .any(|other_id| other_id.eq_ignore_ascii_case(input_id))
                }) {
                    return Err(format!(
                        "physical input device '{input_id}' is split between seats"
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn update_seat(&mut self, seat_id: &SeatId, name: String) -> Result<(), String> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("seat name cannot be empty".to_owned());
        }
        self.seat_mut(seat_id)?.name = trimmed.to_owned();
        self.validate()
    }

    pub fn assign(
        &mut self,
        seat_id: &SeatId,
        slot: AssignmentSlot,
        device_id: String,
    ) -> Result<(), String> {
        if device_id.trim().is_empty() {
            return Err("device ID cannot be empty".to_owned());
        }
        if self.seats.iter().any(|seat| {
            &seat.id != seat_id
                && assignment_value(&seat.devices, slot).is_some_and(|id| id == device_id)
        }) {
            return Err(format!(
                "{slot:?} device '{device_id}' is already assigned to another seat"
            ));
        }

        let assignments = &mut self.seat_mut(seat_id)?.devices;
        match slot {
            AssignmentSlot::Display => assignments.display = Some(DisplayId(device_id)),
            AssignmentSlot::Keyboard => assignments.keyboard = Some(PhysicalDeviceId(device_id)),
            AssignmentSlot::Mouse => assignments.mouse = Some(PhysicalDeviceId(device_id)),
            AssignmentSlot::AudioOutput => {
                assignments.audio_output = Some(PhysicalDeviceId(device_id))
            }
        }
        self.validate()
    }

    pub fn assign_physical_input(
        &mut self,
        seat_id: &SeatId,
        device_id: String,
        capabilities: InputCapabilities,
    ) -> Result<(), String> {
        if device_id.trim().is_empty() {
            return Err("device ID cannot be empty".to_owned());
        }
        if !capabilities.keyboard && !capabilities.mouse {
            return Err("physical input device has no assignable capability".to_owned());
        }
        if self.seats.iter().any(|seat| {
            &seat.id != seat_id
                && input_assignment_ids(&seat.devices).any(|id| id.eq_ignore_ascii_case(&device_id))
        }) {
            return Err(format!(
                "physical input device '{device_id}' is already assigned to another seat"
            ));
        }

        let assignments = &mut self.seat_mut(seat_id)?.devices;
        if capabilities.keyboard {
            assignments.keyboard = Some(PhysicalDeviceId(device_id.clone()));
        }
        if capabilities.mouse {
            assignments.mouse = Some(PhysicalDeviceId(device_id));
        }
        self.validate()
    }

    pub fn unassign(&mut self, seat_id: &SeatId, slot: AssignmentSlot) -> Result<(), String> {
        let assignments = &mut self.seat_mut(seat_id)?.devices;
        match slot {
            AssignmentSlot::Display => assignments.display = None,
            AssignmentSlot::Keyboard | AssignmentSlot::Mouse => {
                let physical_id = match slot {
                    AssignmentSlot::Keyboard => assignments.keyboard.as_ref(),
                    AssignmentSlot::Mouse => assignments.mouse.as_ref(),
                    _ => None,
                }
                .map(|id| id.0.clone());
                if let Some(physical_id) = physical_id {
                    if assignments
                        .keyboard
                        .as_ref()
                        .is_some_and(|id| id.0.eq_ignore_ascii_case(&physical_id))
                    {
                        assignments.keyboard = None;
                    }
                    if assignments
                        .mouse
                        .as_ref()
                        .is_some_and(|id| id.0.eq_ignore_ascii_case(&physical_id))
                    {
                        assignments.mouse = None;
                    }
                }
            }
            AssignmentSlot::AudioOutput => assignments.audio_output = None,
        }
        self.validate()
    }

    pub fn set_virtual_box_vm(&mut self, seat_id: &SeatId, vm_id: String) -> Result<(), String> {
        if !self.seats.iter().any(|seat| &seat.id == seat_id) {
            return Err(format!("unknown seat '{}'", seat_id.0));
        }
        let vm_id = vm_id.trim();
        if vm_id.is_empty() {
            self.backends.virtual_box.seats.remove(&seat_id.0);
        } else {
            self.backends.virtual_box.seats.insert(
                seat_id.0.clone(),
                VirtualBoxSeatConfig {
                    vm_id: vm_id.to_owned(),
                    managed_by_multiseat: false,
                    mouse_capture_policy_owned: false,
                    installation: None,
                },
            );
        }
        self.validate()
    }

    pub fn set_managed_virtual_box_vm(
        &mut self,
        seat_id: &SeatId,
        vm_id: String,
        installation: ManagedVmInstallation,
    ) -> Result<(), String> {
        if !self.seats.iter().any(|seat| &seat.id == seat_id) {
            return Err(format!("unknown seat '{}'", seat_id.0));
        }
        self.backends.virtual_box.seats.insert(
            seat_id.0.clone(),
            VirtualBoxSeatConfig {
                vm_id,
                managed_by_multiseat: true,
                mouse_capture_policy_owned: false,
                installation: Some(installation),
            },
        );
        self.validate()
    }

    pub fn update_managed_vm_installation(
        &mut self,
        seat_id: &SeatId,
        installation: ManagedVmInstallation,
    ) -> Result<(), String> {
        let backend = self
            .backends
            .virtual_box
            .seats
            .get_mut(&seat_id.0)
            .ok_or_else(|| format!("seat '{}' has no VirtualBox VM", seat_id.0))?;
        if !backend.managed_by_multiseat {
            return Err(
                "installation status is available only for MultiSeat Lite managed VMs".to_owned(),
            );
        }
        backend.installation = Some(installation);
        Ok(())
    }

    pub fn set_mouse_capture_policy_owned(
        &mut self,
        seat_id: &SeatId,
        owned: bool,
    ) -> Result<(), String> {
        let backend = self
            .backends
            .virtual_box
            .seats
            .get_mut(&seat_id.0)
            .ok_or_else(|| format!("seat '{}' has no VirtualBox VM", seat_id.0))?;
        if !backend.managed_by_multiseat {
            return Err(
                "input-isolation settings may only be owned for a MultiSeat Lite managed VM"
                    .to_owned(),
            );
        }
        backend.mouse_capture_policy_owned = owned;
        Ok(())
    }

    pub fn input_routing(&self, seat_id: &SeatId) -> SeatInputRouting {
        self.backends
            .input_routing
            .get(&seat_id.0)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_input_routing(
        &mut self,
        seat_id: &SeatId,
        routing: SeatInputRouting,
    ) -> Result<(), String> {
        if !self.seats.iter().any(|seat| &seat.id == seat_id) {
            return Err(format!("unknown seat '{}'", seat_id.0));
        }
        self.backends
            .input_routing
            .insert(seat_id.0.clone(), routing);
        Ok(())
    }

    pub fn device_safety(
        &self,
        physical_device_id: &str,
        backend: DeviceRoutingBackend,
    ) -> Option<&DeviceBackendSafetyRecord> {
        self.backends.device_safety.iter().find(|record| {
            record.backend == backend
                && record
                    .physical_device_id
                    .0
                    .eq_ignore_ascii_case(physical_device_id)
        })
    }

    pub fn record_device_safety(
        &mut self,
        record: DeviceBackendSafetyRecord,
    ) -> Result<(), String> {
        if record.reason.trim().is_empty() {
            return Err("device safety reason must not be empty".to_owned());
        }
        self.backends.device_safety.retain(|existing| {
            existing.backend != record.backend
                || !existing
                    .physical_device_id
                    .0
                    .eq_ignore_ascii_case(&record.physical_device_id.0)
        });
        self.backends.device_safety.push(record);
        Ok(())
    }

    fn seat_mut(&mut self, seat_id: &SeatId) -> Result<&mut SeatConfig, String> {
        self.seats
            .iter_mut()
            .find(|seat| &seat.id == seat_id)
            .ok_or_else(|| format!("unknown seat '{}'", seat_id.0))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AssignmentSlot {
    Display,
    Keyboard,
    Mouse,
    AudioOutput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AssignmentAvailability {
    Present,
    Missing,
    Ambiguous,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedAssignment {
    pub id: String,
    pub container_id: Option<String>,
    pub availability: AssignmentAvailability,
    pub friendly_name: Option<String>,
    pub secondary: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedSeatDeviceAssignments {
    pub display: Option<ResolvedAssignment>,
    pub keyboard: Option<ResolvedAssignment>,
    pub mouse: Option<ResolvedAssignment>,
    pub audio_output: Option<ResolvedAssignment>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedSeat {
    pub id: SeatId,
    pub name: String,
    pub devices: ResolvedSeatDeviceAssignments,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationConfigSnapshot {
    pub configuration: ApplicationConfig,
    pub resolved_seats: Vec<ResolvedSeat>,
    pub load_warning: Option<String>,
    pub configuration_path: String,
}

pub fn reconcile(
    configuration: &ApplicationConfig,
    hardware: &HardwareSnapshot,
) -> Vec<ResolvedSeat> {
    configuration
        .seats
        .iter()
        .map(|seat| ResolvedSeat {
            id: seat.id.clone(),
            name: seat.name.clone(),
            devices: ResolvedSeatDeviceAssignments {
                display: seat.devices.display.as_ref().map(|id| {
                    let matches: Vec<_> = hardware
                        .pnp_monitors
                        .iter()
                        .filter(|monitor| monitor.instance_id.eq_ignore_ascii_case(&id.0))
                        .collect();
                    resolve_matches(
                        &id.0,
                        matches.len(),
                        None,
                        matches
                            .first()
                            .and_then(|monitor| monitor.friendly_name.clone()),
                        matches.first().map(|monitor| monitor.instance_id.clone()),
                    )
                }),
                keyboard: resolve_input(
                    seat.devices.keyboard.as_ref(),
                    InputDeviceType::Keyboard,
                    hardware,
                ),
                mouse: resolve_input(
                    seat.devices.mouse.as_ref(),
                    InputDeviceType::Mouse,
                    hardware,
                ),
                audio_output: seat
                    .devices
                    .audio_output
                    .as_ref()
                    .map(|id| ResolvedAssignment {
                        id: id.0.clone(),
                        container_id: None,
                        availability: AssignmentAvailability::Missing,
                        friendly_name: None,
                        secondary: Some("Audio discovery is not implemented yet".to_owned()),
                    }),
            },
        })
        .collect()
}

fn resolve_input(
    assignment: Option<&PhysicalDeviceId>,
    device_type: InputDeviceType,
    hardware: &HardwareSnapshot,
) -> Option<ResolvedAssignment> {
    assignment.map(|id| {
        let matches: Vec<_> = hardware
            .input_devices
            .iter()
            .filter(|device| {
                device.capabilities.supports(device_type)
                    && (device.id.eq_ignore_ascii_case(&id.0) || device.contains_logical_id(&id.0))
            })
            .collect();
        let first = matches.first();
        resolve_matches(
            first.map_or(id.0.as_str(), |device| device.id.as_str()),
            matches.len(),
            first.and_then(|device| device.container_id.clone()),
            first.and_then(|device| device.friendly_name.clone()),
            first.map(|device| match (&device.vendor_id, &device.product_id) {
                (Some(vendor), Some(product)) => format!("VID {vendor} · PID {product}"),
                _ => device.id.clone(),
            }),
        )
    })
}

fn resolve_matches(
    id: &str,
    match_count: usize,
    container_id: Option<String>,
    friendly_name: Option<String>,
    secondary: Option<String>,
) -> ResolvedAssignment {
    ResolvedAssignment {
        id: id.to_owned(),
        container_id,
        availability: match match_count {
            0 => AssignmentAvailability::Missing,
            1 => AssignmentAvailability::Present,
            _ => AssignmentAvailability::Ambiguous,
        },
        friendly_name,
        secondary,
    }
}

fn assignment_value(assignments: &SeatDeviceAssignments, slot: AssignmentSlot) -> Option<&str> {
    match slot {
        AssignmentSlot::Display => assignments.display.as_ref().map(|id| id.0.as_str()),
        AssignmentSlot::Keyboard => assignments.keyboard.as_ref().map(|id| id.0.as_str()),
        AssignmentSlot::Mouse => assignments.mouse.as_ref().map(|id| id.0.as_str()),
        AssignmentSlot::AudioOutput => assignments.audio_output.as_ref().map(|id| id.0.as_str()),
    }
}

fn input_assignment_ids(assignments: &SeatDeviceAssignments) -> impl Iterator<Item = &str> {
    [assignments.keyboard.as_ref(), assignments.mouse.as_ref()]
        .into_iter()
        .flatten()
        .map(|id| id.0.as_str())
}

fn assignment_maps_to_physical(id: &str, device: &PhysicalInputDevice) -> bool {
    device.id.eq_ignore_ascii_case(id) || device.contains_logical_id(id)
}

fn validate_unique<'a>(
    seen: &mut HashSet<&'a String>,
    value: Option<&'a String>,
    label: &str,
) -> Result<(), String> {
    if let Some(value) = value {
        if value.trim().is_empty() {
            return Err(format!("{label} assignment cannot be empty"));
        }
        if !seen.insert(value) {
            return Err(format!("{label} '{value}' is assigned more than once"));
        }
    }
    Ok(())
}

fn matching_physical_inputs<'a>(
    hardware: &'a HardwareSnapshot,
    id: &str,
    device_type: InputDeviceType,
) -> Vec<&'a PhysicalInputDevice> {
    hardware
        .input_devices
        .iter()
        .filter(|device| {
            device.capabilities.supports(device_type)
                && (device.id.eq_ignore_ascii_case(id) || device.contains_logical_id(id))
        })
        .collect()
}

fn migrate_legacy_input_assignments(
    configuration: &mut ApplicationConfig,
    hardware: &HardwareSnapshot,
) -> bool {
    let candidates: Vec<_> = configuration
        .seats
        .iter()
        .enumerate()
        .flat_map(|(seat_index, seat)| {
            [
                (
                    AssignmentSlot::Keyboard,
                    seat.devices.keyboard.as_ref().map(|id| id.0.clone()),
                ),
                (
                    AssignmentSlot::Mouse,
                    seat.devices.mouse.as_ref().map(|id| id.0.clone()),
                ),
            ]
            .into_iter()
            .filter_map(move |(slot, id)| id.map(|id| (seat_index, slot, id)))
        })
        .filter_map(|(seat_index, slot, old_id)| {
            let device_type = match slot {
                AssignmentSlot::Keyboard => InputDeviceType::Keyboard,
                AssignmentSlot::Mouse => InputDeviceType::Mouse,
                _ => return None,
            };
            let matches = matching_physical_inputs(hardware, &old_id, device_type);
            (matches.len() == 1).then(|| (seat_index, slot, old_id, matches[0].id.clone()))
        })
        .collect();

    let mut owners: HashMap<String, HashSet<usize>> = HashMap::new();
    for (seat_index, _, _, physical_id) in &candidates {
        owners
            .entry(physical_id.to_ascii_lowercase())
            .or_default()
            .insert(*seat_index);
    }

    let mut changed = false;
    for (seat_index, slot, old_id, physical_id) in candidates {
        if old_id.eq_ignore_ascii_case(&physical_id)
            || owners
                .get(&physical_id.to_ascii_lowercase())
                .is_some_and(|seat_owners| seat_owners.len() != 1)
        {
            continue;
        }
        let replacement = Some(PhysicalDeviceId(physical_id));
        match slot {
            AssignmentSlot::Keyboard => {
                configuration.seats[seat_index].devices.keyboard = replacement
            }
            AssignmentSlot::Mouse => configuration.seats[seat_index].devices.mouse = replacement,
            _ => {}
        }
        changed = true;
    }
    changed
}

pub struct ConfigStore {
    path: PathBuf,
    configuration: Mutex<ApplicationConfig>,
    load_warning: Option<String>,
}

impl ConfigStore {
    pub fn initialize(path: PathBuf) -> Result<Self, String> {
        let (configuration, load_warning, should_save) = load_configuration(&path);
        configuration.validate()?;
        let store = Self {
            path,
            configuration: Mutex::new(configuration),
            load_warning,
        };
        if should_save {
            store.save()?;
        }
        Ok(store)
    }

    pub fn get(&self) -> Result<ApplicationConfig, String> {
        self.configuration
            .lock()
            .map(|config| config.clone())
            .map_err(|_| "configuration lock is poisoned".to_owned())
    }

    pub fn snapshot(
        &self,
        hardware: &HardwareSnapshot,
    ) -> Result<ApplicationConfigSnapshot, String> {
        let configuration = self.get()?;
        Ok(ApplicationConfigSnapshot {
            resolved_seats: reconcile(&configuration, hardware),
            configuration,
            load_warning: self.load_warning.clone(),
            configuration_path: self.path.display().to_string(),
        })
    }

    pub fn reconcile_hardware(&self, hardware: &HardwareSnapshot) -> Result<(), String> {
        let mut configuration = self.get()?;
        if !migrate_legacy_input_assignments(&mut configuration, hardware) {
            return Ok(());
        }
        configuration.validate()?;
        save_configuration(&self.path, &configuration)?;
        *self
            .configuration
            .lock()
            .map_err(|_| "configuration lock is poisoned".to_owned())? = configuration;
        Ok(())
    }

    pub fn update_seat(&self, seat_id: SeatId, name: String) -> Result<(), String> {
        self.mutate(|configuration| configuration.update_seat(&seat_id, name))
    }

    pub fn assign(
        &self,
        seat_id: SeatId,
        slot: AssignmentSlot,
        device_id: String,
    ) -> Result<(), String> {
        self.mutate(|configuration| configuration.assign(&seat_id, slot, device_id))
    }

    pub fn assign_physical_input(
        &self,
        seat_id: SeatId,
        device: &PhysicalInputDevice,
    ) -> Result<(), String> {
        self.mutate(|configuration| {
            if configuration.seats.iter().any(|seat| {
                seat.id != seat_id
                    && input_assignment_ids(&seat.devices)
                        .any(|id| assignment_maps_to_physical(id, device))
            }) {
                return Err(format!(
                    "physical input device '{}' is already assigned to another seat",
                    device.id
                ));
            }
            configuration.assign_physical_input(&seat_id, device.id.clone(), device.capabilities)
        })
    }

    pub fn unassign(&self, seat_id: SeatId, slot: AssignmentSlot) -> Result<(), String> {
        self.mutate(|configuration| configuration.unassign(&seat_id, slot))
    }

    pub fn set_virtual_box_vm(&self, seat_id: SeatId, vm_id: String) -> Result<(), String> {
        self.mutate(|configuration| configuration.set_virtual_box_vm(&seat_id, vm_id))
    }

    pub fn set_managed_virtual_box_vm(
        &self,
        seat_id: SeatId,
        vm_id: String,
        installation: ManagedVmInstallation,
    ) -> Result<(), String> {
        self.mutate(|configuration| {
            configuration.set_managed_virtual_box_vm(&seat_id, vm_id, installation)
        })
    }

    pub fn update_managed_vm_installation(
        &self,
        seat_id: SeatId,
        installation: ManagedVmInstallation,
    ) -> Result<(), String> {
        self.mutate(|configuration| {
            configuration.update_managed_vm_installation(&seat_id, installation)
        })
    }

    pub fn set_mouse_capture_policy_owned(
        &self,
        seat_id: SeatId,
        owned: bool,
    ) -> Result<(), String> {
        self.mutate(|configuration| configuration.set_mouse_capture_policy_owned(&seat_id, owned))
    }

    pub fn record_device_safety(&self, record: DeviceBackendSafetyRecord) -> Result<(), String> {
        self.mutate(|configuration| configuration.record_device_safety(record))
    }

    pub fn save(&self) -> Result<(), String> {
        let configuration = self.get()?;
        save_configuration(&self.path, &configuration)
    }

    fn mutate(
        &self,
        operation: impl FnOnce(&mut ApplicationConfig) -> Result<(), String>,
    ) -> Result<(), String> {
        let current = self.get()?;
        let mut updated = current;
        operation(&mut updated)?;
        updated.validate()?;
        save_configuration(&self.path, &updated)?;
        *self
            .configuration
            .lock()
            .map_err(|_| "configuration lock is poisoned".to_owned())? = updated;
        Ok(())
    }
}

fn load_configuration(path: &Path) -> (ApplicationConfig, Option<String>, bool) {
    if !path.exists() {
        return (ApplicationConfig::default(), None, true);
    }

    match read_and_validate(path) {
        Ok(configuration) => (configuration, None, false),
        Err(primary_error) => {
            let backup = backup_path(path);
            match read_and_validate(&backup) {
                Ok(configuration) => (
                    configuration,
                    Some(format!(
                        "configuration was invalid ({primary_error}); loaded backup"
                    )),
                    false,
                ),
                Err(_) => (
                    ApplicationConfig::default(),
                    Some(format!(
                        "configuration was invalid ({primary_error}); using safe defaults"
                    )),
                    false,
                ),
            }
        }
    }
}

fn read_and_validate(path: &Path) -> Result<ApplicationConfig, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("could not read '{}': {error}", path.display()))?;
    let configuration: ApplicationConfig = serde_json::from_str(&content)
        .map_err(|error| format!("could not parse '{}': {error}", path.display()))?;
    configuration.validate()?;
    Ok(configuration)
}

fn save_configuration(path: &Path, configuration: &ApplicationConfig) -> Result<(), String> {
    configuration.validate()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create '{}': {error}", parent.display()))?;
    }
    let serialized = serde_json::to_string_pretty(configuration)
        .map_err(|error| format!("could not serialize configuration: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, format!("{serialized}\n"))
        .map_err(|error| format!("could not write '{}': {error}", temporary.display()))?;
    if path.exists() && read_and_validate(path).is_ok() {
        fs::copy(path, backup_path(path))
            .map_err(|error| format!("could not back up '{}': {error}", path.display()))?;
    }
    fs::copy(&temporary, path)
        .map_err(|error| format!("could not replace '{}': {error}", path.display()))?;
    fs::remove_file(&temporary)
        .map_err(|error| format!("could not remove '{}': {error}", temporary.display()))?;
    Ok(())
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.bak")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::InputDevice;

    fn seat(id: &str) -> SeatId {
        SeatId(id.to_owned())
    }

    fn environment_info() -> crate::environment::EnvironmentInfo {
        crate::environment::EnvironmentInfo {
            current_session_id: 1,
            aster: crate::environment::AsterEnvironmentStatus {
                installation_detected: false,
                related_service_installed: false,
                related_service_running: false,
                mut_enx_device_node_present: false,
                mut_enx_raw_input_visible: false,
                registry_markers_present: false,
                registry_markers: Vec::new(),
                related_services: Vec::new(),
                workplace_state: crate::environment::AsterWorkplaceState::Unknown,
            },
        }
    }

    fn logical_node(id: &str, device_type: InputDeviceType) -> InputDevice {
        InputDevice {
            id: id.into(),
            device_path: None,
            instance_id: id.into(),
            container_id: Some("container-a".into()),
            friendly_name: Some("Composite input".into()),
            manufacturer: None,
            hardware_ids: Vec::new(),
            enumerator: Some("HID".into()),
            device_type,
            vendor_id: Some("1234".into()),
            product_id: Some("5678".into()),
            usb_parent_instance_id: None,
            serial_number: None,
        }
    }

    fn composite_physical_device() -> PhysicalInputDevice {
        PhysicalInputDevice {
            id: "container-a".into(),
            container_id: Some("container-a".into()),
            friendly_name: Some("Composite input".into()),
            manufacturer: None,
            vendor_id: Some("1234".into()),
            product_id: Some("5678".into()),
            usb_parent_instance_id: None,
            serial_number: None,
            capabilities: InputCapabilities {
                keyboard: true,
                mouse: true,
            },
            logical_nodes: vec![
                logical_node("legacy-keyboard-node", InputDeviceType::Keyboard),
                logical_node("legacy-mouse-node", InputDeviceType::Mouse),
            ],
            raw_input_paths: Vec::new(),
            present: true,
        }
    }

    fn hardware_with(device: PhysicalInputDevice) -> HardwareSnapshot {
        HardwareSnapshot {
            displays: Vec::new(),
            logical_input_devices: device.logical_nodes.clone(),
            input_devices: vec![device],
            pnp_monitors: Vec::new(),
            display_config_targets: Vec::new(),
            session_raw_input_devices: Vec::new(),
            aster_related_pnp_devices: Vec::new(),
            environment: environment_info(),
        }
    }

    #[test]
    fn assigns_a_device() {
        let mut config = ApplicationConfig::default();
        config
            .assign(
                &seat("seat-1"),
                AssignmentSlot::Keyboard,
                "keyboard-a".into(),
            )
            .unwrap();
        assert_eq!(
            config.seats[0].devices.keyboard.as_ref().unwrap().0,
            "keyboard-a"
        );
    }

    #[test]
    fn prevents_duplicate_assignment() {
        let mut config = ApplicationConfig::default();
        config
            .assign(&seat("seat-1"), AssignmentSlot::Mouse, "mouse-a".into())
            .unwrap();
        let error = config
            .assign(&seat("seat-2"), AssignmentSlot::Mouse, "mouse-a".into())
            .unwrap_err();
        assert!(error.contains("already assigned"));
    }

    #[test]
    fn physical_device_cannot_be_split_between_seats() {
        let mut config = ApplicationConfig::default();
        config
            .assign_physical_input(
                &seat("seat-1"),
                "container-a".into(),
                InputCapabilities {
                    keyboard: true,
                    mouse: true,
                },
            )
            .unwrap();
        let error = config
            .assign_physical_input(
                &seat("seat-2"),
                "container-a".into(),
                InputCapabilities {
                    keyboard: true,
                    mouse: true,
                },
            )
            .unwrap_err();
        assert!(error.contains("another seat"));
    }

    #[test]
    fn legacy_child_assignment_reserves_its_physical_container() {
        let directory = test_directory("legacy-container-reservation");
        let path = directory.join("config.json");
        let store = ConfigStore::initialize(path).unwrap();
        store
            .assign(
                seat("seat-1"),
                AssignmentSlot::Keyboard,
                "legacy-keyboard-node".into(),
            )
            .unwrap();
        let error = store
            .assign_physical_input(seat("seat-2"), &composite_physical_device())
            .unwrap_err();
        assert!(error.contains("another seat"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn persisted_logical_node_assignment_reconciles_to_aggregate() {
        let directory = test_directory("aggregate-migration");
        let path = directory.join("config.json");
        let mut config = ApplicationConfig::default();
        config
            .assign(
                &seat("seat-1"),
                AssignmentSlot::Keyboard,
                "legacy-keyboard-node".into(),
            )
            .unwrap();
        save_configuration(&path, &config).unwrap();
        let hardware = hardware_with(composite_physical_device());
        let store = ConfigStore::initialize(path.clone()).unwrap();
        store.reconcile_hardware(&hardware).unwrap();
        assert_eq!(
            store.get().unwrap().seats[0]
                .devices
                .keyboard
                .as_ref()
                .unwrap()
                .0,
            "container-a"
        );
        drop(store);
        let reopened = ConfigStore::initialize(path).unwrap();
        assert_eq!(
            reconcile(&reopened.get().unwrap(), &hardware)[0]
                .devices
                .keyboard
                .as_ref()
                .unwrap()
                .availability,
            AssignmentAvailability::Present
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unassigns_a_device() {
        let mut config = ApplicationConfig::default();
        config
            .assign(&seat("seat-1"), AssignmentSlot::Display, "display-a".into())
            .unwrap();
        config
            .unassign(&seat("seat-1"), AssignmentSlot::Display)
            .unwrap();
        assert!(config.seats[0].devices.display.is_none());
    }

    #[test]
    fn virtual_box_vm_uuid_is_persisted_separately_from_seat_devices() {
        let directory = test_directory("virtualbox-vm-selection");
        let path = directory.join("config.json");
        let store = ConfigStore::initialize(path.clone()).unwrap();
        store
            .set_virtual_box_vm(seat("seat-2"), "stable-vm-uuid".into())
            .unwrap();
        drop(store);
        let reopened = ConfigStore::initialize(path).unwrap();
        assert_eq!(
            reopened
                .get()
                .unwrap()
                .backends
                .virtual_box
                .seats
                .get("seat-2")
                .unwrap()
                .vm_id,
            "stable-vm-uuid"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn safe_default_routes_keyboard_natively_and_mouse_through_virtual_box_usb() {
        let config = ApplicationConfig::default();
        assert_eq!(
            config.input_routing(&seat("seat-2")),
            SeatInputRouting {
                keyboard: InputRoutingStrategy::NativeKeyboardRouting,
                mouse: InputRoutingStrategy::VirtualBoxUsbPassthrough,
            }
        );
    }

    #[test]
    fn per_physical_device_usb_safety_incident_persists() {
        let directory = test_directory("device-usb-safety");
        let path = directory.join("config.json");
        let store = ConfigStore::initialize(path.clone()).unwrap();
        store
            .record_device_safety(DeviceBackendSafetyRecord {
                physical_device_id: PhysicalDeviceId("container-keyboard".into()),
                vendor_id: Some("1A2C".into()),
                product_id: Some("4C5E".into()),
                backend: DeviceRoutingBackend::VirtualBoxUsbPassthrough,
                safety: UsbPassthroughSafety::KnownProblematic,
                reason: "host crash during capture".into(),
                observed: Some("0xCA / VBoxUSBMon.sys".into()),
            })
            .unwrap();
        drop(store);

        let reopened = ConfigStore::initialize(path).unwrap();
        let configuration = reopened.get().unwrap();
        let record = configuration
            .device_safety(
                "container-keyboard",
                DeviceRoutingBackend::VirtualBoxUsbPassthrough,
            )
            .unwrap();
        assert_eq!(record.safety, UsbPassthroughSafety::KnownProblematic);
        assert_eq!(record.vendor_id.as_deref(), Some("1A2C"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn managed_installation_journal_persists_without_credentials() {
        let directory = test_directory("managed-installation-journal");
        let path = directory.join("config.json");
        let store = ConfigStore::initialize(path.clone()).unwrap();
        store
            .set_managed_virtual_box_vm(
                seat("seat-2"),
                "managed-vm-uuid".into(),
                ManagedVmInstallation {
                    windows_version: ManagedWindowsVersion::Windows10,
                    virtual_box_os_type_id: "Windows10_64".into(),
                    computer_name: "BARNEN-PC".into(),
                    domain_name: "multiseat.local".into(),
                    state: ManagedInstallState::InstallingWindows,
                    guest_additions: GuestAdditionsInstallState::Installing,
                    guest_additions_version: None,
                    last_error: None,
                    observed: ManagedInstallationObservation::default(),
                },
            )
            .unwrap();
        store
            .set_mouse_capture_policy_owned(seat("seat-2"), true)
            .unwrap();
        let serialized = fs::read_to_string(&path).unwrap();
        assert!(!serialized.contains("password"));
        assert!(!serialized.contains("productKey"));
        drop(store);
        let reopened = ConfigStore::initialize(path).unwrap();
        let backend = reopened
            .get()
            .unwrap()
            .backends
            .virtual_box
            .seats
            .get("seat-2")
            .unwrap()
            .clone();
        assert!(backend.managed_by_multiseat);
        assert!(backend.mouse_capture_policy_owned);
        assert_eq!(
            backend.installation.as_ref().unwrap().state,
            ManagedInstallState::InstallingWindows
        );
        assert_eq!(
            backend.installation.unwrap().windows_version,
            ManagedWindowsVersion::Windows10
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn existing_selected_vm_cannot_become_managed_through_status_updates() {
        let mut config = ApplicationConfig::default();
        config
            .set_virtual_box_vm(&seat("seat-2"), "existing-vm".into())
            .unwrap();
        let result = config.update_managed_vm_installation(
            &seat("seat-2"),
            ManagedVmInstallation {
                windows_version: ManagedWindowsVersion::Windows11,
                virtual_box_os_type_id: "Windows11_64".into(),
                computer_name: "BARNEN-PC".into(),
                domain_name: "multiseat.local".into(),
                state: ManagedInstallState::Ready,
                guest_additions: GuestAdditionsInstallState::Installed,
                guest_additions_version: Some("7.2.14".into()),
                last_error: None,
                observed: ManagedInstallationObservation::default(),
            },
        );
        assert!(result.is_err());
        assert!(!config.backends.virtual_box.seats["seat-2"].managed_by_multiseat);
    }

    #[test]
    fn loads_valid_configuration() {
        let directory = test_directory("valid");
        let path = directory.join("config.json");
        save_configuration(&path, &ApplicationConfig::default()).unwrap();
        let store = ConfigStore::initialize(path).unwrap();
        assert_eq!(store.get().unwrap(), ApplicationConfig::default());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn assignments_survive_store_restart() {
        let directory = test_directory("restart");
        let path = directory.join("config.json");
        {
            let store = ConfigStore::initialize(path.clone()).unwrap();
            store
                .assign(
                    seat("seat-2"),
                    AssignmentSlot::Keyboard,
                    "persistent-keyboard".into(),
                )
                .unwrap();
        }
        let reopened = ConfigStore::initialize(path).unwrap();
        assert_eq!(
            reopened.get().unwrap().seats[1]
                .devices
                .keyboard
                .as_ref()
                .unwrap()
                .0,
            "persistent-keyboard"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_configuration_uses_safe_defaults() {
        let directory = test_directory("malformed");
        let path = directory.join("config.json");
        fs::write(&path, "{ definitely not json").unwrap();
        let store = ConfigStore::initialize(path).unwrap();
        assert_eq!(store.get().unwrap(), ApplicationConfig::default());
        assert!(store.load_warning.is_some());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn disconnected_assignment_remains_missing() {
        let mut config = ApplicationConfig::default();
        config
            .assign(
                &seat("seat-1"),
                AssignmentSlot::Display,
                "missing-display".into(),
            )
            .unwrap();
        let hardware = HardwareSnapshot {
            displays: Vec::new(),
            input_devices: Vec::new(),
            logical_input_devices: Vec::new(),
            pnp_monitors: Vec::new(),
            display_config_targets: Vec::new(),
            session_raw_input_devices: Vec::new(),
            aster_related_pnp_devices: Vec::new(),
            environment: environment_info(),
        };
        let resolved = reconcile(&config, &hardware);
        assert_eq!(
            resolved[0].devices.display.as_ref().unwrap().availability,
            AssignmentAvailability::Missing
        );
        assert_eq!(
            config.seats[0].devices.display.as_ref().unwrap().0,
            "missing-display"
        );
    }

    fn test_directory(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "multiseat-lite-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }
}
