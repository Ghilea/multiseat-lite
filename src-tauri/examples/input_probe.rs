use multiseat_lite_lib::{devices, environment};

fn main() {
    match devices::enumerate() {
        Ok(discovery) => {
            match environment::detect(discovery.aster_device_evidence()) {
                Ok(context) => {
                    println!("Current Windows session ID: {}", context.current_session_id);
                    print_aster_status(&context.aster);
                }
                Err(error) => {
                    eprintln!("Environment detection failed: {error}");
                    std::process::exit(1);
                }
            }
            println!(
                "Raw Input devices visible to this session/workplace: {}",
                discovery.session_raw_input_devices.len()
            );
            for device in discovery.session_raw_input_devices {
                println!("  {device:#?}");
            }
            println!(
                "Present logical PnP keyboard/mouse nodes: {}",
                discovery.logical_pnp_devices.len()
            );
            for device in &discovery.logical_pnp_devices {
                println!("  [logical PnP input node] {device:#?}");
            }
            println!(
                "Aggregated physical input devices: {}",
                discovery.physical_devices.len()
            );
            println!(
                "Keyboard-capable physical devices: {}",
                discovery
                    .physical_devices
                    .iter()
                    .filter(|device| device.capabilities.keyboard)
                    .count()
            );
            println!(
                "Mouse-capable physical devices: {}",
                discovery
                    .physical_devices
                    .iter()
                    .filter(|device| device.capabilities.mouse)
                    .count()
            );
            for device in &discovery.physical_devices {
                println!("  [physical input aggregate] {device:#?}");
            }
            println!(
                "ASTER-related PnP devices kept separate: {}",
                discovery.aster_related_pnp_devices.len()
            );
            for device in discovery.aster_related_pnp_devices {
                println!("  {device:#?}");
            }
        }
        Err(error) => {
            eprintln!("Input enumeration failed: {error}");
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

fn print_aster_status(status: &environment::AsterEnvironmentStatus) {
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
