//! Enumeration of active physical displays.

use serde::Serialize;

#[cfg(target_os = "windows")]
mod windows;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayDevice {
    /// Plug-and-Play identifier supplied by Windows, where available.
    pub id: String,
    /// Present monitor devnode identity used by persisted seat assignments.
    pub pnp_instance_id: Option<String>,
    pub device_name: String,
    /// GDI desktop target name, for example `\\.\DISPLAY2`.
    pub desktop_device_name: String,
    pub friendly_name: Option<String>,
    pub device_path: Option<String>,
    pub adapter_name: String,
    pub adapter_device_id: Option<String>,
    pub primary: bool,
    /// Current virtual-desktop coordinates. Never persisted as identity.
    pub bounds: Option<DisplayBounds>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PnpMonitor {
    pub instance_id: String,
    pub friendly_name: Option<String>,
    pub manufacturer: Option<String>,
    pub hardware_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayConfigTarget {
    pub id: String,
    pub adapter_luid: String,
    pub target_id: u32,
    pub friendly_name: Option<String>,
    pub monitor_device_path: Option<String>,
    pub active: bool,
    pub available: bool,
    pub output_technology: i32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayDiscovery {
    /// GDI displays exposed to the current desktop/session.
    pub session_visible: Vec<DisplayDevice>,
    /// Present monitor-class PnP nodes, independent from GDI desktop attachment.
    pub pnp_monitors: Vec<PnpMonitor>,
    /// Active and inactive targets reported by the CCD DisplayConfig API.
    pub display_config_targets: Vec<DisplayConfigTarget>,
}

#[cfg(target_os = "windows")]
pub fn enumerate() -> Result<DisplayDiscovery, String> {
    windows::enumerate()
}

#[cfg(target_os = "windows")]
pub(crate) fn enumerate_session_visible() -> Result<Vec<DisplayDevice>, String> {
    windows::enumerate_session_visible_displays()
}

#[cfg(not(target_os = "windows"))]
pub fn enumerate() -> Result<DisplayDiscovery, String> {
    Err("display discovery is only implemented for Windows".to_owned())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn enumerate_session_visible() -> Result<Vec<DisplayDevice>, String> {
    Err("display discovery is only implemented for Windows".to_owned())
}
