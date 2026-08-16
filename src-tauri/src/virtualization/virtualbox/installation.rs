use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::seats::{
    GuestAdditionsInstallState, GuestAdditionsStatus, GuestRunLevel, GuestRunLevelProbe,
    ManagedInstallState, ManagedInstallationEvidence, ManagedInstallationObservation,
    ManagedVmInstallation, ManagedVmStatus, WindowsInstallationStatus,
};

use super::{
    parse_machine_readable, validate_iso, ManagedWindowsVmProfile, VirtualBoxRuntimeService,
    VmCreationResult,
};

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsInstallConfig {
    pub username: String,
    pub password: String,
    pub computer_name: String,
    #[serde(default = "default_domain_name")]
    pub domain_name: String,
    pub locale: String,
    pub language: String,
    pub country: String,
    pub time_zone: String,
    pub product_key: Option<String>,
    pub install_guest_additions: bool,
}

// Deliberately excludes secrets if this type is accidentally added to a log.
impl std::fmt::Debug for WindowsInstallConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowsInstallConfig")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("computer_name", &self.computer_name)
            .field("domain_name", &self.domain_name)
            .field("locale", &self.locale)
            .field("language", &self.language)
            .field("country", &self.country)
            .field("time_zone", &self.time_zone)
            .field(
                "product_key",
                &self.product_key.as_ref().map(|_| "<redacted>"),
            )
            .field("install_guest_additions", &self.install_guest_additions)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsInstallDefaults {
    pub username: String,
    pub computer_name: String,
    pub domain_name: String,
    pub hostname: String,
    pub locale: String,
    pub language: String,
    pub country: String,
    pub time_zone: String,
    pub install_guest_additions: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedVmSetupResult {
    pub creation: VmCreationResult,
    pub installation: ManagedVmInstallation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedInstallationSnapshot {
    pub vm_id: String,
    pub vm_state: String,
    pub installation: ManagedVmInstallation,
    pub windows_guest_detected: bool,
    pub monitoring_complete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedVmRecoveryStatus {
    pub can_resume: bool,
    pub classification: ManagedVmRecoveryClassification,
    pub reason: Option<String>,
    pub previous_failure: Option<String>,
    pub previous_problem: Option<String>,
    pub firmware_virtualization: crate::virtualization::DetectionState,
    pub virtual_box_execution: crate::virtualization::DetectionState,
    pub virtualization_ready: bool,
    pub virtualization_blocker: Option<String>,
    pub vm_configuration_valid: bool,
    pub unattended_preparation_present: bool,
    pub unattended_media_path: Option<String>,
    pub vm_id: String,
    pub vm_name: Option<String>,
    pub vm_state: Option<String>,
    pub windows_version: crate::seats::ManagedWindowsVersion,
    pub virtual_box_os_type_id: String,
    pub virtual_box_os_description: Option<String>,
    pub system_disk_path: Option<String>,
    pub iso_path: Option<String>,
    pub iso_medium_uuid: Option<String>,
    pub credentials_required: bool,
    pub suggested_computer_name: String,
    pub suggested_domain_name: String,
    pub suggested_hostname: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ManagedVmRecoveryClassification {
    RecoverablePreBoot,
    RecoverablePreparedInstall,
    InstallationInProgress,
    Ready,
    Unrecoverable,
    Unknown,
}

struct ManagedVmRecoveryPlan {
    status: ManagedVmRecoveryStatus,
    profile: ManagedWindowsVmProfile,
}

#[derive(Clone, Debug)]
enum RecoveryMedia {
    OriginalIso {
        iso_path: String,
    },
    Prepared {
        iso_path: String,
        unattended_media_path: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationFailure {
    pub failed_step: String,
    pub message: String,
}

impl std::fmt::Display for InstallationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Windows installation failed at '{}': {}",
            self.failed_step, self.message
        )
    }
}

pub fn host_windows_install_defaults() -> WindowsInstallDefaults {
    let locale = host_locale_name().unwrap_or_else(|| "en-US".to_owned());
    let mut pieces = locale.split(['-', '_']);
    let language = pieces.next().unwrap_or("en").to_ascii_lowercase();
    let country = pieces.next().unwrap_or("US").to_ascii_uppercase();
    let computer_name = "BARNEN-PC".to_owned();
    let domain_name = default_domain_name();
    let hostname = unattended_hostname(&computer_name, &domain_name)
        .unwrap_or_else(|_| "barnen-pc.multiseat.local".to_owned());
    WindowsInstallDefaults {
        username: "Barnen".to_owned(),
        computer_name,
        domain_name,
        hostname,
        locale: format!("{language}_{country}"),
        language,
        country,
        // An empty value tells VBoxManage to inherit the host time zone.
        time_zone: String::new(),
        install_guest_additions: true,
    }
}

fn default_domain_name() -> String {
    "multiseat.local".to_owned()
}

pub fn apply_legacy_managed_install_defaults(config: &mut WindowsInstallConfig) -> bool {
    migrate_managed_hostname_parts(&mut config.computer_name, &mut config.domain_name)
}

fn migrate_managed_hostname_parts(computer_name: &mut String, domain_name: &mut String) -> bool {
    let mut changed = false;
    if computer_name.trim().is_empty()
        || computer_name
            .trim()
            .eq_ignore_ascii_case("MULTISEAT-BARNEN")
    {
        *computer_name = "BARNEN-PC".to_owned();
        changed = true;
    }
    if domain_name.trim().is_empty() {
        *domain_name = default_domain_name();
        changed = true;
    }
    changed
}

pub fn unattended_hostname(computer_name: &str, domain_name: &str) -> Result<String, String> {
    validate_computer_name(computer_name)?;
    let domain = domain_name.trim().to_ascii_lowercase();
    if domain.is_empty() {
        return Err("DNS domain is required for VirtualBox unattended setup".to_owned());
    }
    for label in domain.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err("DNS domain contains an empty or invalid label".to_owned());
        }
    }
    let hostname = format!("{}.{}", computer_name.trim().to_ascii_lowercase(), domain);
    if hostname.len() > 253 || !hostname.contains('.') {
        return Err("generated unattended hostname is not a valid FQDN".to_owned());
    }
    Ok(hostname)
}

fn validate_computer_name(computer_name: &str) -> Result<(), String> {
    let computer = computer_name.trim();
    if computer.is_empty()
        || computer.len() > 15
        || computer.starts_with('-')
        || computer.ends_with('-')
        || computer.contains('.')
        || !computer
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err(
            "Windows computer name must be 1-15 ASCII letters, digits, or hyphens; it cannot contain dots or begin/end with a hyphen"
                .to_owned(),
        );
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn host_locale_name() -> Option<String> {
    use ::windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buffer = [0_u16; 85];
    let length = unsafe { GetUserDefaultLocaleName(&mut buffer) };
    (length > 1).then(|| String::from_utf16_lossy(&buffer[..length as usize - 1]))
}

#[cfg(not(target_os = "windows"))]
fn host_locale_name() -> Option<String> {
    None
}

pub fn validate_windows_install_config(config: &WindowsInstallConfig) -> Result<(), String> {
    let username = config.username.trim();
    if username.is_empty()
        || username.encode_utf16().count() > 20
        || username == "."
        || username == ".."
        || username
            .chars()
            .any(|character| character.is_control() || "\"/\\[]:;|=,+*?<>".contains(character))
    {
        return Err(
            "Windows username is empty, too long, or contains invalid characters".to_owned(),
        );
    }
    if config.password.is_empty()
        || config.password.chars().count() > 127
        || config.password.chars().any(char::is_control)
    {
        return Err("Windows password must contain 1-127 printable characters".to_owned());
    }
    unattended_hostname(&config.computer_name, &config.domain_name)?;
    if !valid_locale(&config.locale)
        || !(2..=3).contains(&config.language.len())
        || !config
            .language
            .chars()
            .all(|character| character.is_ascii_alphabetic())
        || config.country.len() != 2
        || !config
            .country
            .chars()
            .all(|character| character.is_ascii_alphabetic())
    {
        return Err("locale, language, or country has an invalid format".to_owned());
    }
    if config.time_zone.chars().any(char::is_control) {
        return Err("time zone contains invalid control characters".to_owned());
    }
    if let Some(key) = config
        .product_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
    {
        let groups: Vec<&str> = key.split('-').collect();
        if groups.len() != 5
            || groups.iter().any(|group| {
                group.len() != 5
                    || !group
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric())
            })
        {
            return Err(
                "Windows product key must contain five groups of five characters".to_owned(),
            );
        }
    }
    Ok(())
}

fn valid_locale(locale: &str) -> bool {
    let pieces: Vec<&str> = locale.split('_').collect();
    pieces.len() == 2
        && (2..=3).contains(&pieces[0].len())
        && pieces[0]
            .chars()
            .all(|character| character.is_ascii_alphabetic())
        && pieces[1].len() == 2
        && pieces[1]
            .chars()
            .all(|character| character.is_ascii_alphabetic())
}

fn unattended_arguments(
    vm_id: &str,
    iso_path: &str,
    password_file: &Path,
    additions_iso: Option<&Path>,
    config: &WindowsInstallConfig,
) -> Result<(Vec<String>, Vec<String>), String> {
    let hostname = unattended_hostname(&config.computer_name, &config.domain_name)?;
    let mut arguments = vec![
        "unattended".to_owned(),
        "install".to_owned(),
        vm_id.to_owned(),
        format!("--iso={iso_path}"),
        format!("--user={}", config.username.trim()),
        format!("--user-password-file={}", password_file.display()),
        format!("--full-user-name={}", config.username.trim()),
        format!("--hostname={hostname}"),
        format!("--locale={}", config.locale),
        format!("--language={}", config.language),
        format!("--country={}", config.country),
    ];
    if !config.time_zone.trim().is_empty() {
        arguments.push(format!("--time-zone={}", config.time_zone.trim()));
    }
    let mut sensitive = Vec::new();
    if let Some(key) = config
        .product_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
    {
        let argument = format!("--key={key}");
        sensitive.push(argument.clone());
        arguments.push(argument);
    }
    if config.install_guest_additions {
        arguments.push("--install-additions".to_owned());
        if let Some(path) = additions_iso {
            arguments.push(format!("--additions-iso={}", path.display()));
        }
    }
    arguments.push("--start-vm=gui".to_owned());
    Ok((arguments, sensitive))
}

fn parse_host_virtualization_support(output: &str) -> crate::virtualization::DetectionState {
    output
        .lines()
        .find_map(|line| {
            let (label, value) = line.split_once(':')?;
            label
                .trim()
                .eq_ignore_ascii_case("Processor supports HW virtualization")
                .then(|| match value.trim().to_ascii_lowercase().as_str() {
                    "yes" => crate::virtualization::DetectionState::Yes,
                    "no" => crate::virtualization::DetectionState::No,
                    _ => crate::virtualization::DetectionState::Unknown,
                })
        })
        .unwrap_or(crate::virtualization::DetectionState::Unknown)
}

fn inspect_recovery_media(vm_id: &str, attached_path: &str) -> Result<RecoveryMedia, String> {
    let attached = Path::new(attached_path);
    if attached
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("iso"))
    {
        validate_iso(attached)?;
        return Ok(RecoveryMedia::OriginalIso {
            iso_path: attached_path.to_owned(),
        });
    }
    if !attached
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("viso"))
    {
        return Err(format!(
            "attached installation medium '{}' is neither an ISO nor a managed unattended VISO",
            attached_path
        ));
    }
    let expected_name = format!("Unattended-{vm_id}-aux-iso.viso");
    if attached
        .file_name()
        .and_then(|name| name.to_str())
        .is_none_or(|name| !name.eq_ignore_ascii_case(&expected_name))
    {
        return Err("attached unattended media does not belong to the managed VM UUID".to_owned());
    }
    let definition = fs::read_to_string(attached).map_err(|error| {
        format!(
            "could not inspect managed unattended medium '{}': {error}",
            attached.display()
        )
    })?;
    let iso_path = imported_iso_path(&definition)
        .ok_or_else(|| "managed unattended VISO does not identify its Windows ISO".to_owned())?;
    validate_iso(Path::new(&iso_path))?;

    let parent = attached
        .parent()
        .ok_or_else(|| "managed unattended VISO has no parent directory".to_owned())?;
    for suffix in ["autounattend.xml", "VBOXPOST.CMD"] {
        let artifact = parent.join(format!("Unattended-{vm_id}-{suffix}"));
        if !artifact.is_file() {
            return Err(format!(
                "managed unattended preparation is incomplete: '{}' is missing",
                artifact.display()
            ));
        }
    }
    Ok(RecoveryMedia::Prepared {
        iso_path,
        unattended_media_path: attached_path.to_owned(),
    })
}

