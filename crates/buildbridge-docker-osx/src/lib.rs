//! Typed, host-side lifecycle management for a single Docker-OSX builder.
//!
//! All Docker calls use fixed argv assembled from validated profile fields.
//! No shell is involved and secret material is never passed to Docker.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

mod archive;
mod build_log;
mod device_run;
mod disk;
mod docker;
mod optimizations;
mod podfile;
mod process;
mod profiles;
mod qmp;
mod signing;
mod smoke_build;
mod ssh;
mod templates;
mod usb;
mod workspace;
mod xcode;
pub use archive::*;
use build_log::*;
pub use docker::*;
pub use optimizations::*;
pub use podfile::*;
pub use process::*;
pub use profiles::*;
pub use signing::*;
pub use smoke_build::*;
pub use ssh::*;
pub use templates::*;
pub use workspace::*;
pub use xcode::*;

pub use device_run::{
    AppleDeviceRunPhase, AppleDeviceRunProgress, AppleDeviceRunResult, ConsoleEnd, DeviceSigning,
    SafariInspectorResult, open_safari_web_inspector, pair_guest_device,
    resolve_debug_bundle_identifier, run_apple_device_build,
};
pub use device_run::{
    DeveloperModeState, GuestDevice, PairingState, TransportType, TunnelState, list_guest_devices,
};
pub use disk::{
    ContainerLayout, ContainerRebuildPhase, ContainerRebuildProgress, DISK_IMAGE_NAME,
    DISK_NVRAM_NAME, DiskMigrationPhase, DiskMigrationProgress, MachineDisk, ensure_machine_disk,
    inspect_container_layout, migrate_disk_to_host, rebuild_container, remove_machine_disk,
    required_free_bytes, validate_bind_path,
};
use qmp::USB_PHONE_CONTROLLER;
pub use qmp::{QMP_CONTAINER_DIR, QMP_SOCKET_NAME, guest_reset_for_macos};
use ts_rs::TS;
pub use usb::{
    AttachedUsbDevice, ContainerUsbOptions, HostUsbDevice, HostUsbStatus, MachineUsbStatus,
    USB_UDEV_RULE, USB_UDEV_RULE_PATH, UdevRuleState, UsbHolder, attach_usb_device,
    attached_usb_device, detach_usb_device, host_usb_status, install_iphone_udev_rule,
    is_apple_mobile_product, machine_usb_status, remove_iphone_udev_rule, resolve_usb_options,
    valid_usb_port_path,
};

/// Identifier of the builder that existed before BuildBridge kept a machine registry.
pub const DEFAULT_MACHINE_ID: &str = "default";
/// Container name of the legacy single builder; newer machines derive their own name.
pub const DEFAULT_CONTAINER_NAME: &str = "buildbridge-macos-builder";
pub const DOCKER_IMAGE: &str = "sickcodes/docker-osx:latest";

