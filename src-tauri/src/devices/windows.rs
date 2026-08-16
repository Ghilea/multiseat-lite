use std::{fmt, mem::size_of};

use windows::{
    core::GUID,
    Win32::{
        Devices::DeviceAndDriverInstallation::{
            CM_Get_Device_IDW, CM_Get_Parent, CM_Locate_DevNodeW, SetupDiDestroyDeviceInfoList,
            SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW,
            SetupDiGetDevicePropertyW, SetupDiGetDeviceRegistryPropertyW, CM_LOCATE_DEVNODE_NORMAL,
            CR_SUCCESS, DIGCF_ALLCLASSES, DIGCF_PRESENT, GUID_DEVCLASS_KEYBOARD,
            GUID_DEVCLASS_MOUSE, HDEVINFO, MAX_DEVICE_ID_LEN, SPDRP_DEVICEDESC,
            SPDRP_ENUMERATOR_NAME, SPDRP_FRIENDLYNAME, SPDRP_HARDWAREID, SPDRP_MFG, SPDRP_SERVICE,
            SP_DEVINFO_DATA,
        },
        Devices::Properties::{DEVPKEY_Device_ContainerId, DEVPROPTYPE, DEVPROP_TYPE_GUID},
        Foundation::GetLastError,
        UI::Input::{
            GetRawInputDeviceInfoW, GetRawInputDeviceList, RAWINPUTDEVICELIST, RIDI_DEVICENAME,
            RIM_TYPEHID, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE,
        },
    },
};

use windows::Win32::Devices::DeviceAndDriverInstallation::SETUP_DI_REGISTRY_PROPERTY;

use super::{
    aggregate_physical_devices, AsterRelatedPnpDevice, InputDevice, InputDeviceType,
    InputDiscovery, RawInputDevice,
};

const API_ERROR: u32 = u32::MAX;
const ERROR_NO_MORE_ITEMS_HRESULT: u32 = 0x8007_0103;

#[derive(Debug)]
pub struct RawInputError {
    operation: &'static str,
    code: u32,
}

impl RawInputError {
    fn last(operation: &'static str) -> Self {
        // SAFETY: GetLastError has no preconditions and is read immediately after failure.
        let code = unsafe { GetLastError().0 };
        Self { operation, code }
    }
}

impl fmt::Display for RawInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} failed with Windows error {} (0x{:08X})",
            self.operation, self.code, self.code
        )
    }
}

struct DeviceInfoSet(HDEVINFO);

impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        // SAFETY: The handle was returned by SetupDiGetClassDevsW and is owned here.
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

pub fn enumerate() -> Result<InputDiscovery, RawInputError> {
    let session_raw_input_devices = raw_input_devices()?;
    let raw_paths: Vec<(String, String)> = session_raw_input_devices
        .iter()
        .filter_map(|device| {
            device
                .instance_id
                .as_ref()
                .map(|instance| (instance.clone(), device.device_path.clone()))
        })
        .collect();
    let mut devices = enumerate_device_class(
        &GUID_DEVCLASS_KEYBOARD,
        InputDeviceType::Keyboard,
        &raw_paths,
    )?;
    devices.extend(enumerate_device_class(
        &GUID_DEVCLASS_MOUSE,
        InputDeviceType::Mouse,
        &raw_paths,
    )?);

    devices.sort_by(|left, right| left.id.cmp(&right.id));
    devices.dedup_by(|left, right| left.id.eq_ignore_ascii_case(&right.id));
    let physical_devices = aggregate_physical_devices(&devices, &session_raw_input_devices);
    Ok(InputDiscovery {
        logical_pnp_devices: devices,
        physical_devices,
        session_raw_input_devices,
        aster_related_pnp_devices: enumerate_aster_pnp_devices()?,
    })
}

fn enumerate_aster_pnp_devices() -> Result<Vec<AsterRelatedPnpDevice>, RawInputError> {
    // SAFETY: No class, enumerator, or parent is supplied; flags request all present classes.
    let info_set = unsafe {
        SetupDiGetClassDevsW(None, None, None, DIGCF_ALLCLASSES | DIGCF_PRESENT)
            .map(DeviceInfoSet)
            .map_err(|_| RawInputError::last("SetupDiGetClassDevsW(all classes)"))?
    };
    let mut devices = Vec::new();
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
            return Err(RawInputError::last("SetupDiEnumDeviceInfo(all classes)"));
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
        let service = property_strings(info_set.0, &info, SPDRP_SERVICE)
            .into_iter()
            .next();
        if !is_aster_related(&instance_id)
            && !friendly_name.as_deref().is_some_and(is_aster_related)
            && !service.as_deref().is_some_and(is_aster_related)
        {
            continue;
        }

        devices.push(AsterRelatedPnpDevice {
            instance_id,
            friendly_name,
            service,
            class_guid: format!("{:?}", info.ClassGuid),
        });
    }

    devices.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    Ok(devices)
}