fn imported_iso_path(definition: &str) -> Option<String> {
    let remainder = definition
        .split_once("--import-iso-skip-eltorito")?
        .1
        .trim_start();
    let quote = remainder.chars().next()?;
    if quote == '\'' || quote == '"' {
        let quoted = &remainder[quote.len_utf8()..];
        return quoted.split_once(quote).map(|(path, _)| path.to_owned());
    }
    remainder.split_whitespace().next().map(str::to_owned)
}

fn historical_virtualization_failure(
    journal_error: Option<&str>,
    fields: &std::collections::HashMap<String, String>,
) -> Option<String> {
    let mut evidence = journal_error.unwrap_or_default().to_owned();
    if let Some(log_folder) = fields.get("LogFldr") {
        if let Ok(log) = fs::read_to_string(Path::new(log_folder).join("VBox.log")) {
            evidence.push('\n');
            evidence.push_str(&log);
        }
    }
    let codes: Vec<_> = ["VERR_NEM_NOT_AVAILABLE", "VERR_SVM_DISABLED"]
        .into_iter()
        .filter(|code| evidence.contains(code))
        .collect();
    (!codes.is_empty()).then(|| codes.join("; "))
}

fn previous_problem(previous_failure: Option<&str>, vm_state: Option<&str>) -> Option<String> {
    if previous_failure.is_some_and(|failure| {
        failure.contains("VERR_SVM_DISABLED") || failure.contains("VERR_NEM_NOT_AVAILABLE")
    }) {
        Some("Hardware virtualization was unavailable".to_owned())
    } else if previous_failure.is_some() || vm_state.is_some_and(|state| state == "aborted") {
        Some("The previous installation attempt was interrupted".to_owned())
    } else {
        None
    }
}

fn recovery_can_resume(
    classification: ManagedVmRecoveryClassification,
    firmware: crate::virtualization::DetectionState,
    virtual_box_execution: crate::virtualization::DetectionState,
) -> bool {
    matches!(
        classification,
        ManagedVmRecoveryClassification::RecoverablePreBoot
            | ManagedVmRecoveryClassification::RecoverablePreparedInstall
    ) && firmware == crate::virtualization::DetectionState::Yes
        && virtual_box_execution == crate::virtualization::DetectionState::Yes
}

impl VirtualBoxRuntimeService {
    pub fn managed_vm_recovery_status(
        &self,
        configured_vm_id: &str,
        managed_by_multiseat: bool,
        journal: &ManagedVmInstallation,
    ) -> ManagedVmRecoveryStatus {
        let defaults = host_windows_install_defaults();
        let mut suggested_computer_name = journal.computer_name.clone();
        let mut suggested_domain_name = journal.domain_name.clone();
        migrate_managed_hostname_parts(&mut suggested_computer_name, &mut suggested_domain_name);
        let suggested_hostname =
            unattended_hostname(&suggested_computer_name, &suggested_domain_name)
                .unwrap_or_else(|_| defaults.hostname.clone());
        let (firmware_virtualization, virtual_box_execution, virtualization_blocker) =
            self.virtualization_readiness();
        let virtualization_ready = firmware_virtualization
            == crate::virtualization::DetectionState::Yes
            && virtual_box_execution == crate::virtualization::DetectionState::Yes;
        match self.validate_managed_vm_recovery(configured_vm_id, managed_by_multiseat, journal) {
            Ok(mut plan) => {
                plan.status.firmware_virtualization = firmware_virtualization;
                plan.status.virtual_box_execution = virtual_box_execution;
                plan.status.virtualization_ready = virtualization_ready;
                plan.status.virtualization_blocker = virtualization_blocker;
                plan.status.can_resume = recovery_can_resume(
                    plan.status.classification,
                    firmware_virtualization,
                    virtual_box_execution,
                );
                if !virtualization_ready && plan.status.reason.is_none() {
                    plan.status.reason = plan.status.virtualization_blocker.clone();
                }
                plan.status.suggested_computer_name = suggested_computer_name;
                plan.status.suggested_domain_name = suggested_domain_name;
                plan.status.suggested_hostname = suggested_hostname;
                plan.status
            }
            Err(reason) => ManagedVmRecoveryStatus {
                can_resume: false,
                classification: match journal.state {
                    ManagedInstallState::Ready => ManagedVmRecoveryClassification::Ready,
                    ManagedInstallState::InstallingWindows
                    | ManagedInstallState::Rebooting
                    | ManagedInstallState::InstallingGuestAdditions
                    | ManagedInstallState::WaitingForGuest => {
                        ManagedVmRecoveryClassification::InstallationInProgress
                    }
                    ManagedInstallState::NotCreated | ManagedInstallState::Creating => {
                        ManagedVmRecoveryClassification::Unknown
                    }
                    ManagedInstallState::Failed => ManagedVmRecoveryClassification::Unrecoverable,
                },
                reason: Some(reason),
                previous_failure: journal.last_error.clone(),
                previous_problem: previous_problem(journal.last_error.as_deref(), None),
                firmware_virtualization,
                virtual_box_execution,
                virtualization_ready,
                virtualization_blocker,
                vm_configuration_valid: false,
                unattended_preparation_present: false,
                unattended_media_path: None,
                vm_id: configured_vm_id.to_owned(),
                vm_name: None,
                vm_state: None,
                windows_version: journal.windows_version,
                virtual_box_os_type_id: journal.virtual_box_os_type_id.clone(),
                virtual_box_os_description: None,
                system_disk_path: None,
                iso_path: None,
                iso_medium_uuid: None,
                credentials_required: true,
                suggested_computer_name,
                suggested_domain_name,
                suggested_hostname,
            },
        }
    }

    fn virtualization_readiness(
        &self,
    ) -> (
        crate::virtualization::DetectionState,
        crate::virtualization::DetectionState,
        Option<String>,
    ) {
        #[cfg(target_os = "windows")]
        let firmware = crate::virtualization::windows::firmware_virtualization_state();
        #[cfg(not(target_os = "windows"))]
        let firmware = crate::virtualization::DetectionState::Unknown;

        let execution = match self.checked(&["list", "hostinfo"]) {
            Ok(output) => parse_host_virtualization_support(&output.stdout),
            Err(_) => crate::virtualization::DetectionState::Unknown,
        };
        let blocker = if firmware == crate::virtualization::DetectionState::No {
            Some("firmware virtualization is disabled".to_owned())
        } else if firmware == crate::virtualization::DetectionState::Unknown {
            Some("firmware virtualization readiness could not be determined".to_owned())
        } else if execution == crate::virtualization::DetectionState::No {
            Some("VirtualBox reports that hardware virtualization is unavailable".to_owned())
        } else if execution == crate::virtualization::DetectionState::Unknown {
            Some("VirtualBox execution readiness could not be determined".to_owned())
        } else {
            None
        };
        (firmware, execution, blocker)
    }

