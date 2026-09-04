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

mod device_run;
mod disk;
mod qmp;
mod usb;

pub use device_run::{
    AppleDeviceRunPhase, AppleDeviceRunProgress, AppleDeviceRunResult, ConsoleEnd, DeviceSigning,
    pair_guest_device, run_apple_device_build,
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
pub use qmp::{QMP_CONTAINER_DIR, QMP_SOCKET_NAME};
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
    /// The iOS Simulator runtime version the guest holds, when Xcode is ready and one is installed.
    pub ios_simulator_runtime: Option<String>,
    pub issue: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchProgress {
    pub phase: LaunchPhase,
    pub elapsed_seconds: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
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

/// What a profile is for, read from its entitlements rather than from a name. The plist has no
/// explicit type; the combination of `get-task-allow`, `ProvisionedDevices`, and
/// `ProvisionsAllDevices` is unambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    AppStore,
    Development,
    AdHoc,
    Enterprise,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedIdentity {
    pub identity_name: String,
    pub identity_sha1: String,
    pub certificate_sha256: String,
    pub certificate_expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Deserialize)]
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
}

/// Whether the helper creates the keychain or adds to the one a previous import created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelperImportMode {
    Create,
    Add,
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

/// Which SDK the unsigned test build compiles against. The device SDK ships inside Xcode and is
/// what a signed archive and a device run use, so it needs nothing downloaded; the Simulator is
/// the only target that can be run on screen inside the guest, and Xcode lacks its runtime until
/// Apple's iOS platform — several gigabytes — has been downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsignedBuildTarget {
    #[default]
    DeviceSdk,
    Simulator,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSmokeBuildResult {
    pub target: UnsignedBuildTarget,
    pub xcode_version: String,
    pub native_lockfile_updated: bool,
    pub output_tail: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
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

/// One long-running operation on one machine, as the thing a **Stop** button acts on.
///
/// Every child process the crate starts while the scope is entered — every `ssh`, `docker`
/// and `tar` — is registered here, so cancelling kills what is actually running rather than
/// leaving a build to finish in the background. The scope is thread-local because each
/// operation already runs on its own blocking thread; nothing in the crate's signatures has to
/// know about it.
#[derive(Debug, Default)]
pub struct OperationScope {
    cancelled: AtomicBool,
    children: Mutex<Vec<u32>>,
}

impl OperationScope {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Marks the operation cancelled and terminates every child it has running. Later spawns
    /// under this scope refuse to start.
    pub fn cancel(&self) -> usize {
        self.cancelled.store(true, Ordering::Release);
        let pids = self
            .children
            .lock()
            .map(|children| children.clone())
            .unwrap_or_default();
        for pid in &pids {
            terminate_process(*pid);
        }

        pids.len()
    }

    fn register(&self, pid: u32) {
        if let Ok(mut children) = self.children.lock() {
            children.push(pid);
        }
    }

    fn unregister(&self, pid: u32) {
        if let Ok(mut children) = self.children.lock() {
            children.retain(|child| *child != pid);
        }
    }
}

fn terminate_process(pid: u32) {
    #[cfg(windows)]
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    #[cfg(not(windows))]
    let _ = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

thread_local! {
    static CURRENT_SCOPE: std::cell::RefCell<Option<Arc<OperationScope>>> =
        const { std::cell::RefCell::new(None) };
}

/// Makes `scope` the current operation on this thread until the returned guard drops.
pub fn enter_operation(scope: Arc<OperationScope>) -> OperationGuard {
    CURRENT_SCOPE.with(|current| *current.borrow_mut() = Some(scope));
    OperationGuard(())
}

pub struct OperationGuard(());

impl Drop for OperationGuard {
    fn drop(&mut self) {
        CURRENT_SCOPE.with(|current| *current.borrow_mut() = None);
    }
}

fn current_scope() -> Option<Arc<OperationScope>> {
    CURRENT_SCOPE.with(|current| current.borrow().clone())
}

/// A child process that the current operation scope knows about. The child is optional only
/// so `wait_with_output`, which consumes it, can take it out before the drop unregisters it.
struct TrackedChild {
    child: Option<Child>,
    scope: Option<Arc<OperationScope>>,
}

impl TrackedChild {
    fn wait_with_output(mut self) -> std::io::Result<Output> {
        let child = self
            .child
            .take()
            .expect("a tracked child is present until consumed");
        let pid = child.id();
        let output = child.wait_with_output();
        if let Some(scope) = &self.scope {
            scope.unregister(pid);
        }
        output
    }
}

impl std::ops::Deref for TrackedChild {
    type Target = Child;

    fn deref(&self) -> &Child {
        self.child
            .as_ref()
            .expect("a tracked child is present until consumed")
    }
}

impl std::ops::DerefMut for TrackedChild {
    fn deref_mut(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("a tracked child is present until consumed")
    }
}

impl Drop for TrackedChild {
    fn drop(&mut self) {
        if let (Some(scope), Some(child)) = (&self.scope, &self.child) {
            scope.unregister(child.id());
        }
    }
}

trait TrackedCommand {
    fn tracked_spawn(&mut self) -> std::io::Result<TrackedChild>;
    fn tracked_output(&mut self) -> std::io::Result<Output>;
}

impl TrackedCommand for Command {
    fn tracked_spawn(&mut self) -> std::io::Result<TrackedChild> {
        let scope = current_scope();
        if scope.as_ref().is_some_and(|scope| scope.is_cancelled()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "the operation was stopped",
            ));
        }
        let child = self.spawn()?;
        if let Some(scope) = &scope {
            scope.register(child.id());
        }

        Ok(TrackedChild {
            child: Some(child),
            scope,
        })
    }

    fn tracked_output(&mut self) -> std::io::Result<Output> {
        self.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .tracked_spawn()?
            .wait_with_output()
    }
}

/// How much a guest optimization changes about the machine's security posture, in the words
/// the osx-optimizer project uses for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationTier {
    /// Faster builds, no meaningful change to who can do what on the machine.
    Recommended,
    /// Trades some protection for convenience; the source marks these "at your own risk".
    AtYourOwnRisk,
    /// Removes authentication inside the guest. Only defensible for a VM nothing else can reach.
    ExtremelyInsecure,
}

/// One tweak from sickcodes/osx-optimizer as BuildBridge can run it: a fixed script for the
/// guest, a check that reports whether it is already in effect, and the caveat the source gives.
/// Admin tweaks run in the guest's own Terminal, where `sudo` reads the password from its TTY;
/// BuildBridge never sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestOptimization {
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub tier: OptimizationTier,
    /// The caveat, in the source's words where it gives one.
    pub warning: Option<&'static str>,
    pub needs_admin: bool,
    #[serde(skip)]
    pub apply: &'static str,
    /// Prints `applied` or `not_applied`; anything else reads as unknown.
    #[serde(skip)]
    pub check: &'static str,
}