fn enumerate_device_class(
    class_guid: &GUID,
    device_type: InputDeviceType,
    raw_paths: &[(String, String)],
) -> Result<Vec<InputDevice>, RawInputError> {
    // SAFETY: class_guid is valid, and no enumerator or parent window is supplied.
    let info_set = unsafe {
        SetupDiGetClassDevsW(Some(class_guid), None, None, DIGCF_PRESENT)
            .map(DeviceInfoSet)
            .map_err(|_| RawInputError::last("SetupDiGetClassDevsW"))?
    };
    let mut devices = Vec::new();
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
            return Err(RawInputError::last("SetupDiEnumDeviceInfo"));
        }
        index += 1;

        let instance_id = device_instance_id(info_set.0, &info)?;
        let container_id = device_container_id(info_set.0, &info);
        let enumerator = property_strings(info_set.0, &info, SPDRP_ENUMERATOR_NAME)
            .into_iter()
            .next();

        // ROOT and SWD class nodes represent software/proxy devices, not physical hardware.
        if is_software_device(&instance_id, enumerator.as_deref()) || is_aster_related(&instance_id)
        {
            continue;
        }

        let hardware_ids = property_strings(info_set.0, &info, SPDRP_HARDWAREID);
        let friendly_name = property_strings(info_set.0, &info, SPDRP_FRIENDLYNAME)
            .into_iter()
            .next()
            .or_else(|| {
                property_strings(info_set.0, &info, SPDRP_DEVICEDESC)
                    .into_iter()
                    .next()
            });
        let manufacturer = property_strings(info_set.0, &info, SPDRP_MFG)
            .into_iter()
            .next();
        let raw_path = raw_paths
            .iter()
            .find(|(raw_instance, _)| raw_instance.eq_ignore_ascii_case(&instance_id))
            .map(|(_, path)| path.clone());
        let identifier_source = hardware_ids
            .first()
            .map(String::as_str)
            .unwrap_or(&instance_id);
        let (vendor_id, product_id) = parse_vid_pid(identifier_source);
        let usb_parent_instance_id = usb_parent_instance_id(&instance_id);
        let serial_number = usb_parent_instance_id
            .as_deref()
            .and_then(usb_serial_number);

        devices.push(InputDevice {
            id: instance_id.clone(),
            device_path: raw_path,
            instance_id,
            container_id,
            friendly_name,
            manufacturer,
            hardware_ids,
            enumerator,
            device_type,
            vendor_id,
            product_id,
            usb_parent_instance_id,
            serial_number,
        });
    }

    Ok(devices)
}

fn usb_parent_instance_id(instance_id: &str) -> Option<String> {
    let wide_id: Vec<u16> = instance_id.encode_utf16().chain(Some(0)).collect();
    let mut current = 0;
    // SAFETY: wide_id is nul-terminated and current is valid output storage.
    if unsafe {
        CM_Locate_DevNodeW(
            &mut current,
            windows::core::PCWSTR(wide_id.as_ptr()),
            CM_LOCATE_DEVNODE_NORMAL,
        )
    } != CR_SUCCESS
    {
        return None;
    }

    let mut interface_fallback = None;
    for _ in 0..12 {
        let mut parent = 0;
        // SAFETY: current is a devnode returned by Configuration Manager.
        if unsafe { CM_Get_Parent(&mut parent, current, 0) } != CR_SUCCESS {
            break;
        }
        let mut buffer = vec![0_u16; MAX_DEVICE_ID_LEN as usize];
        // SAFETY: parent is valid and buffer is writable with the documented maximum size.
        if unsafe { CM_Get_Device_IDW(parent, &mut buffer, 0) } != CR_SUCCESS {
            break;
        }
        let parent_id = utf16_string(&buffer);
        let uppercase = parent_id.to_ascii_uppercase();
        if uppercase.starts_with(r"USB\VID_") {
            if !uppercase
                .split('\\')
                .nth(1)
                .is_some_and(|hardware| hardware.contains("&MI_"))
            {
                return Some(parent_id);
            }
            interface_fallback.get_or_insert(parent_id);
        }
        current = parent;
    }
    interface_fallback
}

