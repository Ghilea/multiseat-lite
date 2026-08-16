use super::*;
use crate::devices::{InputCapabilities, InputDevice, InputDeviceType};

const USBHOST: &str = r#"Host USB Devices:

UUID:               11111111-1111-1111-1111-111111111111
VendorId:           0x046d (046D)
ProductId:          0xc31c (C31C)
Revision:           1.0 (0100)
Manufacturer:       Logitech
Product:            Keyboard
SerialNumber:       SERIAL-A
Address:            {A}
Current State:      Available

UUID:               22222222-2222-2222-2222-222222222222
VendorId:           0x046d (046D)
ProductId:          0xc31c (C31C)
SerialNumber:       SERIAL-B
Current State:      Busy
"#;

const VIRTUALBOX_7_2_14_USBHOST: &str = r#"Host USB Devices:

UUID:               6d0b8198-c25c-4271-8e03-b4d41c8c28d9
VendorId:           0xb58e (B58E)
ProductId:          0x9e84 (9E84)
Revision:           1.0 (0100)
Port:               4
USB version/speed:  1/Full
Manufacturer:       Blue Microphones
Product:            Yeti Stereo Microphone
SerialNumber:       REV8
Address:            {36fc9e60-c465-11cf-8056-444553540000}\0010
Current State:      Busy
Future Field:       ignored safely

UUID:               a7466cc4-e3fc-42ed-88bb-3dd4e6f24b2d
VendorId:           0x1532 (1532)
ProductId:          0x021f (021F)
Product:            Razer Ornata
Current State:      Busy

UUID:
VendorId:           definitely-not-hex
Malformed Section:  must not discard valid neighbors

UUID:               43a02d99-5246-49b8-925f-5df3c8604ff1
VendorId:           0x30fa (30FA)
ProductId:          0x1440 (1440)
Current State:      Available
"#;

fn physical(serial: Option<&str>, composite: bool) -> PhysicalInputDevice {
    let mut logical_nodes = vec![logical(InputDeviceType::Keyboard)];
    if composite {
        logical_nodes.push(logical(InputDeviceType::Mouse));
    }
    PhysicalInputDevice {
        id: "container-a".into(),
        container_id: Some("container-a".into()),
        friendly_name: Some("Keyboard".into()),
        manufacturer: Some("Logitech".into()),
        vendor_id: Some("046D".into()),
        product_id: Some("C31C".into()),
        usb_parent_instance_id: Some(format!(
            r"USB\VID_046D&PID_C31C\{}",
            serial.unwrap_or("6&generated")
        )),
        serial_number: serial.map(str::to_owned),
        capabilities: InputCapabilities {
            keyboard: true,
            mouse: composite,
        },
        logical_nodes,
        raw_input_paths: Vec::new(),
        present: true,
    }
}

fn logical(device_type: InputDeviceType) -> InputDevice {
    InputDevice {
        id: format!(r"HID\NODE\{device_type:?}"),
        device_path: None,
        instance_id: format!(r"HID\NODE\{device_type:?}"),
        container_id: Some("container-a".into()),
        friendly_name: Some("Composite input".into()),
        manufacturer: None,
        hardware_ids: Vec::new(),
        enumerator: Some("HID".into()),
        device_type,
        vendor_id: Some("046D".into()),
        product_id: Some("C31C".into()),
        usb_parent_instance_id: None,
        serial_number: None,
    }
}

fn virtual_box(devices: Vec<VirtualBoxUsbDevice>) -> VirtualBoxStatus {
    VirtualBoxStatus {
        installed: true,
        vbox_manage_path: Some("VBoxManage.exe".into()),
        version: Some("7.1.0".into()),
        operational: true,
        probe_error: None,
        parser_warnings: Vec::new(),
        usb_devices: devices,
    }
}

#[test]
fn probe_only_backend_never_performs_activation() {
    let capabilities = BackendCapabilities {
        backend_kind: BackendKind::VirtualBox,
        available: true,
        status_probe: true,
        usb_discovery: true,
        activation_supported: false,
    };
    let backend = ProbeOnlyBackend::new(capabilities.clone());
    assert_eq!(backend.backend_kind(), BackendKind::VirtualBox);
    assert_eq!(backend.capabilities().unwrap(), capabilities);
    assert!(!backend.capabilities().unwrap().activation_supported);
    let seat = ResolvedSeat {
        id: SeatId("seat-2".into()),
        name: "Barnen".into(),
        devices: crate::seats::ResolvedSeatDeviceAssignments::default(),
    };
    assert!(backend.start(&seat).unwrap_err().contains("not supported"));
    assert!(backend
        .stop(&seat.id)
        .unwrap_err()
        .contains("not supported"));
}

