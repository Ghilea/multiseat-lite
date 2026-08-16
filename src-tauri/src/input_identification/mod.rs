//! Interactive identification of session-visible Raw Input keyboard and mouse devices.

use serde::Serialize;

use crate::devices::InputDeviceType;
#[cfg(not(target_os = "windows"))]
use crate::keyboard_routing::{
    GuestKeyboardSink, KeyboardRoutingDiagnosticStatus, KeyboardRoutingTransport,
};

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::InputIdentificationService;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentifiedInputDevice {
    pub device_type: InputDeviceType,
    pub stable_physical_device_id: String,
    pub container_id: Option<String>,
    pub raw_input_device_path: String,
    pub friendly_name: Option<String>,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub current_session_id: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentificationStatus {
    pub active: bool,
    pub session_raw_input_devices: usize,
    pub correlated_physical_devices: usize,
}

#[cfg(not(target_os = "windows"))]
#[derive(Default)]
pub struct InputIdentificationService;

#[cfg(not(target_os = "windows"))]
impl InputIdentificationService {
    pub fn start(&self, _app: tauri::AppHandle) -> Result<IdentificationStatus, String> {
        Err("input identification is only implemented for Windows".to_owned())
    }

    pub fn stop(&self) -> Result<IdentificationStatus, String> {
        Ok(IdentificationStatus {
            active: false,
            session_raw_input_devices: 0,
            correlated_physical_devices: 0,
        })
    }

    pub fn start_keyboard_routing(
        &self,
        _app: tauri::AppHandle,
        _target_physical_device_id: String,
    ) -> Result<KeyboardRoutingDiagnosticStatus, String> {
        Err("keyboard routing diagnostic is only implemented for Windows".to_owned())
    }

    pub fn stop_keyboard_routing(&self) -> Result<KeyboardRoutingDiagnosticStatus, String> {
        Ok(KeyboardRoutingDiagnosticStatus {
            active: false,
            target_physical_device_id: None,
            current_session_id: 0,
            real_guest_injection_enabled: false,
            successful_guest_sends: 0,
            host_input_suppression: "notImplemented".to_owned(),
            transport: KeyboardRoutingTransport::DiagnosticOnly,
        })
    }

    pub fn keyboard_routing_status(&self) -> Option<KeyboardRoutingDiagnosticStatus> {
        None
    }

    pub fn start_keyboard_injection(
        &self,
        _app: tauri::AppHandle,
        _target_physical_device_id: String,
        _sink: Box<dyn GuestKeyboardSink + Send>,
    ) -> Result<KeyboardRoutingDiagnosticStatus, String> {
        Err("keyboard injection is only implemented for Windows".to_owned())
    }
}
