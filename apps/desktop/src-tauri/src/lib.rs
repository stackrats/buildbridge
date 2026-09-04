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
mod machines;
mod tray;

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

#[derive(Default)]
struct AppState {
    runner_running: AtomicBool,
    /// Machines with a long-running operation in flight, keyed by machine id. The value is a
    /// stable snake_case operation key that the frontend maps back to a step.
    busy_machines: Mutex<HashMap<String, &'static str>>,
    /// The cancellable scope of each in-flight operation, keyed by machine id.
    operation_scopes: Mutex<HashMap<String, Arc<OperationScope>>>,
    /// A privileged host change (the USB udev rule) is in flight, so a second authorization
    /// prompt cannot stack on the first.
    host_usb_busy: AtomicBool,
    /// Why the phone a machine holds has not shown up in the guest yet, from the last attach.
    /// Cleared once the guest enumerates it or the phone is detached.
    usb_attach_issues: Mutex<HashMap<String, String>>,
    /// The phones each guest reported at its last listing. Probing `devicectl` costs seconds,
    /// so the view serves this and the listing command refreshes it.
    guest_devices: Mutex<HashMap<String, Vec<buildbridge_docker_osx::GuestDevice>>>,
}

/// Marks the host busy with a privileged USB change until dropped.
struct HostUsbGuard {
    app: AppHandle,
}

impl Drop for HostUsbGuard {
    fn drop(&mut self) {
        self.app
            .state::<AppState>()
            .host_usb_busy
            .store(false, Ordering::Release);
    }
}

fn begin_host_usb_operation(app: &AppHandle) -> Result<HostUsbGuard, String> {
    let claimed = app
        .state::<AppState>()
        .host_usb_busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok();
    if !claimed {
        return Err("A USB rule change is already waiting for authorization.".to_string());
    }

    Ok(HostUsbGuard { app: app.clone() })
}

/// Marks one machine busy until dropped so concurrent operations cannot interleave.
struct MachineOperationGuard {
    app: AppHandle,
    machine_id: String,
    scope: Arc<OperationScope>,
}

impl MachineOperationGuard {
    /// The scope a blocking closure enters so its child processes can be stopped.
    fn scope(&self) -> Arc<OperationScope> {
        Arc::clone(&self.scope)
    }
}

impl Drop for MachineOperationGuard {
    fn drop(&mut self) {
        if let Ok(mut busy) = self.app.state::<AppState>().busy_machines.lock() {
            busy.remove(&self.machine_id);
        }
        if let Ok(mut scopes) = self.app.state::<AppState>().operation_scopes.lock() {
            scopes.remove(&self.machine_id);
        }
        let _ = self.app.emit(
            MACHINE_CHANGED_EVENT,
            MachineChangedEvent {
                machine_id: Some(self.machine_id.clone()),
            },
        );
    }
}

fn begin_machine_operation(
    app: &AppHandle,
    machine_id: &str,
    label: &'static str,
) -> Result<MachineOperationGuard, String> {
    let state = app.state::<AppState>();
    let mut busy = state
        .busy_machines
        .lock()
        .map_err(|_| "The machine operation registry is poisoned.".to_string())?;
    if busy.contains_key(machine_id) {
        return Err(
            "Another operation is still running on this machine. Wait for it to finish."
                .to_string(),
        );
    }
    busy.insert(machine_id.to_string(), label);
    drop(busy);
    let scope = OperationScope::new();
    if let Ok(mut scopes) = state.operation_scopes.lock() {
        scopes.insert(machine_id.to_string(), Arc::clone(&scope));
    }
    let _ = app.emit(
        MACHINE_CHANGED_EVENT,
        MachineChangedEvent {
            machine_id: Some(machine_id.to_string()),
        },
    );

    Ok(MachineOperationGuard {
        app: app.clone(),
        machine_id: machine_id.to_string(),
        scope,
    })
}

/// Stops whatever is running on a machine: kills the operation's host-side children, and for
/// a build that deliberately outlives its SSH session, the job inside the guest as well. The
/// operation then returns as stopped rather than as a failure.
#[tauri::command]
async fn cancel_machine_operation(app: AppHandle, machine_id: String) -> Result<(), String> {
    let scope = app
        .state::<AppState>()
        .operation_scopes
        .lock()
        .map_err(|_| "The machine operation registry is poisoned.".to_string())?
        .get(&machine_id)
        .cloned();
    let label = busy_operation(&app, &machine_id)?;
    let Some(scope) = scope else {
        return Err("Nothing is running on this machine.".to_string());
    };
    if label.as_deref() == Some("migrating_usb") {
        return Err("The disk migration cannot be stopped; wait for it to finish.".to_string());
    }
    scope.cancel();

    if matches!(
        label.as_deref(),
        Some("test_building" | "archiving" | "running_on_device")
    ) {
        let paths = MachinePaths::resolve(&app, &machine_id)?;
        if let Some(access) = load_mac_guest_access(&paths)?
            && let Ok(registry) = machines::load_registry(&app)
            && let Ok(machine) = registry.find(&machine_id)
        {
            let ssh_port = machine.config.ssh_port;
            let identity = paths.guest_identity();
            let known_hosts = paths.known_hosts();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                buildbridge_docker_osx::stop_guest_jobs(
                    ssh_port,
                    &access.username,
                    &identity,
                    &known_hosts,
                )
            })
            .await;
        }
    }

    Ok(())
}

/// The outcome of a blocking operation, with a stop reported as a stop rather than as whatever
/// error the killed process happened to produce.
fn finish_operation<T>(
    scope: &OperationScope,
    joined: Result<Result<T, String>, String>,
) -> Result<T, String> {
    match joined {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) | Err(error) => {
            if scope.is_cancelled() {
                Err(CANCELLED_MESSAGE.to_string())
            } else {
                Err(error)
            }
        }
    }
}

const CANCELLED_MESSAGE: &str = "Stopped.";

fn busy_operation(app: &AppHandle, machine_id: &str) -> Result<Option<String>, String> {
    let state = app.state::<AppState>();
    let busy = state
        .busy_machines
        .lock()
        .map_err(|_| "The machine operation registry is poisoned.".to_string())?;

    Ok(busy.get(machine_id).map(|label| (*label).to_string()))
}

fn emit_machine_progress<T: Serialize + Clone>(
    app: &AppHandle,
    event: &str,
    machine_id: &str,
    progress: T,
) {
    let _ = app.emit(
        event,
        MachineProgressEvent {
            machine_id: machine_id.to_string(),
            progress,
        },
    );
}

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