fn usb_serial_number(usb_instance_id: &str) -> Option<String> {
    let serial = usb_instance_id.rsplit('\\').next()?.trim();
    (!serial.is_empty() && !serial.contains('&')).then(|| serial.to_owned())
}

fn device_container_id(info_set: HDEVINFO, info: &SP_DEVINFO_DATA) -> Option<String> {
    let mut property_type = DEVPROPTYPE::default();
    let mut required = 0;
    // SAFETY: This size query uses a documented property key and valid device node.
    let _ = unsafe {
        SetupDiGetDevicePropertyW(
            info_set,
            info,
            &DEVPKEY_Device_ContainerId,
            &mut property_type,
            None,
            Some(&mut required),
            0,
        )
    };
    if required < size_of::<GUID>() as u32 {
        return None;
    }

    let mut bytes = vec![0_u8; required as usize];
    // SAFETY: bytes has the capacity reported by Windows and all pointers are valid.
    if unsafe {
        SetupDiGetDevicePropertyW(
            info_set,
            info,
            &DEVPKEY_Device_ContainerId,
            &mut property_type,
            Some(&mut bytes),
            None,
            0,
        )
    }
    .is_err()
        || property_type != DEVPROP_TYPE_GUID
    {
        return None;
    }

    // SAFETY: Windows returned DEVPROP_TYPE_GUID and at least size_of::<GUID>() bytes.
    let guid = unsafe { bytes.as_ptr().cast::<GUID>().read_unaligned() };
    Some(format!("{guid:?}"))
}

fn device_instance_id(info_set: HDEVINFO, info: &SP_DEVINFO_DATA) -> Result<String, RawInputError> {
    let mut required = 0;
    // SAFETY: This query supplies valid handles and obtains the required UTF-16 length.
    let _ = unsafe { SetupDiGetDeviceInstanceIdW(info_set, info, None, Some(&mut required)) };
    if required == 0 {
        return Err(RawInputError::last("SetupDiGetDeviceInstanceIdW(size)"));
    }

    let mut buffer = vec![0_u16; required as usize];
    // SAFETY: buffer has the exact capacity reported by the size query.
    unsafe {
        SetupDiGetDeviceInstanceIdW(info_set, info, Some(&mut buffer), None)
            .map_err(|_| RawInputError::last("SetupDiGetDeviceInstanceIdW(data)"))?
    };
    Ok(utf16_string(&buffer))
}

fn property_strings(
    info_set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
    property: SETUP_DI_REGISTRY_PROPERTY,
) -> Vec<String> {
    let mut required = 0;
    // SAFETY: This size query uses a valid device information set and node.
    let _ = unsafe {
        SetupDiGetDeviceRegistryPropertyW(info_set, info, property, None, None, Some(&mut required))
    };
    if required < 2 {
        return Vec::new();
    }

    let mut bytes = vec![0_u8; required as usize];
    // SAFETY: bytes has the exact capacity reported by the property size query.
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

fn raw_input_devices() -> Result<Vec<RawInputDevice>, RawInputError> {
    let entries = raw_input_device_list()?;
    let mut devices = Vec::new();

    for entry in entries {
        let device_type = if entry.dwType == RIM_TYPEKEYBOARD {
            InputDeviceType::Keyboard
        } else if entry.dwType == RIM_TYPEMOUSE {
            InputDeviceType::Mouse
        } else if entry.dwType == RIM_TYPEHID {
            InputDeviceType::Hid
        } else {
            continue;
        };
        let path = raw_input_device_path(entry.hDevice)?;
        devices.push(RawInputDevice {
            id: path.clone(),
            instance_id: instance_id_from_path(&path),
            aster_related: is_aster_related(&path),
            device_path: path,
            device_type,
        });
    }
    devices.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(devices)
}

fn is_software_device(instance_id: &str, enumerator: Option<&str>) -> bool {
    instance_id.to_ascii_uppercase().starts_with("ROOT\\")
        || matches!(enumerator, Some(value) if value.eq_ignore_ascii_case("ROOT") || value.eq_ignore_ascii_case("SWD"))
}

fn is_aster_related(value: &str) -> bool {
    let uppercase = value.to_ascii_uppercase();
    ["ASTER", "MUTENX", "MUTESV", "IBIK"]
        .iter()
        .any(|marker| uppercase.contains(marker))
}

fn utf16_string(value: &[u16]) -> String {
    let length = value
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..length])
}

