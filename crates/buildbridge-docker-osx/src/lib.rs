//! Typed, host-side lifecycle management for a single Docker-OSX builder.
//!
//! All Docker calls use fixed argv assembled from validated profile fields.
//! No shell is involved and secret material is never passed to Docker.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const CONTAINER_NAME: &str = "buildbridge-macos-builder";
pub const DOCKER_IMAGE: &str = "sickcodes/docker-osx:latest";
const XCODE_ARCHIVE_NAME: &str = "BuildBridge-Xcode.xip";
const XCODE_PACKAGE_MAX_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const SIGNING_CERTIFICATE_MAX_BYTES: u64 = 32 * 1024 * 1024;
const PROVISIONING_PROFILE_MAX_BYTES: u64 = 8 * 1024 * 1024;
const SIGNING_MATERIAL_MAX_BYTES: u64 = 128 * 1024 * 1024;
const SIGNING_PROFILE_MAX_COUNT: usize = 20;
const SIGNING_KEYCHAIN_NAME: &str = "buildbridge-signing.keychain-db";
const SIGNING_HELPER_SOURCE: &[u8] = include_bytes!("signing_helper.c");
const APPLE_WWDR_G3_PEM: &[u8] = include_bytes!("../assets/apple-wwdr-g3.pem");
const APPLE_WWDR_G3_DER_SHA256: &str =
    "DCF21878C77F4198E4B4614F03D696D89C66C66008D4244E1B99161AAC91601F";
const APPLE_WORKSPACE_MAX_FILES: u64 = 50_000;
const APPLE_WORKSPACE_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const APPLE_BUILD_OUTPUT_TAIL_LINES: usize = 80;
const APPLE_BUILD_DIAGNOSTIC_LINES: usize = 24;
const APPLE_ARCHIVE_MAX_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const NODE_VERSION: &str = "24.20.0";
const NODE_DARWIN_X64_SHA256: &str =
    "9e5b2644cf107befb6aefca676b96d3296bc10138096f022ed378d6233ed81f4";
const PNPM_VERSION: &str = "11.5.0";
const COCOAPODS_VERSION: &str = "1.16.2";
const ACTIVESUPPORT_VERSION: &str = "6.1.7.10";
const CONCURRENT_RUBY_VERSION: &str = "1.3.5";
const I18N_VERSION: &str = "1.14.7";
const MINITEST_VERSION: &str = "5.24.1";
const TZINFO_VERSION: &str = "2.0.6";
const ZEITWERK_VERSION: &str = "2.6.18";
const FFI_VERSION: &str = "1.17.0";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacOsRelease {
    Tahoe,
    #[default]
    Sequoia,
    Sonoma,
    Ventura,
}

