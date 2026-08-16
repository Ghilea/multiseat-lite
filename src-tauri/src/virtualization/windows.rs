use std::{
    ffi::c_void,
    mem::size_of,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    slice,
};

use ::windows::{
    core::{HRESULT, PCWSTR},
    Win32::{
        Foundation::ERROR_SERVICE_DOES_NOT_EXIST,
        System::{
            Registry::{
                RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
                RRF_SUBKEY_WOW6464KEY,
            },
            Services::{
                CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_HANDLE,
                SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_CONTINUE_PENDING,
                SERVICE_PAUSED, SERVICE_PAUSE_PENDING, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
                SERVICE_START_PENDING, SERVICE_STATUS_PROCESS, SERVICE_STOPPED,
                SERVICE_STOP_PENDING,
            },
            Threading::{
                IsProcessorFeaturePresent, PF_SECOND_LEVEL_ADDRESS_TRANSLATION,
                PF_VIRT_FIRMWARE_ENABLED, PROCESSOR_FEATURE_ID,
            },
        },
    },
};

use super::*;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PF_HYPERVISOR_PRESENT: PROCESSOR_FEATURE_ID = PROCESSOR_FEATURE_ID(23);
const CURRENT_VERSION_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
const HYPER_V_SERVICES: [&str; 2] = ["vmms", "vmcompute"];

struct ServiceHandle(SC_HANDLE);

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // SAFETY: This wrapper owns a handle returned by an SCM API.
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

pub fn probe(
    physical_devices: &[PhysicalInputDevice],
    aster: AsterEnvironmentStatus,
    current_session_id: u32,
) -> Result<BackendProbeSnapshot, String> {
    let system = probe_system(current_session_id);
    let virtual_box = probe_virtual_box();
    let hyper_v = probe_hyper_v(system.hypervisor_present, system.edition.as_deref())?;
    let usb_correlations = correlate_usb_devices(physical_devices, &virtual_box);
    let backend_capabilities = vec![
        BackendCapabilities {
            backend_kind: BackendKind::VirtualBox,
            available: virtual_box.operational,
            status_probe: virtual_box.operational,
            usb_discovery: virtual_box.operational && virtual_box.probe_error.is_none(),
            activation_supported: virtual_box.operational,
        },
        BackendCapabilities {
            backend_kind: BackendKind::HyperV,
            available: hyper_v.feature_state == WindowsFeatureState::Enabled
                || hyper_v.hypervisor_active,
            status_probe: true,
            usb_discovery: false,
            activation_supported: false,
        },
        BackendCapabilities {
            backend_kind: BackendKind::Native,
            available: false,
            status_probe: false,
            usb_discovery: false,
            activation_supported: false,
        },
    ];

    Ok(BackendProbeSnapshot {
        system,
        virtual_box,
        hyper_v,
        aster,
        selected_backend: None,
        backend_capabilities,
        usb_correlations,
    })
}

fn probe_system(current_session_id: u32) -> WindowsSystemInfo {
    let build_number = registry_string(CURRENT_VERSION_KEY, "CurrentBuildNumber");
    let ubr = registry_dword(CURRENT_VERSION_KEY, "UBR");
    WindowsSystemInfo {
        product_name: registry_string(CURRENT_VERSION_KEY, "ProductName"),
        edition: registry_string(CURRENT_VERSION_KEY, "EditionID"),
        display_version: registry_string(CURRENT_VERSION_KEY, "DisplayVersion"),
        build: build_number.map(|build| match ubr {
            Some(revision) => format!("{build}.{revision}"),
            None => build,
        }),
        architecture: std::env::consts::ARCH.to_owned(),
        current_session_id,
        // SAFETY: IsProcessorFeaturePresent is a read-only system capability query.
        hypervisor_present: unsafe { IsProcessorFeaturePresent(PF_HYPERVISOR_PRESENT) }.as_bool(),
        firmware_virtualization_enabled: detection_state(unsafe {
            IsProcessorFeaturePresent(PF_VIRT_FIRMWARE_ENABLED).as_bool()
        }),
        second_level_address_translation: detection_state(unsafe {
            IsProcessorFeaturePresent(PF_SECOND_LEVEL_ADDRESS_TRANSLATION).as_bool()
        }),
    }
}