/// Returns whether a machine identifier is safe to use in container names and directories.
pub fn valid_machine_id(machine_id: &str) -> bool {
    !machine_id.is_empty()
        && machine_id.len() <= 40
        && !machine_id.starts_with('-')
        && !machine_id.ends_with('-')
        && machine_id.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

/// Maps a machine identifier to its managed Docker container name.
///
/// The legacy machine keeps the container it was created with so existing installations
/// continue to resolve after the registry migration.
pub fn container_name(machine_id: &str) -> String {
    if machine_id == DEFAULT_MACHINE_ID {
        DEFAULT_CONTAINER_NAME.to_string()
    } else {
        format!("buildbridge-macos-{machine_id}")
    }
}
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
const APPLE_BUILD_FAILURE_TAIL_LINES: usize = 12;
const APPLE_ARCHIVE_MAX_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const NODE_VERSION: &str = "24.20.0";
const NODE_DARWIN_X64_SHA256: &str =
    "9e5b2644cf107befb6aefca676b96d3296bc10138096f022ed378d6233ed81f4";
const PNPM_VERSION: &str = "11.5.0";
/// Homebrew's relocatable Ruby build for Intel macOS. CocoaPods is installed with it rather
/// than the guest's system Ruby: that one is Ruby 2.6, its headers ship only inside the SDK
/// for the running macOS release, which a newer Xcode no longer carries, and its universal
/// platform makes RubyGems pick arm64 binary gems on an x86_64 guest.
const PORTABLE_RUBY_VERSION: &str = "3.4.6";
const PORTABLE_RUBY_DARWIN_X64_SHA256: &str =
    "99bec6d4440dc4f114754f7b9e18d79258a6dacc4089a9a50638e22a1e8665d0";
const COCOAPODS_VERSION: &str = "1.16.2";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
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

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
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

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub prerequisites: HostPrerequisites,
    pub state: ContainerState,
    pub container_id: Option<String>,
    pub started_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum GuestTrustState {
    #[default]
    Unavailable,
    Untrusted,
    Trusted,
    Mismatch,
}

#[derive(Debug, Clone, Default, Serialize, TS)]
#[ts(export)]
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

#[derive(Debug, Clone, Default, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GuestDiagnostics {
    pub authenticated: bool,
    pub macos_version: Option<String>,
    pub xcode_version: Option<String>,
    pub xcode_path: Option<String>,
    pub xcode_selected: bool,
    /// The iOS Simulator runtime version the guest holds, when Xcode is ready and one is installed.
    pub ios_simulator_runtime: Option<String>,
    pub issue: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LaunchPhase {
    Preparing,
    PullingImage,
    GeneratingIdentity,
    PreparingDisk,
    CreatingContainer,
    Starting,
    Completed,
}

/// Everything a container needs from the host besides its profile: the identity file, the
/// directory holding the macOS disk, the control-socket directory, and USB access when the
/// host can grant it.
#[derive(Debug, Clone)]
pub struct LaunchOptions<'a> {
    pub identity_path: &'a Path,
    pub disk_dir: &'a Path,
    pub qmp_dir: &'a Path,
    pub usb: Option<ContainerUsbOptions>,
    /// The template this machine was cloned from, whose directory every container of the
    /// machine binds read-only; `None` for a machine installed from scratch.
    pub template_dir: Option<&'a Path>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct LaunchProgress {
    pub phase: LaunchPhase,
    #[ts(type = "number")]
    pub elapsed_seconds: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum XcodeImportPhase {
    Preparing,
    Transferring,
    Expanding,
    AwaitingActivation,
    AwaitingAuthorization,
    /// The bridge route is running the activation commands under `sudo`.
    Activating,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct XcodeImportProgress {
    pub phase: XcodeImportPhase,
    #[ts(type = "number")]
    pub transferred_bytes: u64,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub elapsed_seconds: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct XcodeImportResult {
    pub installed_path: String,
    pub activation_commands: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SigningProvisioningPhase {
    /// The Team key route: a distribution certificate is being created at Apple for the kit.
    CreatingCertificate,
    /// The Team key route: an App Store profile is being found or created at Apple for the kit.
    CreatingProfile,
    Preparing,
    Transferring,
    ImportingCertificate,
    InspectingProfiles,
    InstallingProfiles,
    Verifying,
    Completed,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SigningProvisioningProgress {
    pub phase: SigningProvisioningPhase,
    #[ts(type = "number")]
    pub completed_bytes: u64,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub elapsed_seconds: u64,
    pub detail: String,
}

/// What a profile is for, read from its entitlements rather than from a name. The plist has no
/// explicit type; the combination of `get-task-allow`, `ProvisionedDevices`, and
/// `ProvisionsAllDevices` is unambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    AppStore,
    Development,
    AdHoc,
    Enterprise,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningProfileSummary {
    pub uuid: String,
    pub team_identifier: String,
    pub application_identifier: String,
    pub expires_at: String,
    pub developer_certificate_sha256: Vec<String>,
    /// `None` on records written before kinds were read; the archive treats that as App Store.
    #[serde(default)]
    pub kind: Option<ProfileKind>,
    #[serde(default)]
    pub provisioned_device_udids: Vec<String>,
    #[serde(default)]
    pub get_task_allow: bool,
}

/// One certificate/private-key pair in the guest keychain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedIdentity {
    pub identity_name: String,
    pub identity_sha1: String,
    pub certificate_sha256: String,
    pub certificate_expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", from = "SigningProvisioningWire")]
pub struct SigningProvisioningResult {
    pub keychain_path: String,
    /// The distribution identity, when the kit holds one. An App Store archive needs it; a kit
    /// with only a development identity provisions, runs on a phone, and cannot archive.
    pub distribution_identity: Option<ProvisionedIdentity>,
    pub development_team: String,
    pub bundle_identifier: String,
    pub profiles: Vec<ProvisioningProfileSummary>,
    /// The development identity in the same keychain, when the kit holds one.
    pub development_identity: Option<ProvisionedIdentity>,
}

/// The stored shape, old and new. Records written before the distribution identity became
/// optional carried its four fields at the top level; they still read.
#[derive(Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
struct SigningProvisioningWire {
    keychain_path: String,
    #[serde(default)]
    distribution_identity: Option<ProvisionedIdentity>,
    #[serde(default)]
    identity_name: Option<String>,
    #[serde(default)]
    identity_sha1: Option<String>,
    #[serde(default)]
    certificate_sha256: Option<String>,
    #[serde(default)]
    certificate_expires_at: Option<String>,
    development_team: String,
    bundle_identifier: String,
    #[serde(default)]
    profiles: Vec<ProvisioningProfileSummary>,
    #[serde(default)]
    development_identity: Option<ProvisionedIdentity>,
}

impl From<SigningProvisioningWire> for SigningProvisioningResult {
    fn from(wire: SigningProvisioningWire) -> Self {
        let legacy = match (
            wire.identity_name,
            wire.identity_sha1,
            wire.certificate_sha256,
            wire.certificate_expires_at,
        ) {
            (
                Some(identity_name),
                Some(identity_sha1),
                Some(certificate_sha256),
                Some(certificate_expires_at),
            ) => Some(ProvisionedIdentity {
                identity_name,
                identity_sha1,
                certificate_sha256,
                certificate_expires_at,
            }),
            _ => None,
        };
        Self {
            keychain_path: wire.keychain_path,
            distribution_identity: wire.distribution_identity.or(legacy),
            development_team: wire.development_team,
            bundle_identifier: wire.bundle_identifier,
            profiles: wire.profiles,
            development_identity: wire.development_identity,
        }
    }
}

/// The files and passphrases a provisioning run imports: the distribution identity, the
/// development identity, and the profiles. A kit holds at least one identity; which ones it
/// holds decides what the machine can do afterwards.
pub struct SigningMaterial<'a> {
    pub distribution_certificate: Option<(&'a Path, &'a str)>,
    pub development_certificate: Option<(&'a Path, &'a str)>,
    pub profile_paths: &'a [PathBuf],
    pub keychain_password: &'a str,
    /// Identifiers besides the approved one that a profile in the kit may be for: the
    /// project's own Debug identifier, once the device step has registered it.
    pub extra_bundle_identifiers: &'a [String],
}

/// Whether the helper creates the keychain or adds to the one a previous import created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelperImportMode {
    Create,
    Add,
}

#[derive(Debug, Clone, Copy, Serialize, TS)]
#[ts(export)]
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

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AppleProjectProgress {
    pub phase: AppleProjectPhase,
    #[ts(type = "number")]
    pub completed_bytes: u64,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub elapsed_seconds: u64,
    pub detail: String,
    pub log_line: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AppleWorkspaceSyncResult {
    pub guest_path: String,
    pub snapshot_sha256: String,
    #[ts(type = "number")]
    pub source_file_count: u64,
    #[ts(type = "number")]
    pub source_bytes: u64,
    #[ts(type = "number")]
    pub archive_bytes: u64,
}

/// Which SDK the unsigned test build compiles against. The device SDK ships inside Xcode and is
/// what a signed archive and a device run use, so it needs nothing downloaded; the Simulator is
/// the only target that can be run on screen inside the guest, and Xcode lacks its runtime until
/// Apple's iOS platform — several gigabytes — has been downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum UnsignedBuildTarget {
    #[default]
    DeviceSdk,
    Simulator,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AppleSmokeBuildResult {
    pub target: UnsignedBuildTarget,
    pub xcode_version: String,
    pub native_lockfile_updated: bool,
    pub output_tail: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AppleArchivePhase {
    Preparing,
    /// Only when a build chooses an env set: the web assets are rebuilt with it first.
    BuildingWebAssets,
    Archiving,
    Exporting,
    Verifying,
    PackagingArchive,
    Transferring,
    Completed,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AppleArchiveProgress {
    pub phase: AppleArchivePhase,
    #[ts(type = "number")]
    pub completed_bytes: u64,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub elapsed_seconds: u64,
    pub detail: String,
    pub log_line: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AppleArchiveArtifact {
    pub path: String,
    #[ts(type = "number")]
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
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
    #[error("USB passthrough failed: {0}")]
    UsbPassthrough(String),
    #[error("host authorization failed: {0}")]
    HostAuthorization(String),
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

    let mut ios_simulator_runtime = None;
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
            if first_launch_ready {
                ios_simulator_runtime = run_guest_command(
                    ssh_port,
                    username,
                    identity_path,
                    known_hosts_path,
                    SIMULATOR_RUNTIME_PROBE,
                )
                .ok()
                .and_then(|output| parse_simulator_runtime(&output));
            }

            (
                Some(parse_xcode_version(&version)),
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
                    Some(parse_xcode_version(&version)),
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
        ios_simulator_runtime,
        issue,
    }
}

/// The first iOS runtime CoreSimulator lists, or nothing; `true` keeps the exit status clean when
/// there is none. Fixed text, so it can be handed to the guest shell as is.
const SIMULATOR_RUNTIME_PROBE: &str = "/usr/bin/xcrun simctl list runtimes 2>/dev/null | /usr/bin/grep '^iOS ' | /usr/bin/head -1; /usr/bin/true";

/// `iOS 26.0 (26.0 - 23A339) - com.apple.CoreSimulator.SimRuntime.iOS-26-0` → `26.0`. Anything
/// that is not a dotted version is dropped rather than shown.
fn parse_simulator_runtime(output: &str) -> Option<String> {
    output.lines().map(str::trim).find_map(|line| {
        let version = line.strip_prefix("iOS ")?.split_whitespace().next()?;
        let dotted = !version.is_empty()
            && version.len() <= 16
            && version
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.')
            && version
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit());
        dotted.then(|| version.to_string())
    })
}

/// The `xcodebuild` SDK and destination for an unsigned build of one target. Both stay unsigned;
/// only the SDK differs.
fn unsigned_build_destination_args(target: UnsignedBuildTarget) -> &'static str {
    match target {
        UnsignedBuildTarget::DeviceSdk => "-sdk iphoneos -destination 'generic/platform=iOS'",
        UnsignedBuildTarget::Simulator => {
            "-sdk iphonesimulator -destination 'generic/platform=iOS Simulator'"
        }
    }
}

fn unsigned_build_completed_detail(target: UnsignedBuildTarget) -> &'static str {
    match target {
        UnsignedBuildTarget::DeviceSdk => {
            "The unsigned iOS build against the device SDK completed successfully."
        }
        UnsignedBuildTarget::Simulator => {
            "The unsigned iOS Simulator build completed successfully."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_optimization_catalogue_is_fixed_and_self_describing() {
        let items = guest_optimizations();
        assert!(items.len() >= 12);
        let mut ids: Vec<&str> = items.iter().map(|item| item.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), items.len(), "ids are unique");
        for item in items {
            assert!(!item.apply.is_empty() && !item.check.is_empty());
            assert!(
                item.check.contains("applied"),
                "{} reports its state",
                item.id
            );
            if item.apply.contains("/usr/bin/sudo") {
                assert!(
                    item.needs_admin,
                    "{} uses sudo and must go through the Terminal",
                    item.id
                );
            }
            if item.tier == OptimizationTier::ExtremelyInsecure {
                assert!(item.warning.is_some(), "{} must carry its warning", item.id);
            }
        }
        assert!(guest_optimization("disable-spotlight").is_some());
        assert!(guest_optimization("rm-rf").is_none());
    }

    #[test]
    fn xcode_version_is_reported_without_the_label_the_interface_already_renders() {
        assert_eq!(
            parse_xcode_version("Xcode 26.6\nBuild version 17F113"),
            "26.6"
        );
        assert_eq!(parse_xcode_version("26.6"), "26.6");
        assert_eq!(parse_xcode_version(""), "version unknown");
    }

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
        let disk = MachineDisk::new(Path::new("/tmp/buildbridge/disk")).expect("valid");
        let args = create_args(
            "buildbridge-macos-team-mac",
            &MacBuilderConfig::default(),
            ":1",
            Path::new("/tmp/buildbridge/identity.env"),
            &disk,
            Path::new("/tmp/buildbridge/qmp"),
            Some(&ContainerUsbOptions { plugdev_gid: 46 }),
        );

        assert_eq!(args.first().map(String::as_str), Some("create"));
        assert!(args.contains(&"--name=buildbridge-macos-team-mac".to_string()));
        assert!(args.contains(&"--device=/dev/kvm".to_string()));
        assert!(args.contains(&"--env=SHORTNAME=sequoia".to_string()));
        assert!(args.contains(&"--env=GENERATE_SPECIFIC=true".to_string()));
        assert!(args.contains(&"--env=GENERATE_UNIQUE=false".to_string()));
        assert!(args.contains(&"--env=WIDTH=1280".to_string()));
        assert!(args.contains(&"--env=HEIGHT=720".to_string()));
        assert!(args.contains(
            &"--env=EXTRA=-display gtk,zoom-to-fit=on -qmp unix:/buildbridge-qmp/qmp.sock,server,nowait -device usb-ehci,id=buildbridge-phone-usb"
                .to_string()
        ));
        assert!(args.contains(&"--restart=no".to_string()));
        assert!(!args.contains(&"--restart=unless-stopped".to_string()));
        assert!(args.contains(&"--volume=/tmp/buildbridge/identity.env:/env:ro".to_string()));
        assert!(args.contains(&"--volume=/tmp/buildbridge/disk:/image:rw".to_string()));
        assert!(args.contains(&"--env=IMAGE_PATH=/image/mac_hdd_ng.img".to_string()));
        assert!(args.contains(&"--volume=/tmp/buildbridge/qmp:/buildbridge-qmp:rw".to_string()));
        assert!(args.contains(&"--device-cgroup-rule=c 189:* rwm".to_string()));
        assert!(args.contains(&"--volume=/dev/bus/usb:/dev/bus/usb".to_string()));
        assert!(args.contains(&"--group-add=46".to_string()));
        assert_eq!(args.last().map(String::as_str), Some(DOCKER_IMAGE));
        assert!(!args.iter().any(|arg| arg == "--privileged"));
        assert!(!args.iter().any(|arg| {
            let normalized = arg.to_ascii_lowercase();
            normalized.contains("password")
                || normalized.contains("private_key")
                || normalized.contains("secret")
        }));
    }

    #[test]
    fn device_udids_are_accepted_in_both_of_apples_shapes() {
        for udid in [
            "0123456789abcdef0123456789abcdef01234567",
            "00008030-001A2B3C4D5E6F00",
            "00008030-001a2b3c4d5e6f00",
        ] {
            assert!(valid_device_udid(udid), "{udid}");
        }
        for udid in [
            "",
            "0123456789abcdef0123456789abcdef0123456",
            "00008030_001A2B3C4D5E6F00",
            "00008030-001A2B3C4D5E6F0G",
            "../x",
            "00008030-001A2B3C4D5E6F00 ",
        ] {
            assert!(!valid_device_udid(udid), "{udid}");
        }
    }

    #[test]
    fn docker_create_without_a_plugdev_group_omits_usb_access_but_keeps_the_control_socket() {
        let disk = MachineDisk::new(Path::new("/tmp/buildbridge/disk")).expect("valid");
        let args = create_args(
            "buildbridge-macos-team-mac",
            &MacBuilderConfig::default(),
            ":1",
            Path::new("/tmp/buildbridge/identity.env"),
            &disk,
            Path::new("/tmp/buildbridge/qmp"),
            None,
        );

        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with("--device-cgroup-rule"))
        );
        assert!(!args.iter().any(|arg| arg.starts_with("--group-add")));
        assert!(!args.contains(&"--volume=/dev/bus/usb:/dev/bus/usb".to_string()));
        assert!(args.contains(&"--volume=/tmp/buildbridge/qmp:/buildbridge-qmp:rw".to_string()));
        assert!(args.iter().any(|arg| arg.contains("-qmp unix:")));
    }

    #[test]
    fn machine_identifiers_map_to_stable_container_names() {
        assert_eq!(container_name(DEFAULT_MACHINE_ID), DEFAULT_CONTAINER_NAME);
        assert_eq!(container_name("team-mac"), "buildbridge-macos-team-mac");
        assert!(valid_machine_id("default"));
        assert!(valid_machine_id("team-mac-2"));
        assert!(!valid_machine_id(""));
        assert!(!valid_machine_id("-leading"));
        assert!(!valid_machine_id("Upper"));
        assert!(!valid_machine_id("has space"));
        assert!(!valid_machine_id("../escape"));
        assert!(!valid_machine_id(&"a".repeat(41)));
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
    fn guest_public_key_must_be_one_plain_ed25519_line() {
        assert!(valid_guest_public_key(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample+Key/Material= buildbridge-guest"
        ));
        assert!(valid_guest_public_key(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample"
        ));

        for key in [
            "",
            "ssh-ed25519",
            "ssh-rsa AAAA buildbridge-guest",
            "ssh-ed25519 AAAA'; /bin/rm -rf /",
            "ssh-ed25519 AAAA comment extra",
            "ssh-ed25519 AAAA\ncommand",
            "ssh-ed25519 AAAA a comment",
        ] {
            assert!(!valid_guest_public_key(key), "{key:?} must be rejected");
        }
    }

    #[test]
    fn guest_password_is_one_bounded_line() {
        assert!(valid_guest_password("hunter2"));
        assert!(valid_guest_password("pässwörd with spaces and 'quotes'"));
        assert!(!valid_guest_password(""));
        assert!(!valid_guest_password("two\nlines"));
        assert!(!valid_guest_password(&"x".repeat(513)));
    }

    #[test]
    fn guest_key_install_appends_once_with_sshd_permissions_and_confirms() {
        let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample buildbridge-guest";
        let command = guest_key_install_command(key);

        assert!(command.starts_with(
            "umask 077; key='ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample buildbridge-guest'; "
        ));
        assert!(command.contains("/bin/chmod 700 \"$HOME/.ssh\""));
        assert!(command.contains("/usr/bin/grep -qxF \"$key\" \"$HOME/.ssh/authorized_keys\""));
        assert!(command.contains("/bin/chmod 600 \"$HOME/.ssh/authorized_keys\""));
        assert!(command.ends_with("/usr/bin/printf authorized"));
        assert_eq!(command.matches("ssh-ed25519").count(), 1);
    }

    #[test]
    fn password_session_keeps_the_pin_and_takes_the_password_from_the_helper_only() {
        let known_hosts = Path::new("/tmp/buildbridge-test/known_hosts");
        let askpass = Path::new("/tmp/buildbridge-test/askpass");
        let command = guest_password_ssh_command(50_922, "builder", known_hosts, askpass);
        let args: Vec<String> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();

        for expected in [
            "PubkeyAuthentication=no",
            "PasswordAuthentication=yes",
            "NumberOfPasswordPrompts=1",
            "StrictHostKeyChecking=yes",
            "UserKnownHostsFile=/tmp/buildbridge-test/known_hosts",
        ] {
            assert!(args.contains(&expected.to_string()), "{expected} missing");
        }
        assert!(!args.contains(&"BatchMode=yes".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("builder@127.0.0.1"));

        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(envs.contains(&(
            "SSH_ASKPASS".to_string(),
            Some("/tmp/buildbridge-test/askpass".to_string())
        )));
        assert!(envs.contains(&("SSH_ASKPASS_REQUIRE".to_string(), Some("force".to_string()))));
        assert!(envs.contains(&("SSH_AUTH_SOCK".to_string(), None)));
        assert!(!envs.iter().any(|(key, _)| key == GUEST_PASSWORD_ENV));
    }

    #[test]
    fn askpass_helper_is_private_echoes_only_its_variable_and_is_removed_after_use() {
        let helper = GuestAskpassHelper::create().expect("helper should be created");
        let script = helper.script();
        assert!(script.is_file());
        assert_eq!(
            fs::read_to_string(&script).expect("helper should be readable"),
            GUEST_ASKPASS_SCRIPT
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = fs::metadata(&helper.dir)
                .expect("helper directory should exist")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }

        let output = Command::new(&script)
            .arg("builder@127.0.0.1's password: ")
            .env(GUEST_PASSWORD_ENV, "s3cret 'value'")
            .output()
            .expect("helper should run");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"s3cret 'value'\n");

        let dir = helper.dir.clone();
        drop(helper);
        assert!(!dir.exists());
    }

    #[test]
    fn bridge_activation_runs_fixed_commands_under_one_sudo_that_reads_stdin_once() {
        let command = xcode_activation_sudo_command("/Users/builder/Applications/Xcode.app");

        assert!(command.starts_with("/usr/bin/sudo -S -k -p '' /bin/sh -c '"));
        assert!(command.contains("exec </dev/null;"));
        assert!(command.contains(
            "/usr/bin/xcode-select --switch '\"'\"'/Users/builder/Applications/Xcode.app'\"'\"'"
        ));
        assert!(command.contains("/usr/bin/xcodebuild -license accept"));
        assert!(command.contains("/usr/bin/xcodebuild -runFirstLaunch"));
        for marker in ["select", "license", "first-launch", "done"] {
            assert!(command.contains(&format!("{XCODE_ACTIVATION_MARKER}{marker}")));
        }
        assert!(!command.to_lowercase().contains("password"));
    }

    #[test]
    fn bridge_activation_failures_name_the_stage_and_never_the_password() {
        let rejected = describe_bridge_activation_failure(
            "builder",
            "authorizing",
            "Sorry, try again.\n\nsudo: no password was provided\nsudo: 1 incorrect password attempt",
        );
        assert!(rejected.contains("did not accept the password for builder"));
        assert!(rejected.contains("nothing was changed"));

        assert!(
            describe_bridge_activation_failure(
                "guest",
                "authorizing",
                "guest is not in the sudoers file."
            )
            .contains("not an administrator")
        );

        let midway = describe_bridge_activation_failure(
            "builder",
            "first-launch",
            "xcodebuild: error: installation failed",
        );
        assert!(midway.contains("during Xcode's first-launch tasks"));
        assert!(midway.contains("installation failed"));

        assert!(
            describe_bridge_activation_failure("builder", "license", "")
                .contains("while accepting the license; no message was returned")
        );
    }

    #[test]
    fn bridge_activation_reports_each_stage_in_plain_words() {
        assert_eq!(
            xcode_activation_step_detail("select"),
            "Selecting the developer directory"
        );
        assert!(xcode_activation_step_detail("first-launch").contains("several minutes"));
        assert_eq!(xcode_activation_step_detail("unknown"), "Activating Xcode");
        assert!(matches!(
            xcode_activation_progress("detail", 7),
            XcodeImportProgress {
                phase: XcodeImportPhase::Activating,
                elapsed_seconds: 7,
                ..
            }
        ));
    }

    #[test]
    fn key_install_refuses_to_send_a_password_without_a_pin() {
        let missing = Path::new("/nonexistent/buildbridge/known_hosts");
        let error = authorize_guest_key(
            50_922,
            "builder",
            "ssh-ed25519 AAAA buildbridge-guest",
            "secret",
            missing,
        )
        .expect_err("an unpinned guest must be refused");

        assert!(error.to_string().contains("pin"));
    }

    #[test]
    fn password_session_failures_name_the_cause_without_the_password() {
        let denied = describe_password_session_failure(
            "builder",
            "builder@127.0.0.1: Permission denied (publickey,password,keyboard-interactive).",
        );
        assert!(denied.contains("did not accept the password for builder"));
        assert!(
            describe_password_session_failure("builder", "Host key verification failed.")
                .contains("pinned fingerprint")
        );
        assert_eq!(
            describe_password_session_failure(
                "builder",
                "ssh: connect to host 127.0.0.1 port 50922: Connection refused"
            ),
            "ssh: connect to host 127.0.0.1 port 50922: Connection refused"
        );
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
    fn a_usb_capable_host_always_gets_the_phone_controller_and_never_a_phone_on_the_command_line() {
        let without_usb = qemu_extra_args(None);
        assert!(without_usb.contains("-qmp unix:"));
        assert!(!without_usb.contains("usb-ehci"));

        let with_usb = qemu_extra_args(Some(&ContainerUsbOptions { plugdev_gid: 46 }));
        assert!(
            with_usb.starts_with(&without_usb),
            "the console and socket are kept"
        );
        assert!(with_usb.ends_with(" -device usb-ehci,id=buildbridge-phone-usb"));
        // Phones are hot-plugged over QMP; nothing about a phone is baked into the container.
        assert!(!with_usb.contains("usb-host"));
        // The launch script word-splits this, so a stray quote or semicolon would be a hole.
        assert!(!with_usb.contains('\'') && !with_usb.contains(';') && !with_usb.contains("$("));
    }

    #[test]
    fn unsigned_build_targets_pick_the_sdk_and_default_to_the_device() {
        assert_eq!(
            UnsignedBuildTarget::default(),
            UnsignedBuildTarget::DeviceSdk
        );
        assert_eq!(
            unsigned_build_destination_args(UnsignedBuildTarget::DeviceSdk),
            "-sdk iphoneos -destination 'generic/platform=iOS'"
        );
        assert_eq!(
            unsigned_build_destination_args(UnsignedBuildTarget::Simulator),
            "-sdk iphonesimulator -destination 'generic/platform=iOS Simulator'"
        );
        assert_eq!(
            serde_json::to_string(&UnsignedBuildTarget::DeviceSdk).unwrap(),
            "\"device_sdk\""
        );
        assert_eq!(
            serde_json::from_str::<UnsignedBuildTarget>("\"simulator\"").unwrap(),
            UnsignedBuildTarget::Simulator
        );
    }

    #[test]
    fn a_signing_record_from_before_the_distribution_identity_was_optional_still_reads() {
        let legacy = r#"{"keychainPath":"/k","identityName":"iPhone Distribution: Example (TEAM123456)","identitySha1":"1111","certificateSha256":"aaaa","certificateExpiresAt":"2027-09-02T00:00:00Z","developmentTeam":"TEAM123456","bundleIdentifier":"com.example.app","profiles":[]}"#;
        let read: SigningProvisioningResult = serde_json::from_str(legacy).unwrap();
        let distribution = read.distribution_identity.as_ref().expect("legacy fields");
        assert_eq!(distribution.identity_sha1, "1111");
        assert_eq!(distribution.certificate_sha256, "aaaa");
        assert!(read.development_identity.is_none());

        // The new shape round-trips, with either identity absent.
        let development_only = SigningProvisioningResult {
            keychain_path: "/k".to_string(),
            distribution_identity: None,
            development_team: "TEAM123456".to_string(),
            bundle_identifier: "com.example.app".to_string(),
            profiles: Vec::new(),
            development_identity: Some(ProvisionedIdentity {
                identity_name: "Apple Development: Example (TEAM123456)".to_string(),
                identity_sha1: "2222".to_string(),
                certificate_sha256: "bbbb".to_string(),
                certificate_expires_at: "2027-09-02T00:00:00Z".to_string(),
            }),
        };
        let json = serde_json::to_string(&development_only).unwrap();
        assert!(json.contains("\"distributionIdentity\":null"));
        assert!(!json.contains("\"identityName\":\"iPhone"));
        assert_eq!(
            serde_json::from_str::<SigningProvisioningResult>(&json).unwrap(),
            development_only
        );
        // A development-only record never yields an App Store profile to archive with.
        assert!(select_app_store_profile(&development_only).is_none());
    }

    #[test]
    fn profile_flags_that_are_not_booleans_are_refused_and_the_probe_filters_them() {
        // plutil reports a missing key on stdout, so an unfiltered probe would hand the parser
        // its error text; the script keeps only a bare true/false and defaults to false.
        let polluted = "__BUILDBRIDGE_PROFILE__\t01234567-89AB-CDEF-0123-456789ABCDEF\tTEAM123456\tTEAM123456.com.example.app\t2027-09-02 12:00:00 +0000\n__BUILDBRIDGE_PROFILE_CERT__\t01234567-89AB-CDEF-0123-456789ABCDEF\t0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n__BUILDBRIDGE_PROFILE_FLAGS__\t01234567-89AB-CDEF-0123-456789ABCDEF\tfalse\tprofile.plist: Could not extract value\n";
        assert!(parse_profile_summaries(polluted).is_err());
        let script = profile_inspection_command("/tmp/p.mobileprovision");
        assert!(script.contains("Entitlements.get-task-allow raw -o - "));
        assert!(script.contains("| /usr/bin/grep -x -E 'true|false' || /bin/echo false"));
    }

    #[test]
    fn a_guest_podfile_lock_is_adopted_only_when_it_looks_like_one() {
        let lock = "PODS:\n  - Capacitor (8.4.2):\n    - CapacitorCordova\n  - CapacitorCordova (8.4.2)\n\nDEPENDENCIES:\n  - \"Capacitor (from `../../node_modules/@capacitor/ios`)\"\n\nSPEC CHECKSUMS:\n  Capacitor: 52f9\n\nPODFILE CHECKSUM: abcd\n\nCOCOAPODS: 1.16.2\n";
        assert!(validate_podfile_lock(lock).is_ok());
        assert!(validate_podfile_lock("").is_err());
        assert!(validate_podfile_lock("cat: No such file or directory\n").is_err());
        assert!(validate_podfile_lock(&lock.replace("COCOAPODS: ", "COCOA: ")).is_err());
        assert!(validate_podfile_lock(&format!("{lock}\u{7}")).is_err());
    }

    #[test]
    fn podfile_lock_changes_name_each_repinned_pod_and_count_the_lines() {
        let before = "PODS:\n  - Capacitor (8.3.4):\n    - CapacitorCordova\n  - CapacitorCamera (8.2.0):\n    - Capacitor\n  - CapacitorCordova (8.3.4)\n  - Gone (1.0.0)\n\nSPEC CHECKSUMS:\n  Capacitor: d13c\n\nCOCOAPODS: 1.16.2\n";
        let after = "PODS:\n  - Capacitor (8.4.2):\n    - CapacitorCordova\n  - CapacitorCamera (8.2.0):\n    - Capacitor\n  - CapacitorCordova (8.4.2)\n  - New (2.0.0)\n\nSPEC CHECKSUMS:\n  Capacitor: 52f9\n\nCOCOAPODS: 1.16.2\n";
        let changes = podfile_lock_changes(before, after);
        assert!(!changes.identical);
        let names = changes
            .pods
            .iter()
            .map(|change| {
                format!(
                    "{} {}→{}",
                    change.name,
                    change.before.as_deref().unwrap_or("-"),
                    change.after.as_deref().unwrap_or("-")
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "Capacitor 8.3.4→8.4.2",
                "CapacitorCordova 8.3.4→8.4.2",
                "Gone 1.0.0→-",
                "New -→2.0.0",
            ]
        );
        // Three repinned lines plus the checksum line, each way.
        assert_eq!(changes.lines_removed, 4);
        assert_eq!(changes.lines_added, 4);
        assert!(podfile_lock_changes(before, before).identical);
    }

    #[test]
    fn simulator_runtime_probe_reads_the_ios_version_and_ignores_other_platforms() {
        assert_eq!(
            parse_simulator_runtime(
                "iOS 26.0 (26.0 - 23A339) - com.apple.CoreSimulator.SimRuntime.iOS-26-0\n"
            ),
            Some("26.0".to_string())
        );
        assert_eq!(
            parse_simulator_runtime("== Runtimes ==\nwatchOS 26.0 (26.0 - 23R356) - runtime\n"),
            None
        );
        assert_eq!(parse_simulator_runtime(""), None);
        assert_eq!(parse_simulator_runtime("iOS $(rm -rf /) - runtime"), None);
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
            "make: *** No rule to make target `universal-darwin24/ruby/config.h', needed by `nkf.o'.  Stop.",
            "ERROR: Failed to build gem native extension.",
        ] {
            assert!(
                apple_build_log_is_diagnostic(line),
                "{line} should remain in the failure context"
            );
        }

        for line in [
            "warning: Run script build phase will be run during every build",
            "Building native extensions. This could take a while...",
        ] {
            assert!(
                !apple_build_log_is_diagnostic(line),
                "{line} is not a diagnostic"
            );
        }
    }

    #[test]
    fn build_failures_report_the_diagnostics_and_the_last_lines_the_guest_printed() {
        let tail = [
            "Successfully installed ffi-1.17.0-arm64-darwin",
            "Building native extensions. This could take a while...",
            "current directory: /Users/builder/.buildbridge/tools/gems/gems/nkf-0.3.0/ext/nkf",
            "make \"DESTDIR=\"",
            "make: *** No rule to make target `universal-darwin24/ruby/config.h', needed by `nkf.o'.  Stop.",
            "make failed, exit code 2",
            "ERROR: Error installing cocoapods:",
            "ERROR: Failed to build gem native extension.",
        ]
        .map(String::from);
        let diagnostics = tail
            .iter()
            .filter(|line| apple_build_log_is_diagnostic(line))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(diagnostics.len(), 3);

        // Every line once, in the order the guest printed it: the tail already holds the
        // diagnostics, so they are not repeated ahead of it.
        assert_eq!(build_failure_context(&diagnostics, &tail), tail.join("\n"));

        // A diagnostic that scrolled out of the tail is kept ahead of the bounded tail.
        let early = vec!["/project/App.swift:42:7: error: missing argument".to_string()];
        let long_tail = (0..20)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>();
        let context = build_failure_context(&early, &long_tail);
        let lines = context.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), APPLE_BUILD_FAILURE_TAIL_LINES + 1);
        assert_eq!(lines[0], early[0]);
        assert_eq!(lines[1], "line 8");
        assert_eq!(lines[APPLE_BUILD_FAILURE_TAIL_LINES], "line 19");

        assert_eq!(build_failure_context(&[], &[]), "");
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
            "__BUILDBRIDGE_PROFILE__\t01234567-89AB-CDEF-0123-456789ABCDEF\tTEAM123456\tTEAM123456.com.example.app\t2027-09-02 12:00:00 +0000\n__BUILDBRIDGE_PROFILE_CERT__\t01234567-89AB-CDEF-0123-456789ABCDEF\t0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n__BUILDBRIDGE_PROFILE_FLAGS__\t01234567-89AB-CDEF-0123-456789ABCDEF\tfalse\tfalse\n",
        )
        .expect("valid profile metadata should parse");

        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].developer_certificate_sha256.len(), 1);
        assert_eq!(profiles[0].kind, Some(ProfileKind::AppStore));
        assert!(profiles[0].provisioned_device_udids.is_empty());

        let development = parse_profile_summaries(
            "__BUILDBRIDGE_PROFILE__\t22222222-3333-4444-5555-666666666666\tTEAM123456\tTEAM123456.com.example.app\t2027-09-02 12:00:00 +0000\n__BUILDBRIDGE_PROFILE_CERT__\t22222222-3333-4444-5555-666666666666\tbbbb1111cccc2222dddd3333eeee4444ffff5555aaaa6666bbbb7777cccc8888\n__BUILDBRIDGE_PROFILE_FLAGS__\t22222222-3333-4444-5555-666666666666\ttrue\tfalse\n__BUILDBRIDGE_PROFILE_DEVICE__\t22222222-3333-4444-5555-666666666666\t00008030-000a1b2c3d4e5f6a\n",
        )
        .expect("development profile metadata should parse");
        assert_eq!(development[0].kind, Some(ProfileKind::Development));
        assert!(development[0].get_task_allow);
        assert_eq!(
            development[0].provisioned_device_udids,
            vec!["00008030-000A1B2C3D4E5F6A".to_string()]
        );
        assert!(
            parse_profile_summaries(
                "__BUILDBRIDGE_PROFILE__\t01234567-89AB-CDEF-0123-456789ABCDEF\tTEAM123456\tTEAM123456.com.example.app\t2027\n__BUILDBRIDGE_PROFILE_CERT__\t01234567-89AB-CDEF-0123-456789ABCDEF\t0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n"
            )
            .is_err(),
            "the entitlement flags are required"
        );
        assert_eq!(classify_profile(false, false, 0), ProfileKind::AppStore);
        assert_eq!(classify_profile(true, false, 2), ProfileKind::Development);
        assert_eq!(classify_profile(false, false, 2), ProfileKind::AdHoc);
        assert_eq!(classify_profile(false, true, 0), ProfileKind::Enterprise);
        assert!(profile_allows_bundle(
            &profiles[0].application_identifier,
            "com.example.app"
        ));
        assert!(profile_allows_bundle(
            "TEAM123456.com.example.*",
            "com.example.app"
        ));
        assert!(!profile_allows_bundle(
            "TEAM123456.nz.co.another.app",
            "com.example.app"
        ));
        assert!(
            parse_profile_summaries(
                "__BUILDBRIDGE_PROFILE__\t../../escape\tTEAM123456\tTEAM123456.*\t2027\n"
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
            "notAfter=Sep  2 12:00:00 2027 GMT\nsha256 Fingerprint=01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF\nsubject=UID=ABC,CN=Apple Distribution: Example,OU=TEAM123456,O=Example,C=NZ\nsha1 Fingerprint=01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67\n",
        )
        .expect("certificate metadata should parse");
        assert_eq!(expires_at, "Sep  2 12:00:00 2027 GMT");
        assert_eq!(sha1, "0123456789ABCDEF0123456789ABCDEF01234567");
        assert_eq!(sha256.len(), 64);
        assert_eq!(team, "TEAM123456");
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
        assert!(validate_signing_target("TEAM123456", "com.example.app").is_ok());
        assert!(validate_signing_target("$(command)", "com.example.app").is_err());
        assert!(validate_signing_target("TEAM123456", "nz.co.*").is_err());
    }

    #[test]
    fn signed_archive_uses_a_fixed_manual_app_store_connect_recipe() {
        let options = apple_export_options_plist(
            "TEAM123456",
            "com.example.app",
            "01234567-89AB-CDEF-0123-456789ABCDEF",
            "0123456789ABCDEF0123456789ABCDEF01234567",
        );

        assert!(options.contains("<string>app-store-connect</string>"));
        assert!(options.contains("<string>manual</string>"));
        assert!(options.contains("<key>com.example.app</key>"));
        assert!(options.contains("<string>01234567-89AB-CDEF-0123-456789ABCDEF</string>"));
        assert!(options.contains("<key>manageAppVersionAndBuildNumber</key>\n    <false/>"));
        assert!(!options.contains("uploadDestination"));

        let signing_settings = apple_archive_signing_xcconfig(
            "App",
            "TEAM123456",
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
                "    TARGET_NAME = App\n    PRODUCT_BUNDLE_IDENTIFIER = com.example.app",
                "TARGET_NAME"
            ),
            Some("App")
        );
    }

    #[test]
    fn signed_archive_metadata_is_strictly_validated() {
        let inspection = parse_apple_archive_inspection(
            "__BUILDBRIDGE_APP__\tcom.example.app\t3.2.0\t15\n__BUILDBRIDGE_IPA__\tApp.ipa\t1048576\t0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
        )
        .expect("bounded archive metadata should parse");

        assert_eq!(inspection.bundle_identifier, "com.example.app");
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
                "__BUILDBRIDGE_APP__\tcom.example.app\t3.2.0\t15\n__BUILDBRIDGE_IPA__\t../escape.ipa\t1\t0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n"
            )
            .is_err()
        );
    }
}