impl MacOsRelease {
    fn short_name(self) -> &'static str {
        match self {
            Self::Tahoe => "tahoe",
            Self::Sequoia => "sequoia",
            Self::Sonoma => "sonoma",
            Self::Ventura => "ventura",
        }
    }

    fn master_plist_url(self) -> &'static str {
        match self {
            Self::Tahoe | Self::Sequoia | Self::Sonoma => {
                "https://raw.githubusercontent.com/sickcodes/osx-serial-generator/master/config-custom-sonoma.plist"
            }
            Self::Ventura => {
                "https://raw.githubusercontent.com/sickcodes/osx-serial-generator/master/config-custom.plist"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MacBuilderConfig {
    pub name: String,
    pub macos_release: MacOsRelease,
    pub memory_gib: u8,
    pub cpu_cores: u8,
    pub ssh_port: u16,
}

impl Default for MacBuilderConfig {
    fn default() -> Self {
        Self {
            name: "Local macOS builder".to_string(),
            macos_release: MacOsRelease::Sequoia,
            memory_gib: 8,
            cpu_cores: 4,
            ssh_port: 50_922,
        }
    }
}

impl MacBuilderConfig {
    pub fn validate(&self) -> Result<(), ProviderError> {
        let name_length = self.name.trim().chars().count();

        if !(1..=80).contains(&name_length) {
            return Err(ProviderError::InvalidConfig(
                "builder name must contain between 1 and 80 characters".to_string(),
            ));
        }

        if !(4..=64).contains(&self.memory_gib) {
            return Err(ProviderError::InvalidConfig(
                "memory must be between 4 and 64 GiB".to_string(),
            ));
        }

        if !(2..=32).contains(&self.cpu_cores) {
            return Err(ProviderError::InvalidConfig(
                "CPU cores must be between 2 and 32".to_string(),
            ));
        }

        if self.ssh_port < 1024 {
            return Err(ProviderError::InvalidConfig(
                "SSH port must be between 1024 and 65535".to_string(),
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPrerequisites {
    pub supported_host: bool,
    pub docker_cli: bool,
    pub docker_daemon: bool,
    pub docker_version: Option<String>,
    pub kvm_access: bool,
    pub display_access: bool,
    pub display: Option<String>,
    pub ready: bool,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerState {
    #[default]
    Missing,
    Created,
    Running,
    Paused,
    Restarting,
    Exited,
    Dead,
    Unavailable,
    Unknown,
}

impl ContainerState {
    fn from_docker(value: &str) -> Self {
        match value {
            "created" => Self::Created,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "restarting" => Self::Restarting,
            "exited" => Self::Exited,
            "dead" => Self::Dead,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub prerequisites: HostPrerequisites,
    pub state: ContainerState,
    pub container_id: Option<String>,
    pub started_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestTrustState {
    #[default]
    Unavailable,
    Untrusted,
    Trusted,
    Mismatch,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestSshStatus {
    pub port_open: bool,
    pub reachable: bool,
    pub trust: GuestTrustState,
    pub fingerprint: Option<String>,
    pub pinned_fingerprint: Option<String>,
    pub issue: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedHostKey {
    pub known_hosts_line: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestDiagnostics {
    pub authenticated: bool,
    pub macos_version: Option<String>,
    pub xcode_version: Option<String>,
    pub xcode_path: Option<String>,
    pub xcode_selected: bool,
    pub issue: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XcodeImportPhase {
    Preparing,
    Transferring,
    Expanding,
    AwaitingActivation,
    AwaitingAuthorization,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XcodeImportProgress {
    pub phase: XcodeImportPhase,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub elapsed_seconds: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XcodeImportResult {
    pub installed_path: String,
    pub activation_commands: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SigningProvisioningPhase {
    Preparing,
    Transferring,
    ImportingCertificate,
    InspectingProfiles,
    InstallingProfiles,
    Verifying,
    Completed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SigningProvisioningProgress {
    pub phase: SigningProvisioningPhase,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub elapsed_seconds: u64,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningProfileSummary {
    pub uuid: String,
    pub team_identifier: String,
    pub application_identifier: String,
    pub expires_at: String,
    pub developer_certificate_sha256: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SigningProvisioningResult {
    pub keychain_path: String,
    pub identity_name: String,
    pub identity_sha1: String,
    pub certificate_sha256: String,
    pub certificate_expires_at: String,
    pub development_team: String,
    pub bundle_identifier: String,
    pub profiles: Vec<ProvisioningProfileSummary>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppleProjectPhase {
    Snapshotting,
    Transferring,
    Extracting,
    PreparingTools,
    PreparingPlatform,
    InstallingDependencies,
    BuildingWebAssets,
    SyncingIos,
    ResolvingPods,
    Building,
    Completed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleProjectProgress {
    pub phase: AppleProjectPhase,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub elapsed_seconds: u64,
    pub detail: String,
    pub log_line: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleWorkspaceSyncResult {
    pub guest_path: String,
    pub snapshot_sha256: String,
    pub source_file_count: u64,
    pub source_bytes: u64,
    pub archive_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSmokeBuildResult {
    pub xcode_version: String,
    pub native_lockfile_updated: bool,
    pub output_tail: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppleArchivePhase {
    Preparing,
    Archiving,
    Exporting,
    Verifying,
    PackagingArchive,
    Transferring,
    Completed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleArchiveProgress {
    pub phase: AppleArchivePhase,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub elapsed_seconds: u64,
    pub detail: String,
    pub log_line: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleArchiveArtifact {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleArchiveResult {
    pub scheme: String,
    pub configuration: String,
    pub export_method: String,
    pub bundle_identifier: String,
    pub development_team: String,
    pub marketing_version: String,
    pub build_number: String,
    pub provisioning_profile_uuid: String,
    pub ipa: AppleArchiveArtifact,
    pub archive: AppleArchiveArtifact,
    pub output_tail: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("invalid macOS builder configuration: {0}")]
    InvalidConfig(String),
    #[error("the host is not ready for Docker-OSX: {0}")]
    Prerequisites(String),
    #[error("failed to run Docker: {0}")]
    DockerUnavailable(String),
    #[error("failed to prepare the persistent macOS machine identity: {0}")]
    Identity(String),
    #[error("Docker {operation} failed: {message}")]
    DockerCommand {
        operation: &'static str,
        message: String,
    },
    #[error("macOS guest bridge failed: {0}")]
    GuestBridge(String),
}

/// Probes the forwarded SSH port and compares the live guest key with an optional pin.
/// A reachable but unpinned guest is never treated as trusted.
pub fn guest_ssh_status(ssh_port: u16, pinned_host_key: Option<&str>) -> GuestSshStatus {
    if !guest_port_reachable(ssh_port) {
        return GuestSshStatus {
            issue: Some(
                "Guest SSH is not reachable yet. Finish macOS setup and enable Remote Login."
                    .to_string(),
            ),
            ..GuestSshStatus::default()
        };
    }

    let scanned = match scan_guest_host_key(ssh_port) {
        Ok(scanned) => scanned,
        Err(error) => {
            let issue = match &error {
                ProviderError::GuestBridge(message)
                    if message.starts_with("could not run ssh-keyscan") =>
                {
                    error.to_string()
                }
                _ => "The forwarded port is open, but macOS Remote Login has not presented an SSH identity yet. Installation or setup may still be in progress."
                    .to_string(),
            };

            return GuestSshStatus {
                port_open: true,
                issue: Some(issue),
                ..GuestSshStatus::default()
            };
        }
    };

    let Some(pinned_host_key) = pinned_host_key else {
        return GuestSshStatus {
            port_open: true,
            reachable: true,
            trust: GuestTrustState::Untrusted,
            fingerprint: Some(scanned.fingerprint),
            issue: Some(
                "Verify the guest fingerprint, then explicitly trust this macOS machine."
                    .to_string(),
            ),
            ..GuestSshStatus::default()
        };
    };

    let pinned_fingerprint = fingerprint_host_key_line(pinned_host_key).ok();
    let pinned_key = parse_host_key_line(pinned_host_key).ok();
    let scanned_key = parse_host_key_line(&scanned.known_hosts_line)
        .expect("a scanned key has already been validated");
    let trusted = pinned_key.as_ref().is_some_and(|pinned| {
        pinned.host == scanned_key.host
            && pinned.algorithm == scanned_key.algorithm
            && pinned.key == scanned_key.key
    });

    GuestSshStatus {
        port_open: true,
        reachable: true,
        trust: if trusted {
            GuestTrustState::Trusted
        } else {
            GuestTrustState::Mismatch
        },
        fingerprint: Some(scanned.fingerprint),
        pinned_fingerprint,
        issue: (!trusted).then(|| {
            "The guest SSH identity changed. Forget the old pin only after verifying this is the expected machine."
                .to_string()
        }),
    }
}

pub fn scan_guest_host_key(ssh_port: u16) -> Result<ScannedHostKey, ProviderError> {
    if ssh_port < 1024 {
        return Err(ProviderError::GuestBridge(
            "guest SSH port must be between 1024 and 65535".to_string(),
        ));
    }

    let output = Command::new("ssh-keyscan")
        .args([
            "-T",
            "3",
            "-p",
            &ssh_port.to_string(),
            "-t",
            "ed25519",
            "127.0.0.1",
        ])
        .output()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not run ssh-keyscan; install OpenSSH client tools: {error}"
            ))
        })?;

    let stdout = clean_output(&output.stdout);
    let expected_host = format!("[127.0.0.1]:{ssh_port}");
    let parsed = stdout
        .lines()
        .filter_map(|line| parse_host_key_line(line).ok())
        .find(|key| key.host == expected_host && key.algorithm == "ssh-ed25519")
        .ok_or_else(|| {
            let detail = clean_output(&output.stderr);
            let suffix = if detail.is_empty() {
                "the guest did not return an Ed25519 host key".to_string()
            } else {
                detail
            };
            ProviderError::GuestBridge(format!("SSH is reachable but not ready: {suffix}"))
        })?;
    let known_hosts_line = format!("{} {} {}", parsed.host, parsed.algorithm, parsed.key);
    let fingerprint = fingerprint_host_key_line(&known_hosts_line)?;

    Ok(ScannedHostKey {
        known_hosts_line,
        fingerprint,
    })
}

pub fn guest_diagnostics(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> GuestDiagnostics {
    if !valid_guest_username(username) {
        return GuestDiagnostics {
            issue: Some("The macOS short username is invalid.".to_string()),
            ..GuestDiagnostics::default()
        };
    }

    if !identity_path.is_file() || !known_hosts_path.is_file() {
        return GuestDiagnostics {
            issue: Some("Configure a guest key and trust its SSH fingerprint first.".to_string()),
            ..GuestDiagnostics::default()
        };
    }

    let macos_version = match run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        "/usr/bin/sw_vers -productVersion",
    ) {
        Ok(value) => value,
        Err(error) => {
            return GuestDiagnostics {
                issue: Some(error.to_string()),
                ..GuestDiagnostics::default()
            };
        }
    };
    let selected_xcode = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        "/usr/bin/xcodebuild -version",
    );

    let (xcode_version, xcode_path, xcode_selected, issue) = match selected_xcode {
        Ok(version) => {
            let selected_path = run_guest_command(
                ssh_port,
                username,
                identity_path,
                known_hosts_path,
                "/usr/bin/xcode-select --print-path",
            )
            .ok();
            let first_launch_ready = run_guest_command(
                ssh_port,
                username,
                identity_path,
                known_hosts_path,
                "/usr/bin/xcodebuild -checkFirstLaunchStatus",
            )
            .is_ok();

            (
                Some(version),
                selected_path,
                first_launch_ready,
                (!first_launch_ready).then(|| {
                    "Xcode is selected but first-launch setup is incomplete. Resume activation from BuildBridge."
                        .to_string()
                }),
            )
        }
        Err(_) => {
            let installed_path = guest_xcode_application_path(username);
            let direct_xcodebuild =
                format!("{installed_path}/Contents/Developer/usr/bin/xcodebuild -version");

            match run_guest_command(
                ssh_port,
                username,
                identity_path,
                known_hosts_path,
                &direct_xcodebuild,
            ) {
                Ok(version) => (
                    Some(version),
                    Some(installed_path),
                    false,
                    Some(
                        "Xcode is installed but not selected. Activate it from BuildBridge."
                            .to_string(),
                    ),
                ),
                Err(_) => (
                    None,
                    None,
                    false,
                    Some(
                        "SSH authentication succeeded. Import a compatible Xcode .xip package to finish the toolchain setup."
                            .to_string(),
                    ),
                ),
            }
        }
    };

    GuestDiagnostics {
        authenticated: true,
        macos_version: Some(macos_version),
        xcode_version,
        xcode_path,
        xcode_selected,
        issue,
    }
}

fn guest_xcode_application_path(username: &str) -> String {
    format!("/Users/{username}/Applications/Xcode.app")
}

pub fn import_xcode_package<F>(
    package_path: &Path,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    mut on_progress: F,
) -> Result<XcodeImportResult, ProviderError>
where
    F: FnMut(XcodeImportProgress),
{
    if !valid_guest_username(username) {
        return Err(ProviderError::GuestBridge(
            "the macOS short username is invalid".to_string(),
        ));
    }
    if ssh_port < 1024 {
        return Err(ProviderError::GuestBridge(
            "guest SSH port must be between 1024 and 65535".to_string(),
        ));
    }
    if !identity_path.is_file() || !known_hosts_path.is_file() {
        return Err(ProviderError::GuestBridge(
            "configure a guest key and trust its SSH fingerprint before importing Xcode"
                .to_string(),
        ));
    }

    let (validated_path, total_bytes) = validate_xcode_package(package_path)?;
    let started_at = Instant::now();
    let guest_home = format!("/Users/{username}");
    let guest_downloads = format!("{guest_home}/Downloads");
    let guest_applications = format!("{guest_home}/Applications");
    let guest_archive = format!("{guest_downloads}/{XCODE_ARCHIVE_NAME}");
    let installed_path = guest_xcode_application_path(username);

    on_progress(XcodeImportProgress {
        phase: XcodeImportPhase::Preparing,
        transferred_bytes: 0,
        total_bytes,
        elapsed_seconds: 0,
        detail: "Checking the pinned guest and preparing its import directory.".to_string(),
    });

    let prepare_directories = format!("/bin/mkdir -p '{guest_downloads}' '{guest_applications}'");
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &prepare_directories,
    )?;

    let inspect_destination = format!(
        "if /bin/test -e '{installed_path}' || /bin/test -e '{guest_downloads}/Xcode.app'; then /usr/bin/printf occupied; else /usr/bin/printf clear; fi"
    );
    let destination_status = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &inspect_destination,
    )?;
    if destination_status != "clear" {
        return Err(ProviderError::GuestBridge(
            "Xcode.app already exists in the guest user’s Applications or Downloads directory. Activate or move that copy instead of overwriting it."
                .to_string(),
        ));
    }

    stream_xcode_package(
        &validated_path,
        total_bytes,
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &guest_archive,
        started_at,
        &mut on_progress,
    )?;
    expand_xcode_package(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &guest_archive,
        &guest_downloads,
        &installed_path,
        total_bytes,
        started_at,
        &mut on_progress,
    )?;

    let activation_commands = vec![
        format!("sudo xcode-select --switch '{installed_path}'"),
        "sudo xcodebuild -license accept".to_string(),
        "sudo xcodebuild -runFirstLaunch".to_string(),
        "xcodebuild -version".to_string(),
    ];
    on_progress(XcodeImportProgress {
        phase: XcodeImportPhase::AwaitingActivation,
        transferred_bytes: total_bytes,
        total_bytes,
        elapsed_seconds: started_at.elapsed().as_secs(),
        detail: "Xcode is expanded and ready for BuildBridge activation.".to_string(),
    });

    Ok(XcodeImportResult {
        installed_path,
        activation_commands,
    })
}

pub fn activate_xcode<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    mut on_progress: F,
) -> Result<(), ProviderError>
where
    F: FnMut(XcodeImportProgress),
{
    if !valid_guest_username(username) {
        return Err(ProviderError::GuestBridge(
            "the macOS short username is invalid".to_string(),
        ));
    }
    if ssh_port < 1024 {
        return Err(ProviderError::GuestBridge(
            "guest SSH port must be between 1024 and 65535".to_string(),
        ));
    }
    if !identity_path.is_file() || !known_hosts_path.is_file() {
        return Err(ProviderError::GuestBridge(
            "configure a guest key and trust its SSH fingerprint before activating Xcode"
                .to_string(),
        ));
    }

    let installed_path = guest_xcode_application_path(username);
    let inspect_xcode = format!(
        "if /bin/test -x '{installed_path}/Contents/Developer/usr/bin/xcodebuild'; then /usr/bin/printf ready; else /usr/bin/printf missing; fi"
    );
    if run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &inspect_xcode,
    )? != "ready"
    {
        return Err(ProviderError::GuestBridge(
            "the imported Xcode application could not be found; import it again or use the recovery commands"
                .to_string(),
        ));
    }

    let started_at = Instant::now();
    let detail = "A macOS Terminal window is opening. Enter the local macOS login password there; BuildBridge does not receive or store it.";
    on_progress(XcodeImportProgress {
        phase: XcodeImportPhase::AwaitingAuthorization,
        transferred_bytes: 0,
        total_bytes: 0,
        elapsed_seconds: 0,
        detail: detail.to_string(),
    });

    // A bare SSH process cannot display Authorization Services UI in the console
    // audit session. Open a fixed, short-lived command file in the guest's Terminal
    // instead: sudo reads the password directly from its macOS TTY, never over SSH.
    let activation_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let guest_cache = format!("/Users/{username}/Library/Caches/dev.buildbridge.desktop");
    let status_path = format!("{guest_cache}/xcode-activation-{activation_id}.status");
    let script_path = format!("{guest_cache}/xcode-activation-{activation_id}.command");
    let terminal_script = format!(
        r#"#!/bin/zsh
/usr/bin/clear
/usr/bin/printf "BuildBridge Xcode activation\n\n"
/usr/bin/printf "Enter the local macOS login password when sudo asks.\n"
/usr/bin/printf "The password remains inside this macOS Terminal.\n"
/usr/bin/printf "Continuing selects Xcode, accepts its license, and installs required components.\n\n"
trap "/usr/bin/printf \"failed:interrupted\\n\" > {status_path}" EXIT
if /usr/bin/sudo /usr/bin/xcode-select --switch {installed_path} && \
   /usr/bin/sudo /usr/bin/xcodebuild -license accept && \
   /usr/bin/sudo /usr/bin/xcodebuild -runFirstLaunch; then
    trap - EXIT
    /usr/bin/printf "success\n" > {status_path}
    /usr/bin/printf "\nXcode is ready. You can close this window.\n"
else
    result=$?
    trap - EXIT
    /usr/bin/printf "failed:%s\n" "$result" > {status_path}
    /usr/bin/printf "\nActivation did not complete. Return to BuildBridge and retry.\n"
fi
read -k 1 "?Press any key to close this window."
"#
    );
    let quoted_terminal_script = shell_single_quote(&terminal_script);
    let remote_command = format!(
        "/bin/mkdir -p '{guest_cache}'; /bin/rm -f '{status_path}' '{script_path}'; /usr/bin/printf '%s' {quoted_terminal_script} > '{script_path}'; /bin/chmod 700 '{script_path}'; if ! /usr/bin/open -a Terminal '{script_path}'; then /usr/bin/printf launch_failed; exit 0; fi; remaining=1800; while /bin/test ! -f '{status_path}' && /bin/test \"$remaining\" -gt 0; do /bin/sleep 1; remaining=$((remaining - 1)); done; if /bin/test -f '{status_path}'; then /bin/cat '{status_path}'; /bin/rm -f '{status_path}' '{script_path}'; else /usr/bin/printf timeout; fi"
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start macOS Xcode activation: {error}"))
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture Xcode activation output".to_string())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture Xcode activation errors".to_string())
    })?;
    let stdout_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut stdout = stdout;
        let _ = stdout.read_to_end(&mut output);
        output
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut stderr = stderr;
        let _ = stderr.read_to_end(&mut output);
        output
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                on_progress(XcodeImportProgress {
                    phase: XcodeImportPhase::AwaitingAuthorization,
                    transferred_bytes: 0,
                    total_bytes: 0,
                    elapsed_seconds: started_at.elapsed().as_secs(),
                    detail: detail.to_string(),
                });
                thread::sleep(Duration::from_secs(1));
            }
            Err(error) => {
                return Err(ProviderError::GuestBridge(format!(
                    "could not monitor Xcode activation: {error}"
                )));
            }
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();

    if !status.success() {
        let stderr = clean_output(&stderr);
        let stdout = clean_output(&stdout);
        let message = if !stderr.is_empty() { stderr } else { stdout };
        let message = if message.contains("User canceled") || message.contains("-128") {
            "Xcode activation was canceled in macOS. Select Activate Xcode when you are ready."
                .to_string()
        } else if message.is_empty() {
            "macOS could not complete Xcode activation. Use the manual recovery commands if Terminal did not appear."
                .to_string()
        } else {
            format!(
                "macOS could not complete Xcode activation: {message}. Use the manual recovery commands if Terminal did not appear."
            )
        };

        return Err(ProviderError::GuestBridge(message));
    }

    match clean_output(&stdout).as_str() {
        "success" => {}
        "failed:interrupted" => {
            return Err(ProviderError::GuestBridge(
                "Xcode activation was interrupted in the macOS Terminal. Retry when you are ready."
                    .to_string(),
            ));
        }
        result if result.starts_with("failed:") => {
            return Err(ProviderError::GuestBridge(
                "Xcode activation did not complete in macOS. Retry and use the local macOS login password shown during account setup."
                    .to_string(),
            ));
        }
        "launch_failed" => {
            return Err(ProviderError::GuestBridge(
                "macOS could not open the activation Terminal window. Use the manual recovery commands."
                    .to_string(),
            ));
        }
        "timeout" => {
            return Err(ProviderError::GuestBridge(
                "Xcode activation timed out after 30 minutes. Close any activation Terminal window and retry."
                    .to_string(),
            ));
        }
        result => {
            return Err(ProviderError::GuestBridge(format!(
                "macOS returned an unexpected Xcode activation result: {result}"
            )));
        }
    }

    let selected_path = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        "/usr/bin/xcode-select --print-path",
    )
    .map_err(|_| {
        ProviderError::GuestBridge(
            "Xcode remains inactive. Retry and approve the macOS prompt with the local macOS login password, not the Apple Account password."
                .to_string(),
        )
    })?;
    let expected_path = format!("{installed_path}/Contents/Developer");
    if selected_path != expected_path {
        return Err(ProviderError::GuestBridge(format!(
            "macOS selected an unexpected developer directory: {selected_path}"
        )));
    }
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        "/usr/bin/xcodebuild -version",
    )?;
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        "/usr/bin/xcodebuild -checkFirstLaunchStatus",
    )
    .map_err(|_| {
        ProviderError::GuestBridge(
            "Xcode was selected, but macOS still reports incomplete first-launch setup. Retry activation and let the Terminal commands finish."
                .to_string(),
        )
    })?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn provision_signing<F>(
    certificate_path: &Path,
    certificate_password: &str,
    profile_paths: &[PathBuf],
    keychain_password: &str,
    development_team: &str,
    bundle_identifier: &str,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    mut on_progress: F,
) -> Result<SigningProvisioningResult, ProviderError>
where
    F: FnMut(SigningProvisioningProgress),
{
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    validate_signing_target(development_team, bundle_identifier)?;
    let (certificate_path, profile_paths, total_bytes) = validate_signing_material(
        certificate_path,
        certificate_password,
        profile_paths,
        keychain_password,
    )?;
    let started_at = Instant::now();
    let guest_home = format!("/Users/{username}");
    let guest_tools = format!("{guest_home}/.buildbridge/tools");
    let helper_source = format!("{guest_tools}/signing-helper.c");
    let helper_binary = format!("{guest_tools}/signing-helper");
    let xcodebuild =
        format!("{guest_home}/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild");
    let operation_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let staging =
        format!("{guest_home}/Library/Caches/dev.buildbridge.desktop/signing-{operation_id}");
    let guest_certificate = format!("{staging}/identity.p12");
    let guest_certificate_der = format!("{staging}/identity.der");
    let guest_wwdr_g3_pem = format!("{staging}/AppleWWDRCAG3.pem");
    let guest_wwdr_g3_der = format!("{staging}/AppleWWDRCAG3.cer");
    let keychain_path = format!("{guest_home}/Library/Keychains/{SIGNING_KEYCHAIN_NAME}");

    on_progress(signing_progress(
        SigningProvisioningPhase::Preparing,
        0,
        total_bytes,
        started_at,
        "Validating the signing kit and preparing the password-safe guest helper.",
    ));
    let prepare = format!(
        "set -eu; /bin/mkdir -p {} {}; /bin/chmod 700 {}; /usr/bin/security delete-keychain {} >/dev/null 2>&1 || /bin/rm -f {}; /bin/rm -rf {}; /bin/mkdir -p {}; /bin/chmod 700 {}",
        shell_single_quote(&guest_tools),
        shell_single_quote(&format!("{guest_home}/Library/Keychains")),
        shell_single_quote(&guest_tools),
        shell_single_quote(&keychain_path),
        shell_single_quote(&keychain_path),
        shell_single_quote(&staging),
        shell_single_quote(&staging),
        shell_single_quote(&staging),
    );
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &prepare,
    )?;

    let operation = (|| {
        stream_bytes_to_guest(
            SIGNING_HELPER_SOURCE,
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &helper_source,
            "signing helper",
        )?;
        stream_bytes_to_guest(
            APPLE_WWDR_G3_PEM,
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_wwdr_g3_pem,
            "pinned Apple WWDR G3 intermediate",
        )?;
        let compile = format!(
            "set -eu; /usr/bin/xcrun --sdk macosx clang -std=c11 -O2 -Wno-deprecated-declarations {} -framework Security -framework CoreFoundation -o {}; /bin/chmod 700 {}",
            shell_single_quote(&helper_source),
            shell_single_quote(&helper_binary),
            shell_single_quote(&helper_binary),
        );
        run_guest_command(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &compile,
        )?;

        let mut completed_bytes = 0_u64;
        stream_signing_file(
            &certificate_path,
            &guest_certificate,
            total_bytes,
            &mut completed_bytes,
            started_at,
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &mut on_progress,
        )?;
        let guest_profiles = profile_paths
            .iter()
            .enumerate()
            .map(|(index, profile_path)| {
                let guest_path = format!("{staging}/profile-{index:03}.mobileprovision");
                stream_signing_file(
                    profile_path,
                    &guest_path,
                    total_bytes,
                    &mut completed_bytes,
                    started_at,
                    ssh_port,
                    username,
                    identity_path,
                    known_hosts_path,
                    &mut on_progress,
                )?;
                Ok((guest_path, profile_path.clone()))
            })
            .collect::<Result<Vec<_>, ProviderError>>()?;

        on_progress(signing_progress(
            SigningProvisioningPhase::ImportingCertificate,
            total_bytes,
            total_bytes,
            started_at,
            "Creating the dedicated keychain and importing its non-extractable identity.",
        ));
        run_signing_helper(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &helper_binary,
            &keychain_path,
            &guest_certificate,
            &guest_certificate_der,
            &xcodebuild,
            keychain_password,
            certificate_password,
        )?;

        on_progress(signing_progress(
            SigningProvisioningPhase::Verifying,
            total_bytes,
            total_bytes,
            started_at,
            "Verifying the imported certificate and proving that its private key can sign code.",
        ));
        let certificate_output = run_guest_command(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &certificate_metadata_command(&guest_certificate_der),
        )?;
        let (
            certificate_expires_at,
            identity_sha1,
            certificate_sha256,
            certificate_team,
            identity_name,
        ) = parse_certificate_metadata(&certificate_output)?;
        if certificate_team != development_team {
            return Err(ProviderError::GuestBridge(format!(
                "the signing certificate belongs to team {certificate_team}, but the project uses team {development_team}"
            )));
        }
        install_apple_wwdr_g3_intermediate(
            &guest_wwdr_g3_pem,
            &guest_wwdr_g3_der,
            &keychain_path,
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
        )?;
        verify_code_signing_identity(
            &identity_sha1,
            &helper_binary,
            &keychain_path,
            &staging,
            keychain_password,
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
        )?;

        on_progress(signing_progress(
            SigningProvisioningPhase::InspectingProfiles,
            total_bytes,
            total_bytes,
            started_at,
            "Inspecting profile expiry, team, bundle, and included signing certificates.",
        ));
        let profile_output = inspect_guest_profiles(
            &guest_profiles,
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
        )?;
        let profiles = parse_profile_summaries(&profile_output)?;
        if profiles.len() != guest_profiles.len() {
            return Err(ProviderError::GuestBridge(
                "macOS returned incomplete provisioning profile metadata".to_string(),
            ));
        }
        for profile in &profiles {
            if profile.team_identifier != development_team {
                return Err(ProviderError::GuestBridge(format!(
                    "provisioning profile {} belongs to team {}, but the project uses team {}",
                    profile.uuid, profile.team_identifier, development_team
                )));
            }
            if !profile_allows_bundle(&profile.application_identifier, bundle_identifier) {
                return Err(ProviderError::GuestBridge(format!(
                    "provisioning profile {} allows {}, not the project bundle identifier {}",
                    profile.uuid, profile.application_identifier, bundle_identifier
                )));
            }
            if !profile
                .developer_certificate_sha256
                .iter()
                .any(|fingerprint| fingerprint == &certificate_sha256)
            {
                return Err(ProviderError::GuestBridge(format!(
                    "provisioning profile {} does not include the selected signing certificate",
                    profile.uuid
                )));
            }
        }

        on_progress(signing_progress(
            SigningProvisioningPhase::InstallingProfiles,
            total_bytes,
            total_bytes,
            started_at,
            "Installing the verified profiles under their Apple UUIDs.",
        ));
        install_guest_profiles(
            &guest_profiles,
            &profiles,
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
        )?;
        run_guest_command(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &format!(
                "/usr/bin/security lock-keychain {}",
                shell_single_quote(&keychain_path)
            ),
        )?;

        Ok(SigningProvisioningResult {
            keychain_path: keychain_path.clone(),
            identity_name,
            identity_sha1,
            certificate_sha256,
            certificate_expires_at,
            development_team: development_team.to_string(),
            bundle_identifier: bundle_identifier.to_string(),
            profiles,
        })
    })();

    let cleanup = if operation.is_ok() {
        format!("/bin/rm -rf {}", shell_single_quote(&staging))
    } else {
        format!(
            "/usr/bin/security delete-keychain {} >/dev/null 2>&1 || /bin/rm -f {}; /bin/rm -rf {}",
            shell_single_quote(&keychain_path),
            shell_single_quote(&keychain_path),
            shell_single_quote(&staging),
        )
    };
    let _ = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &cleanup,
    );
    let result = operation?;
    on_progress(signing_progress(
        SigningProvisioningPhase::Completed,
        total_bytes,
        total_bytes,
        started_at,
        "Signing identity and provisioning profiles are ready in macOS.",
    ));

    Ok(result)
}

pub fn clear_signing(
    profile_uuids: &[String],
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    if profile_uuids.len() > SIGNING_PROFILE_MAX_COUNT {
        return Err(ProviderError::GuestBridge(
            "too many stored signing profile identifiers".to_string(),
        ));
    }
    for uuid in profile_uuids {
        if !valid_profile_uuid(uuid) {
            return Err(ProviderError::GuestBridge(
                "stored provisioning profile metadata is invalid".to_string(),
            ));
        }
    }

    let guest_home = format!("/Users/{username}");
    let keychain_path = format!("{guest_home}/Library/Keychains/{SIGNING_KEYCHAIN_NAME}");
    let mut command = format!(
        "/usr/bin/security delete-keychain {} >/dev/null 2>&1 || /bin/rm -f {}",
        shell_single_quote(&keychain_path),
        shell_single_quote(&keychain_path),
    );
    for uuid in profile_uuids {
        for directory in [
            format!("{guest_home}/Library/MobileDevice/Provisioning Profiles"),
            format!("{guest_home}/Library/Developer/Xcode/UserData/Provisioning Profiles"),
        ] {
            command.push_str(&format!(
                "; /bin/rm -f {}",
                shell_single_quote(&format!("{directory}/{uuid}.mobileprovision"))
            ));
        }
    }

    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &command,
    )?;

    Ok(())
}

pub fn sync_apple_workspace<F>(
    workspace_path: &Path,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    mut on_progress: F,
) -> Result<AppleWorkspaceSyncResult, ProviderError>
where
    F: FnMut(AppleProjectProgress),
{
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    let workspace_path = fs::canonicalize(workspace_path).map_err(|error| {
        ProviderError::GuestBridge(format!("the approved project is unavailable: {error}"))
    })?;
    if !workspace_path.is_dir()
        || !workspace_path.join("package.json").is_file()
        || !workspace_path.join("ios/App/Podfile").is_file()
        || !workspace_path.join("ios/App/Podfile.lock").is_file()
        || !workspace_path.join("ios/App/App.xcworkspace").is_dir()
    {
        return Err(ProviderError::GuestBridge(
            "the approved project must contain package.json and ios/App/App.xcworkspace with a locked Podfile"
                .to_string(),
        ));
    }

    let started_at = Instant::now();
    on_progress(apple_progress(
        AppleProjectPhase::Snapshotting,
        0,
        0,
        started_at,
        "Inspecting the approved project and excluding local dependencies and secrets.",
        None,
    ));
    let (source_file_count, source_bytes) = inspect_snapshot_tree(&workspace_path)?;
    let temporary_archive = TemporaryArchive::new();
    create_workspace_archive(&workspace_path, &temporary_archive.0)?;
    let archive_bytes = fs::metadata(&temporary_archive.0)
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not inspect the source snapshot: {error}"))
        })?
        .len();
    let snapshot_sha256 = snapshot_sha256(&temporary_archive.0)?;

    let guest_root = format!("/Users/{username}/BuildBridge/workspaces");
    let guest_workspace = format!("{guest_root}/active");
    let guest_staging = format!("{guest_root}/active.incoming");
    let guest_archive = format!("{guest_root}/active.tar.gz");
    let prepare = format!(
        "/bin/mkdir -p '{guest_root}'; /bin/rm -rf '{guest_staging}'; /bin/rm -f '{guest_archive}'"
    );
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &prepare,
    )?;
    stream_workspace_archive(
        &temporary_archive.0,
        archive_bytes,
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &guest_archive,
        started_at,
        &mut on_progress,
    )?;

    on_progress(apple_progress(
        AppleProjectPhase::Extracting,
        archive_bytes,
        archive_bytes,
        started_at,
        "Extracting the bounded snapshot into the BuildBridge guest workspace.",
        None,
    ));
    let extract = format!(
        "set -eu; /bin/mkdir -p '{guest_staging}'; /usr/bin/tar -xzf '{guest_archive}' -C '{guest_staging}'; /bin/test -f '{guest_staging}/package.json'; /bin/test -d '{guest_staging}/ios/App/App.xcworkspace'; /bin/rm -rf '{guest_workspace}.previous'; if /bin/test -d '{guest_workspace}'; then /bin/mv '{guest_workspace}' '{guest_workspace}.previous'; fi; /bin/mv '{guest_staging}' '{guest_workspace}'; /bin/rm -f '{guest_archive}'; /bin/rm -rf '{guest_workspace}.previous'"
    );
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &extract,
    )?;

    Ok(AppleWorkspaceSyncResult {
        guest_path: guest_workspace,
        snapshot_sha256,
        source_file_count,
        source_bytes,
        archive_bytes,
    })
}

pub fn run_apple_smoke_build<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    mut on_progress: F,
) -> Result<AppleSmokeBuildResult, ProviderError>
where
    F: FnMut(AppleProjectProgress),
{
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    let started_at = Instant::now();
    let guest_home = format!("/Users/{username}");
    let workspace = format!("{guest_home}/BuildBridge/workspaces/active");
    let tools = format!("{guest_home}/.buildbridge/tools");
    let node_name = format!("node-v{NODE_VERSION}-darwin-x64");
    let node_root = format!("{tools}/{node_name}");
    let node_archive = format!("{tools}/{node_name}.tar.gz");
    let pnpm = format!("{tools}/pnpm/node_modules/.bin/pnpm");
    let gem_home = format!("{tools}/gems");
    let pod = format!("{gem_home}/bin/pod");
    let developer_dir = format!("{guest_home}/Applications/Xcode.app/Contents/Developer");

    let script = format!(
        r#"set -u
exec 2>&1
phase() {{ /usr/bin/printf '__BUILDBRIDGE_PHASE__:%s\n' "$1"; }}
job_root="{tools}/jobs"
job_state="$job_root/apple-smoke-build"
/bin/mkdir -p "$job_root"

job_owner=0
while /usr/bin/true; do
    if /bin/mkdir "$job_state" 2>/dev/null; then
        job_owner=1
        break
    fi
    if /bin/test -f "$job_state/status"; then
        break
    fi
    if /bin/test -f "$job_state/pid"; then
        job_pid=$(/bin/cat "$job_state/pid")
        if /bin/kill -0 "$job_pid" 2>/dev/null; then
            break
        fi
        /bin/rm -rf "$job_state"
        continue
    fi
    /bin/sleep 1
done

job_log="$job_state/output.log"
job_status="$job_state/status"
if /bin/test "$job_owner" -eq 1; then
    trap '' HUP
    (
        finish_job() {{
            worker_status=$?
            /usr/bin/printf '%s\n' "$worker_status" > "$job_status.incoming"
            /bin/mv "$job_status.incoming" "$job_status"
        }}
        trap finish_job EXIT
        set -eu
/bin/test -f "{workspace}/package.json"
/bin/test -d "{workspace}/ios/App/App.xcworkspace"
export PATH="{node_root}/bin:{tools}/pnpm/node_modules/.bin:{gem_home}/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export GEM_HOME="{gem_home}"
export GEM_PATH="{gem_home}"
export DEVELOPER_DIR="{developer_dir}"
export LANG="en_US.UTF-8"
export RUBYOPT="-rlogger"
export CYPRESS_INSTALL_BINARY=0

phase preparing_tools
/bin/mkdir -p "{tools}"
if /bin/test ! -x "{node_root}/bin/node"; then
    /bin/rm -rf "{node_root}" "{node_archive}"
    /usr/bin/curl --fail --location --show-error --silent "https://nodejs.org/dist/v{NODE_VERSION}/{node_name}.tar.gz" --output "{node_archive}"
    /usr/bin/shasum -a 256 "{node_archive}" | /usr/bin/grep -q "^{NODE_DARWIN_X64_SHA256}  "
    /usr/bin/tar -xzf "{node_archive}" -C "{tools}"
    /bin/rm -f "{node_archive}"
fi
if /bin/test ! -x "{pnpm}"; then
    "{node_root}/bin/npm" install --prefix "{tools}/pnpm" "pnpm@{PNPM_VERSION}" --no-audit --no-fund
fi
if /bin/test ! -x "{pod}"; then
    /usr/bin/gem install concurrent-ruby --version "{CONCURRENT_RUBY_VERSION}" --no-document --conservative --minimal-deps
    /usr/bin/gem install i18n --version "{I18N_VERSION}" --no-document --conservative --minimal-deps
    /usr/bin/gem install minitest --version "{MINITEST_VERSION}" --no-document --conservative --minimal-deps
    /usr/bin/gem install tzinfo --version "{TZINFO_VERSION}" --no-document --conservative --minimal-deps
    /usr/bin/gem install zeitwerk --version "{ZEITWERK_VERSION}" --no-document --conservative --minimal-deps
    /usr/bin/gem install activesupport --version "{ACTIVESUPPORT_VERSION}" --no-document --conservative --minimal-deps
    /usr/bin/gem install ffi --version "{FFI_VERSION}" --no-document --conservative --minimal-deps
    /usr/bin/gem install cocoapods --version "{COCOAPODS_VERSION}" --no-document --conservative --minimal-deps
    /bin/test -x "{pod}"
fi
platform_installed=0
if ! /usr/bin/xcrun simctl list runtimes 2>/dev/null | /usr/bin/grep -q '^iOS '; then
    phase preparing_platform
    platform_assets="/System/Library/AssetsV2/com_apple_MobileAsset_iOSSimulatorRuntime"
    platform_catalog="$platform_assets/com_apple_MobileAsset_iOSSimulatorRuntime.xml"
    platform_log="{tools}/ios-platform-install.log"
    platform_total=0
    /bin/rm -f "$platform_log"
    /usr/bin/xcodebuild -downloadPlatform iOS -architectureVariant universal > "$platform_log" 2>&1 &
    platform_pid=$!
    while /bin/kill -0 "$platform_pid" 2>/dev/null; do
        platform_total=0
        if /bin/test -f "$platform_catalog"; then
            platform_total=$(/usr/bin/plutil -p "$platform_catalog" 2>/dev/null | /usr/bin/awk '/"_DownloadSize"/ {{ if ($3 > max) max = $3 }} END {{ printf "%.0f", max }}')
        fi
        platform_current=0
        if /bin/test -d "$platform_assets"; then
            platform_current_kib=$(/usr/bin/du -sk "$platform_assets" 2>/dev/null | /usr/bin/awk '{{ print $1 }}')
            platform_current=$((platform_current_kib * 1024))
        fi
        platform_stage="locating"
        if /bin/test "$platform_total" -gt 0; then
            platform_stage="downloading"
            if /bin/test "$platform_current" -ge "$platform_total"; then
                platform_current="$platform_total"
                platform_stage="installing"
            fi
        fi
        /usr/bin/printf '__BUILDBRIDGE_PLATFORM_PROGRESS__:%s:%s:%s\n' "$platform_current" "$platform_total" "$platform_stage"
        /bin/sleep 2
    done
    platform_status=0
    wait "$platform_pid" || platform_status=$?
    /bin/cat "$platform_log"
    /bin/rm -f "$platform_log"
    /bin/test "$platform_status" -eq 0
    if /bin/test "$platform_total" -gt 0; then
        /usr/bin/printf '__BUILDBRIDGE_PLATFORM_PROGRESS__:%s:%s:installing\n' "$platform_total" "$platform_total"
    fi
    /usr/bin/xcrun simctl list runtimes | /usr/bin/grep -q '^iOS '
    platform_installed=1
fi

phase installing_dependencies
cd "{workspace}"
"{pnpm}" install --frozen-lockfile --prefer-offline

phase building_web_assets
"{workspace}/node_modules/.bin/vp" build

phase syncing_ios
lock_before=$(/usr/bin/shasum -a 256 "{workspace}/ios/App/Podfile.lock" | /usr/bin/cut -d ' ' -f 1)
"{workspace}/node_modules/.bin/cap" sync ios

phase resolving_pods
cd "{workspace}/ios/App"
"{pod}" install --no-ansi
lock_after=$(/usr/bin/shasum -a 256 "{workspace}/ios/App/Podfile.lock" | /usr/bin/cut -d ' ' -f 1)
if /bin/test "$lock_before" != "$lock_after"; then
    /usr/bin/printf '__BUILDBRIDGE_NATIVE_LOCK_UPDATED__:yes\n'
fi

phase building
build_ios() {{
    /usr/bin/xcodebuild -workspace "{workspace}/ios/App/App.xcworkspace" -scheme App -configuration Debug -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' -derivedDataPath "{workspace}/.buildbridge/DerivedData" CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO COMPILER_INDEX_STORE_ENABLE=NO build
}}
build_status=0
build_ios || build_status=$?
if /bin/test "$build_status" -ne 0 && /bin/test "$platform_installed" -eq 1; then
    /usr/bin/printf '__BUILDBRIDGE_BUILD_RETRY__:platform\n'
    readiness_attempt=0
    while /bin/test "$readiness_attempt" -lt 6; do
        if /usr/bin/xcrun simctl list runtimes 2>/dev/null | /usr/bin/grep -q '^iOS ' && /usr/bin/xcrun simctl list devices available >/dev/null 2>&1; then
            break
        fi
        readiness_attempt=$((readiness_attempt + 1))
        /bin/sleep 2
    done
    /bin/sleep 5
    build_status=0
    build_ios || build_status=$?
fi
/bin/test "$build_status" -eq 0
phase completed
    ) > "$job_log" 2>&1 < /dev/null &
    job_pid=$!
    /usr/bin/printf '%s\n' "$job_pid" > "$job_state/pid"
    trap - HUP
else
    /usr/bin/printf '__BUILDBRIDGE_REATTACHED__:yes\n'
fi

next_line=1
while ! /bin/test -f "$job_status"; do
    if /bin/test -f "$job_log"; then
        line_count=$(/usr/bin/wc -l < "$job_log" | /usr/bin/tr -d ' ')
        if /bin/test "$line_count" -ge "$next_line"; then
            /usr/bin/sed -n "$next_line,$line_count p" "$job_log"
            next_line=$((line_count + 1))
        fi
    fi
    /bin/sleep 1
done
if /bin/test -f "$job_log"; then
    line_count=$(/usr/bin/wc -l < "$job_log" | /usr/bin/tr -d ' ')
    if /bin/test "$line_count" -ge "$next_line"; then
        /usr/bin/sed -n "$next_line,$line_count p" "$job_log"
    fi
fi
job_result=$(/bin/cat "$job_status")
exit "$job_result"
"#
    );

    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start the Apple test build: {error}"))
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture Apple build output".to_string())
    })?;
    let mut phase = AppleProjectPhase::PreparingTools;
    let mut native_lockfile_updated = false;
    let mut output_tail = Vec::new();
    let mut diagnostic_lines = Vec::new();
    let mut last_event = Instant::now() - Duration::from_secs(1);
    on_progress(apple_progress(
        phase,
        0,
        0,
        started_at,
        phase_detail(phase),
        None,
    ));

    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| {
            ProviderError::GuestBridge(format!("could not read Apple build output: {error}"))
        })?;
        if let Some(value) = line.strip_prefix("__BUILDBRIDGE_PHASE__:") {
            phase = apple_project_phase(value).ok_or_else(|| {
                ProviderError::GuestBridge("the guest returned an unknown build phase".to_string())
            })?;
            on_progress(apple_progress(
                phase,
                0,
                0,
                started_at,
                phase_detail(phase),
                None,
            ));
            last_event = Instant::now();
            continue;
        }
        if line == "__BUILDBRIDGE_NATIVE_LOCK_UPDATED__:yes" {
            native_lockfile_updated = true;
            let message = "The guest-only Podfile.lock was refreshed to match the synchronized native plugins.".to_string();
            output_tail.push(message.clone());
            on_progress(apple_progress(
                phase,
                0,
                0,
                started_at,
                phase_detail(phase),
                Some(message),
            ));
            continue;
        }
        if line == "__BUILDBRIDGE_REATTACHED__:yes" {
            let message = "Reattached to the build already running inside macOS.".to_string();
            output_tail.push(message.clone());
            on_progress(apple_progress(
                phase,
                0,
                0,
                started_at,
                "Reconnecting to the active guest build",
                Some(message),
            ));
            continue;
        }
        if let Some(retry_detail) = apple_build_retry_detail(&line) {
            let message = "The first compile started before the new Simulator runtime had settled. BuildBridge is verifying CoreSimulator and retrying once.".to_string();
            output_tail.push(message.clone());
            on_progress(apple_progress(
                AppleProjectPhase::Building,
                0,
                0,
                started_at,
                retry_detail,
                Some(message),
            ));
            last_event = Instant::now();
            continue;
        }
        if let Some((completed_bytes, total_bytes, detail)) = apple_platform_progress(&line) {
            phase = AppleProjectPhase::PreparingPlatform;
            on_progress(apple_progress(
                phase,
                completed_bytes,
                total_bytes,
                started_at,
                detail,
                None,
            ));
            last_event = Instant::now();
            continue;
        }

        let line = sanitize_build_log_line(&line);
        if line.is_empty() {
            continue;
        }
        let is_diagnostic = apple_build_log_is_diagnostic(&line);
        if is_diagnostic {
            diagnostic_lines.push(line.clone());
            if diagnostic_lines.len() > APPLE_BUILD_DIAGNOSTIC_LINES {
                diagnostic_lines.remove(0);
            }
        }
        output_tail.push(line.clone());
        if output_tail.len() > APPLE_BUILD_OUTPUT_TAIL_LINES {
            output_tail.remove(0);
        }
        if is_diagnostic || last_event.elapsed() >= Duration::from_millis(100) {
            on_progress(apple_progress(
                phase,
                0,
                0,
                started_at,
                phase_detail(phase),
                Some(line),
            ));
            last_event = Instant::now();
        }
    }

    let status = child.wait().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish the Apple test build: {error}"))
    })?;
    if status.code() != Some(255) {
        let cleanup = format!("/bin/rm -rf '{tools}/jobs/apple-smoke-build'");
        let _ = run_guest_command(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &cleanup,
        );
    }
    if !status.success() {
        let context = if diagnostic_lines.is_empty() {
            output_tail
                .iter()
                .rev()
                .take(12)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            diagnostic_lines.join("\n")
        };
        return Err(ProviderError::GuestBridge(if context.is_empty() {
            format!("the Apple test build failed during {}", phase_detail(phase))
        } else {
            format!(
                "the Apple test build failed during {}:\n{context}",
                phase_detail(phase)
            )
        }));
    }

    let xcode_version = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        "/usr/bin/xcodebuild -version",
    )?
    .lines()
    .next()
    .unwrap_or("Xcode")
    .to_string();
    on_progress(apple_progress(
        AppleProjectPhase::Completed,
        0,
        0,
        started_at,
        "The unsigned iOS Simulator build completed successfully.",
        None,
    ));

    Ok(AppleSmokeBuildResult {
        xcode_version,
        native_lockfile_updated,
        output_tail,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn run_signed_apple_archive<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    signing: &SigningProvisioningResult,
    scheme: &str,
    keychain_password: &str,
    output_directory: &Path,
    mut on_progress: F,
) -> Result<AppleArchiveResult, ProviderError>
where
    F: FnMut(AppleArchiveProgress),
{
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    validate_signing_target(&signing.development_team, &signing.bundle_identifier)?;
    if !valid_apple_scheme(scheme) {
        return Err(ProviderError::GuestBridge(
            "the stored Xcode scheme is missing or invalid".to_string(),
        ));
    }
    if keychain_password.is_empty() || keychain_password.len() > 512 {
        return Err(ProviderError::GuestBridge(
            "the signing keychain credential is missing or invalid".to_string(),
        ));
    }
    if signing.identity_sha1.len() != 40
        || !signing
            .identity_sha1
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(ProviderError::GuestBridge(
            "the provisioned signing identity is invalid".to_string(),
        ));
    }
    let profile = signing
        .profiles
        .iter()
        .find(|profile| {
            valid_profile_uuid(&profile.uuid)
                && profile.team_identifier == signing.development_team
                && profile_allows_bundle(
                    &profile.application_identifier,
                    &signing.bundle_identifier,
                )
                && profile
                    .developer_certificate_sha256
                    .iter()
                    .any(|fingerprint| fingerprint == &signing.certificate_sha256)
        })
        .ok_or_else(|| {
            ProviderError::GuestBridge(
                "no installed App Store profile matches the provisioned identity and project"
                    .to_string(),
            )
        })?;
    let output_directory = validate_archive_output_directory(output_directory)?;
    let expected_keychain_path =
        format!("/Users/{username}/Library/Keychains/{SIGNING_KEYCHAIN_NAME}");
    if signing.keychain_path != expected_keychain_path {
        return Err(ProviderError::GuestBridge(
            "the provisioned signing keychain path is invalid".to_string(),
        ));
    }
    let ipa_path = output_directory.join(format!("{scheme}-AppStore.ipa"));
    let archive_path = output_directory.join(format!("{scheme}.xcarchive.zip"));
    let ipa_partial_path = ipa_path.with_extension("ipa.part");
    let archive_partial_path = archive_path.with_extension("zip.part");
    for path in [
        &ipa_path,
        &archive_path,
        &ipa_partial_path,
        &archive_partial_path,
    ] {
        if path.exists() {
            return Err(ProviderError::GuestBridge(
                "the signed artifact destination is not empty".to_string(),
            ));
        }
    }

    let started_at = Instant::now();
    let guest_home = format!("/Users/{username}");
    let guest_tools = format!("{guest_home}/.buildbridge/tools");
    let helper_source = format!("{guest_tools}/signing-helper.c");
    let helper_binary = format!("{guest_tools}/signing-helper");
    let xcodebuild =
        format!("{guest_home}/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild");
    let workspace = format!("{guest_home}/BuildBridge/workspaces/active/ios/App/App.xcworkspace");
    let operation_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let staging =
        format!("{guest_home}/Library/Caches/dev.buildbridge.desktop/archive-{operation_id}");
    let guest_archive = format!("{staging}/{scheme}.xcarchive");
    let guest_archive_zip = format!("{staging}/{scheme}.xcarchive.zip");
    let guest_derived_data = format!("{staging}/DerivedData");
    let guest_signing_settings = format!("{staging}/Signing.xcconfig");
    let guest_export_options = format!("{staging}/ExportOptions.plist");
    let guest_export = format!("{staging}/Export");
    let export_options = apple_export_options_plist(
        &signing.development_team,
        &signing.bundle_identifier,
        &profile.uuid,
        &signing.identity_sha1,
    );

    on_progress(archive_progress(
        AppleArchivePhase::Preparing,
        0,
        0,
        started_at,
        "Preparing the fixed Release and App Store Connect export recipe.",
        None,
    ));
    let prepare = format!(
        "set -eu; /bin/mkdir -p {} {}; /bin/chmod 700 {}; /bin/rm -rf {}; /bin/mkdir -p {} {}; /bin/chmod 700 {}",
        shell_single_quote(&guest_tools),
        shell_single_quote(&format!(
            "{guest_home}/Library/Caches/dev.buildbridge.desktop"
        )),
        shell_single_quote(&guest_tools),
        shell_single_quote(&staging),
        shell_single_quote(&staging),
        shell_single_quote(&guest_export),
        shell_single_quote(&staging),
    );
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &prepare,
    )?;

    let operation = (|| {
        let archive_target = resolve_archive_app_target(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &xcodebuild,
            &workspace,
            scheme,
            &signing.bundle_identifier,
        )?;
        let signing_settings = apple_archive_signing_xcconfig(
            &archive_target,
            &signing.development_team,
            &signing.identity_sha1,
            &profile.uuid,
        );
        stream_bytes_to_guest(
            SIGNING_HELPER_SOURCE,
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &helper_source,
            "signing helper",
        )?;
        stream_bytes_to_guest(
            signing_settings.as_bytes(),
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_signing_settings,
            "target-scoped signing settings",
        )?;
        stream_bytes_to_guest(
            export_options.as_bytes(),
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_export_options,
            "App Store export options",
        )?;
        let compile = format!(
            "set -eu; /usr/bin/xcrun --sdk macosx clang -std=c11 -O2 -Wno-deprecated-declarations {} -framework Security -framework CoreFoundation -o {}; /bin/chmod 700 {}",
            shell_single_quote(&helper_source),
            shell_single_quote(&helper_binary),
            shell_single_quote(&helper_binary),
        );
        run_guest_command(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &compile,
        )?;

        let mut output_tail = run_archive_helper(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &helper_binary,
            &signing.keychain_path,
            &xcodebuild,
            &workspace,
            scheme,
            &guest_archive,
            &guest_derived_data,
            &guest_signing_settings,
            &guest_export_options,
            &guest_export,
            keychain_password,
            started_at,
            &mut on_progress,
        )?;

        on_progress(archive_progress(
            AppleArchivePhase::Verifying,
            0,
            0,
            started_at,
            "Verifying the archived app signature and reading release metadata.",
            None,
        ));
        let mut inspection = inspect_apple_archive(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_archive,
            &guest_export,
        )?;
        if inspection.bundle_identifier != signing.bundle_identifier {
            return Err(ProviderError::GuestBridge(format!(
                "the exported archive uses bundle identifier {}, not {}",
                inspection.bundle_identifier, signing.bundle_identifier
            )));
        }
        on_progress(archive_progress(
            AppleArchivePhase::PackagingArchive,
            0,
            0,
            started_at,
            "The signature is valid; packaging the portable Xcode archive.",
            None,
        ));
        let (archive_bytes, archive_sha256) = package_apple_archive(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_archive,
            &guest_archive_zip,
        )?;
        inspection.archive_bytes = archive_bytes;
        inspection.archive_sha256 = archive_sha256;

        let total_bytes = inspection
            .ipa_bytes
            .checked_add(inspection.archive_bytes)
            .ok_or_else(|| {
                ProviderError::GuestBridge("the signed artifact sizes are invalid".to_string())
            })?;
        if inspection.ipa_bytes == 0
            || inspection.archive_bytes == 0
            || inspection.ipa_bytes > APPLE_ARCHIVE_MAX_BYTES
            || inspection.archive_bytes > APPLE_ARCHIVE_MAX_BYTES
            || total_bytes > APPLE_ARCHIVE_MAX_BYTES
        {
            return Err(ProviderError::GuestBridge(
                "the signed artifacts exceed the 20 GiB transfer limit".to_string(),
            ));
        }
        let guest_ipa = format!("{guest_export}/{}", inspection.ipa_name);
        stream_guest_artifact(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_ipa,
            &ipa_partial_path,
            inspection.ipa_bytes,
            0,
            total_bytes,
            "Transferring the signed IPA to this host.",
            started_at,
            &mut on_progress,
        )?;
        stream_guest_artifact(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_archive_zip,
            &archive_partial_path,
            inspection.archive_bytes,
            inspection.ipa_bytes,
            total_bytes,
            "Transferring the portable Xcode archive to this host.",
            started_at,
            &mut on_progress,
        )?;
        verify_local_artifact(
            &ipa_partial_path,
            inspection.ipa_bytes,
            &inspection.ipa_sha256,
        )?;
        verify_local_artifact(
            &archive_partial_path,
            inspection.archive_bytes,
            &inspection.archive_sha256,
        )?;
        fs::rename(&ipa_partial_path, &ipa_path).map_err(|error| {
            ProviderError::GuestBridge(format!("could not retain the signed IPA: {error}"))
        })?;
        fs::rename(&archive_partial_path, &archive_path).map_err(|error| {
            ProviderError::GuestBridge(format!("could not retain the Xcode archive: {error}"))
        })?;
        set_artifact_permissions(&ipa_path)?;
        set_artifact_permissions(&archive_path)?;
        output_tail.push(format!(
            "Verified signed {} ({}) and retained both local artifacts.",
            inspection.bundle_identifier, inspection.marketing_version
        ));
        if output_tail.len() > APPLE_BUILD_OUTPUT_TAIL_LINES {
            output_tail.remove(0);
        }

        Ok(AppleArchiveResult {
            scheme: scheme.to_string(),
            configuration: "Release".to_string(),
            export_method: "app-store-connect".to_string(),
            bundle_identifier: inspection.bundle_identifier,
            development_team: signing.development_team.clone(),
            marketing_version: inspection.marketing_version,
            build_number: inspection.build_number,
            provisioning_profile_uuid: profile.uuid.clone(),
            ipa: AppleArchiveArtifact {
                path: ipa_path.display().to_string(),
                bytes: inspection.ipa_bytes,
                sha256: inspection.ipa_sha256,
            },
            archive: AppleArchiveArtifact {
                path: archive_path.display().to_string(),
                bytes: inspection.archive_bytes,
                sha256: inspection.archive_sha256,
            },
            output_tail,
        })
    })();

    let _ = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!("/bin/rm -rf {}", shell_single_quote(&staging)),
    );
    if operation.is_err() {
        for path in [
            &ipa_path,
            &archive_path,
            &ipa_partial_path,
            &archive_partial_path,
        ] {
            let _ = fs::remove_file(path);
        }
    }
    let result = operation?;
    on_progress(archive_progress(
        AppleArchivePhase::Completed,
        result.ipa.bytes + result.archive.bytes,
        result.ipa.bytes + result.archive.bytes,
        started_at,
        "Signed archive and IPA verified and retained on this host.",
        None,
    ));

    Ok(result)
}

