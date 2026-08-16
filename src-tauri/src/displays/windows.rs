use std::{collections::BTreeMap, mem::size_of};

use windows::{
    core::PCWSTR,
    Win32::{
        Devices::{
            DeviceAndDriverInstallation::{
                SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
                SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceRegistryPropertyW, DIGCF_PRESENT,
                GUID_DEVCLASS_MONITOR, HDEVINFO, SETUP_DI_REGISTRY_PROPERTY, SPDRP_DEVICEDESC,
                SPDRP_FRIENDLYNAME, SPDRP_HARDWAREID, SPDRP_MFG, SP_DEVINFO_DATA,
            },
            Display::{
                DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
                DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
                DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_TARGET_DEVICE_NAME,
                QDC_ALL_PATHS, QDC_VIRTUAL_MODE_AWARE,
            },
        },
        Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LUID},
        Graphics::Gdi::{
            EnumDisplayDevicesW, DISPLAYCONFIG_PATH_ACTIVE, DISPLAY_DEVICEW, DISPLAY_DEVICE_ACTIVE,
            DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_MIRRORING_DRIVER,
            DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICE_REMOTE,
        },
    },
};

use super::{DisplayConfigTarget, DisplayDevice, DisplayDiscovery, PnpMonitor};

const ERROR_NO_MORE_ITEMS_HRESULT: u32 = 0x8007_0103;

struct DeviceInfoSet(HDEVINFO);

impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        // SAFETY: The handle was returned by SetupDiGetClassDevsW and is owned here.
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

pub fn enumerate() -> Result<DisplayDiscovery, String> {
    Ok(DisplayDiscovery {
        session_visible: enumerate_session_visible_displays()?,
        pnp_monitors: enumerate_pnp_monitors()?,
        display_config_targets: enumerate_display_config_targets()?,
    })
}

fn enumerate_session_visible_displays() -> Result<Vec<DisplayDevice>, String> {
    let mut displays = Vec::new();
    let mut adapter_index = 0;

    loop {
        let mut adapter = display_device();
        // SAFETY: adapter is correctly sized and writable; a null device enumerates adapters.
        let found = unsafe {
            EnumDisplayDevicesW(PCWSTR::null(), adapter_index, &mut adapter, 0).as_bool()
        };
        if !found {
            break;
        }
        adapter_index += 1;

        if !adapter
            .StateFlags
            .contains(DISPLAY_DEVICE_ATTACHED_TO_DESKTOP)
            || adapter.StateFlags.contains(DISPLAY_DEVICE_MIRRORING_DRIVER)
            || adapter.StateFlags.contains(DISPLAY_DEVICE_REMOTE)
        {
            continue;
        }

        let adapter_device_name = utf16_field(&adapter.DeviceName);
        let adapter_name = utf16_field(&adapter.DeviceString);
        let adapter_id = non_empty(utf16_field(&adapter.DeviceID));
        let primary = adapter.StateFlags.contains(DISPLAY_DEVICE_PRIMARY_DEVICE);
        let mut monitor_index = 0;

        loop {
            let mut monitor = display_device();
            // SAFETY: adapter.DeviceName is a nul-terminated fixed buffer returned by Windows.
            let found = unsafe {
                EnumDisplayDevicesW(
                    PCWSTR(adapter.DeviceName.as_ptr()),
                    monitor_index,
                    &mut monitor,
                    0,
                )
                .as_bool()
            };
            if !found {
                break;
            }
            monitor_index += 1;

            if !monitor.StateFlags.contains(DISPLAY_DEVICE_ACTIVE) {
                continue;
            }

            let device_id = utf16_field(&monitor.DeviceID);
            let device_key = utf16_field(&monitor.DeviceKey);
            let monitor_name = utf16_field(&monitor.DeviceName);
            let stable_id = non_empty(device_id.clone())
                .or_else(|| non_empty(device_key.clone()))
                .ok_or_else(|| {
                    format!(
                        "session display {monitor_name} on {adapter_device_name} has no Windows identifier"
                    )
                })?;

            displays.push(DisplayDevice {
                id: stable_id,
                device_name: monitor_name,
                friendly_name: non_empty(utf16_field(&monitor.DeviceString)),
                device_path: non_empty(device_key),
                adapter_name: adapter_name.clone(),
                adapter_device_id: adapter_id.clone(),
                primary,
            });
        }
    }

    displays.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(displays)
}

fn enumerate_pnp_monitors() -> Result<Vec<PnpMonitor>, String> {
    // SAFETY: The monitor setup class is valid; only present nodes are requested.
    let info_set = unsafe {
        SetupDiGetClassDevsW(Some(&GUID_DEVCLASS_MONITOR), None, None, DIGCF_PRESENT)
            .map(DeviceInfoSet)
            .map_err(|error| format!("SetupDiGetClassDevsW(monitor) failed: {error}"))?
    };
    let mut monitors = Vec::new();
    let mut index = 0;

    loop {
        let mut info = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        // SAFETY: info_set is valid and info is correctly sized and writable.
        if let Err(error) = unsafe { SetupDiEnumDeviceInfo(info_set.0, index, &mut info) } {
            if error.code().0 as u32 == ERROR_NO_MORE_ITEMS_HRESULT {
                break;
            }
            return Err(format!("SetupDiEnumDeviceInfo(monitor) failed: {error}"));
        }
        index += 1;

        let instance_id = device_instance_id(info_set.0, &info)?;
        let friendly_name = property_strings(info_set.0, &info, SPDRP_FRIENDLYNAME)
            .into_iter()
            .next()
            .or_else(|| {
                property_strings(info_set.0, &info, SPDRP_DEVICEDESC)
                    .into_iter()
                    .next()
            });
        monitors.push(PnpMonitor {
            instance_id,
            friendly_name,
            manufacturer: property_strings(info_set.0, &info, SPDRP_MFG)
                .into_iter()
                .next(),
            hardware_ids: property_strings(info_set.0, &info, SPDRP_HARDWAREID),
        });
    }

    monitors.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    Ok(monitors)
}

