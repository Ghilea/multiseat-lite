use multiseat_lite_lib::{enumerate_hardware, virtualization};

fn main() {
    if let Err(error) = run() {
        eprintln!("Backend probe failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let hardware = enumerate_hardware()?;
    let snapshot = virtualization::probe(
        &hardware.input_devices,
        hardware.environment.aster.clone(),
        hardware.environment.current_session_id,
    )?;

    println!("SYSTEM");
    println!(
        "  Product: {}",
        text(snapshot.system.product_name.as_deref())
    );
    println!("  Edition: {}", text(snapshot.system.edition.as_deref()));
    println!(
        "  Display version: {}",
        text(snapshot.system.display_version.as_deref())
    );
    println!("  Build: {}", text(snapshot.system.build.as_deref()));
    println!("  Architecture: {}", snapshot.system.architecture);
    println!("  Session ID: {}", snapshot.system.current_session_id);

    println!("\nVIRTUALIZATION");
    println!(
        "  Hypervisor present: {}",
        yes_no(snapshot.system.hypervisor_present)
    );
    println!(
        "  Firmware virtualization: {:?}",
        snapshot.system.firmware_virtualization_enabled
    );
    println!(
        "  Second-level address translation: {:?}",
        snapshot.system.second_level_address_translation
    );
    println!("  Selected experimental backend: None");

    println!("\nVIRTUALBOX");
    println!("  Installed: {}", yes_no(snapshot.virtual_box.installed));
    println!(
        "  VBoxManage: {}",
        text(snapshot.virtual_box.vbox_manage_path.as_deref())
    );
    println!(
        "  Version: {}",
        text(snapshot.virtual_box.version.as_deref())
    );
    println!(
        "  Operational: {}",
        yes_no(snapshot.virtual_box.operational)
    );
    println!(
        "  Host USB devices: {}",
        snapshot.virtual_box.usb_devices.len()
    );
    println!(
        "  Parser warnings: {}",
        snapshot.virtual_box.parser_warnings.len()
    );
    for warning in &snapshot.virtual_box.parser_warnings {
        println!("    Warning: {warning}");
    }
    if let Some(error) = &snapshot.virtual_box.probe_error {
        println!("  Probe error: {error}");
    }
    if snapshot.virtual_box.operational {
        match virtualization::VirtualBoxRuntimeService::discover()
            .and_then(|backend| backend.list_vms())
        {
            Ok(vms) => {
                println!("  Registered VMs: {}", vms.machines.len());
                for vm in vms.machines {
                    println!(
                        "    {} | UUID={} | state={} | OS={}",
                        vm.name,
                        vm.uuid,
                        vm.state,
                        text(vm.guest_os_description.as_deref())
                    );
                }
                for warning in vms.warnings {
                    println!("    VM warning: {warning}");
                }
            }
            Err(error) => println!("  VM enumeration error: {error}"),
        }
        match virtualization::VirtualBoxRuntimeService::discover()
            .and_then(|backend| backend.current_host_creation_proposal())
        {
            Ok(proposal) => {
                println!("    RAM: {} MB", proposal.memory_mb);
                println!("    CPUs: {}", proposal.cpus);
                println!("    Disk: {} GB", proposal.disk_gb);
                println!(
                    "  Default managed profile: {:?}",
                    proposal.default_windows_version
                );
                for profile in proposal.profiles {
                    println!("  {} profile:", profile.display_name);
                    println!("    Guest OS type ID: {}", profile.virtual_box_os_type_id);
                    println!("    Description: {}", profile.guest_os_description);
                    println!("    Firmware: {}", profile.firmware);
                    println!("    TPM: {}", profile.tpm);
                    println!("    I/O APIC: {}", yes_no(profile.io_apic_enabled));
                    println!("    Secure Boot: {:?}", profile.secure_boot);
                    println!("    USB: {}", profile.usb_controller);
                    println!(
                        "    Graphics: {} / {} MB VRAM",
                        profile.graphics_controller, profile.vram_mb
                    );
                    println!("    Network: {}", profile.network);
                    println!("    Audio output: {}", yes_no(profile.audio_output_enabled));
                    println!("    Storage: {}", profile.storage_controller);
                    println!(
                        "    Guest Additions: {:?} / bundled ISO {}",
                        profile.guest_additions.state,
                        yes_no(profile.guest_additions.bundled_iso_path.is_some())
                    );
                }
            }
            Err(error) => println!("  Creation profile error: {error}"),
        }
    }

    println!("\nHYPER-V");
    println!("  Feature state: {:?}", snapshot.hyper_v.feature_state);
    println!(
        "  Hypervisor active: {}",
        yes_no(snapshot.hyper_v.hypervisor_active)
    );
    for service in &snapshot.hyper_v.services {
        println!(
            "  Service {}: installed={}, running={}, state={}",
            service.name,
            yes_no(service.installed),
            yes_no(service.running),
            service.state
        );
    }
    if let Some(error) = &snapshot.hyper_v.feature_probe_error {
        println!("  Feature probe note: {error}");
    }

    println!("\nASTER");
    println!(
        "  Installation detected: {}",
        yes_no(snapshot.aster.installation_detected)
    );
    let mut_enx = snapshot
        .aster
        .related_services
        .iter()
        .find(|service| service.name.eq_ignore_ascii_case("MUTENX_SERVICE"));
    println!(
        "  MutEnx service installed: {}",
        yes_no(mut_enx.is_some_and(|service| service.installed))
    );
    println!(
        "  MutEnx service running: {}",
        yes_no(mut_enx.is_some_and(|service| service.running))
    );
    println!(
        "  MutEnx device node present: {}",
        yes_no(snapshot.aster.mut_enx_device_node_present)
    );
    println!(
        "  Workplace/input state: {:?}",
        snapshot.aster.workplace_state
    );

    println!("\nWINDOWS PHYSICAL INPUT -> VIRTUALBOX USB CORRELATION");
    for physical in &hardware.input_devices {
        let mapping = snapshot.usb_correlations.iter().find(|mapping| {
            mapping
                .physical_device_id
                .eq_ignore_ascii_case(&physical.id)
        });
        println!(
            "{}",
            physical
                .friendly_name
                .as_deref()
                .unwrap_or("Physical input device")
        );
        println!("  Container: {}", text(physical.container_id.as_deref()));
        println!(
            "  VID/PID: {}:{}",
            text(physical.vendor_id.as_deref()),
            text(physical.product_id.as_deref())
        );
        println!(
            "  USB parent: {}",
            text(physical.usb_parent_instance_id.as_deref())
        );
        println!("  Serial: {}", text(physical.serial_number.as_deref()));
        if let Some(mapping) = mapping {
            println!("  Mapping: {:?}", mapping.state);
            println!(
                "  VirtualBox UUID: {}",
                text(mapping.matched_uuid.as_deref())
            );
            let matched = mapping.matched_uuid.as_deref().and_then(|uuid| {
                snapshot
                    .virtual_box
                    .usb_devices
                    .iter()
                    .find(|device| device.uuid.eq_ignore_ascii_case(uuid))
            });
            println!(
                "  Current State: {}",
                text(matched.and_then(|device| device.current_state.as_deref()))
            );
            println!("  Reason: {}", mapping.reason);
        }
    }
    Ok(())
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn text(value: Option<&str>) -> &str {
    value.unwrap_or("unavailable")
}