fn valid_apple_scheme(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, ' ' | '-' | '_' | '.')
        })
}

fn validate_archive_output_directory(path: &Path) -> Result<PathBuf, ProviderError> {
    if !path.is_absolute() {
        return Err(ProviderError::GuestBridge(
            "the signed artifact destination must be an absolute directory".to_string(),
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ProviderError::GuestBridge(format!(
            "the signed artifact destination is unavailable: {error}"
        ))
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ProviderError::GuestBridge(
            "the signed artifact destination must be a real directory".to_string(),
        ));
    }

    fs::canonicalize(path).map_err(|error| {
        ProviderError::GuestBridge(format!(
            "could not resolve the signed artifact destination: {error}"
        ))
    })
}

fn apple_export_options_plist(
    development_team: &str,
    bundle_identifier: &str,
    profile_uuid: &str,
    identity_sha1: &str,
) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>destination</key>
    <string>export</string>
    <key>manageAppVersionAndBuildNumber</key>
    <false/>
    <key>method</key>
    <string>app-store-connect</string>
    <key>provisioningProfiles</key>
    <dict>
        <key>{bundle_identifier}</key>
        <string>{profile_uuid}</string>
    </dict>
    <key>signingCertificate</key>
    <string>{identity_sha1}</string>
    <key>signingStyle</key>
    <string>manual</string>
    <key>stripSwiftSymbols</key>
    <true/>
    <key>teamID</key>
    <string>{development_team}</string>
    <key>uploadSymbols</key>
    <true/>
</dict>
</plist>
"#
    )
}