/* Raw Input correlation -------------------------------------------------- */

/*
 * Raw Input paths are useful because later input routing receives the same
 * handles. SetupAPI provides the system-wide present PnP view, while Raw Input
 * provides the current session/workplace view. ASTER or other interception
 * software may alter the latter.
 */

fn raw_input_device_list() -> Result<Vec<RAWINPUTDEVICELIST>, RawInputError> {
    let entry_size = size_of::<RAWINPUTDEVICELIST>() as u32;
    let mut count = 0;

    // SAFETY: The first call intentionally supplies no output buffer and a valid count pointer.
    let result = unsafe { GetRawInputDeviceList(None, &mut count, entry_size) };
    if result == API_ERROR {
        return Err(RawInputError::last("GetRawInputDeviceList(size)"));
    }

    let mut entries = vec![RAWINPUTDEVICELIST::default(); count as usize];
    if entries.is_empty() {
        return Ok(entries);
    }

    // SAFETY: entries contains `count` initialized slots of the size reported to Windows.
    let result =
        unsafe { GetRawInputDeviceList(Some(entries.as_mut_ptr()), &mut count, entry_size) };
    if result == API_ERROR {
        return Err(RawInputError::last("GetRawInputDeviceList(data)"));
    }

    entries.truncate(result as usize);
    Ok(entries)
}

pub(crate) fn raw_input_device_path(
    handle: windows::Win32::Foundation::HANDLE,
) -> Result<String, RawInputError> {
    let mut character_count = 0;

    // SAFETY: This size query uses a valid device handle from GetRawInputDeviceList.
    let result = unsafe {
        GetRawInputDeviceInfoW(Some(handle), RIDI_DEVICENAME, None, &mut character_count)
    };
    if result == API_ERROR {
        return Err(RawInputError::last("GetRawInputDeviceInfoW(size)"));
    }

    let mut buffer = vec![0_u16; character_count as usize + 1];
    // SAFETY: buffer is writable and has room for the character count plus a terminator.
    let result = unsafe {
        GetRawInputDeviceInfoW(
            Some(handle),
            RIDI_DEVICENAME,
            Some(buffer.as_mut_ptr().cast()),
            &mut character_count,
        )
    };
    if result == API_ERROR {
        return Err(RawInputError::last("GetRawInputDeviceInfoW(data)"));
    }

    let length = buffer
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(result as usize);
    Ok(String::from_utf16_lossy(&buffer[..length]))
}

fn instance_id_from_path(path: &str) -> Option<String> {
    let path = path.strip_prefix(r"\\?\")?;
    let mut parts = path.split('#');
    let enumerator = parts.next()?;
    let hardware = parts.next()?;
    let instance = parts.next()?;
    Some(format!(r"{enumerator}\{hardware}\{instance}"))
}

fn parse_vid_pid(path: &str) -> (Option<String>, Option<String>) {
    let uppercase = path.to_ascii_uppercase();
    (
        hexadecimal_component(&uppercase, "VID_"),
        hexadecimal_component(&uppercase, "PID_"),
    )
}

fn hexadecimal_component(value: &str, marker: &str) -> Option<String> {
    let start = value.find(marker)? + marker.len();
    let component: String = value[start..]
        .chars()
        .take_while(|character| character.is_ascii_hexdigit())
        .collect();
    (!component.is_empty()).then_some(component)
}

#[cfg(test)]
mod tests {
    use super::{instance_id_from_path, parse_vid_pid};

    #[test]
    fn parses_stable_parts_from_a_hid_path() {
        let path = r"\\?\HID#VID_046D&PID_C547&MI_01#8&ABC&0&0000#{GUID}";
        assert_eq!(
            instance_id_from_path(path).as_deref(),
            Some(r"HID\VID_046D&PID_C547&MI_01\8&ABC&0&0000")
        );
        assert_eq!(
            parse_vid_pid(path),
            (Some("046D".into()), Some("C547".into()))
        );
    }
}
