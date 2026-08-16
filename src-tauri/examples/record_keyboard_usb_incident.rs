//! Records the manually observed VirtualBox USB keyboard-capture incident.
//!
//! Read-only unless `--record-known-incident` is supplied. This command never
//! invokes VirtualBox and never attaches, detaches, starts, or stops a VM.

use std::{env, path::PathBuf};

use multiseat_lite_lib::{
    devices,
    seats::{ConfigStore, DeviceBackendSafetyRecord, DeviceRoutingBackend, UsbPassthroughSafety},
};

fn main() -> Result<(), String> {
    let config_path =
        PathBuf::from(env::var_os("APPDATA").ok_or_else(|| "APPDATA is unavailable".to_owned())?)
            .join("se.multiseat.lite")
            .join("config.json");
    let store = ConfigStore::initialize(config_path)?;
    let configuration = store.get()?;
    let barnen = configuration
        .seats
        .iter()
        .find(|seat| seat.id.0 == "seat-2")
        .ok_or_else(|| "seat-2 is missing".to_owned())?;
    let keyboard_id = barnen
        .devices
        .keyboard
        .clone()
        .ok_or_else(|| "Barnen has no configured keyboard".to_owned())?;
    let discovery = devices::enumerate()?;
    let keyboard = discovery
        .physical_devices
        .iter()
        .find(|device| device.id.eq_ignore_ascii_case(&keyboard_id.0));

    if keyboard.is_some_and(|device| {
        device.vendor_id.as_deref() != Some("1A2C") || device.product_id.as_deref() != Some("4C5E")
    }) {
        return Err(format!(
            "refusing to record the incident for unexpected VID/PID {:?}:{:?}",
            keyboard.and_then(|device| device.vendor_id.as_deref()),
            keyboard.and_then(|device| device.product_id.as_deref())
        ));
    }

    println!("Physical keyboard ID: {}", keyboard_id.0);
    println!("VID/PID: 1A2C:4C5E");
    println!(
        "Present in current Windows enumeration: {}",
        if keyboard.is_some() { "yes" } else { "no" }
    );
    if !env::args().any(|argument| argument == "--record-known-incident") {
        println!("Read-only. Supply --record-known-incident to persist the safety record.");
        return Ok(());
    }

    store.record_device_safety(DeviceBackendSafetyRecord {
        physical_device_id: keyboard_id,
        vendor_id: Some("1A2C".to_owned()),
        product_id: Some("4C5E".to_owned()),
        backend: DeviceRoutingBackend::VirtualBoxUsbPassthrough,
        safety: UsbPassthroughSafety::KnownProblematic,
        reason: "Windows host crashed while VirtualBox captured this physical keyboard; automatic retry is disabled".to_owned(),
        observed: Some(
            "PNP_DETECTED_FATAL_ERROR 0xCA, Arg1 0x2, VBoxUSBMon.sys/USB stack evidence"
                .to_owned(),
        ),
    })?;
    println!("Recorded: KnownProblematic for VirtualBoxUsbPassthrough");
    Ok(())
}