fn archive_progress(
    phase: AppleArchivePhase,
    completed_bytes: u64,
    total_bytes: u64,
    started_at: Instant,
    detail: &str,
    log_line: Option<String>,
) -> AppleArchiveProgress {
    AppleArchiveProgress {
        phase,
        completed_bytes,
        total_bytes,
        elapsed_seconds: started_at.elapsed().as_secs(),
        detail: detail.to_string(),
        log_line,
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_archive_app_target(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    xcodebuild_path: &str,
    workspace_path: &str,
    scheme: &str,
    expected_bundle_identifier: &str,
) -> Result<String, ProviderError> {
    let command = format!(
        "set -o pipefail; {} -workspace {} -scheme {} -configuration Release -destination 'generic/platform=iOS' -showBuildSettings | /usr/bin/awk '$1 == \"TARGET_NAME\" || $1 == \"PRODUCT_BUNDLE_IDENTIFIER\" {{ print }}'",
        shell_single_quote(xcodebuild_path),
        shell_single_quote(workspace_path),
        shell_single_quote(scheme),
    );
    let output = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &command,
    )?;
    let target = build_setting_value(&output, "TARGET_NAME").ok_or_else(|| {
        ProviderError::GuestBridge(
            "Xcode did not return the application target for the selected scheme".to_string(),
        )
    })?;
    let bundle_identifier =
        build_setting_value(&output, "PRODUCT_BUNDLE_IDENTIFIER").ok_or_else(|| {
            ProviderError::GuestBridge(
                "Xcode did not return the application bundle identifier for the selected scheme"
                    .to_string(),
            )
        })?;
    if bundle_identifier != expected_bundle_identifier {
        return Err(ProviderError::GuestBridge(format!(
            "the selected Xcode scheme resolves to bundle identifier {bundle_identifier}, not {expected_bundle_identifier}"
        )));
    }
    if target.is_empty()
        || target.len() > 128
        || !target
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(ProviderError::GuestBridge(
            "the application target name cannot be represented safely in target-scoped signing settings"
                .to_string(),
        ));
    }
    Ok(target.to_string())
}

fn build_setting_value<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output.lines().find_map(|line| {
        let (candidate, value) = line.trim().split_once(" = ")?;
        (candidate == key).then_some(value.trim())
    })
}

fn apple_archive_signing_xcconfig(
    target: &str,
    development_team: &str,
    identity_sha1: &str,
    profile_uuid: &str,
) -> String {
    format!(
        "BUILDBRIDGE_TEAM_{target} = {development_team}\n\
BUILDBRIDGE_IDENTITY_{target} = {identity_sha1}\n\
BUILDBRIDGE_PROFILE_{target} = {profile_uuid}\n\
BUILDBRIDGE_STYLE_{target} = Manual\n\
DEVELOPMENT_TEAM = $(BUILDBRIDGE_TEAM_$(TARGET_NAME))\n\
CODE_SIGN_IDENTITY = $(BUILDBRIDGE_IDENTITY_$(TARGET_NAME))\n\
PROVISIONING_PROFILE_SPECIFIER = $(BUILDBRIDGE_PROFILE_$(TARGET_NAME))\n\
CODE_SIGN_STYLE = $(BUILDBRIDGE_STYLE_$(TARGET_NAME))\n"
    )
}

#[allow(clippy::too_many_arguments)]
fn run_archive_helper<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    helper_path: &str,
    keychain_path: &str,
    xcodebuild_path: &str,
    workspace_path: &str,
    scheme: &str,
    archive_path: &str,
    derived_data_path: &str,
    signing_settings_path: &str,
    export_options_path: &str,
    export_path: &str,
    keychain_password: &str,
    started_at: Instant,
    on_progress: &mut F,
) -> Result<Vec<String>, ProviderError>
where
    F: FnMut(AppleArchiveProgress),
{
    let remote_command = format!(
        "{} --archive {} {} {} {} {} {} {} {} {} 2>&1",
        shell_single_quote(helper_path),
        shell_single_quote(keychain_path),
        shell_single_quote(xcodebuild_path),
        shell_single_quote(workspace_path),
        shell_single_quote(scheme),
        shell_single_quote(archive_path),
        shell_single_quote(derived_data_path),
        shell_single_quote(signing_settings_path),
        shell_single_quote(export_options_path),
        shell_single_quote(export_path),
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start the signed archive: {error}"))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not open protected archive input".to_string())
    })?;
    write_secret_frame(&mut stdin, keychain_password)?;
    drop(stdin);
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture signed archive output".to_string())
    })?;
    let mut phase = AppleArchivePhase::Archiving;
    let mut output_tail = Vec::new();
    let mut diagnostic_lines = Vec::new();
    let mut last_event = Instant::now() - Duration::from_secs(1);

    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| {
            ProviderError::GuestBridge(format!("could not read signed archive output: {error}"))
        })?;
        if let Some(marker) = line.strip_prefix("__BUILDBRIDGE_ARCHIVE__:") {
            phase = match marker {
                "archiving" => AppleArchivePhase::Archiving,
                "exporting" => AppleArchivePhase::Exporting,
                "complete" => AppleArchivePhase::Verifying,
                _ => {
                    return Err(ProviderError::GuestBridge(
                        "macOS returned an unknown archive phase".to_string(),
                    ));
                }
            };
            on_progress(archive_progress(
                phase,
                0,
                0,
                started_at,
                archive_phase_detail(phase),
                None,
            ));
            last_event = Instant::now();
            continue;
        }

        let line = sanitize_build_log_line(&line);
        if line.is_empty() {
            continue;
        }
        if apple_archive_log_is_diagnostic(&line) {
            diagnostic_lines.push(line.clone());
            if diagnostic_lines.len() > APPLE_BUILD_DIAGNOSTIC_LINES {
                diagnostic_lines.remove(0);
            }
        }
        output_tail.push(line.clone());
        if output_tail.len() > APPLE_BUILD_OUTPUT_TAIL_LINES {
            output_tail.remove(0);
        }
        if apple_archive_log_is_diagnostic(&line)
            || last_event.elapsed() >= Duration::from_millis(100)
        {
            on_progress(archive_progress(
                phase,
                0,
                0,
                started_at,
                archive_phase_detail(phase),
                Some(line),
            ));
            last_event = Instant::now();
        }
    }

    let status = child.wait().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish the signed archive: {error}"))
    })?;
    if !status.success() {
        let context = if diagnostic_lines.is_empty() {
            output_tail
                .iter()
                .rev()
                .take(12)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            diagnostic_lines.join("\n")
        };
        return Err(ProviderError::GuestBridge(if context.is_empty() {
            format!(
                "the signed archive failed during {}",
                archive_phase_detail(phase)
            )
        } else {
            format!(
                "the signed archive failed during {}:\n{context}",
                archive_phase_detail(phase)
            )
        }));
    }

    Ok(output_tail)
}

fn archive_phase_detail(phase: AppleArchivePhase) -> &'static str {
    match phase {
        AppleArchivePhase::Preparing => "Preparing the signed Release recipe",
        AppleArchivePhase::Archiving => "Compiling and signing the Release archive",
        AppleArchivePhase::Exporting => "Exporting the App Store Connect IPA",
        AppleArchivePhase::Verifying => "Verifying the archived app signature",
        AppleArchivePhase::PackagingArchive => "Packaging the portable Xcode archive",
        AppleArchivePhase::Transferring => "Transferring verified artifacts to this host",
        AppleArchivePhase::Completed => "Signed archive and IPA complete",
    }
}

fn apple_archive_log_is_diagnostic(line: &str) -> bool {
    let normalized = line.to_ascii_lowercase();

    apple_build_log_is_diagnostic(line)
        || normalized.contains("provisioning profile")
        || normalized.contains("requires a provisioning profile")
        || normalized.contains("requires a development team")
        || normalized.contains("code signing")
        || normalized.contains("codesign")
        || normalized.contains("xcode_archive_failed")
        || normalized.contains("xcode_export_failed")
        || normalized.starts_with("error: exportarchive")
        || normalized.starts_with("** archive failed **")
        || normalized.starts_with("** export failed **")
}

#[derive(Debug)]
struct AppleArchiveInspection {
    bundle_identifier: String,
    marketing_version: String,
    build_number: String,
    ipa_name: String,
    ipa_bytes: u64,
    ipa_sha256: String,
    archive_bytes: u64,
    archive_sha256: String,
}

fn inspect_apple_archive(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    archive_path: &str,
    export_path: &str,
) -> Result<AppleArchiveInspection, ProviderError> {
    let archive = shell_single_quote(archive_path);
    let export = shell_single_quote(export_path);
    let command = format!(
        "set -eu; app_relative=$(/usr/bin/plutil -extract ApplicationProperties.ApplicationPath raw -o - {archive}/Info.plist); app_name=${{app_relative#Applications/}}; case \"$app_relative\" in Applications/*.app) ;; *) /usr/bin/printf 'invalid_archive_app_path' >&2; exit 1;; esac; case \"$app_name\" in ''|*/*|*..*) /usr/bin/printf 'invalid_archive_app_name' >&2; exit 1;; esac; app_path={archive}/Products/\"$app_relative\"; /usr/bin/codesign --verify --deep --strict --verbose=2 \"$app_path\"; bundle=$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - \"$app_path/Info.plist\"); version=$(/usr/bin/plutil -extract CFBundleShortVersionString raw -o - \"$app_path/Info.plist\"); build=$(/usr/bin/plutil -extract CFBundleVersion raw -o - \"$app_path/Info.plist\"); ipa_count=$(/usr/bin/find {export} -maxdepth 1 -type f -name '*.ipa' | /usr/bin/wc -l | /usr/bin/tr -d ' '); /bin/test \"$ipa_count\" -eq 1; ipa=$(/usr/bin/find {export} -maxdepth 1 -type f -name '*.ipa'); ipa_name=$(/usr/bin/basename \"$ipa\"); case \"$ipa_name\" in ''|*/*|*..*) /usr/bin/printf 'invalid_ipa_name' >&2; exit 1;; esac; ipa_bytes=$(/usr/bin/stat -f %z \"$ipa\"); ipa_sha=$(/usr/bin/shasum -a 256 \"$ipa\" | /usr/bin/awk '{{print $1}}'); /usr/bin/printf '__BUILDBRIDGE_APP__\\t%s\\t%s\\t%s\\n__BUILDBRIDGE_IPA__\\t%s\\t%s\\t%s\\n' \"$bundle\" \"$version\" \"$build\" \"$ipa_name\" \"$ipa_bytes\" \"$ipa_sha\""
    );
    let output = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &command,
    )?;
    parse_apple_archive_inspection(&output)
}