    fn validate_managed_vm_recovery(
        &self,
        configured_vm_id: &str,
        managed_by_multiseat: bool,
        journal: &ManagedVmInstallation,
    ) -> Result<ManagedVmRecoveryPlan, String> {
        if !managed_by_multiseat {
            return Err("unrelated VM is not marked as managed by MultiSeat Lite".to_owned());
        }
        if journal.state != ManagedInstallState::Failed {
            return Err("managed installation is not in a failed recovery state".to_owned());
        }
        let profile = self.resolve_windows_profile(journal.windows_version)?;
        if !journal.virtual_box_os_type_id.is_empty()
            && !journal
                .virtual_box_os_type_id
                .eq_ignore_ascii_case(&profile.virtual_box_os_type_id)
        {
            return Err(format!(
                "managed journal OS type '{}' does not match resolved profile '{}'",
                journal.virtual_box_os_type_id, profile.virtual_box_os_type_id
            ));
        }

        let output = self.checked(&["showvminfo", configured_vm_id, "--machinereadable"])?;
        let fields = parse_machine_readable(&output.stdout);
        let actual_uuid = fields
            .get("UUID")
            .ok_or_else(|| "VirtualBox VM has no UUID".to_owned())?;
        if !actual_uuid.eq_ignore_ascii_case(configured_vm_id) {
            return Err(format!(
                "VirtualBox returned UUID {actual_uuid}, not managed UUID {configured_vm_id}"
            ));
        }
        let description = fields
            .get("ostype")
            .ok_or_else(|| "VirtualBox VM has no OS type description".to_owned())?;
        if !description.eq_ignore_ascii_case(&profile.guest_os_description) {
            return Err(format!(
                "VM OS description '{}' does not match managed profile '{}'",
                description, profile.guest_os_description
            ));
        }
        let state = fields
            .get("VMState")
            .map(String::as_str)
            .unwrap_or("unknown");
        if state.eq_ignore_ascii_case("running")
            || state.eq_ignore_ascii_case("paused")
            || state.eq_ignore_ascii_case("saved")
        {
            return Err(format!(
                "managed installation is already active; current VM state is {state}"
            ));
        }
        if !state.eq_ignore_ascii_case("poweroff") && !state.eq_ignore_ascii_case("aborted") {
            return Err(format!("managed VM has an unknown recovery state: {state}"));
        }
        let expected_firmware =
            if profile.windows_version == crate::seats::ManagedWindowsVersion::Windows11 {
                "EFI"
            } else {
                "BIOS"
            };
        for (field, expected, label) in [
            ("firmware", expected_firmware, "firmware"),
            ("ioapic", "on", "I/O APIC"),
            ("xhci", "on", "xHCI"),
            ("graphicscontroller", "vboxsvga", "graphics controller"),
            ("storagecontrollertype0", "IntelAhci", "SATA controller"),
            ("nic1", "nat", "NAT network"),
            ("audio_out", "on", "audio output"),
        ] {
            if fields
                .get(field)
                .is_none_or(|value| !value.eq_ignore_ascii_case(expected))
            {
                return Err(format!(
                    "managed VM {label} is not compatible with the selected profile"
                ));
            }
        }

        let system_disk = fields
            .get("SATA-0-0")
            .filter(|path| path.as_str() != "none")
            .ok_or_else(|| "managed SATA system disk is not attached".to_owned())?;
        if !Path::new(system_disk).is_file() {
            return Err(format!(
                "managed system disk '{}' is unavailable",
                system_disk
            ));
        }
        let attached_install_media = fields
            .get("SATA-1-0")
            .filter(|path| path.as_str() != "none")
            .ok_or_else(|| "Windows installation ISO is not attached".to_owned())?;
        let media = inspect_recovery_media(configured_vm_id, attached_install_media)?;

        let guest_properties = self.guest_properties(configured_vm_id)?;
        let runlevels = self.probe_guest_runlevels(configured_vm_id, 2_000);
        let guest_os = guest_properties.get("/VirtualBox/GuestInfo/OS/Product");
        let additions = guest_properties.get("/VirtualBox/GuestAdd/Version");
        let additions_communication =
            guest_properties.contains_key("/VirtualBox/GuestAdd/HostVerLastChecked");
        let guest_reached_userland = runlevels
            .iter()
            .any(|probe| probe.success && probe.level >= GuestRunLevel::Userland);
        if journal.state == ManagedInstallState::Ready
            || journal.observed.windows == WindowsInstallationStatus::Installed
            || guest_os
                .map(String::as_str)
                .is_some_and(|value| value.to_ascii_lowercase().contains("windows"))
            || additions.is_some()
            || additions_communication
            || guest_reached_userland
        {
            return Err(
                "guest evidence indicates Windows installation has already progressed or completed"
                    .to_owned(),
            );
        }

        let (classification, iso_path, unattended_media_path, credentials_required) = match media {
            RecoveryMedia::OriginalIso { iso_path } => (
                ManagedVmRecoveryClassification::RecoverablePreBoot,
                iso_path,
                None,
                true,
            ),
            RecoveryMedia::Prepared {
                iso_path,
                unattended_media_path,
            } => (
                ManagedVmRecoveryClassification::RecoverablePreparedInstall,
                iso_path,
                Some(unattended_media_path),
                false,
            ),
        };
        let previous_failure =
            historical_virtualization_failure(journal.last_error.as_deref(), &fields)
                .or_else(|| journal.last_error.clone());

        Ok(ManagedVmRecoveryPlan {
            status: ManagedVmRecoveryStatus {
                can_resume: false,
                classification,
                reason: None,
                previous_problem: previous_problem(previous_failure.as_deref(), Some(state)),
                previous_failure,
                firmware_virtualization: crate::virtualization::DetectionState::Unknown,
                virtual_box_execution: crate::virtualization::DetectionState::Unknown,
                virtualization_ready: false,
                virtualization_blocker: None,
                vm_configuration_valid: true,
                unattended_preparation_present: unattended_media_path.is_some(),
                unattended_media_path,
                vm_id: configured_vm_id.to_owned(),
                vm_name: fields.get("name").cloned(),
                vm_state: Some(state.to_owned()),
                windows_version: journal.windows_version,
                virtual_box_os_type_id: profile.virtual_box_os_type_id.clone(),
                virtual_box_os_description: Some(profile.guest_os_description.clone()),
                system_disk_path: Some(system_disk.clone()),
                iso_path: Some(iso_path),
                iso_medium_uuid: fields.get("SATA-ImageUUID-1-0").cloned(),
                credentials_required,
                suggested_computer_name: String::new(),
                suggested_domain_name: String::new(),
                suggested_hostname: String::new(),
            },
            profile,
        })
    }

    pub fn resume_managed_unattended_install(
        &self,
        configured_vm_id: &str,
        managed_by_multiseat: bool,
        journal: &ManagedVmInstallation,
        config: &WindowsInstallConfig,
        additions_iso: Option<&Path>,
    ) -> Result<ManagedVmInstallation, InstallationFailure> {
        let plan = self
            .validate_managed_vm_recovery(configured_vm_id, managed_by_multiseat, journal)
            .map_err(|message| InstallationFailure {
                failed_step: "validate-managed-vm-recovery".to_owned(),
                message,
            })?;
        let iso_path = plan
            .status
            .iso_path
            .as_deref()
            .ok_or_else(|| InstallationFailure {
                failed_step: "validate-managed-vm-recovery".to_owned(),
                message: "validated recovery has no Windows ISO path".to_owned(),
            })?;
        match plan.status.classification {
            ManagedVmRecoveryClassification::RecoverablePreparedInstall => {
                self.checked(&["startvm", configured_vm_id, "--type", "gui"])
                    .map_err(|message| InstallationFailure {
                        failed_step: "start-prepared-unattended-installation".to_owned(),
                        message,
                    })?;
                Ok(ManagedVmInstallation {
                    state: ManagedInstallState::InstallingWindows,
                    last_error: None,
                    ..journal.clone()
                })
            }
            ManagedVmRecoveryClassification::RecoverablePreBoot => self.prepare_unattended_install(
                configured_vm_id,
                iso_path,
                config,
                &plan.profile,
                additions_iso,
            ),
            _ => Err(InstallationFailure {
                failed_step: "validate-managed-vm-recovery".to_owned(),
                message: "managed VM is not in a retryable recovery classification".to_owned(),
            }),
        }
    }