fn enumerate_display_config_targets() -> Result<Vec<DisplayConfigTarget>, String> {
    let flags = QDC_ALL_PATHS | QDC_VIRTUAL_MODE_AWARE;
    let mut paths = Vec::new();

    for _ in 0..3 {
        let mut path_count = 0;
        let mut mode_count = 0;
        // SAFETY: Both count pointers are valid output storage.
        let size_result =
            unsafe { GetDisplayConfigBufferSizes(flags, &mut path_count, &mut mode_count) };
        if size_result != ERROR_SUCCESS {
            return Err(format!(
                "GetDisplayConfigBufferSizes failed with Windows error {}",
                size_result.0
            ));
        }

        paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        // SAFETY: Arrays are allocated to the sizes returned immediately above.
        let query_result = unsafe {
            QueryDisplayConfig(
                flags,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
        };
        if query_result == ERROR_INSUFFICIENT_BUFFER {
            continue;
        }
        if query_result != ERROR_SUCCESS {
            return Err(format!(
                "QueryDisplayConfig failed with Windows error {}",
                query_result.0
            ));
        }
        paths.truncate(path_count as usize);
        break;
    }

    let mut targets = BTreeMap::<String, DisplayConfigTarget>::new();
    for path in paths {
        let target = path.targetInfo;
        let adapter_luid = format_luid(target.adapterId);
        let id = format!("{adapter_luid}:{}", target.id);
        let name = display_config_target_name(target.adapterId, target.id);
        let active = path.flags & DISPLAYCONFIG_PATH_ACTIVE != 0;
        let available = target.targetAvailable.as_bool();

        targets
            .entry(id.clone())
            .and_modify(|existing| {
                existing.active |= active;
                existing.available |= available;
                if existing.friendly_name.is_none() {
                    existing.friendly_name = name.as_ref().and_then(|value| value.0.clone());
                }
                if existing.monitor_device_path.is_none() {
                    existing.monitor_device_path = name.as_ref().and_then(|value| value.1.clone());
                }
            })
            .or_insert_with(|| DisplayConfigTarget {
                id,
                adapter_luid,
                target_id: target.id,
                friendly_name: name.as_ref().and_then(|value| value.0.clone()),
                monitor_device_path: name.and_then(|value| value.1),
                active,
                available,
                output_technology: target.outputTechnology.0,
            });
    }

    Ok(targets.into_values().collect())
}

fn display_config_target_name(adapter: LUID, id: u32) -> Option<(Option<String>, Option<String>)> {
    let mut name = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            adapterId: adapter,
            id,
        },
        ..Default::default()
    };
    // SAFETY: name begins with the required header and its size/type fields are initialized.
    let result = unsafe { DisplayConfigGetDeviceInfo(&mut name.header) };
    if result != 0 {
        return None;
    }
    Some((
        non_empty(utf16_field(&name.monitorFriendlyDeviceName)),
        non_empty(utf16_field(&name.monitorDevicePath)),
    ))
}

fn device_instance_id(info_set: HDEVINFO, info: &SP_DEVINFO_DATA) -> Result<String, String> {
    let mut required = 0;
    // SAFETY: This size query supplies valid handles and output storage.
    let _ = unsafe { SetupDiGetDeviceInstanceIdW(info_set, info, None, Some(&mut required)) };
    if required == 0 {
        return Err("SetupDiGetDeviceInstanceIdW(monitor size) returned no ID".to_owned());
    }
    let mut buffer = vec![0_u16; required as usize];
    // SAFETY: buffer has the exact capacity reported by Windows.
    unsafe { SetupDiGetDeviceInstanceIdW(info_set, info, Some(&mut buffer), None) }
        .map_err(|error| format!("SetupDiGetDeviceInstanceIdW(monitor) failed: {error}"))?;
    Ok(utf16_field(&buffer))
}

fn property_strings(
    info_set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
    property: SETUP_DI_REGISTRY_PROPERTY,
) -> Vec<String> {
    let mut required = 0;
    // SAFETY: This is a size query for a valid device node.
    let _ = unsafe {
        SetupDiGetDeviceRegistryPropertyW(info_set, info, property, None, None, Some(&mut required))
    };
    if required < 2 {
        return Vec::new();
    }
    let mut bytes = vec![0_u8; required as usize];
    // SAFETY: bytes has the exact capacity reported by Windows.
    if unsafe {
        SetupDiGetDeviceRegistryPropertyW(info_set, info, property, None, Some(&mut bytes), None)
    }
    .is_err()
    {
        return Vec::new();
    }
    let utf16: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    utf16
        .split(|character| *character == 0)
        .filter(|part| !part.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

fn format_luid(value: LUID) -> String {
    format!("{:08X}:{:08X}", value.HighPart as u32, value.LowPart)
}

fn display_device() -> DISPLAY_DEVICEW {
    DISPLAY_DEVICEW {
        cb: size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    }
}

fn utf16_field(value: &[u16]) -> String {
    let length = value
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..length])
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}
