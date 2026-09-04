use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use buildbridge_contract::{
    AppleArchivePayload, BuildLogLine, ClaimedBuild, CompleteBuildRequest, CompletionStatus,
    HeartbeatRequest, LogStream, MachineReport, PROTOCOL_VERSION, PairRunnerRequest,
    RealtimeAuthorizationRequest, RealtimeConfiguration, valid_git_ref,
};
use buildbridge_docker_osx::{
    AppleArchiveArtifact, AppleArchiveProgress, AppleArchiveResult, AppleDeviceRunProgress,
    AppleDeviceRunResult, AppleProjectProgress, AppleSmokeBuildResult, AppleWorkspaceSyncResult,
    ContainerState, GuestDiagnostics, GuestEnvFiles, GuestOptimization, GuestSshStatus,
    GuestTrustState, HostPrerequisites, MacBuilderConfig, OperationScope, PodfileLockChanges,
    RuntimeStatus, SigningProvisioningProgress, SigningProvisioningResult, UnsignedBuildTarget,
    XcodeImportProgress,
};
use buildbridge_runner::{ApiClient, execute};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Listener, Manager, State};

use crate::machines::{MachinePaths, StoredMachine};

mod apple_api;
mod apple_profiles;
mod builds;
mod certificates;
mod devices;
mod env_sets;
mod guest_access;
mod machine_lifecycle;
mod machines;
mod ops;
mod optimizations;
mod records;
mod runner;
mod signing_kits;
mod tray;
mod usb;
mod views;
use apple_profiles::*;
use builds::*;
use certificates::*;
use devices::*;
use env_sets::*;
use guest_access::*;
use machine_lifecycle::*;
use ops::*;
use optimizations::*;
use records::*;
use runner::*;
use signing_kits::*;
use usb::*;
use views::*;

/// Event names shared with the desktop frontend. Every payload carries `machineId`.
const MACHINE_CHANGED_EVENT: &str = "machine-changed";
const LAUNCH_PROGRESS_EVENT: &str = "machine-launch-progress";
const XCODE_PROGRESS_EVENT: &str = "machine-xcode-progress";
const SIGNING_PROGRESS_EVENT: &str = "machine-signing-progress";
const PROJECT_PROGRESS_EVENT: &str = "machine-project-progress";
const ARCHIVE_PROGRESS_EVENT: &str = "machine-archive-progress";
const USB_MIGRATION_PROGRESS_EVENT: &str = "machine-usb-migration-progress";
const DEVICE_SIGNING_PROGRESS_EVENT: &str = "machine-device-signing-progress";
const DEVICE_RUN_PROGRESS_EVENT: &str = "machine-device-progress";
const CONTAINER_REBUILD_PROGRESS_EVENT: &str = "machine-container-rebuild-progress";
const USB_ATTACH_PROGRESS_EVENT: &str = "machine-usb-attach-progress";

