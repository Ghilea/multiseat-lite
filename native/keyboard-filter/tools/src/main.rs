use std::{env, fs, path::PathBuf, process::Command};

use multiseat_keyboard_filter_policy::{
    evaluate_pass_through_readiness, generate_test_machine_inf, InfGenerationRequest,
    PassThroughReadiness, PassThroughReadinessEvidence, PhysicalKeyboardIdentity,
};
use multiseat_lite_lib::{
    devices::{InputDeviceType, PhysicalInputDevice},
    enumerate_hardware,
    seats::{ApplicationConfig, PhysicalDeviceId},
};

const DRIVER_SERVICE: &str = "MultiSeatKeyboardFilter";
const MIN_WINDOWS_BUILD: u32 = 19_041; // Windows 10 2004: KMDF 1.31 baseline.

fn main() {
    if let Err(error) = run() {
        eprintln!("NotReady: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        return Err(usage());
    };
    let config_path = option(&args, "--config")
        .map(PathBuf::from)
        .or_else(default_config_path)
        .ok_or_else(|| "pass --config or set APPDATA".to_owned())?;
    let config = load_config(&config_path)?;
    let hardware = enumerate_hardware()?;
    let keyboards: Vec<_> = hardware
        .input_devices
        .iter()
        .filter(|device| device.capabilities.keyboard)
        .collect();

    match command {
        "readiness" => readiness(
            &config,
            &keyboards,
            args.iter().any(|arg| arg == "--remote-recovery-confirmed"),
        ),
        "generate-inf" => generate_inf(&args, &config, &keyboards),
        "snapshot" => write_hardware_snapshot(&args, &hardware),
        _ => Err(usage()),
    }
}

fn write_hardware_snapshot(
    args: &[String],
    hardware: &multiseat_lite_lib::HardwareSnapshot,
) -> Result<(), String> {
    let output = PathBuf::from(required_option(args, "--output")?);
    if output.exists() {
        return Err("refusing to overwrite an existing identity snapshot".into());
    }
    let contents = serde_json::to_string_pretty(hardware)
        .map_err(|error| format!("could not serialize hardware snapshot: {error}"))?;
    fs::write(&output, contents)
        .map_err(|error| format!("could not write {}: {error}", output.display()))?;
    println!("Read-only hardware identity snapshot: {}", output.display());
    println!("No key contents were captured and no device state was changed.");
    Ok(())
}

fn readiness(
    config: &ApplicationConfig,
    keyboards: &[&PhysicalInputDevice],
    remote_recovery_confirmed: bool,
) -> Result<(), String> {
    let dennis_id = assigned_keyboard(config, "dennis")?;
    let barnen_id = assigned_keyboard(config, "barnen")?;
    let dennis_matches = resolve(keyboards, dennis_id);
    let barnen_matches = resolve(keyboards, barnen_id);
    let physical: Vec<_> = keyboards.iter().map(|device| identity(device)).collect();
    let dennis: Vec<_> = dennis_matches
        .iter()
        .map(|device| identity(device))
        .collect();
    let barnen: Vec<_> = barnen_matches
        .iter()
        .map(|device| identity(device))
        .collect();
    let windows_build = windows_build_number();
    let driver_installed = driver_service_installed();
    let stack_enumerated = keyboards.iter().all(|device| {
        device.logical_nodes.iter().any(|node| {
            node.device_type == InputDeviceType::Keyboard && !node.instance_id.trim().is_empty()
        })
    });

    println!(
        "Windows build: {}",
        windows_build.map_or_else(|| "unknown".into(), |v| v.to_string())
    );
    println!("Driver build architecture: x64");
    println!("Driver service already installed: {driver_installed}");
    println!("Remote recovery explicitly confirmed: {remote_recovery_confirmed}");
    print_resolution("Dennis", dennis_id, &dennis_matches);
    print_resolution("Barnen", barnen_id, &barnen_matches);
    print_keyboard_stacks(keyboards);

    let result = evaluate_pass_through_readiness(&PassThroughReadinessEvidence {
        supported_windows_x64: cfg!(all(target_os = "windows", target_arch = "x86_64"))
            && windows_build.is_some_and(|build| build >= MIN_WINDOWS_BUILD),
        driver_build_is_x64: cfg!(target_arch = "x86_64"),
        physical_keyboards: &physical,
        dennis_matches: &dennis,
        barnen_matches: &barnen,
        keyboard_stack_enumerated: stack_enumerated,
        driver_already_installed: driver_installed,
        remote_recovery_confirmed,
    });
    match result {
        PassThroughReadiness::ReadyForPassThroughDriverTest => {
            println!("ReadyForPassThroughDriverTest");
            Ok(())
        }
        PassThroughReadiness::NotReady(reasons) => {
            println!("NotReady");
            for reason in reasons {
                println!("  - {reason:?}");
            }
            Err("test-machine readiness requirements are not satisfied".into())
        }
    }
}

fn generate_inf(
    args: &[String],
    config: &ApplicationConfig,
    keyboards: &[&PhysicalInputDevice],
) -> Result<(), String> {
    let target_id = required_option(args, "--target")?;
    let container_id = required_option(args, "--container")?;
    let tlc_id = required_option(args, "--tlc")?;
    let hardware_id = required_option(args, "--hardware-id")?;
    let output = PathBuf::from(required_option(args, "--output")?);
    let template_path = option(args, "--template")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("native/keyboard-filter/driver/MultiSeatKeyboardFilter.inf.template")
        });
    let targets: Vec<_> = keyboards
        .iter()
        .copied()
        .filter(|device| device.id.eq_ignore_ascii_case(target_id))
        .collect();
    if targets.len() != 1 {
        return Err(format!(
            "explicit target '{target_id}' resolved {} times",
            targets.len()
        ));
    }
    let target = targets[0];
    if !target
        .container_id
        .as_deref()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(container_id))
    {
        return Err("explicit Container ID does not match the resolved physical target".into());
    }
    let barnen_id = assigned_keyboard(config, "barnen")?;
    if !matches_assignment(target, barnen_id) {
        return Err("explicit target is not the persisted Barnen keyboard assignment".into());
    }
    let dennis_id = assigned_keyboard(config, "dennis")?;
    let dennis_matches = resolve(keyboards, dennis_id);
    if dennis_matches.len() != 1 {
        return Err("Dennis/primary keyboard identity is missing or ambiguous".into());
    }
    let node = target
        .logical_nodes
        .iter()
        .filter(|node| {
            node.device_type == InputDeviceType::Keyboard
                && node.instance_id.eq_ignore_ascii_case(tlc_id)
        })
        .collect::<Vec<_>>();
    if node.len() != 1 {
        return Err(format!(
            "exact keyboard TLC '{tlc_id}' resolved {} times",
            node.len()
        ));
    }
    if !node[0]
        .hardware_ids
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(hardware_id))
    {
        return Err("hardware ID does not belong to the selected keyboard TLC".into());
    }
    let hardware_id_matches = keyboards
        .iter()
        .flat_map(|device| &device.logical_nodes)
        .filter(|logical| logical.device_type == InputDeviceType::Keyboard)
        .flat_map(|logical| &logical.hardware_ids)
        .filter(|candidate| candidate.eq_ignore_ascii_case(hardware_id))
        .count();
    if hardware_id_matches != 1 {
        return Err(format!(
            "hardware ID occurs on {hardware_id_matches} keyboard TLCs; refusing an ambiguous INF binding"
        ));
    }

    println!("Selected physical ID: {}", target.id);
    println!(
        "Container ID: {}",
        target.container_id.as_deref().unwrap_or("unavailable")
    );
    println!("Keyboard TLC instance: {}", node[0].instance_id);
    println!("Exact hardware ID: {hardware_id}");
    println!("Output INF: {}", output.display());
    println!("This command generates only; it never stages or installs a driver.");

    let mut selected_identity = identity(target);
    selected_identity.keyboard_collection_instance_ids = vec![node[0].instance_id.clone()];
    let template = fs::read_to_string(&template_path)
        .map_err(|error| format!("could not read {}: {error}", template_path.display()))?;
    let generated = generate_test_machine_inf(
        &template,
        &InfGenerationRequest {
            target: &selected_identity,
            dennis: &identity(dennis_matches[0]),
            expected_container_id: container_id,
            target_match_count: targets.len(),
            present_physical_keyboard_count: keyboards.len(),
            exact_keyboard_tlc_hardware_id: hardware_id,
        },
    )?;
    if output.exists() {
        return Err("refusing to overwrite an existing INF".into());
    }
    fs::write(&output, generated)
        .map_err(|error| format!("could not write {}: {error}", output.display()))
}