fn parse_apple_archive_inspection(output: &str) -> Result<AppleArchiveInspection, ProviderError> {
    let mut app = None;
    let mut ipa = None;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("__BUILDBRIDGE_APP__\t") {
            let fields = value.split('\t').collect::<Vec<_>>();
            if fields.len() == 3 {
                app = Some((fields[0], fields[1], fields[2]));
            }
        } else if let Some(value) = line.strip_prefix("__BUILDBRIDGE_IPA__\t") {
            let fields = value.split('\t').collect::<Vec<_>>();
            if fields.len() == 3 {
                ipa = Some((fields[0], fields[1], fields[2]));
            }
        }
    }
    let (bundle_identifier, marketing_version, build_number) = app.ok_or_else(|| {
        ProviderError::GuestBridge("macOS returned incomplete archive metadata".to_string())
    })?;
    validate_signing_target("TEAM", bundle_identifier)?;
    if !valid_release_value(marketing_version) || !valid_release_value(build_number) {
        return Err(ProviderError::GuestBridge(
            "macOS returned invalid release version metadata".to_string(),
        ));
    }
    let (ipa_name, ipa_bytes, ipa_sha256) = ipa.ok_or_else(|| {
        ProviderError::GuestBridge("macOS did not return exactly one exported IPA".to_string())
    })?;
    if !valid_artifact_name(ipa_name) || !valid_sha256(ipa_sha256) {
        return Err(ProviderError::GuestBridge(
            "macOS returned invalid IPA metadata".to_string(),
        ));
    }
    let ipa_bytes = ipa_bytes.parse::<u64>().map_err(|_| {
        ProviderError::GuestBridge("macOS returned an invalid IPA size".to_string())
    })?;

    Ok(AppleArchiveInspection {
        bundle_identifier: bundle_identifier.to_string(),
        marketing_version: marketing_version.to_string(),
        build_number: build_number.to_string(),
        ipa_name: ipa_name.to_string(),
        ipa_bytes,
        ipa_sha256: ipa_sha256.to_ascii_uppercase(),
        archive_bytes: 0,
        archive_sha256: String::new(),
    })
}

fn package_apple_archive(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    archive_path: &str,
    archive_zip_path: &str,
) -> Result<(u64, String), ProviderError> {
    let archive = shell_single_quote(archive_path);
    let archive_zip = shell_single_quote(archive_zip_path);
    let command = format!(
        "set -eu; /bin/rm -f {archive_zip}; /usr/bin/ditto -c -k --sequesterRsrc --keepParent {archive} {archive_zip}; archive_bytes=$(/usr/bin/stat -f %z {archive_zip}); archive_sha=$(/usr/bin/shasum -a 256 {archive_zip} | /usr/bin/awk '{{print $1}}'); /usr/bin/printf '%s\\t%s\\n' \"$archive_bytes\" \"$archive_sha\""
    );
    let output = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &command,
    )?;
    let mut fields = output.split('\t');
    let bytes = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            ProviderError::GuestBridge("macOS returned an invalid archive size".to_string())
        })?;
    let sha256 = fields.next().unwrap_or_default();
    if fields.next().is_some() || !valid_sha256(sha256) {
        return Err(ProviderError::GuestBridge(
            "macOS returned invalid archive metadata".to_string(),
        ));
    }

    Ok((bytes, sha256.to_ascii_uppercase()))
}

fn valid_release_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
}

fn valid_artifact_name(value: &str) -> bool {
    value.len() > 4
        && value.len() <= 255
        && value.to_ascii_lowercase().ends_with(".ipa")
        && !value.contains("..")
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, ' ' | '.' | '-' | '_')
        })
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit())
}

#[allow(clippy::too_many_arguments)]
fn stream_guest_artifact<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    guest_path: &str,
    local_path: &Path,
    expected_bytes: u64,
    completed_offset: u64,
    total_bytes: u64,
    detail: &str,
    started_at: Instant,
    on_progress: &mut F,
) -> Result<(), ProviderError>
where
    F: FnMut(AppleArchiveProgress),
{
    let remote_command = format!("set -eu; /bin/cat {}", shell_single_quote(guest_path));
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start artifact transfer: {error}"))
        })?;
    let mut stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not read the signed artifact transfer".to_string())
    })?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(local_path)
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not create the local artifact: {error}"))
        })?;
    set_artifact_permissions(local_path)?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut copied = 0_u64;
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    loop {
        let count = stdout.read(&mut buffer).map_err(|error| {
            ProviderError::GuestBridge(format!("could not read the signed artifact: {error}"))
        })?;
        if count == 0 {
            break;
        }
        copied = copied.checked_add(count as u64).ok_or_else(|| {
            ProviderError::GuestBridge("the signed artifact size overflowed".to_string())
        })?;
        if copied > expected_bytes {
            return Err(ProviderError::GuestBridge(
                "macOS sent more artifact data than declared".to_string(),
            ));
        }
        file.write_all(&buffer[..count]).map_err(|error| {
            ProviderError::GuestBridge(format!("could not retain the signed artifact: {error}"))
        })?;
        if last_progress.elapsed() >= Duration::from_millis(200) || copied == expected_bytes {
            on_progress(archive_progress(
                AppleArchivePhase::Transferring,
                completed_offset + copied,
                total_bytes,
                started_at,
                detail,
                None,
            ));
            last_progress = Instant::now();
        }
    }
    file.sync_all().map_err(|error| {
        ProviderError::GuestBridge(format!("could not flush the signed artifact: {error}"))
    })?;
    drop(file);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish artifact transfer: {error}"))
    })?;
    if !output.status.success() {
        return Err(ProviderError::GuestBridge(format!(
            "the signed artifact transfer failed: {}",
            clean_output(&output.stderr)
        )));
    }
    if copied != expected_bytes {
        return Err(ProviderError::GuestBridge(format!(
            "the signed artifact transfer was incomplete ({copied} of {expected_bytes} bytes)"
        )));
    }

    Ok(())
}

fn verify_local_artifact(
    path: &Path,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<(), ProviderError> {
    let metadata = fs::metadata(path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not inspect the local artifact: {error}"))
    })?;
    if !metadata.is_file() || metadata.len() != expected_bytes {
        return Err(ProviderError::GuestBridge(
            "the local artifact size does not match macOS".to_string(),
        ));
    }
    let mut file = File::open(path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not open the local artifact: {error}"))
    })?;
    let mut signature = [0_u8; 4];
    file.read_exact(&mut signature).map_err(|error| {
        ProviderError::GuestBridge(format!("could not read the local artifact: {error}"))
    })?;
    if signature != *b"PK\x03\x04" {
        return Err(ProviderError::GuestBridge(
            "the transferred artifact is not a valid ZIP container".to_string(),
        ));
    }
    let actual_sha256 = snapshot_sha256(path)?;
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err(ProviderError::GuestBridge(
            "the transferred artifact checksum does not match macOS".to_string(),
        ));
    }

    Ok(())
}

fn set_artifact_permissions(path: &Path) -> Result<(), ProviderError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not restrict signed artifact permissions: {error}"
            ))
        })?;
    }

    Ok(())
}

struct TemporaryArchive(PathBuf);

impl TemporaryArchive {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "buildbridge-workspace-{}-{nonce}.tar.gz",
            std::process::id()
        )))
    }
}

impl Drop for TemporaryArchive {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn validate_guest_operation(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
    if !valid_guest_username(username) {
        return Err(ProviderError::GuestBridge(
            "the macOS short username is invalid".to_string(),
        ));
    }
    if ssh_port < 1024 || !identity_path.is_file() || !known_hosts_path.is_file() {
        return Err(ProviderError::GuestBridge(
            "the pinned macOS guest connection is not configured".to_string(),
        ));
    }

    Ok(())
}

fn inspect_snapshot_tree(root: &Path) -> Result<(u64, u64), ProviderError> {
    fn visit(
        root: &Path,
        directory: &Path,
        files: &mut u64,
        bytes: &mut u64,
    ) -> Result<(), ProviderError> {
        for entry in fs::read_dir(directory).map_err(|error| {
            ProviderError::GuestBridge(format!("could not inspect the approved project: {error}"))
        })? {
            let entry = entry.map_err(|error| {
                ProviderError::GuestBridge(format!("could not inspect a project entry: {error}"))
            })?;
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap_or(&path);
            if snapshot_path_excluded(relative) {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                ProviderError::GuestBridge(format!("could not inspect {}: {error}", path.display()))
            })?;
            if metadata.file_type().is_symlink() {
                return Err(ProviderError::GuestBridge(format!(
                    "the project contains a symbolic link outside excluded dependency folders: {}",
                    relative.display()
                )));
            }
            if metadata.is_dir() {
                visit(root, &path, files, bytes)?;
            } else if metadata.is_file() {
                *files += 1;
                *bytes = bytes.saturating_add(metadata.len());
                if *files > APPLE_WORKSPACE_MAX_FILES || *bytes > APPLE_WORKSPACE_MAX_BYTES {
                    return Err(ProviderError::GuestBridge(
                        "the project snapshot exceeds the 50,000 file or 2 GiB safety limit"
                            .to_string(),
                    ));
                }
            } else {
                return Err(ProviderError::GuestBridge(format!(
                    "the project contains an unsupported filesystem entry: {}",
                    relative.display()
                )));
            }
        }

        Ok(())
    }

    let mut files = 0;
    let mut bytes = 0;
    visit(root, root, &mut files, &mut bytes)?;
    Ok((files, bytes))
}

fn snapshot_path_excluded(relative: &Path) -> bool {
    if [
        "android/build",
        "android/app/build",
        "android/capacitor-cordova-android-plugins/build",
        "ios/App/build",
        "ios/App/Pods",
    ]
    .iter()
    .any(|path| relative.starts_with(path))
    {
        return true;
    }

    let excluded_directory = relative.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(
                ".git"
                    | ".ssh"
                    | ".buildbridge"
                    | ".pnpm-store"
                    | "node_modules"
                    | "dist"
                    | "coverage"
                    | "DerivedData"
                    | "xcuserdata"
                    | ".idea"
                    | ".vscode"
                    | ".gradle"
            )
        )
    });
    if excluded_directory {
        return true;
    }

    let Some(name) = relative.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name == ".env"
        || name.starts_with(".env.")
        || matches!(
            name,
            ".npmrc"
                | ".yarnrc"
                | ".pypirc"
                | ".netrc"
                | "id_rsa"
                | "id_dsa"
                | "id_ecdsa"
                | "id_ed25519"
        )
    {
        return true;
    }

    relative
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "p8" | "p12" | "pfx" | "pem" | "key" | "mobileprovision"
            )
        })
}

fn create_workspace_archive(root: &Path, archive_path: &Path) -> Result<(), ProviderError> {
    let mut command = Command::new("tar");
    command.args(["-czf"]).arg(archive_path);
    for pattern in [
        ".git",
        ".ssh",
        ".buildbridge",
        ".pnpm-store",
        "node_modules",
        "dist",
        "coverage",
        "DerivedData",
        "xcuserdata",
        ".idea",
        ".vscode",
        ".gradle",
        "android/build",
        "android/app/build",
        "android/capacitor-cordova-android-plugins/build",
        "ios/App/build",
        "ios/App/Pods",
        ".env",
        ".env.*",
        ".npmrc",
        ".yarnrc",
        ".pypirc",
        ".netrc",
        "id_rsa",
        "id_dsa",
        "id_ecdsa",
        "id_ed25519",
        "*.p8",
        "*.p12",
        "*.pfx",
        "*.pem",
        "*.key",
        "*.mobileprovision",
    ] {
        command.arg(format!("--exclude={pattern}"));
    }
    let output = command
        .arg("-C")
        .arg(root)
        .arg(".")
        .output()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not run tar for the source snapshot: {error}"
            ))
        })?;
    if !output.status.success() {
        return Err(ProviderError::GuestBridge(format!(
            "could not create the source snapshot: {}",
            clean_output(&output.stderr)
        )));
    }

    Ok(())
}

fn snapshot_sha256(path: &Path) -> Result<String, ProviderError> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not checksum the source snapshot: {error}"))
        })?;
    if !output.status.success() {
        return Err(ProviderError::GuestBridge(
            "sha256sum could not checksum the source snapshot".to_string(),
        ));
    }
    let checksum = clean_output(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    if checksum.len() != 64
        || !checksum
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(ProviderError::GuestBridge(
            "sha256sum returned an invalid source snapshot checksum".to_string(),
        ));
    }

    Ok(checksum)
}

#[allow(clippy::too_many_arguments)]
fn stream_workspace_archive<F>(
    archive_path: &Path,
    total_bytes: u64,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    guest_archive: &str,
    started_at: Instant,
    on_progress: &mut F,
) -> Result<(), ProviderError>
where
    F: FnMut(AppleProjectProgress),
{
    let mut archive = File::open(archive_path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not open the source snapshot: {error}"))
    })?;
    let remote_command = format!("/bin/cat > '{guest_archive}'");
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start source synchronization: {error}"))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not open source synchronization".to_string())
    })?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut completed_bytes = 0;
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    loop {
        let read = archive.read(&mut buffer).map_err(|error| {
            ProviderError::GuestBridge(format!("could not read the source snapshot: {error}"))
        })?;
        if read == 0 {
            break;
        }
        stdin.write_all(&buffer[..read]).map_err(|error| {
            ProviderError::GuestBridge(format!("source synchronization was interrupted: {error}"))
        })?;
        completed_bytes += read as u64;
        if last_progress.elapsed() >= Duration::from_millis(250) || completed_bytes == total_bytes {
            on_progress(apple_progress(
                AppleProjectPhase::Transferring,
                completed_bytes,
                total_bytes,
                started_at,
                "Copying the secret-filtered snapshot through the pinned SSH bridge.",
                None,
            ));
            last_progress = Instant::now();
        }
    }
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish source synchronization: {error}"))
    })?;
    if !output.status.success() {
        return Err(ProviderError::GuestBridge(format!(
            "the guest rejected the source snapshot: {}",
            clean_output(&output.stderr)
        )));
    }

    Ok(())
}

fn apple_progress(
    phase: AppleProjectPhase,
    completed_bytes: u64,
    total_bytes: u64,
    started_at: Instant,
    detail: &str,
    log_line: Option<String>,
) -> AppleProjectProgress {
    AppleProjectProgress {
        phase,
        completed_bytes,
        total_bytes,
        elapsed_seconds: started_at.elapsed().as_secs(),
        detail: detail.to_string(),
        log_line,
    }
}

fn signing_progress(
    phase: SigningProvisioningPhase,
    completed_bytes: u64,
    total_bytes: u64,
    started_at: Instant,
    detail: &str,
) -> SigningProvisioningProgress {
    SigningProvisioningProgress {
        phase,
        completed_bytes,
        total_bytes,
        elapsed_seconds: started_at.elapsed().as_secs(),
        detail: detail.to_string(),
    }
}

fn validate_signing_target(
    development_team: &str,
    bundle_identifier: &str,
) -> Result<(), ProviderError> {
    if development_team.is_empty()
        || development_team.len() > 64
        || !development_team
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(ProviderError::GuestBridge(
            "the project development team is missing or invalid".to_string(),
        ));
    }
    if bundle_identifier.is_empty()
        || bundle_identifier.len() > 255
        || bundle_identifier.starts_with('.')
        || bundle_identifier.ends_with('.')
        || bundle_identifier.contains("..")
        || !bundle_identifier
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
    {
        return Err(ProviderError::GuestBridge(
            "the project bundle identifier is missing or invalid".to_string(),
        ));
    }

    Ok(())
}

fn validate_signing_material(
    certificate_path: &Path,
    certificate_password: &str,
    profile_paths: &[PathBuf],
    keychain_password: &str,
) -> Result<(PathBuf, Vec<PathBuf>, u64), ProviderError> {
    if certificate_password.is_empty() || certificate_password.len() > 512 {
        return Err(ProviderError::GuestBridge(
            "store the certificate passphrase in the operating-system vault first".to_string(),
        ));
    }
    if keychain_password.is_empty() || keychain_password.len() > 512 {
        return Err(ProviderError::GuestBridge(
            "store a dedicated guest keychain password in the operating-system vault first"
                .to_string(),
        ));
    }
    if profile_paths.is_empty() || profile_paths.len() > SIGNING_PROFILE_MAX_COUNT {
        return Err(ProviderError::GuestBridge(format!(
            "select between 1 and {SIGNING_PROFILE_MAX_COUNT} provisioning profiles"
        )));
    }

    let certificate = validate_signing_file(
        certificate_path,
        &["p12", "pfx"],
        SIGNING_CERTIFICATE_MAX_BYTES,
        "signing certificate",
    )?;
    let mut total_bytes = fs::metadata(&certificate)
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not inspect the signing certificate: {error}"
            ))
        })?
        .len();
    let mut profiles = Vec::with_capacity(profile_paths.len());
    for profile_path in profile_paths {
        let profile = validate_signing_file(
            profile_path,
            &["mobileprovision"],
            PROVISIONING_PROFILE_MAX_BYTES,
            "provisioning profile",
        )?;
        if profiles.contains(&profile) {
            return Err(ProviderError::GuestBridge(
                "the same provisioning profile was selected more than once".to_string(),
            ));
        }
        total_bytes = total_bytes
            .checked_add(
                fs::metadata(&profile)
                    .map_err(|error| {
                        ProviderError::GuestBridge(format!(
                            "could not inspect a provisioning profile: {error}"
                        ))
                    })?
                    .len(),
            )
            .ok_or_else(|| {
                ProviderError::GuestBridge("the signing kit size is invalid".to_string())
            })?;
        profiles.push(profile);
    }
    if total_bytes > SIGNING_MATERIAL_MAX_BYTES {
        return Err(ProviderError::GuestBridge(
            "the signing kit exceeds the 128 MiB safety limit".to_string(),
        ));
    }

    Ok((certificate, profiles, total_bytes))
}

