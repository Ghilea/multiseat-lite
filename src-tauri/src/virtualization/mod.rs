//! Virtualization discovery and the VirtualBox second-seat backend boundary.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    devices::PhysicalInputDevice,
    environment::AsterEnvironmentStatus,
    seats::{ResolvedSeat, SeatId},
};

mod virtualbox;
#[cfg(target_os = "windows")]
mod windows;

pub use virtualbox::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BackendKind {
    VirtualBox,
    HyperV,
    Native,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendCapabilities {
    pub backend_kind: BackendKind,
    pub available: bool,
    pub status_probe: bool,
    pub usb_discovery: bool,
    /// Always false during Milestone 3A.
    pub activation_supported: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatValidation {
    pub valid: bool,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SeatRuntimeStatus {
    Unsupported,
    NotConfigured,
    Stopped,
    Starting,
    Running,
    PartiallyRunning,
    Stopping,
    Error,
}

pub trait SeatBackend {
    fn backend_kind(&self) -> BackendKind;
    fn capabilities(&self) -> Result<BackendCapabilities, String>;
    fn validate_seat(&self, seat: &ResolvedSeat) -> Result<SeatValidation, String>;
    fn status(&self, seat_id: &SeatId) -> Result<SeatRuntimeStatus, String>;

    fn start(&self, _seat: &ResolvedSeat) -> Result<(), String> {
        Err("seat activation is not supported by this backend".to_owned())
    }

    fn stop(&self, _seat_id: &SeatId) -> Result<(), String> {
        Err("seat activation is not supported by this backend".to_owned())
    }
}

pub struct ProbeOnlyBackend {
    capabilities: BackendCapabilities,
}

impl ProbeOnlyBackend {
    pub fn new(capabilities: BackendCapabilities) -> Self {
        Self { capabilities }
    }
}

impl SeatBackend for ProbeOnlyBackend {
    fn backend_kind(&self) -> BackendKind {
        self.capabilities.backend_kind
    }

    fn capabilities(&self) -> Result<BackendCapabilities, String> {
        Ok(self.capabilities.clone())
    }

    fn validate_seat(&self, seat: &ResolvedSeat) -> Result<SeatValidation, String> {
        let mut warnings = Vec::new();
        if seat.devices.display.is_none() {
            warnings.push("seat has no display assignment".to_owned());
        }
        if seat.devices.keyboard.is_none() || seat.devices.mouse.is_none() {
            warnings.push("seat does not have both keyboard and mouse assignments".to_owned());
        }
        Ok(SeatValidation {
            valid: self.capabilities.available,
            warnings,
            errors: (!self.capabilities.available)
                .then(|| "backend is not available".to_owned())
                .into_iter()
                .collect(),
        })
    }

    fn status(&self, _seat_id: &SeatId) -> Result<SeatRuntimeStatus, String> {
        Ok(SeatRuntimeStatus::NotConfigured)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DetectionState {
    Yes,
    No,
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsSystemInfo {
    pub product_name: Option<String>,
    pub edition: Option<String>,
    pub display_version: Option<String>,
    pub build: Option<String>,
    pub architecture: String,
    pub current_session_id: u32,
    pub hypervisor_present: bool,
    pub firmware_virtualization_enabled: DetectionState,
    pub second_level_address_translation: DetectionState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostServiceStatus {
    pub name: String,
    pub installed: bool,
    pub running: bool,
    pub state: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowsFeatureState {
    Enabled,
    Disabled,
    Unavailable,
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HyperVStatus {
    pub feature_state: WindowsFeatureState,
    pub feature_probe_error: Option<String>,
    pub services: Vec<HostServiceStatus>,
    pub hypervisor_active: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualBoxUsbDevice {
    pub uuid: String,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub revision: Option<String>,
    pub port: Option<String>,
    pub usb_version_speed: Option<String>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    pub address: Option<String>,
    pub current_state: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualBoxStatus {
    pub installed: bool,
    pub vbox_manage_path: Option<String>,
    pub version: Option<String>,
    pub operational: bool,
    pub probe_error: Option<String>,
    pub parser_warnings: Vec<String>,
    pub usb_devices: Vec<VirtualBoxUsbDevice>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VirtualBoxUsbParseResult {
    pub devices: Vec<VirtualBoxUsbDevice>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UsbCorrelationState {
    Exact,
    Unambiguous,
    Ambiguous,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsbCorrelation {
    pub physical_device_id: String,
    pub container_id: Option<String>,
    pub state: UsbCorrelationState,
    pub matched_uuid: Option<String>,
    pub candidate_uuids: Vec<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendProbeSnapshot {
    pub system: WindowsSystemInfo,
    pub virtual_box: VirtualBoxStatus,
    pub hyper_v: HyperVStatus,
    pub aster: AsterEnvironmentStatus,
    pub selected_backend: Option<BackendKind>,
    pub backend_capabilities: Vec<BackendCapabilities>,
    pub usb_correlations: Vec<UsbCorrelation>,
}

pub fn correlate_usb_devices(
    physical_devices: &[PhysicalInputDevice],
    virtual_box: &VirtualBoxStatus,
) -> Vec<UsbCorrelation> {
    physical_devices
        .iter()
        .map(|physical| correlate_usb_device(physical, virtual_box))
        .collect()
}

fn correlate_usb_device(
    physical: &PhysicalInputDevice,
    virtual_box: &VirtualBoxStatus,
) -> UsbCorrelation {
    let unavailable = |reason: &str| UsbCorrelation {
        physical_device_id: physical.id.clone(),
        container_id: physical.container_id.clone(),
        state: UsbCorrelationState::Unavailable,
        matched_uuid: None,
        candidate_uuids: Vec::new(),
        reason: reason.to_owned(),
    };
    if !virtual_box.installed || !virtual_box.operational {
        return unavailable("VirtualBox USB discovery is unavailable");
    }
    let (Some(vendor), Some(product)) = (&physical.vendor_id, &physical.product_id) else {
        return unavailable("Windows device has no VID/PID");
    };
    let candidates: Vec<_> = virtual_box
        .usb_devices
        .iter()
        .filter(|device| {
            device
                .vendor_id
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(vendor))
                && device
                    .product_id
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(product))
        })
        .collect();
    if candidates.is_empty() {
        return unavailable("no VirtualBox USB device has matching VID/PID");
    }

    if let Some(serial) = physical.serial_number.as_deref() {
        let serial_matches: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|device| {
                device
                    .serial_number
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(serial))
            })
            .collect();
        if serial_matches.len() == 1 {
            return correlation_result(
                physical,
                UsbCorrelationState::Exact,
                serial_matches[0],
                &candidates,
                "unique serial-number match within the VID/PID candidates",
            );
        }
        if serial_matches.len() > 1 {
            return ambiguous_result(
                physical,
                &serial_matches,
                "multiple VirtualBox devices share VID/PID and serial number",
            );
        }
    }

    if candidates.len() == 1 {
        return correlation_result(
            physical,
            UsbCorrelationState::Unambiguous,
            candidates[0],
            &candidates,
            "only one VirtualBox USB candidate has matching VID/PID",
        );
    }
    ambiguous_result(
        physical,
        &candidates,
        "multiple VirtualBox USB devices share VID/PID without a unique serial match",
    )
}

fn correlation_result(
    physical: &PhysicalInputDevice,
    state: UsbCorrelationState,
    matched: &VirtualBoxUsbDevice,
    candidates: &[&VirtualBoxUsbDevice],
    reason: &str,
) -> UsbCorrelation {
    UsbCorrelation {
        physical_device_id: physical.id.clone(),
        container_id: physical.container_id.clone(),
        state,
        matched_uuid: Some(matched.uuid.clone()),
        candidate_uuids: candidates
            .iter()
            .map(|device| device.uuid.clone())
            .collect(),
        reason: reason.to_owned(),
    }
}

fn ambiguous_result(
    physical: &PhysicalInputDevice,
    candidates: &[&VirtualBoxUsbDevice],
    reason: &str,
) -> UsbCorrelation {
    UsbCorrelation {
        physical_device_id: physical.id.clone(),
        container_id: physical.container_id.clone(),
        state: UsbCorrelationState::Ambiguous,
        matched_uuid: None,
        candidate_uuids: candidates
            .iter()
            .map(|device| device.uuid.clone())
            .collect(),
        reason: reason.to_owned(),
    }
}

pub fn parse_virtualbox_usbhost(output: &str) -> VirtualBoxUsbParseResult {
    let mut result = VirtualBoxUsbParseResult::default();
    let mut current: Option<BTreeMap<String, String>> = None;

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if key == "uuid" {
                if let Some(fields) = current.take() {
                    finish_usb_record(fields, &mut result);
                }
                current = Some(BTreeMap::from([("uuid".to_owned(), value)]));
            } else if let Some(fields) = current.as_mut() {
                // Known fields are extracted later; unknown future fields are retained
                // harmlessly in this record and otherwise ignored.
                fields.insert(key, value);
            }
        }
    }
    if let Some(fields) = current {
        finish_usb_record(fields, &mut result);
    }
    result
}

fn finish_usb_record(fields: BTreeMap<String, String>, result: &mut VirtualBoxUsbParseResult) {
    let Some(uuid) = fields
        .get("uuid")
        .filter(|value| !value.is_empty())
        .cloned()
    else {
        result
            .warnings
            .push("ignored VBoxManage USB record with an empty UUID".to_owned());
        return;
    };
    let vendor_id = parsed_hex_field(&fields, "vendorid", &uuid, &mut result.warnings);
    let product_id = parsed_hex_field(&fields, "productid", &uuid, &mut result.warnings);
    result.devices.push(VirtualBoxUsbDevice {
        uuid,
        vendor_id,
        product_id,
        revision: optional_field(&fields, "revision"),
        port: optional_field(&fields, "port"),
        usb_version_speed: optional_field(&fields, "usb version/speed"),
        manufacturer: optional_field(&fields, "manufacturer"),
        product: optional_field(&fields, "product"),
        serial_number: optional_field(&fields, "serialnumber"),
        address: optional_field(&fields, "address"),
        current_state: optional_field(&fields, "current state"),
    });
}

fn parsed_hex_field(
    fields: &BTreeMap<String, String>,
    key: &str,
    uuid: &str,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let value = fields.get(key)?;
    let parsed = parse_hex_id(value);
    if parsed.is_none() {
        warnings.push(format!(
            "USB device {uuid} has an invalid {key} value '{value}'"
        ));
    }
    parsed
}

fn parse_hex_id(value: &str) -> Option<String> {
    let token = value.split_whitespace().next()?.trim_start_matches("0x");
    let digits: String = token
        .chars()
        .take_while(|character| character.is_ascii_hexdigit())
        .collect();
    (!digits.is_empty()).then(|| format!("{:0>4}", digits.to_ascii_uppercase()))
}

fn optional_field(fields: &BTreeMap<String, String>, key: &str) -> Option<String> {
    fields
        .get(key)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("none"))
        .cloned()
}

#[cfg(target_os = "windows")]
pub fn probe(
    physical_devices: &[PhysicalInputDevice],
    aster: AsterEnvironmentStatus,
    current_session_id: u32,
) -> Result<BackendProbeSnapshot, String> {
    windows::probe(physical_devices, aster, current_session_id)
}

#[cfg(not(target_os = "windows"))]
pub fn probe(
    _physical_devices: &[PhysicalInputDevice],
    _aster: AsterEnvironmentStatus,
    _current_session_id: u32,
) -> Result<BackendProbeSnapshot, String> {
    Err("virtualization probing is only implemented for Windows".to_owned())
}

#[cfg(test)]
mod tests;