pub fn guest_optimizations() -> &'static [GuestOptimization] {
    &[
        GuestOptimization {
            id: "disable-spotlight",
            title: "Disable Spotlight indexing",
            summary: "Stops the indexer that otherwise churns through every synchronized project and every Xcode install. The single biggest win for a virtual machine.",
            tier: OptimizationTier::Recommended,
            warning: Some(
                "Spotlight stops finding apps and files; `sudo mdutil -i on -a` turns it back on.",
            ),
            needs_admin: true,
            apply: "/usr/bin/sudo /usr/bin/mdutil -i off -a",
            check: "if /usr/bin/mdutil -s / 2>/dev/null | /usr/bin/grep -qi 'disabled'; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "performance-mode",
            title: "Enable performance mode",
            summary: "Sets Apple's server performance mode in the boot arguments, which dedicates more system resources to long-running processes such as builds.",
            tier: OptimizationTier::Recommended,
            warning: Some("Takes effect after the next restart of the guest."),
            needs_admin: true,
            apply: r#"/usr/bin/sudo /usr/sbin/nvram boot-args="serverperfmode=1 $(/usr/sbin/nvram boot-args 2>/dev/null | /usr/bin/cut -f 2-)""#,
            check: "if /usr/sbin/nvram boot-args 2>/dev/null | /usr/bin/grep -q 'serverperfmode=1'; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "reduce-motion",
            title: "Reduce motion and transparency",
            summary: "Turns off the animations and blur the console window otherwise has to render through QEMU.",
            tier: OptimizationTier::Recommended,
            warning: None,
            needs_admin: false,
            apply: "/usr/bin/defaults write com.apple.Accessibility DifferentiateWithoutColor -int 1 && /usr/bin/defaults write com.apple.Accessibility ReduceMotionEnabled -int 1 && /usr/bin/defaults write com.apple.universalaccess reduceMotion -int 1 && /usr/bin/defaults write com.apple.universalaccess reduceTransparency -int 1",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read com.apple.universalaccess reduceMotion 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "no-state-restore",
            title: "Do not restore apps on login",
            summary: "Stops macOS reopening whatever was running at shutdown, so a restart comes up clean and faster.",
            tier: OptimizationTier::Recommended,
            warning: Some("This may be slower for you depending on what you are doing."),
            needs_admin: false,
            apply: "/usr/bin/defaults write com.apple.loginwindow TALLogoutSavesState -bool false",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read com.apple.loginwindow TALLogoutSavesState 2>/dev/null)\" = 0; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "no-app-nap",
            title: "Keep apps from sleeping",
            summary: "Disables App Nap so background processes such as Xcode are never throttled into a sleeping state.",
            tier: OptimizationTier::Recommended,
            warning: Some("This increases RAM usage."),
            needs_admin: false,
            apply: "/usr/bin/defaults write NSGlobalDomain NSAppSleepDisabled -bool YES",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read NSGlobalDomain NSAppSleepDisabled 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "lighter-login",
            title: "Lighter login screen",
            summary: "Drops the login wallpaper and shows a plain name and password prompt instead of a list of users.",
            tier: OptimizationTier::Recommended,
            warning: None,
            needs_admin: true,
            apply: r#"/usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.loginwindow DesktopPicture "" && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.loginwindow.plist SHOWFULLNAME -bool true && /usr/bin/defaults write com.apple.loginwindow AllowList -string '*'"#,
            check: "if /usr/bin/test \"$(/usr/bin/defaults read /Library/Preferences/com.apple.loginwindow SHOWFULLNAME 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "disable-updates",
            title: "Disable software updates",
            summary: "Stops macOS downloading multi-gigabyte updates in the background, which is what makes a virtual disk grow out of proportion.",
            tier: OptimizationTier::AtYourOwnRisk,
            warning: Some(
                "At your own risk: the guest stops receiving security updates. Update it deliberately instead.",
            ),
            needs_admin: true,
            apply: "/usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.SoftwareUpdate AutomaticDownload -bool false && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.SoftwareUpdate AutomaticCheckEnabled -bool false && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.SoftwareUpdate ConfigDataInstall -int 0 && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.SoftwareUpdate CriticalUpdateInstall -int 0 && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.SoftwareUpdate ScheduleFrequency -int 0 && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.commerce AutoUpdate -bool false && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.commerce AutoUpdateRestartRequired -bool false",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read /Library/Preferences/com.apple.SoftwareUpdate AutomaticDownload 2>/dev/null)\" = 0; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "auto-login",
            title: "Skip the login screen",
            summary: "Logs the console straight into the user account at boot instead of stopping at the login window.",
            tier: OptimizationTier::AtYourOwnRisk,
            warning: Some("At your own risk: anyone who can see the console is logged in."),
            needs_admin: false,
            apply: "/usr/bin/defaults write com.apple.loginwindow autoLoginUser -bool true",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read com.apple.loginwindow autoLoginUser 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "disable-screen-lock",
            title: "Disable screen locking",
            summary: "Keeps the console session unlocked so a build is never waiting behind a lock screen.",
            tier: OptimizationTier::AtYourOwnRisk,
            warning: Some("The console never asks for a password again once it is logged in."),
            needs_admin: false,
            apply: "/usr/bin/defaults write com.apple.loginwindow DisableScreenLock -bool true",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read com.apple.loginwindow DisableScreenLock 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "osascript-over-ssh",
            title: "Allow automation over SSH",
            summary: "Lets scripts run over SSH drive apps with osascript without the accessibility and full-disk-access prompts.",
            tier: OptimizationTier::AtYourOwnRisk,
            warning: Some("Anything that can open an SSH session can then automate the desktop."),
            needs_admin: false,
            apply: "/usr/bin/defaults write com.apple.universalaccessAuthWarning /System/Applications/Utilities/Terminal.app -bool true && /usr/bin/defaults write com.apple.universalaccessAuthWarning /usr/libexec -bool true && /usr/bin/defaults write com.apple.universalaccessAuthWarning /usr/libexec/sshd-keygen-wrapper -bool true && /usr/bin/defaults write com.apple.universalaccessAuthWarning com.apple.Terminal -bool true",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read com.apple.universalaccessAuthWarning /usr/libexec/sshd-keygen-wrapper 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "multi-sessions",
            title: "Enable multiple sessions",
            summary: "Allows more than one user session at a time, so a console login and an SSH-driven build do not fight over the one session.",
            tier: OptimizationTier::AtYourOwnRisk,
            warning: None,
            needs_admin: true,
            apply: r#"/usr/bin/sudo /usr/bin/defaults write .GlobalPreferences MultipleSessionsEnabled -bool TRUE && /usr/bin/defaults write "Apple Global Domain" MultipleSessionsEnabled -bool true"#,
            check: "if /usr/bin/test \"$(/usr/bin/defaults read 'Apple Global Domain' MultipleSessionsEnabled 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "remote-management",
            title: "Enable remote management",
            summary: "Turns on Apple Remote Desktop screen sharing for every user, so the guest can be watched and driven without the QEMU console.",
            tier: OptimizationTier::AtYourOwnRisk,
            warning: Some(
                "At your own risk: every account on the guest can then be reached over the network.",
            ),
            needs_admin: true,
            apply: "/usr/bin/sudo /System/Library/CoreServices/RemoteManagement/ARDAgent.app/Contents/Resources/kickstart -activate -configure -access -off -restart -agent -privs -all -allowAccessFor -allUsers",
            check: "if /usr/bin/pgrep -x ARDAgent >/dev/null 2>&1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "disable-passwords",
            title: "Disable passwords globally",
            summary: "Rewrites every PAM policy so no password is ever required: everyone is root, sudo never asks, and SSH password login accepts an empty password.",
            tier: OptimizationTier::ExtremelyInsecure,
            warning: Some(
                "These macOS optimizations should only be used in CI/CD, behind a VPN, and with no external connectivity. This is not a warning, it is absolutely essential, or anyone can just SSH into the remote mac.",
            ),
            needs_admin: true,
            apply: r#"for PAM_FILE in /etc/pam.d/*; do /usr/bin/sudo /usr/bin/sed -i -e 's/required/optional/g' -e 's/sufficient/optional/g' "$PAM_FILE"; done"#,
            check: "if /usr/bin/grep -q 'required' /etc/pam.d/sudo 2>/dev/null; then /usr/bin/printf not_applied; else /usr/bin/printf applied; fi",
        },
        GuestOptimization {
            id: "everyone-sudoer",
            title: "Make every user a passwordless sudoer",
            summary: "Writes a NOPASSWD sudoers rule for every account under /Users, so scripts can use sudo without a password.",
            tier: OptimizationTier::ExtremelyInsecure,
            warning: Some(
                "These macOS optimizations should only be used in CI/CD, behind a VPN, and with no external connectivity. Any account on the guest becomes root without a password.",
            ),
            needs_admin: true,
            apply: r#"for USER_DIR in /Users/*; do REAL_NAME=$(/usr/bin/basename "$USER_DIR"); if /usr/bin/test "$REAL_NAME" = Shared; then continue; fi; /usr/bin/printf '%s ALL=(ALL) NOPASSWD: ALL\n' "$REAL_NAME" | /usr/bin/sudo /usr/bin/tee "/etc/sudoers.d/$REAL_NAME" >/dev/null; /usr/bin/sudo /bin/chmod 440 "/etc/sudoers.d/$REAL_NAME"; done"#,
            check: "if /usr/bin/sudo -n /usr/bin/true >/dev/null 2>&1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
    ]
}

pub fn guest_optimization(id: &str) -> Option<&'static GuestOptimization> {
    guest_optimizations().iter().find(|item| item.id == id)
}

/// Reports which optimizations are in effect, in one round trip: every check prints its id
/// and state on its own line, and a check that cannot run reads as unknown rather than as
/// "not applied", so nothing is offered as undone when the truth is unknowable.
pub fn check_guest_optimizations(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<Vec<(&'static str, Option<bool>)>, ProviderError> {
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    let script = guest_optimizations()
        .iter()
        .map(|item| {
            format!(
                "/usr/bin/printf '%s=' {}; ({}) 2>/dev/null || /usr/bin/printf unknown; /usr/bin/printf '\\n'",
                shell_single_quote(item.id),
                item.check
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let output = run_guest_command(ssh_port, username, identity_path, known_hosts_path, &script)?;

    Ok(guest_optimizations()
        .iter()
        .map(|item| {
            let state = output
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{}=", item.id)))
                .map(str::trim);
            let applied = match state {
                Some("applied") => Some(true),
                Some("not_applied") => Some(false),
                _ => None,
            };
            (item.id, applied)
        })
        .collect())
}

/// Applies one optimization. A user-level tweak runs straight over the bridge; an admin tweak
/// opens the guest's Terminal with a fixed command file so `sudo` reads the password from its
/// own TTY. Either way the script is the catalogue's, verbatim.
pub fn apply_guest_optimization<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    optimization_id: &str,
    mut on_waiting: F,
) -> Result<(), ProviderError>
where
    F: FnMut(u64),
{
    let item = guest_optimization(optimization_id).ok_or_else(|| {
        ProviderError::GuestBridge("that optimization is not in the catalogue".to_string())
    })?;
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;

    if !item.needs_admin {
        run_guest_command(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            item.apply,
        )?;
        return Ok(());
    }

    let task_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let guest_cache = format!("/Users/{username}/Library/Caches/dev.buildbridge.desktop");
    let status_path = format!("{guest_cache}/optimize-{task_id}.status");
    let script_path = format!("{guest_cache}/optimize-{task_id}.command");
    let terminal_script = format!(
        r#"#!/bin/zsh
/usr/bin/clear
/usr/bin/printf "BuildBridge: {title}\n\n"
/usr/bin/printf "Enter the local macOS login password when sudo asks.\n"
/usr/bin/printf "The password remains inside this macOS Terminal.\n\n"
trap "/usr/bin/printf \"failed:interrupted\\n\" > {status_path}" EXIT
if ( {apply} ); then
    trap - EXIT
    /usr/bin/printf "success\n" > {status_path}
    /usr/bin/printf "\nDone. You can close this window.\n"
else
    result=$?
    trap - EXIT
    /usr/bin/printf "failed:%s\n" "$result" > {status_path}
    /usr/bin/printf "\nThat did not complete. Return to BuildBridge.\n"
fi
read -k 1 "?Press any key to close this window."
"#,
        title = item.title,
        apply = item.apply,
    );
    let quoted = shell_single_quote(&terminal_script);
    let remote_command = format!(
        "/bin/mkdir -p '{guest_cache}'; /bin/rm -f '{status_path}' '{script_path}'; /usr/bin/printf '%s' {quoted} > '{script_path}'; /bin/chmod 700 '{script_path}'; if ! /usr/bin/open -a Terminal '{script_path}'; then /usr/bin/printf launch_failed; exit 0; fi; remaining=900; while /bin/test ! -f '{status_path}' && /bin/test \"$remaining\" -gt 0; do /bin/sleep 1; remaining=$((remaining - 1)); done; if /bin/test -f '{status_path}'; then /bin/cat '{status_path}'; /bin/rm -f '{status_path}' '{script_path}'; else /usr/bin/printf timeout; fi"
    );
    let started_at = Instant::now();
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not open the guest Terminal: {error}"))
        })?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                on_waiting(started_at.elapsed().as_secs());
                thread::sleep(Duration::from_secs(1));
            }
            Err(error) => {
                return Err(ProviderError::GuestBridge(format!(
                    "could not monitor the guest Terminal: {error}"
                )));
            }
        }
    };
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not read the guest Terminal result: {error}"))
    })?;
    if !status.success() {
        return Err(ProviderError::GuestBridge(clean_output(&output.stderr)));
    }
    match clean_output(&output.stdout).as_str() {
        "success" => Ok(()),
        "launch_failed" => Err(ProviderError::GuestBridge(
            "macOS could not open a Terminal window; log in on the console and try again"
                .to_string(),
        )),
        "timeout" => Err(ProviderError::GuestBridge(
            "no password was entered in the guest Terminal within 15 minutes".to_string(),
        )),
        other => Err(ProviderError::GuestBridge(format!(
            "the guest reported: {}",
            if other.is_empty() { "no result" } else { other }
        ))),
    }
}