#[tauri::command]
async fn get_runner_status(app: AppHandle) -> Result<DesktopStatus, String> {
    let config = load_config(&app)?;
    let paired = match config.as_ref() {
        Some(config) => read_token(config.runner_id.clone()).await.is_ok(),
        None => false,
    };

    Ok(DesktopStatus {
        paired,
        // Configuration without a token is the shape a cleared keyring leaves behind. Saying
        // "not paired" there would send someone looking for a pairing code they already used.
        credentials_missing: config.is_some() && !paired,
        server_url: config.as_ref().map(|value| value.server_url.clone()),
        runner_id: config.as_ref().map(|value| value.runner_id.clone()),
        runner_name: config.as_ref().map(|value| value.runner_name.clone()),
        platform: std::env::consts::OS.to_string(),
        architecture: std::env::consts::ARCH.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[tauri::command]
async fn pair_runner(app: AppHandle, input: PairInput) -> Result<DesktopStatus, String> {
    let request = PairRunnerRequest {
        code: input.code,
        name: input.runner_name.clone(),
        platform: std::env::consts::OS.to_string(),
        architecture: std::env::consts::ARCH.to_string(),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        capabilities: runner_capabilities(),
        protocol_version: PROTOCOL_VERSION,
    };
    let paired = ApiClient::pair(&input.server_url, &request)
        .await
        .map_err(|error| error.to_string())?;

    write_token(paired.runner.id.clone(), paired.token).await?;

    let config = StoredConfig {
        server_url: input.server_url.trim_end_matches('/').to_string(),
        runner_id: paired.runner.id,
        runner_name: paired.runner.name,
    };
    save_config(&app, &config)?;

    get_runner_status(app).await
}

#[tauri::command]
async fn unpair_runner(app: AppHandle) -> Result<(), String> {
    if let Some(config) = load_config(&app)? {
        delete_token(config.runner_id).await?;
    }

    let path = config_path(&app)?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }

    Ok(())
}

#[tauri::command]
async fn get_realtime_configuration(app: AppHandle) -> Result<RealtimeConfiguration, String> {
    let (_, client) = paired_client(&app).await?;

    client
        .realtime_configuration()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn authorize_realtime(
    app: AppHandle,
    input: AuthorizeRealtimeInput,
) -> Result<serde_json::Value, String> {
    let (config, client) = paired_client(&app).await?;
    let expected_channel = format!("private-runners.{}", config.runner_id);

    if input.channel_name != expected_channel {
        return Err("The desktop client refused an unexpected realtime channel.".to_string());
    }

    client
        .authorize_realtime(&RealtimeAuthorizationRequest {
            socket_id: input.socket_id,
            channel_name: input.channel_name,
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn heartbeat_runner(app: AppHandle) -> Result<HeartbeatSummary, String> {
    let (_, client) = paired_client(&app).await?;

    let response = client
        .heartbeat(&heartbeat_request(&app).await)
        .await
        .map_err(|error| error.to_string())?;

    Ok(HeartbeatSummary {
        queued_builds: response.queued_builds,
    })
}

/// The part of a heartbeat reply the interface acts on.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatSummary {
    /// Work waiting for this runner. Non-zero means a queue event was missed; claim now.
    queued_builds: u64,
}

#[tauri::command]
async fn run_once(state: State<'_, AppState>, app: AppHandle) -> Result<RunOnceResult, String> {
    if state.runner_running.swap(true, Ordering::AcqRel) {
        return Ok(RunOnceResult {
            state: RunState::Busy,
            build_id: None,
            message: "The runner is already checking for work.".to_string(),
        });
    }

    let result = run_once_inner(&app).await;
    state.runner_running.store(false, Ordering::Release);

    result
}

async fn run_once_inner(app: &AppHandle) -> Result<RunOnceResult, String> {
    let (_, client) = paired_client(app).await?;

    client
        .heartbeat(&heartbeat_request(app).await)
        .await
        .map_err(|error| error.to_string())?;

    let Some(build) = client.claim().await.map_err(|error| error.to_string())? else {
        return Ok(RunOnceResult {
            state: RunState::Idle,
            build_id: None,
            message: "Connected. No queued builds.".to_string(),
        });
    };

    // The runner crate runs what it can by itself; anything that needs a managed machine is
    // executed here, where the machines live.
    let Some(execution) = execute(&build) else {
        return execute_apple_archive_build(app, &client, &build).await;
    };
    client
        .append_logs(&build.id, execution.logs)
        .await
        .map_err(|error| error.to_string())?;
    let succeeded = execution.status == CompletionStatus::Succeeded;
    client
        .complete(
            &build.id,
            &CompleteBuildRequest {
                status: execution.status,
                exit_code: Some(execution.exit_code),
                error: execution.error,
                result: None,
            },
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(RunOnceResult {
        state: if succeeded {
            RunState::Completed
        } else {
            RunState::Failed
        },
        build_id: Some(build.id),
        message: "Build completed and reported to the control plane.".to_string(),
    })
}

/// How often buffered log lines are sent, and how many of those intervals pass between lease
/// renewals. Two minutes is the lease; renewing every minute leaves room for a slow request.
const LOG_PUMP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
const LEASE_RENEWAL_TICKS: u32 = 30;

/// Turns what a machine reports to the desktop's own interface into control-plane log lines.
/// Every progress event the archive pipeline emits is observed here, so the remote log is the
/// same story the Build tab tells, in the same order.
struct LogForwarder {
    pending: Vec<BuildLogLine>,
    next_sequence: u64,
    last_phase: HashMap<String, String>,
}

impl LogForwarder {
    fn new(next_sequence: u64) -> Self {
        Self {
            pending: Vec::new(),
            next_sequence,
            last_phase: HashMap::new(),
        }
    }

    fn push(&mut self, stream: LogStream, message: impl Into<String>) {
        self.pending.push(BuildLogLine {
            sequence: self.next_sequence,
            stream,
            message: message.into(),
        });
        self.next_sequence += 1;
    }

    fn observe(&mut self, event: &str, payload: &serde_json::Value, machine_id: &str) {
        if payload.get("machineId").and_then(serde_json::Value::as_str) != Some(machine_id) {
            return;
        }
        let Some(progress) = payload.get("progress") else {
            return;
        };
        let phase = progress
            .get("phase")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !phase.is_empty() && self.last_phase.get(event) != Some(&phase) {
            self.last_phase.insert(event.to_string(), phase.clone());
            let detail = progress
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            self.push(
                LogStream::System,
                format!("[{}] {detail}", phase.replace('_', " "))
                    .trim_end()
                    .to_string(),
            );
        }
        let line = progress
            .get("logLine")
            .or_else(|| progress.get("log_line"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !line.trim().is_empty() {
            self.push(LogStream::Stdout, line);
        }
    }

    fn take(&mut self) -> Vec<BuildLogLine> {
        std::mem::take(&mut self.pending)
    }
}

/// Sends buffered lines every couple of seconds and keeps the lease alive, on a plain thread so
/// it runs regardless of what the archive pipeline is blocking on. Returns the first failure.
fn spawn_log_pump(
    client: ApiClient,
    build_id: String,
    forwarder: Arc<Mutex<LogForwarder>>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<Option<String>> {
    std::thread::spawn(move || {
        let mut ticks: u32 = 0;
        let mut failure: Option<String> = None;
        loop {
            let stopping = stop.load(Ordering::Acquire);
            let lines = forwarder
                .lock()
                .map(|mut forwarder| forwarder.take())
                .unwrap_or_default();
            if !lines.is_empty()
                && let Err(error) =
                    tauri::async_runtime::block_on(client.append_logs(&build_id, lines))
            {
                failure.get_or_insert(format!("log forwarding failed: {error}"));
            }
            if stopping {
                break;
            }
            ticks += 1;
            if ticks.is_multiple_of(LEASE_RENEWAL_TICKS)
                && let Err(error) = tauri::async_runtime::block_on(client.renew_lease(&build_id))
            {
                failure.get_or_insert(format!("lease renewal failed: {error}"));
            }
            std::thread::sleep(LOG_PUMP_INTERVAL);
        }
        failure
    })
}

/// Runs a claimed `apple_archive` build on one of this host's machines and reports it.
async fn execute_apple_archive_build(
    app: &AppHandle,
    client: &ApiClient,
    build: &ClaimedBuild,
) -> Result<RunOnceResult, String> {
    let payload = AppleArchivePayload::from_value(&build.payload)?;
    let forwarder = Arc::new(Mutex::new(LogForwarder::new(build.next_log_sequence)));
    let stop = Arc::new(AtomicBool::new(false));
    let pump = spawn_log_pump(
        client.clone(),
        build.id.clone(),
        Arc::clone(&forwarder),
        Arc::clone(&stop),
    );

    // The pipeline emits the same events the Build tab listens to; forward them as the log.
    let listeners: Vec<tauri::EventId> = [PROJECT_PROGRESS_EVENT, ARCHIVE_PROGRESS_EVENT]
        .into_iter()
        .map(|event| {
            let forwarder = Arc::clone(&forwarder);
            let machine_id = payload.machine_id.clone();
            app.listen(event, move |emitted| {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(emitted.payload())
                    && let Ok(mut forwarder) = forwarder.lock()
                {
                    forwarder.observe(event, &value, &machine_id);
                }
            })
        })
        .collect();

    let outcome = run_remote_apple_archive(app, &payload, &forwarder).await;
    let env_set_name = match payload.env_set.clone() {
        Some(name) => Some(name),
        None => match attached_env_set_id(app, &payload.machine_id) {
            Ok(Some(id)) => read_env_sets()
                .await
                .ok()
                .and_then(|stored| stored.sets.into_iter().find(|set| set.id == id))
                .map(|set| set.name),
            _ => None,
        },
    };

    for id in listeners {
        app.unlisten(id);
    }
    if let Err(error) = &outcome
        && let Ok(mut forwarder) = forwarder.lock()
    {
        forwarder.push(LogStream::Stderr, error.clone());
    }
    stop.store(true, Ordering::Release);
    let pump_failure = pump.join().ok().flatten();

    let request = match &outcome {
        Ok(archive) => CompleteBuildRequest {
            status: CompletionStatus::Succeeded,
            exit_code: Some(0),
            error: pump_failure,
            result: Some(archive_result_json(archive, env_set_name.as_deref())),
        },
        Err(error) => CompleteBuildRequest {
            status: CompletionStatus::Failed,
            exit_code: Some(1),
            error: Some(error.clone()),
            result: None,
        },
    };
    client
        .complete(&build.id, &request)
        .await
        .map_err(|error| error.to_string())?;

    Ok(match outcome {
        Ok(archive) => RunOnceResult {
            state: RunState::Completed,
            build_id: Some(build.id.clone()),
            message: format!(
                "Signed archive {} ({}) built and reported to the control plane.",
                archive.marketing_version, archive.build_number
            ),
        },
        Err(error) => RunOnceResult {
            state: RunState::Failed,
            build_id: Some(build.id.clone()),
            message: format!("Signed archive failed: {error}"),
        },
    })
}

/// The remote pipeline is the Build tab's own steps in order — synchronize, test build, signed
/// archive — on the approved folder or on a fetched revision of the same project.
async fn run_remote_apple_archive(
    app: &AppHandle,
    payload: &AppleArchivePayload,
    forwarder: &Arc<Mutex<LogForwarder>>,
) -> Result<AppleArchiveResult, String> {
    let log = |stream: LogStream, message: String| {
        if let Ok(mut forwarder) = forwarder.lock() {
            forwarder.push(stream, message);
        }
    };
    let machine = machines::load_registry(app)?
        .find(&payload.machine_id)?
        .clone();
    let paths = MachinePaths::resolve(app, &payload.machine_id)?;
    let approved = load_apple_workspace(&paths)?.ok_or_else(|| {
        "No project is approved on this machine. Approve one in the desktop first.".to_string()
    })?;
    log(
        LogStream::System,
        format!(
            "BuildBridge {} · signed archive of {} on {}",
            env!("CARGO_PKG_VERSION"),
            approved.name,
            machine.config.name
        ),
    );

    let source = match payload.git_ref.as_deref() {
        Some(git_ref) => {
            let remote = project_remote_url(&approved.local_path).ok_or_else(|| {
                "The approved project has no git remote, so only its folder as-is can be built."
                    .to_string()
            })?;
            log(
                LogStream::System,
                format!("$ git fetch {} {git_ref}", redact_remote(&remote)),
            );
            let checkout = paths.checkout_dir();
            let fetch_ref = git_ref.to_string();
            let fetch_dir = checkout.clone();
            let commit = tauri::async_runtime::spawn_blocking(move || {
                checkout_project_ref(&remote, &fetch_ref, &fetch_dir)
            })
            .await
            .map_err(|error| error.to_string())??;
            log(
                LogStream::System,
                format!("Checked out {git_ref} at {commit}"),
            );

            let inspected = inspect_apple_workspace(&checkout.to_string_lossy())?;
            if inspected.bundle_identifier != approved.bundle_identifier
                || inspected.development_team != approved.development_team
            {
                return Err(
                    "That revision targets a different bundle identifier or team than the approved project, so the provisioned signing would not match it."
                        .to_string(),
                );
            }
            Some((
                checkout,
                WorkspaceSource {
                    kind: "git".to_string(),
                    git_ref: Some(git_ref.to_string()),
                    commit: Some(commit),
                },
            ))
        }
        None => None,
    };

    // A remote build names a set; the runner only ever builds with a set it already holds.
    let env_set_id = match payload.env_set.as_deref() {
        Some(name) => Some(
            read_env_sets()
                .await?
                .sets
                .into_iter()
                .find(|set| set.name == name)
                .map(|set| set.id)
                .ok_or_else(|| format!("No env set named {name} is stored on this host."))?,
        ),
        None => attached_env_set_id(app, &payload.machine_id)?,
    };
    log(
        LogStream::System,
        match &env_set_id {
            Some(_) => format!(
                "Env set: {}",
                payload
                    .env_set
                    .clone()
                    .unwrap_or_else(|| "attached to the machine".to_string())
            ),
            None => "Env set: none".to_string(),
        },
    );

    sync_apple_workspace_from(app, &payload.machine_id, source).await?;
    run_apple_smoke_build(app.clone(), payload.machine_id.clone(), None).await?;
    let archived =
        run_apple_signed_archive(app.clone(), payload.machine_id.clone(), env_set_id).await?;

    Ok(archived.archive)
}

/// What the control plane keeps about a finished archive: names, sizes and checksums. The files
/// themselves stay on this host.
fn archive_result_json(archive: &AppleArchiveResult, env_set: Option<&str>) -> serde_json::Value {
    let artifact = |artifact: &AppleArchiveArtifact| {
        serde_json::json!({
            "name": std::path::Path::new(&artifact.path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| artifact.path.clone()),
            "bytes": artifact.bytes,
            "sha256": artifact.sha256,
        })
    };

    serde_json::json!({
        "version": archive.marketing_version,
        "build_number": archive.build_number,
        "bundle_identifier": archive.bundle_identifier,
        "scheme": archive.scheme,
        "export_method": archive.export_method,
        "env_set": env_set,
        "artifacts": [artifact(&archive.ipa), artifact(&archive.archive)],
    })
}

#[tauri::command]
async fn list_machines(app: AppHandle) -> Result<MachineListView, String> {
    build_machine_list_view(&app).await
}

#[tauri::command]
async fn create_machine(
    app: AppHandle,
    profile: MacBuilderConfig,
) -> Result<MachineListView, String> {
    profile.validate().map_err(|error| error.to_string())?;
    let mut registry = machines::load_registry(&app)?;
    if registry.machines.len() >= machines::MAX_MACHINES {
        return Err(format!(
            "BuildBridge manages at most {} machines on one host.",
            machines::MAX_MACHINES
        ));
    }
    registry.ensure_unique_ssh_port(&profile, None)?;
    let existing = registry
        .machines
        .iter()
        .map(|machine| machine.id.as_str())
        .collect::<Vec<_>>();
    let id = machines::machine_id_from_name(&profile.name, &existing);
    // Nothing is attached on creation: which kit signs, and which env a build runs with, are
    // choices the machine page asks for.
    registry.machines.push(StoredMachine {
        id,
        config: profile,
        created_at_epoch_seconds: machines::now_epoch_seconds(),
        signing_kit_id: None,
        env_set_id: None,
    });
    machines::save_registry(&app, &registry)?;
    tray::refresh(&app);

    build_machine_list_view(&app).await
}

#[tauri::command]
async fn delete_machine(
    app: AppHandle,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MachineListView, String> {
    if !input.confirmed {
        return Err("Confirm the machine deletion before continuing.".to_string());
    }
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let mut registry = machines::load_registry(&app)?;
    let index = registry.position(&machine_id)?;
    let guard = begin_machine_operation(&app, &machine_id, "deleting")?;
    let container_name = paths.container_name.clone();
    let scope = guard.scope();
    let removal = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        let runtime =
            buildbridge_docker_osx::status(&container_name).map_err(|error| error.to_string())?;
        if is_live(runtime.state) {
            return Err("Stop the machine before deleting it.".to_string());
        }
        buildbridge_docker_osx::remove(&container_name).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    let removal = removal.and_then(|result| result.map(|_| ()));
    if let Err(error) = removal {
        drop(guard);
        return Err(error);
    }
    paths.remove_machine_files()?;
    registry.machines.remove(index);
    machines::save_registry(&app, &registry)?;
    drop(guard);
    tray::refresh(&app);

    build_machine_list_view(&app).await
}

#[tauri::command]
async fn discard_machine_container(
    app: AppHandle,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MacBuilderView, String> {
    if !input.confirmed {
        return Err("Confirm discarding the macOS disk before continuing.".to_string());
    }
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    machines::load_registry(&app)?.find(&machine_id)?;
    let guard = begin_machine_operation(&app, &machine_id, "discarding")?;
    let container_name = paths.container_name.clone();
    let scope = guard.scope();
    let removal = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::remove(&container_name).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result.map(|_| ()));
    if let Err(error) = removal {
        drop(guard);
        return Err(error);
    }
    remove_file_if_present(&paths.known_hosts())?;
    remove_signing_provisioning_record(&paths)?;
    remove_file_if_present(&paths.apple_device_run_record())?;
    // The disk is the macOS installation the person just agreed to discard.
    paths.remove_container_storage()?;
    clear_usb_attach_issue(&app, &machine_id);
    drop(guard);
    tray::refresh(&app);

    build_mac_builder_view(&app, &paths).await
}

/// Installs the udev rule that stops usbmuxd from claiming iPhones on this host, through one
/// authorization prompt. Host-level, so it holds no machine.
#[tauri::command]
async fn install_usb_release_rule(
    app: AppHandle,
) -> Result<buildbridge_docker_osx::HostUsbStatus, String> {
    let guard = begin_host_usb_operation(&app)?;
    let staging = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?
        .join("usb");
    let installed = tauri::async_runtime::spawn_blocking(move || {
        buildbridge_docker_osx::install_iphone_udev_rule(&staging)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?;
    drop(guard);

    installed
}

#[tauri::command]
async fn remove_usb_release_rule(
    app: AppHandle,
) -> Result<buildbridge_docker_osx::HostUsbStatus, String> {
    let guard = begin_host_usb_operation(&app)?;
    let removed = tauri::async_runtime::spawn_blocking(|| {
        buildbridge_docker_osx::remove_iphone_udev_rule().map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?;
    drop(guard);

    removed
}

/// Moves a container's macOS disk onto this host and recreates the container with the disk
/// bound in, the control socket, and USB access. Nothing on the disk changes; the signing
/// record is rebound to the new container because the keychain it describes moved with it.
#[tauri::command]
async fn migrate_machine_for_usb(
    app: AppHandle,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MacBuilderView, String> {
    if !input.confirmed {
        return Err("Confirm the container migration before continuing.".to_string());
    }
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let guard = begin_machine_operation(&app, &machine_id, "migrating_usb")?;
    let identity_path = paths.identity();
    let disk_dir = paths.disk_dir();
    let qmp_dir = paths.qmp_dir();
    let container_name = paths.container_name.clone();
    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        let options = buildbridge_docker_osx::LaunchOptions {
            identity_path: &identity_path,
            disk_dir: &disk_dir,
            qmp_dir: &qmp_dir,
            usb: buildbridge_docker_osx::resolve_usb_options(),
        };
        buildbridge_docker_osx::migrate_disk_to_host(
            &container_name,
            &profile,
            &options,
            |progress| {
                emit_machine_progress(
                    &event_app,
                    USB_MIGRATION_PROGRESS_EVENT,
                    &event_machine_id,
                    progress,
                );
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let runtime = finish_operation(&cancel_probe, joined)?;
    if let (Some(container_id), Some(mut stored)) =
        (runtime.container_id, load_signing_provisioning(&paths)?)
    {
        stored.container_id = container_id;
        save_signing_provisioning(&paths, &stored)?;
    }
    tray::refresh(&app);

    build_mac_builder_view(&app, &paths).await
}

/// Recreates the container from the machine's current profile so it picks up an option it was
/// created without — the phone's USB controller — with the disk, identity and signing kept.
/// macOS restarts once, which is the whole cost.
#[tauri::command]
async fn rebuild_machine_container(
    app: AppHandle,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MacBuilderView, String> {
    if !input.confirmed {
        return Err("Confirm the machine restart before continuing.".to_string());
    }
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let guard = begin_machine_operation(&app, &machine_id, "rebuilding_container")?;
    let identity_path = paths.identity();
    let disk_dir = paths.disk_dir();
    let qmp_dir = paths.qmp_dir();
    let container_name = paths.container_name.clone();
    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        let options = buildbridge_docker_osx::LaunchOptions {
            identity_path: &identity_path,
            disk_dir: &disk_dir,
            qmp_dir: &qmp_dir,
            usb: buildbridge_docker_osx::resolve_usb_options(),
        };
        buildbridge_docker_osx::rebuild_container(&container_name, &profile, &options, |progress| {
            emit_machine_progress(
                &event_app,
                CONTAINER_REBUILD_PROGRESS_EVENT,
                &event_machine_id,
                progress,
            );
        })
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let runtime = finish_operation(&cancel_probe, joined)?;
    // The container is new, so the keychain record has to point at it or signing reads as lost.
    if let (Some(container_id), Some(mut stored)) =
        (runtime.container_id, load_signing_provisioning(&paths)?)
    {
        stored.container_id = container_id;
        save_signing_provisioning(&paths, &stored)?;
    }
    clear_usb_attach_issue(&app, &machine_id);
    tray::refresh(&app);

    build_mac_builder_view(&app, &paths).await
}

/// Hands one host port to the running guest. The phone leaves this host until detached, or
/// until the machine stops.
#[tauri::command]
async fn attach_usb_device(
    app: AppHandle,
    machine_id: String,
    input: AttachUsbDeviceInput,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    if !buildbridge_docker_osx::valid_usb_port_path(&input.port) {
        return Err("The USB port is not valid.".to_string());
    }
    let current = build_mac_builder_view(&app, &paths).await?;
    let guest_reset = buildbridge_docker_osx::guest_reset_for_macos(
        current.guest.diagnostics.macos_version.as_deref(),
    );
    let guard = begin_machine_operation(&app, &machine_id, "attaching_usb")?;
    let container_name = paths.container_name.clone();
    let socket = paths.qmp_socket();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        let runtime =
            buildbridge_docker_osx::status(&container_name).map_err(|error| error.to_string())?;
        if runtime.state != ContainerState::Running {
            return Err("Start the machine before attaching a phone.".to_string());
        }
        let device = buildbridge_docker_osx::host_usb_status(None)
            .devices
            .into_iter()
            .find(|device| device.bus == input.bus && device.port == input.port)
            .ok_or_else(|| {
                "No Apple device is plugged into that port. Plug the phone in and refresh."
                    .to_string()
            })?;
        buildbridge_docker_osx::attach_usb_device(&socket, &container_name, &device, guest_reset)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let attached = finish_operation(&cancel_probe, joined)?;
    match attached.issue {
        Some(issue) if !attached.enumerated => {
            if let Ok(mut issues) = app.state::<AppState>().usb_attach_issues.lock() {
                issues.insert(machine_id.clone(), issue);
            }
        }
        _ => clear_usb_attach_issue(&app, &machine_id),
    }
    if attached.enumerated {
        settle_attached_phone(
            &app,
            &machine_id,
            &paths,
            profile.ssh_port,
            &access.username,
        )
        .await?;
    }

    build_mac_builder_view(&app, &paths).await
}

/// After QEMU holds the phone: wait for macOS to register it, then pair with it, which raises
/// the Trust prompt on the phone. Attaching is one click from the person's side; the phases are
/// reported so the wait reads as a wait.
async fn settle_attached_phone(
    app: &AppHandle,
    machine_id: &str,
    paths: &MachinePaths,
    ssh_port: u16,
    username: &str,
) -> Result<(), String> {
    let guard = begin_machine_operation(app, machine_id, "settling_phone")?;
    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();
    let event_app = app.clone();
    let event_machine_id = machine_id.to_string();
    let username = username.to_string();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        let started = std::time::Instant::now();
        let report = |phase: &str, detail: &str| {
            emit_machine_progress(
                &event_app,
                USB_ATTACH_PROGRESS_EVENT,
                &event_machine_id,
                serde_json::json!({
                    "phase": phase,
                    "elapsedSeconds": started.elapsed().as_secs(),
                    "detail": detail,
                }),
            );
        };
        report(
            "waiting_for_macos",
            "macOS is enumerating the phone; this takes up to a minute",
        );
        let mut devices = Vec::new();
        while started.elapsed() < std::time::Duration::from_secs(90) {
            devices = buildbridge_docker_osx::list_guest_devices(
                ssh_port,
                &username,
                &identity_path,
                &known_hosts_path,
            )
            .unwrap_or_default();
            if devices
                .iter()
                .any(|device| device.transport_type == buildbridge_docker_osx::TransportType::Wired)
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
        let wired = devices
            .iter()
            .find(|device| device.transport_type == buildbridge_docker_osx::TransportType::Wired);
        match wired {
            Some(device)
                if device.pairing_state != buildbridge_docker_osx::PairingState::Paired =>
            {
                if let Some(udid) = &device.udid {
                    report(
                        "pairing",
                        "Unlock the phone and tap Trust when it asks about this computer",
                    );
                    if let Ok(paired) = buildbridge_docker_osx::pair_guest_device(
                        ssh_port,
                        &username,
                        &identity_path,
                        &known_hosts_path,
                        udid,
                    ) {
                        devices = paired;
                    }
                }
            }
            _ => {}
        }
        report("completed", "Done");
        Ok::<_, String>(devices)
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let devices = finish_operation(&cancel_probe, joined)?;
    if let Ok(mut cache) = app.state::<AppState>().guest_devices.lock() {
        cache.insert(machine_id.to_string(), devices);
    }

    Ok(())
}

#[tauri::command]
async fn detach_usb_device(app: AppHandle, machine_id: String) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    machines::load_registry(&app)?.find(&machine_id)?;
    let guard = begin_machine_operation(&app, &machine_id, "detaching_usb")?;
    let socket = paths.qmp_socket();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::detach_usb_device(&socket).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    finish_operation(&cancel_probe, joined)?;
    clear_usb_attach_issue(&app, &machine_id);

    build_mac_builder_view(&app, &paths).await
}

fn clear_usb_attach_issue(app: &AppHandle, machine_id: &str) {
    if let Ok(mut issues) = app.state::<AppState>().usb_attach_issues.lock() {
        issues.remove(machine_id);
    }
}

/// Asks the guest which phones it sees and serves the answer from the view until the next
/// listing. Holds the machine so the probe cannot interleave with a build.
/// Pairs the guest with the phone. The command raises Trust on the phone if needed and waits
/// for the answer, so this is what the trust rung's button does.
#[tauri::command]
async fn pair_guest_device(
    app: AppHandle,
    machine_id: String,
    input: PairGuestDeviceInput,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    if !buildbridge_docker_osx::valid_device_udid(&input.udid) {
        return Err("The device identifier is not a UDID.".to_string());
    }
    let current = build_mac_builder_view(&app, &paths).await?;
    ensure_apple_project_guest_ready(&current)?;
    let guard = begin_machine_operation(&app, &machine_id, "pairing_device")?;
    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::pair_guest_device(
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            &input.udid,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let devices = finish_operation(&cancel_probe, joined)?;
    if let Ok(mut cache) = app.state::<AppState>().guest_devices.lock() {
        cache.insert(machine_id.clone(), devices);
    }

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
async fn list_guest_devices(app: AppHandle, machine_id: String) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let current = build_mac_builder_view(&app, &paths).await?;
    ensure_apple_project_guest_ready(&current)?;
    let guard = begin_machine_operation(&app, &machine_id, "listing_devices")?;
    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::list_guest_devices(
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let devices = finish_operation(&cancel_probe, joined)?;
    if let Ok(mut cache) = app.state::<AppState>().guest_devices.lock() {
        cache.insert(machine_id.clone(), devices);
    }

    build_mac_builder_view(&app, &paths).await
}

/// Opens the webview's own inspector — console, network, elements — which a development build
/// of Tauri carries. A release build does not, and says so rather than doing nothing.
#[tauri::command]
fn open_developer_tools(window: tauri::WebviewWindow) -> Result<(), String> {
    #[cfg(debug_assertions)]
    {
        window.open_devtools();
        Ok(())
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = window;
        Err("The web inspector is only built into development builds of BuildBridge.".to_string())
    }
}

#[tauri::command]
async fn get_mac_builder_status(
    app: AppHandle,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
async fn configure_mac_builder(
    app: AppHandle,
    machine_id: String,
    profile: MacBuilderConfig,
) -> Result<MacBuilderView, String> {
    profile.validate().map_err(|error| error.to_string())?;
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let mut registry = machines::load_registry(&app)?;
    let index = registry.position(&machine_id)?;
    registry.ensure_unique_ssh_port(&profile, Some(&machine_id))?;
    ensure_mac_builder_profile_can_change(&paths, &registry.machines[index].config, &profile)
        .await?;
    registry.machines[index].config = profile;
    machines::save_registry(&app, &registry)?;
    tray::refresh(&app);

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
async fn launch_mac_builder(app: AppHandle, machine_id: String) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let guard = begin_machine_operation(&app, &machine_id, "starting")?;
    let identity_path = paths.identity();
    let disk_dir = paths.disk_dir();
    let qmp_dir = paths.qmp_dir();
    let container_name = paths.container_name.clone();
    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        let options = buildbridge_docker_osx::LaunchOptions {
            identity_path: &identity_path,
            disk_dir: &disk_dir,
            qmp_dir: &qmp_dir,
            usb: buildbridge_docker_osx::resolve_usb_options(),
        };
        buildbridge_docker_osx::launch(&container_name, &profile, &options, |progress| {
            emit_machine_progress(
                &event_app,
                LAUNCH_PROGRESS_EVENT,
                &event_machine_id,
                progress,
            );
        })
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    finish_operation(&cancel_probe, joined)?;
    tray::refresh(&app);

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
async fn stop_mac_builder(app: AppHandle, machine_id: String) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    machines::load_registry(&app)?.find(&machine_id)?;
    let guard = begin_machine_operation(&app, &machine_id, "stopping")?;
    let container_name = paths.container_name.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::stop(&container_name).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    finish_operation(&cancel_probe, joined)?;
    tray::refresh(&app);

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
async fn save_signing_kit(
    app: AppHandle,
    input: SigningKitInput,
) -> Result<Vec<SigningKitSummary>, String> {
    let requested_id = input.kit_id.clone();
    let incoming = normalize_signing_kit(input)?;
    let mut kits = read_signing_kits().await?;

    match requested_id {
        Some(id) => {
            let existing = kits
                .kits
                .iter()
                .find(|kit| kit.id == id)
                .cloned()
                .ok_or_else(|| "This signing kit is no longer stored.".to_string())?;
            let merged = merge_signing_kit(existing, incoming);
            if let Some(stored) = kits.kits.iter_mut().find(|kit| kit.id == id) {
                *stored = merged;
            }
        }
        None => {
            if kits.kits.len() >= MAX_SIGNING_KITS {
                return Err(format!(
                    "BuildBridge stores at most {MAX_SIGNING_KITS} signing kits."
                ));
            }
            let existing_ids = kits
                .kits
                .iter()
                .map(|kit| kit.id.as_str())
                .collect::<Vec<_>>();
            let created = StoredSigningKit {
                id: machines::machine_id_from_name(&incoming.name, &existing_ids),
                created_at_epoch_seconds: machines::now_epoch_seconds(),
                ..incoming
            };
            kits.kits.push(created);
        }
    }

    write_signing_kits(kits).await?;

    list_signing_kits(app).await
}

#[tauri::command]
async fn list_signing_kits(app: AppHandle) -> Result<Vec<SigningKitSummary>, String> {
    let kits = read_signing_kits().await?.kits;
    let registry = machines::load_registry(&app)?;

    Ok(kits
        .iter()
        .map(|kit| {
            let mut summary = summarize_signing_kit(kit);
            summary.attached_machines = registry
                .machines
                .iter()
                .filter(|machine| machine.signing_kit_id.as_deref() == Some(kit.id.as_str()))
                .map(|machine| machine.config.name.clone())
                .collect();

            summary
        })
        .collect())
}

#[tauri::command]
async fn delete_signing_kit(
    app: AppHandle,
    kit_id: String,
    input: ConfirmInput,
) -> Result<Vec<SigningKitSummary>, String> {
    if !input.confirmed {
        return Err("Confirm removing the signing kit before continuing.".to_string());
    }
    let mut kits = read_signing_kits().await?;
    let index = kits
        .kits
        .iter()
        .position(|kit| kit.id == kit_id)
        .ok_or_else(|| "This signing kit is no longer stored.".to_string())?;
    let removed = kits.kits.remove(index);
    write_signing_kits(kits).await?;
    remove_managed_profiles_for(&app, &removed)?;
    remove_managed_certificate_for(&app, &removed)?;

    let mut registry = machines::load_registry(&app)?;
    let mut detached = false;
    for machine in &mut registry.machines {
        if machine.signing_kit_id.as_deref() == Some(kit_id.as_str()) {
            machine.signing_kit_id = None;
            detached = true;
        }
    }
    if detached {
        machines::save_registry(&app, &registry)?;
    }

    list_signing_kits(app).await
}

#[tauri::command]
async fn attach_signing_kit(
    app: AppHandle,
    machine_id: String,
    input: AttachSigningKitInput,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    if let Some(id) = input.kit_id.as_deref() {
        let stored = read_signing_kits().await?;
        if !stored.kits.iter().any(|kit| kit.id == id) {
            return Err("This signing kit is no longer stored.".to_string());
        }
    }
    let mut registry = machines::load_registry(&app)?;
    let index = registry.position(&machine_id)?;
    registry.machines[index].signing_kit_id = input.kit_id;
    machines::save_registry(&app, &registry)?;

    build_mac_builder_view(&app, &paths).await
}

/// Creates an Apple Distribution identity for a kit without a Mac anywhere: the private key is
/// generated on this host, Apple signs a CSR for it through the kit's Team key, and the result is
/// packaged as a `.p12` straight into the kit. Nothing at Apple is revoked or replaced.
#[tauri::command]
async fn create_apple_distribution_certificate(
    app: AppHandle,
    kit_id: String,
    input: CreateAppleCertificateInput,
) -> Result<CreateAppleCertificateResult, String> {
    create_apple_certificate_command(app, kit_id, input, apple_api::CertificateKind::Distribution)
        .await
}

/// The development counterpart: the identity a Debug build on a registered phone is signed
/// with. It joins the kit next to the distribution one and never replaces it.
#[tauri::command]
async fn create_apple_development_certificate(
    app: AppHandle,
    kit_id: String,
    input: CreateAppleCertificateInput,
) -> Result<CreateAppleCertificateResult, String> {
    create_apple_certificate_command(app, kit_id, input, apple_api::CertificateKind::Development)
        .await
}

async fn create_apple_certificate_command(
    app: AppHandle,
    kit_id: String,
    input: CreateAppleCertificateInput,
    kind: apple_api::CertificateKind,
) -> Result<CreateAppleCertificateResult, String> {
    if !input.confirmed {
        return Err("Confirm the Apple certificate creation before continuing.".to_string());
    }
    let kits = read_signing_kits().await?.kits;
    let mut kit = kits
        .iter()
        .find(|kit| kit.id == kit_id)
        .cloned()
        .ok_or_else(|| "This signing kit is no longer stored.".to_string())?;
    let (certificate, saved_path) = create_apple_certificate_for_kit(&app, &mut kit, kind)
        .await
        .map_err(|error| {
            with_other_kit_hint(
                error,
                kind,
                &kits_holding_distribution_identity(&kits, &kit_id),
            )
        })?;

    Ok(CreateAppleCertificateResult {
        certificate,
        saved_path,
        kit: summarize_signing_kit(&kit),
    })
}

/// The other kits on this host that already hold a distribution identity. When Apple refuses a
/// second distribution certificate, the one it counts is usually in one of these.
fn kits_holding_distribution_identity(kits: &[StoredSigningKit], except_id: &str) -> Vec<String> {
    kits.iter()
        .filter(|kit| kit.id != except_id && kit.signing_certificate_path.is_some())
        .map(|kit| kit.name.clone())
        .collect()
}

/// Apple's refusal says to revoke or export; if another kit here already holds the identity
/// Apple is counting, the right move is to attach that kit instead, so the message says so.
fn with_other_kit_hint(
    error: String,
    kind: apple_api::CertificateKind,
    holders: &[String],
) -> String {
    if kind != apple_api::CertificateKind::Distribution
        || holders.is_empty()
        || !error.contains("refused to issue another distribution certificate")
    {
        return error;
    }
    let holders = holders.join(", ");
    let verb = if holders.contains(", ") {
        "already hold"
    } else {
        "already holds"
    };
    format!(
        "{error} On this host, {holders} {verb} a distribution identity for this team; attach that kit to the machine instead of creating a second certificate."
    )
}

/// Creates an identity of one kind for a kit without a Mac anywhere: the private key is
/// generated on this host, Apple signs a CSR for it through the kit's Team key, and the result
/// is packaged as a `.p12` straight into the kit. Nothing at Apple is revoked or replaced.
async fn create_apple_certificate_for_kit(
    app: &AppHandle,
    kit: &mut StoredSigningKit,
    kind: apple_api::CertificateKind,
) -> Result<(apple_api::AppleCertificateSummary, String), String> {
    let key_id = kit.app_store_connect_key_id.clone().ok_or_else(|| {
        "This kit has no App Store Connect key. Add one to the kit first; creating a certificate needs a Team key with the Admin role."
            .to_string()
    })?;
    let issuer_id = kit
        .app_store_connect_issuer_id
        .clone()
        .ok_or_else(|| "This kit has no App Store Connect Issuer ID.".to_string())?;
    let private_key = kit
        .app_store_connect_private_key
        .clone()
        .ok_or_else(|| "This kit has no App Store Connect .p8 key.".to_string())?;

    let directory = managed_apple_certificates_dir(app)?.join(format!(
        "{}-{}",
        kind.file_stem(),
        machines::now_epoch_seconds()
    ));
    let work = tauri::async_runtime::spawn_blocking({
        let directory = directory.clone();
        move || prepare_certificate_request(&directory, kind.common_name())
    })
    .await
    .map_err(|error| error.to_string())??;

    let created =
        match apple_api::create_certificate(&key_id, &issuer_id, &private_key, &work.csr_pem, kind)
            .await
        {
            Ok(created) => created,
            Err(error) => {
                let _ = fs::remove_dir_all(&directory);
                return Err(error);
            }
        };

    let packaged = tauri::async_runtime::spawn_blocking({
        let directory = directory.clone();
        let display_name = created.certificate.name.clone();
        let content = created.content.clone();
        move || package_certificate(&directory, &display_name, &content, kind.file_stem())
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result)
    .map_err(|error| {
        let _ = fs::remove_dir_all(&directory);
        format!(
            "Apple issued certificate {}, but BuildBridge could not package it: {error}. Nothing at Apple was revoked; download it from the developer portal, or revoke it there and try again.",
            created.certificate.name
        )
    })?;

    match kind {
        apple_api::CertificateKind::Distribution => {
            kit.signing_certificate_path = Some(packaged.p12_path.clone());
            kit.signing_certificate_password = Some(packaged.password);
        }
        apple_api::CertificateKind::Development => {
            kit.development_certificate_path = Some(packaged.p12_path.clone());
            kit.development_certificate_password = Some(packaged.password);
            kit.development_certificate_serial_number =
                Some(created.certificate.serial_number.clone());
        }
    }
    save_signing_kit_record(kit.clone()).await.map_err(|error| {
        format!(
            "Apple issued certificate {} and it was packaged at {}, but BuildBridge could not update the kit in the OS vault: {error}. Nothing at Apple was revoked.",
            created.certificate.name, packaged.p12_path
        )
    })?;

    Ok((created.certificate, packaged.p12_path))
}

/// Apple's serial number for the kit's development `.p12`: recorded when BuildBridge created
/// it, read out of the file with OpenSSL otherwise. The passphrase goes through the environment.
fn development_certificate_serial(kit: &StoredSigningKit) -> Result<String, String> {
    if let Some(serial) = &kit.development_certificate_serial_number {
        return Ok(serial.clone());
    }
    let path = kit
        .development_certificate_path
        .as_deref()
        .ok_or_else(|| "This kit has no development certificate.".to_string())?;
    let password = kit
        .development_certificate_password
        .as_deref()
        .ok_or_else(|| {
            "Store the development certificate passphrase in the operating-system vault first."
                .to_string()
        })?;
    let env = Some(("BUILDBRIDGE_P12_PASSWORD", password));
    let pem = openssl(
        &[
            "pkcs12",
            "-in",
            path,
            "-nokeys",
            "-clcerts",
            "-passin",
            "env:BUILDBRIDGE_P12_PASSWORD",
        ],
        None,
        env,
    )
    .or_else(|_| {
        openssl(
            &[
                "pkcs12",
                "-legacy",
                "-in",
                path,
                "-nokeys",
                "-clcerts",
                "-passin",
                "env:BUILDBRIDGE_P12_PASSWORD",
            ],
            None,
            env,
        )
    })?;
    let serial = openssl(&["x509", "-noout", "-serial"], Some(&pem), None)?;
    let serial = String::from_utf8_lossy(&serial);
    let serial = serial.trim();
    let serial = serial
        .strip_prefix("serial=")
        .unwrap_or(serial)
        .trim()
        .to_ascii_uppercase();
    if serial.is_empty()
        || !serial
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(
            "OpenSSL could not read the development certificate's serial number.".to_string(),
        );
    }

    Ok(serial)
}

/// Whether the kit already holds a copy of the profile with this UUID, by managed file name.
fn kit_holds_profile_uuid(kit: &StoredSigningKit, uuid: &str) -> bool {
    kit.provisioning_profile_paths.iter().any(|path| {
        std::path::Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.to_ascii_uppercase()
                    .starts_with(&uuid.to_ascii_uppercase())
            })
    })
}

/// Where identities created here are kept: the `.p12` and Apple's `.cer`, owner-only.
fn managed_apple_certificates_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("macos-builder").join("certificates"))
        .map_err(|error| error.to_string())
}

struct CertificateRequestFiles {
    csr_pem: String,
}

struct PackagedCertificate {
    p12_path: String,
    password: String,
}

/// Generates the private key and CSR on this host with fixed-argv OpenSSL. The key is written
/// owner-only from OpenSSL's standard output rather than by OpenSSL itself, so it never exists
/// with looser permissions even for an instant.
fn prepare_certificate_request(
    directory: &std::path::Path,
    common_name: &str,
) -> Result<CertificateRequestFiles, String> {
    openssl(&["version"], None, None).map_err(|_| {
        "OpenSSL is not installed on this host. Install the openssl package and try again."
            .to_string()
    })?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create the certificate directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Could not protect the certificate directory: {error}"))?;
    }

    let key_pem = openssl(&certificate_key_args(), None, None)?;
    let key_path = directory.join("key.pem");
    write_restricted_file(&key_path, &key_pem)?;

    let csr_pem = openssl(
        &certificate_csr_args(&key_path.to_string_lossy(), common_name),
        None,
        None,
    )?;
    let csr_pem = String::from_utf8(csr_pem)
        .map_err(|_| "OpenSSL produced an unreadable certificate request.".to_string())?;
    if !apple_api::valid_csr_pem(&csr_pem) {
        return Err("OpenSSL produced an unreadable certificate request.".to_string());
    }

    Ok(CertificateRequestFiles { csr_pem })
}

/// Packages Apple's certificate with the host key as a password-protected `.p12`, then removes
/// the loose key and PEM. The password is generated here and handed to OpenSSL through the
/// environment, never as an argument.
fn package_certificate(
    directory: &std::path::Path,
    display_name: &str,
    certificate_der: &[u8],
    file_stem: &str,
) -> Result<PackagedCertificate, String> {
    let key_path = directory.join("key.pem");
    let cer_path = directory.join("certificate.cer");
    let pem_path = directory.join("certificate.pem");
    let p12_path = directory.join(format!("{file_stem}.p12"));
    write_restricted_file(&cer_path, certificate_der)?;

    let certificate_pem = openssl(
        &["x509", "-inform", "DER", "-in", &cer_path.to_string_lossy()],
        None,
        None,
    )?;
    write_restricted_file(&pem_path, &certificate_pem)?;

    let password = String::from_utf8(openssl(&["rand", "-base64", "24"], None, None)?)
        .map_err(|_| "OpenSSL produced an unreadable password.".to_string())?
        .trim()
        .to_string();
    if password.len() < 24 {
        return Err("OpenSSL produced an unusable password.".to_string());
    }

    let p12 = openssl(
        &certificate_p12_args(
            &key_path.to_string_lossy(),
            &pem_path.to_string_lossy(),
            display_name,
        ),
        None,
        Some(("BUILDBRIDGE_P12_PASSWORD", &password)),
    )?;
    if p12.is_empty() {
        return Err("OpenSSL produced an empty .p12.".to_string());
    }
    write_restricted_file(&p12_path, &p12)?;
    remove_file_if_present(&key_path)?;
    remove_file_if_present(&pem_path)?;

    Ok(PackagedCertificate {
        p12_path: p12_path.to_string_lossy().to_string(),
        password,
    })
}

fn certificate_key_args() -> Vec<String> {
    [
        "genpkey",
        "-algorithm",
        "RSA",
        "-pkeyopt",
        "rsa_keygen_bits:2048",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn certificate_csr_args(key_path: &str, common_name: &str) -> Vec<String> {
    let subject = format!("/CN={common_name}");
    ["req", "-new", "-batch", "-key", key_path, "-subj", &subject]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The algorithms macOS's Security framework can read. OpenSSL 3 writes PBES2 with AES and a
/// SHA-256 MAC by default, and `SecPKCS12Import` on macOS 26 answers that with
/// `errSecPkcs12VerifyFailure` (-25264, "MAC verification failed"), which reads like a wrong
/// password and is not. SHA-1 for the MAC and 3DES for the bags is what Keychain Access itself
/// exports, imports on every macOS, and needs no legacy provider on the host.
const MACOS_PKCS12_ALGORITHMS: [&str; 6] = [
    "-keypbe",
    "PBE-SHA1-3DES",
    "-certpbe",
    "PBE-SHA1-3DES",
    "-macalg",
    "sha1",
];

fn certificate_p12_args(key_path: &str, certificate_pem: &str, display_name: &str) -> Vec<String> {
    [
        "pkcs12",
        "-export",
        "-inkey",
        key_path,
        "-in",
        certificate_pem,
        "-name",
        display_name,
        "-passout",
        "env:BUILDBRIDGE_P12_PASSWORD",
    ]
    .into_iter()
    .chain(MACOS_PKCS12_ALGORITHMS)
    .map(str::to_string)
    .collect()
}

/// Whether a `.p12`, as `openssl pkcs12 -info` describes it, was written with algorithms macOS
/// cannot read: a SHA-256 MAC or PBES2 bags.
fn pkcs12_needs_repackaging(info: &str) -> bool {
    info.lines().any(|line| {
        let line = line.trim();
        (line.starts_with("MAC:") && !line.contains("sha1")) || line.contains("PBES2")
    })
}

/// A kit file BuildBridge packaged before it knew what macOS reads is rewritten in place with
/// the same key, certificate and passphrase, so nothing at Apple is touched and the kit keeps
/// its path. The key crosses only a pipe between two OpenSSL processes; the passphrase rides the
/// environment, never an argument. Files macOS can already read — every Mac export — are left
/// exactly as they are.
fn ensure_macos_importable_pkcs12(path: &str, password: &str) -> Result<String, String> {
    let env = Some(("BUILDBRIDGE_P12_PASSWORD", password));
    // `pkcs12 -info` writes its report to stderr, so it is read from there. A file OpenSSL
    // cannot describe at all is left for the guest import to name precisely.
    let info = openssl_report(
        &[
            "pkcs12",
            "-info",
            "-noout",
            "-in",
            path,
            "-passin",
            "env:BUILDBRIDGE_P12_PASSWORD",
        ],
        env,
    )
    .unwrap_or_default();
    if !pkcs12_needs_repackaging(&info) {
        return Ok(path.to_string());
    }
    let pem = openssl(
        &[
            "pkcs12",
            "-in",
            path,
            "-nodes",
            "-passin",
            "env:BUILDBRIDGE_P12_PASSWORD",
        ],
        None,
        env,
    )?;
    // `pkcs12 -export` reads its input twice, once for certificates and once for the key, so
    // a pipe cannot serve it. The unencrypted PEM exists for the length of that one command,
    // owner-only, beside the file it came from in the kit's owner-only directory.
    let pem_path = format!("{path}.repack.pem");
    write_owner_only(&pem_path, &pem)?;
    let display_name = std::path::Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("identity")
        .to_string();
    let repacked = format!("{path}.repacked");
    let args: Vec<String> = [
        "pkcs12",
        "-export",
        "-in",
        pem_path.as_str(),
        "-name",
        display_name.as_str(),
        "-passout",
        "env:BUILDBRIDGE_P12_PASSWORD",
        "-out",
        repacked.as_str(),
    ]
    .into_iter()
    .chain(MACOS_PKCS12_ALGORITHMS)
    .map(str::to_string)
    .collect();
    let exported = openssl(&args, None, env);
    let _ = fs::remove_file(&pem_path);
    exported.map_err(|error| {
        let _ = fs::remove_file(&repacked);
        format!("The identity at {path} could not be repackaged for macOS: {error}")
    })?;
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&repacked, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("The repackaged identity could not be protected: {error}"))?;
    }
    fs::rename(&repacked, path).map_err(|error| {
        let _ = fs::remove_file(&repacked);
        format!("The repackaged identity could not replace {path}: {error}")
    })?;

    Ok(path.to_string())
}

/// Runs OpenSSL for what it says rather than what it outputs: `pkcs12 -info` reports on
/// stderr. Both streams, as text, on success.
fn openssl_report(args: &[impl AsRef<str>], env: Option<(&str, &str)>) -> Result<String, String> {
    let mut command = Command::new("openssl");
    command.args(args.iter().map(AsRef::as_ref));
    command.stdin(std::process::Stdio::null());
    if let Some((name, value)) = env {
        command.env(name, value);
    }
    let output = command
        .output()
        .map_err(|error| format!("Could not run openssl: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));

    Ok(text)
}

/// A file only this user can read, from the first byte: created with the mode, not chmodded
/// after the write.
fn write_owner_only(path: &str, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("Could not create {path}: {error}"))?;
    file.write_all(bytes)
        .map_err(|error| format!("Could not write {path}: {error}"))
}

fn openssl(
    args: &[impl AsRef<str>],
    stdin: Option<&[u8]>,
    env: Option<(&str, &str)>,
) -> Result<Vec<u8>, String> {
    let mut command = Command::new("openssl");
    command.args(args.iter().map(AsRef::as_ref));
    command.stdin(if stdin.is_some() {
        std::process::Stdio::piped()
    } else {
        std::process::Stdio::null()
    });
    if let Some((name, value)) = env {
        command.env(name, value);
    }
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not run openssl: {error}"))?;
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        use std::io::Write;
        pipe.write_all(bytes)
            .map_err(|error| format!("Could not feed openssl: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("Could not finish openssl: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "openssl {} failed: {}",
            args.first().map(AsRef::as_ref).unwrap_or("command"),
            stderr.trim().lines().last().unwrap_or("no output")
        ));
    }

    Ok(output.stdout)
}

/// One optimization as the interface shows it: the catalogue entry plus whether the guest
/// already has it, when the guest can be asked.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GuestOptimizationView {
    #[serde(flatten)]
    optimization: &'static GuestOptimization,
    /// `None` when the guest cannot be asked right now, or the check could not tell.
    applied: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GuestOptimizationsView {
    /// Whether the guest is reachable enough to check or apply anything.
    available: bool,
    reason: Option<String>,
    items: Vec<GuestOptimizationView>,
}

/// Lists the catalogue with each item's current state on this machine's guest.
#[tauri::command]
async fn list_guest_optimizations(
    app: AppHandle,
    machine_id: String,
) -> Result<GuestOptimizationsView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let view = build_mac_builder_view(&app, &paths).await?;
    let guest_ready = view.runtime.state == ContainerState::Running
        && view.guest.ssh.trust == GuestTrustState::Trusted
        && view.guest.diagnostics.authenticated;
    let catalogue = buildbridge_docker_osx::guest_optimizations();
    if !guest_ready {
        return Ok(GuestOptimizationsView {
            available: false,
            reason: Some(
                "Start the machine, pin its identity and authorize the access key first."
                    .to_string(),
            ),
            items: catalogue
                .iter()
                .map(|optimization| GuestOptimizationView {
                    optimization,
                    applied: None,
                })
                .collect(),
        });
    }

    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let ssh_port = view.profile.ssh_port;
    let identity = paths.guest_identity();
    let known_hosts = paths.known_hosts();
    let states = tauri::async_runtime::spawn_blocking(move || {
        buildbridge_docker_osx::check_guest_optimizations(
            ssh_port,
            &access.username,
            &identity,
            &known_hosts,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;

    Ok(GuestOptimizationsView {
        available: true,
        reason: None,
        items: catalogue
            .iter()
            .map(|optimization| GuestOptimizationView {
                optimization,
                applied: states
                    .iter()
                    .find(|(id, _)| *id == optimization.id)
                    .and_then(|(_, applied)| *applied),
            })
            .collect(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyOptimizationInput {
    optimization_id: String,
    confirmed: bool,
}

/// Applies one catalogue optimization to the guest. Admin items open the guest Terminal for
/// the password; the operation is busy — and stoppable — until it returns.
#[tauri::command]
async fn apply_guest_optimization(
    app: AppHandle,
    machine_id: String,
    input: ApplyOptimizationInput,
) -> Result<GuestOptimizationsView, String> {
    if !input.confirmed {
        return Err("Confirm the optimization before applying it.".to_string());
    }
    let optimization = buildbridge_docker_osx::guest_optimization(&input.optimization_id)
        .ok_or_else(|| "That optimization is not in the catalogue.".to_string())?;
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let view = build_mac_builder_view(&app, &paths).await?;
    if view.runtime.state != ContainerState::Running {
        return Err("Start the macOS machine first.".to_string());
    }
    if view.guest.ssh.trust != GuestTrustState::Trusted || !view.guest.diagnostics.authenticated {
        return Err("Finish the pinned macOS guest connection first.".to_string());
    }
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let guard = begin_machine_operation(&app, &machine_id, "optimizing")?;
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let ssh_port = view.profile.ssh_port;
    let identity = paths.guest_identity();
    let known_hosts = paths.known_hosts();
    let optimization_id = optimization.id.to_string();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::apply_guest_optimization(
            ssh_port,
            &access.username,
            &identity,
            &known_hosts,
            &optimization_id,
            |_elapsed| {},
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    finish_operation(&cancel_probe, joined)?;

    list_guest_optimizations(app, machine_id).await
}

#[tauri::command]
async fn list_env_sets(app: AppHandle) -> Result<Vec<EnvSetSummary>, String> {
    let sets = read_env_sets().await?.sets;
    let registry = machines::load_registry(&app)?;

    Ok(sets
        .iter()
        .map(|set| summarize_env_set(set, &registry.machines))
        .collect())
}

#[tauri::command]
async fn save_env_set(app: AppHandle, input: EnvSetInput) -> Result<Vec<EnvSetSummary>, String> {
    let name = input.name.trim().to_string();
    if name.is_empty() || name.len() > 60 {
        return Err("Give the env set a name of up to 60 characters.".to_string());
    }
    let mut sets = read_env_sets().await?;

    match input.set_id.clone() {
        Some(id) => {
            let index = sets
                .sets
                .iter()
                .position(|set| set.id == id)
                .ok_or_else(|| "This env set is no longer stored.".to_string())?;
            let variables = merge_env_variables(Some(&sets.sets[index]), &input)?;
            sets.sets[index].name = name;
            sets.sets[index].variables = variables;
        }
        None => {
            if sets.sets.len() >= MAX_ENV_SETS {
                return Err(format!(
                    "BuildBridge stores at most {MAX_ENV_SETS} env sets."
                ));
            }
            let variables = merge_env_variables(None, &input)?;
            let existing_ids = sets
                .sets
                .iter()
                .map(|set| set.id.as_str())
                .collect::<Vec<_>>();
            sets.sets.push(StoredEnvSet {
                id: machines::machine_id_from_name(&name, &existing_ids),
                name,
                variables,
                created_at_epoch_seconds: machines::now_epoch_seconds(),
            });
        }
    }

    write_env_sets(sets).await?;

    list_env_sets(app).await
}

#[tauri::command]
async fn delete_env_set(
    app: AppHandle,
    set_id: String,
    input: ConfirmInput,
) -> Result<Vec<EnvSetSummary>, String> {
    if !input.confirmed {
        return Err("Confirm removing the env set before continuing.".to_string());
    }
    let mut sets = read_env_sets().await?;
    let index = sets
        .sets
        .iter()
        .position(|set| set.id == set_id)
        .ok_or_else(|| "This env set is no longer stored.".to_string())?;
    sets.sets.remove(index);
    write_env_sets(sets).await?;

    let mut registry = machines::load_registry(&app)?;
    let mut detached = false;
    for machine in &mut registry.machines {
        if machine.env_set_id.as_deref() == Some(set_id.as_str()) {
            machine.env_set_id = None;
            detached = true;
        }
    }
    if detached {
        machines::save_registry(&app, &registry)?;
    }

    list_env_sets(app).await
}

#[tauri::command]
async fn attach_env_set(
    app: AppHandle,
    machine_id: String,
    input: AttachEnvSetInput,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    if let Some(id) = input.set_id.as_deref() {
        let stored = read_env_sets().await?;
        if !stored.sets.iter().any(|set| set.id == id) {
            return Err("This env set is no longer stored.".to_string());
        }
    }
    let mut registry = machines::load_registry(&app)?;
    let index = registry.position(&machine_id)?;
    registry.machines[index].env_set_id = input.set_id;
    machines::save_registry(&app, &registry)?;

    build_mac_builder_view(&app, &paths).await
}

/// Every secret in one set with its value, for the editor to hold masked behind an eye icon. This
/// is the only way a secret leaves the vault for the interface: by set, on request, and never
/// inside a summary.
#[tauri::command]
async fn reveal_env_secrets(set_id: String) -> Result<Vec<EnvVariableSummary>, String> {
    let stored = read_env_sets().await?;

    stored_env_secrets(&stored, &set_id)
}

#[tauri::command]
async fn verify_apple_developer_team(
    app: AppHandle,
    machine_id: String,
) -> Result<apple_api::AppleTeamVerificationResult, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let workspace = load_apple_workspace(&paths)?.ok_or_else(|| {
        "Approve an Apple project before verifying its developer team.".to_string()
    })?;
    let development_team = workspace.development_team.ok_or_else(|| {
        "BuildBridge could not detect DEVELOPMENT_TEAM in the approved project.".to_string()
    })?;
    let bundle_identifier = workspace.bundle_identifier.ok_or_else(|| {
        "BuildBridge could not detect PRODUCT_BUNDLE_IDENTIFIER in the approved project."
            .to_string()
    })?;
    let secrets = resolve_signing_kit_for(&app, &machine_id).await?;
    let key_id = secrets.app_store_connect_key_id.ok_or_else(|| {
        "The stored signing kit does not include an App Store Connect Key ID.".to_string()
    })?;
    let issuer_id = secrets.app_store_connect_issuer_id.ok_or_else(|| {
        "The stored signing kit does not include an App Store Connect Issuer ID.".to_string()
    })?;
    let private_key = secrets.app_store_connect_private_key.ok_or_else(|| {
        "The stored signing kit does not include an App Store Connect .p8 key.".to_string()
    })?;

    apple_api::verify_developer_team(
        &key_id,
        &issuer_id,
        &private_key,
        &development_team,
        &bundle_identifier,
    )
    .await
}

#[tauri::command]
async fn create_apple_replacement_profile(
    app: AppHandle,
    machine_id: String,
    input: CreateAppleProfileInput,
) -> Result<CreateAppleProfileResult, String> {
    if !input.confirmed {
        return Err(
            "Confirm the Apple provisioning-profile creation before continuing.".to_string(),
        );
    }

    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let workspace = load_apple_workspace(&paths)?.ok_or_else(|| {
        "Approve an Apple project before creating a provisioning profile.".to_string()
    })?;
    let bundle_identifier = workspace.bundle_identifier.ok_or_else(|| {
        "BuildBridge could not detect PRODUCT_BUNDLE_IDENTIFIER in the approved project."
            .to_string()
    })?;
    let mut secrets = resolve_signing_kit_for(&app, &machine_id).await?;
    if secrets.provisioning_profile_paths.len() >= MAX_PROVISIONING_PROFILES {
        return Err(format!(
            "The signing kit already retains {MAX_PROVISIONING_PROFILES} profiles. Remove obsolete local profile paths before creating another one."
        ));
    }
    let key_id = secrets.app_store_connect_key_id.as_deref().ok_or_else(|| {
        "The stored signing kit does not include an App Store Connect Key ID.".to_string()
    })?;
    let issuer_id = secrets
        .app_store_connect_issuer_id
        .as_deref()
        .ok_or_else(|| {
            "The stored signing kit does not include an App Store Connect Issuer ID.".to_string()
        })?;
    let private_key = secrets
        .app_store_connect_private_key
        .as_deref()
        .ok_or_else(|| {
            "The stored signing kit does not include an App Store Connect .p8 key.".to_string()
        })?;

    let created = apple_api::create_replacement_profile(
        key_id,
        issuer_id,
        private_key,
        &bundle_identifier,
        input.certificate_id.trim(),
    )
    .await?;
    let saved_path = save_managed_apple_profile(&app, &created.profile, &created.content).map_err(
        |error| {
            format!(
                "Apple created profile {}, but BuildBridge could not retain it locally: {error}. The Apple profile was not revoked; verify again before retrying.",
                created.profile.name
            )
        },
    )?;
    let saved_path_string = saved_path
        .to_str()
        .ok_or_else(|| {
            format!(
                "Apple created profile {}, but its managed local path is not valid UTF-8. The Apple profile was not revoked; verify again before retrying.",
                created.profile.name
            )
        })?
        .to_string();
    if !secrets
        .provisioning_profile_paths
        .iter()
        .any(|path| path == &saved_path_string)
    {
        secrets
            .provisioning_profile_paths
            .push(saved_path_string.clone());
    }
    save_signing_kit_record(secrets.clone())
        .await
        .map_err(|error| {
            format!(
                "Apple created profile {} and saved it at {}, but BuildBridge could not add it to the OS vault: {error}. The Apple profile was not revoked; verify again before retrying.",
                created.profile.name, saved_path_string
            )
        })?;
    let summary = summarize_signing_kit(&secrets);

    Ok(CreateAppleProfileResult {
        profile: created.profile,
        certificate: created.certificate,
        saved_path: saved_path_string,
        kit: summary,
    })
}

/// Downloads an existing Apple profile into this host's managed store and adds it to the kit.
///
/// Apple keeps the profile; BuildBridge only holds a copy. Losing that copy — a cleared vault, a
/// new host — should not mean hunting for the file, so an active profile can be taken back with
/// one action instead of being re-downloaded by hand.
#[tauri::command]
async fn download_apple_profile(
    app: AppHandle,
    machine_id: String,
    profile_id: String,
) -> Result<DownloadAppleProfileResult, String> {
    let mut secrets = resolve_signing_kit_for(&app, &machine_id).await?;
    if secrets.provisioning_profile_paths.len() >= MAX_PROVISIONING_PROFILES {
        return Err(format!(
            "This kit already holds {MAX_PROVISIONING_PROFILES} profiles. Remove obsolete paths before adding another."
        ));
    }
    let key_id = secrets.app_store_connect_key_id.as_deref().ok_or_else(|| {
        "This kit has no App Store Connect Key ID, so Apple cannot be asked for the profile."
            .to_string()
    })?;
    let issuer_id = secrets
        .app_store_connect_issuer_id
        .as_deref()
        .ok_or_else(|| "This kit has no App Store Connect Issuer ID.".to_string())?;
    let private_key = secrets
        .app_store_connect_private_key
        .as_deref()
        .ok_or_else(|| "This kit has no App Store Connect .p8 key.".to_string())?;

    let (profile, content) =
        apple_api::download_profile(key_id, issuer_id, private_key, profile_id.trim()).await?;
    let saved_path = save_managed_apple_profile(&app, &profile, &content)?;
    let saved_path_string = saved_path
        .to_str()
        .ok_or_else(|| "The managed profile path is not valid UTF-8.".to_string())?
        .to_string();
    if !secrets
        .provisioning_profile_paths
        .iter()
        .any(|path| path == &saved_path_string)
    {
        secrets
            .provisioning_profile_paths
            .push(saved_path_string.clone());
    }
    save_signing_kit_record(secrets.clone()).await?;

    Ok(DownloadAppleProfileResult {
        profile,
        saved_path: saved_path_string,
        kit: summarize_signing_kit(&secrets),
    })
}

#[tauri::command]
async fn configure_mac_guest_access(
    app: AppHandle,
    machine_id: String,
    input: MacGuestAccessInput,
) -> Result<MacBuilderView, String> {
    let username = input.username.trim().to_string();
    if !buildbridge_docker_osx::valid_guest_username(&username) {
        return Err(
            "Use the macOS short username: 1–32 letters, numbers, periods, underscores, or hyphens."
                .to_string(),
        );
    }

    let paths = MachinePaths::resolve(&app, &machine_id)?;
    machines::load_registry(&app)?.find(&machine_id)?;
    let identity_path = paths.guest_identity();
    tauri::async_runtime::spawn_blocking(move || ensure_mac_guest_keypair(&identity_path))
        .await
        .map_err(|error| error.to_string())??;
    save_mac_guest_access(&paths, &StoredMacGuestAccess { username })?;

    build_mac_builder_view(&app, &paths).await
}

/// Installs the BuildBridge key into the guest user's `authorized_keys` through one
/// password-authenticated SSH session on the pinned host key — the `ssh-copy-id` route. The
/// password exists in this request and in the environment of that single `ssh` process; it is
/// not stored, logged, or reused. Everything after this step signs in with the key. The
/// username and key are kept even when the password is rejected, so the Terminal route stays
/// available.
#[tauri::command]
async fn authorize_mac_guest_key(
    app: AppHandle,
    machine_id: String,
    input: AuthorizeMacGuestKeyInput,
) -> Result<MacBuilderView, String> {
    let username = input.username.trim().to_string();
    if !buildbridge_docker_osx::valid_guest_username(&username) {
        return Err(
            "Use the macOS short username: 1–32 letters, numbers, periods, underscores, or hyphens."
                .to_string(),
        );
    }
    if input.password.is_empty() {
        return Err("Enter the local macOS login password to install the key.".to_string());
    }

    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let known_hosts_path = paths.known_hosts();
    if !known_hosts_path.is_file() {
        return Err(
            "Pin the guest identity first, so the password only ever goes to the machine you verified."
                .to_string(),
        );
    }
    let identity_path = paths.guest_identity();
    let public_key =
        tauri::async_runtime::spawn_blocking(move || ensure_mac_guest_keypair(&identity_path))
            .await
            .map_err(|error| error.to_string())??;
    save_mac_guest_access(
        &paths,
        &StoredMacGuestAccess {
            username: username.clone(),
        },
    )?;

    let password = input.password;
    tauri::async_runtime::spawn_blocking(move || {
        buildbridge_docker_osx::authorize_guest_key(
            profile.ssh_port,
            &username,
            &public_key,
            &password,
            &known_hosts_path,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
async fn trust_mac_builder_guest(
    app: AppHandle,
    machine_id: String,
    input: TrustMacGuestInput,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let known_hosts_path = paths.known_hosts();

    if known_hosts_path.exists() {
        return Err(
            "A guest identity is already pinned. Forget it first if you intentionally rebuilt the machine."
                .to_string(),
        );
    }

    tauri::async_runtime::spawn_blocking(move || {
        let scanned = buildbridge_docker_osx::scan_guest_host_key(profile.ssh_port)
            .map_err(|error| error.to_string())?;
        if scanned.fingerprint != input.fingerprint {
            return Err(
                "The guest fingerprint changed before it could be trusted. Refresh and verify it again."
                    .to_string(),
            );
        }
        write_restricted_file(
            &known_hosts_path,
            format!("{}\n", scanned.known_hosts_line).as_bytes(),
        )
    })
    .await
    .map_err(|error| error.to_string())??;

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
async fn forget_mac_builder_guest_trust(
    app: AppHandle,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    remove_file_if_present(&paths.known_hosts())?;

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
async fn import_mac_xcode_package(
    app: AppHandle,
    machine_id: String,
    input: ImportMacXcodeInput,
) -> Result<ImportMacXcodeResult, String> {
    let package_path = PathBuf::from(input.path.trim());
    if input.path.trim().is_empty() {
        return Err("Drop or enter the absolute path to an Xcode .xip package.".to_string());
    }

    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let current = build_mac_builder_view(&app, &paths).await?;
    if current.runtime.state != ContainerState::Running {
        return Err("Start the macOS machine before importing Xcode.".to_string());
    }
    if current.guest.ssh.trust != GuestTrustState::Trusted
        || !current.guest.diagnostics.authenticated
    {
        return Err(
            "Trust the guest fingerprint and finish BuildBridge key authentication first."
                .to_string(),
        );
    }
    if current.guest.diagnostics.xcode_selected {
        return Err("The guest already has an active Xcode toolchain.".to_string());
    }

    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();
    let guard = begin_machine_operation(&app, &machine_id, "importing_xcode")?;
    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::import_xcode_package(
            &package_path,
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            |progress: XcodeImportProgress| {
                emit_machine_progress(
                    &event_app,
                    XCODE_PROGRESS_EVENT,
                    &event_machine_id,
                    progress,
                );
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let imported = finish_operation(&cancel_probe, joined)?;
    let view = build_mac_builder_view(&app, &paths).await?;

    Ok(ImportMacXcodeResult {
        view,
        installed_path: imported.installed_path,
        activation_commands: imported.activation_commands,
    })
}

#[tauri::command]
async fn activate_mac_xcode(
    app: AppHandle,
    machine_id: String,
    input: ActivateMacXcodeInput,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let current = build_mac_builder_view(&app, &paths).await?;
    if current.runtime.state != ContainerState::Running {
        return Err("Start the macOS machine before activating Xcode.".to_string());
    }
    if current.guest.ssh.trust != GuestTrustState::Trusted
        || !current.guest.diagnostics.authenticated
    {
        return Err(
            "Trust the guest fingerprint and finish BuildBridge key authentication first."
                .to_string(),
        );
    }
    if current.guest.diagnostics.xcode_selected {
        return Ok(current);
    }
    if current.guest.diagnostics.xcode_version.is_none() {
        return Err("Import and expand Xcode before activating it.".to_string());
    }

    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();
    let guard = begin_machine_operation(&app, &machine_id, "activating_xcode")?;
    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let password = input.password.filter(|value| !value.is_empty());
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        let mut report = |progress: XcodeImportProgress| {
            emit_machine_progress(
                &event_app,
                XCODE_PROGRESS_EVENT,
                &event_machine_id,
                progress,
            );
        };
        match password {
            Some(password) => buildbridge_docker_osx::activate_xcode_with_password(
                profile.ssh_port,
                &access.username,
                &identity_path,
                &known_hosts_path,
                &password,
                &mut report,
            ),
            None => buildbridge_docker_osx::activate_xcode(
                profile.ssh_port,
                &access.username,
                &identity_path,
                &known_hosts_path,
                &mut report,
            ),
        }
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    finish_operation(&cancel_probe, joined)?;

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
async fn provision_mac_signing(
    app: AppHandle,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    let secrets = resolve_signing_kit_for(&app, &machine_id).await?;
    provision_with_kit(&app, &machine_id, secrets).await
}

/// Provisions one kit into a machine's guest keychain: the distribution identity every kit
/// has, the development identity when the kit holds one, and every profile, replacing what
/// the previous provisioning installed.
async fn provision_with_kit(
    app: &AppHandle,
    machine_id: &str,
    secrets: StoredSigningKit,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(app, machine_id)?;
    let profile = machines::load_registry(app)?
        .find(machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let workspace = load_apple_workspace(&paths)?
        .ok_or_else(|| "Approve and verify an Apple project first.".to_string())?;
    if !workspace.last_build_succeeded {
        return Err(
            "Complete the unsigned project test build before provisioning signing.".to_string(),
        );
    }
    let development_team = workspace.development_team.clone().ok_or_else(|| {
        "BuildBridge could not detect one development team. Re-approve the project after setting DEVELOPMENT_TEAM in Xcode."
            .to_string()
    })?;
    let bundle_identifier = workspace.bundle_identifier.clone().ok_or_else(|| {
        "BuildBridge could not detect one release bundle identifier. Re-approve the project after setting PRODUCT_BUNDLE_IDENTIFIER in Xcode."
            .to_string()
    })?;
    let current = build_mac_builder_view(app, &paths).await?;
    ensure_apple_project_guest_ready(&current)?;
    let container_id = current
        .runtime
        .container_id
        .clone()
        .ok_or_else(|| "The running macOS container identity is unavailable.".to_string())?;
    let previous_profile_uuids = current
        .signing
        .as_ref()
        .map(|signing| {
            signing
                .profiles
                .iter()
                .map(|profile| profile.uuid.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let distribution_certificate = match (
        secrets.signing_certificate_path,
        secrets.signing_certificate_password,
    ) {
        (Some(path), Some(password)) => Some((
            PathBuf::from(ensure_macos_importable_pkcs12(&path, &password)?),
            password,
        )),
        (Some(_), None) => {
            return Err(
                "Store the certificate passphrase in the operating-system vault first.".to_string(),
            );
        }
        (None, _) => None,
    };
    let development_certificate = match (
        secrets.development_certificate_path,
        secrets.development_certificate_password,
    ) {
        (Some(path), Some(password)) => Some((
            PathBuf::from(ensure_macos_importable_pkcs12(&path, &password)?),
            password,
        )),
        (Some(_), None) => {
            return Err(
                "Store the development certificate passphrase in the operating-system vault first."
                    .to_string(),
            );
        }
        (None, _) => None,
    };
    if distribution_certificate.is_none() && development_certificate.is_none() {
        return Err(
            "This kit holds no signing identity. Store a distribution identity with its App Store profile, or a development identity for phone builds, first."
                .to_string(),
        );
    }
    let profile_paths = secrets
        .provisioning_profile_paths
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if distribution_certificate.is_some() && profile_paths.is_empty() {
        return Err("Choose at least one .mobileprovision profile first.".to_string());
    }
    let keychain_password = secrets.guest_keychain_password.ok_or_else(|| {
        "Store a dedicated guest keychain password in the operating-system vault first.".to_string()
    })?;
    let extra_bundle_identifiers: Vec<String> = workspace
        .debug_bundle_identifier
        .iter()
        .filter(|debug| *debug != &bundle_identifier)
        .cloned()
        .collect();
    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();

    let guard = begin_machine_operation(app, machine_id, "provisioning_signing")?;
    if current.signing.is_some()
        && let Err(error) = remove_signing_provisioning_record(&paths)
    {
        drop(guard);
        return Err(error);
    }

    let event_app = app.clone();
    let event_machine_id = machine_id.to_string();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        if !previous_profile_uuids.is_empty() {
            buildbridge_docker_osx::clear_signing(
                &previous_profile_uuids,
                profile.ssh_port,
                &access.username,
                &identity_path,
                &known_hosts_path,
            )
            .map_err(|error| error.to_string())?;
        }
        let material = buildbridge_docker_osx::SigningMaterial {
            distribution_certificate: distribution_certificate
                .as_ref()
                .map(|(path, password)| (path.as_path(), password.as_str())),
            development_certificate: development_certificate
                .as_ref()
                .map(|(path, password)| (path.as_path(), password.as_str())),
            profile_paths: &profile_paths,
            keychain_password: &keychain_password,
            extra_bundle_identifiers: &extra_bundle_identifiers,
        };
        buildbridge_docker_osx::provision_signing(
            &material,
            &development_team,
            &bundle_identifier,
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            |progress: SigningProvisioningProgress| {
                emit_machine_progress(
                    &event_app,
                    SIGNING_PROGRESS_EVENT,
                    &event_machine_id,
                    progress,
                );
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let result = finish_operation(&cancel_probe, joined)?;
    save_signing_provisioning(
        &paths,
        &StoredSigningProvisioning {
            container_id,
            result,
        },
    )?;

    build_mac_builder_view(app, &paths).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrepareDeviceSigningInput {
    udid: String,
    device_name: String,
    confirmed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PrepareDeviceSigningResult {
    view: MacBuilderView,
    certificate_created: bool,
    device_already_registered: bool,
    profile_created: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DeviceSigningPhase {
    CheckingKit,
    CreatingCertificate,
    RegisteringDevice,
    CheckingProfiles,
    CreatingProfile,
    DownloadingProfile,
    Provisioning,
    Completed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceSigningProgress {
    phase: DeviceSigningPhase,
    elapsed_seconds: u64,
    detail: String,
}

/// Makes one phone buildable: a development identity in the kit (created at Apple if needed),
/// the phone registered with the team, a development profile that lists it, and both
/// identities provisioned into the guest keychain. Each step persists before the next, so a
/// retry resumes rather than repeats, and nothing at Apple is revoked.
#[tauri::command]
async fn prepare_apple_device_signing(
    app: AppHandle,
    machine_id: String,
    input: PrepareDeviceSigningInput,
) -> Result<PrepareDeviceSigningResult, String> {
    if !input.confirmed {
        return Err("Confirm the device registration before continuing.".to_string());
    }
    let udid = input.udid.trim().to_ascii_uppercase();
    apple_api::validate_device_udid(&udid)?;
    let device_name = apple_api::validate_device_name(&input.device_name)?;
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let mut workspace = load_apple_workspace(&paths)?
        .ok_or_else(|| "Approve and verify an Apple project first.".to_string())?;
    if !workspace.last_build_succeeded {
        return Err("Complete the unsigned project test build first.".to_string());
    }
    let bundle_identifier = workspace.bundle_identifier.clone().ok_or_else(|| {
        "BuildBridge could not detect one release bundle identifier. Re-approve the project after setting PRODUCT_BUNDLE_IDENTIFIER in Xcode."
            .to_string()
    })?;
    let current = build_mac_builder_view(&app, &paths).await?;
    ensure_apple_project_guest_ready(&current)?;
    let mut kit = resolve_signing_kit_for(&app, &machine_id).await?;
    let (key_id, issuer_id, private_key) = match (
        kit.app_store_connect_key_id.clone(),
        kit.app_store_connect_issuer_id.clone(),
        kit.app_store_connect_private_key.clone(),
    ) {
        (Some(key_id), Some(issuer_id), Some(private_key)) => (key_id, issuer_id, private_key),
        _ => {
            return Err(
                "This kit has no App Store Connect key. Device signing needs a Team key with the Admin role to register the iPhone and create a development profile."
                    .to_string(),
            );
        }
    };
    if kit.guest_keychain_password.is_none() {
        return Err(
            "Store a dedicated guest keychain password in the operating-system vault first."
                .to_string(),
        );
    }

    // The identifier the Debug build carries, which is what its profile must be for. A project
    // often gives Debug its own suffixed identifier so both builds fit on one phone.
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let device_bundle_identifier = {
        let identity_path = paths.guest_identity();
        let known_hosts_path = paths.known_hosts();
        let scheme = workspace.scheme.clone();
        let ssh_port = profile.ssh_port;
        let username = access.username.clone();
        tauri::async_runtime::spawn_blocking(move || {
            buildbridge_docker_osx::resolve_debug_bundle_identifier(
                ssh_port,
                &username,
                &identity_path,
                &known_hosts_path,
                &scheme,
            )
            .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())??
    };
    if device_bundle_identifier != bundle_identifier {
        apple_api::validate_bundle_identifier(&device_bundle_identifier)?;
    }

    // Already prepared for this phone: nothing to do beyond confirming the registration.
    if kit.development_certificate_path.is_some()
        && current.signing.as_ref().is_some_and(|signing| {
            buildbridge_docker_osx::select_development_profile(
                signing,
                &device_bundle_identifier,
                &udid,
            )
            .is_some()
        })
    {
        let registered =
            apple_api::register_device(&key_id, &issuer_id, &private_key, &udid, &device_name)
                .await?;
        return Ok(PrepareDeviceSigningResult {
            view: current,
            certificate_created: false,
            device_already_registered: registered.already_registered,
            profile_created: false,
        });
    }

    let guard = begin_machine_operation(&app, &machine_id, "preparing_device_signing")?;
    let started = std::time::Instant::now();
    let report = |phase: DeviceSigningPhase, detail: &str| {
        emit_machine_progress(
            &app,
            DEVICE_SIGNING_PROGRESS_EVENT,
            &machine_id,
            DeviceSigningProgress {
                phase,
                elapsed_seconds: started.elapsed().as_secs(),
                detail: detail.to_string(),
            },
        );
    };
    let outcome: Result<(bool, bool, bool), String> = async {
        report(
            DeviceSigningPhase::CheckingKit,
            "Checking the kit for a development identity",
        );
        let mut certificate_created = false;
        if kit.development_certificate_path.is_none() {
            report(
                DeviceSigningPhase::CreatingCertificate,
                "Creating an Apple Development certificate for a key generated on this host",
            );
            create_apple_certificate_for_kit(&app, &mut kit, apple_api::CertificateKind::Development)
                .await?;
            certificate_created = true;
        }
        let serial = tauri::async_runtime::spawn_blocking({
            let kit = kit.clone();
            move || development_certificate_serial(&kit)
        })
        .await
        .map_err(|error| error.to_string())??;
        if kit.development_certificate_serial_number.is_none() {
            kit.development_certificate_serial_number = Some(serial.clone());
            save_signing_kit_record(kit.clone()).await?;
        }
        let certificate = apple_api::find_certificate_by_serial(
            &key_id,
            &issuer_id,
            &private_key,
            &serial,
            apple_api::CertificateKind::Development,
        )
        .await?
        .ok_or_else(|| {
            format!(
                "The kit's development certificate (serial {serial}) is not an unexpired development certificate on the Apple team. Create one from the kit, or store the .p12 Apple issued for this team."
            )
        })?;

        report(
            DeviceSigningPhase::RegisteringDevice,
            &format!("Registering {device_name} with the team"),
        );
        let registered =
            apple_api::register_device(&key_id, &issuer_id, &private_key, &udid, &device_name)
                .await?;

        if device_bundle_identifier != bundle_identifier {
            report(
                DeviceSigningPhase::CheckingProfiles,
                &format!(
                    "Registering the Debug identifier {device_bundle_identifier} at Apple with the main app's capabilities"
                ),
            );
            let ensured = apple_api::ensure_bundle_id(
                &key_id,
                &issuer_id,
                &private_key,
                &device_bundle_identifier,
                &format!("{} Debug", workspace.name),
            )
            .await?;
            if ensured.created {
                let main = apple_api::ensure_bundle_id(
                    &key_id,
                    &issuer_id,
                    &private_key,
                    &bundle_identifier,
                    &workspace.name,
                )
                .await?;
                apple_api::copy_bundle_id_capabilities(
                    &key_id,
                    &issuer_id,
                    &private_key,
                    &main.id,
                    &ensured.id,
                )
                .await?;
            }
            if workspace.debug_bundle_identifier.as_deref() != Some(&device_bundle_identifier) {
                workspace.debug_bundle_identifier = Some(device_bundle_identifier.clone());
                save_apple_workspace(&paths, &workspace)?;
            }
        }
        report(
            DeviceSigningPhase::CheckingProfiles,
            "Checking the team's development profiles for this phone",
        );
        let search = apple_api::find_development_profile(
            &key_id,
            &issuer_id,
            &private_key,
            &device_bundle_identifier,
            &certificate.id,
            &udid,
        )
        .await?;
        let mut profile_created = false;
        let (profile, content) = match search.matching {
            Some(profile) if kit_holds_profile_uuid(&kit, &profile.uuid) => (profile, None),
            Some(profile) => {
                report(
                    DeviceSigningPhase::DownloadingProfile,
                    "Downloading the development profile that already lists this phone",
                );
                let (profile, content) =
                    apple_api::download_profile(&key_id, &issuer_id, &private_key, &profile.id)
                        .await?;
                (profile, Some(content))
            }
            None => {
                report(
                    DeviceSigningPhase::CreatingProfile,
                    "Creating a development profile listing this phone",
                );
                let mut device_ids = search.superseded_device_ids;
                if !device_ids.contains(&registered.device.id) {
                    device_ids.push(registered.device.id.clone());
                }
                let created = apple_api::create_development_profile(
                    &key_id,
                    &issuer_id,
                    &private_key,
                    &device_bundle_identifier,
                    &certificate.id,
                    &device_ids,
                )
                .await?;
                profile_created = true;
                (created.profile, Some(created.content))
            }
        };
        if let Some(content) = content {
            if kit.provisioning_profile_paths.len() >= MAX_PROVISIONING_PROFILES {
                return Err(format!(
                    "This kit already holds {MAX_PROVISIONING_PROFILES} profiles. Remove obsolete paths before adding another."
                ));
            }
            let saved_path = save_managed_apple_profile(&app, &profile, &content)?;
            let saved_path = saved_path
                .to_str()
                .ok_or_else(|| "The managed profile path is not valid UTF-8.".to_string())?
                .to_string();
            if !kit
                .provisioning_profile_paths
                .iter()
                .any(|path| path == &saved_path)
            {
                kit.provisioning_profile_paths.push(saved_path);
            }
            save_signing_kit_record(kit.clone()).await?;
        }

        Ok((
            certificate_created,
            registered.already_registered,
            profile_created,
        ))
    }
    .await;
    drop(guard);
    let (certificate_created, device_already_registered, profile_created) = outcome?;

    report(
        DeviceSigningPhase::Provisioning,
        "Provisioning both identities and every profile into the guest keychain",
    );
    let view = provision_with_kit(&app, &machine_id, kit).await?;
    let ready = view.signing.as_ref().is_some_and(|signing| {
        buildbridge_docker_osx::select_development_profile(
            signing,
            &device_bundle_identifier,
            &udid,
        )
        .is_some()
    });
    if !ready {
        return Err(
            "Provisioning finished, but the installed development profile does not list this iPhone. Verify the team and try again."
                .to_string(),
        );
    }
    report(
        DeviceSigningPhase::Completed,
        "Ready to sign for this iPhone",
    );

    Ok(PrepareDeviceSigningResult {
        view,
        certificate_created,
        device_already_registered,
        profile_created,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunAppleDeviceBuildInput {
    udid: String,
    #[serde(default)]
    env_set_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunAppleDeviceResult {
    view: MacBuilderView,
    run: AppleDeviceRunResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAppleDeviceRun {
    container_id: String,
    snapshot_sha256: String,
    device_identifier: String,
    result: AppleDeviceRunResult,
    finished_at_epoch_seconds: u64,
}

/// Builds the Debug configuration for one phone, installs and launches it, and streams its
/// console until the session ends. A Stop while the app runs is the normal end and the run is
/// retained; a failure before launch is kept as the step's diagnostic.
#[tauri::command]
async fn run_apple_device_build(
    app: AppHandle,
    machine_id: String,
    input: RunAppleDeviceBuildInput,
) -> Result<RunAppleDeviceResult, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let udid = input.udid.trim().to_ascii_uppercase();
    apple_api::validate_device_udid(&udid)?;
    remove_apple_device_run_error(&paths)?;
    let chosen_env = guest_env_files_for_set(input.env_set_id.as_deref()).await?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let workspace = load_apple_workspace(&paths)?
        .ok_or_else(|| "Approve and synchronize a local Apple project first.".to_string())?;
    if !workspace.last_build_succeeded || workspace.last_snapshot_sha256.is_none() {
        return Err("Complete the unsigned project test build first.".to_string());
    }
    let current = build_mac_builder_view(&app, &paths).await?;
    ensure_apple_project_guest_ready(&current)?;
    let signing = current
        .signing
        .clone()
        .ok_or_else(|| "Provision and verify signing in macOS first.".to_string())?;
    let device_profile = buildbridge_docker_osx::select_development_profile(
        &signing,
        workspace
            .debug_bundle_identifier
            .as_deref()
            .unwrap_or(&signing.bundle_identifier),
        &udid,
    )
    .cloned()
    .ok_or_else(|| "Prepare signing for this iPhone first.".to_string())?;
    let identity = signing
        .development_identity
        .clone()
        .ok_or_else(|| "The kit has no development identity provisioned.".to_string())?;
    let device = current
        .guest
        .devices
        .iter()
        .find(|device| {
            device
                .udid
                .as_deref()
                .is_some_and(|listed| listed.eq_ignore_ascii_case(&udid))
        })
        .cloned()
        .ok_or_else(|| {
            "The guest does not list that iPhone. Refresh the devices and try again.".to_string()
        })?;
    if !device.ready {
        return Err(device
            .issue
            .clone()
            .unwrap_or_else(|| "The iPhone is not ready for a build.".to_string()));
    }
    let secrets = resolve_signing_kit_for(&app, &machine_id).await?;
    let keychain_password = secrets.guest_keychain_password.ok_or_else(|| {
        "The signing keychain credential is missing from the OS vault.".to_string()
    })?;
    let container_id = current
        .runtime
        .container_id
        .clone()
        .ok_or_else(|| "The macOS builder container identity is unavailable.".to_string())?;
    let snapshot_sha256 = workspace
        .last_snapshot_sha256
        .clone()
        .expect("checked above");
    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();
    let scheme = workspace.scheme.clone();
    let guard = begin_machine_operation(&app, &machine_id, "running_on_device")?;

    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        let device_signing = buildbridge_docker_osx::DeviceSigning {
            keychain_path: &signing.keychain_path,
            identity_sha1: &identity.identity_sha1,
            development_team: &signing.development_team,
            bundle_identifier: &signing.bundle_identifier,
            profile: &device_profile,
        };
        buildbridge_docker_osx::run_apple_device_build(
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            &device_signing,
            &scheme,
            &device,
            &keychain_password,
            chosen_env.as_ref().map(|(_, files)| files),
            |progress: AppleDeviceRunProgress| {
                emit_machine_progress(
                    &event_app,
                    DEVICE_RUN_PROGRESS_EVENT,
                    &event_machine_id,
                    progress,
                );
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let run = match finish_operation(&cancel_probe, joined) {
        Ok(run) => run,
        Err(error) => {
            if error != CANCELLED_MESSAGE {
                let _ = save_apple_device_run_error(&paths, &error);
            }
            return Err(error);
        }
    };
    save_apple_device_run(
        &paths,
        &StoredAppleDeviceRun {
            container_id,
            snapshot_sha256,
            device_identifier: run.device.identifier.clone(),
            result: run.clone(),
            finished_at_epoch_seconds: machines::now_epoch_seconds(),
        },
    )?;
    remove_apple_device_run_error(&paths)?;
    let view = build_mac_builder_view(&app, &paths).await?;

    Ok(RunAppleDeviceResult { view, run })
}

#[tauri::command]
async fn clear_apple_device_run(
    app: AppHandle,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    remove_file_if_present(&paths.apple_device_run_record())?;
    remove_apple_device_run_error(&paths)?;

    build_mac_builder_view(&app, &paths).await
}

/// Copies the Podfile.lock CocoaPods wrote in the guest into the approved host project, so a
/// drifted lock can be adopted without a Mac. The host copy it replaces is kept beside the
/// machine's records, the change is reported pod by pod, and the archive's drift block lifts:
/// the guest workspace already compiled with exactly this lock. Committing it stays the user's.
#[tauri::command]
async fn adopt_guest_podfile_lock(
    app: AppHandle,
    machine_id: String,
) -> Result<AdoptPodfileLockResult, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let mut workspace = load_apple_workspace(&paths)?
        .ok_or_else(|| "Approve and synchronize a local Apple project first.".to_string())?;
    if !workspace.last_native_lock_updated {
        return Err(
            "The guest did not refresh Podfile.lock in its last test build, so there is nothing to adopt."
                .to_string(),
        );
    }
    let current = build_mac_builder_view(&app, &paths).await?;
    ensure_apple_project_guest_ready(&current)?;
    let host_lock = PathBuf::from(&workspace.local_path).join("ios/App/Podfile.lock");
    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();

    let guard = begin_machine_operation(&app, &machine_id, "adopting_lock")?;
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::read_guest_podfile_lock(
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let guest_lock = finish_operation(&cancel_probe, joined)?;

    let before = match fs::read_to_string(&host_lock) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(format!(
                "The project's Podfile.lock could not be read: {error}"
            ));
        }
    };
    let changes = buildbridge_docker_osx::podfile_lock_changes(&before, &guest_lock);
    let backup = paths.podfile_lock_backup();
    if !before.is_empty() {
        write_restricted_file(&backup, before.as_bytes())?;
    }
    let incoming = host_lock.with_extension("lock.buildbridge-incoming");
    fs::write(&incoming, guest_lock.as_bytes())
        .and_then(|()| fs::rename(&incoming, &host_lock))
        .map_err(|error| {
            let _ = fs::remove_file(&incoming);
            format!("The project's Podfile.lock could not be replaced: {error}")
        })?;

    workspace.last_native_lock_updated = false;
    save_apple_workspace(&paths, &workspace)?;
    let view = build_mac_builder_view(&app, &paths).await?;

    Ok(AdoptPodfileLockResult {
        view,
        changes,
        host_path: host_lock.to_string_lossy().into_owned(),
        backup_path: backup.to_string_lossy().into_owned(),
    })
}

fn load_apple_device_run(paths: &MachinePaths) -> Result<Option<StoredAppleDeviceRun>, String> {
    match fs::read(paths.apple_device_run_record()) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("The device run record is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn save_apple_device_run(paths: &MachinePaths, run: &StoredAppleDeviceRun) -> Result<(), String> {
    let encoded = serde_json::to_vec_pretty(run).map_err(|error| error.to_string())?;

    write_restricted_file(&paths.apple_device_run_record(), &encoded)
}

fn save_apple_device_run_error(paths: &MachinePaths, error: &str) -> Result<(), String> {
    let error = error
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .take(8_000)
        .collect::<String>();
    write_restricted_file(&paths.apple_device_run_error(), error.as_bytes())
}

fn remove_apple_device_run_error(paths: &MachinePaths) -> Result<(), String> {
    remove_file_if_present(&paths.apple_device_run_error())
}

#[tauri::command]
async fn clear_mac_guest_signing(
    app: AppHandle,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let current = build_mac_builder_view(&app, &paths).await?;
    ensure_apple_project_guest_ready(&current)?;
    let profile_uuids = current
        .signing
        .as_ref()
        .ok_or_else(|| "No provisioned signing state is recorded for this guest.".to_string())?
        .profiles
        .iter()
        .map(|profile| profile.uuid.clone())
        .collect::<Vec<_>>();
    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();

    let guard = begin_machine_operation(&app, &machine_id, "clearing_signing")?;
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::clear_signing(
            &profile_uuids,
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    finish_operation(&cancel_probe, joined)?;
    remove_signing_provisioning_record(&paths)?;

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
async fn approve_apple_workspace(
    app: AppHandle,
    machine_id: String,
    input: ApproveAppleWorkspaceInput,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    machines::load_registry(&app)?.find(&machine_id)?;
    let approved = inspect_apple_workspace(input.path.trim())?;
    let workspace = match load_apple_workspace(&paths)? {
        Some(existing) if existing.local_path == approved.local_path => StoredAppleWorkspace {
            name: approved.name,
            ios_workspace: approved.ios_workspace,
            scheme: approved.scheme,
            development_team: approved.development_team,
            bundle_identifier: approved.bundle_identifier,
            ..existing
        },
        _ => approved,
    };
    save_apple_workspace(&paths, &workspace)?;

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
async fn clear_apple_workspace(
    app: AppHandle,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    remove_file_if_present(&paths.apple_workspace())?;

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
async fn sync_apple_workspace(
    app: AppHandle,
    machine_id: String,
) -> Result<SyncAppleWorkspaceResult, String> {
    sync_apple_workspace_from(&app, &machine_id, None).await
}

/// Synchronizes a source tree into the guest: the approved folder as it is, or a checked-out
/// revision of the same project when a remote build names a ref.
async fn sync_apple_workspace_from(
    app: &AppHandle,
    machine_id: &str,
    source: Option<(PathBuf, WorkspaceSource)>,
) -> Result<SyncAppleWorkspaceResult, String> {
    let paths = MachinePaths::resolve(app, machine_id)?;
    let profile = machines::load_registry(app)?
        .find(machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let mut workspace = load_apple_workspace(&paths)?
        .ok_or_else(|| "Approve a local Apple project first.".to_string())?;
    let current = build_mac_builder_view(app, &paths).await?;
    ensure_apple_project_guest_ready(&current)?;
    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();
    let (workspace_path, source) = source.unwrap_or_else(|| {
        (
            PathBuf::from(&workspace.local_path),
            WorkspaceSource::folder(),
        )
    });
    let env_files = guest_env_files_for(app, machine_id).await?;
    let guard = begin_machine_operation(app, machine_id, "synchronizing")?;

    let event_app = app.clone();
    let event_machine_id = machine_id.to_string();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::sync_apple_workspace(
            &workspace_path,
            env_files.as_ref(),
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            |progress: AppleProjectProgress| {
                emit_machine_progress(
                    &event_app,
                    PROJECT_PROGRESS_EVENT,
                    &event_machine_id,
                    progress,
                );
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let sync = finish_operation(&cancel_probe, joined)?;

    workspace.last_snapshot_sha256 = Some(sync.snapshot_sha256.clone());
    workspace.last_sync_file_count = Some(sync.source_file_count);
    workspace.last_sync_bytes = Some(sync.source_bytes);
    workspace.last_build_succeeded = false;
    workspace.last_xcode_version = None;
    workspace.last_native_lock_updated = false;
    workspace.last_build_target = None;
    workspace.last_source = Some(source);
    save_apple_workspace(&paths, &workspace)?;
    let view = build_mac_builder_view(app, &paths).await?;

    Ok(SyncAppleWorkspaceResult { view, sync })
}

#[tauri::command]
async fn run_apple_smoke_build(
    app: AppHandle,
    machine_id: String,
    input: Option<RunAppleSmokeBuildInput>,
) -> Result<RunAppleSmokeBuildResult, String> {
    let target = input.unwrap_or_default().target;
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let mut workspace = load_apple_workspace(&paths)?
        .ok_or_else(|| "Approve and synchronize a local Apple project first.".to_string())?;
    if workspace.last_snapshot_sha256.is_none() {
        return Err("Synchronize the approved project before running a test build.".to_string());
    }
    let current = build_mac_builder_view(&app, &paths).await?;
    ensure_apple_project_guest_ready(&current)?;
    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();
    let guard = begin_machine_operation(&app, &machine_id, "test_building")?;
    workspace.last_build_succeeded = false;
    workspace.last_xcode_version = None;
    workspace.last_native_lock_updated = false;
    workspace.last_build_target = None;
    if let Err(error) = save_apple_workspace(&paths, &workspace) {
        drop(guard);
        return Err(error);
    }

    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::run_apple_smoke_build(
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            target,
            |progress: AppleProjectProgress| {
                emit_machine_progress(
                    &event_app,
                    PROJECT_PROGRESS_EVENT,
                    &event_machine_id,
                    progress,
                );
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let build = finish_operation(&cancel_probe, joined)?;

    workspace.last_build_succeeded = true;
    workspace.last_xcode_version = Some(build.xcode_version.clone());
    workspace.last_native_lock_updated = build.native_lockfile_updated;
    workspace.last_build_target = Some(build.target);
    save_apple_workspace(&paths, &workspace)?;
    let view = build_mac_builder_view(&app, &paths).await?;

    Ok(RunAppleSmokeBuildResult { view, build })
}

#[tauri::command]
async fn run_apple_signed_archive(
    app: AppHandle,
    machine_id: String,
    env_set_id: Option<String>,
) -> Result<RunAppleArchiveResult, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    remove_apple_archive_error(&paths)?;
    // The env is a per-build choice: it rebuilds the web assets inside the guest before the
    // archive, so the synced source and the test build are not repeated.
    let chosen_env = guest_env_files_for_set(env_set_id.as_deref()).await?;
    let profile = machines::load_registry(&app)?
        .find(&machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let workspace = load_apple_workspace(&paths)?
        .ok_or_else(|| "Approve and synchronize a local Apple project first.".to_string())?;
    if !workspace.last_build_succeeded || workspace.last_snapshot_sha256.is_none() {
        return Err("Complete the unsigned project test build first.".to_string());
    }
    if workspace.last_native_lock_updated {
        return Err(
            "The guest updated Podfile.lock. Synchronize an approved host lock before creating a signed Release archive."
                .to_string(),
        );
    }
    let current = build_mac_builder_view(&app, &paths).await?;
    ensure_apple_project_guest_ready(&current)?;
    let signing = current
        .signing
        .clone()
        .ok_or_else(|| "Provision and verify signing in macOS first.".to_string())?;
    let distribution = signing.distribution_identity.clone().ok_or_else(|| {
        "The provisioned kit holds only a development identity. A signed archive needs a distribution identity and an App Store profile; add them to the kit and provision again."
            .to_string()
    })?;
    if workspace.development_team.as_deref() != Some(&signing.development_team)
        || workspace.bundle_identifier.as_deref() != Some(&signing.bundle_identifier)
    {
        return Err(
            "The provisioned signing identity no longer matches the approved project.".to_string(),
        );
    }
    let secrets = resolve_signing_kit_for(&app, &machine_id).await?;
    let keychain_password = secrets.guest_keychain_password.ok_or_else(|| {
        "The signing keychain credential is missing from the OS vault.".to_string()
    })?;
    let container_id = current
        .runtime
        .container_id
        .clone()
        .ok_or_else(|| "The macOS builder container identity is unavailable.".to_string())?;
    let snapshot_sha256 = workspace
        .last_snapshot_sha256
        .clone()
        .expect("checked above");
    let output_directory = prepare_apple_archive_output_dir(&paths)?;
    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();
    let signing_certificate_sha256 = distribution.certificate_sha256;
    let guard = match begin_machine_operation(&app, &machine_id, "archiving") {
        Ok(guard) => guard,
        Err(error) => {
            let _ = fs::remove_dir(&output_directory);
            return Err(error);
        }
    };

    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let operation_output_directory = output_directory.clone();
    let scheme = workspace.scheme.clone();
    let env_set_name = chosen_env.as_ref().map(|(name, _)| name.clone());
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::run_signed_apple_archive(
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            &signing,
            &scheme,
            &keychain_password,
            chosen_env.as_ref().map(|(_, files)| files),
            &operation_output_directory,
            |progress: AppleArchiveProgress| {
                emit_machine_progress(
                    &event_app,
                    ARCHIVE_PROGRESS_EVENT,
                    &event_machine_id,
                    progress,
                );
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let archive = match finish_operation(&cancel_probe, joined) {
        Ok(archive) => archive,
        Err(error) => {
            let _ = fs::remove_dir_all(&output_directory);
            if error != CANCELLED_MESSAGE {
                let _ = save_apple_archive_error(&paths, &error);
            }
            return Err(error);
        }
    };
    if let Err(error) = save_apple_archive(
        &paths,
        &StoredAppleArchive {
            container_id,
            snapshot_sha256,
            signing_certificate_sha256,
            result: archive.clone(),
            env_set_name,
        },
    ) {
        let _ = fs::remove_dir_all(&output_directory);
        let _ = save_apple_archive_error(&paths, &error);
        return Err(error);
    }
    remove_apple_archive_error(&paths)?;
    let view = build_mac_builder_view(&app, &paths).await?;

    Ok(RunAppleArchiveResult { view, archive })
}

#[tauri::command]
async fn reveal_apple_archive(app: AppHandle, machine_id: String) -> Result<(), String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let stored = load_apple_archive(&paths)?
        .ok_or_else(|| "No retained signed archive is available.".to_string())?;
    let directory = validated_apple_archive_directory(&paths, &stored.result)?;
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(target_os = "windows") {
        Command::new("explorer")
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(directory)
        .spawn()
        .map_err(|error| format!("Could not reveal the signed artifacts: {error}"))?;

    Ok(())
}

#[tauri::command]
async fn clear_apple_archive(app: AppHandle, machine_id: String) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    if let Some(stored) = load_apple_archive(&paths)? {
        let directory = validated_apple_archive_directory(&paths, &stored.result)?;
        fs::remove_dir_all(directory)
            .map_err(|error| format!("Could not remove the signed artifacts: {error}"))?;
    }
    remove_apple_archive_record(&paths)?;
    remove_apple_archive_error(&paths)?;

    build_mac_builder_view(&app, &paths).await
}

fn is_live(state: ContainerState) -> bool {
    matches!(
        state,
        ContainerState::Running | ContainerState::Paused | ContainerState::Restarting
    )
}

fn ensure_apple_project_guest_ready(view: &MacBuilderView) -> Result<(), String> {
    if view.runtime.state != ContainerState::Running {
        return Err("Start the macOS machine first.".to_string());
    }
    if view.guest.ssh.trust != GuestTrustState::Trusted || !view.guest.diagnostics.authenticated {
        return Err("Finish the pinned macOS guest connection first.".to_string());
    }
    if !view.guest.diagnostics.xcode_selected {
        return Err("Activate Xcode before preparing an Apple project.".to_string());
    }

    Ok(())
}

async fn build_machine_list_view(app: &AppHandle) -> Result<MachineListView, String> {
    let registry = machines::load_registry(app)?;
    let mut entries = Vec::with_capacity(registry.machines.len());
    for machine in &registry.machines {
        let paths = MachinePaths::resolve(app, &machine.id)?;
        let workspace_name = load_apple_workspace(&paths)?.map(|workspace| workspace.name);
        let signing = load_signing_provisioning(&paths)?;
        entries.push((machine.clone(), paths, workspace_name, signing));
    }
    let busy = app
        .state::<AppState>()
        .busy_machines
        .lock()
        .map_err(|_| "The machine operation registry is poisoned.".to_string())?
        .clone();
    let (host, machines) = tauri::async_runtime::spawn_blocking(move || {
        let host = buildbridge_docker_osx::probe_host();
        let mut summaries = Vec::with_capacity(entries.len());
        for (machine, paths, workspace_name, signing) in entries {
            let runtime = buildbridge_docker_osx::status(&paths.container_name)
                .map_err(|error| error.to_string())?;
            // Signing belongs to the container it was imported into; a rebuilt container drops it.
            let signing = signing
                .filter(|stored| runtime.container_id.as_deref() == Some(&stored.container_id));
            summaries.push(MachineSummary {
                id: machine.id.clone(),
                config: machine.config,
                created_at_epoch_seconds: machine.created_at_epoch_seconds,
                state: runtime.state,
                container_id: runtime.container_id,
                busy_operation: busy.get(&machine.id).map(|label| (*label).to_string()),
                guest_configured: paths.guest_access().is_file(),
                trust_pinned: paths.known_hosts().is_file(),
                workspace_name,
                signing_kit_name: None,
                signing_provisioned: signing.is_some(),
                signing_identity: signing.and_then(|stored| {
                    let result = stored.result;
                    result
                        .distribution_identity
                        .or(result.development_identity)
                        .map(|identity| identity.identity_name)
                }),
                archive_retained: paths.apple_archive_record().is_file(),
                env_set_name: None,
                usb_ready: buildbridge_docker_osx::inspect_container_layout(&paths.container_name)
                    .ok()
                    .flatten()
                    .is_some_and(|layout| {
                        layout.disk_on_host && layout.usb_access && layout.control_socket
                    }),
                device_run_retained: paths.apple_device_run_record().is_file(),
            });
        }

        Ok::<_, String>((host, summaries))
    })
    .await
    .map_err(|error| error.to_string())??;

    // Kit and env-set names come from the vault, which the blocking probe above must not touch.
    let kits = read_signing_kits().await?.kits;
    let env_sets = read_env_sets()
        .await
        .map(|stored| stored.sets)
        .unwrap_or_default();
    let attachments = machines::load_registry(app)?;
    let mut machines = machines;
    for summary in &mut machines {
        let attached = attachments
            .find(&summary.id)
            .ok()
            .and_then(|machine| machine.signing_kit_id.clone());
        let resolved = match attached {
            Some(id) => kits.iter().find(|kit| kit.id == id),
            None if kits.len() == 1 => kits.first(),
            None => None,
        };
        summary.signing_kit_name = resolved.map(|kit| kit.name.clone());
        summary.env_set_name = attachments
            .find(&summary.id)
            .ok()
            .and_then(|machine| machine.env_set_id.as_deref())
            .and_then(|id| env_sets.iter().find(|set| set.id == id))
            .map(|set| set.name.clone());
    }

    Ok(MachineListView { host, machines })
}

async fn build_mac_builder_view(
    app: &AppHandle,
    paths: &MachinePaths,
) -> Result<MacBuilderView, String> {
    let profile = machines::load_registry(app)?
        .find(&paths.id)?
        .config
        .clone();
    let guest_access = load_mac_guest_access(paths)?;
    let busy_operation = busy_operation(app, &paths.id)?;
    let probe_paths = paths.clone();
    let probe_profile = profile.clone();
    let cached_devices = app
        .state::<AppState>()
        .guest_devices
        .lock()
        .ok()
        .and_then(|devices| devices.get(&paths.id).cloned())
        .unwrap_or_default();
    let (runtime, logs, guest, mut usb) = tauri::async_runtime::spawn_blocking(move || {
        let runtime = buildbridge_docker_osx::status(&probe_paths.container_name)
            .map_err(|error| error.to_string())?;
        let logs = buildbridge_docker_osx::recent_logs(&probe_paths.container_name)
            .map_err(|error| error.to_string())?;
        let guest = build_mac_guest_view(
            &probe_profile,
            &runtime,
            guest_access.as_ref(),
            &probe_paths,
            cached_devices,
        )?;
        let usb = buildbridge_docker_osx::machine_usb_status(
            &probe_paths.container_name,
            &probe_paths.qmp_socket(),
            runtime.state,
        );

        Ok::<_, String>((runtime, logs, guest, usb))
    })
    .await
    .map_err(|error| error.to_string())??;
    // The attach reports why the guest has not enumerated the phone; the probe cannot, so the
    // last reason is carried until the phone shows up or is detached.
    if let Some(attached) = usb.attached.as_mut() {
        let remembered = app
            .state::<AppState>()
            .usb_attach_issues
            .lock()
            .ok()
            .and_then(|issues| issues.get(&paths.id).cloned());
        if attached.enumerated {
            clear_usb_attach_issue(app, &paths.id);
        } else {
            attached.issue = remembered;
        }
    }
    let attached = attached_kit_id(app, &paths.id)?;
    let (kits, vault_issue) = match read_signing_kits().await {
        Ok(stored) => (stored.kits, None),
        Err(issue) => (Vec::new(), Some(issue)),
    };
    let resolved = resolve_signing_kit(&kits, attached.as_deref());
    let signing_kit = resolved.map(summarize_signing_kit);
    let env_set = match attached_env_set_id(app, &paths.id)? {
        Some(id) => read_env_sets()
            .await
            .ok()
            .and_then(|stored| stored.sets.into_iter().find(|set| set.id == id))
            .map(|set| summarize_env_set(&set, &[])),
        None => None,
    };
    let apple_workspace = load_apple_workspace(paths)?;
    let signing = load_signing_provisioning(paths)?
        .filter(|stored| runtime.container_id.as_deref() == Some(&stored.container_id))
        .map(|stored| stored.result);
    let (archive, archive_env_set) = load_apple_archive(paths)?
        .filter(|stored| {
            std::path::Path::new(&stored.result.ipa.path).is_file()
                && std::path::Path::new(&stored.result.archive.path).is_file()
        })
        .map(|stored| (Some(stored.result), stored.env_set_name))
        .unwrap_or((None, None));
    let archive_error = read_optional_text(&paths.apple_archive_error())?;
    let device_run = load_apple_device_run(paths)?
        .filter(|stored| runtime.container_id.as_deref() == Some(&stored.container_id))
        .map(|stored| stored.result);
    let device_run_error = read_optional_text(&paths.apple_device_run_error())?;

    Ok(MacBuilderView {
        machine_id: paths.id.clone(),
        profile,
        busy_operation,
        runtime,
        signing_kit,
        env_set,
        signing_health: signing_health(vault_issue.as_deref(), resolved, signing.is_some()),
        vault_issue,
        guest,
        apple_workspace,
        signing,
        archive,
        archive_env_set,
        archive_error,
        logs,
        usb,
        device_run,
        device_run_error,
    })
}

fn build_mac_guest_view(
    profile: &MacBuilderConfig,
    runtime: &RuntimeStatus,
    access: Option<&StoredMacGuestAccess>,
    paths: &MachinePaths,
    devices: Vec<buildbridge_docker_osx::GuestDevice>,
) -> Result<MacGuestAccessView, String> {
    let username = access.map(|value| value.username.clone());
    let public_key = read_optional_text(&paths.guest_public_key())?;

    if runtime.state != ContainerState::Running {
        return Ok(MacGuestAccessView {
            username,
            public_key,
            ssh: GuestSshStatus {
                issue: Some("Start the macOS machine to probe guest SSH.".to_string()),
                ..GuestSshStatus::default()
            },
            diagnostics: GuestDiagnostics::default(),
            devices: Vec::new(),
        });
    }

    let known_hosts_path = paths.known_hosts();
    let pinned_host_key = read_optional_text(&known_hosts_path)?;
    let ssh =
        buildbridge_docker_osx::guest_ssh_status(profile.ssh_port, pinned_host_key.as_deref());
    let diagnostics = if ssh.trust == GuestTrustState::Trusted {
        match access {
            Some(access) => buildbridge_docker_osx::guest_diagnostics(
                profile.ssh_port,
                &access.username,
                &paths.guest_identity(),
                &known_hosts_path,
            ),
            None => GuestDiagnostics {
                issue: Some("Enter the macOS short username to configure key access.".to_string()),
                ..GuestDiagnostics::default()
            },
        }
    } else {
        GuestDiagnostics::default()
    };

    Ok(MacGuestAccessView {
        username,
        public_key,
        ssh,
        diagnostics,
        devices,
    })
}

async fn ensure_mac_builder_profile_can_change(
    paths: &MachinePaths,
    stored: &MacBuilderConfig,
    profile: &MacBuilderConfig,
) -> Result<(), String> {
    let unchanged_hardware = stored.macos_release == profile.macos_release
        && stored.memory_gib == profile.memory_gib
        && stored.cpu_cores == profile.cpu_cores
        && stored.ssh_port == profile.ssh_port;
    if unchanged_hardware {
        return Ok(());
    }

    let container_name = paths.container_name.clone();
    let runtime = tauri::async_runtime::spawn_blocking(move || {
        buildbridge_docker_osx::status(&container_name)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;

    if !matches!(
        runtime.state,
        ContainerState::Missing | ContainerState::Unavailable
    ) {
        return Err(
            "Stop the machine and discard its container before changing its hardware profile. The name can be changed at any time."
                .to_string(),
        );
    }

    Ok(())
}

fn normalize_signing_kit(input: SigningKitInput) -> Result<StoredSigningKit, String> {
    let name = input.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 60 {
        return Err("Give the signing kit a name of 1 to 60 characters.".to_string());
    }
    let app_store_connect_private_key_path =
        optional_trim(input.app_store_connect_private_key_path);
    let inferred_key_id = app_store_connect_private_key_path
        .as_deref()
        .and_then(infer_app_store_connect_key_id);
    let supplied_key_id = optional_trim(input.app_store_connect_key_id);
    if let (Some(supplied), Some(inferred)) = (&supplied_key_id, &inferred_key_id)
        && !supplied.eq_ignore_ascii_case(inferred)
    {
        return Err(format!(
            "The App Store Connect key ID does not match the selected AuthKey_{inferred}.p8 file."
        ));
    }
    let app_store_connect_private_key = app_store_connect_private_key_path
        .as_deref()
        .map(read_app_store_connect_private_key)
        .transpose()?;
    let secrets = StoredSigningKit {
        id: input.kit_id.unwrap_or_default(),
        name,
        created_at_epoch_seconds: 0,
        app_store_connect_key_id: supplied_key_id.or(inferred_key_id),
        app_store_connect_issuer_id: optional_trim(input.app_store_connect_issuer_id),
        app_store_connect_private_key,
        signing_certificate_path: optional_trim(input.signing_certificate_path),
        signing_certificate_password: optional_trim(input.signing_certificate_password),
        provisioning_profile_paths: input
            .provisioning_profile_paths
            .into_iter()
            .filter_map(optional_trim)
            .collect(),
        guest_keychain_password: optional_trim(input.guest_keychain_password),
        development_certificate_path: optional_trim(input.development_certificate_path),
        development_certificate_password: optional_trim(input.development_certificate_password),
        development_certificate_serial_number: None,
    };
    let app_store_connect_values = [
        secrets.app_store_connect_key_id.is_some(),
        secrets.app_store_connect_issuer_id.is_some(),
        secrets.app_store_connect_private_key.is_some(),
    ];

    if app_store_connect_values.iter().any(|value| *value)
        && !app_store_connect_values.iter().all(|value| *value)
    {
        return Err(
            "App Store Connect key ID, issuer ID, and private .p8 path must be provided together."
                .to_string(),
        );
    }

    if let Some(private_key) = &secrets.app_store_connect_private_key
        && (private_key.len() > 16_384
            || !private_key.contains("-----BEGIN PRIVATE KEY-----")
            || !private_key.contains("-----END PRIVATE KEY-----"))
    {
        return Err("The App Store Connect private key is not a valid PEM key.".to_string());
    }

    if secrets.provisioning_profile_paths.len() > MAX_PROVISIONING_PROFILES {
        return Err(format!(
            "At most {MAX_PROVISIONING_PROFILES} provisioning profiles can be stored."
        ));
    }

    if let Some(path) = &secrets.signing_certificate_path {
        validate_secret_file(path, &["p12", "pfx"], "signing certificate")?;
    }
    if let Some(path) = &secrets.development_certificate_path {
        validate_secret_file(path, &["p12", "pfx"], "development certificate")?;
    }

    for path in &secrets.provisioning_profile_paths {
        validate_secret_file(path, &["mobileprovision"], "provisioning profile")?;
    }

    for value in [
        secrets.app_store_connect_key_id.as_deref(),
        secrets.app_store_connect_issuer_id.as_deref(),
        secrets.signing_certificate_password.as_deref(),
        secrets.guest_keychain_password.as_deref(),
        secrets.development_certificate_password.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if value.len() > 512 {
            return Err("A credential field exceeds the 512 character limit.".to_string());
        }
    }

    Ok(secrets)
}

fn merge_signing_kit(
    existing: StoredSigningKit,
    mut incoming: StoredSigningKit,
) -> StoredSigningKit {
    incoming.id = existing.id.clone();
    incoming.created_at_epoch_seconds = existing.created_at_epoch_seconds;
    if incoming.app_store_connect_key_id.is_none() {
        incoming.app_store_connect_key_id = existing.app_store_connect_key_id;
        incoming.app_store_connect_issuer_id = existing.app_store_connect_issuer_id;
        incoming.app_store_connect_private_key = existing.app_store_connect_private_key;
    }
    incoming.signing_certificate_path = incoming
        .signing_certificate_path
        .or(existing.signing_certificate_path);
    incoming.signing_certificate_password = incoming
        .signing_certificate_password
        .or(existing.signing_certificate_password);
    if incoming.provisioning_profile_paths.is_empty() {
        incoming.provisioning_profile_paths = existing.provisioning_profile_paths;
    }
    incoming.guest_keychain_password = incoming
        .guest_keychain_password
        .or(existing.guest_keychain_password);
    // A newly supplied development file invalidates the serial recorded for the old one.
    let new_development_file = incoming.development_certificate_path.is_some();
    incoming.development_certificate_path = incoming
        .development_certificate_path
        .or(existing.development_certificate_path);
    incoming.development_certificate_password = incoming
        .development_certificate_password
        .or(existing.development_certificate_password);
    if !new_development_file {
        incoming.development_certificate_serial_number =
            existing.development_certificate_serial_number;
    }

    incoming
}

fn optional_trim(value: String) -> Option<String> {
    let value = value.trim().to_string();

    (!value.is_empty()).then_some(value)
}

fn validate_secret_file(path: &str, extensions: &[&str], label: &str) -> Result<(), String> {
    let path = std::path::Path::new(path);
    let valid_extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        });

    if !path.is_absolute() || !path.is_file() || !valid_extension {
        return Err(format!(
            "The {label} must be an existing absolute path with a supported extension."
        ));
    }

    Ok(())
}

fn infer_app_store_connect_key_id(path: &str) -> Option<String> {
    let file_name = std::path::Path::new(path).file_name()?.to_str()?;
    let key_id = file_name.strip_prefix("AuthKey_")?.strip_suffix(".p8")?;

    (key_id.len() == 10
        && key_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric()))
    .then(|| key_id.to_ascii_uppercase())
}

fn read_app_store_connect_private_key(path: &str) -> Result<String, String> {
    validate_secret_file(path, &["p8"], "App Store Connect private key")?;
    let path = std::path::Path::new(path);
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Could not inspect the App Store Connect private key: {error}"))?;
    if metadata.len() == 0 || metadata.len() > 16_384 {
        return Err(
            "The App Store Connect private key must be between 1 byte and 16 KiB.".to_string(),
        );
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(
                "Restrict the App Store Connect private key to the current user (chmod 600) before storing it."
                    .to_string(),
            );
        }
    }

    let private_key = fs::read_to_string(path)
        .map_err(|error| format!("Could not read the App Store Connect private key: {error}"))?;
    let private_key = private_key.trim().to_string();
    if !private_key.contains("-----BEGIN PRIVATE KEY-----")
        || !private_key.contains("-----END PRIVATE KEY-----")
    {
        return Err("The App Store Connect private key is not a valid PEM key.".to_string());
    }

    Ok(private_key)
}

fn summarize_signing_kit(secrets: &StoredSigningKit) -> SigningKitSummary {
    SigningKitSummary {
        id: secrets.id.clone(),
        name: secrets.name.clone(),
        created_at_epoch_seconds: secrets.created_at_epoch_seconds,
        attached_machines: Vec::new(),
        signing_certificate_password_stored: secrets.signing_certificate_password.is_some(),
        app_store_connect_configured: secrets.app_store_connect_key_id.is_some()
            && secrets.app_store_connect_issuer_id.is_some()
            && secrets.app_store_connect_private_key.is_some(),
        app_store_connect_key_id: secrets.app_store_connect_key_id.clone(),
        signing_certificate_configured: secrets.signing_certificate_path.is_some(),
        signing_certificate_name: secrets.signing_certificate_path.as_ref().and_then(|path| {
            std::path::Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        }),
        provisioning_profile_names: secrets
            .provisioning_profile_paths
            .iter()
            .map(|path| {
                std::path::Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(path)
                    .to_string()
            })
            .collect(),
        guest_keychain_configured: secrets.guest_keychain_password.is_some(),
        development_certificate_configured: secrets.development_certificate_path.is_some(),
        development_certificate_name: secrets.development_certificate_path.as_ref().and_then(
            |path| {
                std::path::Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            },
        ),
        development_certificate_password_stored: secrets.development_certificate_password.is_some(),
    }
}

fn runner_capabilities() -> Vec<String> {
    let mut capabilities = vec!["diagnostics".to_string()];

    match std::env::consts::OS {
        "macos" => capabilities.push("xcode".to_string()),
        "linux" | "windows" => capabilities.push("docker".to_string()),
        _ => {}
    }

    capabilities
}

async fn heartbeat_request(app: &AppHandle) -> HeartbeatRequest {
    HeartbeatRequest {
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: runner_capabilities(),
        machines: machine_reports(app).await,
    }
}

/// The machines as a control plane needs to see them: named, and either ready for a signed
/// archive or not. A failure to inspect them leaves the list empty rather than failing the
/// heartbeat, so a Docker hiccup never reads as a runner going offline.
async fn machine_reports(app: &AppHandle) -> Vec<MachineReport> {
    let Ok(view) = build_machine_list_view(app).await else {
        return Vec::new();
    };
    let env_set_names: Vec<String> = read_env_sets()
        .await
        .map(|stored| stored.sets.into_iter().map(|set| set.name).collect())
        .unwrap_or_default();

    view.machines
        .into_iter()
        .map(|summary| {
            let workspace = MachinePaths::resolve(app, &summary.id)
                .ok()
                .and_then(|paths| load_apple_workspace(&paths).ok().flatten());
            let ready = summary.state == ContainerState::Running
                && summary.trust_pinned
                && summary.signing_provisioned
                && workspace.is_some();

            MachineReport {
                id: summary.id,
                name: summary.config.name,
                ready,
                project: workspace.as_ref().map(|workspace| workspace.name.clone()),
                bundle_identifier: workspace
                    .as_ref()
                    .and_then(|workspace| workspace.bundle_identifier.clone()),
                repository: workspace
                    .as_ref()
                    .and_then(|workspace| project_remote_url(&workspace.local_path))
                    .map(|remote| redact_remote(&remote)),
                env_set: summary.env_set_name,
                env_sets: env_set_names.clone(),
            }
        })
        .collect()
}

/// The approved project's `origin` remote, if the folder is a git checkout. This is the only
/// repository a remote build may fetch from; the control plane never supplies one.
fn project_remote_url(local_path: &str) -> Option<String> {
    if !std::path::Path::new(local_path).join(".git").exists() {
        return None;
    }
    let output = Command::new("git")
        .args(["-C", local_path, "remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let remote = String::from_utf8_lossy(&output.stdout).trim().to_string();

    (!remote.is_empty()).then_some(remote)
}

/// A remote URL without any credentials that may be embedded in it.
fn redact_remote(remote: &str) -> String {
    match (remote.find("://"), remote.find('@')) {
        (Some(scheme_end), Some(at)) if at > scheme_end => {
            format!("{}{}", &remote[..scheme_end + 3], &remote[at + 1..])
        }
        _ => remote.to_string(),
    }
}

/// Checks out one revision of the approved project into the machine's checkout directory and
/// returns the commit it resolved to. Fixed argv, no shell; the ref was validated by both the
/// control plane and the contract crate, and is validated once more here.
fn checkout_project_ref(
    remote: &str,
    git_ref: &str,
    checkout: &std::path::Path,
) -> Result<String, String> {
    if !valid_git_ref(git_ref) {
        return Err("That ref is not a branch, tag, or commit as git names them.".to_string());
    }
    if !checkout.join(".git").is_dir() {
        if checkout.exists() {
            fs::remove_dir_all(checkout)
                .map_err(|error| format!("Could not reset the checkout directory: {error}"))?;
        }
        if let Some(parent) = checkout.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create the checkout directory: {error}"))?;
        }
        git(&[
            "clone",
            "--quiet",
            "--no-checkout",
            "--",
            remote,
            &checkout.to_string_lossy(),
        ])?;
    }
    let dir = checkout.to_string_lossy().to_string();
    git(&["-C", &dir, "remote", "set-url", "origin", remote])?;
    git(&["-C", &dir, "fetch", "--quiet", "--force", "origin", git_ref])?;
    git(&[
        "-C",
        &dir,
        "checkout",
        "--quiet",
        "--force",
        "--detach",
        "FETCH_HEAD",
    ])?;
    git(&["-C", &dir, "clean", "--quiet", "-fdx"])?;
    let commit = git(&["-C", &dir, "rev-parse", "HEAD"])?.trim().to_string();

    Ok(commit)
}

fn git(args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| format!("git is not available on this host: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "git {} failed: {}",
            args.iter()
                .find(|arg| !arg.starts_with('-') && **arg != "-C")
                .copied()
                .unwrap_or("command"),
            stderr.trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn paired_client(app: &AppHandle) -> Result<(StoredConfig, ApiClient), String> {
    let config = load_config(app)?.ok_or_else(|| "Pair this runner first.".to_string())?;
    let token = read_token(config.runner_id.clone()).await?;
    let client = ApiClient::new(&config.server_url, token).map_err(|error| error.to_string())?;

    Ok((config, client))
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("runner.json"))
        .map_err(|error| error.to_string())
}

fn load_config(app: &AppHandle) -> Result<Option<StoredConfig>, String> {
    let path = config_path(app)?;

    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("The runner configuration is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn save_config(app: &AppHandle, config: &StoredConfig) -> Result<(), String> {
    let path = config_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "The runner configuration directory is unavailable.".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    fs::write(
        path,
        serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn prepare_apple_archive_output_dir(paths: &MachinePaths) -> Result<PathBuf, String> {
    let root = paths.artifacts_dir();
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    set_restricted_directory_permissions(&root)?;
    let operation_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "The system clock is earlier than the Unix epoch.".to_string())?
        .as_millis();
    let directory = root.join(format!("archive-{operation_id}-{}", std::process::id()));
    fs::create_dir(&directory).map_err(|error| error.to_string())?;
    set_restricted_directory_permissions(&directory)?;

    Ok(directory)
}

fn remove_file_if_present(path: &std::path::Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn managed_apple_profiles_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("macos-builder").join("profiles"))
        .map_err(|error| error.to_string())
}

/// A provisioning profile this host already downloaded, kept so a kit can be rebuilt without
/// going back to Apple. Only the file name and path are exposed; the contents stay on disk.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagedAppleProfile {
    file_name: String,
    path: String,
    saved_at_epoch_seconds: u64,
}

/// Managed copies are always written with this extension, so anything else in the directory is
/// not ours to offer.
fn is_managed_profile_file(file_name: &str) -> bool {
    !file_name.starts_with('.')
        && std::path::Path::new(file_name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mobileprovision"))
}

/// Lists the profiles already on this host. A vault that loses its paths does not lose these
/// files, so they can be attached to a kit again in one step.
#[tauri::command]
fn list_managed_apple_profiles(app: AppHandle) -> Result<Vec<ManagedAppleProfile>, String> {
    let directory = managed_apple_profiles_dir(&app)?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("Could not read {}: {error}", directory.display())),
    };

    let mut profiles: Vec<ManagedAppleProfile> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let file_name = entry.file_name().to_string_lossy().to_string();
            if !is_managed_profile_file(&file_name) {
                return None;
            }
            let saved_at_epoch_seconds = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |elapsed| elapsed.as_secs());

            Some(ManagedAppleProfile {
                file_name,
                path: entry.path().to_string_lossy().to_string(),
                saved_at_epoch_seconds,
            })
        })
        .collect();

    profiles.sort_by(|left, right| {
        right
            .saved_at_epoch_seconds
            .cmp(&left.saved_at_epoch_seconds)
            .then_with(|| left.file_name.cmp(&right.file_name))
    });
    profiles.truncate(MAX_PROVISIONING_PROFILES);

    Ok(profiles)
}

fn save_managed_apple_profile(
    app: &AppHandle,
    profile: &apple_api::AppleProvisioningProfileSummary,
    content: &[u8],
) -> Result<PathBuf, String> {
    let file_id = if safe_apple_resource_component(&profile.uuid) {
        &profile.uuid
    } else if safe_apple_resource_component(&profile.id) {
        &profile.id
    } else {
        return Err("Apple returned an unsafe profile identifier.".to_string());
    };
    if content.is_empty() || content.len() > 2 * 1024 * 1024 {
        return Err("Apple returned profile content with an unsafe size.".to_string());
    }

    let path = managed_apple_profiles_dir(app)?.join(format!("{file_id}.mobileprovision"));
    write_restricted_file(&path, content)?;

    Ok(path)
}

fn safe_apple_resource_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn load_signing_provisioning(
    paths: &MachinePaths,
) -> Result<Option<StoredSigningProvisioning>, String> {
    let path = paths.signing_provisioning();

    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("The guest signing record is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn load_apple_archive(paths: &MachinePaths) -> Result<Option<StoredAppleArchive>, String> {
    let path = paths.apple_archive_record();

    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("The signed archive record is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn save_apple_archive(paths: &MachinePaths, archive: &StoredAppleArchive) -> Result<(), String> {
    let path = paths.apple_archive_record();
    let encoded = serde_json::to_vec_pretty(archive).map_err(|error| error.to_string())?;

    write_restricted_file(&path, &encoded)
}

fn remove_apple_archive_record(paths: &MachinePaths) -> Result<(), String> {
    remove_file_if_present(&paths.apple_archive_record())
}

fn save_apple_archive_error(paths: &MachinePaths, error: &str) -> Result<(), String> {
    let error = error
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .take(8_000)
        .collect::<String>();
    write_restricted_file(&paths.apple_archive_error(), error.as_bytes())
}

fn remove_apple_archive_error(paths: &MachinePaths) -> Result<(), String> {
    remove_file_if_present(&paths.apple_archive_error())
}

fn validated_apple_archive_directory(
    paths: &MachinePaths,
    result: &AppleArchiveResult,
) -> Result<PathBuf, String> {
    let root = paths.artifacts_dir();
    let root = fs::canonicalize(root)
        .map_err(|error| format!("The managed artifact directory is unavailable: {error}"))?;
    let ipa = fs::canonicalize(&result.ipa.path)
        .map_err(|error| format!("The retained IPA is unavailable: {error}"))?;
    let archive = fs::canonicalize(&result.archive.path)
        .map_err(|error| format!("The retained Xcode archive is unavailable: {error}"))?;
    if !ipa.is_file() || !archive.is_file() {
        return Err("The retained signed artifacts are no longer regular files.".to_string());
    }
    let directory = ipa
        .parent()
        .ok_or_else(|| "The retained IPA has no parent directory.".to_string())?;
    if archive.parent() != Some(directory) || directory.parent() != Some(root.as_path()) {
        return Err(
            "The signed artifact record is outside BuildBridge's managed directory.".to_string(),
        );
    }

    Ok(directory.to_path_buf())
}

fn save_signing_provisioning(
    paths: &MachinePaths,
    signing: &StoredSigningProvisioning,
) -> Result<(), String> {
    let path = paths.signing_provisioning();
    let encoded = serde_json::to_vec_pretty(signing).map_err(|error| error.to_string())?;

    write_restricted_file(&path, &encoded)
}

fn remove_signing_provisioning_record(paths: &MachinePaths) -> Result<(), String> {
    remove_file_if_present(&paths.signing_provisioning())
}

fn load_apple_workspace(paths: &MachinePaths) -> Result<Option<StoredAppleWorkspace>, String> {
    let path = paths.apple_workspace();

    match fs::read(path) {
        Ok(bytes) => {
            let mut workspace: StoredAppleWorkspace =
                serde_json::from_slice(&bytes).map_err(|error| {
                    format!("The approved Apple project configuration is invalid: {error}")
                })?;
            if workspace.development_team.is_none() || workspace.bundle_identifier.is_none() {
                let project_path = PathBuf::from(&workspace.local_path)
                    .join("ios/App/App.xcodeproj/project.pbxproj");
                if let Ok(project) = fs::read_to_string(project_path) {
                    workspace.development_team = one_xcode_setting(&project, "DEVELOPMENT_TEAM");
                    workspace.bundle_identifier = release_bundle_identifier(&project);
                }
            }

            Ok(Some(workspace))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn save_apple_workspace(
    paths: &MachinePaths,
    workspace: &StoredAppleWorkspace,
) -> Result<(), String> {
    let path = paths.apple_workspace();
    let encoded = serde_json::to_vec_pretty(workspace).map_err(|error| error.to_string())?;

    write_restricted_file(&path, &encoded)
}

fn inspect_apple_workspace(path: &str) -> Result<StoredAppleWorkspace, String> {
    if path.is_empty() {
        return Err("Choose an absolute local project directory.".to_string());
    }
    let requested = std::path::Path::new(path);
    if !requested.is_absolute() {
        return Err("The approved project path must be absolute.".to_string());
    }
    let canonical = fs::canonicalize(requested)
        .map_err(|error| format!("The selected project directory is unavailable: {error}"))?;
    if !canonical.is_dir() {
        return Err("The selected project path is not a directory.".to_string());
    }
    for required in [
        "package.json",
        "pnpm-lock.yaml",
        "capacitor.config.ts",
        "ios/App/Podfile",
        "ios/App/Podfile.lock",
        "ios/App/App.xcodeproj/project.pbxproj",
    ] {
        if !canonical.join(required).is_file() {
            return Err(format!("This project is missing {required}."));
        }
    }
    if !canonical.join("ios/App/App.xcworkspace").is_dir() {
        return Err("This project is missing ios/App/App.xcworkspace.".to_string());
    }

    let package: serde_json::Value = serde_json::from_slice(
        &fs::read(canonical.join("package.json"))
            .map_err(|error| format!("Could not read package.json: {error}"))?,
    )
    .map_err(|error| format!("package.json is invalid: {error}"))?;
    let name = package
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.trim().is_empty() && name.len() <= 120)
        .map(str::to_string)
        .or_else(|| {
            canonical
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .ok_or_else(|| "The selected project name is invalid.".to_string())?;
    let local_path = canonical
        .to_str()
        .filter(|path| path.len() <= 4_096)
        .ok_or_else(|| "The selected project path is not valid UTF-8.".to_string())?
        .to_string();
    let project_file = fs::read_to_string(canonical.join("ios/App/App.xcodeproj/project.pbxproj"))
        .map_err(|error| format!("Could not read the Xcode project settings: {error}"))?;
    let development_team = one_xcode_setting(&project_file, "DEVELOPMENT_TEAM");
    let bundle_identifier = release_bundle_identifier(&project_file);

    Ok(StoredAppleWorkspace {
        local_path,
        name,
        ios_workspace: "ios/App/App.xcworkspace".to_string(),
        scheme: "App".to_string(),
        development_team,
        bundle_identifier,
        last_snapshot_sha256: None,
        last_sync_file_count: None,
        last_sync_bytes: None,
        last_build_succeeded: false,
        last_xcode_version: None,
        last_native_lock_updated: false,
        last_build_target: None,
        debug_bundle_identifier: None,
        last_source: None,
    })
}

fn xcode_setting_values(project: &str, key: &str) -> Vec<String> {
    let prefix = format!("{key} = ");
    let mut values = Vec::new();
    for line in project.lines() {
        let line = line.trim();
        let Some(value) = line
            .strip_prefix(&prefix)
            .and_then(|value| value.strip_suffix(';'))
        else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        let valid = !value.is_empty()
            && value.len() <= 255
            && value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '-')
            });
        if valid && !values.iter().any(|stored| stored == value) {
            values.push(value.to_string());
        }
    }

    values
}

fn one_xcode_setting(project: &str, key: &str) -> Option<String> {
    let mut values = xcode_setting_values(project, key);

    (values.len() == 1).then(|| values.remove(0))
}

fn release_bundle_identifier(project: &str) -> Option<String> {
    let mut values = xcode_setting_values(project, "PRODUCT_BUNDLE_IDENTIFIER");
    if values.len() == 1 {
        return values.pop();
    }
    let release_values = values
        .into_iter()
        .filter(|value| !value.ends_with(".debug") && !value.ends_with(".Debug"))
        .collect::<Vec<_>>();

    (release_values.len() == 1).then(|| release_values[0].clone())
}

fn load_mac_guest_access(paths: &MachinePaths) -> Result<Option<StoredMacGuestAccess>, String> {
    let path = paths.guest_access();

    match fs::read(path) {
        Ok(bytes) => {
            let access: StoredMacGuestAccess = serde_json::from_slice(&bytes).map_err(|error| {
                format!("The macOS guest access configuration is invalid: {error}")
            })?;
            if !buildbridge_docker_osx::valid_guest_username(&access.username) {
                return Err("The stored macOS guest username is invalid.".to_string());
            }
            Ok(Some(access))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn save_mac_guest_access(
    paths: &MachinePaths,
    access: &StoredMacGuestAccess,
) -> Result<(), String> {
    let path = paths.guest_access();
    let encoded = serde_json::to_vec_pretty(access).map_err(|error| error.to_string())?;

    write_restricted_file(&path, &encoded)
}

fn ensure_mac_guest_keypair(identity_path: &std::path::Path) -> Result<String, String> {
    let public_key_path = identity_path.with_file_name("guest_ed25519.pub");
    match (identity_path.is_file(), public_key_path.is_file()) {
        (true, true) => return read_mac_guest_public_key(&public_key_path),
        (true, false) | (false, true) => {
            return Err(
                "The BuildBridge guest SSH keypair is incomplete. Restore the missing key before continuing."
                    .to_string(),
            );
        }
        (false, false) => {}
    }

    let parent = identity_path
        .parent()
        .ok_or_else(|| "The macOS guest key directory is unavailable.".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let output = Command::new("ssh-keygen")
        .args([
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            "buildbridge-guest",
            "-f",
        ])
        .arg(identity_path)
        .output()
        .map_err(|error| {
            format!("Could not run ssh-keygen; install OpenSSH client tools: {error}")
        })?;

    if !output.status.success() {
        return Err(format!(
            "Could not create the guest SSH key: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    set_restricted_permissions(identity_path)?;
    read_mac_guest_public_key(&public_key_path)
}

fn read_mac_guest_public_key(path: &std::path::Path) -> Result<String, String> {
    let public_key = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let public_key = public_key.trim();
    let fields: Vec<_> = public_key.split_whitespace().collect();

    if public_key.len() > 2_048
        || fields.len() < 2
        || fields.len() > 3
        || fields.first() != Some(&"ssh-ed25519")
    {
        return Err("The BuildBridge guest SSH public key is invalid.".to_string());
    }

    Ok(public_key.to_string())
}

fn read_optional_text(path: &std::path::Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value.trim().to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn write_restricted_file(path: &std::path::Path, contents: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "The macOS guest configuration directory is unavailable.".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    fs::write(path, contents).map_err(|error| error.to_string())?;
    set_restricted_permissions(path)
}

fn set_restricted_permissions(path: &std::path::Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}

fn set_restricted_directory_permissions(path: &std::path::Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }

    Ok(())
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

/// Reads every stored kit.
///
/// The first release kept one unnamed record in this entry. That shape is recognised by the
/// absence of a `kits` array and migrated in memory to a single kit named "Signing kit", so an
/// existing vault keeps working without a separate migration step.
/// Decodes a vault entry, migrating the pre-registry single record.
///
/// Kept separate from the keyring so migration, corruption and defaulting are all testable.
fn parse_signing_vault(encoded: &str) -> Result<StoredSigningKits, String> {
    let value: serde_json::Value = serde_json::from_str(encoded)
        .map_err(|error| format!("The signing vault entry is invalid: {error}"))?;

    if value.get("kits").is_some() {
        let mut kits: StoredSigningKits = serde_json::from_value(value)
            .map_err(|error| format!("The signing vault entry is invalid: {error}"))?;
        for kit in &mut kits.kits {
            if kit.id.is_empty() {
                kit.id = DEFAULT_SIGNING_KIT_ID.to_string();
            }
            if kit.name.is_empty() {
                kit.name = "Signing kit".to_string();
            }
        }
        return Ok(kits);
    }

    let legacy: StoredSigningKit = serde_json::from_value(value)
        .map_err(|error| format!("The signing vault entry is invalid: {error}"))?;

    Ok(StoredSigningKits {
        kits: vec![StoredSigningKit {
            id: DEFAULT_SIGNING_KIT_ID.to_string(),
            name: "Signing kit".to_string(),
            ..legacy
        }],
    })
}

/// The kit a machine uses: its explicit attachment, and nothing else. Signing material is
/// never picked on a machine's behalf — not even when the host holds exactly one kit — because
/// which identity signs a build is a decision, and the interface should show it being made.
fn resolve_signing_kit<'a>(
    kits: &'a [StoredSigningKit],
    attached: Option<&str>,
) -> Option<&'a StoredSigningKit> {
    let id = attached?;

    kits.iter().find(|kit| kit.id == id)
}

/// Whether a kit holds everything provisioning needs.
/// A kit provisions with either identity. The distribution set — identity, passphrase and at
/// least one profile — is what an archive needs; a development identity with its passphrase is
/// enough to run on a phone, and a kit holding only that is complete for that route alone.
fn kit_is_complete(kit: &StoredSigningKit) -> bool {
    kit.guest_keychain_password.is_some()
        && (kit_has_distribution_set(kit) || kit_has_development_identity(kit))
}

fn kit_has_distribution_set(kit: &StoredSigningKit) -> bool {
    kit.signing_certificate_path.is_some()
        && kit.signing_certificate_password.is_some()
        && !kit.provisioning_profile_paths.is_empty()
}

fn kit_has_development_identity(kit: &StoredSigningKit) -> bool {
    kit.development_certificate_path.is_some() && kit.development_certificate_password.is_some()
}

/// Classifies what a machine can do about signing right now.
///
/// `KitMissing` is the state left by an operating-system keyring being cleared: the guest still
/// holds a provisioned keychain, but the material that created it is gone, so the interface must
/// ask for the kit again instead of claiming signing is configured.
fn signing_health(
    vault_issue: Option<&str>,
    kit: Option<&StoredSigningKit>,
    provisioned: bool,
) -> SigningHealth {
    match (vault_issue, kit) {
        (Some(_), _) => SigningHealth::VaultUnavailable,
        (None, None) if provisioned => SigningHealth::KitMissing,
        (None, None) => SigningHealth::Unconfigured,
        (None, Some(kit)) if kit_is_complete(kit) => SigningHealth::Ready,
        (None, Some(_)) => SigningHealth::Incomplete,
    }
}

async fn read_signing_kits() -> Result<StoredSigningKits, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let entry = mac_builder_credential_entry()?;

        match entry.get_password() {
            Ok(encoded) => parse_signing_vault(&encoded),
            Err(keyring::Error::NoEntry) => Ok(StoredSigningKits::default()),
            Err(error) => Err(format!(
                "The operating-system credential vault could not be read: {error}"
            )),
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn write_signing_kits(kits: StoredSigningKits) -> Result<(), String> {
    let encoded = serde_json::to_string(&kits).map_err(|error| error.to_string())?;

    tauri::async_runtime::spawn_blocking(move || {
        mac_builder_credential_entry()?
            .set_password(&encoded)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The kit a machine will provision: its explicit attachment, or the only kit on the host.
async fn resolve_signing_kit_for(
    app: &AppHandle,
    machine_id: &str,
) -> Result<StoredSigningKit, String> {
    optional_signing_kit_for(app, machine_id)
        .await?
        .ok_or_else(|| {
            "Attach a signing kit to this machine first. Signing kits are stored once on this host and attached per machine."
                .to_string()
        })
}

/// The kit attached to a machine, if it is attached to one that is still stored.
async fn optional_signing_kit_for(
    app: &AppHandle,
    machine_id: &str,
) -> Result<Option<StoredSigningKit>, String> {
    let attached = attached_kit_id(app, machine_id)?;
    let kits = read_signing_kits().await?.kits;

    Ok(resolve_signing_kit(&kits, attached.as_deref()).cloned())
}

fn attached_kit_id(app: &AppHandle, machine_id: &str) -> Result<Option<String>, String> {
    Ok(machines::load_registry(app)?
        .find(machine_id)
        .ok()
        .and_then(|machine| machine.signing_kit_id.clone()))
}

async fn save_signing_kit_record(kit: StoredSigningKit) -> Result<(), String> {
    let mut kits = read_signing_kits().await?;
    match kits.kits.iter_mut().find(|stored| stored.id == kit.id) {
        Some(stored) => *stored = kit,
        None => kits.kits.push(kit),
    }

    write_signing_kits(kits).await
}

fn env_set_credential_entry() -> Result<Entry, String> {
    Entry::new(ENV_SET_CREDENTIAL_SERVICE, MAC_BUILDER_CREDENTIAL_ACCOUNT)
        .map_err(|error| error.to_string())
}

async fn read_env_sets() -> Result<StoredEnvSets, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let entry = env_set_credential_entry()?;

        match entry.get_password() {
            Ok(encoded) => serde_json::from_str::<StoredEnvSets>(&encoded)
                .map_err(|error| format!("The env set vault entry is invalid: {error}")),
            Err(keyring::Error::NoEntry) => Ok(StoredEnvSets::default()),
            Err(error) => Err(format!(
                "The operating-system credential vault could not be read: {error}"
            )),
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn write_env_sets(sets: StoredEnvSets) -> Result<(), String> {
    let encoded = serde_json::to_string(&sets).map_err(|error| error.to_string())?;

    tauri::async_runtime::spawn_blocking(move || {
        env_set_credential_entry()?
            .set_password(&encoded)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

fn attached_env_set_id(app: &AppHandle, machine_id: &str) -> Result<Option<String>, String> {
    Ok(machines::load_registry(app)?
        .find(machine_id)
        .ok()
        .and_then(|machine| machine.env_set_id.clone()))
}

/// The attached set rendered for the guest, or `None` when the machine has none. A missing set
/// is an error rather than a silent build without variables, because that is how a production
/// build ends up pointed at the wrong backend.
async fn guest_env_files_for(
    app: &AppHandle,
    machine_id: &str,
) -> Result<Option<GuestEnvFiles>, String> {
    Ok(
        guest_env_files_for_set(attached_env_set_id(app, machine_id)?.as_deref())
            .await?
            .map(|(_, files)| files),
    )
}

/// One stored set rendered for the guest, with its name; `None` for no set at all.
async fn guest_env_files_for_set(
    set_id: Option<&str>,
) -> Result<Option<(String, GuestEnvFiles)>, String> {
    let Some(id) = set_id else {
        return Ok(None);
    };
    let stored = read_env_sets().await?;
    let set = stored
        .sets
        .into_iter()
        .find(|set| set.id == id)
        .ok_or_else(|| {
            "That env set is no longer stored. Choose another or build without one.".to_string()
        })?;

    Ok(Some((
        set.name.clone(),
        GuestEnvFiles {
            dotenv: render_dotenv(&set.variables),
            shell: render_shell_env(&set.variables),
        },
    )))
}

/// Plain values are already in every summary, so only the secrets come back this way.
fn stored_env_secrets(
    stored: &StoredEnvSets,
    set_id: &str,
) -> Result<Vec<EnvVariableSummary>, String> {
    let set = stored
        .sets
        .iter()
        .find(|set| set.id == set_id)
        .ok_or_else(|| "This env set is no longer stored.".to_string())?;

    Ok(set
        .variables
        .iter()
        .filter(|variable| variable.secret)
        .map(|variable| EnvVariableSummary {
            key: variable.key.clone(),
            value: variable.value.clone(),
        })
        .collect())
}

fn summarize_env_set(set: &StoredEnvSet, machines: &[machines::StoredMachine]) -> EnvSetSummary {
    EnvSetSummary {
        id: set.id.clone(),
        name: set.name.clone(),
        variables: set
            .variables
            .iter()
            .filter(|variable| !variable.secret)
            .map(|variable| EnvVariableSummary {
                key: variable.key.clone(),
                value: variable.value.clone(),
            })
            .collect(),
        secret_keys: set
            .variables
            .iter()
            .filter(|variable| variable.secret)
            .map(|variable| variable.key.clone())
            .collect(),
        created_at_epoch_seconds: set.created_at_epoch_seconds,
        attached_machines: machines
            .iter()
            .filter(|machine| machine.env_set_id.as_deref() == Some(set.id.as_str()))
            .map(|machine| machine.config.name.clone())
            .collect(),
    }
}

/// A variable name as every dotenv loader and POSIX shell agree on it.
fn valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 120
        && key
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Why a value cannot be stored, if it cannot. Both renderings below have to read it back the
/// same way, which rules out line breaks and a value that mixes both kinds of quote.
fn env_value_issue(value: &str) -> Option<&'static str> {
    if value.len() > MAX_ENV_VALUE_LENGTH {
        return Some("is longer than 4096 characters");
    }
    if value
        .chars()
        .any(|c| c == '\n' || c == '\r' || (c.is_control() && c != '\t'))
    {
        return Some("cannot contain line breaks or control characters");
    }
    if value.contains('"') && value.contains('\'') {
        return Some("cannot contain both single and double quotes");
    }

    None
}

/// Applies an edit to a set: every listed key with a typed value takes it, a listed key with no
/// value keeps what is stored, and an unlisted stored key is removed. A stored secret is never
/// carried over as a plain variable, since that would show a value stored on the promise that it
/// never would be.
fn merge_env_variables(
    existing: Option<&StoredEnvSet>,
    input: &EnvSetInput,
) -> Result<Vec<StoredEnvVariable>, String> {
    if input.variables.len() > MAX_ENV_VARIABLES {
        return Err(format!(
            "An env set holds at most {MAX_ENV_VARIABLES} variables."
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut variables = Vec::with_capacity(input.variables.len());
    for variable in &input.variables {
        let key = variable.key.trim();
        if !valid_env_key(key) {
            return Err(format!(
                "\"{key}\" is not a valid variable name. Use letters, digits and underscores, not starting with a digit."
            ));
        }
        if !seen.insert(key.to_string()) {
            return Err(format!("{key} is listed twice."));
        }
        let value = match &variable.value {
            Some(value) => value.clone(),
            None => {
                let stored = existing
                    .and_then(|set| set.variables.iter().find(|stored| stored.key == key))
                    .ok_or_else(|| format!("{key} needs a value."))?;
                if stored.secret && !variable.secret {
                    return Err(format!(
                        "{key} is stored as a secret. Enter its value to keep it as a variable."
                    ));
                }
                stored.value.clone()
            }
        };
        if let Some(issue) = env_value_issue(&value) {
            return Err(format!("The value of {key} {issue}."));
        }
        variables.push(StoredEnvVariable {
            key: key.to_string(),
            value,
            secret: variable.secret,
        });
    }

    Ok(variables)
}

/// The set as Vite's dotenv loader reads it. Double quotes with `$` escaped so nothing expands;
/// single quotes when the value itself holds a double quote, which dotenv takes verbatim.
fn render_dotenv(variables: &[StoredEnvVariable]) -> String {
    let mut out = String::from(
        "# Written by BuildBridge from the attached env set. Not part of the project.\n",
    );
    for variable in variables {
        out.push_str(&variable.key);
        out.push('=');
        if variable.value.contains('"') {
            out.push('\'');
            out.push_str(&variable.value);
            out.push('\'');
        } else {
            out.push('"');
            for character in variable.value.chars() {
                match character {
                    '\\' => out.push_str("\\\\"),
                    '$' => out.push_str("\\$"),
                    '`' => out.push_str("\\`"),
                    other => out.push(other),
                }
            }
            out.push('"');
        }
        out.push('\n');
    }

    out
}

/// The same set as a POSIX shell sources it: single-quoted, which is exact for anything but a
/// single quote, and that is spelled `'\''`.
fn render_shell_env(variables: &[StoredEnvVariable]) -> String {
    let mut out = String::from("# Written by BuildBridge from the attached env set.\n");
    for variable in variables {
        out.push_str("export ");
        out.push_str(&variable.key);
        out.push_str("='");
        out.push_str(&variable.value.replace('\'', "'\\''"));
        out.push_str("'\n");
    }

    out
}

/// Removes an identity BuildBridge created for this kit. Its password lived only in the kit
/// that was just deleted, so the file could not be used again anyway.
fn remove_managed_certificate_for(app: &AppHandle, kit: &StoredSigningKit) -> Result<(), String> {
    let managed = managed_apple_certificates_dir(app)?;
    for path in [
        kit.signing_certificate_path.as_ref(),
        kit.development_certificate_path.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        let candidate = std::path::Path::new(path);
        if candidate.starts_with(&managed)
            && let Some(directory) = candidate.parent()
            && directory != managed
        {
            let _ = fs::remove_dir_all(directory);
        }
    }

    Ok(())
}

/// Removes only the Apple-created profile files this kit owns, leaving other kits alone.
fn remove_managed_profiles_for(app: &AppHandle, kit: &StoredSigningKit) -> Result<(), String> {
    let managed = managed_apple_profiles_dir(app)?;
    for path in &kit.provisioning_profile_paths {
        let candidate = std::path::Path::new(path);
        if candidate.starts_with(&managed) {
            remove_file_if_present(candidate)?;
        }
    }

    Ok(())
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
