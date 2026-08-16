//! Read-only diagnostic for validating recovery of an existing managed VM.

use std::{env, fs, path::PathBuf};

use multiseat_lite_lib::{seats::ApplicationConfig, virtualization::VirtualBoxRuntimeService};

fn main() -> Result<(), String> {
    let app_data = env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "APPDATA is unavailable".to_owned())?;
    let config_path = app_data.join("se.multiseat.lite").join("config.json");
    let text = fs::read_to_string(&config_path)
        .map_err(|error| format!("could not read '{}': {error}", config_path.display()))?;
    let configuration: ApplicationConfig = serde_json::from_str(&text)
        .map_err(|error| format!("could not parse '{}': {error}", config_path.display()))?;
    let backend = configuration
        .backends
        .virtual_box
        .seats
        .get("seat-2")
        .ok_or_else(|| "seat-2 has no VirtualBox configuration".to_owned())?;
    let journal = backend
        .installation
        .as_ref()
        .ok_or_else(|| "seat-2 has no managed installation journal".to_owned())?;
    let runtime = VirtualBoxRuntimeService::discover()?;
    let status =
        runtime.managed_vm_recovery_status(&backend.vm_id, backend.managed_by_multiseat, journal);

    println!("Managed: {}", backend.managed_by_multiseat);
    println!("UUID: {}", status.vm_id);
    println!("Name: {}", status.vm_name.as_deref().unwrap_or("unknown"));
    println!("State: {}", status.vm_state.as_deref().unwrap_or("unknown"));
    println!("Recovery classification: {:?}", status.classification);
    println!(
        "Firmware virtualization: {:?}",
        status.firmware_virtualization
    );
    println!("VirtualBox execution: {:?}", status.virtual_box_execution);
    println!("Virtualization ready: {}", status.virtualization_ready);
    println!("VM configuration valid: {}", status.vm_configuration_valid);
    println!(
        "Unattended preparation present: {}",
        status.unattended_preparation_present
    );
    println!(
        "Unattended media: {}",
        status.unattended_media_path.as_deref().unwrap_or("none")
    );
    if let Some(problem) = status.previous_problem.as_deref() {
        println!("Previous problem: {problem}");
    }
    if let Some(failure) = status.previous_failure.as_deref() {
        println!("Previous failure: {failure}");
    }
    println!("OS type ID: {}", status.virtual_box_os_type_id);
    println!(
        "OS type description: {}",
        status
            .virtual_box_os_description
            .as_deref()
            .unwrap_or("unknown")
    );
    println!(
        "System disk: {}",
        status.system_disk_path.as_deref().unwrap_or("unknown")
    );
    println!("ISO: {}", status.iso_path.as_deref().unwrap_or("unknown"));
    println!("Credentials required: {}", status.credentials_required);
    println!(
        "Suggested computer name: {}",
        status.suggested_computer_name
    );
    println!("Suggested DNS domain: {}", status.suggested_domain_name);
    println!("Generated hostname: {}", status.suggested_hostname);
    println!("Can safely resume: {}", status.can_resume);
    if let Some(reason) = status.reason {
        println!("Reason: {reason}");
    }
    Ok(())
}