    pub fn prepare_unattended_install(
        &self,
        vm_id: &str,
        iso_path: &str,
        config: &WindowsInstallConfig,
        profile: &ManagedWindowsVmProfile,
        additions_iso: Option<&Path>,
    ) -> Result<ManagedVmInstallation, InstallationFailure> {
        validate_windows_install_config(config).map_err(|message| InstallationFailure {
            failed_step: "validate-installation".to_owned(),
            message,
        })?;
        if config.install_guest_additions && additions_iso.is_none() {
            return Err(InstallationFailure {
                failed_step: "locate-guest-additions".to_owned(),
                message: "Guest Additions was requested but VBoxGuestAdditions.iso was not found"
                    .to_owned(),
            });
        }
        let vm = self
            .inspect_vm(vm_id)
            .map_err(|message| InstallationFailure {
                failed_step: "verify-managed-guest-profile".to_owned(),
                message,
            })?;
        if vm
            .guest_os_description
            .as_deref()
            .is_none_or(|description| {
                !description.eq_ignore_ascii_case(&profile.guest_os_description)
            })
        {
            return Err(InstallationFailure {
                failed_step: "verify-managed-guest-profile".to_owned(),
                message: format!(
                    "managed VM does not use selected {} profile (ID {}, description {})",
                    profile.display_name,
                    profile.virtual_box_os_type_id,
                    profile.guest_os_description
                ),
            });
        }

        let mut password_file = NamedTempFile::new().map_err(|error| InstallationFailure {
            failed_step: "create-password-file".to_owned(),
            message: format!("could not create temporary password file: {error}"),
        })?;
        password_file
            .write_all(config.password.as_bytes())
            .and_then(|_| password_file.flush())
            .map_err(|error| InstallationFailure {
                failed_step: "write-password-file".to_owned(),
                message: format!("could not prepare temporary password file: {error}"),
            })?;

        let hostname =
            unattended_hostname(&config.computer_name, &config.domain_name).map_err(|message| {
                InstallationFailure {
                    failed_step: "validate-installation-hostname".to_owned(),
                    message,
                }
            })?;
        let (arguments, sensitive) =
            unattended_arguments(vm_id, iso_path, password_file.path(), additions_iso, config)
                .map_err(|message| InstallationFailure {
                    failed_step: "build-unattended-command".to_owned(),
                    message,
                })?;
        let argument_refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let sensitive_refs: Vec<&str> = sensitive.iter().map(String::as_str).collect();
        self.checked_redacted(&argument_refs, &sensitive_refs)
            .map_err(|message| InstallationFailure {
                failed_step: "prepare-and-start-unattended-installation".to_owned(),
                message: format!(
                    "Computer name: {}; DNS domain: {}; generated hostname: {hostname}; {message}",
                    config.computer_name.trim(),
                    config.domain_name.trim()
                ),
            })?;
        // The temporary password file is securely scoped to the synchronous
        // VBoxManage preparation call and is deleted when this function exits.
        Ok(ManagedVmInstallation {
            windows_version: profile.windows_version,
            virtual_box_os_type_id: profile.virtual_box_os_type_id.clone(),
            computer_name: config.computer_name.trim().to_owned(),
            domain_name: config.domain_name.trim().to_ascii_lowercase(),
            state: ManagedInstallState::InstallingWindows,
            guest_additions: if config.install_guest_additions {
                GuestAdditionsInstallState::Installing
            } else {
                GuestAdditionsInstallState::NotInstalled
            },
            guest_additions_version: None,
            last_error: None,
            observed: ManagedInstallationObservation {
                windows: WindowsInstallationStatus::Installing,
                managed: ManagedVmStatus::Installing,
                ..ManagedInstallationObservation::default()
            },
        })
    }

    pub fn inspect_managed_installation(
        &self,
        vm_id: &str,
        previous: &ManagedVmInstallation,
    ) -> Result<ManagedInstallationSnapshot, String> {
        let vm = self.inspect_vm(vm_id)?;
        let runlevel_probes = self.probe_guest_runlevels(vm_id, 2_000);
        let properties = self.guest_properties(vm_id)?;
        let installation =
            reconcile_installation_observation(previous, &vm.state, runlevel_probes, &properties);
        let windows_guest_detected =
            installation.observed.windows == WindowsInstallationStatus::Installed;
        let monitoring_complete = matches!(
            installation.observed.managed,
            ManagedVmStatus::Ready | ManagedVmStatus::Failed
        );

        Ok(ManagedInstallationSnapshot {
            vm_id: vm_id.to_owned(),
            vm_state: vm.state,
            installation,
            windows_guest_detected,
            monitoring_complete,
        })
    }

    pub fn probe_guest_runlevels(&self, vm_id: &str, timeout_ms: u64) -> Vec<GuestRunLevelProbe> {
        [
            (GuestRunLevel::System, "system"),
            (GuestRunLevel::Userland, "userland"),
            (GuestRunLevel::Desktop, "desktop"),
        ]
        .into_iter()
        .map(|(level, name)| {
            let started = Instant::now();
            let arguments = vec![
                "guestcontrol".to_owned(),
                vm_id.to_owned(),
                "waitrunlevel".to_owned(),
                name.to_owned(),
                "--timeout".to_owned(),
                timeout_ms.to_string(),
            ];
            match self.executor.execute(&arguments) {
                Ok(output) => GuestRunLevelProbe {
                    level,
                    // VBoxManage normally prints nothing when the requested
                    // runlevel is reached. Its process exit code is authoritative.
                    success: output.exit_code == Some(0),
                    exit_code: output.exit_code,
                    stdout: output.stdout,
                    stderr: output.stderr,
                    elapsed_ms: elapsed_millis(started),
                },
                Err(error) => GuestRunLevelProbe {
                    level,
                    success: false,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: error,
                    elapsed_ms: elapsed_millis(started),
                },
            }
        })
        .collect()
    }

    fn guest_properties(
        &self,
        vm_id: &str,
    ) -> Result<std::collections::HashMap<String, String>, String> {
        let output = self.checked(&["guestproperty", "enumerate", vm_id])?;
        Ok(parse_guest_properties(&output.stdout))
    }
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn parse_guest_properties(output: &str) -> std::collections::HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if let Some((name, remainder)) = line.split_once(" = ") {
                let value = remainder
                    .split_once(" @ ")
                    .map(|(value, _)| value)
                    .unwrap_or(remainder)
                    .trim()
                    .trim_matches('\'');
                return Some((name.trim().to_owned(), value.to_owned()));
            }
            let remainder = line.strip_prefix("Name: ")?;
            let (name, remainder) = remainder.split_once(", value: ")?;
            let value = remainder
                .split_once(", timestamp:")
                .map(|(value, _)| value)
                .unwrap_or(remainder);
            Some((name.trim().to_owned(), value.trim().to_owned()))
        })
        .collect()
}