fn validate_signing_file(
    path: &Path,
    extensions: &[&str],
    maximum_bytes: u64,
    label: &str,
) -> Result<PathBuf, ProviderError> {
    if !path.is_absolute()
        || !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extensions
                    .iter()
                    .any(|expected| extension.eq_ignore_ascii_case(expected))
            })
    {
        return Err(ProviderError::GuestBridge(format!(
            "the {label} must be an absolute path with a supported extension"
        )));
    }
    let canonical = fs::canonicalize(path).map_err(|error| {
        ProviderError::GuestBridge(format!("the {label} is unavailable: {error}"))
    })?;
    let metadata = fs::metadata(&canonical).map_err(|error| {
        ProviderError::GuestBridge(format!("could not inspect the {label}: {error}"))
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum_bytes {
        return Err(ProviderError::GuestBridge(format!(
            "the {label} is empty, not a regular file, or exceeds its size limit"
        )));
    }

    Ok(canonical)
}

fn stream_bytes_to_guest(
    contents: &[u8],
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    guest_path: &str,
    label: &str,
) -> Result<(), ProviderError> {
    let remote_command = format!("umask 077; /bin/cat > {}", shell_single_quote(guest_path));
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start {label} transfer: {error}"))
        })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ProviderError::GuestBridge(format!("could not open {label} transfer")))?;
    stdin.write_all(contents).map_err(|error| {
        ProviderError::GuestBridge(format!("{label} transfer was interrupted: {error}"))
    })?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish {label} transfer: {error}"))
    })?;
    if !output.status.success() {
        return Err(ProviderError::GuestBridge(format!(
            "the guest rejected the {label}: {}",
            clean_output(&output.stderr)
        )));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn stream_signing_file<F>(
    path: &Path,
    guest_path: &str,
    total_bytes: u64,
    completed_bytes: &mut u64,
    started_at: Instant,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    on_progress: &mut F,
) -> Result<(), ProviderError>
where
    F: FnMut(SigningProvisioningProgress),
{
    let mut file = File::open(path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not open signing material: {error}"))
    })?;
    let remote_command = format!("umask 077; /bin/cat > {}", shell_single_quote(guest_path));
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not start signing material transfer: {error}"
            ))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not open signing material transfer".to_string())
    })?;
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    loop {
        let count = file.read(&mut buffer).map_err(|error| {
            ProviderError::GuestBridge(format!("could not read signing material: {error}"))
        })?;
        if count == 0 {
            break;
        }
        stdin.write_all(&buffer[..count]).map_err(|error| {
            ProviderError::GuestBridge(format!(
                "signing material transfer was interrupted: {error}"
            ))
        })?;
        *completed_bytes += count as u64;
        if last_progress.elapsed() >= Duration::from_millis(200) || *completed_bytes == total_bytes
        {
            on_progress(signing_progress(
                SigningProvisioningPhase::Transferring,
                *completed_bytes,
                total_bytes,
                started_at,
                "Transferring certificate and profiles over the pinned SSH bridge.",
            ));
            last_progress = Instant::now();
        }
    }
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!(
            "could not finish signing material transfer: {error}"
        ))
    })?;
    if !output.status.success() {
        return Err(ProviderError::GuestBridge(format!(
            "the guest rejected signing material: {}",
            clean_output(&output.stderr)
        )));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_signing_helper(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    helper_path: &str,
    keychain_path: &str,
    certificate_path: &str,
    certificate_der_path: &str,
    xcodebuild_path: &str,
    keychain_password: &str,
    certificate_password: &str,
) -> Result<(), ProviderError> {
    let remote_command = format!(
        "{} {} {} {} {}",
        shell_single_quote(helper_path),
        shell_single_quote(keychain_path),
        shell_single_quote(certificate_path),
        shell_single_quote(certificate_der_path),
        shell_single_quote(xcodebuild_path),
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start signing import: {error}"))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not open the protected signing input".to_string())
    })?;
    write_secret_frame(&mut stdin, keychain_password)?;
    write_secret_frame(&mut stdin, certificate_password)?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish signing import: {error}"))
    })?;
    if !output.status.success() {
        let detail = clean_output(&output.stderr);
        let message = if detail == "identity_count:0" {
            "the PKCS#12 archive contains no certificate/private-key identity; export the distribution certificate from Keychain Access → My Certificates with its private key expanded underneath it"
                .to_string()
        } else if let Some(count) = detail.strip_prefix("identity_count:") {
            format!(
                "the PKCS#12 archive contains {count} identities; export exactly one distribution identity from Keychain Access"
            )
        } else if detail == "partition_acl_missing" {
            "macOS imported the identity but did not expose the private-key authorization record required for unattended Apple signing"
                .to_string()
        } else if detail.starts_with("authorize_private_key:") {
            format!(
                "macOS imported the identity but could not authorize its private key for Apple signing ({detail})"
            )
        } else if detail.starts_with("import_certificate:-25293") {
            "the certificate passphrase is incorrect or the PKCS#12 archive is damaged".to_string()
        } else if detail.is_empty() {
            "macOS could not import the signing certificate".to_string()
        } else {
            format!("macOS could not provision the signing keychain ({detail})")
        };
        return Err(ProviderError::GuestBridge(message));
    }
    if clean_output(&output.stdout) != "ok" {
        return Err(ProviderError::GuestBridge(
            "the signing helper returned an unexpected result".to_string(),
        ));
    }

    Ok(())
}

fn write_secret_frame(writer: &mut impl Write, secret: &str) -> Result<(), ProviderError> {
    let length = u32::try_from(secret.len()).map_err(|_| {
        ProviderError::GuestBridge("a signing credential exceeds its size limit".to_string())
    })?;
    writer.write_all(&length.to_be_bytes()).map_err(|error| {
        ProviderError::GuestBridge(format!("could not send protected signing input: {error}"))
    })?;
    writer.write_all(secret.as_bytes()).map_err(|error| {
        ProviderError::GuestBridge(format!("could not send protected signing input: {error}"))
    })
}

fn inspect_guest_profiles(
    profiles: &[(String, PathBuf)],
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<String, ProviderError> {
    let mut command = "set -eu; umask 077".to_string();
    for (guest_profile, _) in profiles {
        let profile = shell_single_quote(guest_profile);
        let plist = shell_single_quote(&format!("{guest_profile}.plist"));
        let certificate = shell_single_quote(&format!("{guest_profile}.certificate.der"));
        command.push_str(&format!(
            "; /usr/bin/security cms -D -i {profile} > {plist}; uuid=$(/usr/bin/plutil -extract UUID raw -o - {plist}); team=$(/usr/bin/plutil -extract TeamIdentifier.0 raw -o - {plist}); app_id=$(/usr/bin/plutil -extract Entitlements.application-identifier raw -o - {plist}); expires_at=$(/usr/bin/plutil -extract ExpirationDate raw -o - {plist}); if ! expiry_epoch=$(/bin/date -j -f '%Y-%m-%d %H:%M:%S %z' \"$expires_at\" +%s 2>/dev/null || /bin/date -j -f '%Y-%m-%dT%H:%M:%SZ' \"$expires_at\" +%s 2>/dev/null); then /usr/bin/printf 'invalid_profile_expiry:%s' \"$uuid\" >&2; exit 1; fi; if /bin/test \"$expiry_epoch\" -le \"$(/bin/date +%s)\"; then /usr/bin/printf 'expired_profile:%s' \"$uuid\" >&2; exit 1; fi; /usr/bin/printf '__BUILDBRIDGE_PROFILE__\\t%s\\t%s\\t%s\\t%s\\n' \"$uuid\" \"$team\" \"$app_id\" \"$expires_at\"; certificate_count=$(/usr/bin/plutil -extract DeveloperCertificates xml1 -o - {plist} | /usr/bin/grep -c '<data>'); certificate_index=0; while /bin/test \"$certificate_index\" -lt \"$certificate_count\"; do /usr/bin/plutil -extract \"DeveloperCertificates.$certificate_index\" raw -o - {plist} | /usr/bin/base64 -D > {certificate}; certificate_sha256=$(/usr/bin/openssl dgst -sha256 {certificate} | /usr/bin/awk '{{print $NF}}'); /usr/bin/printf '__BUILDBRIDGE_PROFILE_CERT__\\t%s\\t%s\\n' \"$uuid\" \"$certificate_sha256\"; certificate_index=$((certificate_index + 1)); done; /bin/rm -f {certificate} {plist}"
        ));
    }

    match run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &command,
    ) {
        Err(ProviderError::GuestBridge(message)) if message.starts_with("expired_profile:") => {
            Err(ProviderError::GuestBridge(format!(
                "provisioning profile {} has expired",
                message.trim_start_matches("expired_profile:")
            )))
        }
        Err(ProviderError::GuestBridge(message))
            if message.starts_with("invalid_profile_expiry:") =>
        {
            Err(ProviderError::GuestBridge(format!(
                "provisioning profile {} has an unreadable expiration date",
                message.trim_start_matches("invalid_profile_expiry:")
            )))
        }
        result => result,
    }
}

fn parse_profile_summaries(output: &str) -> Result<Vec<ProvisioningProfileSummary>, ProviderError> {
    let mut profiles: Vec<ProvisioningProfileSummary> = Vec::new();
    for line in output.lines() {
        if let Some(values) = line.strip_prefix("__BUILDBRIDGE_PROFILE_CERT__\t") {
            let Some((uuid, fingerprint)) = values.split_once('\t') else {
                return Err(ProviderError::GuestBridge(
                    "macOS returned invalid provisioning profile certificate metadata".to_string(),
                ));
            };
            let fingerprint = fingerprint.to_ascii_uppercase();
            let Some(profile) = profiles.iter_mut().find(|profile| profile.uuid == uuid) else {
                return Err(ProviderError::GuestBridge(
                    "macOS returned out-of-order provisioning profile metadata".to_string(),
                ));
            };
            if fingerprint.len() != 64
                || !fingerprint
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
                || profile
                    .developer_certificate_sha256
                    .iter()
                    .any(|stored| stored == &fingerprint)
            {
                return Err(ProviderError::GuestBridge(
                    "macOS returned invalid provisioning profile certificate metadata".to_string(),
                ));
            }
            profile.developer_certificate_sha256.push(fingerprint);
            continue;
        }
        let Some(values) = line.strip_prefix("__BUILDBRIDGE_PROFILE__\t") else {
            continue;
        };
        let mut fields = values.splitn(4, '\t');
        let uuid = fields.next().unwrap_or_default();
        let team_identifier = fields.next().unwrap_or_default();
        let application_identifier = fields.next().unwrap_or_default();
        let expires_at = fields.next().unwrap_or_default();
        if !valid_profile_uuid(uuid)
            || team_identifier.is_empty()
            || team_identifier.len() > 64
            || !team_identifier
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
            || application_identifier.is_empty()
            || application_identifier.len() > 320
            || !application_identifier.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '*')
            })
            || application_identifier.matches('*').count() > 1
            || (application_identifier.contains('*') && !application_identifier.ends_with('*'))
            || expires_at.is_empty()
            || expires_at.len() > 96
            || !expires_at
                .chars()
                .all(|character| character.is_ascii_graphic() || character == ' ')
            || profiles
                .iter()
                .any(|profile: &ProvisioningProfileSummary| profile.uuid == uuid)
        {
            return Err(ProviderError::GuestBridge(
                "macOS returned invalid provisioning profile metadata".to_string(),
            ));
        }
        profiles.push(ProvisioningProfileSummary {
            uuid: uuid.to_string(),
            team_identifier: team_identifier.to_string(),
            application_identifier: application_identifier.to_string(),
            expires_at: expires_at.to_string(),
            developer_certificate_sha256: Vec::new(),
        });
    }
    if profiles
        .iter()
        .any(|profile| profile.developer_certificate_sha256.is_empty())
    {
        return Err(ProviderError::GuestBridge(
            "a provisioning profile contains no developer signing certificate".to_string(),
        ));
    }

    Ok(profiles)
}

fn valid_profile_uuid(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(index, character)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                character == '-'
            } else {
                character.is_ascii_hexdigit()
            }
        })
}

fn profile_allows_bundle(application_identifier: &str, bundle_identifier: &str) -> bool {
    let Some((_, profile_bundle)) = application_identifier.split_once('.') else {
        return false;
    };

    profile_bundle == bundle_identifier
        || profile_bundle == "*"
        || profile_bundle
            .strip_suffix('*')
            .is_some_and(|prefix| bundle_identifier.starts_with(prefix))
}

fn install_guest_profiles(
    guest_profiles: &[(String, PathBuf)],
    profiles: &[ProvisioningProfileSummary],
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
    let guest_home = format!("/Users/{username}");
    let directories = [
        format!("{guest_home}/Library/MobileDevice/Provisioning Profiles"),
        format!("{guest_home}/Library/Developer/Xcode/UserData/Provisioning Profiles"),
    ];
    let mut command = format!(
        "set -eu; umask 077; /bin/mkdir -p {} {}",
        shell_single_quote(&directories[0]),
        shell_single_quote(&directories[1]),
    );
    let mut staged_profiles = Vec::new();
    for ((guest_profile, _), profile) in guest_profiles.iter().zip(profiles) {
        for directory in &directories {
            let destination = format!("{directory}/{}.mobileprovision", profile.uuid);
            let pending = format!("{destination}.buildbridge-incoming");
            command.push_str(&format!(
                "; /bin/rm -f {}; /bin/cp {} {}; /bin/chmod 600 {}",
                shell_single_quote(&pending),
                shell_single_quote(guest_profile),
                shell_single_quote(&pending),
                shell_single_quote(&pending),
            ));
            staged_profiles.push((pending, destination));
        }
    }
    for (pending, destination) in staged_profiles {
        command.push_str(&format!(
            "; /bin/mv -f {} {}",
            shell_single_quote(&pending),
            shell_single_quote(&destination),
        ));
    }

    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &command,
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn verify_code_signing_identity(
    identity_sha1: &str,
    helper_path: &str,
    keychain_path: &str,
    staging_path: &str,
    keychain_password: &str,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
    let probe_path = format!("{staging_path}/codesign-probe");
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!(
            "set -eu; /bin/cp /usr/bin/true {probe}; /bin/chmod 700 {probe}",
            probe = shell_single_quote(&probe_path),
        ),
    )?;

    let remote_command = format!(
        "{} --probe {} {} {}",
        shell_single_quote(helper_path),
        shell_single_quote(keychain_path),
        shell_single_quote(identity_sha1),
        shell_single_quote(&probe_path),
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start the signing probe: {error}"))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not open the protected signing input".to_string())
    })?;
    write_secret_frame(&mut stdin, keychain_password)?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish the signing probe: {error}"))
    })?;
    if !output.status.success() {
        let detail = clean_output(&output.stderr);
        let detail = if detail.is_empty() {
            "macOS rejected the private-key operation".to_string()
        } else {
            detail
        };
        return Err(ProviderError::GuestBridge(format!(
            "the archive imported one certificate/private-key identity, but macOS could not use it to sign code: {detail}"
        )));
    }
    if clean_output(&output.stdout) != "ok" {
        return Err(ProviderError::GuestBridge(
            "the signing probe returned an unexpected result".to_string(),
        ));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn install_apple_wwdr_g3_intermediate(
    pem_path: &str,
    der_path: &str,
    keychain_path: &str,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &apple_wwdr_g3_import_command(pem_path, der_path, keychain_path),
    )
    .map(|_| ())
    .map_err(|error| match error {
        ProviderError::GuestBridge(message) => ProviderError::GuestBridge(format!(
            "macOS could not install Apple's pinned WWDR G3 signing intermediate: {message}"
        )),
        error => error,
    })
}

fn apple_wwdr_g3_import_command(pem_path: &str, der_path: &str, keychain_path: &str) -> String {
    let pem = shell_single_quote(pem_path);
    let der = shell_single_quote(der_path);
    let keychain = shell_single_quote(keychain_path);
    format!(
        "set -eu; /usr/bin/openssl x509 -in {pem} -outform DER -out {der}; actual_sha256=$(/usr/bin/openssl dgst -sha256 {der} | /usr/bin/awk '{{print toupper($NF)}}'); if /bin/test \"$actual_sha256\" != '{APPLE_WWDR_G3_DER_SHA256}'; then /usr/bin/printf 'the bundled Apple intermediate failed its pinned SHA-256 check' >&2; exit 1; fi; /usr/bin/security import {der} -k {keychain} >/dev/null"
    )
}

fn certificate_metadata_command(certificate_path: &str) -> String {
    let certificate = shell_single_quote(certificate_path);
    format!(
        "set -eu; certificate_start=$(/usr/bin/openssl x509 -inform DER -in {certificate} -noout -startdate); certificate_start=${{certificate_start#notBefore=}}; certificate_end=$(/usr/bin/openssl x509 -inform DER -in {certificate} -noout -enddate); certificate_end=${{certificate_end#notAfter=}}; if ! certificate_start_epoch=$(/bin/date -j -f '%b %e %T %Y %Z' \"$certificate_start\" +%s 2>/dev/null); then /usr/bin/printf 'macOS could not parse the certificate start date: %s' \"$certificate_start\" >&2; exit 1; fi; if ! certificate_end_epoch=$(/bin/date -j -f '%b %e %T %Y %Z' \"$certificate_end\" +%s 2>/dev/null); then /usr/bin/printf 'macOS could not parse the certificate expiry date: %s' \"$certificate_end\" >&2; exit 1; fi; guest_now_epoch=$(/bin/date +%s); guest_now=$(/bin/date -u '+%Y-%m-%dT%H:%M:%SZ'); if /bin/test \"$guest_now_epoch\" -lt \"$certificate_start_epoch\"; then /usr/bin/printf 'the .p12 certificate is not valid until %s; the macOS guest clock is %s' \"$certificate_start\" \"$guest_now\" >&2; exit 1; fi; if /bin/test \"$guest_now_epoch\" -ge \"$certificate_end_epoch\"; then /usr/bin/printf 'the .p12 certificate expired at %s; the macOS guest clock is %s' \"$certificate_end\" \"$guest_now\" >&2; exit 1; fi; /usr/bin/openssl x509 -inform DER -in {certificate} -noout -enddate -fingerprint -sha256 -subject -nameopt RFC2253; /usr/bin/openssl x509 -inform DER -in {certificate} -noout -fingerprint -sha1"
    )
}

fn parse_certificate_metadata(
    output: &str,
) -> Result<(String, String, String, String, String), ProviderError> {
    let mut expires_at = None;
    let mut sha1 = None;
    let mut sha256 = None;
    let mut team_identifier = None;
    let mut identity_name = None;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("notAfter=") {
            let value = value.trim();
            if !value.is_empty()
                && value.len() <= 96
                && value
                    .chars()
                    .all(|character| character.is_ascii_graphic() || character == ' ')
            {
                expires_at = Some(value.to_string());
            }
        } else if line.to_ascii_lowercase().starts_with("sha1 fingerprint=") {
            let value = line
                .split_once('=')
                .map(|(_, value)| value)
                .unwrap_or_default();
            let normalized = value.replace(':', "").to_ascii_uppercase();
            if normalized.len() == 40
                && normalized
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            {
                sha1 = Some(normalized);
            }
        } else if line.to_ascii_lowercase().starts_with("sha256 fingerprint=") {
            let value = line
                .split_once('=')
                .map(|(_, value)| value)
                .unwrap_or_default();
            let normalized = value.replace(':', "").to_ascii_uppercase();
            if normalized.len() == 64
                && normalized
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            {
                sha256 = Some(normalized);
            }
        } else if let Some(subject) = line.strip_prefix("subject=") {
            team_identifier = subject.split(',').find_map(|component| {
                let value = component.trim().strip_prefix("OU=")?;
                (!value.is_empty()
                    && value.len() <= 64
                    && value
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric()))
                .then(|| value.to_string())
            });
            identity_name = subject.split(',').find_map(|component| {
                let value = component.trim().strip_prefix("CN=")?;
                (!value.is_empty()
                    && value.len() <= 256
                    && value.chars().all(|character| !character.is_control()))
                .then(|| value.replace("\\,", ","))
            });
        }
    }

    match (expires_at, sha1, sha256, team_identifier, identity_name) {
        (
            Some(expires_at),
            Some(sha1),
            Some(sha256),
            Some(team_identifier),
            Some(identity_name),
        ) => Ok((expires_at, sha1, sha256, team_identifier, identity_name)),
        _ => Err(ProviderError::GuestBridge(
            "macOS returned invalid signing certificate metadata".to_string(),
        )),
    }
}

