use std::{env, path::PathBuf};

use multiseat_lite_lib::{
    enumerate_hardware,
    seats::{ConfigStore, SeatId},
    virtualization::VirtualBoxRuntimeService,
};

fn main() -> Result<(), String> {
    let app_data = env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "APPDATA is unavailable".to_owned())?;
    let store = ConfigStore::initialize(app_data.join("se.multiseat.lite").join("config.json"))?;
    let runtime = VirtualBoxRuntimeService::discover()?;
    let seat_id = SeatId("seat-2".to_owned());
    let mut configuration = store.get()?;

    if env::args().any(|argument| argument == "--apply-mouse-policy") {
        if runtime.ensure_managed_mouse_capture_disabled(&seat_id, &configuration)? {
            store.set_mouse_capture_policy_owned(seat_id.clone(), true)?;
            configuration = store.get()?;
            println!("Managed mouse-capture policy applied and ownership recorded: yes");
        } else {
            return Err("seat-2 is not a MultiSeat Lite managed VM; no setting was changed".into());
        }
    }

    let hardware = enumerate_hardware()?;
    let snapshot = runtime.status(&seat_id, &configuration, &hardware.input_devices)?;
    println!("VM UUID: {}", snapshot.vm_id.as_deref().unwrap_or("none"));
    println!(
        "Mouse capture policy: {}",
        snapshot
            .input_isolation
            .mouse_capture_policy
            .as_deref()
            .unwrap_or("unknown")
    );
    println!(
        "Mouse capture disabled: {}",
        snapshot.input_isolation.mouse_capture_disabled
    );
    println!(
        "Policy owned by MultiSeat Lite: {}",
        snapshot.input_isolation.mouse_capture_policy_owned
    );
    println!(
        "Mouse Integration requested: {:?}",
        snapshot.input_isolation.mouse_integration.requested
    );
    println!(
        "Mouse Integration observed: {:?}",
        snapshot.input_isolation.mouse_integration.observed
    );
    println!(
        "Mouse Integration control: {:?}",
        snapshot.input_isolation.mouse_integration.control
    );
    println!(
        "Mouse isolation: {:?}",
        snapshot.input_isolation.mouse_isolation
    );
    println!(
        "Presentation state: {:?}",
        snapshot.display_presentation.state
    );
    println!(
        "Presentation mode: {:?}",
        snapshot.display_presentation.mode
    );
    println!(
        "Assigned display: {}",
        snapshot
            .display_presentation
            .assigned_display_id
            .as_deref()
            .unwrap_or("none")
    );
    println!(
        "Presentation bounds: {:?}",
        snapshot.display_presentation.bounds
    );
    if let Some(error) = snapshot.display_presentation.last_error {
        println!("Presentation note: {error}");
    }
    if let Some(message) = snapshot.input_isolation.mouse_integration.message {
        println!("Mouse Integration note: {message}");
    }
    if let Some(error) = snapshot.input_isolation.mouse_capture_policy_error {
        println!("Policy probe error: {error}");
    }
    Ok(())
}
