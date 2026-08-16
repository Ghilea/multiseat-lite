//! Detection of Windows session context and known multiseat middleware.

use serde::Serialize;

#[cfg(target_os = "windows")]
mod windows;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceRuntimeState {
    NotInstalled,
    Stopped,
    StartPending,
    StopPending,
    Running,
    ContinuePending,
    PausePending,
    Paused,
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedServiceStatus {
    pub name: String,
    pub installed: bool,
    pub running: bool,
    pub state: ServiceRuntimeState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AsterWorkplaceState {
    Active,
    Inactive,
    Unknown,
}

#[derive(Clone, Debug, Default)]
pub struct AsterDeviceEvidence {
    pub mut_enx_device_node_present: bool,
    pub mut_enx_raw_input_visible: bool,
    pub other_aster_related_device_present: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsterEnvironmentStatus {
    pub installation_detected: bool,
    pub related_service_installed: bool,
    pub related_service_running: bool,
    pub mut_enx_device_node_present: bool,
    pub mut_enx_raw_input_visible: bool,
    pub registry_markers_present: bool,
    pub registry_markers: Vec<String>,
    pub related_services: Vec<RelatedServiceStatus>,
    pub workplace_state: AsterWorkplaceState,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentInfo {
    pub current_session_id: u32,
    pub aster: AsterEnvironmentStatus,
}

#[cfg(target_os = "windows")]
pub fn detect(device_evidence: AsterDeviceEvidence) -> Result<EnvironmentInfo, String> {
    windows::detect(device_evidence)
}

#[cfg(not(target_os = "windows"))]
pub fn detect(device_evidence: AsterDeviceEvidence) -> Result<EnvironmentInfo, String> {
    Ok(EnvironmentInfo {
        current_session_id: 0,
        aster: AsterEnvironmentStatus {
            installation_detected: device_evidence.mut_enx_device_node_present
                || device_evidence.mut_enx_raw_input_visible
                || device_evidence.other_aster_related_device_present,
            related_service_installed: false,
            related_service_running: false,
            mut_enx_device_node_present: device_evidence.mut_enx_device_node_present,
            mut_enx_raw_input_visible: device_evidence.mut_enx_raw_input_visible,
            registry_markers_present: false,
            registry_markers: Vec::new(),
            related_services: Vec::new(),
            workplace_state: AsterWorkplaceState::Unknown,
        },
    })
}
