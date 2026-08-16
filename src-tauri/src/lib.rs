pub mod devices;
mod displays;
pub mod environment;
mod input_identification;
pub mod keyboard_routing;
pub mod seats;
pub mod virtualization;

use serde::Serialize;
use tauri::Manager;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemOverview {
    devices: usize,
    displays: usize,
    seats: usize,
    virtualization_available: bool,
}

#[tauri::command]
fn get_system_overview(
    store: tauri::State<'_, seats::ConfigStore>,
) -> Result<SystemOverview, String> {
    let hardware = enumerate_hardware()?;
    let backend = virtualization::probe(
        &hardware.input_devices,
        hardware.environment.aster.clone(),
        hardware.environment.current_session_id,
    )?;
    Ok(SystemOverview {
        devices: hardware.input_devices.len(),
        displays: hardware.pnp_monitors.len(),
        seats: store.get()?.seats.len(),
        virtualization_available: backend
            .backend_capabilities
            .iter()
            .any(|capability| capability.available),
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareSnapshot {
    pub displays: Vec<displays::DisplayDevice>,
    /// Aggregated, assignable physical input devices.
    pub input_devices: Vec<devices::PhysicalInputDevice>,
    /// Individual Windows keyboard/mouse class devnodes retained for diagnostics.
    pub logical_input_devices: Vec<devices::InputDevice>,
    pub pnp_monitors: Vec<displays::PnpMonitor>,
    pub display_config_targets: Vec<displays::DisplayConfigTarget>,
    pub session_raw_input_devices: Vec<devices::RawInputDevice>,
    pub aster_related_pnp_devices: Vec<devices::AsterRelatedPnpDevice>,
    pub environment: environment::EnvironmentInfo,
}

pub fn enumerate_hardware() -> Result<HardwareSnapshot, String> {
    let display_discovery = displays::enumerate()?;
    let input_discovery = devices::enumerate()?;
    let environment = environment::detect(input_discovery.aster_device_evidence())?;

    Ok(HardwareSnapshot {
        displays: display_discovery.session_visible,
        pnp_monitors: display_discovery.pnp_monitors,
        display_config_targets: display_discovery.display_config_targets,
        input_devices: input_discovery.physical_devices,
        logical_input_devices: input_discovery.logical_pnp_devices,
        session_raw_input_devices: input_discovery.session_raw_input_devices,
        aster_related_pnp_devices: input_discovery.aster_related_pnp_devices,
        environment,
    })
}

#[tauri::command]
fn get_hardware_snapshot() -> Result<HardwareSnapshot, String> {
    enumerate_hardware()
}

#[tauri::command]
fn get_backend_probe() -> Result<virtualization::BackendProbeSnapshot, String> {
    let hardware = enumerate_hardware()?;
    virtualization::probe(
        &hardware.input_devices,
        hardware.environment.aster.clone(),
        hardware.environment.current_session_id,
    )
}

#[tauri::command]
fn get_application_config(
    store: tauri::State<'_, seats::ConfigStore>,
) -> Result<seats::ApplicationConfigSnapshot, String> {
    configuration_snapshot(&store)
}

#[tauri::command(rename_all = "camelCase")]
fn update_seat(
    store: tauri::State<'_, seats::ConfigStore>,
    seat_id: seats::SeatId,
    name: String,
) -> Result<seats::ApplicationConfigSnapshot, String> {
    store.update_seat(seat_id, name)?;
    configuration_snapshot(&store)
}

#[tauri::command(rename_all = "camelCase")]
fn assign_device(
    store: tauri::State<'_, seats::ConfigStore>,
    seat_id: seats::SeatId,
    slot: seats::AssignmentSlot,
    device_id: String,
) -> Result<seats::ApplicationConfigSnapshot, String> {
    let hardware = enumerate_hardware()?;
    store.reconcile_hardware(&hardware)?;
    validate_assignable_device(&hardware, slot, &device_id)?;
    match slot {
        seats::AssignmentSlot::Keyboard | seats::AssignmentSlot::Mouse => {
            let physical = hardware
                .input_devices
                .iter()
                .find(|device| device.id.eq_ignore_ascii_case(&device_id))
                .ok_or_else(|| format!("physical input device '{device_id}' disappeared"))?;
            store.assign_physical_input(seat_id, physical)?;
        }
        _ => store.assign(seat_id, slot, device_id)?,
    }
    let configuration = store.get()?;
    Ok(store.snapshot(&hardware).map(|mut snapshot| {
        snapshot.configuration = configuration;
        snapshot
    })?)
}

#[tauri::command(rename_all = "camelCase")]
fn unassign_device(
    store: tauri::State<'_, seats::ConfigStore>,
    seat_id: seats::SeatId,
    slot: seats::AssignmentSlot,
) -> Result<seats::ApplicationConfigSnapshot, String> {
    store.unassign(seat_id, slot)?;
    configuration_snapshot(&store)
}

#[tauri::command]
fn save_configuration(store: tauri::State<'_, seats::ConfigStore>) -> Result<(), String> {
    store.save()
}

#[tauri::command]
fn start_input_identification(
    app: tauri::AppHandle,
    service: tauri::State<'_, input_identification::InputIdentificationService>,
) -> Result<input_identification::IdentificationStatus, String> {
    service.start(app)
}

#[tauri::command]
fn stop_input_identification(
    service: tauri::State<'_, input_identification::InputIdentificationService>,
) -> Result<input_identification::IdentificationStatus, String> {
    service.stop()
}

#[tauri::command(rename_all = "camelCase")]
fn start_keyboard_routing_diagnostic(
    app: tauri::AppHandle,
    store: tauri::State<'_, seats::ConfigStore>,
    service: tauri::State<'_, input_identification::InputIdentificationService>,
    seat_id: seats::SeatId,
) -> Result<keyboard_routing::KeyboardRoutingDiagnosticStatus, String> {
    let configuration = store.get()?;
    let routing = configuration.input_routing(&seat_id);
    if routing.keyboard != seats::InputRoutingStrategy::NativeKeyboardRouting {
        return Err("seat keyboard is not configured for native routing".to_owned());
    }
    let keyboard = configuration
        .seats
        .iter()
        .find(|seat| seat.id == seat_id)
        .and_then(|seat| seat.devices.keyboard.as_ref())
        .ok_or_else(|| "seat has no configured keyboard".to_owned())?;
    service.start_keyboard_routing(app, keyboard.0.clone())
}

#[tauri::command(rename_all = "camelCase")]
fn start_native_keyboard_injection_test(
    app: tauri::AppHandle,
    store: tauri::State<'_, seats::ConfigStore>,
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
    service: tauri::State<'_, input_identification::InputIdentificationService>,
    seat_id: seats::SeatId,
) -> Result<keyboard_routing::KeyboardRoutingDiagnosticStatus, String> {
    let configuration = store.get()?;
    if configuration.input_routing(&seat_id).keyboard
        != seats::InputRoutingStrategy::NativeKeyboardRouting
    {
        return Err("seat keyboard is not configured for native routing".to_owned());
    }
    let backend = configuration
        .backends
        .virtual_box
        .seats
        .get(&seat_id.0)
        .ok_or_else(|| "seat has no VirtualBox VM selected".to_owned())?;
    if !backend.managed_by_multiseat {
        return Err("keyboard injection is limited to a MultiSeat Lite managed VM".to_owned());
    }
    let vm = runtime.inspect_vm(&backend.vm_id)?;
    if !vm.state.eq_ignore_ascii_case("running") {
        return Err(format!(
            "managed VM '{}' must be running before keyboard injection (state: {})",
            vm.name, vm.state
        ));
    }
    let keyboard = configuration
        .seats
        .iter()
        .find(|seat| seat.id == seat_id)
        .and_then(|seat| seat.devices.keyboard.as_ref())
        .ok_or_else(|| "seat has no configured keyboard".to_owned())?;
    let hardware = enumerate_hardware()?;
    if !hardware
        .input_devices
        .iter()
        .any(|device| device.capabilities.keyboard && device.id.eq_ignore_ascii_case(&keyboard.0))
    {
        return Err(format!(
            "configured Barnen keyboard '{}' is not present",
            keyboard.0
        ));
    }
    let sink = virtualization::VirtualBoxGuestKeyboardSink::discover(backend.vm_id.clone())?;
    service.start_keyboard_injection(app, keyboard.0.clone(), Box::new(sink))
}

#[tauri::command]
fn stop_keyboard_routing_diagnostic(
    service: tauri::State<'_, input_identification::InputIdentificationService>,
) -> Result<keyboard_routing::KeyboardRoutingDiagnosticStatus, String> {
    service.stop_keyboard_routing()
}

#[tauri::command]
fn list_virtual_box_vms(
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
) -> Result<virtualization::VirtualMachineList, String> {
    runtime.list_vms()
}

#[tauri::command(rename_all = "camelCase")]
fn select_virtual_box_vm(
    store: tauri::State<'_, seats::ConfigStore>,
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
    seat_id: seats::SeatId,
    vm_id: String,
) -> Result<seats::ApplicationConfigSnapshot, String> {
    if !vm_id.trim().is_empty() {
        let vm = runtime.inspect_vm(&vm_id)?;
        let errors = virtualization::validate_vm(&vm);
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
    }
    store.set_virtual_box_vm(seat_id, vm_id)?;
    configuration_snapshot(&store)
}

#[tauri::command(rename_all = "camelCase")]
fn get_seat_runtime_status(
    store: tauri::State<'_, seats::ConfigStore>,
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
    seat_id: seats::SeatId,
) -> Result<virtualization::SeatRuntimeSnapshot, String> {
    let hardware = enumerate_hardware()?;
    runtime.status(&seat_id, &store.get()?, &hardware.input_devices)
}

#[tauri::command(rename_all = "camelCase")]
fn start_virtual_box_seat(
    window: tauri::Window,
    store: tauri::State<'_, seats::ConfigStore>,
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
    seat_id: seats::SeatId,
) -> Result<virtualization::SeatOperationResult, String> {
    let hardware = enumerate_hardware()?;
    store.reconcile_hardware(&hardware)?;
    let mut configuration = store.get()?;
    if runtime.ensure_managed_mouse_capture_disabled(&seat_id, &configuration)? {
        store.set_mouse_capture_policy_owned(seat_id.clone(), true)?;
        configuration = store.get()?;
    }
    match runtime.start(
        &seat_id,
        &configuration,
        &hardware.input_devices,
        focus_anchor(&window),
    ) {
        Ok(snapshot) => Ok(virtualization::SeatOperationResult {
            success: true,
            runtime: snapshot,
            errors: Vec::new(),
            rollback_attempted: false,
        }),
        Err(error) => {
            runtime.record_error(&seat_id, &error)?;
            let snapshot = runtime.status(&seat_id, &configuration, &hardware.input_devices)?;
            let rollback_attempted = snapshot
                .logs
                .iter()
                .any(|log| log.action == "rollback-detach");
            Ok(virtualization::SeatOperationResult {
                success: false,
                runtime: snapshot,
                errors: vec![error],
                rollback_attempted,
            })
        }
    }
}

#[tauri::command(rename_all = "camelCase")]
fn allow_virtual_box_host_input_temporarily(
    store: tauri::State<'_, seats::ConfigStore>,
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
    seat_id: seats::SeatId,
) -> Result<virtualization::SeatOperationResult, String> {
    let hardware = enumerate_hardware()?;
    let runtime =
        runtime.allow_host_input_temporarily(&seat_id, &store.get()?, &hardware.input_devices)?;
    Ok(virtualization::SeatOperationResult {
        success: true,
        runtime,
        errors: Vec::new(),
        rollback_attempted: false,
    })
}

#[tauri::command(rename_all = "camelCase")]
fn lock_virtual_box_seat_input(
    window: tauri::Window,
    store: tauri::State<'_, seats::ConfigStore>,
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
    seat_id: seats::SeatId,
) -> Result<virtualization::SeatOperationResult, String> {
    let hardware = enumerate_hardware()?;
    let runtime = runtime.lock_seat_input(
        &seat_id,
        &store.get()?,
        &hardware.input_devices,
        focus_anchor(&window),
    )?;
    Ok(virtualization::SeatOperationResult {
        success: true,
        runtime,
        errors: Vec::new(),
        rollback_attempted: false,
    })
}

fn focus_anchor(window: &tauri::Window) -> Option<isize> {
    #[cfg(target_os = "windows")]
    {
        return window.hwnd().ok().map(|handle| handle.0 as isize);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        None
    }
}

#[tauri::command(rename_all = "camelCase")]
fn stop_virtual_box_seat(
    store: tauri::State<'_, seats::ConfigStore>,
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
    input_service: tauri::State<'_, input_identification::InputIdentificationService>,
    seat_id: seats::SeatId,
) -> Result<virtualization::SeatOperationResult, String> {
    let _ = input_service.stop_keyboard_routing();
    let hardware = enumerate_hardware()?;
    let snapshot = runtime.stop(&seat_id, &store.get()?, &hardware.input_devices)?;
    Ok(virtualization::SeatOperationResult {
        success: true,
        runtime: snapshot,
        errors: Vec::new(),
        rollback_attempted: false,
    })
}

#[tauri::command(rename_all = "camelCase")]
fn release_virtual_box_seat_devices(
    store: tauri::State<'_, seats::ConfigStore>,
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
    seat_id: seats::SeatId,
) -> Result<virtualization::SeatOperationResult, String> {
    let hardware = enumerate_hardware()?;
    let snapshot = runtime.release(&seat_id, &store.get()?, &hardware.input_devices)?;
    Ok(virtualization::SeatOperationResult {
        success: true,
        runtime: snapshot,
        errors: Vec::new(),
        rollback_attempted: false,
    })
}

#[tauri::command]
fn get_virtual_box_vm_creation_proposal(
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
) -> Result<virtualization::VmCreationProposal, String> {
    runtime.current_host_creation_proposal()
}

#[tauri::command(rename_all = "camelCase")]
fn create_virtual_box_vm(
    store: tauri::State<'_, seats::ConfigStore>,
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
    seat_id: seats::SeatId,
    request: virtualization::VmCreationRequest,
) -> Result<virtualization::ManagedVmSetupResult, String> {
    let result = runtime
        .create_vm(&request)
        .map_err(|failure| failure.to_string())?;
    let initial_installation = seats::ManagedVmInstallation {
        windows_version: request.windows_version,
        virtual_box_os_type_id: result.profile.virtual_box_os_type_id.clone(),
        computer_name: request.installation.computer_name.clone(),
        domain_name: request.installation.domain_name.clone(),
        state: seats::ManagedInstallState::Creating,
        guest_additions: if request.installation.install_guest_additions {
            seats::GuestAdditionsInstallState::Installing
        } else {
            seats::GuestAdditionsInstallState::NotInstalled
        },
        guest_additions_version: None,
        last_error: None,
        observed: seats::ManagedInstallationObservation {
            windows: seats::WindowsInstallationStatus::Installing,
            managed: seats::ManagedVmStatus::Installing,
            ..seats::ManagedInstallationObservation::default()
        },
    };
    if let Err(persistence_error) = store.set_managed_virtual_box_vm(
        seat_id.clone(),
        result.vm.uuid.clone(),
        initial_installation,
    ) {
        let rollback = runtime
            .rollback_created_vm_after_persistence_failure(&result.vm.uuid)
            .map(|_| "managed VM deleted".to_owned())
            .unwrap_or_else(|error| format!("rollback failed: {error}"));
        return Err(format!(
            "VM was created but its stable UUID could not be persisted: {persistence_error}; {rollback}"
        ));
    }
    let additions_iso = virtualization::bundled_guest_additions_iso();
    match runtime.prepare_unattended_install(
        &result.vm.uuid,
        &request.iso_path,
        &request.installation,
        &result.profile,
        additions_iso.as_deref(),
    ) {
        Ok(installation) => {
            store.update_managed_vm_installation(seat_id, installation.clone())?;
            Ok(virtualization::ManagedVmSetupResult {
                creation: result,
                installation,
            })
        }
        Err(failure) => {
            let failed = seats::ManagedVmInstallation {
                windows_version: request.windows_version,
                virtual_box_os_type_id: result.profile.virtual_box_os_type_id.clone(),
                computer_name: request.installation.computer_name.clone(),
                domain_name: request.installation.domain_name.clone(),
                state: seats::ManagedInstallState::Failed,
                guest_additions: if request.installation.install_guest_additions {
                    seats::GuestAdditionsInstallState::Failed
                } else {
                    seats::GuestAdditionsInstallState::NotInstalled
                },
                guest_additions_version: None,
                last_error: Some(failure.to_string()),
                observed: seats::ManagedInstallationObservation {
                    managed: seats::ManagedVmStatus::Failed,
                    ..seats::ManagedInstallationObservation::default()
                },
            };
            let persistence = store.update_managed_vm_installation(seat_id, failed);
            Err(match persistence {
                Ok(()) => failure.to_string(),
                Err(error) => format!("{failure}; could not persist failure journal: {error}"),
            })
        }
    }
}

#[tauri::command(rename_all = "camelCase")]
fn get_managed_vm_installation_status(
    store: tauri::State<'_, seats::ConfigStore>,
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
    seat_id: seats::SeatId,
) -> Result<virtualization::ManagedInstallationSnapshot, String> {
    let configuration = store.get()?;
    let backend = configuration
        .backends
        .virtual_box
        .seats
        .get(&seat_id.0)
        .ok_or_else(|| format!("seat '{}' has no VirtualBox VM", seat_id.0))?;
    if !backend.managed_by_multiseat {
        return Err("the selected VM is not managed by MultiSeat Lite".to_owned());
    }
    let previous = backend
        .installation
        .as_ref()
        .ok_or_else(|| "managed VM has no installation journal".to_owned())?;
    let snapshot = runtime.inspect_managed_installation(&backend.vm_id, previous)?;
    store.update_managed_vm_installation(seat_id, snapshot.installation.clone())?;
    Ok(snapshot)
}

#[tauri::command(rename_all = "camelCase")]
fn get_managed_vm_recovery_status(
    store: tauri::State<'_, seats::ConfigStore>,
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
    seat_id: seats::SeatId,
) -> Result<virtualization::ManagedVmRecoveryStatus, String> {
    let configuration = store.get()?;
    let backend = configuration
        .backends
        .virtual_box
        .seats
        .get(&seat_id.0)
        .ok_or_else(|| format!("seat '{}' has no VirtualBox VM", seat_id.0))?;
    let journal = backend
        .installation
        .as_ref()
        .ok_or_else(|| "managed VM has no installation journal".to_owned())?;
    Ok(runtime.managed_vm_recovery_status(&backend.vm_id, backend.managed_by_multiseat, journal))
}

#[tauri::command(rename_all = "camelCase")]
fn resume_managed_vm_installation(
    store: tauri::State<'_, seats::ConfigStore>,
    runtime: tauri::State<'_, virtualization::VirtualBoxRuntimeService>,
    seat_id: seats::SeatId,
    mut installation: virtualization::WindowsInstallConfig,
) -> Result<seats::ManagedVmInstallation, String> {
    virtualization::apply_legacy_managed_install_defaults(&mut installation);
    let configuration = store.get()?;
    let backend = configuration
        .backends
        .virtual_box
        .seats
        .get(&seat_id.0)
        .cloned()
        .ok_or_else(|| format!("seat '{}' has no VirtualBox VM", seat_id.0))?;
    let previous = backend
        .installation
        .clone()
        .ok_or_else(|| "managed VM has no installation journal".to_owned())?;
    let recovery =
        runtime.managed_vm_recovery_status(&backend.vm_id, backend.managed_by_multiseat, &previous);
    if !recovery.can_resume {
        return Err(recovery
            .reason
            .unwrap_or_else(|| "managed VM cannot safely resume".to_owned()));
    }

    let preparing = seats::ManagedVmInstallation {
        state: seats::ManagedInstallState::Creating,
        guest_additions: if installation.install_guest_additions {
            seats::GuestAdditionsInstallState::Installing
        } else {
            seats::GuestAdditionsInstallState::NotInstalled
        },
        guest_additions_version: None,
        // Keep the previous diagnostic until the retry has actually started.
        last_error: previous.last_error.clone(),
        ..previous.clone()
    };
    store.update_managed_vm_installation(seat_id.clone(), preparing)?;

    let additions_iso = virtualization::bundled_guest_additions_iso();
    match runtime.resume_managed_unattended_install(
        &backend.vm_id,
        backend.managed_by_multiseat,
        &previous,
        &installation,
        additions_iso.as_deref(),
    ) {
        Ok(journal) => {
            store.update_managed_vm_installation(seat_id, journal.clone())?;
            Ok(journal)
        }
        Err(failure) => {
            let failure_text = failure.to_string();
            let diagnostic = previous.last_error.as_deref().map_or_else(
                || failure_text.clone(),
                |previous_error| {
                    format!(
                        "Previous failure: {previous_error}\nLatest retry failure: {failure_text}"
                    )
                },
            );
            let failed = seats::ManagedVmInstallation {
                state: seats::ManagedInstallState::Failed,
                guest_additions: if installation.install_guest_additions {
                    seats::GuestAdditionsInstallState::Failed
                } else {
                    seats::GuestAdditionsInstallState::NotInstalled
                },
                guest_additions_version: None,
                last_error: Some(diagnostic),
                ..previous
            };
            let persistence = store.update_managed_vm_installation(seat_id, failed);
            Err(match persistence {
                Ok(()) => failure.to_string(),
                Err(error) => format!("{failure}; could not persist failure journal: {error}"),
            })
        }
    }
}

fn configuration_snapshot(
    store: &seats::ConfigStore,
) -> Result<seats::ApplicationConfigSnapshot, String> {
    let hardware = enumerate_hardware()?;
    store.reconcile_hardware(&hardware)?;
    store.snapshot(&hardware)
}

fn reconcile_managed_installations_on_startup(
    store: &seats::ConfigStore,
    runtime: &virtualization::VirtualBoxRuntimeService,
) {
    let Ok(configuration) = store.get() else {
        return;
    };
    let managed: Vec<_> = configuration
        .backends
        .virtual_box
        .seats
        .iter()
        .filter_map(|(seat_id, backend)| {
            (backend.managed_by_multiseat)
                .then(|| {
                    backend
                        .installation
                        .clone()
                        .map(|journal| (seat_id.clone(), backend.vm_id.clone(), journal))
                })
                .flatten()
        })
        .collect();
    for (seat_id, vm_id, journal) in managed {
        match runtime.inspect_managed_installation(&vm_id, &journal) {
            Ok(snapshot) if snapshot.installation != journal => {
                if let Err(error) = store.update_managed_vm_installation(
                    seats::SeatId(seat_id.clone()),
                    snapshot.installation,
                ) {
                    eprintln!("managed VM startup reconciliation for seat '{seat_id}' could not be persisted: {error}");
                }
            }
            Ok(_) => {}
            Err(error) => eprintln!(
                "managed VM startup reconciliation for seat '{seat_id}' was unavailable: {error}"
            ),
        }
    }
}

fn validate_assignable_device(
    hardware: &HardwareSnapshot,
    slot: seats::AssignmentSlot,
    device_id: &str,
) -> Result<(), String> {
    let match_count = match slot {
        seats::AssignmentSlot::Display => hardware
            .pnp_monitors
            .iter()
            .filter(|monitor| monitor.instance_id.eq_ignore_ascii_case(device_id))
            .count(),
        seats::AssignmentSlot::Keyboard => hardware
            .input_devices
            .iter()
            .filter(|device| {
                device.capabilities.keyboard && device.id.eq_ignore_ascii_case(device_id)
            })
            .count(),
        seats::AssignmentSlot::Mouse => hardware
            .input_devices
            .iter()
            .filter(|device| device.capabilities.mouse && device.id.eq_ignore_ascii_case(device_id))
            .count(),
        seats::AssignmentSlot::AudioOutput => {
            return Err("audio output discovery is not implemented yet".to_owned())
        }
    };
    match match_count {
        0 => Err(format!(
            "{slot:?} device '{device_id}' is not present in physical PnP discovery"
        )),
        1 => Ok(()),
        _ => Err(format!(
            "{slot:?} device '{device_id}' is ambiguous in physical PnP discovery"
        )),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let config_path = app
                .path()
                .app_config_dir()
                .map_err(std::io::Error::other)?
                .join("config.json");
            let store =
                seats::ConfigStore::initialize(config_path).map_err(std::io::Error::other)?;
            let runtime = virtualization::VirtualBoxRuntimeService::from_environment();
            reconcile_managed_installations_on_startup(&store, &runtime);
            app.manage(store);
            app.manage(input_identification::InputIdentificationService::default());
            app.manage(runtime);
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                let service = window.state::<input_identification::InputIdentificationService>();
                let _ = service.stop();
                let runtime = window.state::<virtualization::VirtualBoxRuntimeService>();
                let _ = runtime.shutdown_focus_protection();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_system_overview,
            get_hardware_snapshot,
            get_backend_probe,
            get_application_config,
            update_seat,
            assign_device,
            unassign_device,
            save_configuration,
            start_input_identification,
            stop_input_identification,
            start_keyboard_routing_diagnostic,
            start_native_keyboard_injection_test,
            stop_keyboard_routing_diagnostic,
            list_virtual_box_vms,
            select_virtual_box_vm,
            get_seat_runtime_status,
            start_virtual_box_seat,
            allow_virtual_box_host_input_temporarily,
            lock_virtual_box_seat_input,
            stop_virtual_box_seat,
            release_virtual_box_seat_devices,
            get_virtual_box_vm_creation_proposal,
            create_virtual_box_vm,
            get_managed_vm_installation_status,
            get_managed_vm_recovery_status,
            resume_managed_vm_installation
        ])
        .run(tauri::generate_context!())
        .expect("kunde inte starta Multiseat Lite");
}