fn assigned_keyboard<'a>(config: &'a ApplicationConfig, seat: &str) -> Result<&'a str, String> {
    config
        .seats
        .iter()
        .find(|candidate| {
            candidate.id.0.eq_ignore_ascii_case(seat) || candidate.name.eq_ignore_ascii_case(seat)
        })
        .and_then(|seat| seat.devices.keyboard.as_ref())
        .map(|PhysicalDeviceId(id)| id.as_str())
        .ok_or_else(|| format!("{seat} keyboard assignment is missing"))
}

fn resolve<'a>(
    keyboards: &'a [&'a PhysicalInputDevice],
    assignment: &str,
) -> Vec<&'a PhysicalInputDevice> {
    keyboards
        .iter()
        .copied()
        .filter(|device| matches_assignment(device, assignment))
        .collect()
}

fn matches_assignment(device: &PhysicalInputDevice, assignment: &str) -> bool {
    device.id.eq_ignore_ascii_case(assignment)
        || device
            .container_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case(assignment))
        || device.contains_logical_id(assignment)
}

fn identity(device: &PhysicalInputDevice) -> PhysicalKeyboardIdentity {
    PhysicalKeyboardIdentity {
        stable_physical_id: device.id.clone(),
        container_id: device.container_id.clone(),
        keyboard_collection_instance_ids: device
            .logical_nodes
            .iter()
            .filter(|node| node.device_type == InputDeviceType::Keyboard)
            .map(|node| node.instance_id.clone())
            .collect(),
        vendor_id: device.vendor_id.clone(),
        product_id: device.product_id.clone(),
    }
}

