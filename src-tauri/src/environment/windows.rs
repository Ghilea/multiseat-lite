use std::{mem::size_of, slice};

use windows::{
    core::{HRESULT, PCWSTR},
    Win32::{
        Foundation::ERROR_SERVICE_DOES_NOT_EXIST,
        System::{
            Registry::{
                RegCloseKey, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY,
            },
            RemoteDesktop::ProcessIdToSessionId,
            Services::{
                CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_HANDLE,
                SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_CONTINUE_PENDING,
                SERVICE_PAUSED, SERVICE_PAUSE_PENDING, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
                SERVICE_START_PENDING, SERVICE_STATUS_PROCESS, SERVICE_STOPPED,
                SERVICE_STOP_PENDING,
            },
            Threading::GetCurrentProcessId,
        },
    },
};

use super::{
    AsterDeviceEvidence, AsterEnvironmentStatus, AsterWorkplaceState, EnvironmentInfo,
    RelatedServiceStatus, ServiceRuntimeState,
};

const ASTER_REGISTRY_MARKERS: [(&str, &str); 1] =
    [("ASTER software registry key", r"SOFTWARE\IBIK")];
const ASTER_SERVICES: [&str; 2] = ["MUTENX_SERVICE", "MUTESV_SERVICE"];

struct ServiceHandle(SC_HANDLE);

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // SAFETY: The handle was returned by an SCM API and is owned by this wrapper.
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

pub fn detect(device_evidence: AsterDeviceEvidence) -> Result<EnvironmentInfo, String> {
    let mut session_id = 0;
    // SAFETY: Both functions receive valid output storage and the current process ID.
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session_id) }
        .map_err(|error| format!("ProcessIdToSessionId failed: {error}"))?;

    let registry_markers: Vec<String> = ASTER_REGISTRY_MARKERS
        .iter()
        .filter(|(_, path)| registry_key_exists(path))
        .map(|(label, _)| (*label).to_owned())
        .collect();
    let related_services = query_related_services()?;
    let aster = build_aster_status(registry_markers, related_services, device_evidence);

    Ok(EnvironmentInfo {
        current_session_id: session_id,
        aster,
    })
}

fn build_aster_status(
    registry_markers: Vec<String>,
    related_services: Vec<RelatedServiceStatus>,
    device_evidence: AsterDeviceEvidence,
) -> AsterEnvironmentStatus {
    let related_service_installed = related_services.iter().any(|service| service.installed);
    let related_service_running = related_services.iter().any(|service| service.running);
    let installation_detected = !registry_markers.is_empty()
        || related_service_installed
        || device_evidence.mut_enx_device_node_present
        || device_evidence.mut_enx_raw_input_visible
        || device_evidence.other_aster_related_device_present;

    AsterEnvironmentStatus {
        installation_detected,
        related_service_installed,
        related_service_running,
        mut_enx_device_node_present: device_evidence.mut_enx_device_node_present,
        mut_enx_raw_input_visible: device_evidence.mut_enx_raw_input_visible,
        registry_markers_present: !registry_markers.is_empty(),
        registry_markers,
        related_services,
        // No documented read-only API used here establishes ASTER's workplace mode.
        workplace_state: AsterWorkplaceState::Unknown,
    }
}

fn query_related_services() -> Result<Vec<RelatedServiceStatus>, String> {
    // SAFETY: Null machine/database names select the local machine and active database.
    let manager = unsafe { OpenSCManagerW(None, None, SC_MANAGER_CONNECT) }
        .map(ServiceHandle)
        .map_err(|error| format!("OpenSCManagerW(SC_MANAGER_CONNECT) failed: {error}"))?;
    ASTER_SERVICES
        .iter()
        .map(|name| query_service(&manager, name))
        .collect()
}

fn query_service(manager: &ServiceHandle, name: &str) -> Result<RelatedServiceStatus, String> {
    let wide_name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: manager is valid and wide_name is nul-terminated for the duration of the call.
    let service = match unsafe {
        OpenServiceW(manager.0, PCWSTR(wide_name.as_ptr()), SERVICE_QUERY_STATUS)
    } {
        Ok(handle) => ServiceHandle(handle),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_SERVICE_DOES_NOT_EXIST.0) => {
            return Ok(RelatedServiceStatus {
                name: name.to_owned(),
                installed: false,
                running: false,
                state: ServiceRuntimeState::NotInstalled,
            });
        }
        Err(error) => return Err(format!("OpenServiceW('{name}') failed: {error}")),
    };

    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut bytes_needed = 0;
    // SAFETY: status is valid writable storage represented as a byte slice of its exact size.
    let buffer = unsafe {
        slice::from_raw_parts_mut(
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            size_of::<SERVICE_STATUS_PROCESS>(),
        )
    };
    // SAFETY: service has SERVICE_QUERY_STATUS access and buffer is correctly sized.
    unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            Some(buffer),
            &mut bytes_needed,
        )
    }
    .map_err(|error| format!("QueryServiceStatusEx('{name}') failed: {error}"))?;

    let state = service_runtime_state(status.dwCurrentState);
    Ok(RelatedServiceStatus {
        name: name.to_owned(),
        installed: true,
        running: state == ServiceRuntimeState::Running,
        state,
    })
}

fn service_runtime_state(
    state: windows::Win32::System::Services::SERVICE_STATUS_CURRENT_STATE,
) -> ServiceRuntimeState {
    match state {
        SERVICE_STOPPED => ServiceRuntimeState::Stopped,
        SERVICE_START_PENDING => ServiceRuntimeState::StartPending,
        SERVICE_STOP_PENDING => ServiceRuntimeState::StopPending,
        SERVICE_RUNNING => ServiceRuntimeState::Running,
        SERVICE_CONTINUE_PENDING => ServiceRuntimeState::ContinuePending,
        SERVICE_PAUSE_PENDING => ServiceRuntimeState::PausePending,
        SERVICE_PAUSED => ServiceRuntimeState::Paused,
        _ => ServiceRuntimeState::Unknown,
    }
}

fn registry_key_exists(path: &str) -> bool {
    let wide_path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut key = HKEY::default();
    // SAFETY: wide_path is nul-terminated and key points to valid output storage.
    let result = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(wide_path.as_ptr()),
            None,
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    if result.0 != 0 {
        return false;
    }

    // SAFETY: key was opened successfully above and is owned by this function.
    let _ = unsafe { RegCloseKey(key) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Services::SERVICE_STATUS_CURRENT_STATE;

    #[test]
    fn maps_documented_service_runtime_states() {
        assert_eq!(
            service_runtime_state(SERVICE_RUNNING),
            ServiceRuntimeState::Running
        );
        assert_eq!(
            service_runtime_state(SERVICE_STOPPED),
            ServiceRuntimeState::Stopped
        );
        assert_eq!(
            service_runtime_state(SERVICE_STATUS_CURRENT_STATE(999)),
            ServiceRuntimeState::Unknown
        );
    }

    #[test]
    fn installed_but_stopped_does_not_become_running_or_active() {
        let status = build_aster_status(
            vec!["ASTER software registry key".into()],
            vec![RelatedServiceStatus {
                name: "MUTENX_SERVICE".into(),
                installed: true,
                running: false,
                state: ServiceRuntimeState::Stopped,
            }],
            AsterDeviceEvidence {
                mut_enx_device_node_present: true,
                ..Default::default()
            },
        );
        assert!(status.installation_detected);
        assert!(status.related_service_installed);
        assert!(!status.related_service_running);
        assert_eq!(status.workplace_state, AsterWorkplaceState::Unknown);
    }
}