#[test]
fn parses_virtualbox_usbhost_output() {
    let parsed = parse_virtualbox_usbhost(USBHOST);
    assert!(parsed.warnings.is_empty());
    let devices = parsed.devices;
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].vendor_id.as_deref(), Some("046D"));
    assert_eq!(devices[0].product_id.as_deref(), Some("C31C"));
    assert_eq!(devices[0].serial_number.as_deref(), Some("SERIAL-A"));
    assert_eq!(devices[1].current_state.as_deref(), Some("Busy"));
}

#[test]
fn parses_virtualbox_7_2_14_heading_optional_fields_and_malformed_record() {
    let parsed = parse_virtualbox_usbhost(VIRTUALBOX_7_2_14_USBHOST);
    assert_eq!(parsed.devices.len(), 3);
    assert_eq!(parsed.warnings.len(), 1);
    assert!(parsed.warnings[0].contains("empty UUID"));

    let yeti = &parsed.devices[0];
    assert_eq!(yeti.uuid, "6d0b8198-c25c-4271-8e03-b4d41c8c28d9");
    assert_eq!(yeti.vendor_id.as_deref(), Some("B58E"));
    assert_eq!(yeti.product_id.as_deref(), Some("9E84"));
    assert_eq!(yeti.revision.as_deref(), Some("1.0 (0100)"));
    assert_eq!(yeti.port.as_deref(), Some("4"));
    assert_eq!(yeti.usb_version_speed.as_deref(), Some("1/Full"));
    assert_eq!(yeti.manufacturer.as_deref(), Some("Blue Microphones"));
    assert_eq!(yeti.product.as_deref(), Some("Yeti Stereo Microphone"));
    assert_eq!(yeti.serial_number.as_deref(), Some("REV8"));
    assert_eq!(yeti.current_state.as_deref(), Some("Busy"));

    let keyboard = &parsed.devices[1];
    assert_eq!(keyboard.vendor_id.as_deref(), Some("1532"));
    assert_eq!(keyboard.product_id.as_deref(), Some("021F"));
    assert!(keyboard.serial_number.is_none());

    let mouse = &parsed.devices[2];
    assert_eq!(mouse.vendor_id.as_deref(), Some("30FA"));
    assert_eq!(mouse.product_id.as_deref(), Some("1440"));
    assert!(mouse.product.is_none());
    assert!(mouse.serial_number.is_none());
    assert_eq!(mouse.current_state.as_deref(), Some("Available"));
}

#[test]
fn exact_usb_correlation_uses_serial_number() {
    let devices = parse_virtualbox_usbhost(USBHOST).devices;
    let mappings =
        correlate_usb_devices(&[physical(Some("SERIAL-A"), false)], &virtual_box(devices));
    assert_eq!(mappings[0].state, UsbCorrelationState::Exact);
    assert_eq!(
        mappings[0].matched_uuid.as_deref(),
        Some("11111111-1111-1111-1111-111111111111")
    );
}

#[test]
fn same_vid_pid_with_different_serials_maps_correct_devices() {
    let devices = parse_virtualbox_usbhost(USBHOST).devices;
    let mappings = correlate_usb_devices(
        &[
            physical(Some("SERIAL-A"), false),
            physical(Some("SERIAL-B"), false),
        ],
        &virtual_box(devices),
    );
    assert_eq!(mappings[0].state, UsbCorrelationState::Exact);
    assert_eq!(mappings[1].state, UsbCorrelationState::Exact);
    assert_ne!(mappings[0].matched_uuid, mappings[1].matched_uuid);
}

#[test]
fn same_vid_pid_without_distinguishing_information_is_ambiguous() {
    let devices = parse_virtualbox_usbhost(USBHOST).devices;
    let mappings = correlate_usb_devices(&[physical(None, false)], &virtual_box(devices));
    assert_eq!(mappings[0].state, UsbCorrelationState::Ambiguous);
    assert_eq!(mappings[0].candidate_uuids.len(), 2);
    assert!(mappings[0].matched_uuid.is_none());
}

#[test]
fn composite_hid_container_produces_one_usb_correlation() {
    let devices = parse_virtualbox_usbhost(USBHOST).devices;
    let mappings =
        correlate_usb_devices(&[physical(Some("SERIAL-A"), true)], &virtual_box(devices));
    assert_eq!(mappings.len(), 1);
    assert_eq!(mappings[0].state, UsbCorrelationState::Exact);
}

#[test]
fn backend_not_installed_makes_mapping_unavailable() {
    let unavailable = VirtualBoxStatus {
        installed: false,
        vbox_manage_path: None,
        version: None,
        operational: false,
        probe_error: None,
        parser_warnings: Vec::new(),
        usb_devices: Vec::new(),
    };
    let mappings = correlate_usb_devices(&[physical(Some("SERIAL-A"), false)], &unavailable);
    assert_eq!(mappings[0].state, UsbCorrelationState::Unavailable);
}