fn load_config(path: &PathBuf) -> Result<ApplicationConfig, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("could not parse {}: {error}", path.display()))
}

fn default_config_path() -> Option<PathBuf> {
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("se.multiseat.lite").join("config.json"))
}

fn windows_build_number() -> Option<u32> {
    let output = Command::new("reg.exe")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "/v",
            "CurrentBuildNumber",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .last()?
        .parse()
        .ok()
}

fn driver_service_installed() -> bool {
    Command::new("sc.exe")
        .args(["query", DRIVER_SERVICE])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn print_resolution(label: &str, assignment: &str, matches: &[&PhysicalInputDevice]) {
    println!("{label} assignment: {assignment}");
    println!("{label} physical matches: {}", matches.len());
    for device in matches {
        println!(
            "  physical={} container={}",
            device.id,
            device.container_id.as_deref().unwrap_or("unavailable")
        );
    }
}

fn print_keyboard_stacks(keyboards: &[&PhysicalInputDevice]) {
    println!("Physical keyboard stacks: {}", keyboards.len());
    for device in keyboards {
        println!(
            "  physical={} container={}",
            device.id,
            device.container_id.as_deref().unwrap_or("unavailable")
        );
        for node in device
            .logical_nodes
            .iter()
            .filter(|node| node.device_type == InputDeviceType::Keyboard)
        {
            println!("    TLC instance={}", node.instance_id);
            for hardware_id in &node.hardware_ids {
                println!("      hardware ID={hardware_id}");
            }
        }
    }
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}

fn required_option<'a>(args: &'a [String], name: &str) -> Result<&'a str, String> {
    option(args, name).ok_or_else(|| format!("missing required option {name}"))
}

fn usage() -> String {
    "usage: keyboard-filter-tools readiness [--config PATH] [--remote-recovery-confirmed]\n       keyboard-filter-tools generate-inf --target PHYSICAL_ID --container CONTAINER_ID --tlc INSTANCE_ID --hardware-id ID --output PATH [--config PATH] [--template PATH]\n       keyboard-filter-tools snapshot --output PATH [--config PATH]".into()
}