fn probe_virtual_box() -> VirtualBoxStatus {
    let (installed, path) = find_vbox_manage();
    let Some(path) = path else {
        return VirtualBoxStatus {
            installed,
            vbox_manage_path: None,
            version: None,
            operational: false,
            probe_error: installed
                .then(|| "VirtualBox registration was found but VBoxManage.exe was not".to_owned()),
            parser_warnings: Vec::new(),
            usb_devices: Vec::new(),
        };
    };
    let path_text = path.display().to_string();
    let version_output = hidden_command(&path, &["--version"]);
    let (version, operational, mut probe_error) = match version_output {
        Ok(output) if output.status.success() => (nonempty_stdout(&output), true, None),
        Ok(output) => (
            None,
            false,
            Some(format_command_failure("VBoxManage --version", &output)),
        ),
        Err(error) => (
            None,
            false,
            Some(format!("could not execute VBoxManage --version: {error}")),
        ),
    };

    let mut parser_warnings = Vec::new();
    let usb_devices = if operational {
        match hidden_command(&path, &["list", "usbhost"]) {
            Ok(output) if output.status.success() => match String::from_utf8(output.stdout) {
                Ok(stdout) => {
                    let parsed = parse_virtualbox_usbhost(&stdout);
                    parser_warnings = parsed.warnings;
                    parsed.devices
                }
                Err(error) => {
                    probe_error = Some(format!("VBoxManage usbhost output was not UTF-8: {error}"));
                    Vec::new()
                }
            },
            Ok(output) => {
                probe_error = Some(format_command_failure("VBoxManage list usbhost", &output));
                Vec::new()
            }
            Err(error) => {
                probe_error = Some(format!(
                    "could not execute VBoxManage list usbhost: {error}"
                ));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    VirtualBoxStatus {
        installed: true,
        vbox_manage_path: Some(path_text),
        version,
        operational,
        probe_error,
        parser_warnings,
        usb_devices,
    }
}

fn probe_hyper_v(
    hypervisor_active: bool,
    windows_edition: Option<&str>,
) -> Result<HyperVStatus, String> {
    let (feature_state, feature_probe_error) =
        if windows_edition.is_some_and(|edition| edition.to_ascii_lowercase().contains("core")) {
            (WindowsFeatureState::Unavailable, None)
        } else {
            probe_hyper_v_feature()
        };
    let manager = unsafe { OpenSCManagerW(None, None, SC_MANAGER_CONNECT) }
        .map(ServiceHandle)
        .map_err(|error| format!("OpenSCManagerW for Hyper-V probe failed: {error}"))?;
    let services = HYPER_V_SERVICES
        .iter()
        .map(|name| query_service(&manager, name))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(HyperVStatus {
        feature_state,
        feature_probe_error,
        services,
        hypervisor_active,
    })
}

fn probe_hyper_v_feature() -> (WindowsFeatureState, Option<String>) {
    let script = "(Get-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V-All -ErrorAction Stop).State.ToString()";
    match hidden_command(
        Path::new("powershell.exe"),
        &["-NoProfile", "-NonInteractive", "-Command", script],
    ) {
        Ok(output) if output.status.success() => {
            let state = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_ascii_lowercase();
            let parsed = match state.as_str() {
                "enabled" => WindowsFeatureState::Enabled,
                "disabled" | "disablepending" => WindowsFeatureState::Disabled,
                _ => WindowsFeatureState::Unknown,
            };
            (parsed, None)
        }
        Ok(output) => (
            WindowsFeatureState::Unknown,
            Some(format_command_failure(
                "Get-WindowsOptionalFeature",
                &output,
            )),
        ),
        Err(error) => (
            WindowsFeatureState::Unknown,
            Some(format!("could not query Hyper-V optional feature: {error}")),
        ),
    }
}

fn query_service(manager: &ServiceHandle, name: &str) -> Result<HostServiceStatus, String> {
    let wide_name = wide(name);
    let service = match unsafe {
        OpenServiceW(manager.0, PCWSTR(wide_name.as_ptr()), SERVICE_QUERY_STATUS)
    } {
        Ok(handle) => ServiceHandle(handle),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_SERVICE_DOES_NOT_EXIST.0) => {
            return Ok(HostServiceStatus {
                name: name.to_owned(),
                installed: false,
                running: false,
                state: "notInstalled".to_owned(),
            });
        }
        Err(error) => return Err(format!("OpenServiceW('{name}') failed: {error}")),
    };
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0;
    let buffer = unsafe {
        slice::from_raw_parts_mut(
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            size_of::<SERVICE_STATUS_PROCESS>(),
        )
    };
    unsafe { QueryServiceStatusEx(service.0, SC_STATUS_PROCESS_INFO, Some(buffer), &mut needed) }
        .map_err(|error| format!("QueryServiceStatusEx('{name}') failed: {error}"))?;
    let (state, running) = match status.dwCurrentState {
        SERVICE_STOPPED => ("stopped", false),
        SERVICE_START_PENDING => ("startPending", false),
        SERVICE_STOP_PENDING => ("stopPending", false),
        SERVICE_RUNNING => ("running", true),
        SERVICE_CONTINUE_PENDING => ("continuePending", false),
        SERVICE_PAUSE_PENDING => ("pausePending", false),
        SERVICE_PAUSED => ("paused", false),
        _ => ("unknown", false),
    };
    Ok(HostServiceStatus {
        name: name.to_owned(),
        installed: true,
        running,
        state: state.to_owned(),
    })
}

pub(super) fn vbox_manage_path() -> Option<PathBuf> {
    find_vbox_manage().1
}

fn find_vbox_manage() -> (bool, Option<PathBuf>) {
    let install_dir = registry_string(r"SOFTWARE\Oracle\VirtualBox", "InstallDir");
    let registered = install_dir.is_some()
        || registry_string(r"SOFTWARE\Oracle\VirtualBox", "Version").is_some();
    if let Some(install_dir) = install_dir {
        let path = PathBuf::from(install_dir).join("VBoxManage.exe");
        if path.is_file() {
            return (true, Some(path));
        }
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        let path = PathBuf::from(program_files)
            .join("Oracle")
            .join("VirtualBox")
            .join("VBoxManage.exe");
        if path.is_file() {
            return (true, Some(path));
        }
    }
    let path = hidden_command(Path::new("where.exe"), &["VBoxManage.exe"])
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|stdout| {
            stdout
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(PathBuf::from)
        })
        .filter(|path| path.is_file());
    (registered || path.is_some(), path)
}

fn hidden_command(program: &Path, arguments: &[&str]) -> std::io::Result<Output> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

fn format_command_failure(operation: &str, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    format!(
        "{operation} failed with exit code {:?}{}",
        output.status.code(),
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    )
}

fn nonempty_stdout(output: &Output) -> Option<String> {
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn detection_state(value: bool) -> DetectionState {
    if value {
        DetectionState::Yes
    } else {
        DetectionState::No
    }
}

pub(crate) fn firmware_virtualization_state() -> DetectionState {
    // SAFETY: IsProcessorFeaturePresent is a read-only system capability query.
    detection_state(unsafe { IsProcessorFeaturePresent(PF_VIRT_FIRMWARE_ENABLED).as_bool() })
}

fn registry_string(subkey: &str, value: &str) -> Option<String> {
    let subkey = wide(subkey);
    let value = wide(value);
    let flags = RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY;
    let mut bytes = 0;
    let result = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            flags,
            None,
            None,
            Some(&mut bytes),
        )
    };
    if result.0 != 0 || bytes < 2 {
        return None;
    }
    let mut buffer = vec![0_u16; bytes as usize / 2];
    let result = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            flags,
            None,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            Some(&mut bytes),
        )
    };
    if result.0 != 0 {
        return None;
    }
    let length = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..length]))
}

fn registry_dword(subkey: &str, value: &str) -> Option<u32> {
    let subkey = wide(subkey);
    let value = wide(value);
    let mut result_value = 0_u32;
    let mut bytes = size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD | RRF_SUBKEY_WOW6464KEY,
            None,
            Some((&mut result_value as *mut u32).cast::<c_void>()),
            Some(&mut bytes),
        )
    };
    (result.0 == 0).then_some(result_value)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
