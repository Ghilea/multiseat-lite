//! Enumeration and aggregation of Windows input devices.

use std::collections::BTreeMap;

use serde::Serialize;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub(crate) fn raw_input_device_path(
    handle: ::windows::Win32::Foundation::HANDLE,
) -> Result<String, String> {
    windows::raw_input_device_path(handle).map_err(|error| error.to_string())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDevice {
    /// Stable Plug-and-Play device instance identifier supplied by Windows.
    pub id: String,
    pub device_path: Option<String>,
    pub instance_id: String,
    /// Groups multiple HID collections that belong to one physical device.
    pub container_id: Option<String>,
    pub friendly_name: Option<String>,
    pub manufacturer: Option<String>,
    pub hardware_ids: Vec<String>,
    pub enumerator: Option<String>,
    pub device_type: InputDeviceType,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub usb_parent_instance_id: Option<String>,
    pub serial_number: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputCapabilities {
    pub keyboard: bool,
    pub mouse: bool,
}

impl InputCapabilities {
    pub fn supports(self, device_type: InputDeviceType) -> bool {
        match device_type {
            InputDeviceType::Keyboard => self.keyboard,
            InputDeviceType::Mouse => self.mouse,
            InputDeviceType::Hid => false,
        }
    }

    fn include(&mut self, device_type: InputDeviceType) {
        match device_type {
            InputDeviceType::Keyboard => self.keyboard = true,
            InputDeviceType::Mouse => self.mouse = true,
            InputDeviceType::Hid => {}
        }
    }
}

/// One assignable physical device. Container ID is the preferred stable identity.
/// If Windows supplies no Container ID, `id` falls back to the member devnode's
/// stable PnP instance ID and that node is deliberately not merged by name/model.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalInputDevice {
    pub id: String,
    pub container_id: Option<String>,
    pub friendly_name: Option<String>,
    pub manufacturer: Option<String>,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub usb_parent_instance_id: Option<String>,
    pub serial_number: Option<String>,
    pub capabilities: InputCapabilities,
    pub logical_nodes: Vec<InputDevice>,
    pub raw_input_paths: Vec<String>,
    pub present: bool,
}

impl PhysicalInputDevice {
    pub fn contains_logical_id(&self, id: &str) -> bool {
        self.logical_nodes.iter().any(|node| {
            node.id.eq_ignore_ascii_case(id) || node.instance_id.eq_ignore_ascii_case(id)
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InputDeviceType {
    Keyboard,
    Mouse,
    Hid,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawInputDevice {
    pub id: String,
    pub device_path: String,
    pub instance_id: Option<String>,
    pub device_type: InputDeviceType,
    pub aster_related: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsterRelatedPnpDevice {
    pub instance_id: String,
    pub friendly_name: Option<String>,
    pub service: Option<String>,
    pub class_guid: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDiscovery {
    pub logical_pnp_devices: Vec<InputDevice>,
    pub physical_devices: Vec<PhysicalInputDevice>,
    pub session_raw_input_devices: Vec<RawInputDevice>,
    pub aster_related_pnp_devices: Vec<AsterRelatedPnpDevice>,
}

impl InputDiscovery {
    pub fn aster_device_evidence(&self) -> crate::environment::AsterDeviceEvidence {
        let mut_enx_device_node_present = self.aster_related_pnp_devices.iter().any(|device| {
            contains_mut_enx(&device.instance_id)
                || device.service.as_deref().is_some_and(contains_mut_enx)
        });
        let mut_enx_raw_input_visible = self.session_raw_input_devices.iter().any(|device| {
            device.aster_related
                && (contains_mut_enx(&device.id) || contains_mut_enx(&device.device_path))
        });
        crate::environment::AsterDeviceEvidence {
            mut_enx_device_node_present,
            mut_enx_raw_input_visible,
            other_aster_related_device_present: !self.aster_related_pnp_devices.is_empty()
                || self
                    .session_raw_input_devices
                    .iter()
                    .any(|device| device.aster_related),
        }
    }
}

fn contains_mut_enx(value: &str) -> bool {
    value.to_ascii_uppercase().contains("MUTENX")
}

pub fn aggregate_physical_devices(
    logical_nodes: &[InputDevice],
    raw_input_devices: &[RawInputDevice],
) -> Vec<PhysicalInputDevice> {
    let mut groups: BTreeMap<String, Vec<InputDevice>> = BTreeMap::new();
    for node in logical_nodes {
        let stable_id = node.container_id.as_deref().unwrap_or(&node.instance_id);
        groups
            .entry(stable_id.to_ascii_lowercase())
            .or_default()
            .push(node.clone());
    }

    groups
        .into_values()
        .map(|mut nodes| {
            nodes.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
            let container_id = nodes.iter().find_map(|node| node.container_id.clone());
            let id = container_id
                .clone()
                .unwrap_or_else(|| nodes[0].instance_id.clone());
            let mut capabilities = InputCapabilities::default();
            for node in &nodes {
                capabilities.include(node.device_type);
            }
            let mut raw_input_paths: Vec<String> = raw_input_devices
                .iter()
                .filter(|raw| {
                    raw.instance_id.as_deref().is_some_and(|raw_id| {
                        nodes
                            .iter()
                            .any(|node| node.instance_id.eq_ignore_ascii_case(raw_id))
                    })
                })
                .map(|raw| raw.device_path.clone())
                .chain(nodes.iter().filter_map(|node| node.device_path.clone()))
                .collect();
            raw_input_paths.sort_by_key(|path| path.to_ascii_lowercase());
            raw_input_paths.dedup_by(|left, right| left.eq_ignore_ascii_case(right));

            PhysicalInputDevice {
                id,
                container_id,
                friendly_name: nodes.iter().find_map(|node| node.friendly_name.clone()),
                manufacturer: nodes.iter().find_map(|node| node.manufacturer.clone()),
                vendor_id: nodes.iter().find_map(|node| node.vendor_id.clone()),
                product_id: nodes.iter().find_map(|node| node.product_id.clone()),
                usb_parent_instance_id: nodes
                    .iter()
                    .find_map(|node| node.usb_parent_instance_id.clone()),
                serial_number: nodes.iter().find_map(|node| node.serial_number.clone()),
                capabilities,
                logical_nodes: nodes,
                raw_input_paths,
                present: true,
            }
        })
        .collect()
}

#[cfg(target_os = "windows")]
pub fn enumerate() -> Result<InputDiscovery, String> {
    windows::enumerate().map_err(|error| error.to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn enumerate() -> Result<InputDiscovery, String> {
    Err("input device discovery is only implemented for Windows".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(
        instance: &str,
        container: &str,
        name: &str,
        device_type: InputDeviceType,
    ) -> InputDevice {
        InputDevice {
            id: instance.into(),
            device_path: Some(format!(r"\\?\{instance}")),
            instance_id: instance.into(),
            container_id: Some(container.into()),
            friendly_name: Some(name.into()),
            manufacturer: Some("Test manufacturer".into()),
            hardware_ids: vec![r"HID\VID_1234&PID_5678".into()],
            enumerator: Some("HID".into()),
            device_type,
            vendor_id: Some("1234".into()),
            product_id: Some("5678".into()),
            usb_parent_instance_id: None,
            serial_number: None,
        }
    }

    #[test]
    fn four_keyboard_nodes_in_one_container_become_one_physical_device() {
        let nodes: Vec<_> = (0..4)
            .map(|index| {
                node(
                    &format!(r"HID\KEYBOARD\{index}"),
                    "container-a",
                    "Keyboard",
                    InputDeviceType::Keyboard,
                )
            })
            .collect();
        let physical = aggregate_physical_devices(&nodes, &[]);
        assert_eq!(physical.len(), 1);
        assert_eq!(physical[0].logical_nodes.len(), 4);
        assert!(physical[0].capabilities.keyboard);
    }

    #[test]
    fn two_mouse_nodes_in_one_container_become_one_physical_device() {
        let nodes = vec![
            node(
                r"HID\MOUSE\0",
                "container-a",
                "Mouse",
                InputDeviceType::Mouse,
            ),
            node(
                r"HID\MOUSE\1",
                "container-a",
                "Mouse",
                InputDeviceType::Mouse,
            ),
        ];
        let physical = aggregate_physical_devices(&nodes, &[]);
        assert_eq!(physical.len(), 1);
        assert_eq!(physical[0].logical_nodes.len(), 2);
        assert!(physical[0].capabilities.mouse);
    }

    #[test]
    fn identical_names_in_different_containers_remain_separate() {
        let nodes = vec![
            node(
                r"HID\KEYBOARD\0",
                "container-a",
                "Same name",
                InputDeviceType::Keyboard,
            ),
            node(
                r"HID\KEYBOARD\1",
                "container-b",
                "Same name",
                InputDeviceType::Keyboard,
            ),
        ];
        let physical = aggregate_physical_devices(&nodes, &[]);
        assert_eq!(physical.len(), 2);
    }

    #[test]
    fn one_container_can_have_keyboard_and_mouse_capabilities() {
        let nodes = vec![
            node(
                r"HID\KEYBOARD\0",
                "container-a",
                "Composite",
                InputDeviceType::Keyboard,
            ),
            node(
                r"HID\MOUSE\0",
                "container-a",
                "Composite",
                InputDeviceType::Mouse,
            ),
        ];
        let physical = aggregate_physical_devices(&nodes, &[]);
        assert_eq!(physical.len(), 1);
        assert_eq!(
            physical[0].capabilities,
            InputCapabilities {
                keyboard: true,
                mouse: true
            }
        );
    }
}
