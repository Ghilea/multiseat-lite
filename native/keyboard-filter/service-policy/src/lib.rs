//! User-mode safety policy for the future MultiSeat keyboard filter service.
//!
//! This crate has no driver or Tauri dependency so the decisions that guard
//! suppression can be tested without loading kernel code.

use std::collections::{BTreeSet, VecDeque};

pub const DEFAULT_LEASE_MS: u32 = 2_000;
pub const MIN_LEASE_MS: u32 = 500;
pub const MAX_LEASE_MS: u32 = 5_000;
pub const PROTOCOL_MAGIC: u32 = 0x4C4B_534D;
pub const PROTOCOL_VERSION: u16 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicalKeyboardIdentity {
    /// MultiSeat stable identity: Container ID, or the existing PnP fallback.
    pub stable_physical_id: String,
    pub container_id: Option<String>,
    /// Exact keyboard TLC devnode instance IDs resolved by SetupAPI.
    pub keyboard_collection_instance_ids: Vec<String>,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
}

impl PhysicalKeyboardIdentity {
    pub fn same_physical_device(&self, other: &Self) -> bool {
        match (&self.container_id, &other.container_id) {
            (Some(left), Some(right)) => equal_id(left, right),
            _ => equal_id(&self.stable_physical_id, &other.stable_physical_id),
        }
    }

    fn normalized_collections(&self) -> BTreeSet<String> {
        self.keyboard_collection_instance_ids
            .iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect()
    }
}

