//! Read-only managed installation evidence probe. Pass `--persist` only to
//! reconcile the non-sensitive MultiSeat Lite journal after inspection.

use std::{env, path::PathBuf};

use multiseat_lite_lib::{
    seats::{ConfigStore, SeatId},
    virtualization::VirtualBoxRuntimeService,
};

fn main() -> Result<(), String> {
    let persist = env::args().any(|argument| argument == "--persist");
    let app_data = env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "APPDATA is unavailable".to_owned())?;
    let config_path = app_data.join("se.multiseat.lite").join("config.json");
    let store = ConfigStore::initialize(config_path)?;
    let configuration = store.get()?;
    let backend = configuration
        .backends
        .virtual_box
        .seats
        .get("seat-2")
        .ok_or_else(|| "seat-2 has no VirtualBox configuration".to_owned())?;
    if !backend.managed_by_multiseat {
        return Err("seat-2 is not marked as managed by MultiSeat Lite".to_owned());
    }
    let previous = backend
        .installation
        .as_ref()
        .ok_or_else(|| "seat-2 has no managed installation journal".to_owned())?;
    let runtime = VirtualBoxRuntimeService::discover()?;
    let snapshot = runtime.inspect_managed_installation(&backend.vm_id, previous)?;

    println!("UUID: {}", snapshot.vm_id);
    println!("VM state: {}", snapshot.vm_state);
    for probe in &snapshot.installation.observed.evidence.runlevel_probes {
        println!(
            "Runlevel {:?}: exit={:?}, success={}, elapsed_ms={}, stdout_empty={}, stderr={}",
            probe.level,
            probe.exit_code,
            probe.success,
            probe.elapsed_ms,
            probe.stdout.is_empty(),
            if probe.stderr.trim().is_empty() {
                "<empty>"
            } else {
                probe.stderr.trim()
            }
        );
    }
    println!(
        "Windows installation: {:?}",
        snapshot.installation.observed.windows
    );
    println!(
        "Guest Additions communication: {:?}",
        snapshot.installation.observed.guest_additions
    );
    println!(
        "Guest Additions version: {}",
        snapshot
            .installation
            .observed
            .evidence
            .guest_additions_version
            .as_deref()
            .unwrap_or("unknown")
    );
    println!(
        "Guest Additions HostVerLastChecked: {}",
        snapshot
            .installation
            .observed
            .evidence
            .guest_add_host_version_last_checked
            .as_deref()
            .unwrap_or("unknown")
    );
    println!(
        "Strongest guest runlevel: {:?}",
        snapshot.installation.observed.run_level
    );
    println!(
        "Managed VM status: {:?}",
        snapshot.installation.observed.managed
    );
    println!("Monitoring complete: {}", snapshot.monitoring_complete);
    if let Some(event) = snapshot
        .installation
        .observed
        .evidence
        .reconciliation_event
        .as_deref()
    {
        println!("Reconciliation: {event}");
    }

    if persist && snapshot.installation != *previous {
        store.update_managed_vm_installation(SeatId("seat-2".to_owned()), snapshot.installation)?;
        println!("Journal persisted: yes");
    } else {
        println!("Journal persisted: no");
    }
    Ok(())
}