fn reconcile_installation_observation(
    previous: &ManagedVmInstallation,
    vm_state: &str,
    runlevel_probes: Vec<GuestRunLevelProbe>,
    properties: &std::collections::HashMap<String, String>,
) -> ManagedVmInstallation {
    let strongest_current_runlevel = runlevel_probes
        .iter()
        .filter(|probe| probe.success)
        .map(|probe| probe.level)
        .max()
        .unwrap_or(GuestRunLevel::Unknown);
    let run_level = strongest_current_runlevel.max(previous.observed.run_level);
    let current_guest_os = properties.get("/VirtualBox/GuestInfo/OS/Product").cloned();
    let guest_os_product = current_guest_os
        .clone()
        .or_else(|| previous.observed.evidence.guest_os_product.clone());
    let current_additions_version = properties.get("/VirtualBox/GuestAdd/Version").cloned();
    let guest_additions_version = current_additions_version
        .clone()
        .or_else(|| previous.observed.evidence.guest_additions_version.clone())
        .or_else(|| previous.guest_additions_version.clone());
    let current_host_version = properties
        .get("/VirtualBox/GuestAdd/HostVerLastChecked")
        .cloned();
    let host_version = current_host_version.clone().or_else(|| {
        previous
            .observed
            .evidence
            .guest_add_host_version_last_checked
            .clone()
    });
    let reset_counter = properties
        .get("/VirtualBox/VMInfo/ResetCounter")
        .and_then(|value| value.parse().ok())
        .or(previous.observed.evidence.reset_counter);

    let expected_windows = match previous.windows_version {
        crate::seats::ManagedWindowsVersion::Windows10 => "windows 10",
        crate::seats::ManagedWindowsVersion::Windows11 => "windows 11",
    };
    let os_product_matches = guest_os_product
        .as_deref()
        .is_some_and(|value| value.to_ascii_lowercase().contains(expected_windows));
    let guest_communication_observed =
        strongest_current_runlevel >= GuestRunLevel::System || current_additions_version.is_some();
    let partial_guest_evidence = current_host_version.is_some();
    let completion_observed =
        strongest_current_runlevel >= GuestRunLevel::Userland || os_product_matches;
    let windows = if previous.observed.windows == WindowsInstallationStatus::Installed
        || completion_observed
    {
        WindowsInstallationStatus::Installed
    } else if matches!(
        previous.state,
        ManagedInstallState::Creating
            | ManagedInstallState::InstallingWindows
            | ManagedInstallState::Rebooting
    ) {
        WindowsInstallationStatus::Installing
    } else {
        WindowsInstallationStatus::Unknown
    };
    let guest_additions = if current_additions_version.is_some()
        || (guest_additions_version.is_some()
            && previous.observed.run_level >= GuestRunLevel::System)
    {
        GuestAdditionsStatus::InstalledVersionKnown
    } else if guest_communication_observed {
        GuestAdditionsStatus::Communicating
    } else if partial_guest_evidence {
        GuestAdditionsStatus::PartialEvidence
    } else if windows == WindowsInstallationStatus::Installed {
        GuestAdditionsStatus::Unavailable
    } else {
        GuestAdditionsStatus::Unknown
    };
    let vm_state_normalized = vm_state.to_ascii_lowercase();
    let strong_readiness = strongest_current_runlevel >= GuestRunLevel::Userland
        || (os_product_matches && current_additions_version.is_some());
    let managed = if windows == WindowsInstallationStatus::Installed
        && vm_state_normalized == "saved"
    {
        ManagedVmStatus::Saved
    } else if windows == WindowsInstallationStatus::Installed
        && matches!(vm_state_normalized.as_str(), "poweroff" | "poweredoff")
    {
        ManagedVmStatus::Stopped
    } else if windows == WindowsInstallationStatus::Installed && strong_readiness {
        ManagedVmStatus::Ready
    } else if windows == WindowsInstallationStatus::Installed
        && vm_state_normalized == "running"
        && partial_guest_evidence
        && strongest_current_runlevel == GuestRunLevel::Unknown
        && current_guest_os.is_none()
        && current_additions_version.is_none()
    {
        ManagedVmStatus::Degraded
    } else if windows == WindowsInstallationStatus::Installed && vm_state_normalized == "running" {
        ManagedVmStatus::OperationalUnverified
    } else if windows == WindowsInstallationStatus::Installed {
        ManagedVmStatus::WaitingForGuestAdditions
    } else if matches!(vm_state_normalized.as_str(), "aborted" | "gurumeditation") {
        ManagedVmStatus::Failed
    } else {
        ManagedVmStatus::Installing
    };
    let previous_completion_was_strong = previous.state == ManagedInstallState::Ready
        && (previous.observed.run_level >= GuestRunLevel::Userland
            || (previous.observed.evidence.guest_os_product.is_some()
                && previous.observed.evidence.guest_additions_version.is_some()));
    let state = match managed {
        ManagedVmStatus::Ready => ManagedInstallState::Ready,
        ManagedVmStatus::Saved | ManagedVmStatus::Stopped if previous_completion_was_strong => {
            ManagedInstallState::Ready
        }
        ManagedVmStatus::WindowsInstalled
        | ManagedVmStatus::WaitingForGuestAdditions
        | ManagedVmStatus::OperationalUnverified
        | ManagedVmStatus::Degraded
        | ManagedVmStatus::Stopped
        | ManagedVmStatus::Saved => ManagedInstallState::WaitingForGuest,
        ManagedVmStatus::Failed => ManagedInstallState::Failed,
        ManagedVmStatus::Installing => ManagedInstallState::InstallingWindows,
    };
    let reconciliation_changed = previous.state != state
        || previous.observed.managed != managed
        || previous.observed.guest_additions != guest_additions;
    let reconciliation_event = reconciliation_changed.then(|| {
        format!(
            "Persisted state: {:?}; observed: {}; reconciled: {:?}",
            previous.state,
            if vm_state_normalized == "saved" {
                "VM is saved; guest runtime readiness is unavailable".to_owned()
            } else if matches!(vm_state_normalized.as_str(), "poweroff" | "poweredoff") {
                "VM is powered off; guest runtime readiness is unavailable".to_owned()
            } else if strongest_current_runlevel >= GuestRunLevel::Userland {
                format!("{:?} runlevel reached", strongest_current_runlevel)
            } else if current_additions_version.is_some() && os_product_matches {
                "Guest OS and Guest Additions versions verified".to_owned()
            } else if current_host_version.is_some() {
                "partial Guest Additions host-version evidence only".to_owned()
            } else {
                "no strong guest readiness evidence".to_owned()
            },
            managed
        )
    });
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());

    let mut installation = previous.clone();
    installation.state = state;
    installation.guest_additions = match guest_additions {
        GuestAdditionsStatus::InstalledVersionKnown => GuestAdditionsInstallState::Installed,
        GuestAdditionsStatus::RepairRequired => GuestAdditionsInstallState::Failed,
        GuestAdditionsStatus::Unavailable => GuestAdditionsInstallState::NotInstalled,
        GuestAdditionsStatus::PartialEvidence
        | GuestAdditionsStatus::Communicating
        | GuestAdditionsStatus::Unknown => GuestAdditionsInstallState::Unknown,
    };
    installation.guest_additions_version = guest_additions_version.clone();
    if managed == ManagedVmStatus::Ready {
        installation.last_error = None;
    } else if managed == ManagedVmStatus::Failed && installation.last_error.is_none() {
        installation.last_error = Some(format!("VirtualBox VM entered state '{vm_state}'"));
    }
    installation.observed = ManagedInstallationObservation {
        windows,
        guest_additions,
        run_level,
        managed,
        evidence: ManagedInstallationEvidence {
            last_vm_state: Some(vm_state.to_owned()),
            runlevel_probes,
            guest_os_product,
            guest_additions_version,
            guest_add_host_version_last_checked: host_version,
            reset_counter,
            reconciliation_timestamp_unix_ms: timestamp,
            reconciliation_event: reconciliation_event
                .or_else(|| previous.observed.evidence.reconciliation_event.clone()),
        },
    };
    installation
}