const CREDENTIAL_SERVICE: &str = "dev.buildbridge.desktop";
const MAC_BUILDER_CREDENTIAL_SERVICE: &str = "dev.buildbridge.desktop.macos-builder";
const MAC_BUILDER_CREDENTIAL_ACCOUNT: &str = "default";
/// Identifier given to the kit migrated from the pre-registry vault record.
const DEFAULT_SIGNING_KIT_ID: &str = "default";
/// Upper bound on stored kits; each one holds credentials for a developer team.
const MAX_SIGNING_KITS: usize = 12;
const MAX_PROVISIONING_PROFILES: usize = 20;
const ENV_SET_CREDENTIAL_SERVICE: &str = "dev.buildbridge.desktop.env-sets";
const MAX_ENV_SETS: usize = 12;
const MAX_ENV_VARIABLES: usize = 100;
const MAX_ENV_VALUE_LENGTH: usize = 4096;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MachineProgressEvent<T: Serialize> {
    machine_id: String,
    #[serde(flatten)]
    progress: T,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MachineChangedEvent {
    pub(crate) machine_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmInput {
    confirmed: bool,
}

/// What a machine can do about signing, including the state left behind when the host's
/// credential vault is cleared while the guest keeps its provisioned keychain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SigningHealth {
    /// No kit resolved, and none has been used on this machine before.
    Unconfigured,
    /// A kit is resolved and holds everything provisioning needs.
    Ready,
    /// A kit is resolved but is missing a certificate, profile or password.
    Incomplete,
    /// This machine has provisioned signing, but no kit remains in the vault.
    KitMissing,
    /// The credential vault itself could not be read.
    VaultUnavailable,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MachineSummary {
    id: String,
    config: MacBuilderConfig,
    created_at_epoch_seconds: u64,
    state: ContainerState,
    container_id: Option<String>,
    busy_operation: Option<String>,
    guest_configured: bool,
    trust_pinned: bool,
    workspace_name: Option<String>,
    signing_kit_name: Option<String>,
    signing_provisioned: bool,
    signing_identity: Option<String>,
    archive_retained: bool,
    env_set_name: Option<String>,
    /// The container keeps its disk on the host and can be handed USB devices.
    usb_ready: bool,
    device_run_retained: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MachineListView {
    host: HostPrerequisites,
    machines: Vec<MachineSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredConfig {
    server_url: String,
    runner_id: String,
    runner_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopStatus {
    paired: bool,
    /// A runner is configured on this host but its token is gone from the vault: pair again.
    credentials_missing: bool,
    server_url: Option<String>,
    runner_id: Option<String>,
    runner_name: Option<String>,
    platform: String,
    architecture: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairInput {
    server_url: String,
    code: String,
    runner_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthorizeRealtimeInput {
    socket_id: String,
    channel_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunOnceResult {
    state: RunState,
    build_id: Option<String>,
    message: String,
}

/// One named set of Apple signing material, held in the operating-system vault.
///
/// A host can hold several: one per developer team or per app. A machine is attached to one
/// kit, and provisioning imports that kit's identity into that machine's guest keychain.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSigningKit {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    app_store_connect_key_id: Option<String>,
    app_store_connect_issuer_id: Option<String>,
    app_store_connect_private_key: Option<String>,
    signing_certificate_path: Option<String>,
    signing_certificate_password: Option<String>,
    provisioning_profile_paths: Vec<String>,
    guest_keychain_password: Option<String>,
    #[serde(default)]
    created_at_epoch_seconds: u64,
    /// The optional development identity, for Debug builds on registered phones.
    #[serde(default)]
    development_certificate_path: Option<String>,
    #[serde(default)]
    development_certificate_password: Option<String>,
    /// Apple's serial for the development `.p12` BuildBridge created; derived with OpenSSL for
    /// a hand-supplied file and cached here.
    #[serde(default)]
    development_certificate_serial_number: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSigningKits {
    #[serde(default)]
    kits: Vec<StoredSigningKit>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SigningKitInput {
    /// Absent creates a kit; present updates that kit in place.
    kit_id: Option<String>,
    name: String,
    app_store_connect_key_id: String,
    app_store_connect_issuer_id: String,
    app_store_connect_private_key_path: String,
    signing_certificate_path: String,
    signing_certificate_password: String,
    provisioning_profile_paths: Vec<String>,
    guest_keychain_password: String,
    #[serde(default)]
    development_certificate_path: String,
    #[serde(default)]
    development_certificate_password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachSigningKitInput {
    /// Null detaches the machine from every kit.
    kit_id: Option<String>,
}

/// Everything about a kit that is safe to show: names and counts, never a secret value.
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SigningKitSummary {
    id: String,
    name: String,
    app_store_connect_configured: bool,
    app_store_connect_key_id: Option<String>,
    signing_certificate_configured: bool,
    signing_certificate_name: Option<String>,
    signing_certificate_password_stored: bool,
    provisioning_profile_names: Vec<String>,
    guest_keychain_configured: bool,
    created_at_epoch_seconds: u64,
    /// Machines currently attached to this kit, by display name.
    attached_machines: Vec<String>,
    development_certificate_configured: bool,
    development_certificate_name: Option<String>,
    development_certificate_password_stored: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAppleProfileInput {
    certificate_id: String,
    confirmed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAppleCertificateInput {
    confirmed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateAppleCertificateResult {
    certificate: apple_api::AppleCertificateSummary,
    saved_path: String,
    kit: SigningKitSummary,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadAppleProfileResult {
    profile: apple_api::AppleProvisioningProfileSummary,
    saved_path: String,
    kit: SigningKitSummary,
}

/// One named set of environment variables for a build, held in the host's vault like a signing
/// kit and attached per machine. A plain variable's value is shown back in the interface; a
/// secret's is left out of every summary and comes back only to the editor, on request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredEnvVariable {
    key: String,
    value: String,
    /// Masked in the interface and left out of every summary; `reveal_env_secrets` hands a set's
    /// secrets to the editor on request. Sets stored before the distinction existed were promised
    /// their values would not be shown, so a missing flag reads as a secret.
    #[serde(default = "stored_as_secret")]
    secret: bool,
}

fn stored_as_secret() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredEnvSet {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    variables: Vec<StoredEnvVariable>,
    #[serde(default)]
    created_at_epoch_seconds: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredEnvSets {
    #[serde(default)]
    sets: Vec<StoredEnvSet>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnvVariableInput {
    key: String,
    /// `None` keeps the value already stored under this key.
    value: Option<String>,
    /// Masked in the interface and left out of every summary once stored.
    secret: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnvSetInput {
    /// Absent creates a set; present updates that set in place.
    set_id: Option<String>,
    name: String,
    /// The complete variable list: a stored key that is not listed is removed.
    variables: Vec<EnvVariableInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachEnvSetInput {
    /// Null detaches the machine from every set.
    set_id: Option<String>,
}

/// One variable as the interface shows it, value included.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvVariableSummary {
    key: String,
    value: String,
}

/// An env set as the interface may show it: plain variables with their values, secrets by key.
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvSetSummary {
    id: String,
    name: String,
    variables: Vec<EnvVariableSummary>,
    /// Their values are left out; `reveal_env_secrets` hands them to the editor on request.
    secret_keys: Vec<String>,
    created_at_epoch_seconds: u64,
    attached_machines: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateAppleProfileResult {
    profile: apple_api::AppleProvisioningProfileSummary,
    certificate: apple_api::AppleCertificateSummary,
    saved_path: String,
    kit: SigningKitSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredMacGuestAccess {
    username: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MacGuestAccessInput {
    username: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthorizeMacGuestKeyInput {
    username: String,
    password: String,
}

impl std::fmt::Debug for AuthorizeMacGuestKeyInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorizeMacGuestKeyInput")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// How Xcode activation gets its administrator password: typed here, it runs over the bridge
/// with `sudo`; absent or blank, the guest Terminal opens and the user types it there.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivateMacXcodeInput {
    #[serde(default)]
    password: Option<String>,
}

impl std::fmt::Debug for ActivateMacXcodeInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActivateMacXcodeInput")
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustMacGuestInput {
    fingerprint: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportMacXcodeInput {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApproveAppleWorkspaceInput {
    path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAppleWorkspace {
    local_path: String,
    name: String,
    ios_workspace: String,
    scheme: String,
    #[serde(default)]
    development_team: Option<String>,
    #[serde(default)]
    bundle_identifier: Option<String>,
    last_snapshot_sha256: Option<String>,
    last_sync_file_count: Option<u64>,
    last_sync_bytes: Option<u64>,
    last_build_succeeded: bool,
    last_xcode_version: Option<String>,
    #[serde(default)]
    last_native_lock_updated: bool,
    /// Which SDK the last unsigned build compiled against; None on records from before the choice.
    #[serde(default)]
    last_build_target: Option<UnsignedBuildTarget>,
    /// The identifier the App target's Debug configuration builds, once the device step has
    /// registered it at Apple; the device build is signed for it and installs beside the store
    /// build. None until then, or when it is the approved identifier.
    #[serde(default)]
    debug_bundle_identifier: Option<String>,
    /// What the last snapshot was taken from: the approved folder as it was, or a checked-out
    /// revision of it requested by a remote build.
    #[serde(default)]
    last_source: Option<WorkspaceSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceSource {
    /// `folder` or `git`.
    kind: String,
    git_ref: Option<String>,
    commit: Option<String>,
}

impl WorkspaceSource {
    fn folder() -> Self {
        Self {
            kind: "folder".to_string(),
            git_ref: None,
            commit: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSigningProvisioning {
    container_id: String,
    result: SigningProvisioningResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAppleArchive {
    container_id: String,
    snapshot_sha256: String,
    signing_certificate_sha256: String,
    result: AppleArchiveResult,
    /// The env set the web assets were built with, if the build chose one.
    #[serde(default)]
    env_set_name: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct MacGuestAccessView {
    username: Option<String>,
    public_key: Option<String>,
    ssh: GuestSshStatus,
    diagnostics: GuestDiagnostics,
    /// The phones the guest saw at its last listing; empty until one is requested.
    devices: Vec<buildbridge_docker_osx::GuestDevice>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MacBuilderView {
    machine_id: String,
    profile: MacBuilderConfig,
    busy_operation: Option<String>,
    runtime: RuntimeStatus,
    /// The signing kit this machine will provision, resolved through its attachment.
    signing_kit: Option<SigningKitSummary>,
    /// The env set written into the guest workspace at sync, if one is attached.
    env_set: Option<EnvSetSummary>,
    signing_health: SigningHealth,
    /// Why the credential vault could not be read, when that is the problem.
    vault_issue: Option<String>,
    guest: MacGuestAccessView,
    apple_workspace: Option<StoredAppleWorkspace>,
    signing: Option<SigningProvisioningResult>,
    archive: Option<AppleArchiveResult>,
    /// The env set the retained archive was built with, if any.
    archive_env_set: Option<String>,
    archive_error: Option<String>,
    logs: Vec<String>,
    /// USB passthrough: the host's phones and rule, the container's access, the attachment.
    usb: buildbridge_docker_osx::MachineUsbStatus,
    /// The last run on a phone, kept until cleared; bound to the container like signing.
    device_run: Option<AppleDeviceRunResult>,
    /// The last failed device run, retained like `archive_error` until cleared.
    device_run_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachUsbDeviceInput {
    bus: u8,
    port: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportMacXcodeResult {
    view: MacBuilderView,
    installed_path: String,
    activation_commands: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncAppleWorkspaceResult {
    view: MacBuilderView,
    sync: AppleWorkspaceSyncResult,
}

/// The guest's refreshed Podfile.lock adopted into the approved project: what changed, where it
/// went, and where the previous copy is kept.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdoptPodfileLockResult {
    view: MacBuilderView,
    changes: PodfileLockChanges,
    host_path: String,
    backup_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairGuestDeviceInput {
    udid: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunAppleSmokeBuildInput {
    #[serde(default)]
    target: UnsignedBuildTarget,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunAppleSmokeBuildResult {
    view: MacBuilderView,
    build: AppleSmokeBuildResult,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunAppleArchiveResult {
    view: MacBuilderView,
    archive: AppleArchiveResult,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum RunState {
    Idle,
    Completed,
    /// The build ran and was reported, but did not succeed.
    Failed,
    Busy,
}

async fn read_token(runner_id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        credential_entry(&runner_id)?
            .get_password()
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn write_token(runner_id: String, token: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        credential_entry(&runner_id)?
            .set_password(&token)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn delete_token(runner_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let entry = credential_entry(&runner_id)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

fn credential_entry(runner_id: &str) -> Result<Entry, String> {
    Entry::new(CREDENTIAL_SERVICE, runner_id).map_err(|error| error.to_string())
}

fn mac_builder_credential_entry() -> Result<Entry, String> {
    Entry::new(
        MAC_BUILDER_CREDENTIAL_SERVICE,
        MAC_BUILDER_CREDENTIAL_ACCOUNT,
    )
    .map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Native file pickers for the paths BuildBridge asks for: signing files, a project
        // folder, and the Xcode archive.
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_runner_status,
            pair_runner,
            unpair_runner,
            get_realtime_configuration,
            authorize_realtime,
            heartbeat_runner,
            run_once,
            list_machines,
            create_machine,
            delete_machine,
            discard_machine_container,
            get_mac_builder_status,
            open_developer_tools,
            open_safari_web_inspector,
            configure_mac_builder,
            launch_mac_builder,
            stop_mac_builder,
            install_usb_release_rule,
            remove_usb_release_rule,
            migrate_machine_for_usb,
            attach_usb_device,
            rebuild_machine_container,
            detach_usb_device,
            list_guest_devices,
            pair_guest_device,
            prepare_apple_device_signing,
            run_apple_device_build,
            clear_apple_device_run,
            create_apple_development_certificate,
            list_signing_kits,
            save_signing_kit,
            delete_signing_kit,
            attach_signing_kit,
            verify_apple_developer_team,
            create_apple_replacement_profile,
            download_apple_profile,
            create_apple_distribution_certificate,
            list_managed_apple_profiles,
            cancel_machine_operation,
            list_guest_optimizations,
            apply_guest_optimization,
            list_env_sets,
            save_env_set,
            delete_env_set,
            attach_env_set,
            reveal_env_secrets,
            configure_mac_guest_access,
            authorize_mac_guest_key,
            trust_mac_builder_guest,
            forget_mac_builder_guest_trust,
            import_mac_xcode_package,
            activate_mac_xcode,
            provision_mac_signing,
            clear_mac_guest_signing,
            approve_apple_workspace,
            clear_apple_workspace,
            sync_apple_workspace,
            run_apple_smoke_build,
            adopt_guest_podfile_lock,
            run_apple_signed_archive,
            reveal_apple_archive,
            clear_apple_archive,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .setup(|app| {
            let _ = tray::setup(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_secret_input() -> SigningKitInput {
        SigningKitInput {
            kit_id: None,
            name: "Team kit".to_string(),
            app_store_connect_key_id: String::new(),
            app_store_connect_issuer_id: String::new(),
            app_store_connect_private_key_path: String::new(),
            signing_certificate_path: String::new(),
            signing_certificate_password: String::new(),
            provisioning_profile_paths: Vec::new(),
            guest_keychain_password: String::new(),
            development_certificate_path: String::new(),
            development_certificate_password: String::new(),
        }
    }

    #[test]
    fn identities_are_packaged_with_the_algorithms_macos_reads() {
        let args = certificate_p12_args("/k.pem", "/c.pem", "Apple Development: X");
        for flag in ["-keypbe", "-certpbe", "-macalg"] {
            assert!(args.iter().any(|arg| arg == flag), "{flag}");
        }
        assert!(args.windows(2).any(|w| w[0] == "-macalg" && w[1] == "sha1"));
        assert!(args.iter().filter(|arg| *arg == "PBE-SHA1-3DES").count() == 2);
        assert!(args.iter().any(|arg| arg == "env:BUILDBRIDGE_P12_PASSWORD"));
        assert!(!args.iter().any(|arg| arg.contains("secret")));

        // OpenSSL 3's default is what macOS refuses; a Mac export is what it wrote itself.
        assert!(pkcs12_needs_repackaging(
            "MAC: sha256, Iteration 2048\nMAC length: 32, salt length: 8\nPKCS7 Encrypted data: PBES2, PBKDF2, AES-256-CBC, Iteration 2048, PRF hmacWithSHA256\n"
        ));
        assert!(!pkcs12_needs_repackaging(
            "MAC: sha1, Iteration 2048\nMAC length: 20, salt length: 8\nPKCS7 Encrypted data: pbeWithSHA1And3-KeyTripleDES-CBC, Iteration 2048\n"
        ));
        assert!(!pkcs12_needs_repackaging(""));
    }

    #[test]
    fn a_refused_distribution_certificate_names_the_kit_that_already_holds_one() {
        let refusal =
            "Apple refused to issue another distribution certificate. Apple says: current."
                .to_string();
        let hinted = with_other_kit_hint(
            refusal.clone(),
            apple_api::CertificateKind::Distribution,
            &["Dist kit".to_string()],
        );
        assert!(hinted.starts_with(&refusal));
        assert!(hinted.contains("Dist kit already holds a distribution identity"));
        assert!(hinted.contains("attach that kit"));

        let two = with_other_kit_hint(
            refusal.clone(),
            apple_api::CertificateKind::Distribution,
            &["A".to_string(), "B".to_string()],
        );
        assert!(two.contains("A, B already hold"));

        // No holders, a different kind, or a different error: untouched.
        assert_eq!(
            with_other_kit_hint(
                refusal.clone(),
                apple_api::CertificateKind::Distribution,
                &[]
            ),
            refusal
        );
        assert_eq!(
            with_other_kit_hint(
                refusal.clone(),
                apple_api::CertificateKind::Development,
                &["Dist kit".to_string()]
            ),
            refusal
        );
        assert_eq!(
            with_other_kit_hint(
                "Apple rejected the Team API key.".to_string(),
                apple_api::CertificateKind::Distribution,
                &["Dist kit".to_string()]
            ),
            "Apple rejected the Team API key."
        );
    }

    #[test]
    fn app_store_connect_credentials_must_be_complete() {
        let input = SigningKitInput {
            app_store_connect_key_id: "KEY123".to_string(),
            ..empty_secret_input()
        };

        let error = normalize_signing_kit(input).expect_err("partial key must fail");

        assert_eq!(
            error,
            "App Store Connect key ID, issuer ID, and private .p8 path must be provided together."
        );
    }

    #[test]
    fn malformed_app_store_connect_private_key_is_rejected() {
        let private_key_path = write_test_private_key("malformed", "not a private key");
        let input = SigningKitInput {
            app_store_connect_key_id: "KEY123".to_string(),
            app_store_connect_issuer_id: "issuer-123".to_string(),
            app_store_connect_private_key_path: private_key_path.display().to_string(),
            ..empty_secret_input()
        };

        let error = normalize_signing_kit(input).expect_err("invalid PEM must fail");
        remove_test_private_key(&private_key_path);

        assert_eq!(
            error,
            "The App Store Connect private key is not a valid PEM key."
        );
    }

    #[test]
    fn app_store_connect_key_id_is_inferred_from_the_private_key_filename() {
        let private_key_path = write_test_private_key(
            "AuthKey_837B3VAM6Z",
            "-----BEGIN PRIVATE KEY-----\ntest-only\n-----END PRIVATE KEY-----",
        );
        let input = SigningKitInput {
            app_store_connect_issuer_id: "issuer-123".to_string(),
            app_store_connect_private_key_path: private_key_path.display().to_string(),
            ..empty_secret_input()
        };

        let secrets = normalize_signing_kit(input).expect("complete key should be accepted");
        remove_test_private_key(&private_key_path);

        assert_eq!(
            secrets.app_store_connect_key_id.as_deref(),
            Some("837B3VAM6Z")
        );
        assert!(
            secrets
                .app_store_connect_private_key
                .as_deref()
                .is_some_and(|key| key.contains("test-only"))
        );
    }

    #[test]
    fn mismatched_app_store_connect_key_id_is_rejected() {
        let private_key_path = write_test_private_key(
            "AuthKey_837B3VAM6Z",
            "-----BEGIN PRIVATE KEY-----\ntest-only\n-----END PRIVATE KEY-----",
        );
        let input = SigningKitInput {
            app_store_connect_key_id: "DIFFERENT1".to_string(),
            app_store_connect_issuer_id: "issuer-123".to_string(),
            app_store_connect_private_key_path: private_key_path.display().to_string(),
            ..empty_secret_input()
        };

        let error = normalize_signing_kit(input).expect_err("mismatched key must fail");
        remove_test_private_key(&private_key_path);

        assert_eq!(
            error,
            "The App Store Connect key ID does not match the selected AuthKey_837B3VAM6Z.p8 file."
        );
    }

    fn write_test_private_key(name: &str, contents: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("buildbridge-{}-{nonce}", std::process::id()));
        fs::create_dir(&directory).expect("test private key directory should be created");
        let path = directory.join(format!("{name}.p8"));
        fs::write(&path, contents).expect("test private key should be written");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .expect("test private key permissions should be restricted");
        }

        path
    }

    fn remove_test_private_key(path: &std::path::Path) {
        fs::remove_file(path).expect("test private key should be removed");
        fs::remove_dir(path.parent().expect("test key should have a parent"))
            .expect("test private key directory should be removed");
    }

    #[test]
    fn secret_summary_exposes_metadata_without_secret_values() {
        let secrets = StoredSigningKit {
            id: "team".to_string(),
            name: "Team kit".to_string(),
            created_at_epoch_seconds: 0,
            app_store_connect_key_id: Some("KEY123".to_string()),
            app_store_connect_issuer_id: Some("issuer-123".to_string()),
            app_store_connect_private_key: Some("private-value".to_string()),
            signing_certificate_path: Some("/secure/signing.p12".to_string()),
            signing_certificate_password: Some("certificate-password".to_string()),
            provisioning_profile_paths: vec!["/secure/app.mobileprovision".to_string()],
            guest_keychain_password: Some("keychain-password".to_string()),
            development_certificate_path: None,
            development_certificate_password: None,
            development_certificate_serial_number: None,
        };

        let summary = summarize_signing_kit(&secrets);
        let encoded = serde_json::to_string(&summary).expect("summary should serialize");

        assert!(summary.app_store_connect_configured);
        assert_eq!(summary.app_store_connect_key_id.as_deref(), Some("KEY123"));
        assert_eq!(
            summary.signing_certificate_name.as_deref(),
            Some("signing.p12")
        );
        assert_eq!(
            summary.provisioning_profile_names,
            vec!["app.mobileprovision"]
        );
        assert_eq!(summary.name, "Team kit");
        assert!(!encoded.contains("private-value"));
        assert!(!encoded.contains("certificate-password"));
        assert!(!encoded.contains("keychain-password"));
    }

    #[test]
    fn saving_one_signing_route_preserves_the_other_stored_route() {
        let existing = StoredSigningKit {
            id: "team".to_string(),
            name: "Team kit".to_string(),
            created_at_epoch_seconds: 7,
            app_store_connect_key_id: Some("KEY123".to_string()),
            app_store_connect_issuer_id: Some("issuer-123".to_string()),
            app_store_connect_private_key: Some("private-value".to_string()),
            signing_certificate_path: None,
            signing_certificate_password: None,
            provisioning_profile_paths: Vec::new(),
            guest_keychain_password: None,
            development_certificate_path: None,
            development_certificate_password: None,
            development_certificate_serial_number: None,
        };
        let incoming = StoredSigningKit {
            id: String::new(),
            name: "Renamed kit".to_string(),
            signing_certificate_path: Some("/secure/signing.p12".to_string()),
            signing_certificate_password: Some("certificate-password".to_string()),
            provisioning_profile_paths: vec!["/secure/app.mobileprovision".to_string()],
            guest_keychain_password: Some("keychain-password".to_string()),
            development_certificate_path: None,
            development_certificate_password: None,
            development_certificate_serial_number: None,
            ..StoredSigningKit::default()
        };

        let merged = merge_signing_kit(existing, incoming);

        assert_eq!(merged.app_store_connect_key_id.as_deref(), Some("KEY123"));
        assert_eq!(
            merged.signing_certificate_path.as_deref(),
            Some("/secure/signing.p12")
        );
        assert_eq!(merged.provisioning_profile_paths.len(), 1);
        assert_eq!(merged.id, "team", "an update keeps the kit's identity");
        assert_eq!(merged.created_at_epoch_seconds, 7);
        assert_eq!(merged.name, "Renamed kit", "an update may rename the kit");
    }

    #[test]
    fn a_kit_needs_a_name() {
        let input = SigningKitInput {
            name: "   ".to_string(),
            ..empty_secret_input()
        };

        assert_eq!(
            normalize_signing_kit(input).expect_err("a blank name must fail"),
            "Give the signing kit a name of 1 to 60 characters."
        );
    }

    #[test]
    fn xcode_project_settings_detect_one_team_and_prefer_the_release_bundle() {
        let project = r#"
            DEVELOPMENT_TEAM = TEAM123456;
            PRODUCT_BUNDLE_IDENTIFIER = com.example.app.debug;
            DEVELOPMENT_TEAM = TEAM123456;
            PRODUCT_BUNDLE_IDENTIFIER = com.example.app;
        "#;

        assert_eq!(
            one_xcode_setting(project, "DEVELOPMENT_TEAM").as_deref(),
            Some("TEAM123456")
        );
        assert_eq!(
            release_bundle_identifier(project).as_deref(),
            Some("com.example.app")
        );
        assert!(
            one_xcode_setting("DEVELOPMENT_TEAM = $(malicious);", "DEVELOPMENT_TEAM").is_none()
        );
    }

    #[test]
    fn older_workspace_records_default_new_signing_metadata() {
        let workspace: StoredAppleWorkspace = serde_json::from_value(serde_json::json!({
            "localPath": "/project",
            "name": "example",
            "iosWorkspace": "ios/App/App.xcworkspace",
            "scheme": "App",
            "lastSnapshotSha256": null,
            "lastSyncFileCount": null,
            "lastSyncBytes": null,
            "lastBuildSucceeded": false,
            "lastXcodeVersion": null,
            "lastNativeLockUpdated": false
        }))
        .expect("older workspace record should remain readable");

        assert_eq!(workspace.development_team, None);
        assert_eq!(workspace.bundle_identifier, None);
    }

    fn kit(id: &str, complete: bool) -> StoredSigningKit {
        StoredSigningKit {
            id: id.to_string(),
            name: format!("{id} kit"),
            signing_certificate_path: complete.then(|| "/secure/dist.p12".to_string()),
            signing_certificate_password: complete.then(|| "passphrase".to_string()),
            provisioning_profile_paths: if complete {
                vec!["/secure/app.mobileprovision".to_string()]
            } else {
                Vec::new()
            },
            guest_keychain_password: complete.then(|| "keychain".to_string()),
            development_certificate_path: None,
            development_certificate_password: None,
            development_certificate_serial_number: None,
            ..StoredSigningKit::default()
        }
    }

    fn variable(key: &str, value: &str) -> StoredEnvVariable {
        StoredEnvVariable {
            key: key.to_string(),
            value: value.to_string(),
            secret: false,
        }
    }

    fn secret(key: &str, value: &str) -> StoredEnvVariable {
        StoredEnvVariable {
            secret: true,
            ..variable(key, value)
        }
    }

    #[test]
    fn env_keys_follow_shell_and_dotenv_naming() {
        for ok in ["VITE_API_URL", "_private", "A1", "lower_case"] {
            assert!(valid_env_key(ok), "{ok}");
        }
        for bad in ["", "1ABC", "MY-KEY", "MY KEY", "a.b", "KEY="] {
            assert!(!valid_env_key(bad), "{bad}");
        }
    }

    #[test]
    fn env_values_that_the_two_renderings_would_disagree_on_are_refused() {
        assert_eq!(env_value_issue("https://api.example.com"), None);
        assert_eq!(env_value_issue("it's fine"), None);
        assert_eq!(env_value_issue("say \"hi\""), None);
        assert!(env_value_issue("both ' and \"").is_some());
        assert!(env_value_issue("two\nlines").is_some());
        assert!(env_value_issue(&"x".repeat(MAX_ENV_VALUE_LENGTH + 1)).is_some());
    }

    #[test]
    fn dotenv_rendering_keeps_dollars_and_quotes_literal() {
        let rendered = render_dotenv(&[
            variable("VITE_API_URL", "https://api.example.com/v1"),
            variable("VITE_PRICE", "$5 and `more`"),
            variable("VITE_QUOTED", "say \"hi\""),
        ]);

        assert!(rendered.contains("VITE_API_URL=\"https://api.example.com/v1\"\n"));
        assert!(rendered.contains("VITE_PRICE=\"\\$5 and \\`more\\`\"\n"));
        assert!(rendered.contains("VITE_QUOTED='say \"hi\"'\n"));
        assert!(rendered.starts_with("# Written by BuildBridge"));
    }

    #[test]
    fn shell_rendering_single_quotes_everything_exactly() {
        let rendered = render_shell_env(&[
            variable("API_URL", "https://api.example.com/$path"),
            variable("GREETING", "it's fine"),
        ]);

        assert!(rendered.contains("export API_URL='https://api.example.com/$path'\n"));
        assert!(rendered.contains("export GREETING='it'\\''s fine'\n"));
    }

    #[test]
    fn editing_a_set_keeps_unchanged_values_and_drops_unlisted_keys() {
        let existing = StoredEnvSet {
            id: "production".to_string(),
            name: "production".to_string(),
            variables: vec![variable("KEEP", "old"), variable("GONE", "x")],
            created_at_epoch_seconds: 0,
        };
        let input = EnvSetInput {
            set_id: Some("production".to_string()),
            name: "production".to_string(),
            variables: vec![
                EnvVariableInput {
                    key: "KEEP".to_string(),
                    value: None,
                    secret: false,
                },
                EnvVariableInput {
                    key: "NEW".to_string(),
                    value: Some("fresh".to_string()),
                    secret: false,
                },
            ],
        };

        let merged = merge_env_variables(Some(&existing), &input).expect("merges");

        assert_eq!(
            merged,
            vec![variable("KEEP", "old"), variable("NEW", "fresh")]
        );
    }

    #[test]
    fn a_new_key_needs_a_value_and_keys_cannot_repeat() {
        let missing = EnvSetInput {
            set_id: None,
            name: "staging".to_string(),
            variables: vec![EnvVariableInput {
                key: "NEW".to_string(),
                value: None,
                secret: false,
            }],
        };
        assert!(merge_env_variables(None, &missing).is_err());

        let repeated = EnvSetInput {
            set_id: None,
            name: "staging".to_string(),
            variables: vec![
                EnvVariableInput {
                    key: "A".to_string(),
                    value: Some("1".to_string()),
                    secret: false,
                },
                EnvVariableInput {
                    key: "A".to_string(),
                    value: Some("2".to_string()),
                    secret: false,
                },
            ],
        };
        assert!(merge_env_variables(None, &repeated).is_err());
    }

    #[test]
    fn a_stored_secret_is_kept_blank_but_never_carried_into_a_plain_variable() {
        let existing = StoredEnvSet {
            id: "production".to_string(),
            name: "production".to_string(),
            variables: vec![
                secret("TOKEN", "hidden"),
                variable("URL", "https://a.example"),
            ],
            created_at_epoch_seconds: 0,
        };
        let listed = |secret: bool| EnvSetInput {
            set_id: Some("production".to_string()),
            name: "production".to_string(),
            variables: vec![EnvVariableInput {
                key: "TOKEN".to_string(),
                value: None,
                secret,
            }],
        };

        assert_eq!(
            merge_env_variables(Some(&existing), &listed(true)).expect("keeps"),
            vec![secret("TOKEN", "hidden")]
        );
        let refused = merge_env_variables(Some(&existing), &listed(false)).expect_err("refuses");
        assert!(refused.contains("stored as a secret"), "{refused}");
    }

    #[test]
    fn sets_stored_before_secrets_were_distinguished_read_back_as_secrets() {
        let stored: StoredEnvSets = serde_json::from_str(
            r#"{"sets":[{"id":"p","name":"p","variables":[{"key":"A","value":"1"},{"key":"B","value":"2","secret":false}]}]}"#,
        )
        .expect("parses");

        assert_eq!(
            stored.sets[0].variables,
            vec![secret("A", "1"), variable("B", "2")]
        );
    }

    #[test]
    fn a_summary_shows_plain_values_and_only_the_keys_of_secrets() {
        let set = StoredEnvSet {
            id: "production".to_string(),
            name: "production".to_string(),
            variables: vec![
                variable("URL", "https://a.example"),
                secret("TOKEN", "hidden"),
            ],
            created_at_epoch_seconds: 0,
        };

        let summary = summarize_env_set(&set, &[]);

        assert_eq!(summary.variables.len(), 1);
        assert_eq!(summary.variables[0].key, "URL");
        assert_eq!(summary.variables[0].value, "https://a.example");
        assert_eq!(summary.secret_keys, vec!["TOKEN".to_string()]);
        let encoded = serde_json::to_string(&summary).expect("serializes");
        assert!(!encoded.contains("hidden"), "{encoded}");
    }

    #[test]
    fn a_set_reveals_its_secrets_and_nothing_else() {
        let stored = StoredEnvSets {
            sets: vec![StoredEnvSet {
                id: "production".to_string(),
                name: "production".to_string(),
                variables: vec![
                    variable("URL", "https://a.example"),
                    secret("TOKEN", "hidden"),
                ],
                created_at_epoch_seconds: 0,
            }],
        };

        assert_eq!(
            stored_env_secrets(&stored, "production").expect("reveals"),
            vec![EnvVariableSummary {
                key: "TOKEN".to_string(),
                value: "hidden".to_string(),
            }]
        );
        assert!(stored_env_secrets(&stored, "staging").is_err());
    }

    #[test]
    fn the_openssl_recipe_is_fixed_argv_with_the_password_kept_out_of_it() {
        let key = certificate_key_args();
        assert_eq!(key[0], "genpkey");
        assert!(key.contains(&"rsa_keygen_bits:2048".to_string()));

        let csr = certificate_csr_args("/keys/key.pem", "BuildBridge Distribution");
        assert!(csr.contains(&"-batch".to_string()), "must never prompt");
        assert!(csr.contains(&"/CN=BuildBridge Distribution".to_string()));

        let p12 = certificate_p12_args(
            "/keys/key.pem",
            "/keys/certificate.pem",
            "Apple Distribution: Example",
        );
        assert!(p12.contains(&"env:BUILDBRIDGE_P12_PASSWORD".to_string()));
        assert!(!p12.iter().any(|arg| arg.starts_with("pass:")));
        assert!(
            !p12.contains(&"-out".to_string()),
            "the .p12 is written owner-only by BuildBridge"
        );
        for arg in key.iter().chain(csr.iter()).chain(p12.iter()) {
            assert!(!arg.contains(';') && !arg.contains("$("));
        }
    }

    #[test]
    fn only_managed_profile_copies_are_offered_from_the_host() {
        assert!(is_managed_profile_file(
            "2f3d9c10-0a6b-4f4e-9c2e-1d0b7a5e6c11.mobileprovision"
        ));
        assert!(is_managed_profile_file("AppStore.MOBILEPROVISION"));
        assert!(!is_managed_profile_file(".hidden.mobileprovision"));
        assert!(!is_managed_profile_file("notes.txt"));
        assert!(!is_managed_profile_file("mobileprovision"));
        assert!(!is_managed_profile_file(""));
    }

    #[test]
    fn the_pre_registry_vault_record_migrates_to_one_named_kit() {
        // Exactly what the first release wrote: a bare object with no `kits` array.
        let legacy = serde_json::json!({
            "appStoreConnectKeyId": "KEYID12345",
            "appStoreConnectIssuerId": "issuer",
            "appStoreConnectPrivateKey": "-----BEGIN PRIVATE KEY-----",
            "signingCertificatePath": "/secure/dist.p12",
            "signingCertificatePassword": "passphrase",
            "provisioningProfilePaths": ["/secure/app.mobileprovision"],
            "guestKeychainPassword": "keychain"
        })
        .to_string();

        let kits = parse_signing_vault(&legacy).expect("the legacy record should migrate");

        assert_eq!(kits.kits.len(), 1);
        assert_eq!(kits.kits[0].id, DEFAULT_SIGNING_KIT_ID);
        assert_eq!(kits.kits[0].name, "Signing kit");
        assert_eq!(
            kits.kits[0].signing_certificate_path.as_deref(),
            Some("/secure/dist.p12"),
            "migration must carry the material across, not just the shape"
        );
        assert!(kit_is_complete(&kits.kits[0]));
    }

    #[test]
    fn a_registry_vault_record_is_read_as_stored_and_gains_missing_identity() {
        let stored = serde_json::json!({
            "kits": [
                { "id": "team-a", "name": "Team A", "provisioningProfilePaths": [],
                  "appStoreConnectKeyId": null, "appStoreConnectIssuerId": null,
                  "appStoreConnectPrivateKey": null, "signingCertificatePath": null,
                  "signingCertificatePassword": null, "guestKeychainPassword": null },
                { "provisioningProfilePaths": [],
                  "appStoreConnectKeyId": null, "appStoreConnectIssuerId": null,
                  "appStoreConnectPrivateKey": null, "signingCertificatePath": null,
                  "signingCertificatePassword": null, "guestKeychainPassword": null }
            ]
        })
        .to_string();

        let kits = parse_signing_vault(&stored).expect("a registry record should load");

        assert_eq!(kits.kits.len(), 2);
        assert_eq!(kits.kits[0].id, "team-a");
        assert_eq!(
            kits.kits[1].id, DEFAULT_SIGNING_KIT_ID,
            "a blank id is filled in"
        );
        assert_eq!(kits.kits[1].name, "Signing kit");
    }

    #[test]
    fn an_empty_registry_vault_record_is_not_mistaken_for_a_legacy_one() {
        let kits = parse_signing_vault(r#"{"kits": []}"#).expect("an empty registry should load");

        assert!(kits.kits.is_empty());
    }

    #[test]
    fn unreadable_vault_content_is_reported_rather_than_silently_empty() {
        let error = parse_signing_vault("not json").expect_err("garbage must not read as empty");

        assert!(
            error.starts_with("The signing vault entry is invalid"),
            "unexpected message: {error}"
        );
    }

    #[test]
    fn a_machine_uses_its_attached_kit_and_no_other() {
        let kits = vec![kit("team-a", true), kit("team-b", true)];

        let resolved =
            resolve_signing_kit(&kits, Some("team-b")).expect("the attachment should resolve");

        assert_eq!(resolved.id, "team-b");
        assert!(resolve_signing_kit(&kits, Some("gone")).is_none());
    }

    #[test]
    fn a_kit_is_never_picked_for_a_machine_even_when_it_is_the_only_one() {
        let kits = vec![kit("only", true)];

        assert!(
            resolve_signing_kit(&kits, None).is_none(),
            "which identity signs a build is a choice the interface must show being made"
        );
    }

    #[test]
    fn an_attachment_to_a_removed_kit_resolves_to_nothing() {
        let kits = vec![kit("team-a", true)];

        assert!(resolve_signing_kit(&kits, Some("deleted")).is_none());
    }

    #[test]
    fn a_provisioned_machine_whose_vault_was_cleared_reports_a_missing_kit() {
        // The shape left behind when the operating-system keyring is recreated: the guest still
        // holds a provisioned keychain, but nothing remains to unlock or rebuild it with.
        assert_eq!(
            signing_health(None, None, true),
            SigningHealth::KitMissing,
            "this must not read as 'never configured'"
        );
        assert_eq!(
            signing_health(None, None, false),
            SigningHealth::Unconfigured
        );
    }

    #[test]
    fn signing_health_separates_a_complete_kit_from_a_partial_one() {
        assert_eq!(
            signing_health(None, Some(&kit("team", true)), false),
            SigningHealth::Ready
        );
        assert_eq!(
            signing_health(None, Some(&kit("team", false)), false),
            SigningHealth::Incomplete
        );
    }

    #[test]
    fn an_unreadable_vault_outranks_every_other_signing_state() {
        assert_eq!(
            signing_health(Some("vault locked"), Some(&kit("team", true)), true),
            SigningHealth::VaultUnavailable,
            "a stale cached kit must not mask a vault that cannot be read"
        );
    }

    #[test]
    fn a_kit_is_complete_only_with_certificate_password_profile_and_keychain() {
        assert!(kit_is_complete(&kit("team", true)));

        for missing in ["certificate", "password", "profile", "keychain"] {
            let mut partial = kit("team", true);
            match missing {
                "certificate" => partial.signing_certificate_path = None,
                "password" => partial.signing_certificate_password = None,
                "profile" => partial.provisioning_profile_paths.clear(),
                _ => partial.guest_keychain_password = None,
            }
            assert!(
                !kit_is_complete(&partial),
                "a kit without its {missing} cannot provision"
            );
        }
    }

    #[test]
    fn a_team_key_with_the_keychain_password_completes_a_kit_because_the_rest_is_created() {
        let mut key_only = kit("team", false);
        key_only.guest_keychain_password = Some("keychain".to_string());
        assert!(!kit_is_complete(&key_only), "nothing to sign with yet");

        key_only.app_store_connect_key_id = Some("KEYID12345".to_string());
        key_only.app_store_connect_issuer_id = Some("issuer".to_string());
        assert!(
            !kit_is_complete(&key_only),
            "a Team key is all three parts, not two"
        );

        key_only.app_store_connect_private_key = Some("-----BEGIN PRIVATE KEY-----".to_string());
        assert!(
            kit_is_complete(&key_only),
            "certificates and profiles are created at Apple when first needed"
        );

        key_only.guest_keychain_password = None;
        assert!(
            !kit_is_complete(&key_only),
            "the keychain password is the one thing nothing can create"
        );
    }

    #[test]
    fn a_development_identity_alone_completes_a_kit_for_the_phone_route() {
        let mut dev_only = kit("team", true);
        dev_only.signing_certificate_path = None;
        dev_only.signing_certificate_password = None;
        dev_only.provisioning_profile_paths.clear();
        assert!(!kit_is_complete(&dev_only), "no identity at all");

        dev_only.development_certificate_path = Some("/kits/development.p12".to_string());
        assert!(!kit_is_complete(&dev_only), "a .p12 without its passphrase");

        dev_only.development_certificate_password = Some("secret".to_string());
        assert!(
            kit_is_complete(&dev_only),
            "a development identity with no profile yet"
        );

        dev_only.guest_keychain_password = None;
        assert!(
            !kit_is_complete(&dev_only),
            "the keychain password is always needed"
        );
    }

    #[test]
    fn managed_apple_profile_names_cannot_escape_the_profile_directory() {
        assert!(safe_apple_resource_component(
            "00000000-0000-0000-0000-000000000001"
        ));
        assert!(safe_apple_resource_component("opaqueAppleId123"));
        assert!(!safe_apple_resource_component("../outside"));
        assert!(!safe_apple_resource_component("profile/name"));
        assert!(!safe_apple_resource_component(""));
    }
}