fn equal_id(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

#[derive(Clone, Debug)]
pub struct EligibilityRequest<'a> {
    pub selected: &'a PhysicalKeyboardIdentity,
    pub barnen_assignment: &'a PhysicalKeyboardIdentity,
    pub dennis_assignment: &'a PhysicalKeyboardIdentity,
    pub present_keyboards: &'a [PhysicalKeyboardIdentity],
    /// Number of present physical devices to which discovery resolved the
    /// selected persistent identity. It must be exactly one.
    pub selected_resolution_count: usize,
    pub configuration_valid: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuppressionRejection {
    InvalidConfiguration,
    NotBarnenAssignment,
    DennisSelected,
    AmbiguousOrMissingIdentity,
    TargetNotPresent,
    NoKeyboardCollections,
    NoAlternateHostKeyboard,
}

pub fn evaluate_eligibility(
    request: &EligibilityRequest<'_>,
) -> Result<ApprovedTarget, SuppressionRejection> {
    if !request.configuration_valid {
        return Err(SuppressionRejection::InvalidConfiguration);
    }
    if !request
        .selected
        .same_physical_device(request.barnen_assignment)
    {
        return Err(SuppressionRejection::NotBarnenAssignment);
    }
    if request
        .selected
        .same_physical_device(request.dennis_assignment)
    {
        return Err(SuppressionRejection::DennisSelected);
    }
    if request.selected_resolution_count != 1 {
        return Err(SuppressionRejection::AmbiguousOrMissingIdentity);
    }
    if !request
        .present_keyboards
        .iter()
        .any(|device| device.same_physical_device(request.selected))
    {
        return Err(SuppressionRejection::TargetNotPresent);
    }
    let collections = request.selected.normalized_collections();
    if collections.is_empty() {
        return Err(SuppressionRejection::NoKeyboardCollections);
    }
    if !request
        .present_keyboards
        .iter()
        .any(|device| !device.same_physical_device(request.selected))
    {
        return Err(SuppressionRejection::NoAlternateHostKeyboard);
    }
    Ok(ApprovedTarget {
        stable_physical_id: request.selected.stable_physical_id.clone(),
        container_id: request.selected.container_id.clone(),
        keyboard_collection_instance_ids: collections,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovedTarget {
    pub stable_physical_id: String,
    pub container_id: Option<String>,
    keyboard_collection_instance_ids: BTreeSet<String>,
}

impl ApprovedTarget {
    pub fn contains_collection(&self, instance_id: &str) -> bool {
        self.keyboard_collection_instance_ids
            .contains(&instance_id.trim().to_ascii_lowercase())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailOpenReason {
    NoTarget,
    SuppressionDisabled,
    LeaseExpired,
    ServiceDisconnected,
    DeviceDisconnected,
    InvalidConfiguration,
    DriverUnavailable,
    DisabledByBuild,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuppressionCapability {
    DisabledByBuild,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverStatus {
    pub driver_loaded: bool,
    pub target_resolved: bool,
    pub target_instance: Option<String>,
    pub suppression_requested: bool,
    pub suppression_active: bool,
    pub fail_open: bool,
    pub fail_open_reason: Option<FailOpenReason>,
    pub lease_expires_at_ms: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Default)]
pub struct SuppressionStateMachine {
    target: Option<ApprovedTarget>,
    service_connected: bool,
    device_present: bool,
    configuration_valid: bool,
    suppression_requested: bool,
    lease_expires_at_ms: Option<u64>,
    last_error: Option<String>,
}

impl SuppressionStateMachine {
    pub fn connect_service(&mut self) {
        self.service_connected = true;
    }

    pub fn set_target(&mut self, target: ApprovedTarget) {
        self.disable();
        self.target = Some(target);
        self.device_present = true;
        self.configuration_valid = true;
    }

    pub fn enable(&mut self, now_ms: u64, lease_ms: u32) -> Result<(), String> {
        if !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&lease_ms) {
            return self.fail("lease duration is outside the permitted range");
        }
        if !self.service_connected || self.target.is_none() || !self.device_present {
            return self.fail("target, device, and service must be healthy before enable");
        }
        if !self.configuration_valid {
            return self.fail("configuration is invalid");
        }
        self.suppression_requested = true;
        self.lease_expires_at_ms = Some(now_ms.saturating_add(u64::from(lease_ms)));
        self.last_error = None;
        Ok(())
    }

    pub fn heartbeat(&mut self, now_ms: u64, lease_ms: u32) -> Result<(), String> {
        if !self.is_active(now_ms) {
            self.disable();
            return self.fail("suppression lease is not active");
        }
        if !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&lease_ms) {
            self.disable();
            return self.fail("lease duration is outside the permitted range");
        }
        self.lease_expires_at_ms = Some(now_ms.saturating_add(u64::from(lease_ms)));
        Ok(())
    }

    pub fn disable(&mut self) {
        self.suppression_requested = false;
        self.lease_expires_at_ms = None;
    }

    pub fn disconnect_service(&mut self) {
        self.service_connected = false;
        self.disable();
    }

    pub fn device_disconnected(&mut self) {
        self.device_present = false;
        self.disable();
    }

    pub fn invalidate_configuration(&mut self) {
        self.configuration_valid = false;
        self.disable();
    }

    pub fn packet_is_suppression_candidate(&mut self, instance_id: &str, now_ms: u64) -> bool {
        if !self.is_active(now_ms) {
            return false;
        }
        self.target
            .as_ref()
            .is_some_and(|target| target.contains_collection(instance_id))
    }

    pub fn query(&self, now_ms: u64) -> DriverStatus {
        let reason = if self.target.is_none() {
            Some(FailOpenReason::NoTarget)
        } else if !self.configuration_valid {
            Some(FailOpenReason::InvalidConfiguration)
        } else if !self.device_present {
            Some(FailOpenReason::DeviceDisconnected)
        } else if !self.service_connected {
            Some(FailOpenReason::ServiceDisconnected)
        } else if !self.suppression_requested {
            Some(FailOpenReason::SuppressionDisabled)
        } else if !self.lease_is_valid(now_ms) {
            Some(FailOpenReason::LeaseExpired)
        } else {
            None
        };
        DriverStatus {
            driver_loaded: true,
            target_resolved: self.target.is_some(),
            target_instance: self
                .target
                .as_ref()
                .map(|target| target.stable_physical_id.clone()),
            suppression_requested: self.suppression_requested,
            suppression_active: reason.is_none(),
            fail_open: reason.is_some(),
            fail_open_reason: reason,
            lease_expires_at_ms: self.lease_expires_at_ms,
            last_error: self.last_error.clone(),
        }
    }

    fn is_active(&self, now_ms: u64) -> bool {
        self.service_connected
            && self.device_present
            && self.configuration_valid
            && self.suppression_requested
            && self.target.is_some()
            && self.lease_is_valid(now_ms)
    }

    fn lease_is_valid(&self, now_ms: u64) -> bool {
        self.lease_expires_at_ms
            .is_some_and(|expiry| now_ms < expiry)
    }

    fn fail<T>(&mut self, message: &str) -> Result<T, String> {
        self.disable();
        self.last_error = Some(message.to_owned());
        Err(message.to_owned())
    }
}

/// Privileged-service boundary. The desktop process should call an authenticated
/// service API; only the service implements this driver-facing interface.
pub trait KeyboardSuppressionDriver {
    fn set_target(&mut self, target: ApprovedTarget) -> Result<(), String>;
    fn enable_suppression(&mut self, now_ms: u64, lease_ms: u32) -> Result<(), String>;
    fn disable_suppression(&mut self) -> Result<(), String>;
    fn heartbeat(&mut self, now_ms: u64, lease_ms: u32) -> Result<(), String>;
    fn query_status(&self, now_ms: u64) -> Result<DriverStatus, String>;
}

pub struct MockKeyboardSuppressionDriver {
    state: SuppressionStateMachine,
    available: bool,
    heartbeat_count: u64,
}

impl Default for MockKeyboardSuppressionDriver {
    fn default() -> Self {
        Self {
            state: SuppressionStateMachine::default(),
            available: true,
            heartbeat_count: 0,
        }
    }
}

impl MockKeyboardSuppressionDriver {
    pub fn connect_service(&mut self) {
        self.state.connect_service();
    }

    pub fn disconnect_service(&mut self) {
        self.state.disconnect_service();
    }

    pub fn device_disconnected(&mut self) {
        self.state.device_disconnected();
    }

    pub fn invalidate_configuration(&mut self) {
        self.state.invalidate_configuration();
    }

    pub fn make_unavailable(&mut self) {
        self.available = false;
        self.state.disconnect_service();
    }

    pub fn packet_is_suppression_candidate(&mut self, instance_id: &str, now_ms: u64) -> bool {
        self.state
            .packet_is_suppression_candidate(instance_id, now_ms)
    }
}

impl KeyboardSuppressionDriver for MockKeyboardSuppressionDriver {
    fn set_target(&mut self, target: ApprovedTarget) -> Result<(), String> {
        self.state.set_target(target);
        Ok(())
    }

    fn enable_suppression(&mut self, now_ms: u64, lease_ms: u32) -> Result<(), String> {
        let _ = (now_ms, lease_ms);
        Err("suppression capability is DisabledByBuild".to_owned())
    }

    fn disable_suppression(&mut self) -> Result<(), String> {
        self.state.disable();
        Ok(())
    }

    fn heartbeat(&mut self, now_ms: u64, lease_ms: u32) -> Result<(), String> {
        let _ = (now_ms, lease_ms);
        if !self.available || !self.state.service_connected {
            return Err("driver/service channel is unavailable".into());
        }
        self.heartbeat_count = self.heartbeat_count.saturating_add(1);
        Ok(())
    }

    fn query_status(&self, now_ms: u64) -> Result<DriverStatus, String> {
        if !self.available {
            return Err("pass-through driver is unavailable".to_owned());
        }
        let mut status = self.state.query(now_ms);
        status.suppression_requested = false;
        status.suppression_active = false;
        status.fail_open = true;
        status.fail_open_reason = Some(FailOpenReason::DisabledByBuild);
        Ok(status)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolCommand {
    SetTargetDevice = 1,
    EnableSuppression = 2,
    DisableSuppression = 3,
    QueryStatus = 4,
    Heartbeat = 5,
    SetObservation = 6,
    ReadEvents = 7,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolHeader {
    pub magic: u32,
    pub version: u16,
    pub size: u16,
    pub command: u32,
    pub request_id: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    MalformedMessage,
    VersionMismatch,
    CommandMismatch,
}

pub fn validate_protocol_header(
    header: ProtocolHeader,
    actual_size: usize,
    expected_size: u16,
    command: ProtocolCommand,
) -> Result<(), ProtocolError> {
    if header.magic != PROTOCOL_MAGIC
        || header.size != expected_size
        || actual_size != usize::from(expected_size)
    {
        return Err(ProtocolError::MalformedMessage);
    }
    if header.version != PROTOCOL_VERSION {
        return Err(ProtocolError::VersionMismatch);
    }
    if header.command != command as u32 {
        return Err(ProtocolError::CommandMismatch);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostDelivery {
    PassThrough,
}

/// User-mode model of the driver's fixed-capacity, nonblocking observation
/// ring. Overflow changes diagnostics only; host delivery is always pass-through.
pub struct ObservationRing<T> {
    entries: VecDeque<T>,
    capacity: usize,
    overflow_count: u64,
    next_sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sequenced<T> {
    pub sequence: u64,
    pub value: T,
}

impl<T> ObservationRing<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
            overflow_count: 0,
            next_sequence: 1,
        }
    }

    pub fn observe(&mut self, value: T) -> HostDelivery {
        if self.entries.len() == self.capacity {
            self.overflow_count = self.overflow_count.saturating_add(1);
        } else {
            self.entries.push_back(value);
        }
        HostDelivery::PassThrough
    }

    pub fn drain(&mut self) -> Vec<Sequenced<T>> {
        self.entries
            .drain(..)
            .map(|value| {
                let sequence = self.next_sequence;
                self.next_sequence = self.next_sequence.saturating_add(1);
                Sequenced { sequence, value }
            })
            .collect()
    }

    pub fn overflow_count(&self) -> u64 {
        self.overflow_count
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadinessReason {
    UnsupportedWindows,
    ArchitectureMismatch,
    DennisUnresolved,
    BarnenUnresolved,
    AmbiguousDennis,
    AmbiguousBarnen,
    SamePhysicalKeyboard,
    FewerThanTwoPhysicalKeyboards,
    KeyboardStackNotEnumerated,
    DriverAlreadyInstalled,
    RemoteRecoveryNotConfirmed,
}

#[derive(Clone, Debug)]
pub struct PassThroughReadinessEvidence<'a> {
    pub supported_windows_x64: bool,
    pub driver_build_is_x64: bool,
    pub physical_keyboards: &'a [PhysicalKeyboardIdentity],
    pub dennis_matches: &'a [PhysicalKeyboardIdentity],
    pub barnen_matches: &'a [PhysicalKeyboardIdentity],
    pub keyboard_stack_enumerated: bool,
    pub driver_already_installed: bool,
    pub remote_recovery_confirmed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PassThroughReadiness {
    ReadyForPassThroughDriverTest,
    NotReady(Vec<ReadinessReason>),
}

pub fn evaluate_pass_through_readiness(
    evidence: &PassThroughReadinessEvidence<'_>,
) -> PassThroughReadiness {
    let mut reasons = Vec::new();
    if !evidence.supported_windows_x64 {
        reasons.push(ReadinessReason::UnsupportedWindows);
    }
    if !evidence.driver_build_is_x64 {
        reasons.push(ReadinessReason::ArchitectureMismatch);
    }
    if evidence.physical_keyboards.len() < 2 {
        reasons.push(ReadinessReason::FewerThanTwoPhysicalKeyboards);
    }
    match evidence.dennis_matches.len() {
        0 => reasons.push(ReadinessReason::DennisUnresolved),
        1 => {}
        _ => reasons.push(ReadinessReason::AmbiguousDennis),
    }
    match evidence.barnen_matches.len() {
        0 => reasons.push(ReadinessReason::BarnenUnresolved),
        1 => {}
        _ => reasons.push(ReadinessReason::AmbiguousBarnen),
    }
    if let ([dennis], [barnen]) = (evidence.dennis_matches, evidence.barnen_matches) {
        if dennis.same_physical_device(barnen) {
            reasons.push(ReadinessReason::SamePhysicalKeyboard);
        }
    }
    if !evidence.keyboard_stack_enumerated {
        reasons.push(ReadinessReason::KeyboardStackNotEnumerated);
    }
    if evidence.driver_already_installed {
        reasons.push(ReadinessReason::DriverAlreadyInstalled);
    }
    if !evidence.remote_recovery_confirmed {
        reasons.push(ReadinessReason::RemoteRecoveryNotConfirmed);
    }
    if reasons.is_empty() {
        PassThroughReadiness::ReadyForPassThroughDriverTest
    } else {
        PassThroughReadiness::NotReady(reasons)
    }
}

#[derive(Clone, Debug)]
pub struct InfGenerationRequest<'a> {
    pub target: &'a PhysicalKeyboardIdentity,
    pub dennis: &'a PhysicalKeyboardIdentity,
    pub expected_container_id: &'a str,
    pub target_match_count: usize,
    pub present_physical_keyboard_count: usize,
    pub exact_keyboard_tlc_hardware_id: &'a str,
}

pub fn generate_test_machine_inf(
    template: &str,
    request: &InfGenerationRequest<'_>,
) -> Result<String, String> {
    if request.target.same_physical_device(request.dennis) {
        return Err("refusing to generate an INF for the Dennis/primary keyboard".into());
    }
    if request.expected_container_id.trim().is_empty()
        || !request
            .target
            .container_id
            .as_deref()
            .is_some_and(|container| {
                container.eq_ignore_ascii_case(request.expected_container_id.trim())
            })
    {
        return Err("selected TLC does not belong to the explicitly expected Container ID".into());
    }
    if request.target_match_count != 1 {
        return Err("target selection is missing or ambiguous".into());
    }
    if request.present_physical_keyboard_count < 2 {
        return Err("at least two distinct physical keyboards are required".into());
    }
    if request.target.keyboard_collection_instance_ids.len() != 1 {
        return Err("select exactly one keyboard TLC for this staged INF".into());
    }
    let hardware_id = request.exact_keyboard_tlc_hardware_id.trim();
    if !hardware_id.to_ascii_uppercase().starts_with("HID\\")
        || hardware_id.contains(['\r', '\n', ',', '"'])
    {
        return Err("hardware ID is not a safe exact HID TLC hardware ID".into());
    }
    let token = "REPLACE_WITH_EXACT_KEYBOARD_TLC_HARDWARE_ID";
    if template.matches(token).count() != 1 {
        return Err("INF template must contain exactly one target token".into());
    }
    Ok(template.replace(token, hardware_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyboard(
        stable: &str,
        container: &str,
        collection: &str,
        vid: &str,
    ) -> PhysicalKeyboardIdentity {
        PhysicalKeyboardIdentity {
            stable_physical_id: stable.into(),
            container_id: Some(container.into()),
            keyboard_collection_instance_ids: vec![collection.into()],
            vendor_id: Some(vid.into()),
            product_id: Some("4C5E".into()),
        }
    }

    fn approved() -> ApprovedTarget {
        let barnen = keyboard("barnen", "container-b", "hid\\barnen&col01", "1A2C");
        let dennis = keyboard("dennis", "container-d", "hid\\dennis&col01", "1A2C");
        evaluate_eligibility(&EligibilityRequest {
            selected: &barnen,
            barnen_assignment: &barnen,
            dennis_assignment: &dennis,
            present_keyboards: &[barnen.clone(), dennis.clone()],
            selected_resolution_count: 1,
            configuration_valid: true,
        })
        .unwrap()
    }

    #[test]
    fn barnen_selected_is_eligible_and_same_vid_pid_does_not_merge_identity() {
        let barnen = keyboard("barnen", "container-b", "hid\\barnen&col01", "1A2C");
        let dennis = keyboard("dennis", "container-d", "hid\\dennis&col01", "1A2C");
        let result = evaluate_eligibility(&EligibilityRequest {
            selected: &barnen,
            barnen_assignment: &barnen,
            dennis_assignment: &dennis,
            present_keyboards: &[barnen.clone(), dennis.clone()],
            selected_resolution_count: 1,
            configuration_valid: true,
        });
        assert!(result.is_ok());
        assert!(!barnen.same_physical_device(&dennis));
    }

    #[test]
    fn target_resolution_is_exact_to_keyboard_tlc() {
        let target = approved();
        assert!(target.contains_collection("HID\\BARNEN&COL01"));
        assert!(!target.contains_collection("HID\\BARNEN&COL02"));
        assert!(!target.contains_collection("HID\\DENNIS&COL01"));
    }

    #[test]
    fn dennis_selected_is_rejected() {
        let dennis = keyboard("dennis", "container-d", "hid\\dennis", "1532");
        assert_eq!(
            evaluate_eligibility(&EligibilityRequest {
                selected: &dennis,
                barnen_assignment: &dennis,
                dennis_assignment: &dennis,
                present_keyboards: std::slice::from_ref(&dennis),
                selected_resolution_count: 1,
                configuration_valid: true,
            }),
            Err(SuppressionRejection::DennisSelected)
        );
    }

    #[test]
    fn ambiguous_identity_and_no_alternate_keyboard_are_rejected() {
        let barnen = keyboard("barnen", "container-b", "hid\\barnen", "1A2C");
        let dennis = keyboard("dennis", "container-d", "hid\\dennis", "1532");
        let base = EligibilityRequest {
            selected: &barnen,
            barnen_assignment: &barnen,
            dennis_assignment: &dennis,
            present_keyboards: std::slice::from_ref(&barnen),
            selected_resolution_count: 2,
            configuration_valid: true,
        };
        assert_eq!(
            evaluate_eligibility(&base),
            Err(SuppressionRejection::AmbiguousOrMissingIdentity)
        );
        assert_eq!(
            evaluate_eligibility(&EligibilityRequest {
                selected_resolution_count: 1,
                ..base
            }),
            Err(SuppressionRejection::NoAlternateHostKeyboard)
        );
    }

    fn configured_driver() -> MockKeyboardSuppressionDriver {
        let mut driver = MockKeyboardSuppressionDriver::default();
        driver.connect_service();
        driver.set_target(approved()).unwrap();
        driver
    }

    #[test]
    fn disabled_build_rejects_enable_and_all_packets_pass() {
        let mut driver = configured_driver();
        driver.heartbeat(900, DEFAULT_LEASE_MS).unwrap();
        assert!(driver
            .enable_suppression(1_000, DEFAULT_LEASE_MS)
            .unwrap_err()
            .contains("DisabledByBuild"));
        assert!(!driver.packet_is_suppression_candidate("HID\\BARNEN&COL01", 1_100));
        assert!(!driver.packet_is_suppression_candidate("hid\\dennis&col01", 1_100));
        let status = driver.query_status(1_100).unwrap();
        assert_eq!(
            status.fail_open_reason,
            Some(FailOpenReason::DisabledByBuild)
        );
        assert!(!status.suppression_active);
    }

    #[test]
    fn service_disconnect_and_device_disconnect_fail_open() {
        let mut driver = configured_driver();
        driver.disconnect_service();
        assert!(!driver.packet_is_suppression_candidate("hid\\barnen&col01", 1_100));

        let mut driver = configured_driver();
        driver.device_disconnected();
        assert!(!driver.packet_is_suppression_candidate("hid\\barnen&col01", 1_100));
    }

    #[test]
    fn invalid_configuration_fails_open() {
        let mut driver = configured_driver();
        driver.invalidate_configuration();
        assert!(!driver.packet_is_suppression_candidate("hid\\barnen&col01", 1_100));
    }

    #[test]
    fn driver_unavailable_is_fail_open() {
        let mut driver = configured_driver();
        driver.make_unavailable();
        assert!(driver.query_status(1_000).is_err());
        assert!(!driver.packet_is_suppression_candidate("hid\\barnen&col01", 1_000));
    }

    #[test]
    fn malformed_and_version_mismatched_protocol_messages_are_rejected() {
        let valid = ProtocolHeader {
            magic: PROTOCOL_MAGIC,
            version: PROTOCOL_VERSION,
            size: 24,
            command: ProtocolCommand::QueryStatus as u32,
            request_id: 7,
        };
        assert_eq!(
            validate_protocol_header(valid, 23, 24, ProtocolCommand::QueryStatus),
            Err(ProtocolError::MalformedMessage)
        );
        assert_eq!(
            validate_protocol_header(
                ProtocolHeader {
                    version: PROTOCOL_VERSION + 1,
                    ..valid
                },
                24,
                24,
                ProtocolCommand::QueryStatus
            ),
            Err(ProtocolError::VersionMismatch)
        );
    }

    #[test]
    fn ring_overflow_never_changes_host_pass_through() {
        let mut ring = ObservationRing::new(2);
        assert_eq!(ring.observe(10), HostDelivery::PassThrough);
        assert_eq!(ring.observe(20), HostDelivery::PassThrough);
        assert_eq!(ring.observe(30), HostDelivery::PassThrough);
        assert_eq!(ring.overflow_count(), 1);
        let events = ring.drain();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[1].sequence, 2);
    }

    #[test]
    fn readiness_rejects_ambiguous_or_unsafe_test_machine() {
        let barnen = keyboard("barnen", "container-b", "hid\\barnen", "1A2C");
        let dennis = keyboard("dennis", "container-d", "hid\\dennis", "1A2C");
        let result = evaluate_pass_through_readiness(&PassThroughReadinessEvidence {
            supported_windows_x64: true,
            driver_build_is_x64: true,
            physical_keyboards: &[barnen.clone(), dennis.clone()],
            dennis_matches: std::slice::from_ref(&dennis),
            barnen_matches: &[barnen.clone(), barnen],
            keyboard_stack_enumerated: true,
            driver_already_installed: false,
            remote_recovery_confirmed: false,
        });
        let PassThroughReadiness::NotReady(reasons) = result else {
            panic!("unsafe machine was accepted")
        };
        assert!(reasons.contains(&ReadinessReason::AmbiguousBarnen));
        assert!(reasons.contains(&ReadinessReason::RemoteRecoveryNotConfirmed));
    }

    #[test]
    fn inf_generation_requires_exact_non_primary_tlc() {
        let barnen = keyboard("barnen", "container-b", "hid\\barnen", "1A2C");
        let dennis = keyboard("dennis", "container-d", "hid\\dennis", "1A2C");
        let generated = generate_test_machine_inf(
            "target=REPLACE_WITH_EXACT_KEYBOARD_TLC_HARDWARE_ID",
            &InfGenerationRequest {
                target: &barnen,
                dennis: &dennis,
                expected_container_id: "container-b",
                target_match_count: 1,
                present_physical_keyboard_count: 2,
                exact_keyboard_tlc_hardware_id: "HID\\VID_1A2C&PID_4C5E&COL01",
            },
        )
        .unwrap();
        assert!(generated.contains("HID\\VID_1A2C&PID_4C5E&COL01"));
        assert!(generate_test_machine_inf(
            "REPLACE_WITH_EXACT_KEYBOARD_TLC_HARDWARE_ID",
            &InfGenerationRequest {
                target: &dennis,
                dennis: &dennis,
                expected_container_id: "container-d",
                target_match_count: 1,
                present_physical_keyboard_count: 2,
                exact_keyboard_tlc_hardware_id: "HID\\PRIMARY",
            }
        )
        .is_err());
    }

    #[test]
    fn inf_generation_rejects_mismatched_explicit_container() {
        let barnen = keyboard("barnen", "container-b", "hid\\barnen", "1A2C");
        let dennis = keyboard("dennis", "container-d", "hid\\dennis", "1532");
        let result = generate_test_machine_inf(
            "REPLACE_WITH_EXACT_KEYBOARD_TLC_HARDWARE_ID",
            &InfGenerationRequest {
                target: &barnen,
                dennis: &dennis,
                expected_container_id: "container-other",
                target_match_count: 1,
                present_physical_keyboard_count: 2,
                exact_keyboard_tlc_hardware_id: "HID\\VID_1A2C&PID_4C5E&COL01",
            },
        );
        assert!(result.is_err());
    }
}