fn apple_project_phase(value: &str) -> Option<AppleProjectPhase> {
    match value {
        "preparing_tools" => Some(AppleProjectPhase::PreparingTools),
        "preparing_platform" => Some(AppleProjectPhase::PreparingPlatform),
        "installing_dependencies" => Some(AppleProjectPhase::InstallingDependencies),
        "building_web_assets" => Some(AppleProjectPhase::BuildingWebAssets),
        "syncing_ios" => Some(AppleProjectPhase::SyncingIos),
        "resolving_pods" => Some(AppleProjectPhase::ResolvingPods),
        "building" => Some(AppleProjectPhase::Building),
        "completed" => Some(AppleProjectPhase::Completed),
        _ => None,
    }
}

fn apple_platform_progress(value: &str) -> Option<(u64, u64, &'static str)> {
    let value = value.strip_prefix("__BUILDBRIDGE_PLATFORM_PROGRESS__:")?;
    let mut values = value.splitn(3, ':');
    let completed_bytes: u64 = values.next()?.parse().ok()?;
    let total_bytes: u64 = values.next()?.parse().ok()?;
    let detail = match values.next()? {
        "locating" => "Apple is locating the matching iOS Simulator platform",
        "downloading" => "Downloading Apple's iOS Simulator platform",
        "installing" => "Simulator downloaded; macOS is installing and registering it",
        _ => return None,
    };

    Some((completed_bytes.min(total_bytes), total_bytes, detail))
}

fn apple_build_log_is_diagnostic(line: &str) -> bool {
    let normalized = line.to_ascii_lowercase();

    normalized.starts_with("error:")
        || normalized.contains(": error:")
        || normalized.contains("com.apple.actool.errors")
        || normalized.contains("failed with a nonzero exit code")
        || normalized.starts_with("** build failed **")
        || normalized.starts_with("the following build commands failed:")
        || normalized.trim_start().starts_with("compileassetcatalog")
}

fn apple_build_retry_detail(value: &str) -> Option<&'static str> {
    match value {
        "__BUILDBRIDGE_BUILD_RETRY__:platform" => {
            Some("Verifying the new Simulator runtime before one automatic retry")
        }
        _ => None,
    }
}

fn phase_detail(phase: AppleProjectPhase) -> &'static str {
    match phase {
        AppleProjectPhase::Snapshotting => "Creating the source snapshot",
        AppleProjectPhase::Transferring => "Synchronizing source",
        AppleProjectPhase::Extracting => "Preparing the guest workspace",
        AppleProjectPhase::PreparingTools => "Preparing Node, pnpm, and CocoaPods",
        AppleProjectPhase::PreparingPlatform => {
            "Downloading and installing Apple's iOS Simulator platform"
        }
        AppleProjectPhase::InstallingDependencies => "Installing locked project dependencies",
        AppleProjectPhase::BuildingWebAssets => "Building web assets",
        AppleProjectPhase::SyncingIos => "Synchronizing the Capacitor iOS project",
        AppleProjectPhase::ResolvingPods => "Resolving locked CocoaPods",
        AppleProjectPhase::Building => "Compiling the unsigned iOS Simulator app",
        AppleProjectPhase::Completed => "Unsigned test build complete",
    }
}

fn sanitize_build_log_line(line: &str) -> String {
    line.chars()
        .filter(|character| !character.is_control() || matches!(character, '\t'))
        .take(1_000)
        .collect::<String>()
        .trim()
        .to_string()
}

fn validate_xcode_package(package_path: &Path) -> Result<(PathBuf, u64), ProviderError> {
    if !package_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xip"))
    {
        return Err(ProviderError::GuestBridge(
            "select an Apple Xcode .xip archive".to_string(),
        ));
    }

    let canonical_path = fs::canonicalize(package_path).map_err(|error| {
        ProviderError::GuestBridge(format!(
            "the selected Xcode package is unavailable: {error}"
        ))
    })?;
    let metadata = fs::metadata(&canonical_path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not inspect the Xcode package: {error}"))
    })?;

    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > XCODE_PACKAGE_MAX_BYTES {
        return Err(ProviderError::GuestBridge(
            "the selected Xcode package must be a non-empty file no larger than 20 GiB".to_string(),
        ));
    }

    let mut archive = File::open(&canonical_path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not open the Xcode package: {error}"))
    })?;
    let mut signature = [0_u8; 4];
    archive.read_exact(&mut signature).map_err(|error| {
        ProviderError::GuestBridge(format!("could not read the Xcode package: {error}"))
    })?;
    if signature != *b"xar!" {
        return Err(ProviderError::GuestBridge(
            "the selected file is not a complete Apple XIP archive".to_string(),
        ));
    }

    Ok((canonical_path, metadata.len()))
}

#[allow(clippy::too_many_arguments)]
fn stream_xcode_package<F>(
    package_path: &Path,
    total_bytes: u64,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    guest_archive: &str,
    started_at: Instant,
    on_progress: &mut F,
) -> Result<(), ProviderError>
where
    F: FnMut(XcodeImportProgress),
{
    let mut package = File::open(package_path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not open the Xcode package: {error}"))
    })?;
    let remote_command = format!(
        "/bin/cat > '{guest_archive}.part' && /bin/mv -f '{guest_archive}.part' '{guest_archive}'"
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not start the secure Xcode transfer: {error}"
            ))
        })?;
    let mut child_stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not open the secure Xcode transfer".to_string())
    })?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut transferred_bytes = 0_u64;
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    let transfer_result = loop {
        let read = match package.read(&mut buffer) {
            Ok(read) => read,
            Err(error) => {
                break Err(ProviderError::GuestBridge(format!(
                    "could not read the Xcode package: {error}"
                )));
            }
        };
        if read == 0 {
            break Ok(());
        }
        if let Err(error) = child_stdin.write_all(&buffer[..read]) {
            break Err(ProviderError::GuestBridge(format!(
                "the secure Xcode transfer was interrupted: {error}"
            )));
        }

        transferred_bytes += read as u64;
        if last_progress.elapsed() >= Duration::from_millis(250) || transferred_bytes == total_bytes
        {
            on_progress(XcodeImportProgress {
                phase: XcodeImportPhase::Transferring,
                transferred_bytes,
                total_bytes,
                elapsed_seconds: started_at.elapsed().as_secs(),
                detail: "Copying the signed XIP archive through the pinned SSH bridge.".to_string(),
            });
            last_progress = Instant::now();
        }
    };
    drop(child_stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish the Xcode transfer: {error}"))
    })?;
    transfer_result?;

    if !output.status.success() {
        let message = clean_output(&output.stderr);
        return Err(ProviderError::GuestBridge(if message.is_empty() {
            "the guest rejected the Xcode package transfer".to_string()
        } else {
            message
        }));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn expand_xcode_package<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    guest_archive: &str,
    guest_downloads: &str,
    installed_path: &str,
    total_bytes: u64,
    started_at: Instant,
    on_progress: &mut F,
) -> Result<(), ProviderError>
where
    F: FnMut(XcodeImportProgress),
{
    let expand_command = format!(
        "set -eu; staging=$(/usr/bin/mktemp -d '{guest_downloads}/.buildbridge-xcode.XXXXXX'); cd \"$staging\"; /usr/bin/xip --expand '{guest_archive}'; /bin/mv \"$staging/Xcode.app\" '{installed_path}'; /bin/rmdir \"$staging\"; /bin/rm -f '{guest_archive}'"
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(expand_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start Xcode expansion: {error}"))
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture Xcode expansion output".to_string())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture Xcode expansion errors".to_string())
    })?;
    let stdout_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut stdout = stdout;
        let _ = stdout.read_to_end(&mut output);
        output
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut stderr = stderr;
        let _ = stderr.read_to_end(&mut output);
        output
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                on_progress(XcodeImportProgress {
                    phase: XcodeImportPhase::Expanding,
                    transferred_bytes: total_bytes,
                    total_bytes,
                    elapsed_seconds: started_at.elapsed().as_secs(),
                    detail:
                        "Verifying and expanding Xcode inside macOS. This can take several minutes."
                            .to_string(),
                });
                thread::sleep(Duration::from_secs(1));
            }
            Err(error) => {
                return Err(ProviderError::GuestBridge(format!(
                    "could not monitor Xcode expansion: {error}"
                )));
            }
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();

    if !status.success() {
        let stderr = clean_output(&stderr);
        let stdout = clean_output(&stdout);
        let message = if !stderr.is_empty() { stderr } else { stdout };
        return Err(ProviderError::GuestBridge(if message.is_empty() {
            "macOS could not verify or expand the Xcode package".to_string()
        } else {
            format!("macOS could not expand the Xcode package: {message}")
        }));
    }

    Ok(())
}

pub fn valid_guest_username(username: &str) -> bool {
    let mut chars = username.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    username.len() <= 32
        && (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub fn fingerprint_host_key_line(line: &str) -> Result<String, ProviderError> {
    let parsed = parse_host_key_line(line)?;
    let mut child = Command::new("ssh-keygen")
        .args(["-lf", "-", "-E", "sha256"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not run ssh-keygen; install OpenSSH client tools: {error}"
            ))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not send the host key to ssh-keygen".to_string())
    })?;
    writeln!(stdin, "{} {}", parsed.algorithm, parsed.key).map_err(|error| {
        ProviderError::GuestBridge(format!("could not fingerprint the SSH host key: {error}"))
    })?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not fingerprint the SSH host key: {error}"))
    })?;

    if !output.status.success() {
        return Err(ProviderError::GuestBridge(
            "the SSH host key is malformed".to_string(),
        ));
    }

    clean_output(&output.stdout)
        .split_whitespace()
        .find(|field| field.starts_with("SHA256:"))
        .map(str::to_string)
        .ok_or_else(|| {
            ProviderError::GuestBridge("ssh-keygen returned an invalid fingerprint".to_string())
        })
}

#[derive(Debug)]
struct ParsedHostKey<'a> {
    host: &'a str,
    algorithm: &'a str,
    key: &'a str,
}

fn parse_host_key_line(line: &str) -> Result<ParsedHostKey<'_>, ProviderError> {
    let mut fields = line.split_whitespace();
    let host = fields.next();
    let algorithm = fields.next();
    let key = fields.next();

    if host.is_none()
        || algorithm != Some("ssh-ed25519")
        || key.is_none()
        || fields.next().is_some()
    {
        return Err(ProviderError::GuestBridge(
            "the SSH host key pin is invalid".to_string(),
        ));
    }

    Ok(ParsedHostKey {
        host: host.expect("checked above"),
        algorithm: algorithm.expect("checked above"),
        key: key.expect("checked above"),
    })
}

fn guest_port_reachable(ssh_port: u16) -> bool {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), ssh_port);

    TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok()
}

fn run_guest_command(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    command: &str,
) -> Result<String, ProviderError> {
    let output = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(command)
        .output()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not run ssh; install OpenSSH client tools: {error}"
            ))
        })?;

    if output.status.success() {
        Ok(clean_output(&output.stdout))
    } else {
        let message = clean_output(&output.stderr);
        Err(ProviderError::GuestBridge(if message.is_empty() {
            "SSH authentication failed; add the BuildBridge public key to the guest user"
                .to_string()
        } else {
            message
        }))
    }
}

fn guest_ssh_command(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Command {
    let destination = format!("{username}@127.0.0.1");
    let mut command = Command::new("ssh");
    command
        .arg("-F")
        .arg("/dev/null")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=3",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "PasswordAuthentication=no",
            "-o",
            "KbdInteractiveAuthentication=no",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "LogLevel=ERROR",
            "-p",
            &ssh_port.to_string(),
            "-i",
        ])
        .arg(identity_path)
        .arg("-o")
        .arg(format!("UserKnownHostsFile={}", known_hosts_path.display()))
        .arg(destination);

    command
}

pub fn probe_host() -> HostPrerequisites {
    let supported_host = cfg!(target_os = "linux") && std::env::consts::ARCH == "x86_64";
    let docker_cli_output = Command::new("docker").arg("--version").output();
    let docker_cli = docker_cli_output.is_ok();
    let docker_version = docker_cli_output
        .ok()
        .filter(|output| output.status.success())
        .map(|output| clean_output(&output.stdout));
    let docker_daemon = docker_cli
        && Command::new("docker")
            .args(["info", "--format", "{{.ServerVersion}}"])
            .output()
            .is_ok_and(|output| output.status.success());
    let kvm_access = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok();
    let display = std::env::var("DISPLAY")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let display_access = display.is_some() && std::path::Path::new("/tmp/.X11-unix").is_dir();
    let mut issues = Vec::new();

    if !supported_host {
        issues.push("Docker-OSX currently requires an x86_64 Linux host with KVM.".to_string());
    }
    if !docker_cli {
        issues.push("Install the Docker CLI before creating a macOS builder.".to_string());
    } else if !docker_daemon {
        issues.push("Start Docker and ensure this user can access the daemon.".to_string());
    }
    if !kvm_access {
        issues.push("Grant this user read/write access to /dev/kvm.".to_string());
    }
    if !display_access {
        issues.push("An X11 display is required for the first-boot macOS console.".to_string());
    }

    HostPrerequisites {
        supported_host,
        docker_cli,
        docker_daemon,
        docker_version,
        kvm_access,
        display_access,
        display,
        ready: issues.is_empty(),
        issues,
    }
}

pub fn status() -> Result<RuntimeStatus, ProviderError> {
    let prerequisites = probe_host();

    if !prerequisites.docker_daemon {
        return Ok(RuntimeStatus {
            prerequisites,
            state: ContainerState::Unavailable,
            container_id: None,
            started_at: None,
        });
    }

    let (state, container_id, started_at) = inspect_container()?;

    Ok(RuntimeStatus {
        prerequisites,
        state,
        container_id,
        started_at,
    })
}

pub fn launch(
    config: &MacBuilderConfig,
    identity_path: &Path,
) -> Result<RuntimeStatus, ProviderError> {
    config.validate()?;
    let prerequisites = probe_host();

    if !prerequisites.ready {
        return Err(ProviderError::Prerequisites(prerequisites.issues.join(" ")));
    }

    let (state, _, _) = inspect_container()?;

    if state == ContainerState::Missing {
        ensure_image()?;
        ensure_identity(identity_path)?;
        create_container(
            config,
            prerequisites.display.as_deref().unwrap_or(":0"),
            identity_path,
        )?;
    } else {
        ensure_manual_restart_policy()?;
    }

    let (state, _, _) = inspect_container()?;
    if state != ContainerState::Running {
        run_docker("start", &["start".to_string(), CONTAINER_NAME.to_string()])?;
    }

    status()
}

pub fn stop() -> Result<RuntimeStatus, ProviderError> {
    let current = status()?;

    if current.state == ContainerState::Paused {
        run_docker(
            "unpause",
            &["unpause".to_string(), CONTAINER_NAME.to_string()],
        )?;
    }

    if matches!(
        current.state,
        ContainerState::Running | ContainerState::Paused | ContainerState::Restarting
    ) {
        run_docker(
            "stop",
            &[
                "stop".to_string(),
                "--time=30".to_string(),
                CONTAINER_NAME.to_string(),
            ],
        )?;
    }

    status()
}

pub fn recent_logs() -> Result<Vec<String>, ProviderError> {
    let current = status()?;

    if matches!(
        current.state,
        ContainerState::Missing | ContainerState::Unavailable
    ) {
        return Ok(Vec::new());
    }

    let output = run_docker(
        "logs",
        &[
            "logs".to_string(),
            "--tail=80".to_string(),
            CONTAINER_NAME.to_string(),
        ],
    )?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(combined
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect())
}

fn inspect_container() -> Result<(ContainerState, Option<String>, Option<String>), ProviderError> {
    let output = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{.State.Status}}|{{.Id}}|{{.State.StartedAt}}",
            CONTAINER_NAME,
        ])
        .output()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?;

    if !output.status.success() {
        let message = clean_output(&output.stderr);

        if is_missing_container_error(&message) {
            return Ok((ContainerState::Missing, None, None));
        }

        return Err(ProviderError::DockerCommand {
            operation: "inspect",
            message,
        });
    }

    let value = clean_output(&output.stdout);
    let mut fields = value.splitn(3, '|');
    let state = fields.next().unwrap_or_default();
    let container_id = fields.next();
    let started_at = fields.next().and_then(normalize_started_at);

    if container_id.is_none() {
        return Ok((ContainerState::Unknown, None, None));
    }

    Ok((
        ContainerState::from_docker(state),
        container_id.map(str::to_string),
        started_at,
    ))
}

fn normalize_started_at(value: &str) -> Option<String> {
    let value = value.trim();

    (!value.is_empty() && !value.starts_with("0001-")).then(|| value.to_string())
}

fn is_missing_container_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();

    normalized.contains("no such object") || normalized.contains("no such container")
}

fn ensure_image() -> Result<(), ProviderError> {
    let exists = Command::new("docker")
        .args(["image", "inspect", DOCKER_IMAGE])
        .output()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?
        .status
        .success();

    if !exists {
        run_docker(
            "image pull",
            &["pull".to_string(), DOCKER_IMAGE.to_string()],
        )?;
    }

    Ok(())
}

fn ensure_identity(identity_path: &Path) -> Result<(), ProviderError> {
    validate_identity_path(identity_path)?;

    if identity_path.is_file() {
        return validate_identity_file(identity_path);
    }

    let parent = identity_path
        .parent()
        .ok_or_else(|| ProviderError::Identity("identity directory is unavailable".to_string()))?;
    fs::create_dir_all(parent).map_err(|error| ProviderError::Identity(error.to_string()))?;
    let mount = format!("--volume={}:/buildbridge-identity:rw", parent.display());
    let args = vec![
        "run".to_string(),
        "--rm".to_string(),
        mount,
        "--workdir=/home/arch/OSX-KVM/Docker-OSX/osx-serial-generator".to_string(),
        "--entrypoint=/home/arch/OSX-KVM/Docker-OSX/osx-serial-generator/generate-unique-machine-values.sh".to_string(),
        DOCKER_IMAGE.to_string(),
        "--count=1".to_string(),
        "--output-env=/buildbridge-identity/identity.env".to_string(),
    ];
    run_docker("machine identity generation", &args)?;
    validate_identity_file(identity_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(identity_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| ProviderError::Identity(error.to_string()))?;
    }

    Ok(())
}

fn validate_identity_path(identity_path: &Path) -> Result<(), ProviderError> {
    if !identity_path.is_absolute()
        || identity_path.file_name().and_then(|name| name.to_str()) != Some("identity.env")
        || identity_path.to_string_lossy().contains(':')
    {
        return Err(ProviderError::Identity(
            "identity path must be an absolute identity.env path without ':'".to_string(),
        ));
    }

    Ok(())
}

fn validate_identity_file(identity_path: &Path) -> Result<(), ProviderError> {
    let identity = fs::read_to_string(identity_path)
        .map_err(|error| ProviderError::Identity(error.to_string()))?;
    let required_values = [
        "export DEVICE_MODEL=",
        "export SERIAL=",
        "export BOARD_SERIAL=",
        "export UUID=",
        "export MAC_ADDRESS=",
    ];

    if identity.len() > 8_192
        || required_values
            .iter()
            .any(|required| !identity.contains(required))
    {
        return Err(ProviderError::Identity(
            "the generated identity file is incomplete".to_string(),
        ));
    }

    Ok(())
}