/// Stops any BuildBridge job still running inside the guest after its host-side operation was
/// cancelled. The test build deliberately survives a dropped SSH session so a desktop restart
/// can reattach; a Stop button must reach past that.
pub fn stop_guest_jobs(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    let jobs = format!("/Users/{username}/.buildbridge/tools/jobs");
    let script = format!(
        "for pid_file in {}/*/pid; do if /bin/test -f \"$pid_file\"; then pid=$(/bin/cat \"$pid_file\"); pgid=$(/bin/ps -o pgid= -p \"$pid\" 2>/dev/null | /usr/bin/tr -d ' '); if /bin/test -n \"$pgid\"; then /bin/kill -TERM -- \"-$pgid\" 2>/dev/null || /bin/true; fi; /bin/kill -TERM \"$pid\" 2>/dev/null || /bin/true; fi; done; /bin/rm -rf {}/*; /usr/bin/true",
        shell_single_quote(&jobs),
        shell_single_quote(&jobs)
    );
    run_guest_command(ssh_port, username, identity_path, known_hosts_path, &script).map(|_| ())
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

/// Installs the BuildBridge public key into the guest user's `authorized_keys` over one
/// password-authenticated SSH session — the `ssh-copy-id` route. The session is pinned to the
/// already-trusted host key, so the password only ever reaches the machine whose fingerprint
/// was verified. The password is handed to `ssh` through the environment of that one process
/// and read back by a fixed askpass helper; it is never an argument, never written to disk,
/// and never part of an error. Every later guest operation authenticates with the key.
pub fn authorize_guest_key(
    ssh_port: u16,
    username: &str,
    public_key: &str,
    password: &str,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
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
    if !known_hosts_path.is_file() {
        return Err(ProviderError::GuestBridge(
            "pin the guest SSH fingerprint before sending it a password".to_string(),
        ));
    }
    if !valid_guest_public_key(public_key) {
        return Err(ProviderError::GuestBridge(
            "the BuildBridge guest public key is invalid".to_string(),
        ));
    }
    if !valid_guest_password(password) {
        return Err(ProviderError::GuestBridge(
            "enter the local macOS login password: up to 512 characters on one line".to_string(),
        ));
    }

    let askpass = GuestAskpassHelper::create()?;
    let output =
        guest_password_ssh_command(ssh_port, username, known_hosts_path, &askpass.script())
            .env(GUEST_PASSWORD_ENV, password)
            .arg(guest_key_install_command(public_key))
            .tracked_output()
            .map_err(|error| {
                ProviderError::GuestBridge(format!(
                    "could not run ssh; install OpenSSH client tools: {error}"
                ))
            })?;
    drop(askpass);

    if !output.status.success() {
        return Err(ProviderError::GuestBridge(
            describe_password_session_failure(username, &clean_output(&output.stderr)),
        ));
    }
    match clean_output(&output.stdout).as_str() {
        "authorized" => Ok(()),
        other => Err(ProviderError::GuestBridge(format!(
            "the guest did not confirm the key install: {}",
            if other.is_empty() { "no result" } else { other }
        ))),
    }
}

fn describe_password_session_failure(username: &str, stderr: &str) -> String {
    if stderr.contains("Permission denied") {
        format!(
            "macOS did not accept the password for {username}. Check the short username and the local macOS login password, or add the key from the guest Terminal instead."
        )
    } else if stderr.contains("Host key verification failed")
        || stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
    {
        "the guest SSH identity no longer matches the pinned fingerprint; forget the pin only after verifying this is the expected machine".to_string()
    } else if stderr.is_empty() {
        "the password-authenticated SSH session failed without a message".to_string()
    } else {
        stderr.to_string()
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
    let installed_path =
        ensure_imported_xcode(ssh_port, username, identity_path, known_hosts_path)?;

    let started_at = Instant::now();
    let detail = "A macOS Terminal window is opening. Enter the local macOS login password there; BuildBridge does not receive or store it.";
    on_progress(XcodeImportProgress {
        phase: XcodeImportPhase::AwaitingAuthorization,
        transferred_bytes: 0,
        total_bytes: 0,
        elapsed_seconds: 0,
        detail: detail.to_string(),
    });

    // The no-password route. A bare SSH process cannot display Authorization Services UI in
    // the console audit session, so open a fixed, short-lived command file in the guest's
    // Terminal: sudo reads the password from its macOS TTY and it never leaves the guest. The
    // bridge route below is the alternative when the password is typed into the desktop.
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
        .tracked_spawn()
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

    verify_xcode_activation(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &installed_path,
    )
}

/// Marker the bridge route prints between its commands so progress can name the one running.
const XCODE_ACTIVATION_MARKER: &str = "__BUILDBRIDGE_ACTIVATION__:";

/// Activates Xcode over the pinned bridge instead of the guest Terminal. The local macOS login
/// password is written once to the SSH session's stdin, where a single `sudo -S` reads it and
/// runs the same fixed `xcode-select`, license, and first-launch commands the Terminal route
/// runs. The password is never an argument on either side, never a file, and is gone when the
/// session ends. Output streams back so the interface can say which command is running.
pub fn activate_xcode_with_password<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    password: &str,
    mut on_progress: F,
) -> Result<(), ProviderError>
where
    F: FnMut(XcodeImportProgress),
{
    if !valid_guest_password(password) {
        return Err(ProviderError::GuestBridge(
            "enter the local macOS login password: up to 512 characters on one line".to_string(),
        ));
    }
    let installed_path =
        ensure_imported_xcode(ssh_port, username, identity_path, known_hosts_path)?;

    let started_at = Instant::now();
    let mut step = "authorizing".to_string();
    let mut detail = xcode_activation_step_detail(&step).to_string();
    on_progress(xcode_activation_progress(&detail, 0));

    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(xcode_activation_sudo_command(&installed_path))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not start Xcode activation over the bridge: {error}"
            ))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge(
            "could not hand the password to the activation session".to_string(),
        )
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture Xcode activation output".to_string())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture Xcode activation errors".to_string())
    })?;
    // sudo reads exactly one line. Closing stdin right after is what stops it asking again, and
    // a session that died before reading explains itself through its exit status below.
    let _ = stdin
        .write_all(password.as_bytes())
        .and_then(|()| stdin.write_all(b"\n"))
        .and_then(|()| stdin.flush());
    drop(stdin);

    let (lines, received) = std::sync::mpsc::channel::<String>();
    let stdout_reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut stderr = stderr;
        let _ = stderr.read_to_end(&mut output);
        output
    });
    loop {
        match received.recv_timeout(Duration::from_secs(1)) {
            Ok(line) => {
                if let Some(reached) = line.strip_prefix(XCODE_ACTIVATION_MARKER) {
                    step = reached.trim().to_string();
                    detail = xcode_activation_step_detail(&step).to_string();
                } else {
                    let line = sanitize_build_log_line(&line);
                    if !line.is_empty() {
                        detail = format!("{} · {line}", xcode_activation_step_detail(&step));
                    }
                }
                on_progress(xcode_activation_progress(
                    &detail,
                    started_at.elapsed().as_secs(),
                ));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                on_progress(xcode_activation_progress(
                    &detail,
                    started_at.elapsed().as_secs(),
                ));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = stdout_reader.join();
    let status = child.wait().map_err(|error| {
        ProviderError::GuestBridge(format!("could not monitor Xcode activation: {error}"))
    })?;
    let stderr = clean_output(&stderr_reader.join().unwrap_or_default());

    if !status.success() {
        return Err(ProviderError::GuestBridge(
            describe_bridge_activation_failure(username, &step, &stderr),
        ));
    }

    verify_xcode_activation(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &installed_path,
    )
}

/// The one remote command of the bridge route: `sudo` takes the password from stdin, ignores any
/// cached credential, prints nothing as a prompt, and runs the fixed activation script, whose
/// own stdin is `/dev/null` so nothing downstream can read the session again.
fn xcode_activation_sudo_command(installed_path: &str) -> String {
    let script = format!(
        "exec </dev/null; /usr/bin/printf '{marker}select\\n'; /usr/bin/xcode-select --switch '{installed_path}' && /usr/bin/printf '{marker}license\\n' && /usr/bin/xcodebuild -license accept && /usr/bin/printf '{marker}first-launch\\n' && /usr/bin/xcodebuild -runFirstLaunch && /usr/bin/printf '{marker}done\\n'",
        marker = XCODE_ACTIVATION_MARKER
    );

    format!(
        "/usr/bin/sudo -S -k -p '' /bin/sh -c {}",
        shell_single_quote(&script)
    )
}

fn xcode_activation_step_detail(step: &str) -> &'static str {
    match step {
        "authorizing" => "Authorizing with sudo over the pinned bridge",
        "select" => "Selecting the developer directory",
        "license" => "Accepting Apple's license",
        "first-launch" => {
            "Running Xcode's first-launch tasks and installing required components. This can take several minutes."
        }
        "done" => "Activation finished; verifying",
        _ => "Activating Xcode",
    }
}

fn xcode_activation_progress(detail: &str, elapsed_seconds: u64) -> XcodeImportProgress {
    XcodeImportProgress {
        phase: XcodeImportPhase::Activating,
        transferred_bytes: 0,
        total_bytes: 0,
        elapsed_seconds,
        detail: detail.to_string(),
    }
}

fn describe_bridge_activation_failure(username: &str, step: &str, stderr: &str) -> String {
    if stderr.contains("incorrect password attempt") || stderr.contains("Sorry, try again") {
        return format!(
            "macOS did not accept the password for {username}; nothing was changed. Check the local macOS login password, or leave it blank to type it in the guest Terminal."
        );
    }
    if stderr.contains("not in the sudoers") || stderr.contains("not allowed to") {
        return format!(
            "{username} is not an administrator on the guest, so sudo refused. Activate with an administrator account or use the manual commands."
        );
    }

    let stage = match step {
        "authorizing" => " before any command ran",
        "select" => " while selecting the developer directory",
        "license" => " while accepting the license",
        "first-launch" => " during Xcode's first-launch tasks",
        _ => "",
    };
    if stderr.is_empty() {
        format!("macOS could not complete Xcode activation{stage}; no message was returned")
    } else {
        format!("macOS could not complete Xcode activation{stage}: {stderr}")
    }
}

/// Confirms the imported Xcode application is where the import left it and returns its path.
fn ensure_imported_xcode(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<String, ProviderError> {
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

    Ok(installed_path)
}

/// Checks that activation, whichever route ran it, left the expected developer directory
/// selected and first launch complete.
fn verify_xcode_activation(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    installed_path: &str,
) -> Result<(), ProviderError> {
    let selected_path = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        "/usr/bin/xcode-select --print-path",
    )
    .map_err(|_| {
        ProviderError::GuestBridge(
            "Xcode remains inactive. Retry activation with the local macOS login password, not the Apple Account password."
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
            "Xcode was selected, but macOS still reports incomplete first-launch setup. Retry activation and let it finish."
                .to_string(),
        )
    })?;

    Ok(())
}

/// A Podfile.lock is small; anything past this is not one.
pub const PODFILE_LOCK_MAX_BYTES: usize = 1024 * 1024;

/// One pod whose pinned version differs between two lockfiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PodfileLockChange {
    pub name: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PodfileLockChanges {
    pub pods: Vec<PodfileLockChange>,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub identical: bool,
}

/// The Podfile.lock CocoaPods wrote in the guest workspace, bounded and checked for the shape
/// of a lockfile before the host adopts it.
pub fn read_guest_podfile_lock(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<String, ProviderError> {
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    let lock = format!("/Users/{username}/BuildBridge/workspaces/active/ios/App/Podfile.lock");
    let content = device_run::run_guest_command_capped(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!("/bin/cat {}", shell_single_quote(&lock)),
        PODFILE_LOCK_MAX_BYTES,
    )?;
    validate_podfile_lock(&content)?;
    Ok(content)
}

/// Non-empty, bounded, printable, and shaped like CocoaPods wrote it: a `PODS:` section first
/// and the `COCOAPODS:` version line somewhere after.
fn validate_podfile_lock(content: &str) -> Result<(), ProviderError> {
    let printable = content
        .chars()
        .all(|character| !character.is_control() || matches!(character, '\n' | '\t' | '\r'));
    if content.is_empty()
        || content.len() > PODFILE_LOCK_MAX_BYTES
        || !printable
        || !content.starts_with("PODS:")
        || !content.contains("\nCOCOAPODS: ")
    {
        return Err(ProviderError::GuestBridge(
            "the guest workspace holds no readable Podfile.lock".to_string(),
        ));
    }
    Ok(())
}

/// The top-level pods of a lockfile's `PODS:` section — `  - Name (version)` — by name.
fn podfile_lock_pods(content: &str) -> Vec<(String, String)> {
    let mut pods = Vec::new();
    let mut in_pods = false;
    for line in content.lines() {
        if line == "PODS:" {
            in_pods = true;
            continue;
        }
        if in_pods && !line.starts_with(' ') {
            break;
        }
        let Some(entry) = line.strip_prefix("  - ") else {
            continue;
        };
        if line.starts_with("    ") {
            continue;
        }
        let entry = entry.trim_end_matches(':');
        let Some((name, rest)) = entry.split_once(" (") else {
            continue;
        };
        let Some(version) = rest.strip_suffix(')') else {
            continue;
        };
        pods.push((name.to_string(), version.to_string()));
    }
    pods
}

/// What adopting `after` over `before` changes: each pod whose pin differs, and the raw line
/// counts either way, so the change can be judged before it is committed.
pub fn podfile_lock_changes(before: &str, after: &str) -> PodfileLockChanges {
    if before == after {
        return PodfileLockChanges {
            identical: true,
            ..PodfileLockChanges::default()
        };
    }
    let before_pods = podfile_lock_pods(before);
    let after_pods = podfile_lock_pods(after);
    let mut pods = Vec::new();
    for (name, version) in &before_pods {
        match after_pods.iter().find(|(other, _)| other == name) {
            Some((_, after_version)) if after_version == version => {}
            Some((_, after_version)) => pods.push(PodfileLockChange {
                name: name.clone(),
                before: Some(version.clone()),
                after: Some(after_version.clone()),
            }),
            None => pods.push(PodfileLockChange {
                name: name.clone(),
                before: Some(version.clone()),
                after: None,
            }),
        }
    }
    for (name, version) in &after_pods {
        if !before_pods.iter().any(|(other, _)| other == name) {
            pods.push(PodfileLockChange {
                name: name.clone(),
                before: None,
                after: Some(version.clone()),
            });
        }
    }

    let mut counts: HashMap<&str, i64> = HashMap::new();
    for line in before.lines() {
        *counts.entry(line).or_default() += 1;
    }
    for line in after.lines() {
        *counts.entry(line).or_default() -= 1;
    }
    let lines_removed = counts.values().filter(|count| **count > 0).sum::<i64>() as usize;
    let lines_added = counts
        .values()
        .filter(|count| **count < 0)
        .map(|count| -count)
        .sum::<i64>() as usize;

    PodfileLockChanges {
        pods,
        lines_added,
        lines_removed,
        identical: false,
    }
}

/// What one identity import needs beyond the identity itself: the guest helper, the keychain,
/// the pinned Apple intermediate, and the bridge to reach them.
struct IdentityImportContext<'a> {
    ssh_port: u16,
    username: &'a str,
    identity_path: &'a Path,
    known_hosts_path: &'a Path,
    helper_binary: &'a str,
    keychain_path: &'a str,
    staging: &'a str,
    xcodebuild: &'a str,
    keychain_password: &'a str,
    development_team: &'a str,
    wwdr_g3_pem: &'a str,
    wwdr_g3_der: &'a str,
    total_bytes: u64,
    started_at: Instant,
}

/// Streams one `.p12` into the staging directory, imports it through the helper (creating the
/// keychain for the first identity, adding to it for the second), installs the pinned Apple
/// intermediate alongside the first, checks the team, and proves the private key can sign.
#[allow(clippy::too_many_arguments)]
fn import_signing_identity<F>(
    context: &IdentityImportContext<'_>,
    host_path: &Path,
    password: &str,
    stem: &str,
    label: &str,
    mode: HelperImportMode,
    completed_bytes: &mut u64,
    on_progress: &mut F,
) -> Result<ProvisionedIdentity, ProviderError>
where
    F: FnMut(SigningProvisioningProgress),
{
    let guest_p12 = format!("{}/{stem}.p12", context.staging);
    let guest_der = format!("{}/{stem}.der", context.staging);
    stream_signing_file(
        host_path,
        &guest_p12,
        context.total_bytes,
        completed_bytes,
        context.started_at,
        context.ssh_port,
        context.username,
        context.identity_path,
        context.known_hosts_path,
        on_progress,
    )?;
    on_progress(signing_progress(
        SigningProvisioningPhase::ImportingCertificate,
        context.total_bytes,
        context.total_bytes,
        context.started_at,
        match mode {
            HelperImportMode::Create => {
                "Creating the dedicated keychain and importing its non-extractable identity."
            }
            HelperImportMode::Add => "Importing the second identity into the same keychain.",
        },
    ));
    run_signing_helper(
        context.ssh_port,
        context.username,
        context.identity_path,
        context.known_hosts_path,
        context.helper_binary,
        context.keychain_path,
        &guest_p12,
        &guest_der,
        context.xcodebuild,
        context.keychain_password,
        password,
        mode,
    )?;

    on_progress(signing_progress(
        SigningProvisioningPhase::Verifying,
        context.total_bytes,
        context.total_bytes,
        context.started_at,
        "Verifying the imported certificate and proving that its private key can sign code.",
    ));
    let output = run_guest_command(
        context.ssh_port,
        context.username,
        context.identity_path,
        context.known_hosts_path,
        &certificate_metadata_command(&guest_der),
    )?;
    let (certificate_expires_at, identity_sha1, certificate_sha256, team, identity_name) =
        parse_certificate_metadata(&output)?;
    if team != context.development_team {
        return Err(ProviderError::GuestBridge(format!(
            "the {label} belongs to team {team}, but the project uses team {}",
            context.development_team
        )));
    }
    if mode == HelperImportMode::Create {
        install_apple_wwdr_g3_intermediate(
            context.wwdr_g3_pem,
            context.wwdr_g3_der,
            context.keychain_path,
            context.ssh_port,
            context.username,
            context.identity_path,
            context.known_hosts_path,
        )?;
    }
    verify_code_signing_identity(
        &identity_sha1,
        context.helper_binary,
        context.keychain_path,
        context.staging,
        context.keychain_password,
        context.ssh_port,
        context.username,
        context.identity_path,
        context.known_hosts_path,
    )?;

    Ok(ProvisionedIdentity {
        identity_name,
        identity_sha1,
        certificate_sha256,
        certificate_expires_at,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn provision_signing<F>(
    material: &SigningMaterial<'_>,
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
    let keychain_password = material.keychain_password;
    let distribution = material
        .distribution_certificate
        .map(|(path, password)| {
            validate_identity_material(path, password, "signing certificate")
                .map(|(file, bytes)| (file, password, bytes))
        })
        .transpose()?;
    let development = material
        .development_certificate
        .map(|(path, password)| {
            validate_identity_material(path, password, "development certificate")
                .map(|(file, bytes)| (file, password, bytes))
        })
        .transpose()?;
    if distribution.is_none() && development.is_none() {
        return Err(ProviderError::GuestBridge(
            "the signing kit holds no identity: store a distribution identity or a development identity first"
                .to_string(),
        ));
    }
    let (profile_paths, profile_bytes) = validate_signing_material(
        material.profile_paths,
        keychain_password,
        distribution.is_some(),
    )?;
    let total_bytes = profile_bytes
        + distribution
            .as_ref()
            .map(|(_, _, bytes)| *bytes)
            .unwrap_or(0)
        + development
            .as_ref()
            .map(|(_, _, bytes)| *bytes)
            .unwrap_or(0);
    if total_bytes > SIGNING_MATERIAL_MAX_BYTES {
        return Err(ProviderError::GuestBridge(
            "the signing kit is larger than the material a provisioning run accepts".to_string(),
        ));
    }
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

        // The first identity creates the keychain and the second joins it, so one unlock serves
        // both the archive and a device build. Each gets the team check and the signing probe.
        let context = IdentityImportContext {
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            helper_binary: &helper_binary,
            keychain_path: &keychain_path,
            staging: &staging,
            xcodebuild: &xcodebuild,
            keychain_password,
            development_team,
            wwdr_g3_pem: &guest_wwdr_g3_pem,
            wwdr_g3_der: &guest_wwdr_g3_der,
            total_bytes,
            started_at,
        };
        let mut mode = HelperImportMode::Create;
        let distribution_identity = match &distribution {
            Some((path, password, _)) => {
                let identity = import_signing_identity(
                    &context,
                    path,
                    password,
                    "identity",
                    "signing certificate",
                    mode,
                    &mut completed_bytes,
                    &mut on_progress,
                )?;
                mode = HelperImportMode::Add;
                Some(identity)
            }
            None => None,
        };
        let development_identity = match &development {
            Some((path, password, _)) => Some(import_signing_identity(
                &context,
                path,
                password,
                "development",
                "development certificate",
                mode,
                &mut completed_bytes,
                &mut on_progress,
            )?),
            None => None,
        };

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
            // Development profiles carry the development certificate; App Store, ad hoc and
            // enterprise profiles all carry the distribution one.
            let (required_fingerprint, identity_label) =
                if profile.kind == Some(ProfileKind::Development) {
                    (
                        development_identity
                            .as_ref()
                            .map(|identity| identity.certificate_sha256.as_str()),
                        "development",
                    )
                } else {
                    (
                        distribution_identity
                            .as_ref()
                            .map(|identity| identity.certificate_sha256.as_str()),
                        "distribution",
                    )
                };
            let Some(required_fingerprint) = required_fingerprint else {
                return Err(ProviderError::GuestBridge(format!(
                    "provisioning profile {} is a {identity_label} profile, but the kit has no {identity_label} identity",
                    profile.uuid
                )));
            };
            if !profile
                .developer_certificate_sha256
                .iter()
                .any(|fingerprint| fingerprint == required_fingerprint)
            {
                return Err(ProviderError::GuestBridge(format!(
                    "provisioning profile {} does not include the kit's {identity_label} certificate",
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
            distribution_identity,
            development_team: development_team.to_string(),
            bundle_identifier: bundle_identifier.to_string(),
            profiles,
            development_identity,
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
        "Signing identities and provisioning profiles are ready in macOS.",
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

/// A build's environment, rendered twice because its two readers quote differently: Vite's
/// dotenv loader reads `.env.production.local`, and the guest build shell sources `env.sh`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestEnvFiles {
    pub dotenv: String,
    pub shell: String,
}

pub fn sync_apple_workspace<F>(
    workspace_path: &Path,
    env: Option<&GuestEnvFiles>,
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
    let guest_env = format!("{guest_root}/active.env");
    let guest_env_shell = format!("{guest_root}/active.env.sh");
    let prepare = format!(
        "/bin/mkdir -p '{guest_root}'; /bin/rm -rf '{guest_staging}'; /bin/rm -f '{guest_archive}' '{guest_env}' '{guest_env_shell}'"
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

    // The environment travels the same pinned channel as the source, as owner-only files that
    // land next to it: one for Vite, one for the build shell. Never as command arguments.
    if let Some(env) = env {
        stream_bytes_to_guest(
            env.dotenv.as_bytes(),
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_env,
            "env file",
        )?;
        stream_bytes_to_guest(
            env.shell.as_bytes(),
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_env_shell,
            "env script",
        )?;
    }

    on_progress(apple_progress(
        AppleProjectPhase::Extracting,
        archive_bytes,
        archive_bytes,
        started_at,
        "Extracting the bounded snapshot into the BuildBridge guest workspace.",
        None,
    ));
    let extract = format!(
        "set -eu; /bin/mkdir -p '{guest_staging}'; /usr/bin/tar -xzf '{guest_archive}' -C '{guest_staging}'; /bin/test -f '{guest_staging}/package.json'; /bin/test -d '{guest_staging}/ios/App/App.xcworkspace'; /bin/rm -rf '{guest_workspace}.previous'; if /bin/test -d '{guest_workspace}'; then /bin/mv '{guest_workspace}' '{guest_workspace}.previous'; fi; /bin/mv '{guest_staging}' '{guest_workspace}'; if /bin/test -f '{guest_env}'; then /bin/mv '{guest_env}' '{guest_workspace}/.env.production.local'; /bin/chmod 600 '{guest_workspace}/.env.production.local'; fi; if /bin/test -f '{guest_env_shell}'; then /bin/mkdir -p '{guest_workspace}/.buildbridge'; /bin/mv '{guest_env_shell}' '{guest_workspace}/.buildbridge/env.sh'; /bin/chmod 600 '{guest_workspace}/.buildbridge/env.sh'; fi; /bin/rm -f '{guest_archive}'; /bin/rm -rf '{guest_workspace}.previous'"
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
    target: UnsignedBuildTarget,
    mut on_progress: F,
) -> Result<AppleSmokeBuildResult, ProviderError>
where
    F: FnMut(AppleProjectProgress),
{
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    let started_at = Instant::now();
    let needs_simulator = match target {
        UnsignedBuildTarget::Simulator => "1",
        UnsignedBuildTarget::DeviceSdk => "0",
    };
    let build_destination = unsigned_build_destination_args(target);
    let guest_home = format!("/Users/{username}");
    let workspace = format!("{guest_home}/BuildBridge/workspaces/active");
    let GuestToolchain {
        tools,
        node_root,
        pnpm,
        ruby_root,
        gem_home,
        pod,
        developer_dir,
        path,
    } = guest_toolchain(&guest_home);
    let node_name = format!("node-v{NODE_VERSION}-darwin-x64");
    let node_archive = format!("{tools}/{node_name}.tar.gz");
    let ruby_archive = format!("{tools}/portable-ruby-{PORTABLE_RUBY_VERSION}.tar.gz");

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
export PATH="{path}"
export GEM_HOME="{gem_home}"
export GEM_PATH="{gem_home}"
export DEVELOPER_DIR="{developer_dir}"
export LANG="en_US.UTF-8"
export CYPRESS_INSTALL_BINARY=0
if /bin/test -f "{workspace}/.buildbridge/env.sh"; then
    . "{workspace}/.buildbridge/env.sh"
fi

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
if /bin/test ! -x "{ruby_root}/bin/ruby"; then
    /bin/rm -rf "{ruby_root}" "{ruby_archive}"
    /usr/bin/curl --fail --location --show-error --silent "https://github.com/Homebrew/homebrew-portable-ruby/releases/download/{PORTABLE_RUBY_VERSION}/portable-ruby-{PORTABLE_RUBY_VERSION}.el_capitan.bottle.tar.gz" --output "{ruby_archive}"
    /usr/bin/shasum -a 256 "{ruby_archive}" | /usr/bin/grep -q "^{PORTABLE_RUBY_DARWIN_X64_SHA256}  "
    /usr/bin/tar -xzf "{ruby_archive}" -C "{tools}"
    /bin/rm -f "{ruby_archive}"
    /bin/test -x "{ruby_root}/bin/ruby"
fi
if /bin/test ! -x "{pod}"; then
    /bin/rm -rf "{gem_home}" "{tools}/gems"
    "{ruby_root}/bin/gem" install cocoapods --version "{COCOAPODS_VERSION}" --no-document
    /bin/test -x "{pod}"
fi
platform_installed=0
if /bin/test "{needs_simulator}" -eq 1 && ! /usr/bin/xcrun simctl list runtimes 2>/dev/null | /usr/bin/grep -q '^iOS '; then
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
    /usr/bin/xcodebuild -workspace "{workspace}/ios/App/App.xcworkspace" -scheme App -configuration Debug {build_destination} -derivedDataPath "{workspace}/.buildbridge/DerivedData" CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO COMPILER_INDEX_STORE_ENABLE=NO build
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
        .tracked_spawn()
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
        on_progress(apple_progress(
            phase,
            0,
            0,
            started_at,
            phase_detail(phase),
            Some(line),
        ));
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
        let context = build_failure_context(&diagnostic_lines, &output_tail);
        return Err(ProviderError::GuestBridge(if context.is_empty() {
            format!("the Apple test build failed during {}", phase_detail(phase))
        } else {
            format!(
                "the Apple test build failed during {}:\n{context}",
                phase_detail(phase)
            )
        }));
    }

    let xcode_version = parse_xcode_version(&run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        "/usr/bin/xcodebuild -version",
    )?);
    on_progress(apple_progress(
        AppleProjectPhase::Completed,
        0,
        0,
        started_at,
        unsigned_build_completed_detail(target),
        None,
    ));

    Ok(AppleSmokeBuildResult {
        target,
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
    env: Option<&GuestEnvFiles>,
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
    let identity = signing.distribution_identity.as_ref().ok_or_else(|| {
        ProviderError::GuestBridge(
            "no distribution identity is provisioned: the kit holds only a development identity, which signs a Debug build for a phone but not an App Store archive"
                .to_string(),
        )
    })?;
    if identity.identity_sha1.len() != 40
        || !identity
            .identity_sha1
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(ProviderError::GuestBridge(
            "the provisioned signing identity is invalid".to_string(),
        ));
    }
    let profile = select_app_store_profile(signing).ok_or_else(|| {
        ProviderError::GuestBridge(
            "no installed App Store profile matches the provisioned distribution identity and project (development and ad hoc profiles are not used for App Store archives)"
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
        &identity.identity_sha1,
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
        if let Some(env) = env {
            rebuild_web_assets_with_env(
                env,
                ssh_port,
                username,
                identity_path,
                known_hosts_path,
                &guest_home,
                started_at,
                &mut on_progress,
            )?;
        }
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
            &identity.identity_sha1,
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
        .tracked_spawn()
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
        on_progress(archive_progress(
            phase,
            0,
            0,
            started_at,
            archive_phase_detail(phase),
            Some(line),
        ));
    }

    let status = child.wait().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish the signed archive: {error}"))
    })?;
    if !status.success() {
        let context = build_failure_context(&diagnostic_lines, &output_tail);
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
        AppleArchivePhase::BuildingWebAssets => "Rebuilding the web assets with the chosen env set",
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
        .tracked_spawn()
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
        .tracked_output()
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
        .tracked_spawn()
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

/// Rebuilds the web assets inside the guest with a chosen env set, in place, and re-syncs them
/// into the iOS project. This is what makes an env a per-build choice: the source snapshot and
/// the installed dependencies stay, only the assets that read the environment are produced again.
/// Refuses to continue if the native lockfile moves, exactly as the test build would.
#[allow(clippy::too_many_arguments)]
fn rebuild_web_assets_with_env<F>(
    env: &GuestEnvFiles,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    guest_home: &str,
    started_at: Instant,
    on_progress: &mut F,
) -> Result<(), ProviderError>
where
    F: FnMut(AppleArchiveProgress),
{
    on_progress(archive_progress(
        AppleArchivePhase::BuildingWebAssets,
        0,
        0,
        started_at,
        "Applying the chosen env set and rebuilding the web assets.",
        None,
    ));
    let root = format!("{guest_home}/BuildBridge/workspaces/active");
    let GuestToolchain {
        gem_home,
        developer_dir,
        path,
        ..
    } = guest_toolchain(guest_home);

    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!(
            "set -eu; /bin/test -d {}; /bin/mkdir -p {}",
            shell_single_quote(&root),
            shell_single_quote(&format!("{root}/.buildbridge"))
        ),
    )?;
    stream_bytes_to_guest(
        env.dotenv.as_bytes(),
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!("{root}/.env.production.local"),
        "env file",
    )?;
    stream_bytes_to_guest(
        env.shell.as_bytes(),
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!("{root}/.buildbridge/env.sh"),
        "env script",
    )?;

    let script = format!(
        r#"set -u
exec 2>&1
set -e
export PATH="{path}"
export GEM_HOME="{gem_home}"
export GEM_PATH="{gem_home}"
export DEVELOPER_DIR="{developer_dir}"
export LANG="en_US.UTF-8"
export CYPRESS_INSTALL_BINARY=0
/bin/test -f "{root}/package.json"
/bin/test -x "{root}/node_modules/.bin/vp"
/bin/test -x "{root}/node_modules/.bin/cap"
. "{root}/.buildbridge/env.sh"
cd "{root}"
"{root}/node_modules/.bin/vp" build
lock_before=$(/usr/bin/shasum -a 256 "{root}/ios/App/Podfile.lock" | /usr/bin/cut -d ' ' -f 1)
"{root}/node_modules/.bin/cap" sync ios
lock_after=$(/usr/bin/shasum -a 256 "{root}/ios/App/Podfile.lock" | /usr/bin/cut -d ' ' -f 1)
if /bin/test "$lock_before" != "$lock_after"; then
    /usr/bin/printf '__BUILDBRIDGE_NATIVE_LOCK_UPDATED__:yes\n'
    exit 3
fi
"#
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start the web asset rebuild: {error}"))
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture the web asset rebuild output".to_string())
    })?;
    let mut tail: Vec<String> = Vec::new();
    let mut lock_moved = false;
    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| {
            ProviderError::GuestBridge(format!("could not read the web asset rebuild: {error}"))
        })?;
        if line == "__BUILDBRIDGE_NATIVE_LOCK_UPDATED__:yes" {
            lock_moved = true;
            continue;
        }
        if tail.len() >= 40 {
            tail.remove(0);
        }
        tail.push(line.clone());
        on_progress(archive_progress(
            AppleArchivePhase::BuildingWebAssets,
            0,
            0,
            started_at,
            "Applying the chosen env set and rebuilding the web assets.",
            Some(line),
        ));
    }
    let status = child.wait().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish the web asset rebuild: {error}"))
    })?;
    if lock_moved {
        return Err(ProviderError::GuestBridge(
            "rebuilding with this env set changed Podfile.lock; synchronize and run the test build again before a signed archive"
                .to_string(),
        ));
    }
    if !status.success() {
        return Err(ProviderError::GuestBridge(format!(
            "the web asset rebuild failed:\n{}",
            tail.join("\n")
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

/// One `.p12` and its passphrase, checked before anything is streamed to the guest.
fn validate_identity_material(
    path: &Path,
    password: &str,
    label: &str,
) -> Result<(PathBuf, u64), ProviderError> {
    if password.is_empty() || password.len() > 512 {
        return Err(ProviderError::GuestBridge(format!(
            "store the {label} passphrase in the operating-system vault first"
        )));
    }
    let file = validate_signing_file(path, &["p12", "pfx"], SIGNING_CERTIFICATE_MAX_BYTES, label)?;
    let bytes = fs::metadata(&file)
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not inspect the {label}: {error}"))
        })?
        .len();
    Ok((file, bytes))
}

/// The profiles and the keychain password. A kit with a distribution identity must carry at
/// least one profile, because that is what the archive signs with; a development-only kit may
/// carry none yet, since the device step adds its profile later.
fn validate_signing_material(
    profile_paths: &[PathBuf],
    keychain_password: &str,
    profiles_required: bool,
) -> Result<(Vec<PathBuf>, u64), ProviderError> {
    if keychain_password.is_empty() || keychain_password.len() > 512 {
        return Err(ProviderError::GuestBridge(
            "store a dedicated guest keychain password in the operating-system vault first"
                .to_string(),
        ));
    }
    if (profiles_required && profile_paths.is_empty())
        || profile_paths.len() > SIGNING_PROFILE_MAX_COUNT
    {
        return Err(ProviderError::GuestBridge(format!(
            "select between {} and {SIGNING_PROFILE_MAX_COUNT} provisioning profiles",
            u8::from(profiles_required)
        )));
    }

    let mut total_bytes = 0_u64;
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

    Ok((profiles, total_bytes))
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
        .tracked_spawn()
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
        .tracked_spawn()
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
    mode: HelperImportMode,
) -> Result<(), ProviderError> {
    let remote_command = format!(
        "{}{} {} {} {} {}",
        shell_single_quote(helper_path),
        match mode {
            HelperImportMode::Create => "",
            HelperImportMode::Add => " --add",
        },
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
        .tracked_spawn()
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

/// The fixed shell for reading one profile's metadata in the guest: decode, read the UUID,
/// team, application identifier and expiry, hash each developer certificate, then the
/// entitlement flags and provisioned devices. `plutil` prints a missing key's error on stdout
/// and exits 1, so each boolean is filtered to a bare `true`/`false` and defaults to `false`.
fn profile_inspection_command(guest_profile: &str) -> String {
    let profile = shell_single_quote(guest_profile);
    let plist = shell_single_quote(&format!("{guest_profile}.plist"));
    let certificate = shell_single_quote(&format!("{guest_profile}.certificate.der"));
    format!(
        "; /usr/bin/security cms -D -i {profile} > {plist}; uuid=$(/usr/bin/plutil -extract UUID raw -o - {plist}); team=$(/usr/bin/plutil -extract TeamIdentifier.0 raw -o - {plist}); app_id=$(/usr/bin/plutil -extract Entitlements.application-identifier raw -o - {plist}); expires_at=$(/usr/bin/plutil -extract ExpirationDate raw -o - {plist}); if ! expiry_epoch=$(/bin/date -j -f '%Y-%m-%d %H:%M:%S %z' \"$expires_at\" +%s 2>/dev/null || /bin/date -j -f '%Y-%m-%dT%H:%M:%SZ' \"$expires_at\" +%s 2>/dev/null); then /usr/bin/printf 'invalid_profile_expiry:%s' \"$uuid\" >&2; exit 1; fi; if /bin/test \"$expiry_epoch\" -le \"$(/bin/date +%s)\"; then /usr/bin/printf 'expired_profile:%s' \"$uuid\" >&2; exit 1; fi; /usr/bin/printf '__BUILDBRIDGE_PROFILE__\\t%s\\t%s\\t%s\\t%s\\n' \"$uuid\" \"$team\" \"$app_id\" \"$expires_at\"; certificate_count=$(/usr/bin/plutil -extract DeveloperCertificates xml1 -o - {plist} | /usr/bin/grep -c '<data>'); certificate_index=0; while /bin/test \"$certificate_index\" -lt \"$certificate_count\"; do /usr/bin/plutil -extract \"DeveloperCertificates.$certificate_index\" raw -o - {plist} | /usr/bin/base64 -D > {certificate}; certificate_sha256=$(/usr/bin/openssl dgst -sha256 {certificate} | /usr/bin/awk '{{print $NF}}'); /usr/bin/printf '__BUILDBRIDGE_PROFILE_CERT__\\t%s\\t%s\\n' \"$uuid\" \"$certificate_sha256\"; certificate_index=$((certificate_index + 1)); done; get_task_allow=$(/usr/bin/plutil -extract Entitlements.get-task-allow raw -o - {plist} 2>/dev/null | /usr/bin/grep -x -E 'true|false' || /bin/echo false); provisions_all=$(/usr/bin/plutil -extract ProvisionsAllDevices raw -o - {plist} 2>/dev/null | /usr/bin/grep -x -E 'true|false' || /bin/echo false); /usr/bin/printf '__BUILDBRIDGE_PROFILE_FLAGS__\\t%s\\t%s\\t%s\\n' \"$uuid\" \"$get_task_allow\" \"$provisions_all\"; device_count=$(/usr/bin/plutil -extract ProvisionedDevices xml1 -o - {plist} 2>/dev/null | /usr/bin/grep -c '<string>' || /usr/bin/true); device_index=0; while /bin/test \"${{device_count:-0}}\" -gt \"$device_index\"; do device_udid=$(/usr/bin/plutil -extract \"ProvisionedDevices.$device_index\" raw -o - {plist}); /usr/bin/printf '__BUILDBRIDGE_PROFILE_DEVICE__\\t%s\\t%s\\n' \"$uuid\" \"$device_udid\"; device_index=$((device_index + 1)); done; /bin/rm -f {certificate} {plist}"
    )
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
        command.push_str(&profile_inspection_command(guest_profile));
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
    // (get-task-allow, ProvisionsAllDevices) per profile, in the order the guest reported.
    let mut flags: Vec<(String, bool, bool)> = Vec::new();
    for line in output.lines() {
        if let Some(values) = line.strip_prefix("__BUILDBRIDGE_PROFILE_FLAGS__\t") {
            let fields = values.split('\t').collect::<Vec<_>>();
            let parse_flag = |value: &str| match value {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
            let (Some(&uuid), Some(get_task_allow), Some(provisions_all)) = (
                fields.first(),
                fields.get(1).and_then(|value| parse_flag(value)),
                fields.get(2).and_then(|value| parse_flag(value)),
            ) else {
                return Err(ProviderError::GuestBridge(
                    "macOS returned invalid provisioning profile entitlement metadata".to_string(),
                ));
            };
            if fields.len() != 3
                || !profiles.iter().any(|profile| profile.uuid == uuid)
                || flags.iter().any(|(seen, _, _)| seen == uuid)
            {
                return Err(ProviderError::GuestBridge(
                    "macOS returned out-of-order provisioning profile metadata".to_string(),
                ));
            }
            flags.push((uuid.to_string(), get_task_allow, provisions_all));
            continue;
        }
        if let Some(values) = line.strip_prefix("__BUILDBRIDGE_PROFILE_DEVICE__\t") {
            let Some((uuid, udid)) = values.split_once('\t') else {
                return Err(ProviderError::GuestBridge(
                    "macOS returned invalid provisioning profile device metadata".to_string(),
                ));
            };
            let Some(profile) = profiles.iter_mut().find(|profile| profile.uuid == uuid) else {
                return Err(ProviderError::GuestBridge(
                    "macOS returned out-of-order provisioning profile metadata".to_string(),
                ));
            };
            let udid = udid.to_ascii_uppercase();
            if !valid_device_udid(&udid)
                || profile.provisioned_device_udids.len() >= 400
                || profile.provisioned_device_udids.contains(&udid)
            {
                return Err(ProviderError::GuestBridge(
                    "macOS returned invalid provisioning profile device metadata".to_string(),
                ));
            }
            profile.provisioned_device_udids.push(udid);
            continue;
        }
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
            kind: None,
            provisioned_device_udids: Vec::new(),
            get_task_allow: false,
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
    for profile in &mut profiles {
        let Some((_, get_task_allow, provisions_all)) =
            flags.iter().find(|(uuid, _, _)| uuid == &profile.uuid)
        else {
            return Err(ProviderError::GuestBridge(
                "macOS returned incomplete provisioning profile metadata".to_string(),
            ));
        };
        profile.get_task_allow = *get_task_allow;
        profile.kind = Some(classify_profile(
            *get_task_allow,
            *provisions_all,
            profile.provisioned_device_udids.len(),
        ));
    }

    Ok(profiles)
}

/// What a profile is for. In-house profiles provision every device; App Store profiles name
/// none and cannot be debugged; development profiles can be debugged; ad hoc ones name devices
/// but cannot.
pub fn classify_profile(
    get_task_allow: bool,
    provisions_all_devices: bool,
    device_count: usize,
) -> ProfileKind {
    if provisions_all_devices {
        ProfileKind::Enterprise
    } else if get_task_allow {
        ProfileKind::Development
    } else if device_count == 0 {
        ProfileKind::AppStore
    } else {
        ProfileKind::AdHoc
    }
}

fn profile_matches_project(
    profile: &ProvisioningProfileSummary,
    signing: &SigningProvisioningResult,
) -> bool {
    valid_profile_uuid(&profile.uuid)
        && profile.team_identifier == signing.development_team
        && profile_allows_bundle(&profile.application_identifier, &signing.bundle_identifier)
}

/// The profile an App Store archive signs with: the project's, carrying the distribution
/// certificate, and not a development or ad hoc profile that happens to match too. A record
/// written before kinds were read counts as App Store, which is all a kit could hold then.
pub fn select_app_store_profile(
    signing: &SigningProvisioningResult,
) -> Option<&ProvisioningProfileSummary> {
    let identity = signing.distribution_identity.as_ref()?;
    signing.profiles.iter().find(|profile| {
        matches!(profile.kind, Some(ProfileKind::AppStore) | None)
            && profile_matches_project(profile, signing)
            && profile
                .developer_certificate_sha256
                .iter()
                .any(|fingerprint| fingerprint == &identity.certificate_sha256)
    })
}

/// The profile a device build signs with: a development profile carrying the development
/// certificate and listing the phone.
pub fn select_development_profile<'a>(
    signing: &'a SigningProvisioningResult,
    udid: &str,
) -> Option<&'a ProvisioningProfileSummary> {
    let identity = signing.development_identity.as_ref()?;
    signing.profiles.iter().find(|profile| {
        profile.kind == Some(ProfileKind::Development)
            && profile_matches_project(profile, signing)
            && profile
                .developer_certificate_sha256
                .iter()
                .any(|fingerprint| fingerprint == &identity.certificate_sha256)
            && profile
                .provisioned_device_udids
                .iter()
                .any(|listed| listed.eq_ignore_ascii_case(udid))
    })
}

/// A device UDID as Apple prints it: 40 hex characters on older phones, or 8 hex characters,
/// a hyphen and 16 more on phones since the iPhone XS. Case does not matter.
pub fn valid_device_udid(value: &str) -> bool {
    let bytes = value.as_bytes();

    match bytes.len() {
        40 => bytes.iter().all(u8::is_ascii_hexdigit),
        25 => {
            bytes[8] == b'-'
                && bytes[..8].iter().all(u8::is_ascii_hexdigit)
                && bytes[9..].iter().all(u8::is_ascii_hexdigit)
        }
        _ => false,
    }
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
        .tracked_spawn()
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
        || normalized.starts_with("make: ***")
        || normalized.contains("extconf failed")
        || normalized.contains("mkmf.rb can't find header files")
        || normalized.contains("com.apple.actool.errors")
        || normalized.contains("failed with a nonzero exit code")
        || normalized.starts_with("** build failed **")
        || normalized.starts_with("the following build commands failed:")
        || normalized.trim_start().starts_with("compileassetcatalog")
}

/// What a failed guest build reports: every diagnostic line kept during the run, then the last
/// lines the guest printed so the failing command is named even when its output matches no
/// known diagnostic shape. Lines already in the tail are not repeated.
fn build_failure_context(diagnostic_lines: &[String], output_tail: &[String]) -> String {
    let tail = &output_tail[output_tail
        .len()
        .saturating_sub(APPLE_BUILD_FAILURE_TAIL_LINES)..];

    diagnostic_lines
        .iter()
        .filter(|line| !tail.contains(line))
        .chain(tail.iter())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Where the pinned toolchain lives under the guest home directory. Every guest recipe derives
/// its paths from here so the test build and the signed archive agree.
struct GuestToolchain {
    tools: String,
    node_root: String,
    pnpm: String,
    ruby_root: String,
    gem_home: String,
    pod: String,
    developer_dir: String,
    /// The `PATH` the recipes export: pinned tools first, then only Apple's system directories.
    path: String,
}

fn guest_toolchain(guest_home: &str) -> GuestToolchain {
    let tools = format!("{guest_home}/.buildbridge/tools");
    let node_root = format!("{tools}/node-v{NODE_VERSION}-darwin-x64");
    let ruby_root = format!("{tools}/portable-ruby/{PORTABLE_RUBY_VERSION}");
    let gem_home = format!("{tools}/gems-ruby-{PORTABLE_RUBY_VERSION}");
    let path = format!(
        "{node_root}/bin:{tools}/pnpm/node_modules/.bin:{ruby_root}/bin:{gem_home}/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    );

    GuestToolchain {
        pnpm: format!("{tools}/pnpm/node_modules/.bin/pnpm"),
        pod: format!("{gem_home}/bin/pod"),
        developer_dir: format!("{guest_home}/Applications/Xcode.app/Contents/Developer"),
        tools,
        node_root,
        ruby_root,
        gem_home,
        path,
    }
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
        AppleProjectPhase::PreparingTools => "Preparing Node, pnpm, Ruby, and CocoaPods",
        AppleProjectPhase::PreparingPlatform => {
            "Downloading and installing Apple's iOS Simulator platform"
        }
        AppleProjectPhase::InstallingDependencies => "Installing locked project dependencies",
        AppleProjectPhase::BuildingWebAssets => "Building web assets",
        AppleProjectPhase::SyncingIos => "Synchronizing the Capacitor iOS project",
        AppleProjectPhase::ResolvingPods => "Resolving locked CocoaPods",
        AppleProjectPhase::Building => "Compiling the unsigned iOS app",
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
        .tracked_spawn()
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
        .tracked_spawn()
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
        .tracked_output()
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

/// Environment variable the one-time key install hands the macOS password to `ssh` through.
/// Only the askpass helper reads it.
const GUEST_PASSWORD_ENV: &str = "BUILDBRIDGE_GUEST_PASSWORD";

/// The fixed askpass helper OpenSSH runs during the one-time key install. It only echoes the
/// password variable set on that one `ssh` process, so the password never becomes an
/// argument, a file, or a log line.
const GUEST_ASKPASS_SCRIPT: &str = "#!/bin/sh\nprintf '%s\\n' \"$BUILDBRIDGE_GUEST_PASSWORD\"\n";

/// A private, single-use directory holding the askpass helper for one key install. Dropping
/// it removes the helper again, whether or not `ssh` succeeded.
struct GuestAskpassHelper {
    dir: PathBuf,
}

impl GuestAskpassHelper {
    fn create() -> Result<Self, ProviderError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "buildbridge-askpass-{}-{nanos}",
            std::process::id()
        ));
        let failed = |error: std::io::Error| {
            ProviderError::GuestBridge(format!("could not prepare the password helper: {error}"))
        };

        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

            fs::DirBuilder::new()
                .mode(0o700)
                .create(&dir)
                .map_err(failed)?;
            let helper = Self { dir };
            let script = helper.script();
            fs::write(&script, GUEST_ASKPASS_SCRIPT).map_err(failed)?;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).map_err(failed)?;

            Ok(helper)
        }
        #[cfg(not(unix))]
        {
            let _ = (dir, failed);
            Err(ProviderError::GuestBridge(
                "the one-time key install needs a Unix host".to_string(),
            ))
        }
    }

    fn script(&self) -> PathBuf {
        self.dir.join("askpass")
    }
}

impl Drop for GuestAskpassHelper {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Returns whether a public key is the one BuildBridge generated: a single `ssh-ed25519` line
/// with base64 material and at most a plain comment, so it can be quoted into a guest command.
pub fn valid_guest_public_key(public_key: &str) -> bool {
    let mut fields = public_key.split(' ');
    let algorithm = fields.next();
    let Some(material) = fields.next() else {
        return false;
    };
    let comment_is_plain = match fields.next() {
        None => true,
        Some(comment) => {
            !comment.is_empty()
                && comment.chars().all(|character| {
                    character.is_ascii_alphanumeric()
                        || matches!(character, '-' | '_' | '.' | '@' | ':')
                })
        }
    };

    public_key.len() <= 2_048
        && algorithm == Some("ssh-ed25519")
        && !material.is_empty()
        && material.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '=')
        })
        && comment_is_plain
        && fields.next().is_none()
}

/// The askpass helper answers with one line, so the password has to be one.
fn valid_guest_password(password: &str) -> bool {
    !password.is_empty() && password.len() <= 512 && !password.chars().any(char::is_control)
}

/// The command the password session runs in the guest: append the key once, with the
/// permissions sshd insists on, and confirm. Idempotent, so a retry never duplicates the line.
fn guest_key_install_command(public_key: &str) -> String {
    let quoted = shell_single_quote(public_key);
    format!(
        "umask 077; key={quoted}; /bin/mkdir -p \"$HOME/.ssh\" && /bin/chmod 700 \"$HOME/.ssh\" && {{ /usr/bin/grep -qxF \"$key\" \"$HOME/.ssh/authorized_keys\" 2>/dev/null || /usr/bin/printf '%s\\n' \"$key\" >> \"$HOME/.ssh/authorized_keys\"; }} && /bin/chmod 600 \"$HOME/.ssh/authorized_keys\" && /usr/bin/printf authorized"
    )
}

/// The one `ssh` invocation that authenticates with a password instead of the key. It still
/// refuses anything but the pinned host identity, tries the password once, and gets it from
/// the askpass helper rather than a terminal or an agent.
fn guest_password_ssh_command(
    ssh_port: u16,
    username: &str,
    known_hosts_path: &Path,
    askpass_path: &Path,
) -> Command {
    let destination = format!("{username}@127.0.0.1");
    let mut command = Command::new("ssh");
    command
        .arg("-F")
        .arg("/dev/null")
        .args([
            "-o",
            "PubkeyAuthentication=no",
            "-o",
            "PasswordAuthentication=yes",
            "-o",
            "KbdInteractiveAuthentication=yes",
            "-o",
            "NumberOfPasswordPrompts=1",
            "-o",
            "ConnectTimeout=3",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "LogLevel=ERROR",
            "-p",
        ])
        .arg(ssh_port.to_string())
        .arg("-o")
        .arg(format!("UserKnownHostsFile={}", known_hosts_path.display()))
        .arg(destination)
        .env("SSH_ASKPASS", askpass_path)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env_remove("SSH_AUTH_SOCK");

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

/// Reports the host prerequisites and the state of one managed container.
pub fn status(container_name: &str) -> Result<RuntimeStatus, ProviderError> {
    let prerequisites = probe_host();

    if !prerequisites.docker_daemon {
        return Ok(RuntimeStatus {
            prerequisites,
            state: ContainerState::Unavailable,
            container_id: None,
            started_at: None,
        });
    }

    let (state, container_id, started_at) = inspect_container(container_name)?;

    Ok(RuntimeStatus {
        prerequisites,
        state,
        container_id,
        started_at,
    })
}

/// Creates the managed container when it is missing, then starts it.
///
/// Progress is reported per phase so the desktop can explain a multi-gigabyte image pull
/// instead of showing an unexplained wait.
pub fn launch<F>(
    container_name: &str,
    config: &MacBuilderConfig,
    options: &LaunchOptions<'_>,
    mut on_progress: F,
) -> Result<RuntimeStatus, ProviderError>
where
    F: FnMut(LaunchProgress),
{
    config.validate()?;
    let disk = MachineDisk::new(options.disk_dir)?;
    validate_bind_path(options.qmp_dir, "control socket directory")?;
    let started = Instant::now();
    let mut report = |phase: LaunchPhase, detail: &str| {
        on_progress(LaunchProgress {
            phase,
            elapsed_seconds: started.elapsed().as_secs(),
            detail: detail.to_string(),
        });
    };
    report(LaunchPhase::Preparing, "Checking the host and Docker");
    let prerequisites = probe_host();

    if !prerequisites.ready {
        return Err(ProviderError::Prerequisites(prerequisites.issues.join(" ")));
    }

    let (state, _, _) = inspect_container(container_name)?;

    if state == ContainerState::Missing {
        report(
            LaunchPhase::PullingImage,
            "Pulling the Docker-OSX image; the first pull downloads several gigabytes",
        );
        ensure_image()?;
        report(
            LaunchPhase::GeneratingIdentity,
            "Generating a stable machine identity",
        );
        ensure_identity(options.identity_path)?;
        report(
            LaunchPhase::PreparingDisk,
            "Preparing the macOS disk on this host",
        );
        ensure_machine_disk(&disk)?;
        disk::ensure_control_dir(options.qmp_dir)?;
        report(
            LaunchPhase::CreatingContainer,
            "Creating the managed container",
        );
        create_container(
            container_name,
            config,
            prerequisites.display.as_deref().unwrap_or(":0"),
            options,
            &disk,
        )?;
    } else {
        ensure_manual_restart_policy(container_name)?;
    }

    let (state, _, _) = inspect_container(container_name)?;
    if state != ContainerState::Running {
        // A control directory the daemon would have to create is created as root, and QEMU
        // then cannot bind its socket there; make it before every start, not only the first.
        disk::ensure_control_dir(options.qmp_dir)?;
        report(LaunchPhase::Starting, "Starting the macOS machine");
        run_docker("start", &["start".to_string(), container_name.to_string()])?;
    }

    report(LaunchPhase::Completed, "The macOS machine is running");
    status(container_name)
}

/// Stops the container gracefully while preserving it and its macOS disk.
pub fn stop(container_name: &str) -> Result<RuntimeStatus, ProviderError> {
    let current = status(container_name)?;

    if current.state == ContainerState::Paused {
        run_docker(
            "unpause",
            &["unpause".to_string(), container_name.to_string()],
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
                container_name.to_string(),
            ],
        )?;
    }

    status(container_name)
}

/// Removes a stopped container together with the macOS disk stored inside it.
///
/// This is deliberately separate from [`stop`]: callers must confirm the data loss and
/// stop the machine first. A live container is never removed implicitly.
pub fn remove(container_name: &str) -> Result<RuntimeStatus, ProviderError> {
    let current = status(container_name)?;

    match current.state {
        ContainerState::Missing | ContainerState::Unavailable => return Ok(current),
        ContainerState::Running | ContainerState::Paused | ContainerState::Restarting => {
            return Err(ProviderError::DockerCommand {
                operation: "container removal",
                message: "stop the machine before discarding its container".to_string(),
            });
        }
        _ => {}
    }

    run_docker(
        "container removal",
        &["rm".to_string(), container_name.to_string()],
    )?;

    status(container_name)
}

pub fn recent_logs(container_name: &str) -> Result<Vec<String>, ProviderError> {
    let current = status(container_name)?;

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
            container_name.to_string(),
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

fn inspect_container(
    container_name: &str,
) -> Result<(ContainerState, Option<String>, Option<String>), ProviderError> {
    let output = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{.State.Status}}|{{.Id}}|{{.State.StartedAt}}",
            container_name,
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
    container_name: &str,
    config: &MacBuilderConfig,
    display: &str,
    options: &LaunchOptions<'_>,
    disk: &MachineDisk,
) -> Result<(), ProviderError> {
    validate_identity_path(options.identity_path)?;
    let args = create_args(
        container_name,
        config,
        display,
        options.identity_path,
        disk,
        options.qmp_dir,
        options.usb.as_ref(),
    );
    run_docker("container creation", &args)?;

    Ok(())
}

fn ensure_manual_restart_policy(container_name: &str) -> Result<(), ProviderError> {
    run_docker(
        "restart policy update",
        &[
            "update".to_string(),
            "--restart=no".to_string(),
            container_name.to_string(),
        ],
    )?;

    Ok(())
}

/// The container's fixed argv. The disk, NVRAM and control directory are bound from the host;
/// USB access is a device cgroup rule plus the device tree plus the plugdev group — never
/// `--privileged`. `EXTRA` is word-split by Docker-OSX's `Launch.sh`, so the display and the
/// QMP socket are two flags in one value.
fn create_args(
    container_name: &str,
    config: &MacBuilderConfig,
    display: &str,
    identity_path: &Path,
    disk: &MachineDisk,
    qmp_dir: &Path,
    usb: Option<&ContainerUsbOptions>,
) -> Vec<String> {
    let mut args = vec![
        "create".to_string(),
        format!("--name={container_name}"),
        "--label=dev.buildbridge.managed=true".to_string(),
        "--restart=no".to_string(),
        "--interactive".to_string(),
        "--tty".to_string(),
        "--device=/dev/kvm".to_string(),
        format!("--publish={}:10022", config.ssh_port),
        "--volume=/tmp/.X11-unix:/tmp/.X11-unix:rw".to_string(),
        format!("--volume={}:/env:ro", identity_path.display()),
    ];
    args.extend(disk::disk_bind_args(disk));
    args.push(format!(
        "--volume={}:{QMP_CONTAINER_DIR}:rw",
        qmp_dir.display()
    ));
    if let Some(usb) = usb {
        args.push(format!(
            "--device-cgroup-rule=c {}:* rwm",
            usb::USB_BUS_MAJOR
        ));
        args.push("--volume=/dev/bus/usb:/dev/bus/usb".to_string());
        args.push(format!("--group-add={}", usb.plugdev_gid));
    }
    args.extend([
        format!("--env=DISPLAY={display}"),
        format!("--env=RAM={}", config.memory_gib),
        format!("--env=SMP={}", config.cpu_cores),
        format!("--env=CORES={}", config.cpu_cores),
        "--env=WIDTH=1280".to_string(),
        "--env=HEIGHT=720".to_string(),
        format!("--env=EXTRA={}", qemu_extra_args(usb)),
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
    ]);

    args
}

/// QEMU's extra arguments: the console, the control socket, and — whenever the host can pass
/// USB through at all — a dedicated USB 2.0 controller for a phone. The controller is there from
/// the start so that attaching a phone later is a hot-plug over QMP and never a rebuild: on the
/// machine's own emulated xHCI macOS never assigns an iPhone an address, and adding a controller
/// to a running guest is PCI hot-plug, which is not worth asking of macOS. It costs nothing when
/// no phone is attached. The image's launch script word-splits this, so every value is fixed.
fn qemu_extra_args(usb: Option<&ContainerUsbOptions>) -> String {
    let mut extra = format!(
        "-display gtk,zoom-to-fit=on -qmp unix:{QMP_CONTAINER_DIR}/{QMP_SOCKET_NAME},server,nowait"
    );
    if usb.is_some() {
        extra.push_str(&format!(" -device usb-ehci,id={USB_PHONE_CONTROLLER}"));
    }

    extra
}

fn run_docker(operation: &'static str, args: &[String]) -> Result<Output, ProviderError> {
    let output = Command::new("docker")
        .args(args)
        .tracked_output()
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

/// Reduces `xcodebuild -version` output ("Xcode 26.6\nBuild version 17F113") to the bare
/// version the desktop contract carries ("26.6"); every caller renders its own "Xcode" label.
fn parse_xcode_version(raw: &str) -> String {
    let line = raw.lines().next().unwrap_or_default().trim();
    let version = line.strip_prefix("Xcode").unwrap_or(line).trim();

    if version.is_empty() {
        "version unknown".to_string()
    } else {
        version.to_string()
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
