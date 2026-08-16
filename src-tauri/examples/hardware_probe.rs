use multiseat_lite_lib::enumerate_hardware;

fn main() {
    match enumerate_hardware() {
        Ok(snapshot) => {
            println!(
                "Current Windows session ID: {}",
                snapshot.environment.current_session_id
            );
            print_aster_status(&snapshot.environment.aster);

            println!(
                "Displays visible to current session: {}",
                snapshot.displays.len()
            );
            for display in snapshot.displays {
                println!("  [session display] {display:#?}");
            }
            println!("Connected PnP monitors: {}", snapshot.pnp_monitors.len());
            for monitor in snapshot.pnp_monitors {
                println!("  [PnP monitor] {monitor:#?}");
            }
            let active_targets = snapshot
                .display_config_targets
                .iter()
                .filter(|target| target.active)
                .count();
            let inactive_available_targets = snapshot
                .display_config_targets
                .iter()
                .filter(|target| !target.active && target.available)
                .count();
            let inactive_unavailable_targets = snapshot
                .display_config_targets
                .len()
                .saturating_sub(active_targets + inactive_available_targets);
            println!(
                "DisplayConfig targets: {} active, {} inactive/available, {} inactive/unavailable",
                active_targets, inactive_available_targets, inactive_unavailable_targets
            );
            for target in snapshot.display_config_targets {
                println!("  [DisplayConfig target] {target:#?}");
            }

            println!(
                "Logical PnP keyboard/mouse nodes: {}",
                snapshot.logical_input_devices.len()
            );
            for device in &snapshot.logical_input_devices {
                println!("  [logical PnP input node] {device:#?}");
            }
            println!("Physical input devices: {}", snapshot.input_devices.len());
            println!(
                "Keyboard-capable physical devices: {}",
                snapshot
                    .input_devices
                    .iter()
                    .filter(|device| device.capabilities.keyboard)
                    .count()
            );
            println!(
                "Mouse-capable physical devices: {}",
                snapshot
                    .input_devices
                    .iter()
                    .filter(|device| device.capabilities.mouse)
                    .count()
            );
            for device in &snapshot.input_devices {
                println!("  [physical input aggregate] {device:#?}");
            }

            println!(
                "Raw Input devices visible to this session/workplace: {}",
                snapshot.session_raw_input_devices.len()
            );
            for device in snapshot.session_raw_input_devices {
                println!("  [session Raw Input] {device:#?}");
            }

            println!(
                "ASTER-related PnP devices (kept separate): {}",
                snapshot.aster_related_pnp_devices.len()
            );
            for device in snapshot.aster_related_pnp_devices {
                println!("  [ASTER-related PnP] {device:#?}");
            }
        }
        Err(error) => {
            eprintln!("Hardware enumeration failed: {error}");
            std::process::exit(1);
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn print_aster_status(status: &multiseat_lite_lib::environment::AsterEnvironmentStatus) {
    println!(
        "ASTER installation detected: {}",
        yes_no(status.installation_detected)
    );
    println!(
        "ASTER-related service installed: {}",
        yes_no(status.related_service_installed)
    );
    println!(
        "ASTER-related service running: {}",
        yes_no(status.related_service_running)
    );
    if let Some(service) = status
        .related_services
        .iter()
        .find(|service| service.name.eq_ignore_ascii_case("MUTENX_SERVICE"))
    {
        println!("MutEnx service installed: {}", yes_no(service.installed));
        println!("MutEnx service running: {}", yes_no(service.running));
        println!("MutEnx service state: {:?}", service.state);
    }
    println!(
        "MutEnx device node present: {}",
        yes_no(status.mut_enx_device_node_present)
    );
    println!(
        "MutEnx Raw Input proxy visible in this session: {}",
        yes_no(status.mut_enx_raw_input_visible)
    );
    println!(
        "ASTER-related registry markers present: {}",
        yes_no(status.registry_markers_present)
    );
    println!("ASTER workplace state: {:?}", status.workplace_state);
    for marker in &status.registry_markers {
        println!("  ASTER registry marker: {marker}");
    }
    for service in &status.related_services {
        println!("  ASTER-related service: {service:#?}");
    }
}