fn create_container(
    config: &MacBuilderConfig,
    display: &str,
    identity_path: &Path,
) -> Result<(), ProviderError> {
    let args = create_args(config, display, identity_path);
    run_docker("container creation", &args)?;

    Ok(())
}

fn ensure_manual_restart_policy() -> Result<(), ProviderError> {
    run_docker(
        "restart policy update",
        &[
            "update".to_string(),
            "--restart=no".to_string(),
            CONTAINER_NAME.to_string(),
        ],
    )?;

    Ok(())
}

fn create_args(config: &MacBuilderConfig, display: &str, identity_path: &Path) -> Vec<String> {
    vec![
        "create".to_string(),
        format!("--name={CONTAINER_NAME}"),
        "--label=dev.buildbridge.managed=true".to_string(),
        "--restart=no".to_string(),
        "--interactive".to_string(),
        "--tty".to_string(),
        "--device=/dev/kvm".to_string(),
        format!("--publish={}:10022", config.ssh_port),
        "--volume=/tmp/.X11-unix:/tmp/.X11-unix:rw".to_string(),
        format!("--volume={}:/env:ro", identity_path.display()),
        format!("--env=DISPLAY={display}"),
        format!("--env=RAM={}", config.memory_gib),
        format!("--env=SMP={}", config.cpu_cores),
        format!("--env=CORES={}", config.cpu_cores),
        "--env=WIDTH=1280".to_string(),
        "--env=HEIGHT=720".to_string(),
        "--env=EXTRA=-display gtk,zoom-to-fit=on".to_string(),
        format!("--env=SHORTNAME={}", config.macos_release.short_name()),
        "--env=CPU=Haswell-noTSX".to_string(),
        "--env=CPUID_FLAGS=kvm=on,vendor=GenuineIntel,+invtsc,vmware-cpuid-freq=on".to_string(),
        format!(
            "--env=MASTER_PLIST_URL={}",
            config.macos_release.master_plist_url()
        ),
        "--env=GENERATE_SPECIFIC=true".to_string(),
        "--env=GENERATE_UNIQUE=false".to_string(),
        "--env=NOPICKER=false".to_string(),
        DOCKER_IMAGE.to_string(),
    ]
}

fn run_docker(operation: &'static str, args: &[String]) -> Result<Output, ProviderError> {
    let output = Command::new("docker")
        .args(args)
        .output()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?;

    if output.status.success() {
        Ok(output)
    } else {
        Err(ProviderError::DockerCommand {
            operation,
            message: clean_output(&output.stderr),
        })
    }
}

fn clean_output(output: &[u8]) -> String {
    String::from_utf8_lossy(output)
        .trim()
        .chars()
        .take(4_000)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_is_valid_and_serializes_as_the_desktop_contract() {
        let config = MacBuilderConfig::default();

        config.validate().expect("default profile should be valid");

        assert_eq!(
            serde_json::to_value(config).expect("profile should serialize"),
            serde_json::json!({
                "name": "Local macOS builder",
                "macosRelease": "sequoia",
                "memoryGib": 8,
                "cpuCores": 4,
                "sshPort": 50922
            })
        );
    }

    #[test]
    fn unsafe_or_unusable_profile_values_are_rejected() {
        for config in [
            MacBuilderConfig {
                name: String::new(),
                ..MacBuilderConfig::default()
            },
            MacBuilderConfig {
                memory_gib: 2,
                ..MacBuilderConfig::default()
            },
            MacBuilderConfig {
                cpu_cores: 1,
                ..MacBuilderConfig::default()
            },
            MacBuilderConfig {
                ssh_port: 22,
                ..MacBuilderConfig::default()
            },
        ] {
            assert!(config.validate().is_err());
        }
    }

    #[test]
    fn docker_create_uses_fixed_argv_without_privileged_mode_or_secrets() {
        let args = create_args(
            &MacBuilderConfig::default(),
            ":1",
            Path::new("/tmp/buildbridge/identity.env"),
        );

        assert_eq!(args.first().map(String::as_str), Some("create"));
        assert!(args.contains(&"--device=/dev/kvm".to_string()));
        assert!(args.contains(&"--env=SHORTNAME=sequoia".to_string()));
        assert!(args.contains(&"--env=GENERATE_SPECIFIC=true".to_string()));
        assert!(args.contains(&"--env=GENERATE_UNIQUE=false".to_string()));
        assert!(args.contains(&"--env=WIDTH=1280".to_string()));
        assert!(args.contains(&"--env=HEIGHT=720".to_string()));
        assert!(args.contains(&"--env=EXTRA=-display gtk,zoom-to-fit=on".to_string()));
        assert!(args.contains(&"--restart=no".to_string()));
        assert!(!args.contains(&"--restart=unless-stopped".to_string()));
        assert!(args.contains(&"--volume=/tmp/buildbridge/identity.env:/env:ro".to_string()));
        assert!(args.contains(&DOCKER_IMAGE.to_string()));
        assert!(!args.iter().any(|arg| arg == "--privileged"));
        assert!(!args.iter().any(|arg| {
            let normalized = arg.to_ascii_lowercase();
            normalized.contains("password")
                || normalized.contains("private_key")
                || normalized.contains("secret")
        }));
    }

    #[test]
    fn identity_path_is_fixed_to_a_safe_absolute_filename() {
        assert!(validate_identity_path(Path::new("/tmp/builder/identity.env")).is_ok());
        assert!(validate_identity_path(Path::new("relative/identity.env")).is_err());
        assert!(validate_identity_path(Path::new("/tmp/builder/other.env")).is_err());
        assert!(validate_identity_path(Path::new("/tmp/bad:path/identity.env")).is_err());
    }

    #[test]
    fn a_missing_builder_container_is_an_expected_first_run_state() {
        assert!(is_missing_container_error(
            "error: no such object: buildbridge-macos-builder"
        ));
        assert!(is_missing_container_error(
            "Error response from daemon: No such container: buildbridge-macos-builder"
        ));
        assert!(!is_missing_container_error(
            "permission denied while trying to connect to the Docker daemon socket"
        ));
    }

    #[test]
    fn docker_started_at_ignores_the_unset_timestamp() {
        assert_eq!(
            normalize_started_at("2026-09-01T09:18:17.123456789Z"),
            Some("2026-09-01T09:18:17.123456789Z".to_string())
        );
        assert_eq!(normalize_started_at("0001-01-01T00:00:00Z"), None);
        assert_eq!(normalize_started_at("  "), None);
    }

    #[test]
    fn host_key_fingerprint_uses_the_openssh_sha256_format() {
        let fingerprint = fingerprint_host_key_line(
            "[127.0.0.1]:50922 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGUHHkjz1yOuYGQcsLcSGJXoXmyZg71dE7jb9HEyst2P",
        )
        .expect("fixture host key should parse");

        assert_eq!(
            fingerprint,
            "SHA256:SRqIbMNe4TLKp1BqYXY0QOA9HAC8+AiqO59E3Fg2lqE"
        );
    }

    #[test]
    fn malformed_or_ambiguous_host_key_pins_are_rejected() {
        for line in [
            "",
            "host ssh-rsa AQIDBA==",
            "host ssh-ed25519 not-base64",
            "host ssh-ed25519 AQIDBA== unexpected-field",
        ] {
            assert!(fingerprint_host_key_line(line).is_err());
        }
    }

    #[test]
    fn guest_username_is_safe_for_an_ssh_destination() {
        for username in ["matt", "build_user", "build.user", "_service", "build-user"] {
            assert!(valid_guest_username(username));
        }

        for username in ["", "-option", "user@host", "name space", "$(command)"] {
            assert!(!valid_guest_username(username));
        }
    }

    #[test]
    fn guest_script_content_is_single_quoted_without_interpolation() {
        assert_eq!(shell_single_quote("plain"), "'plain'");
        assert_eq!(shell_single_quote("it’s safe"), "'it’s safe'");
        assert_eq!(shell_single_quote("it's safe"), "'it'\"'\"'s safe'");
    }

    #[test]
    fn xcode_import_accepts_only_complete_xip_archives() {
        let fixture = std::env::temp_dir().join(format!(
            "buildbridge-xcode-package-{}.xip",
            std::process::id()
        ));
        fs::write(&fixture, b"xar!fixture").expect("fixture should be written");

        let (_, size) = validate_xcode_package(&fixture).expect("XIP fixture should validate");
        assert_eq!(size, 11);

        fs::write(&fixture, b"not-an-xip").expect("invalid fixture should be written");
        assert!(validate_xcode_package(&fixture).is_err());
        fs::remove_file(&fixture).expect("fixture should be removed");
    }

    #[test]
    fn workspace_snapshot_excludes_dependencies_outputs_and_secret_material() {
        for path in [
            "node_modules/package/index.js",
            ".git/config",
            ".ssh/id_ed25519",
            ".buildbridge/credentials.json",
            ".env.production",
            ".npmrc",
            "keys/AuthKey_ABC123.p8",
            "signing/distribution.P12",
            "ios/App/Pods/Manifest.lock",
            "ios/App/build/App.app/Info.plist",
            "android/app/build/outputs/app.apk",
            "ios/App/App.xcworkspace/xcuserdata/user.xcuserdatad/data.plist",
        ] {
            assert!(
                snapshot_path_excluded(Path::new(path)),
                "{path} should be excluded"
            );
        }

        for path in [
            "package.json",
            "pnpm-lock.yaml",
            "src/main.ts",
            "ios/App/Podfile.lock",
            "ios/App/App.xcworkspace/contents.xcworkspacedata",
        ] {
            assert!(
                !snapshot_path_excluded(Path::new(path)),
                "{path} should be included"
            );
        }
    }

    #[test]
    fn apple_build_progress_recognizes_the_platform_download_phase() {
        assert!(matches!(
            apple_project_phase("preparing_platform"),
            Some(AppleProjectPhase::PreparingPlatform)
        ));
        assert!(apple_project_phase("unexpected").is_none());
    }

    #[test]
    fn apple_build_progress_reports_platform_bytes_and_installation_state() {
        assert_eq!(
            apple_platform_progress(
                "__BUILDBRIDGE_PLATFORM_PROGRESS__:5301600256:10603200512:downloading"
            ),
            Some((
                5_301_600_256,
                10_603_200_512,
                "Downloading Apple's iOS Simulator platform"
            ))
        );
        assert_eq!(
            apple_platform_progress(
                "__BUILDBRIDGE_PLATFORM_PROGRESS__:10604044288:10603200512:installing"
            ),
            Some((
                10_603_200_512,
                10_603_200_512,
                "Simulator downloaded; macOS is installing and registering it"
            ))
        );
        assert!(apple_platform_progress("Finding content...").is_none());
    }

    #[test]
    fn apple_build_failure_preserves_actionable_diagnostics() {
        for line in [
            "error: simulator runtime is not ready",
            "/project/App.swift:42:7: error: missing argument",
            "Command CompileAssetCatalogVariant failed with a nonzero exit code",
            "** BUILD FAILED **",
            "The following build commands failed:",
            "    CompileAssetCatalogVariant thinned /tmp/App.app /project/Assets.xcassets",
        ] {
            assert!(
                apple_build_log_is_diagnostic(line),
                "{line} should remain in the failure context"
            );
        }

        assert!(!apple_build_log_is_diagnostic(
            "warning: Run script build phase will be run during every build"
        ));
    }

    #[test]
    fn apple_build_reports_the_bounded_post_install_retry() {
        assert_eq!(
            apple_build_retry_detail("__BUILDBRIDGE_BUILD_RETRY__:platform"),
            Some("Verifying the new Simulator runtime before one automatic retry")
        );
        assert!(apple_build_retry_detail("__BUILDBRIDGE_BUILD_RETRY__:unknown").is_none());
    }

    #[test]
    fn provisioning_profile_metadata_is_validated_and_matches_exact_or_wildcard_bundles() {
        let profiles = parse_profile_summaries(
            "__BUILDBRIDGE_PROFILE__\t01234567-89AB-CDEF-0123-456789ABCDEF\tF5QA294KSX\tF5QA294KSX.nz.co.thinksolar.app\t2027-09-02 12:00:00 +0000\n__BUILDBRIDGE_PROFILE_CERT__\t01234567-89AB-CDEF-0123-456789ABCDEF\t0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
        )
        .expect("valid profile metadata should parse");

        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].developer_certificate_sha256.len(), 1);
        assert!(profile_allows_bundle(
            &profiles[0].application_identifier,
            "nz.co.thinksolar.app"
        ));
        assert!(profile_allows_bundle(
            "F5QA294KSX.nz.co.thinksolar.*",
            "nz.co.thinksolar.app"
        ));
        assert!(!profile_allows_bundle(
            "F5QA294KSX.nz.co.another.app",
            "nz.co.thinksolar.app"
        ));
        assert!(
            parse_profile_summaries(
                "__BUILDBRIDGE_PROFILE__\t../../escape\tF5QA294KSX\tF5QA294KSX.*\t2027\n"
            )
            .is_err()
        );
    }

    #[test]
    fn signing_identity_and_certificate_metadata_are_bounded() {
        let metadata_command = certificate_metadata_command("/tmp/identity.der");
        assert!(metadata_command.contains("-startdate"));
        assert!(metadata_command.contains("certificate_end_epoch"));
        assert!(metadata_command.contains("the macOS guest clock is"));
        assert!(!metadata_command.contains("-checkend"));

        let intermediate_command = apple_wwdr_g3_import_command(
            "/tmp/AppleWWDRCAG3.pem",
            "/tmp/AppleWWDRCAG3.cer",
            "/tmp/signing.keychain-db",
        );
        assert!(intermediate_command.contains(APPLE_WWDR_G3_DER_SHA256));
        assert!(intermediate_command.contains("security import"));
        assert!(APPLE_WWDR_G3_PEM.starts_with(b"-----BEGIN CERTIFICATE-----\n"));

        let (expires_at, sha1, sha256, team, name) = parse_certificate_metadata(
            "notAfter=Sep  2 12:00:00 2027 GMT\nsha256 Fingerprint=01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF\nsubject=UID=ABC,CN=Apple Distribution: Example,OU=F5QA294KSX,O=Example,C=NZ\nsha1 Fingerprint=01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67\n",
        )
        .expect("certificate metadata should parse");
        assert_eq!(expires_at, "Sep  2 12:00:00 2027 GMT");
        assert_eq!(sha1, "0123456789ABCDEF0123456789ABCDEF01234567");
        assert_eq!(sha256.len(), 64);
        assert_eq!(team, "F5QA294KSX");
        assert_eq!(name, "Apple Distribution: Example");
    }

    #[test]
    fn signing_credentials_use_length_framing_instead_of_command_arguments() {
        let mut payload = Vec::new();
        write_secret_frame(&mut payload, "line1\nline2")
            .expect("a bounded credential should serialize");

        assert_eq!(&payload[..4], &(11_u32.to_be_bytes()));
        assert_eq!(&payload[4..], b"line1\nline2");
        let helper_source = String::from_utf8_lossy(SIGNING_HELPER_SOURCE);
        assert!(helper_source.contains("read_secret"));
        assert!(helper_source.contains("identity_count"));
        assert!(helper_source.contains("CSSM_ACL_AUTHORIZATION_PARTITION_ID"));
        assert!(helper_source.contains("SecKeychainItemSetAccessWithPassword"));
        assert!(helper_source.contains("SecKeychainUnlock"));
        assert!(helper_source.contains("execv"));
        assert!(helper_source.contains("\"--probe\""));
        assert!(helper_source.contains("\"--archive\""));
        assert!(helper_source.contains("\"-exportArchive\""));
        assert!(helper_source.contains("\"-xcconfig\""));
        assert!(!helper_source.contains("PROVISIONING_PROFILE_SPECIFIER"));
        assert!(helper_source.contains("CFSTR(\"apple-tool:\")"));
        assert!(helper_source.contains("CFSTR(\"apple:\")"));
        assert!(!helper_source.contains("set-key-partition-list"));
        assert!(!helper_source.contains("security import"));
    }

    #[test]
    fn signing_target_rejects_shell_or_wildcard_values() {
        assert!(validate_signing_target("F5QA294KSX", "nz.co.thinksolar.app").is_ok());
        assert!(validate_signing_target("$(command)", "nz.co.thinksolar.app").is_err());
        assert!(validate_signing_target("F5QA294KSX", "nz.co.*").is_err());
    }

    #[test]
    fn signed_archive_uses_a_fixed_manual_app_store_connect_recipe() {
        let options = apple_export_options_plist(
            "F5QA294KSX",
            "nz.co.thinksolar.app",
            "01234567-89AB-CDEF-0123-456789ABCDEF",
            "0123456789ABCDEF0123456789ABCDEF01234567",
        );

        assert!(options.contains("<string>app-store-connect</string>"));
        assert!(options.contains("<string>manual</string>"));
        assert!(options.contains("<key>nz.co.thinksolar.app</key>"));
        assert!(options.contains("<string>01234567-89AB-CDEF-0123-456789ABCDEF</string>"));
        assert!(options.contains("<key>manageAppVersionAndBuildNumber</key>\n    <false/>"));
        assert!(!options.contains("uploadDestination"));

        let signing_settings = apple_archive_signing_xcconfig(
            "App",
            "F5QA294KSX",
            "0123456789ABCDEF0123456789ABCDEF01234567",
            "01234567-89AB-CDEF-0123-456789ABCDEF",
        );
        assert!(signing_settings.contains("BUILDBRIDGE_PROFILE_App"));
        assert!(
            signing_settings
                .contains("PROVISIONING_PROFILE_SPECIFIER = $(BUILDBRIDGE_PROFILE_$(TARGET_NAME))")
        );
        assert_eq!(
            build_setting_value(
                "    TARGET_NAME = App\n    PRODUCT_BUNDLE_IDENTIFIER = nz.co.thinksolar.app",
                "TARGET_NAME"
            ),
            Some("App")
        );
    }

    #[test]
    fn signed_archive_metadata_is_strictly_validated() {
        let inspection = parse_apple_archive_inspection(
            "__BUILDBRIDGE_APP__\tnz.co.thinksolar.app\t3.2.0\t15\n__BUILDBRIDGE_IPA__\tApp.ipa\t1048576\t0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
        )
        .expect("bounded archive metadata should parse");

        assert_eq!(inspection.bundle_identifier, "nz.co.thinksolar.app");
        assert_eq!(inspection.marketing_version, "3.2.0");
        assert_eq!(inspection.build_number, "15");
        assert_eq!(inspection.ipa_name, "App.ipa");
        assert_eq!(inspection.ipa_bytes, 1_048_576);
        assert_eq!(
            inspection.ipa_sha256,
            "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF"
        );

        assert!(
            parse_apple_archive_inspection(
                "__BUILDBRIDGE_APP__\tnz.co.thinksolar.app\t3.2.0\t15\n__BUILDBRIDGE_IPA__\t../escape.ipa\t1\t0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n"
            )
            .is_err()
        );
    }
}