pub fn bundled_guest_additions_iso() -> Option<PathBuf> {
    super::guest_additions_iso_path()
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use crate::virtualization::virtualbox::{
        GuestAdditionsPreparation, GuestAdditionsState, SecureBootStatus, VBoxManageExecutor,
        VBoxOutput, VirtualBoxRuntimeService,
    };

    use super::*;

    struct PropertyExecutor;

    impl VBoxManageExecutor for PropertyExecutor {
        fn execute(&self, arguments: &[String]) -> Result<VBoxOutput, String> {
            let runlevel = arguments.iter().any(|value| value == "waitrunlevel");
            let stdout = if arguments.first().is_some_and(|value| value == "showvminfo") {
                "name=\"Barnen\"\nUUID=\"vm-id\"\nVMState=\"running\"\nostype=\"Windows 11 (64-bit)\"\n"
            } else {
                ""
            };
            Ok(VBoxOutput {
                success: !runlevel,
                exit_code: Some(if runlevel { 1 } else { 0 }),
                stdout: stdout.to_owned(),
                stderr: runlevel
                    .then(|| "runlevel unavailable".to_owned())
                    .unwrap_or_default(),
            })
        }

        fn wait(&self, _: Duration) {}
    }

    struct RunlevelExecutor {
        highest: GuestRunLevel,
        properties: String,
    }

    struct ExitCodeOnlyExecutor;

    impl VBoxManageExecutor for ExitCodeOnlyExecutor {
        fn execute(&self, _arguments: &[String]) -> Result<VBoxOutput, String> {
            Ok(VBoxOutput {
                // Deliberately inconsistent to prove the probe uses exit_code.
                success: false,
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    impl VBoxManageExecutor for RunlevelExecutor {
        fn execute(&self, arguments: &[String]) -> Result<VBoxOutput, String> {
            let (success, stdout, stderr) = match arguments.first().map(String::as_str) {
                Some("showvminfo") => (
                    true,
                    "name=\"Barnen\"\nUUID=\"vm-id\"\nVMState=\"running\"\nostype=\"Windows 10 (64-bit)\"\n".to_owned(),
                    String::new(),
                ),
                Some("guestproperty") => (true, self.properties.clone(), String::new()),
                Some("guestcontrol") => {
                    let requested = arguments
                        .iter()
                        .position(|value| value == "waitrunlevel")
                        .and_then(|index| arguments.get(index + 1))
                        .map(|value| match value.as_str() {
                            "system" => GuestRunLevel::System,
                            "userland" => GuestRunLevel::Userland,
                            "desktop" => GuestRunLevel::Desktop,
                            _ => GuestRunLevel::Unknown,
                        })
                        .unwrap_or(GuestRunLevel::Unknown);
                    let reached = requested != GuestRunLevel::Unknown && requested <= self.highest;
                    (
                        reached,
                        String::new(),
                        (!reached).then(|| "runlevel unavailable".to_owned()).unwrap_or_default(),
                    )
                }
                _ => (true, String::new(), String::new()),
            };
            Ok(VBoxOutput {
                success,
                exit_code: Some(if success { 0 } else { 1 }),
                stdout,
                stderr,
            })
        }
    }

    struct FailingExecutor;

    impl VBoxManageExecutor for FailingExecutor {
        fn execute(&self, arguments: &[String]) -> Result<VBoxOutput, String> {
            Ok(VBoxOutput {
                success: false,
                exit_code: Some(1),
                stdout: String::new(),
                stderr: format!("rejected {}", arguments.join(" ")),
            })
        }
    }

    struct UnattendedFailingExecutor;

    impl VBoxManageExecutor for UnattendedFailingExecutor {
        fn execute(&self, arguments: &[String]) -> Result<VBoxOutput, String> {
            if arguments.first().is_some_and(|value| value == "showvminfo") {
                return Ok(VBoxOutput {
                    success: true,
                    exit_code: Some(0),
                    stdout: "name=\"Barnen\"\nUUID=\"vm-id\"\nVMState=\"poweroff\"\nostype=\"Windows 10 (64-bit)\"\n".to_owned(),
                    stderr: String::new(),
                });
            }
            Ok(VBoxOutput {
                success: false,
                exit_code: Some(1),
                stdout: String::new(),
                stderr: format!("rejected {}", arguments.join(" ")),
            })
        }
    }

    #[derive(Clone)]
    struct RecordingInstallExecutor(Arc<Mutex<Vec<Vec<String>>>>);

    impl VBoxManageExecutor for RecordingInstallExecutor {
        fn execute(&self, arguments: &[String]) -> Result<VBoxOutput, String> {
            self.0.lock().unwrap().push(arguments.to_vec());
            let stdout = if arguments.first().is_some_and(|value| value == "showvminfo") {
                "name=\"Barnen\"\nUUID=\"vm-id\"\nVMState=\"poweroff\"\nostype=\"Windows 10 (64-bit)\"\n"
            } else {
                ""
            };
            Ok(VBoxOutput {
                success: true,
                exit_code: Some(0),
                stdout: stdout.to_owned(),
                stderr: String::new(),
            })
        }
    }

    #[derive(Clone)]
    struct RecoveryExecutor {
        calls: Arc<Mutex<Vec<Vec<String>>>>,
        vm_info: String,
        hostinfo: String,
        guest_os: Option<String>,
    }

    impl VBoxManageExecutor for RecoveryExecutor {
        fn execute(&self, arguments: &[String]) -> Result<VBoxOutput, String> {
            self.calls.lock().unwrap().push(arguments.to_vec());
            if arguments
                .first()
                .is_some_and(|value| value == "guestcontrol")
            {
                return Ok(VBoxOutput {
                    success: false,
                    exit_code: Some(1),
                    stdout: String::new(),
                    stderr: "runlevel unavailable".to_owned(),
                });
            }
            let stdout = match arguments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                ["list", "ostypes"] => "ID / Description: Win10-Recovery-ID -- Windows 10 (64-bit)\nFamily: Windows (Microsoft Windows)\nArchitecture: x86 (64-bit)\n\nID / Description: Win11-Recovery-ID -- Windows 11 (64-bit)\nFamily: Windows (Microsoft Windows)\nArchitecture: x86 (64-bit)\n".to_owned(),
                ["list", "hostinfo"] => self.hostinfo.clone(),
                ["showvminfo", ..] => self.vm_info.clone(),
                ["guestproperty", "get", _, "/VirtualBox/GuestInfo/OS/Product"] => self
                    .guest_os
                    .as_ref()
                    .map(|value| format!("Value: {value}\n"))
                    .unwrap_or_else(|| "No value set!\n".to_owned()),
                ["guestproperty", "get", ..] => "No value set!\n".to_owned(),
                ["guestproperty", "enumerate", ..] => self
                    .guest_os
                    .as_ref()
                    .map(|value| format!("/VirtualBox/GuestInfo/OS/Product = '{value}' @ 1\n"))
                    .unwrap_or_default(),
                ["unattended", "install", ..] => String::new(),
                _ => String::new(),
            };
            Ok(VBoxOutput {
                success: true,
                exit_code: Some(0),
                stdout,
                stderr: String::new(),
            })
        }
    }

    fn config() -> WindowsInstallConfig {
        WindowsInstallConfig {
            username: "Barnen".to_owned(),
            password: "s3cret password".to_owned(),
            computer_name: "BARNEN-PC".to_owned(),
            domain_name: "multiseat.local".to_owned(),
            locale: "sv_SE".to_owned(),
            language: "sv".to_owned(),
            country: "SE".to_owned(),
            time_zone: String::new(),
            product_key: None,
            install_guest_additions: true,
        }
    }

    fn profile(
        windows_version: crate::seats::ManagedWindowsVersion,
        guest_os_type: &str,
    ) -> ManagedWindowsVmProfile {
        let windows_11 = windows_version == crate::seats::ManagedWindowsVersion::Windows11;
        ManagedWindowsVmProfile {
            windows_version,
            display_name: if windows_11 {
                "Windows 11 x64"
            } else {
                "Windows 10 x64"
            }
            .into(),
            virtual_box_os_type_id: guest_os_type.into(),
            guest_os_description: if windows_11 {
                "Windows 11 (64-bit)"
            } else {
                "Windows 10 (64-bit)"
            }
            .into(),
            architecture: "x86_64".into(),
            firmware: if windows_11 { "UEFI" } else { "BIOS" }.into(),
            tpm: if windows_11 { "2.0" } else { "Not required" }.into(),
            tpm_required: windows_11,
            io_apic_enabled: true,
            secure_boot: if windows_11 {
                SecureBootStatus::Configured
            } else {
                SecureBootStatus::NotRequired
            },
            secure_boot_required: windows_11,
            usb_controller: "xHCI".into(),
            graphics_controller: "VBoxSVGA".into(),
            vram_mb: 128,
            network: "NAT".into(),
            audio_output_enabled: true,
            storage_controller: "SATA (Intel AHCI)".into(),
            guest_additions: GuestAdditionsPreparation {
                state: GuestAdditionsState::NotInstalled,
                bundled_iso_path: None,
                automatic_installation_supported: true,
            },
        }
    }

    fn installing_journal() -> ManagedVmInstallation {
        ManagedVmInstallation {
            windows_version: crate::seats::ManagedWindowsVersion::Windows10,
            virtual_box_os_type_id: "Windows10_64".into(),
            computer_name: "BARNEN-PC".into(),
            domain_name: "multiseat.local".into(),
            state: ManagedInstallState::InstallingWindows,
            guest_additions: GuestAdditionsInstallState::Installing,
            guest_additions_version: None,
            last_error: None,
            observed: ManagedInstallationObservation {
                windows: WindowsInstallationStatus::Installing,
                managed: ManagedVmStatus::Installing,
                ..ManagedInstallationObservation::default()
            },
        }
    }

    fn recovery_fixture(
        reported_uuid: &str,
        description: &str,
    ) -> (tempfile::TempDir, RecoveryExecutor, ManagedVmInstallation) {
        use std::{
            fs,
            fs::OpenOptions,
            io::{Seek, SeekFrom, Write},
        };

        let directory = tempfile::tempdir().unwrap();
        let disk = directory.path().join("Barnen-system.vdi");
        fs::write(&disk, b"managed disk fixture").unwrap();
        let iso = directory.path().join("Windows.iso");
        let mut iso_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&iso)
            .unwrap();
        iso_file.seek(SeekFrom::Start(32_769)).unwrap();
        iso_file.write_all(b"CD001").unwrap();
        let vm_info = format!(
            "name=\"Windows - Barnen\"\nUUID=\"{reported_uuid}\"\nVMState=\"poweroff\"\nostype=\"{description}\"\nfirmware=\"BIOS\"\nioapic=\"on\"\nxhci=\"on\"\ngraphicscontroller=\"vboxsvga\"\nstoragecontrollertype0=\"IntelAhci\"\nnic1=\"nat\"\naudio_out=\"on\"\n\"SATA-0-0\"=\"{}\"\n\"SATA-1-0\"=\"{}\"\n\"SATA-ImageUUID-1-0\"=\"iso-medium-uuid\"\n",
            disk.display(),
            iso.display()
        );
        let executor = RecoveryExecutor {
            calls: Arc::new(Mutex::new(Vec::new())),
            vm_info,
            hostinfo: "Processor supports HW virtualization: yes\n".to_owned(),
            guest_os: None,
        };
        let journal = ManagedVmInstallation {
            windows_version: crate::seats::ManagedWindowsVersion::Windows10,
            virtual_box_os_type_id: "Win10-Recovery-ID".into(),
            computer_name: "MULTISEAT-BARNEN".into(),
            domain_name: String::new(),
            state: ManagedInstallState::Failed,
            guest_additions: GuestAdditionsInstallState::Failed,
            guest_additions_version: None,
            last_error: Some("Windows installation failed at 'verify-managed-guest-profile': representation mismatch".into()),
            observed: ManagedInstallationObservation::default(),
        };
        (directory, executor, journal)
    }

    fn prepared_recovery_fixture(
        vm_state: &str,
    ) -> (tempfile::TempDir, RecoveryExecutor, ManagedVmInstallation) {
        use std::fs;

        let (directory, mut executor, mut journal) =
            recovery_fixture("managed-vm-uuid", "Windows 10 (64-bit)");
        let iso = directory.path().join("Windows.iso");
        let viso = directory
            .path()
            .join("Unattended-managed-vm-uuid-aux-iso.viso");
        fs::write(
            &viso,
            format!(
                "--import-iso-skip-eltorito '{}' '/autounattend.xml=managed'",
                iso.display()
            ),
        )
        .unwrap();
        fs::write(
            directory
                .path()
                .join("Unattended-managed-vm-uuid-autounattend.xml"),
            b"managed fixture",
        )
        .unwrap();
        fs::write(
            directory
                .path()
                .join("Unattended-managed-vm-uuid-VBOXPOST.CMD"),
            b"managed fixture",
        )
        .unwrap();
        executor.vm_info = executor
            .vm_info
            .replace("VMState=\"poweroff\"", &format!("VMState=\"{vm_state}\""))
            .replace(
                &format!("\"SATA-1-0\"=\"{}\"", iso.display()),
                &format!("\"SATA-1-0\"=\"{}\"", viso.display()),
            );
        journal.last_error =
            Some("VirtualBox start failed: VERR_NEM_NOT_AVAILABLE; VERR_SVM_DISABLED".into());
        (directory, executor, journal)
    }

    #[test]
    fn command_preserves_paths_with_spaces_and_uses_password_file() {
        let config = config();
        let (arguments, sensitive) = unattended_arguments(
            "vm-id",
            r"C:\Install media\Windows 11.iso",
            Path::new(r"C:\Temp files\password.txt"),
            Some(Path::new(
                r"C:\Program Files\Oracle\VirtualBox\VBoxGuestAdditions.iso",
            )),
            &config,
        )
        .unwrap();
        assert!(arguments.contains(&r"--iso=C:\Install media\Windows 11.iso".to_owned()));
        assert!(arguments.contains(&r"--user-password-file=C:\Temp files\password.txt".to_owned()));
        assert!(arguments
            .iter()
            .all(|argument| !argument.contains(&config.password)));
        assert!(arguments
            .iter()
            .any(|argument| argument == "--install-additions"));
        assert!(arguments
            .iter()
            .any(|argument| argument == "--hostname=barnen-pc.multiseat.local"));
        assert!(sensitive.is_empty());
    }

    #[test]
    fn optional_product_key_is_omitted_or_marked_sensitive() {
        let mut config = config();
        let (arguments, _) =
            unattended_arguments("vm", "w.iso", Path::new("p"), None, &config).unwrap();
        assert!(arguments
            .iter()
            .all(|argument| !argument.starts_with("--key=")));
        config.product_key = Some("AAAAA-BBBBB-CCCCC-DDDDD-EEEEE".to_owned());
        let (arguments, sensitive) =
            unattended_arguments("vm", "w.iso", Path::new("p"), None, &config).unwrap();
        let key_argument = "--key=AAAAA-BBBBB-CCCCC-DDDDD-EEEEE".to_owned();
        assert!(arguments.contains(&key_argument));
        assert_eq!(sensitive, vec![key_argument]);
        assert!(!format!("{config:?}").contains("AAAAA-BBBBB"));
        assert!(!format!("{config:?}").contains("s3cret"));
    }

    #[test]
    fn unattended_install_uses_and_records_selected_windows_profile() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let service =
            VirtualBoxRuntimeService::new(Box::new(RecordingInstallExecutor(calls.clone())));
        let mut install_config = config();
        install_config.install_guest_additions = false;
        let selected = profile(
            crate::seats::ManagedWindowsVersion::Windows10,
            "Win10-Test-ID",
        );
        let journal = service
            .prepare_unattended_install(
                "vm-id",
                r"C:\Install media\Windows 10.iso",
                &install_config,
                &selected,
                None,
            )
            .unwrap();
        assert_eq!(
            journal.windows_version,
            crate::seats::ManagedWindowsVersion::Windows10
        );
        assert_eq!(journal.virtual_box_os_type_id, "Win10-Test-ID");
        assert!(calls.lock().unwrap().iter().any(|arguments| {
            arguments.first().is_some_and(|value| value == "unattended")
                && arguments
                    .iter()
                    .any(|value| value == r"--iso=C:\Install media\Windows 10.iso")
        }));
    }

    #[test]
    fn validates_account_and_locale_fields() {
        assert!(validate_windows_install_config(&config()).is_ok());
        let mut invalid = config();
        invalid.username = "bad/name".to_owned();
        assert!(validate_windows_install_config(&invalid).is_err());
        let mut invalid = config();
        invalid.locale = "Swedish".to_owned();
        assert!(validate_windows_install_config(&invalid).is_err());
    }

    #[test]
    fn builds_and_validates_virtualbox_fqdn_separately_from_computer_name() {
        assert_eq!(
            unattended_hostname("BARNEN-PC", "multiseat.local").unwrap(),
            "barnen-pc.multiseat.local"
        );
        assert!(unattended_hostname("MULTISEAT-BARNEN", "multiseat.local").is_err());
        assert!(unattended_hostname("FIFTEEN-CHAR-PC", "multiseat.local").is_ok());
        assert_eq!("FIFTEEN-CHAR-PC".len(), 15);
        assert!(unattended_hostname("BARNEN-PC", "").is_err());
        assert!(unattended_hostname("BARNEN_PC", "multiseat.local").is_err());
        assert!(unattended_hostname("BARNEN-PC", "bad..local").is_err());
    }

    #[test]
    fn migrates_only_known_legacy_managed_computer_name() {
        let mut legacy = config();
        legacy.computer_name = "MULTISEAT-BARNEN".into();
        legacy.domain_name.clear();
        assert!(apply_legacy_managed_install_defaults(&mut legacy));
        assert_eq!(legacy.computer_name, "BARNEN-PC");
        assert_eq!(legacy.domain_name, "multiseat.local");
        assert_eq!(
            unattended_hostname(&legacy.computer_name, &legacy.domain_name).unwrap(),
            "barnen-pc.multiseat.local"
        );

        let mut arbitrary_invalid = config();
        arbitrary_invalid.computer_name = "USER-ENTERED-NAME-TOO-LONG".into();
        assert!(!apply_legacy_managed_install_defaults(
            &mut arbitrary_invalid
        ));
        assert!(validate_windows_install_config(&arbitrary_invalid).is_err());
    }

    #[test]
    fn missing_guest_properties_never_claim_guest_additions_or_ready() {
        let service = VirtualBoxRuntimeService::new(Box::new(PropertyExecutor));
        let previous = ManagedVmInstallation {
            windows_version: crate::seats::ManagedWindowsVersion::Windows11,
            virtual_box_os_type_id: "Windows11_64".into(),
            computer_name: "BARNEN-PC".into(),
            domain_name: "multiseat.local".into(),
            state: ManagedInstallState::InstallingWindows,
            guest_additions: GuestAdditionsInstallState::Installing,
            guest_additions_version: None,
            last_error: None,
            observed: ManagedInstallationObservation {
                windows: WindowsInstallationStatus::Installing,
                managed: ManagedVmStatus::Installing,
                ..ManagedInstallationObservation::default()
            },
        };
        let snapshot = service
            .inspect_managed_installation("vm-id", &previous)
            .unwrap();
        assert_ne!(snapshot.installation.state, ManagedInstallState::Ready);
        assert_ne!(
            snapshot.installation.guest_additions,
            GuestAdditionsInstallState::Installed
        );
    }

    #[test]
    fn waitrunlevel_exit_zero_with_empty_stdout_is_success() {
        let service = VirtualBoxRuntimeService::new(Box::new(ExitCodeOnlyExecutor));
        let probes = service.probe_guest_runlevels("vm-id", 5_000);
        assert_eq!(probes.len(), 3);
        assert!(probes.iter().all(|probe| probe.success));
        assert!(probes.iter().all(|probe| probe.exit_code == Some(0)));
        assert!(probes.iter().all(|probe| probe.stdout.is_empty()));
    }

    #[test]
    fn desktop_without_version_means_windows_installed_and_operational() {
        let service = VirtualBoxRuntimeService::new(Box::new(RunlevelExecutor {
            highest: GuestRunLevel::Desktop,
            properties: String::new(),
        }));
        let snapshot = service
            .inspect_managed_installation("vm-id", &installing_journal())
            .unwrap();
        assert_eq!(
            snapshot.installation.observed.windows,
            WindowsInstallationStatus::Installed
        );
        assert_eq!(
            snapshot.installation.observed.guest_additions,
            GuestAdditionsStatus::Communicating
        );
        assert_eq!(
            snapshot.installation.observed.managed,
            ManagedVmStatus::Ready
        );
        assert!(snapshot.installation.guest_additions_version.is_none());
        assert_eq!(snapshot.installation.state, ManagedInstallState::Ready);
        assert!(snapshot.monitoring_complete);
    }

    #[test]
    fn userland_advances_stale_windows_installation_without_reinstalling() {
        let service = VirtualBoxRuntimeService::new(Box::new(RunlevelExecutor {
            highest: GuestRunLevel::Userland,
            properties: String::new(),
        }));
        let snapshot = service
            .inspect_managed_installation("vm-id", &installing_journal())
            .unwrap();
        assert_ne!(
            snapshot.installation.state,
            ManagedInstallState::InstallingWindows
        );
        assert_eq!(
            snapshot.installation.observed.windows,
            WindowsInstallationStatus::Installed
        );
        assert!(snapshot
            .installation
            .observed
            .evidence
            .reconciliation_event
            .as_deref()
            .is_some_and(|event| event.contains("Userland runlevel reached")));
    }

    #[test]
    fn guest_additions_version_is_optional_but_recorded_when_known() {
        let properties = "/VirtualBox/GuestAdd/HostVerLastChecked = '7.2.14' @ 1\n/VirtualBox/GuestAdd/Version = '7.2.14' @ 1\n";
        let service = VirtualBoxRuntimeService::new(Box::new(RunlevelExecutor {
            highest: GuestRunLevel::Unknown,
            properties: properties.to_owned(),
        }));
        let snapshot = service
            .inspect_managed_installation("vm-id", &installing_journal())
            .unwrap();
        assert_eq!(
            snapshot.installation.observed.guest_additions,
            GuestAdditionsStatus::InstalledVersionKnown
        );
        assert_eq!(
            snapshot.installation.guest_additions_version.as_deref(),
            Some("7.2.14")
        );
    }

    #[test]
    fn host_version_only_is_partial_evidence_and_never_ready() {
        let properties = "/VirtualBox/GuestAdd/HostVerLastChecked = '7.2.14' @ 1\n/VirtualBox/VMInfo/ResetCounter = '2' @ 1\n";
        let service = VirtualBoxRuntimeService::new(Box::new(RunlevelExecutor {
            highest: GuestRunLevel::Unknown,
            properties: properties.to_owned(),
        }));
        let mut journal = installing_journal();
        journal.state = ManagedInstallState::Rebooting;
        journal.observed.windows = WindowsInstallationStatus::Installed;
        let snapshot = service
            .inspect_managed_installation("vm-id", &journal)
            .unwrap();
        assert_eq!(
            snapshot.installation.observed.managed,
            ManagedVmStatus::Degraded
        );
        assert_eq!(
            snapshot.installation.observed.guest_additions,
            GuestAdditionsStatus::PartialEvidence
        );
        assert_eq!(
            snapshot.installation.state,
            ManagedInstallState::WaitingForGuest
        );
        assert!(!snapshot.monitoring_complete);
        assert_eq!(
            snapshot
                .installation
                .observed
                .evidence
                .guest_add_host_version_last_checked
                .as_deref(),
            Some("7.2.14")
        );
        assert_eq!(
            snapshot.installation.observed.evidence.reset_counter,
            Some(2)
        );
    }

    #[test]
    fn saved_vm_preserves_installed_windows_but_reports_saved_readiness() {
        let mut journal = installing_journal();
        journal.observed.windows = WindowsInstallationStatus::Installed;
        let observation = reconcile_installation_observation(
            &journal,
            "saved",
            vec![GuestRunLevelProbe {
                level: GuestRunLevel::System,
                success: false,
                exit_code: Some(1),
                ..GuestRunLevelProbe::default()
            }],
            &std::collections::HashMap::new(),
        );
        assert_eq!(
            observation.observed.windows,
            WindowsInstallationStatus::Installed
        );
        assert_eq!(observation.observed.managed, ManagedVmStatus::Saved);
        assert_eq!(observation.state, ManagedInstallState::WaitingForGuest);
    }

    #[test]
    fn command_errors_redact_product_keys() {
        let service = VirtualBoxRuntimeService::new(Box::new(FailingExecutor));
        let key = "--key=AAAAA-BBBBB-CCCCC-DDDDD-EEEEE";
        let error = service
            .checked_redacted(&["unattended", "install", key], &[key])
            .unwrap_err();
        assert!(!error.contains("AAAAA-BBBBB"));
        assert!(error.contains("<redacted>"));
    }

    #[test]
    fn unattended_failure_is_structured_and_secret_free() {
        let service = VirtualBoxRuntimeService::new(Box::new(UnattendedFailingExecutor));
        let mut config = config();
        config.install_guest_additions = false;
        config.product_key = Some("AAAAA-BBBBB-CCCCC-DDDDD-EEEEE".to_owned());
        let failure = service
            .prepare_unattended_install(
                "vm-id",
                "Windows 10.iso",
                &config,
                &profile(
                    crate::seats::ManagedWindowsVersion::Windows10,
                    "Win10-Test-ID",
                ),
                None,
            )
            .unwrap_err();
        assert_eq!(
            failure.failed_step,
            "prepare-and-start-unattended-installation"
        );
        let report = failure.to_string();
        assert!(report.contains("Computer name: BARNEN-PC"));
        assert!(report.contains("DNS domain: multiseat.local"));
        assert!(report.contains("generated hostname: barnen-pc.multiseat.local"));
        assert!(!report.contains("s3cret"));
        assert!(!report.contains("AAAAA-BBBBB"));
    }

    #[test]
    fn failed_profile_verification_can_recover_existing_managed_vm() {
        let (_directory, executor, journal) =
            recovery_fixture("managed-vm-uuid", "Windows 10 (64-bit)");
        let service = VirtualBoxRuntimeService::new(Box::new(executor));
        let status = service.managed_vm_recovery_status("managed-vm-uuid", true, &journal);
        assert!(status.can_resume, "{:?}", status.reason);
        assert_eq!(status.virtual_box_os_type_id, "Win10-Recovery-ID");
        assert_eq!(
            status.virtual_box_os_description.as_deref(),
            Some("Windows 10 (64-bit)")
        );
        assert!(status.credentials_required);
    }

    #[test]
    fn incomplete_hostname_failure_can_recover_existing_managed_vm() {
        let (_directory, executor, mut journal) =
            recovery_fixture("managed-vm-uuid", "Windows 10 (64-bit)");
        journal.last_error = Some(
            "Windows installation failed at 'prepare-and-start-unattended-installation': VBoxManage error: Incomplete hostname 'MULTISEAT-BARNEN'"
                .into(),
        );
        let service = VirtualBoxRuntimeService::new(Box::new(executor));
        let status = service.managed_vm_recovery_status("managed-vm-uuid", true, &journal);
        assert!(status.can_resume, "{:?}", status.reason);
        assert_eq!(status.suggested_computer_name, "BARNEN-PC");
        assert_eq!(status.suggested_domain_name, "multiseat.local");
        assert_eq!(status.suggested_hostname, "barnen-pc.multiseat.local");
    }

    #[test]
    fn virtualization_start_failures_are_state_recoverable_without_error_whitelisting() {
        for failure in [
            "VERR_SVM_DISABLED",
            "VERR_NEM_NOT_AVAILABLE",
            "an arbitrary pre-boot failure from a future VirtualBox release",
        ] {
            let (_directory, executor, mut journal) =
                recovery_fixture("managed-vm-uuid", "Windows 10 (64-bit)");
            journal.last_error = Some(failure.to_owned());
            let service = VirtualBoxRuntimeService::new(Box::new(executor));
            let status = service.managed_vm_recovery_status("managed-vm-uuid", true, &journal);
            assert_eq!(
                status.classification,
                ManagedVmRecoveryClassification::RecoverablePreBoot
            );
            assert!(status.can_resume, "{failure}: {:?}", status.reason);
        }
    }

    #[test]
    fn recovery_is_recognized_but_blocked_when_virtualbox_execution_is_unavailable() {
        let (_directory, mut executor, journal) =
            recovery_fixture("managed-vm-uuid", "Windows 10 (64-bit)");
        executor.hostinfo = "Processor supports HW virtualization: no\n".to_owned();
        let service = VirtualBoxRuntimeService::new(Box::new(executor));
        let status = service.managed_vm_recovery_status("managed-vm-uuid", true, &journal);
        assert_eq!(
            status.classification,
            ManagedVmRecoveryClassification::RecoverablePreBoot
        );
        assert!(!status.can_resume);
        assert_eq!(
            status.virtual_box_execution,
            crate::virtualization::DetectionState::No
        );
    }

    #[test]
    fn firmware_preflight_blocks_then_allows_the_same_recoverable_state() {
        let classification = ManagedVmRecoveryClassification::RecoverablePreparedInstall;
        assert!(!recovery_can_resume(
            classification,
            crate::virtualization::DetectionState::No,
            crate::virtualization::DetectionState::Yes,
        ));
        assert!(recovery_can_resume(
            classification,
            crate::virtualization::DetectionState::Yes,
            crate::virtualization::DetectionState::Yes,
        ));
    }

    #[test]
    fn prepared_install_reuses_media_and_system_disk_without_duplicate_preparation() {
        let (_directory, executor, journal) = prepared_recovery_fixture("aborted");
        let calls = executor.calls.clone();
        let service = VirtualBoxRuntimeService::new(Box::new(executor));
        let status = service.managed_vm_recovery_status("managed-vm-uuid", true, &journal);
        assert_eq!(
            status.classification,
            ManagedVmRecoveryClassification::RecoverablePreparedInstall
        );
        assert!(status.unattended_preparation_present);
        assert!(!status.credentials_required);
        assert!(status
            .system_disk_path
            .as_deref()
            .is_some_and(|path| path.ends_with("Barnen-system.vdi")));

        let mut install_config = config();
        install_config.password.clear();
        install_config.install_guest_additions = false;
        service
            .resume_managed_unattended_install(
                "managed-vm-uuid",
                true,
                &journal,
                &install_config,
                None,
            )
            .unwrap();
        let calls = calls.lock().unwrap();
        assert!(calls
            .iter()
            .any(|arguments| arguments == &["startvm", "managed-vm-uuid", "--type", "gui"]));
        assert!(!calls.iter().any(|arguments| arguments
            .first()
            .is_some_and(|value| value == "unattended"
                || value == "createvm"
                || value == "storageattach")));
    }

    #[test]
    fn completed_windows_guest_is_not_treated_as_fresh_retry() {
        let (_directory, mut executor, journal) =
            recovery_fixture("managed-vm-uuid", "Windows 10 (64-bit)");
        executor.guest_os = Some("Microsoft Windows 10".to_owned());
        let service = VirtualBoxRuntimeService::new(Box::new(executor));
        let status = service.managed_vm_recovery_status("managed-vm-uuid", true, &journal);
        assert!(!status.can_resume);
        assert_eq!(
            status.classification,
            ManagedVmRecoveryClassification::Unrecoverable
        );
    }

    #[test]
    fn recovery_rejects_wrong_profile_uuid_and_unmanaged_vm() {
        let (_directory, executor, journal) =
            recovery_fixture("managed-vm-uuid", "Windows 11 (64-bit)");
        let service = VirtualBoxRuntimeService::new(Box::new(executor));
        assert!(
            !service
                .managed_vm_recovery_status("managed-vm-uuid", true, &journal)
                .can_resume
        );

        let (_directory, executor, journal) =
            recovery_fixture("different-vm-uuid", "Windows 10 (64-bit)");
        let service = VirtualBoxRuntimeService::new(Box::new(executor));
        assert!(
            !service
                .managed_vm_recovery_status("managed-vm-uuid", true, &journal)
                .can_resume
        );

        let (_directory, executor, journal) =
            recovery_fixture("managed-vm-uuid", "Windows 10 (64-bit)");
        let service = VirtualBoxRuntimeService::new(Box::new(executor));
        assert!(
            !service
                .managed_vm_recovery_status("managed-vm-uuid", false, &journal)
                .can_resume
        );
    }

    #[test]
    fn recovery_reuses_vm_without_creation_and_requires_credentials_again() {
        let (_directory, executor, journal) =
            recovery_fixture("managed-vm-uuid", "Windows 10 (64-bit)");
        let calls = executor.calls.clone();
        let service = VirtualBoxRuntimeService::new(Box::new(executor));
        let mut missing_credentials = config();
        missing_credentials.password.clear();
        missing_credentials.install_guest_additions = false;
        let failure = service
            .resume_managed_unattended_install(
                "managed-vm-uuid",
                true,
                &journal,
                &missing_credentials,
                None,
            )
            .unwrap_err();
        assert_eq!(failure.failed_step, "validate-installation");

        let mut install_config = config();
        install_config.install_guest_additions = false;
        let resumed = service
            .resume_managed_unattended_install(
                "managed-vm-uuid",
                true,
                &journal,
                &install_config,
                None,
            )
            .unwrap();
        assert_eq!(resumed.state, ManagedInstallState::InstallingWindows);
        let calls = calls.lock().unwrap();
        assert!(calls
            .iter()
            .any(|arguments| arguments.first().is_some_and(|value| value == "unattended")));
        assert!(!calls
            .iter()
            .any(|arguments| arguments.first().is_some_and(|value| value == "createvm")));
    }
}
