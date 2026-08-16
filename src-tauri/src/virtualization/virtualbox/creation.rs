use std::{
    fmt,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::seats::ManagedWindowsVersion;

use super::{
    host_windows_install_defaults, parse_machine_readable, validate_windows_install_config,
    VirtualBoxRuntimeService, VirtualMachine, WindowsInstallConfig, WindowsInstallDefaults,
};

const WINDOWS_VRAM_MB: u32 = 128;
const ISO_PRIMARY_VOLUME_DESCRIPTOR: u64 = 32_769;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SecureBootStatus {
    NotRequired,
    WillConfigure,
    Configured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GuestAdditionsState {
    NotInstalled,
    InstallationPending,
    Installed,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestAdditionsPreparation {
    pub state: GuestAdditionsState,
    pub bundled_iso_path: Option<String>,
    pub automatic_installation_supported: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedWindowsVmProfile {
    pub windows_version: ManagedWindowsVersion,
    pub display_name: String,
    pub virtual_box_os_type_id: String,
    pub guest_os_description: String,
    pub architecture: String,
    pub firmware: String,
    pub tpm: String,
    pub tpm_required: bool,
    pub io_apic_enabled: bool,
    pub secure_boot: SecureBootStatus,
    pub secure_boot_required: bool,
    pub usb_controller: String,
    pub graphics_controller: String,
    pub vram_mb: u32,
    pub network: String,
    pub audio_output_enabled: bool,
    pub storage_controller: String,
    pub guest_additions: GuestAdditionsPreparation,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VmCreationRequest {
    pub name: String,
    pub iso_path: String,
    pub memory_mb: u64,
    pub cpus: u32,
    pub disk_gb: u64,
    pub windows_version: ManagedWindowsVersion,
    pub installation: WindowsInstallConfig,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VmCreationProposal {
    pub host_memory_mb: u64,
    pub host_logical_cpus: u32,
    pub memory_mb: u64,
    pub cpus: u32,
    pub disk_gb: u64,
    pub default_windows_version: ManagedWindowsVersion,
    pub profiles: Vec<ManagedWindowsVmProfile>,
    pub installation_defaults: WindowsInstallDefaults,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VmCreationLogEntry {
    pub step: String,
    pub outcome: String,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VmCreationResult {
    pub vm: VirtualMachine,
    pub profile: ManagedWindowsVmProfile,
    pub changed_settings: Vec<String>,
    pub logs: Vec<VmCreationLogEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VmCreationFailure {
    pub failed_step: String,
    pub message: String,
    pub logs: Vec<VmCreationLogEntry>,
    pub rollback_actions: Vec<VmCreationLogEntry>,
}

impl fmt::Display for VmCreationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "VM creation failed at '{}': {}",
            self.failed_step, self.message
        )?;
        if !self.logs.is_empty() {
            write!(formatter, "; creation log: ")?;
            for (index, entry) in self.logs.iter().enumerate() {
                if index > 0 {
                    write!(formatter, ", ")?;
                }
                write!(formatter, "{}={}", entry.step, entry.outcome)?;
            }
        }
        if !self.rollback_actions.is_empty() {
            write!(formatter, "; rollback: ")?;
            for (index, action) in self.rollback_actions.iter().enumerate() {
                if index > 0 {
                    write!(formatter, "; ")?;
                }
                write!(
                    formatter,
                    "{}={} ({})",
                    action.step, action.outcome, action.detail
                )?;
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct CreationTransaction {
    vm_identifier: Option<String>,
    vm_uuid: Option<String>,
    disk_path: Option<String>,
    disk_created: bool,
    disk_attached: bool,
    logs: Vec<VmCreationLogEntry>,
}

#[derive(Debug)]
struct StepFailure {
    step: String,
    message: String,
}

fn creation_proposal(
    host_memory_mb: u64,
    host_cpus: u32,
    profiles: Vec<ManagedWindowsVmProfile>,
) -> VmCreationProposal {
    let memory_mb = (host_memory_mb * 40 / 100)
        .clamp(4096, 16384)
        .min(host_memory_mb.saturating_sub(4096).max(2048));
    let cpus = (host_cpus / 2)
        .clamp(2, 8)
        .min(host_cpus.saturating_sub(1).max(1));
    VmCreationProposal {
        host_memory_mb,
        host_logical_cpus: host_cpus,
        memory_mb,
        cpus,
        disk_gb: 72,
        default_windows_version: ManagedWindowsVersion::Windows10,
        profiles,
        installation_defaults: host_windows_install_defaults(),
    }
}

fn windows_profile(
    windows_version: ManagedWindowsVersion,
    os_type: VirtualBoxOsType,
    secure_boot: SecureBootStatus,
) -> ManagedWindowsVmProfile {
    let windows_11 = windows_version == ManagedWindowsVersion::Windows11;
    ManagedWindowsVmProfile {
        windows_version,
        display_name: if windows_11 {
            "Windows 11 x64"
        } else {
            "Windows 10 x64"
        }
        .to_owned(),
        virtual_box_os_type_id: os_type.id,
        guest_os_description: os_type.description,
        architecture: "x86_64".to_owned(),
        firmware: if windows_11 { "UEFI" } else { "BIOS" }.to_owned(),
        tpm: if windows_11 {
            "2.0 (VirtualBox virtual TPM)"
        } else {
            "Not required"
        }
        .to_owned(),
        tpm_required: windows_11,
        io_apic_enabled: true,
        secure_boot,
        secure_boot_required: windows_11,
        usb_controller: "xHCI".to_owned(),
        graphics_controller: "VBoxSVGA".to_owned(),
        vram_mb: WINDOWS_VRAM_MB,
        network: "NAT".to_owned(),
        audio_output_enabled: true,
        storage_controller: "SATA (Intel AHCI)".to_owned(),
        guest_additions: GuestAdditionsPreparation {
            state: GuestAdditionsState::NotInstalled,
            bundled_iso_path: guest_additions_iso_path().map(|path| path.display().to_string()),
            automatic_installation_supported: true,
        },
    }
}

pub(crate) fn guest_additions_iso_path() -> Option<PathBuf> {
    super::ProcessVBoxManageExecutor::discover()
        .ok()
        .and_then(|executor| guest_additions_iso_next_to(&executor.path))
}

fn guest_additions_iso_next_to(vbox_manage_path: &Path) -> Option<PathBuf> {
    vbox_manage_path
        .parent()
        .map(|directory| directory.join("VBoxGuestAdditions.iso"))
        .filter(|path| path.is_file())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct VirtualBoxOsType {
    id: String,
    description: String,
    architecture: String,
}

fn parse_virtual_box_os_types(output: &str) -> Vec<VirtualBoxOsType> {
    let mut records = Vec::new();
    let mut current: Option<VirtualBoxOsType> = None;
    for line in output.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("ID / Description:") {
            if let Some(record) = current.take() {
                records.push(record);
            }
            let (id, description) = value
                .split_once("--")
                .map(|(id, description)| (id.trim(), description.trim()))
                .unwrap_or((value.trim(), ""));
            current = Some(VirtualBoxOsType {
                id: id.to_owned(),
                description: description.to_owned(),
                architecture: String::new(),
            });
        } else if let (Some(record), Some(value)) =
            (current.as_mut(), line.strip_prefix("Architecture:"))
        {
            record.architecture = value.trim().to_owned();
        }
    }
    if let Some(record) = current {
        records.push(record);
    }
    records
}

fn resolve_windows_os_type(
    records: &[VirtualBoxOsType],
    windows_version: ManagedWindowsVersion,
) -> Result<VirtualBoxOsType, String> {
    let version_name = match windows_version {
        ManagedWindowsVersion::Windows10 => "Windows 10",
        ManagedWindowsVersion::Windows11 => "Windows 11",
    };
    records
        .iter()
        .find(|record| {
            record.description.contains(version_name)
                && record.description.contains("64-bit")
                && record.architecture.contains("x86")
                && record.architecture.contains("64-bit")
        })
        .cloned()
        .ok_or_else(|| {
            format!("installed VirtualBox does not report a supported {version_name} x64 OS type")
        })
}

#[cfg(target_os = "windows")]
fn current_host_resources() -> Result<(u64, u32), String> {
    use ::windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut memory = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut memory) }
        .map_err(|error| format!("GlobalMemoryStatusEx failed: {error}"))?;
    let cpus = std::thread::available_parallelism()
        .map(|value| value.get() as u32)
        .map_err(|error| format!("could not query logical CPU count: {error}"))?;
    Ok((memory.ullTotalPhys / 1024 / 1024, cpus))
}

impl VirtualBoxRuntimeService {
    pub fn current_host_creation_proposal(&self) -> Result<VmCreationProposal, String> {
        let (host_memory_mb, host_cpus) = current_host_resources()?;
        Ok(creation_proposal(
            host_memory_mb,
            host_cpus,
            self.supported_windows_profiles()?,
        ))
    }

    fn supported_windows_profiles(&self) -> Result<Vec<ManagedWindowsVmProfile>, String> {
        let output = self.checked(&["list", "ostypes"])?;
        let records = parse_virtual_box_os_types(&output.stdout);
        Ok(vec![
            windows_profile(
                ManagedWindowsVersion::Windows10,
                resolve_windows_os_type(&records, ManagedWindowsVersion::Windows10)?,
                SecureBootStatus::NotRequired,
            ),
            windows_profile(
                ManagedWindowsVersion::Windows11,
                resolve_windows_os_type(&records, ManagedWindowsVersion::Windows11)?,
                SecureBootStatus::WillConfigure,
            ),
        ])
    }

    pub(crate) fn resolve_windows_profile(
        &self,
        windows_version: ManagedWindowsVersion,
    ) -> Result<ManagedWindowsVmProfile, String> {
        self.supported_windows_profiles()?
            .into_iter()
            .find(|profile| profile.windows_version == windows_version)
            .ok_or_else(|| "selected Windows profile is unavailable".to_owned())
    }

    pub fn create_vm(
        &self,
        request: &VmCreationRequest,
    ) -> Result<VmCreationResult, VmCreationFailure> {
        let (host_memory_mb, host_cpus) =
            current_host_resources().map_err(|message| VmCreationFailure {
                failed_step: "host-resource-validation".to_owned(),
                message,
                logs: Vec::new(),
                rollback_actions: Vec::new(),
            })?;
        self.create_vm_with_host_resources(request, host_memory_mb, host_cpus)
    }

    pub(crate) fn rollback_created_vm_after_persistence_failure(
        &self,
        vm_id: &str,
    ) -> Result<(), String> {
        self.checked(&["unregistervm", vm_id, "--delete"])
            .map(|_| ())
    }

    fn create_vm_with_host_resources(
        &self,
        request: &VmCreationRequest,
        host_memory_mb: u64,
        host_cpus: u32,
    ) -> Result<VmCreationResult, VmCreationFailure> {
        let mut transaction = CreationTransaction::default();
        match self.create_vm_steps(request, host_memory_mb, host_cpus, &mut transaction) {
            Ok(result) => Ok(result),
            Err(error) => {
                let rollback_actions = self.rollback_creation(&transaction);
                Err(VmCreationFailure {
                    failed_step: error.step,
                    message: error.message,
                    logs: transaction.logs,
                    rollback_actions,
                })
            }
        }
    }

    fn create_vm_steps(
        &self,
        request: &VmCreationRequest,
        host_memory_mb: u64,
        host_cpus: u32,
        transaction: &mut CreationTransaction,
    ) -> Result<VmCreationResult, StepFailure> {
        let vm_name = request.name.trim();
        if vm_name.is_empty() {
            return Err(step_error("validate-request", "VM name is required"));
        }
        validate_iso(Path::new(&request.iso_path))
            .map_err(|message| step_error("validate-windows-iso", message))?;
        validate_resources(request, host_memory_mb, host_cpus)
            .map_err(|message| step_error("validate-resources", message))?;
        validate_windows_install_config(&request.installation)
            .map_err(|message| step_error("validate-installation", message))?;
        if request.installation.install_guest_additions && guest_additions_iso_path().is_none() {
            return Err(step_error(
                "locate-guest-additions",
                "Guest Additions installation was requested but VBoxGuestAdditions.iso was not found",
            ));
        }
        let mut profile = self
            .resolve_windows_profile(request.windows_version)
            .map_err(|message| step_error("resolve-virtualbox-os-type", message))?;
        transaction.logs.push(success_log(
            "resolve-virtualbox-os-type",
            format!(
                "{} uses VirtualBox OS type {}",
                profile.display_name, profile.virtual_box_os_type_id
            ),
        ));
        transaction.logs.push(success_log(
            "validate-windows-iso",
            format!("readable ISO-9660 image: {}", request.iso_path),
        ));

        let existing = self
            .list_vms()
            .map_err(|message| step_error("check-existing-vms", message))?;
        if !existing.warnings.is_empty() {
            return Err(step_error(
                "check-existing-vms",
                format!(
                    "VM enumeration was incomplete, so collision safety cannot be proven: {}",
                    existing.warnings.join("; ")
                ),
            ));
        }
        if let Some(vm) = existing
            .machines
            .iter()
            .find(|vm| vm.name.eq_ignore_ascii_case(vm_name))
        {
            return Err(step_error(
                "check-existing-vms",
                format!(
                    "a VM named '{}' already exists with UUID {}; it will not be modified",
                    vm.name, vm.uuid
                ),
            ));
        }
        let existing_uuids: Vec<String> = existing
            .machines
            .iter()
            .map(|vm| vm.uuid.to_ascii_lowercase())
            .collect();
        transaction.logs.push(success_log(
            "check-existing-vms",
            "no name collision detected",
        ));

        let created = self.creation_step(
            transaction,
            "register-vm",
            &[
                "createvm",
                "--name",
                vm_name,
                "--ostype",
                &profile.virtual_box_os_type_id,
                "--platform-architecture",
                "x86",
                "--register",
            ],
        )?;
        // The name was proven unique immediately before registration, so it is a
        // safe rollback identifier even if VBoxManage omits the new UUID.
        transaction.vm_identifier = Some(vm_name.to_owned());
        let fields = parse_machine_readable(&created.stdout);
        let vm_id = fields
            .get("UUID")
            .cloned()
            .or_else(|| {
                created
                    .stdout
                    .lines()
                    .find_map(|line| line.split("UUID:").nth(1).map(str::trim).map(str::to_owned))
            })
            .ok_or_else(|| {
                step_error(
                    "parse-created-vm",
                    "VBoxManage createvm returned no VM UUID",
                )
            })?;
        if existing_uuids
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&vm_id))
        {
            // Do not delete by a colliding UUID. The unique name remains the only
            // rollback identifier that can safely denote this operation's VM.
            return Err(step_error(
                "validate-created-vm-uuid",
                format!("VirtualBox returned pre-existing VM UUID {vm_id}"),
            ));
        }
        transaction.vm_uuid = Some(vm_id.clone());
        transaction.vm_identifier = Some(vm_id.clone());

        let memory = request.memory_mb.to_string();
        let cpus = request.cpus.to_string();
        let firmware = if request.windows_version == ManagedWindowsVersion::Windows11 {
            "efi"
        } else {
            "bios"
        };
        let mut hardware_arguments = vec![
            "modifyvm",
            &vm_id,
            "--memory",
            &memory,
            "--cpus",
            &cpus,
            "--firmware",
            firmware,
        ];
        if profile.tpm_required {
            hardware_arguments.extend(["--tpm-type", "2.0"]);
        }
        hardware_arguments.extend([
            "--ioapic",
            "on",
            "--usb-xhci",
            "on",
            "--graphicscontroller",
            "vboxsvga",
            "--vram",
            "128",
            "--nic1",
            "nat",
            "--audio-enabled",
            "on",
            "--audio-in",
            "off",
            "--audio-out",
            "on",
            "--boot1",
            "dvd",
            "--boot2",
            "disk",
            "--boot3",
            "none",
            "--boot4",
            "none",
        ]);
        self.creation_step(
            transaction,
            "configure-managed-windows-hardware",
            &hardware_arguments,
        )?;

        if profile.secure_boot_required {
            for (step, arguments) in [
                (
                    "initialize-uefi-variable-store",
                    vec!["modifynvram", &vm_id, "inituefivarstore"],
                ),
                (
                    "enroll-microsoft-secure-boot-signatures",
                    vec!["modifynvram", &vm_id, "enrollmssignatures"],
                ),
                (
                    "enroll-virtualbox-platform-key",
                    vec!["modifynvram", &vm_id, "enrollorclpk"],
                ),
                (
                    "enable-secure-boot",
                    vec!["modifynvram", &vm_id, "secureboot", "--enable"],
                ),
            ] {
                self.creation_step(transaction, step, &arguments)?;
            }
            profile.secure_boot = SecureBootStatus::Configured;
        }

        let info = self.creation_step(
            transaction,
            "inspect-managed-vm-location",
            &["showvminfo", &vm_id, "--machinereadable"],
        )?;
        let info_fields = parse_machine_readable(&info.stdout);
        let config_file = info_fields.get("CfgFile").ok_or_else(|| {
            step_error(
                "inspect-managed-vm-location",
                "new VM has no configuration file path",
            )
        })?;
        let disk = Path::new(config_file)
            .parent()
            .ok_or_else(|| {
                step_error(
                    "derive-system-disk-path",
                    "VM configuration has no parent directory",
                )
            })?
            .join("Barnen-system.vdi");
        let disk_text = disk.display().to_string();
        transaction.disk_path = Some(disk_text.clone());
        let size_mb = request
            .disk_gb
            .checked_mul(1024)
            .ok_or_else(|| step_error("derive-system-disk-size", "disk size overflow"))?
            .to_string();
        self.creation_step(
            transaction,
            "create-system-disk",
            &[
                "createmedium",
                "disk",
                "--filename",
                &disk_text,
                "--size",
                &size_mb,
                "--format",
                "VDI",
            ],
        )?;
        transaction.disk_created = true;
        self.creation_step(
            transaction,
            "create-sata-controller",
            &[
                "storagectl",
                &vm_id,
                "--name",
                "SATA",
                "--add",
                "sata",
                "--controller",
                "IntelAhci",
            ],
        )?;
        self.creation_step(
            transaction,
            "attach-system-disk",
            &[
                "storageattach",
                &vm_id,
                "--storagectl",
                "SATA",
                "--port",
                "0",
                "--device",
                "0",
                "--type",
                "hdd",
                "--medium",
                &disk_text,
            ],
        )?;
        transaction.disk_attached = true;
        self.creation_step(
            transaction,
            "attach-windows-iso",
            &[
                "storageattach",
                &vm_id,
                "--storagectl",
                "SATA",
                "--port",
                "1",
                "--device",
                "0",
                "--type",
                "dvddrive",
                "--medium",
                &request.iso_path,
            ],
        )?;

        let vm = self
            .inspect_vm(&vm_id)
            .map_err(|message| step_error("verify-created-vm", message))?;
        Ok(VmCreationResult {
            vm,
            changed_settings: profile_settings(&profile),
            profile,
            logs: transaction.logs.clone(),
        })
    }

    fn creation_step(
        &self,
        transaction: &mut CreationTransaction,
        step: &str,
        arguments: &[&str],
    ) -> Result<super::VBoxOutput, StepFailure> {
        match self.checked(arguments) {
            Ok(output) => {
                transaction.logs.push(success_log(
                    step,
                    format!("VBoxManage accepted {} argument(s)", arguments.len()),
                ));
                Ok(output)
            }
            Err(message) => {
                transaction.logs.push(VmCreationLogEntry {
                    step: step.to_owned(),
                    outcome: "error".to_owned(),
                    detail: message.clone(),
                });
                Err(step_error(step, message))
            }
        }
    }

    fn rollback_creation(&self, transaction: &CreationTransaction) -> Vec<VmCreationLogEntry> {
        let mut actions = Vec::new();
        if transaction.disk_created && !transaction.disk_attached {
            if let Some(path) = transaction.disk_path.as_deref() {
                actions.push(self.rollback_step(
                    "delete-unattached-managed-disk",
                    &["closemedium", "disk", path, "--delete"],
                ));
            }
        }
        if let Some(identifier) = transaction.vm_identifier.as_deref() {
            actions.push(self.rollback_step(
                "delete-managed-vm",
                &["unregistervm", identifier, "--delete"],
            ));
        }
        actions
    }

    fn rollback_step(&self, step: &str, arguments: &[&str]) -> VmCreationLogEntry {
        match self.checked(arguments) {
            Ok(_) => success_log(
                step,
                "removed only resource owned by this creation operation",
            ),
            Err(message) => VmCreationLogEntry {
                step: step.to_owned(),
                outcome: "error".to_owned(),
                detail: message,
            },
        }
    }
}

fn validate_resources(
    request: &VmCreationRequest,
    host_memory_mb: u64,
    host_cpus: u32,
) -> Result<(), String> {
    if request.memory_mb < 2048 || request.cpus == 0 || request.disk_gb < 32 {
        return Err("VM resource values are below safe minimums".to_owned());
    }
    let maximum_memory = host_memory_mb.saturating_sub(4096);
    let maximum_cpus = host_cpus.saturating_sub(1).max(1);
    if request.memory_mb > maximum_memory || request.cpus > maximum_cpus {
        return Err(format!(
            "requested resources do not leave the host reserve (maximum {maximum_memory} MB and {maximum_cpus} CPUs)"
        ));
    }
    if request.disk_gb > 2048 {
        return Err("virtual disk size exceeds the 2048 GB safety limit".to_owned());
    }
    Ok(())
}

pub(crate) fn validate_iso(path: &Path) -> Result<(), String> {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("iso"))
    {
        return Err(format!("'{}' is not an .iso file", path.display()));
    }
    let mut file = File::open(path)
        .map_err(|error| format!("Windows ISO '{}' is not readable: {error}", path.display()))?;
    let metadata = file.metadata().map_err(|error| {
        format!(
            "could not inspect Windows ISO '{}': {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() || metadata.len() < ISO_PRIMARY_VOLUME_DESCRIPTOR + 5 {
        return Err(format!(
            "'{}' is too small to be a valid ISO image",
            path.display()
        ));
    }
    file.seek(SeekFrom::Start(ISO_PRIMARY_VOLUME_DESCRIPTOR))
        .map_err(|error| format!("could not seek Windows ISO '{}': {error}", path.display()))?;
    let mut signature = [0_u8; 5];
    file.read_exact(&mut signature)
        .map_err(|error| format!("could not read Windows ISO '{}': {error}", path.display()))?;
    if &signature != b"CD001" {
        return Err(format!(
            "'{}' does not contain an ISO-9660 primary volume descriptor",
            path.display()
        ));
    }
    Ok(())
}

fn profile_settings(profile: &ManagedWindowsVmProfile) -> Vec<String> {
    let mut settings = vec![
        format!("Managed guest: {}", profile.display_name),
        format!("VirtualBox OS type ID: {}", profile.virtual_box_os_type_id),
        format!("Guest OS description: {}", profile.guest_os_description),
        format!("Architecture: {}", profile.architecture),
        format!("Firmware: {}", profile.firmware),
        format!("TPM: {}", profile.tpm),
        "I/O APIC: enabled".to_owned(),
        if profile.secure_boot_required {
            "Secure Boot: Microsoft signatures + Oracle platform key".to_owned()
        } else {
            "Secure Boot: not required".to_owned()
        },
        format!("USB: {}", profile.usb_controller),
        format!(
            "Graphics: {} / {} MB VRAM",
            profile.graphics_controller, profile.vram_mb
        ),
        format!("Network: {}", profile.network),
        "Audio output: enabled".to_owned(),
        format!("Storage: {}", profile.storage_controller),
    ];
    settings.shrink_to_fit();
    settings
}

fn success_log(step: &str, detail: impl Into<String>) -> VmCreationLogEntry {
    VmCreationLogEntry {
        step: step.to_owned(),
        outcome: "success".to_owned(),
        detail: detail.into(),
    }
}

fn step_error(step: &str, message: impl Into<String>) -> StepFailure {
    StepFailure {
        step: step.to_owned(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, OpenOptions},
        io::{Seek, SeekFrom, Write},
        sync::{Arc, Mutex},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;
    use crate::virtualization::{VBoxManageExecutor, VBoxOutput};

    const OS_TYPES: &str = "ID / Description: Win10-Test-ID -- Windows 10 (64-bit)\nFamily: Windows (Microsoft Windows)\nArchitecture: x86 (64-bit)\n\nID / Description: Win11-Test-ID -- Windows 11 (64-bit)\nFamily: Windows (Microsoft Windows)\nArchitecture: x86 (64-bit)\n";

    #[derive(Clone)]
    struct CreationExecutor {
        calls: Arc<Mutex<Vec<Vec<String>>>>,
        existing_vm: bool,
        uuid_collision: bool,
        fail_step_command: Option<String>,
    }

    impl VBoxManageExecutor for CreationExecutor {
        fn execute(&self, arguments: &[String]) -> Result<VBoxOutput, String> {
            self.calls.lock().unwrap().push(arguments.to_vec());
            if self
                .fail_step_command
                .as_deref()
                .is_some_and(|command| arguments.first().is_some_and(|value| value == command))
            {
                return Ok(VBoxOutput {
                    success: false,
                    exit_code: Some(1),
                    stdout: String::new(),
                    stderr: "injected failure".to_owned(),
                });
            }
            let mut stdout = match arguments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                ["list", "ostypes"] => OS_TYPES.to_owned(),
                ["list", "vms"] if self.existing_vm => "\"Windows - Barnen\" {existing-uuid}\n".to_owned(),
                ["list", "vms"] if self.uuid_collision => "\"Unrelated VM\" {managed-vm-uuid}\n".to_owned(),
                ["list", "vms"] => String::new(),
                ["createvm", ..] => "UUID: managed-vm-uuid\n".to_owned(),
                ["showvminfo", "existing-uuid", "--machinereadable"] => "name=\"Windows - Barnen\"\nUUID=\"existing-uuid\"\nVMState=\"poweroff\"\nostype=\"Windows11_64\"\nxhci=\"on\"\n".to_owned(),
                ["showvminfo", "managed-vm-uuid", "--machinereadable"] => String::new(),
                _ => String::new(),
            };
            if arguments.len() == 3
                && arguments[0] == "showvminfo"
                && arguments[1] == "managed-vm-uuid"
                && arguments[2] == "--machinereadable"
            {
                let os_type = self
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .rev()
                    .find_map(|call| {
                        (call.first().is_some_and(|value| value == "createvm"))
                            .then(|| call.iter().position(|value| value == "--ostype"))
                            .flatten()
                            .and_then(|index| call.get(index + 1))
                            .cloned()
                    })
                    .unwrap_or_else(|| "Win10-Test-ID".to_owned());
                stdout = format!("name=\"Windows - Barnen\"\nUUID=\"managed-vm-uuid\"\nVMState=\"poweroff\"\nostype=\"{os_type}\"\nxhci=\"on\"\nCfgFile=\"C:\\VirtualBox VMs\\Windows - Barnen\\Windows - Barnen.vbox\"\n");
            }
            Ok(VBoxOutput {
                success: true,
                exit_code: Some(0),
                stdout,
                stderr: String::new(),
            })
        }

        fn wait(&self, _duration: std::time::Duration) {}
    }

    fn valid_iso(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("multiseat-{label}-{unique} with spaces"));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("Windows 11 installer.iso");
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.seek(SeekFrom::Start(ISO_PRIMARY_VOLUME_DESCRIPTOR))
            .unwrap();
        file.write_all(b"CD001").unwrap();
        path
    }

    fn request(path: &Path) -> VmCreationRequest {
        VmCreationRequest {
            name: "Windows - Barnen".into(),
            iso_path: path.display().to_string(),
            memory_mb: 8192,
            cpus: 4,
            disk_gb: 72,
            windows_version: ManagedWindowsVersion::Windows10,
            installation: WindowsInstallConfig {
                username: "Barnen".into(),
                password: "test-password".into(),
                computer_name: "BARNEN-PC".into(),
                domain_name: "multiseat.local".into(),
                locale: "sv_SE".into(),
                language: "sv".into(),
                country: "SE".into(),
                time_zone: String::new(),
                product_key: None,
                install_guest_additions: false,
            },
        }
    }

    fn service(executor: &CreationExecutor) -> VirtualBoxRuntimeService {
        VirtualBoxRuntimeService::new(Box::new(executor.clone()))
    }

    #[test]
    fn windows_profiles_preserve_distinct_security_requirements() {
        let records = parse_virtual_box_os_types(OS_TYPES);
        let profiles = vec![
            windows_profile(
                ManagedWindowsVersion::Windows10,
                resolve_windows_os_type(&records, ManagedWindowsVersion::Windows10).unwrap(),
                SecureBootStatus::NotRequired,
            ),
            windows_profile(
                ManagedWindowsVersion::Windows11,
                resolve_windows_os_type(&records, ManagedWindowsVersion::Windows11).unwrap(),
                SecureBootStatus::WillConfigure,
            ),
        ];
        let proposal = creation_proposal(32_768, 16, profiles);
        assert_eq!(
            proposal.default_windows_version,
            ManagedWindowsVersion::Windows10
        );
        let windows_10 = &proposal.profiles[0];
        assert_eq!(windows_10.virtual_box_os_type_id, "Win10-Test-ID");
        assert_eq!(windows_10.guest_os_description, "Windows 10 (64-bit)");
        assert_eq!(windows_10.firmware, "BIOS");
        assert!(!windows_10.tpm_required);
        assert!(!windows_10.secure_boot_required);
        let windows_11 = &proposal.profiles[1];
        assert_eq!(windows_11.firmware, "UEFI");
        assert_eq!(windows_11.guest_os_description, "Windows 11 (64-bit)");
        assert_eq!(windows_11.tpm, "2.0 (VirtualBox virtual TPM)");
        assert!(windows_11.tpm_required);
        assert!(windows_11.secure_boot_required);
        assert_eq!(windows_11.usb_controller, "xHCI");
        assert!(windows_11.io_apic_enabled);
        assert_eq!(
            windows_11.guest_additions.state,
            GuestAdditionsState::NotInstalled
        );
        assert!(windows_11.guest_additions.automatic_installation_supported);
    }

    #[test]
    fn discovers_guest_additions_iso_next_to_vboxmanage() {
        let fixture = valid_iso("guest-additions-discovery");
        let directory = fixture.parent().unwrap().to_path_buf();
        let executable = directory.join("VBoxManage.exe");
        let additions = directory.join("VBoxGuestAdditions.iso");
        fs::write(&executable, []).unwrap();
        fs::write(&additions, []).unwrap();
        assert_eq!(guest_additions_iso_next_to(&executable), Some(additions));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn creation_uses_argument_safe_paths_with_spaces_and_complete_profile() {
        let iso = valid_iso("arguments");
        let executor = CreationExecutor {
            calls: Arc::new(Mutex::new(Vec::new())),
            existing_vm: false,
            uuid_collision: false,
            fail_step_command: None,
        };
        let mut creation_request = request(&iso);
        creation_request.windows_version = ManagedWindowsVersion::Windows11;
        let result = service(&executor)
            .create_vm_with_host_resources(&creation_request, 32_768, 16)
            .unwrap();
        let calls = executor.calls.lock().unwrap();
        let iso_argument = iso.display().to_string();
        assert!(calls
            .iter()
            .any(|arguments| arguments.iter().any(|argument| argument == &iso_argument)));
        let modify = calls
            .iter()
            .find(|arguments| arguments.first().is_some_and(|value| value == "modifyvm"))
            .unwrap();
        for pair in [
            ["--firmware", "efi"],
            ["--tpm-type", "2.0"],
            ["--ioapic", "on"],
            ["--usb-xhci", "on"],
            ["--graphicscontroller", "vboxsvga"],
        ] {
            assert!(modify.windows(2).any(|window| window == pair));
        }
        assert_eq!(result.profile.secure_boot, SecureBootStatus::Configured);
        fs::remove_dir_all(iso.parent().unwrap()).unwrap();
    }

    #[test]
    fn secure_boot_enrollment_is_explicit_and_ordered() {
        let iso = valid_iso("secureboot");
        let executor = CreationExecutor {
            calls: Arc::new(Mutex::new(Vec::new())),
            existing_vm: false,
            uuid_collision: false,
            fail_step_command: None,
        };
        let mut creation_request = request(&iso);
        creation_request.windows_version = ManagedWindowsVersion::Windows11;
        service(&executor)
            .create_vm_with_host_resources(&creation_request, 32_768, 16)
            .unwrap();
        let calls = executor.calls.lock().unwrap();
        let operations: Vec<_> = calls
            .iter()
            .filter(|args| args.first().is_some_and(|value| value == "modifynvram"))
            .map(|args| args[2].clone())
            .collect();
        assert_eq!(
            operations,
            vec![
                "inituefivarstore",
                "enrollmssignatures",
                "enrollorclpk",
                "secureboot"
            ]
        );
        fs::remove_dir_all(iso.parent().unwrap()).unwrap();
    }

    #[test]
    fn windows_10_creation_omits_tpm_and_secure_boot_commands() {
        let iso = valid_iso("windows-10-security");
        let executor = CreationExecutor {
            calls: Arc::new(Mutex::new(Vec::new())),
            existing_vm: false,
            uuid_collision: false,
            fail_step_command: None,
        };
        let result = service(&executor)
            .create_vm_with_host_resources(&request(&iso), 32_768, 16)
            .unwrap();
        let calls = executor.calls.lock().unwrap();
        let modify = calls
            .iter()
            .find(|arguments| arguments.first().is_some_and(|value| value == "modifyvm"))
            .unwrap();
        assert!(modify
            .windows(2)
            .any(|window| window == ["--firmware", "bios"]));
        assert!(!modify.iter().any(|argument| argument == "--tpm-type"));
        assert!(!calls.iter().any(|arguments| arguments
            .first()
            .is_some_and(|value| value == "modifynvram")));
        assert_eq!(
            result.profile.windows_version,
            ManagedWindowsVersion::Windows10
        );
        assert_eq!(result.profile.secure_boot, SecureBootStatus::NotRequired);
        fs::remove_dir_all(iso.parent().unwrap()).unwrap();
    }

    #[test]
    fn failed_creation_rolls_back_only_owned_vm_and_disk() {
        let iso = valid_iso("rollback");
        let executor = CreationExecutor {
            calls: Arc::new(Mutex::new(Vec::new())),
            existing_vm: false,
            uuid_collision: false,
            fail_step_command: Some("storagectl".into()),
        };
        let failure = service(&executor)
            .create_vm_with_host_resources(&request(&iso), 32_768, 16)
            .unwrap_err();
        assert_eq!(failure.failed_step, "create-sata-controller");
        let calls = executor.calls.lock().unwrap();
        assert!(calls.iter().any(|args| {
            args.first().is_some_and(|value| value == "closemedium")
                && args
                    .iter()
                    .any(|value| value == "C:\\VirtualBox VMs\\Windows - Barnen\\Barnen-system.vdi")
        }));
        assert!(calls
            .iter()
            .any(|args| args.as_slice() == ["unregistervm", "managed-vm-uuid", "--delete"]));
        assert!(!calls
            .iter()
            .any(|args| args.iter().any(|value| value == "existing-uuid")));
        assert!(!calls
            .iter()
            .any(|args| args.first().is_some_and(|value| value == "unattended")));
        fs::remove_dir_all(iso.parent().unwrap()).unwrap();
    }

    #[test]
    fn existing_vm_with_same_name_is_never_modified() {
        let iso = valid_iso("collision");
        let executor = CreationExecutor {
            calls: Arc::new(Mutex::new(Vec::new())),
            existing_vm: true,
            uuid_collision: false,
            fail_step_command: None,
        };
        let failure = service(&executor)
            .create_vm_with_host_resources(&request(&iso), 32_768, 16)
            .unwrap_err();
        assert_eq!(failure.failed_step, "check-existing-vms");
        let calls = executor.calls.lock().unwrap();
        assert!(!calls
            .iter()
            .any(|args| args.first().is_some_and(|value| value == "createvm"
                || value == "modifyvm"
                || value == "unregistervm")));
        fs::remove_dir_all(iso.parent().unwrap()).unwrap();
    }

    #[test]
    fn colliding_created_uuid_never_targets_the_existing_uuid_for_rollback() {
        let iso = valid_iso("uuid-collision");
        let executor = CreationExecutor {
            calls: Arc::new(Mutex::new(Vec::new())),
            existing_vm: false,
            uuid_collision: true,
            fail_step_command: None,
        };
        let failure = service(&executor)
            .create_vm_with_host_resources(&request(&iso), 32_768, 16)
            .unwrap_err();
        assert_eq!(failure.failed_step, "validate-created-vm-uuid");
        let calls = executor.calls.lock().unwrap();
        assert!(calls
            .iter()
            .any(|args| { args.as_slice() == ["unregistervm", "Windows - Barnen", "--delete"] }));
        assert!(!calls.iter().any(|args| {
            args.first().is_some_and(|value| value == "unregistervm")
                && args.iter().any(|value| value == "managed-vm-uuid")
        }));
        fs::remove_dir_all(iso.parent().unwrap()).unwrap();
    }

    #[test]
    fn invalid_or_unreadable_iso_is_rejected_before_vboxmanage() {
        let path = std::env::temp_dir().join("not-a-windows-image.txt");
        let executor = CreationExecutor {
            calls: Arc::new(Mutex::new(Vec::new())),
            existing_vm: false,
            uuid_collision: false,
            fail_step_command: None,
        };
        let failure = service(&executor)
            .create_vm_with_host_resources(&request(&path), 32_768, 16)
            .unwrap_err();
        assert_eq!(failure.failed_step, "validate-windows-iso");
        assert!(executor.calls.lock().unwrap().is_empty());
    }
}
