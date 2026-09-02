use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use buildbridge_contract::{
    CompleteBuildRequest, HeartbeatRequest, PROTOCOL_VERSION, PairRunnerRequest,
    RealtimeAuthorizationRequest, RealtimeConfiguration,
};
use buildbridge_docker_osx::{
    AppleArchiveProgress, AppleArchiveResult, AppleProjectProgress, AppleSmokeBuildResult,
    AppleWorkspaceSyncResult, ContainerState, GuestDiagnostics, GuestSshStatus, GuestTrustState,
    MacBuilderConfig, RuntimeStatus, SigningProvisioningProgress, SigningProvisioningResult,
    XcodeImportProgress,
};
use buildbridge_runner::{ApiClient, execute};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

mod apple_api;
mod tray;

const CREDENTIAL_SERVICE: &str = "dev.buildbridge.desktop";
const MAC_BUILDER_CREDENTIAL_SERVICE: &str = "dev.buildbridge.desktop.macos-builder";
const MAC_BUILDER_CREDENTIAL_ACCOUNT: &str = "default";

#[derive(Default)]
struct AppState {
    runner_running: AtomicBool,
    builder_running: AtomicBool,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredMacBuilderSecrets {
    app_store_connect_key_id: Option<String>,
    app_store_connect_issuer_id: Option<String>,
    app_store_connect_private_key: Option<String>,
    signing_certificate_path: Option<String>,
    signing_certificate_password: Option<String>,
    provisioning_profile_paths: Vec<String>,
    guest_keychain_password: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MacBuilderSecretsInput {
    app_store_connect_key_id: String,
    app_store_connect_issuer_id: String,
    app_store_connect_private_key_path: String,
    signing_certificate_path: String,
    signing_certificate_password: String,
    provisioning_profile_paths: Vec<String>,
    guest_keychain_password: String,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct MacBuilderSecretSummary {
    app_store_connect_configured: bool,
    app_store_connect_key_id: Option<String>,
    signing_certificate_configured: bool,
    signing_certificate_name: Option<String>,
    provisioning_profile_count: usize,
    guest_keychain_configured: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAppleProfileInput {
    certificate_id: String,
    confirmed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateAppleProfileResult {
    profile: apple_api::AppleProvisioningProfileSummary,
    certificate: apple_api::AppleCertificateSummary,
    saved_path: String,
    secrets: MacBuilderSecretSummary,
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
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct MacGuestAccessView {
    username: Option<String>,
    public_key: Option<String>,
    ssh: GuestSshStatus,
    diagnostics: GuestDiagnostics,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MacBuilderView {
    profile: Option<MacBuilderConfig>,
    runtime: RuntimeStatus,
    secrets: MacBuilderSecretSummary,
    guest: MacGuestAccessView,
    apple_workspace: Option<StoredAppleWorkspace>,
    signing: Option<SigningProvisioningResult>,
    archive: Option<AppleArchiveResult>,
    archive_error: Option<String>,
    logs: Vec<String>,
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
async fn heartbeat_runner(app: AppHandle) -> Result<(), String> {
    let (_, client) = paired_client(&app).await?;

    client
        .heartbeat(&heartbeat_request())
        .await
        .map_err(|error| error.to_string())
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
        .heartbeat(&heartbeat_request())
        .await
        .map_err(|error| error.to_string())?;

    let Some(build) = client.claim().await.map_err(|error| error.to_string())? else {
        return Ok(RunOnceResult {
            state: RunState::Idle,
            build_id: None,
            message: "Connected. No queued builds.".to_string(),
        });
    };

    let execution = execute(&build);
    client
        .append_logs(&build.id, execution.logs)
        .await
        .map_err(|error| error.to_string())?;
    client
        .complete(
            &build.id,
            &CompleteBuildRequest {
                status: execution.status,
                exit_code: Some(execution.exit_code),
                error: execution.error,
            },
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(RunOnceResult {
        state: RunState::Completed,
        build_id: Some(build.id),
        message: "Build completed and reported to the control plane.".to_string(),
    })
}

#[tauri::command]
async fn get_mac_builder_status(app: AppHandle) -> Result<MacBuilderView, String> {
    build_mac_builder_view(&app).await
}

#[tauri::command]
async fn configure_mac_builder(
    app: AppHandle,
    profile: MacBuilderConfig,
) -> Result<MacBuilderView, String> {
    profile.validate().map_err(|error| error.to_string())?;
    ensure_mac_builder_profile_can_change(&app, &profile).await?;
    save_mac_builder_config(&app, &profile)?;

    build_mac_builder_view(&app).await
}

#[tauri::command]
async fn launch_mac_builder(
    state: State<'_, AppState>,
    app: AppHandle,
    profile: MacBuilderConfig,
) -> Result<MacBuilderView, String> {
    profile.validate().map_err(|error| error.to_string())?;
    ensure_mac_builder_profile_can_change(&app, &profile).await?;
    save_mac_builder_config(&app, &profile)?;
    let identity_path = mac_builder_identity_path(&app)?;

    if state.builder_running.swap(true, Ordering::AcqRel) {
        return Err("A macOS builder operation is already running.".to_string());
    }

    let joined = tauri::async_runtime::spawn_blocking(move || {
        buildbridge_docker_osx::launch(&profile, &identity_path).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    state.builder_running.store(false, Ordering::Release);
    let result = joined?;
    result?;
    tray::refresh(&app);

    build_mac_builder_view(&app).await
}

#[tauri::command]
async fn stop_mac_builder(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<MacBuilderView, String> {
    if state.builder_running.swap(true, Ordering::AcqRel) {
        return Err("A macOS builder operation is already running.".to_string());
    }

    let joined = tauri::async_runtime::spawn_blocking(|| {
        buildbridge_docker_osx::stop().map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    state.builder_running.store(false, Ordering::Release);
    let result = joined?;
    result?;
    tray::refresh(&app);

    build_mac_builder_view(&app).await
}

#[tauri::command]
async fn save_mac_builder_secrets(
    input: MacBuilderSecretsInput,
) -> Result<MacBuilderSecretSummary, String> {
    let incoming = normalize_mac_builder_secrets(input)?;
    let existing = read_mac_builder_secrets().await?.unwrap_or_default();
    let secrets = merge_mac_builder_secrets(existing, incoming);
    let summary = summarize_mac_builder_secrets(&secrets);
    store_mac_builder_secrets(secrets).await?;

    Ok(summary)
}

#[tauri::command]
async fn clear_mac_builder_secrets(app: AppHandle) -> Result<MacBuilderSecretSummary, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let entry = mac_builder_credential_entry()?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    })
    .await
    .map_err(|error| error.to_string())??;
    clear_managed_apple_profiles(&app)?;

    Ok(MacBuilderSecretSummary::default())
}

#[tauri::command]
async fn verify_apple_developer_team(
    app: AppHandle,
) -> Result<apple_api::AppleTeamVerificationResult, String> {
    let workspace = load_apple_workspace(&app)?.ok_or_else(|| {
        "Approve an Apple project before verifying its developer team.".to_string()
    })?;
    let development_team = workspace.development_team.ok_or_else(|| {
        "BuildBridge could not detect DEVELOPMENT_TEAM in the approved project.".to_string()
    })?;
    let bundle_identifier = workspace.bundle_identifier.ok_or_else(|| {
        "BuildBridge could not detect PRODUCT_BUNDLE_IDENTIFIER in the approved project."
            .to_string()
    })?;
    let secrets = read_mac_builder_secrets()
        .await?
        .ok_or_else(|| "Store an App Store Connect Team API key first.".to_string())?;
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
    input: CreateAppleProfileInput,
) -> Result<CreateAppleProfileResult, String> {
    if !input.confirmed {
        return Err(
            "Confirm the Apple provisioning-profile creation before continuing.".to_string(),
        );
    }

    let workspace = load_apple_workspace(&app)?.ok_or_else(|| {
        "Approve an Apple project before creating a provisioning profile.".to_string()
    })?;
    let bundle_identifier = workspace.bundle_identifier.ok_or_else(|| {
        "BuildBridge could not detect PRODUCT_BUNDLE_IDENTIFIER in the approved project."
            .to_string()
    })?;
    let mut secrets = read_mac_builder_secrets()
        .await?
        .ok_or_else(|| "Store an App Store Connect Team API key first.".to_string())?;
    if secrets.provisioning_profile_paths.len() >= 20 {
        return Err(
            "The signing kit already retains 20 profiles. Remove obsolete local profile paths before creating another one."
                .to_string(),
        );
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
    store_mac_builder_secrets(secrets.clone())
        .await
        .map_err(|error| {
            format!(
                "Apple created profile {} and saved it at {}, but BuildBridge could not add it to the OS vault: {error}. The Apple profile was not revoked; verify again before retrying.",
                created.profile.name, saved_path_string
            )
        })?;
    let summary = summarize_mac_builder_secrets(&secrets);

    Ok(CreateAppleProfileResult {
        profile: created.profile,
        certificate: created.certificate,
        saved_path: saved_path_string,
        secrets: summary,
    })
}

#[tauri::command]
async fn configure_mac_guest_access(
    app: AppHandle,
    input: MacGuestAccessInput,
) -> Result<MacBuilderView, String> {
    let username = input.username.trim().to_string();
    if !buildbridge_docker_osx::valid_guest_username(&username) {
        return Err(
            "Use the macOS short username: 1–32 letters, numbers, periods, underscores, or hyphens."
                .to_string(),
        );
    }

    let identity_path = mac_guest_identity_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || ensure_mac_guest_keypair(&identity_path))
        .await
        .map_err(|error| error.to_string())??;
    save_mac_guest_access(&app, &StoredMacGuestAccess { username })?;

    build_mac_builder_view(&app).await
}

#[tauri::command]
async fn trust_mac_builder_guest(
    app: AppHandle,
    input: TrustMacGuestInput,
) -> Result<MacBuilderView, String> {
    let profile = load_mac_builder_config(&app)?
        .ok_or_else(|| "Save the macOS builder profile first.".to_string())?;
    let known_hosts_path = mac_guest_known_hosts_path(&app)?;

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

    build_mac_builder_view(&app).await
}

#[tauri::command]
async fn forget_mac_builder_guest_trust(app: AppHandle) -> Result<MacBuilderView, String> {
    let path = mac_guest_known_hosts_path(&app)?;
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }

    build_mac_builder_view(&app).await
}

#[tauri::command]
async fn import_mac_xcode_package(
    state: State<'_, AppState>,
    app: AppHandle,
    input: ImportMacXcodeInput,
) -> Result<ImportMacXcodeResult, String> {
    let package_path = PathBuf::from(input.path.trim());
    if input.path.trim().is_empty() {
        return Err("Drop or enter the absolute path to an Xcode .xip package.".to_string());
    }

    let profile = load_mac_builder_config(&app)?
        .ok_or_else(|| "Save and launch the macOS builder first.".to_string())?;
    let access = load_mac_guest_access(&app)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let current = build_mac_builder_view(&app).await?;
    if current.runtime.state != ContainerState::Running {
        return Err("Launch the macOS builder before importing Xcode.".to_string());
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

    let identity_path = mac_guest_identity_path(&app)?;
    let known_hosts_path = mac_guest_known_hosts_path(&app)?;
    if state.builder_running.swap(true, Ordering::AcqRel) {
        return Err("A macOS builder operation is already running.".to_string());
    }

    let event_app = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        buildbridge_docker_osx::import_xcode_package(
            &package_path,
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            |progress: XcodeImportProgress| {
                let _ = event_app.emit("mac-builder-xcode-import-progress", progress);
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    state.builder_running.store(false, Ordering::Release);
    let imported = joined??;
    let view = build_mac_builder_view(&app).await?;

    Ok(ImportMacXcodeResult {
        view,
        installed_path: imported.installed_path,
        activation_commands: imported.activation_commands,
    })
}

#[tauri::command]
async fn activate_mac_xcode(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<MacBuilderView, String> {
    let profile = load_mac_builder_config(&app)?
        .ok_or_else(|| "Save and launch the macOS builder first.".to_string())?;
    let access = load_mac_guest_access(&app)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let current = build_mac_builder_view(&app).await?;
    if current.runtime.state != ContainerState::Running {
        return Err("Launch the macOS builder before activating Xcode.".to_string());
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

    let identity_path = mac_guest_identity_path(&app)?;
    let known_hosts_path = mac_guest_known_hosts_path(&app)?;
    if state.builder_running.swap(true, Ordering::AcqRel) {
        return Err("A macOS builder operation is already running.".to_string());
    }

    let event_app = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        buildbridge_docker_osx::activate_xcode(
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            |progress: XcodeImportProgress| {
                let _ = event_app.emit("mac-builder-xcode-import-progress", progress);
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    state.builder_running.store(false, Ordering::Release);
    joined??;

    build_mac_builder_view(&app).await
}

#[tauri::command]
async fn provision_mac_signing(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<MacBuilderView, String> {
    let profile = load_mac_builder_config(&app)?
        .ok_or_else(|| "Save and launch the macOS builder first.".to_string())?;
    let access = load_mac_guest_access(&app)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let workspace = load_apple_workspace(&app)?
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
    let current = build_mac_builder_view(&app).await?;
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

    let secrets = read_mac_builder_secrets().await?.ok_or_else(|| {
        "Store a signing certificate and profiles in the OS vault first.".to_string()
    })?;
    let certificate_path = PathBuf::from(
        secrets
            .signing_certificate_path
            .ok_or_else(|| "Choose a .p12 or .pfx signing certificate first.".to_string())?,
    );
    let certificate_password = secrets.signing_certificate_password.ok_or_else(|| {
        "Store the certificate passphrase in the operating-system vault first.".to_string()
    })?;
    let profile_paths = secrets
        .provisioning_profile_paths
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if profile_paths.is_empty() {
        return Err("Choose at least one .mobileprovision profile first.".to_string());
    }
    let keychain_password = secrets.guest_keychain_password.ok_or_else(|| {
        "Store a dedicated guest keychain password in the operating-system vault first.".to_string()
    })?;
    let identity_path = mac_guest_identity_path(&app)?;
    let known_hosts_path = mac_guest_known_hosts_path(&app)?;

    if state.builder_running.swap(true, Ordering::AcqRel) {
        return Err("A macOS builder operation is already running.".to_string());
    }
    if current.signing.is_some()
        && let Err(error) = remove_signing_provisioning_record(&app)
    {
        state.builder_running.store(false, Ordering::Release);
        return Err(error);
    }

    let event_app = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
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
        buildbridge_docker_osx::provision_signing(
            &certificate_path,
            &certificate_password,
            &profile_paths,
            &keychain_password,
            &development_team,
            &bundle_identifier,
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            |progress: SigningProvisioningProgress| {
                let _ = event_app.emit("mac-builder-signing-progress", progress);
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    state.builder_running.store(false, Ordering::Release);
    let result = joined??;
    save_signing_provisioning(
        &app,
        &StoredSigningProvisioning {
            container_id,
            result,
        },
    )?;

    build_mac_builder_view(&app).await
}

#[tauri::command]
async fn clear_mac_guest_signing(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<MacBuilderView, String> {
    let profile = load_mac_builder_config(&app)?
        .ok_or_else(|| "Save and launch the macOS builder first.".to_string())?;
    let access = load_mac_guest_access(&app)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let current = build_mac_builder_view(&app).await?;
    ensure_apple_project_guest_ready(&current)?;
    let profile_uuids = current
        .signing
        .as_ref()
        .ok_or_else(|| "No provisioned signing state is recorded for this guest.".to_string())?
        .profiles
        .iter()
        .map(|profile| profile.uuid.clone())
        .collect::<Vec<_>>();
    let identity_path = mac_guest_identity_path(&app)?;
    let known_hosts_path = mac_guest_known_hosts_path(&app)?;

    if state.builder_running.swap(true, Ordering::AcqRel) {
        return Err("A macOS builder operation is already running.".to_string());
    }
    let joined = tauri::async_runtime::spawn_blocking(move || {
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
    state.builder_running.store(false, Ordering::Release);
    joined??;
    remove_signing_provisioning_record(&app)?;

    build_mac_builder_view(&app).await
}

#[tauri::command]
async fn approve_apple_workspace(
    app: AppHandle,
    input: ApproveAppleWorkspaceInput,
) -> Result<MacBuilderView, String> {
    let approved = inspect_apple_workspace(input.path.trim())?;
    let workspace = match load_apple_workspace(&app)? {
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
    save_apple_workspace(&app, &workspace)?;

    build_mac_builder_view(&app).await
}

#[tauri::command]
async fn clear_apple_workspace(app: AppHandle) -> Result<MacBuilderView, String> {
    let path = apple_workspace_path(&app)?;
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }

    build_mac_builder_view(&app).await
}

#[tauri::command]
async fn sync_apple_workspace(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<SyncAppleWorkspaceResult, String> {
    let profile = load_mac_builder_config(&app)?
        .ok_or_else(|| "Save and launch the macOS builder first.".to_string())?;
    let access = load_mac_guest_access(&app)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let mut workspace = load_apple_workspace(&app)?
        .ok_or_else(|| "Approve a local Apple project first.".to_string())?;
    let current = build_mac_builder_view(&app).await?;
    ensure_apple_project_guest_ready(&current)?;
    let identity_path = mac_guest_identity_path(&app)?;
    let known_hosts_path = mac_guest_known_hosts_path(&app)?;
    let workspace_path = PathBuf::from(&workspace.local_path);
    if state.builder_running.swap(true, Ordering::AcqRel) {
        return Err("A macOS builder operation is already running.".to_string());
    }

    let event_app = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        buildbridge_docker_osx::sync_apple_workspace(
            &workspace_path,
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            |progress: AppleProjectProgress| {
                let _ = event_app.emit("mac-builder-apple-project-progress", progress);
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    state.builder_running.store(false, Ordering::Release);
    let sync = joined??;

    workspace.last_snapshot_sha256 = Some(sync.snapshot_sha256.clone());
    workspace.last_sync_file_count = Some(sync.source_file_count);
    workspace.last_sync_bytes = Some(sync.source_bytes);
    workspace.last_build_succeeded = false;
    workspace.last_xcode_version = None;
    workspace.last_native_lock_updated = false;
    save_apple_workspace(&app, &workspace)?;
    let view = build_mac_builder_view(&app).await?;

    Ok(SyncAppleWorkspaceResult { view, sync })
}

#[tauri::command]
async fn run_apple_smoke_build(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<RunAppleSmokeBuildResult, String> {
    let profile = load_mac_builder_config(&app)?
        .ok_or_else(|| "Save and launch the macOS builder first.".to_string())?;
    let access = load_mac_guest_access(&app)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let mut workspace = load_apple_workspace(&app)?
        .ok_or_else(|| "Approve and synchronize a local Apple project first.".to_string())?;
    if workspace.last_snapshot_sha256.is_none() {
        return Err("Synchronize the approved project before running a test build.".to_string());
    }
    let current = build_mac_builder_view(&app).await?;
    ensure_apple_project_guest_ready(&current)?;
    let identity_path = mac_guest_identity_path(&app)?;
    let known_hosts_path = mac_guest_known_hosts_path(&app)?;
    if state.builder_running.swap(true, Ordering::AcqRel) {
        return Err("A macOS builder operation is already running.".to_string());
    }
    workspace.last_build_succeeded = false;
    workspace.last_xcode_version = None;
    workspace.last_native_lock_updated = false;
    if let Err(error) = save_apple_workspace(&app, &workspace) {
        state.builder_running.store(false, Ordering::Release);
        return Err(error);
    }

    let event_app = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        buildbridge_docker_osx::run_apple_smoke_build(
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            |progress: AppleProjectProgress| {
                let _ = event_app.emit("mac-builder-apple-project-progress", progress);
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    state.builder_running.store(false, Ordering::Release);
    let build = joined??;

    workspace.last_build_succeeded = true;
    workspace.last_xcode_version = Some(build.xcode_version.clone());
    workspace.last_native_lock_updated = build.native_lockfile_updated;
    save_apple_workspace(&app, &workspace)?;
    let view = build_mac_builder_view(&app).await?;

    Ok(RunAppleSmokeBuildResult { view, build })
}

#[tauri::command]
async fn run_apple_signed_archive(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<RunAppleArchiveResult, String> {
    remove_apple_archive_error(&app)?;
    let profile = load_mac_builder_config(&app)?
        .ok_or_else(|| "Save and launch the macOS builder first.".to_string())?;
    let access = load_mac_guest_access(&app)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let workspace = load_apple_workspace(&app)?
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
    let current = build_mac_builder_view(&app).await?;
    ensure_apple_project_guest_ready(&current)?;
    let signing = current
        .signing
        .clone()
        .ok_or_else(|| "Provision and verify signing in macOS first.".to_string())?;
    if workspace.development_team.as_deref() != Some(&signing.development_team)
        || workspace.bundle_identifier.as_deref() != Some(&signing.bundle_identifier)
    {
        return Err(
            "The provisioned signing identity no longer matches the approved project.".to_string(),
        );
    }
    let secrets = read_mac_builder_secrets()
        .await?
        .ok_or_else(|| "Store the signing kit in the OS vault first.".to_string())?;
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
    let output_directory = prepare_apple_archive_output_dir(&app)?;
    let identity_path = mac_guest_identity_path(&app)?;
    let known_hosts_path = mac_guest_known_hosts_path(&app)?;
    let signing_certificate_sha256 = signing.certificate_sha256.clone();
    if state.builder_running.swap(true, Ordering::AcqRel) {
        let _ = fs::remove_dir(&output_directory);
        return Err("A macOS builder operation is already running.".to_string());
    }

    let event_app = app.clone();
    let operation_output_directory = output_directory.clone();
    let scheme = workspace.scheme.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        buildbridge_docker_osx::run_signed_apple_archive(
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            &signing,
            &scheme,
            &keychain_password,
            &operation_output_directory,
            |progress: AppleArchiveProgress| {
                let _ = event_app.emit("mac-builder-apple-archive-progress", progress);
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    state.builder_running.store(false, Ordering::Release);
    let archive = match joined {
        Ok(result) => match result {
            Ok(archive) => archive,
            Err(error) => {
                let _ = fs::remove_dir_all(&output_directory);
                let _ = save_apple_archive_error(&app, &error);
                return Err(error);
            }
        },
        Err(error) => {
            let _ = fs::remove_dir_all(&output_directory);
            let _ = save_apple_archive_error(&app, &error);
            return Err(error);
        }
    };
    if let Err(error) = save_apple_archive(
        &app,
        &StoredAppleArchive {
            container_id,
            snapshot_sha256,
            signing_certificate_sha256,
            result: archive.clone(),
        },
    ) {
        let _ = fs::remove_dir_all(&output_directory);
        let _ = save_apple_archive_error(&app, &error);
        return Err(error);
    }
    remove_apple_archive_error(&app)?;
    let view = build_mac_builder_view(&app).await?;

    Ok(RunAppleArchiveResult { view, archive })
}

#[tauri::command]
async fn reveal_apple_archive(app: AppHandle) -> Result<(), String> {
    let stored = load_apple_archive(&app)?
        .ok_or_else(|| "No retained signed archive is available.".to_string())?;
    let directory = validated_apple_archive_directory(&app, &stored.result)?;
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
async fn clear_apple_archive(app: AppHandle) -> Result<MacBuilderView, String> {
    if let Some(stored) = load_apple_archive(&app)? {
        let directory = validated_apple_archive_directory(&app, &stored.result)?;
        fs::remove_dir_all(directory)
            .map_err(|error| format!("Could not remove the signed artifacts: {error}"))?;
    }
    remove_apple_archive_record(&app)?;
    remove_apple_archive_error(&app)?;

    build_mac_builder_view(&app).await
}

fn ensure_apple_project_guest_ready(view: &MacBuilderView) -> Result<(), String> {
    if view.runtime.state != ContainerState::Running {
        return Err("Launch the macOS builder first.".to_string());
    }
    if view.guest.ssh.trust != GuestTrustState::Trusted || !view.guest.diagnostics.authenticated {
        return Err("Finish the pinned macOS guest connection first.".to_string());
    }
    if !view.guest.diagnostics.xcode_selected {
        return Err("Activate Xcode before preparing an Apple project.".to_string());
    }

    Ok(())
}

async fn build_mac_builder_view(app: &AppHandle) -> Result<MacBuilderView, String> {
    let profile = load_mac_builder_config(app)?;
    let guest_access = load_mac_guest_access(app)?;
    let guest_identity_path = mac_guest_identity_path(app)?;
    let guest_public_key_path = mac_guest_public_key_path(app)?;
    let guest_known_hosts_path = mac_guest_known_hosts_path(app)?;
    let probe_profile = profile.clone();
    let (runtime, logs, guest) = tauri::async_runtime::spawn_blocking(move || {
        let runtime = buildbridge_docker_osx::status().map_err(|error| error.to_string())?;
        let logs = buildbridge_docker_osx::recent_logs().map_err(|error| error.to_string())?;
        let guest = build_mac_guest_view(
            probe_profile.as_ref(),
            &runtime,
            guest_access.as_ref(),
            &guest_identity_path,
            &guest_public_key_path,
            &guest_known_hosts_path,
        )?;

        Ok::<_, String>((runtime, logs, guest))
    })
    .await
    .map_err(|error| error.to_string())??;
    let secrets = read_mac_builder_secrets()
        .await?
        .as_ref()
        .map(summarize_mac_builder_secrets)
        .unwrap_or_default();
    let apple_workspace = load_apple_workspace(app)?;
    let signing = load_signing_provisioning(app)?
        .filter(|stored| runtime.container_id.as_deref() == Some(&stored.container_id))
        .map(|stored| stored.result);
    let archive = load_apple_archive(app)?
        .filter(|stored| {
            std::path::Path::new(&stored.result.ipa.path).is_file()
                && std::path::Path::new(&stored.result.archive.path).is_file()
        })
        .map(|stored| stored.result);
    let archive_error = read_optional_text(&apple_archive_error_path(app)?)?;

    Ok(MacBuilderView {
        profile,
        runtime,
        secrets,
        guest,
        apple_workspace,
        signing,
        archive,
        archive_error,
        logs,
    })
}

fn build_mac_guest_view(
    profile: Option<&MacBuilderConfig>,
    runtime: &RuntimeStatus,
    access: Option<&StoredMacGuestAccess>,
    identity_path: &std::path::Path,
    public_key_path: &std::path::Path,
    known_hosts_path: &std::path::Path,
) -> Result<MacGuestAccessView, String> {
    let username = access.map(|value| value.username.clone());
    let public_key = read_optional_text(public_key_path)?;

    let Some(profile) = profile else {
        return Ok(MacGuestAccessView {
            username,
            public_key,
            ssh: GuestSshStatus {
                issue: Some(
                    "Save and launch a macOS builder before configuring guest access.".to_string(),
                ),
                ..GuestSshStatus::default()
            },
            diagnostics: GuestDiagnostics::default(),
        });
    };

    if runtime.state != ContainerState::Running {
        return Ok(MacGuestAccessView {
            username,
            public_key,
            ssh: GuestSshStatus {
                issue: Some("Launch the macOS builder to probe guest SSH.".to_string()),
                ..GuestSshStatus::default()
            },
            diagnostics: GuestDiagnostics::default(),
        });
    }

    let pinned_host_key = read_optional_text(known_hosts_path)?;
    let ssh =
        buildbridge_docker_osx::guest_ssh_status(profile.ssh_port, pinned_host_key.as_deref());
    let diagnostics = if ssh.trust == GuestTrustState::Trusted {
        match access {
            Some(access) => buildbridge_docker_osx::guest_diagnostics(
                profile.ssh_port,
                &access.username,
                identity_path,
                known_hosts_path,
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
    })
}

async fn ensure_mac_builder_profile_can_change(
    app: &AppHandle,
    profile: &MacBuilderConfig,
) -> Result<(), String> {
    let Some(stored) = load_mac_builder_config(app)? else {
        return Ok(());
    };

    if stored == *profile {
        return Ok(());
    }

    let runtime = tauri::async_runtime::spawn_blocking(buildbridge_docker_osx::status)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;

    if !matches!(
        runtime.state,
        buildbridge_docker_osx::ContainerState::Missing
            | buildbridge_docker_osx::ContainerState::Unavailable
    ) {
        return Err(
            "Stop and deliberately rebuild the existing macOS container before changing its machine profile."
                .to_string(),
        );
    }

    Ok(())
}

fn normalize_mac_builder_secrets(
    input: MacBuilderSecretsInput,
) -> Result<StoredMacBuilderSecrets, String> {
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
    let secrets = StoredMacBuilderSecrets {
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

    if secrets.provisioning_profile_paths.len() > 20 {
        return Err("At most 20 provisioning profiles can be stored.".to_string());
    }

    if let Some(path) = &secrets.signing_certificate_path {
        validate_secret_file(path, &["p12", "pfx"], "signing certificate")?;
    }

    for path in &secrets.provisioning_profile_paths {
        validate_secret_file(path, &["mobileprovision"], "provisioning profile")?;
    }

    for value in [
        secrets.app_store_connect_key_id.as_deref(),
        secrets.app_store_connect_issuer_id.as_deref(),
        secrets.signing_certificate_password.as_deref(),
        secrets.guest_keychain_password.as_deref(),
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

fn merge_mac_builder_secrets(
    existing: StoredMacBuilderSecrets,
    mut incoming: StoredMacBuilderSecrets,
) -> StoredMacBuilderSecrets {
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

fn summarize_mac_builder_secrets(secrets: &StoredMacBuilderSecrets) -> MacBuilderSecretSummary {
    MacBuilderSecretSummary {
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
        provisioning_profile_count: secrets.provisioning_profile_paths.len(),
        guest_keychain_configured: secrets.guest_keychain_password.is_some(),
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

fn heartbeat_request() -> HeartbeatRequest {
    HeartbeatRequest {
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: runner_capabilities(),
    }
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

fn mac_builder_config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("mac-builder.json"))
        .map_err(|error| error.to_string())
}

fn mac_builder_identity_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("macos-builder").join("identity.env"))
        .map_err(|error| error.to_string())
}

fn mac_guest_access_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("macos-builder").join("guest.json"))
        .map_err(|error| error.to_string())
}

fn mac_guest_identity_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("macos-builder").join("guest_ed25519"))
        .map_err(|error| error.to_string())
}

fn mac_guest_public_key_path(app: &AppHandle) -> Result<PathBuf, String> {
    mac_guest_identity_path(app).map(|path| path.with_file_name("guest_ed25519.pub"))
}

fn mac_guest_known_hosts_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("macos-builder").join("known_hosts"))
        .map_err(|error| error.to_string())
}

fn apple_workspace_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("macos-builder").join("apple-workspace.json"))
        .map_err(|error| error.to_string())
}

fn signing_provisioning_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("macos-builder").join("signing.json"))
        .map_err(|error| error.to_string())
}

fn apple_archive_record_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("macos-builder").join("archive.json"))
        .map_err(|error| error.to_string())
}

fn apple_archive_error_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("macos-builder").join("archive-error.txt"))
        .map_err(|error| error.to_string())
}

fn managed_apple_artifacts_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map(|path| path.join("macos-builder").join("artifacts"))
        .map_err(|error| error.to_string())
}

fn prepare_apple_archive_output_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let root = managed_apple_artifacts_dir(app)?;
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

fn managed_apple_profiles_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("macos-builder").join("profiles"))
        .map_err(|error| error.to_string())
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

fn clear_managed_apple_profiles(app: &AppHandle) -> Result<(), String> {
    let directory = managed_apple_profiles_dir(app)?;
    match fs::remove_dir_all(directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "The vault was cleared, but BuildBridge could not remove its managed profile files: {error}"
        )),
    }
}

fn safe_apple_resource_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn load_signing_provisioning(app: &AppHandle) -> Result<Option<StoredSigningProvisioning>, String> {
    let path = signing_provisioning_path(app)?;

    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("The guest signing record is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn load_apple_archive(app: &AppHandle) -> Result<Option<StoredAppleArchive>, String> {
    let path = apple_archive_record_path(app)?;

    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("The signed archive record is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn save_apple_archive(app: &AppHandle, archive: &StoredAppleArchive) -> Result<(), String> {
    let path = apple_archive_record_path(app)?;
    let encoded = serde_json::to_vec_pretty(archive).map_err(|error| error.to_string())?;

    write_restricted_file(&path, &encoded)
}

fn remove_apple_archive_record(app: &AppHandle) -> Result<(), String> {
    let path = apple_archive_record_path(app)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn save_apple_archive_error(app: &AppHandle, error: &str) -> Result<(), String> {
    let error = error
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .take(8_000)
        .collect::<String>();
    write_restricted_file(&apple_archive_error_path(app)?, error.as_bytes())
}

fn remove_apple_archive_error(app: &AppHandle) -> Result<(), String> {
    let path = apple_archive_error_path(app)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn validated_apple_archive_directory(
    app: &AppHandle,
    result: &AppleArchiveResult,
) -> Result<PathBuf, String> {
    let root = managed_apple_artifacts_dir(app)?;
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
    app: &AppHandle,
    signing: &StoredSigningProvisioning,
) -> Result<(), String> {
    let path = signing_provisioning_path(app)?;
    let encoded = serde_json::to_vec_pretty(signing).map_err(|error| error.to_string())?;

    write_restricted_file(&path, &encoded)
}

fn remove_signing_provisioning_record(app: &AppHandle) -> Result<(), String> {
    let path = signing_provisioning_path(app)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn load_apple_workspace(app: &AppHandle) -> Result<Option<StoredAppleWorkspace>, String> {
    let path = apple_workspace_path(app)?;

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

fn save_apple_workspace(app: &AppHandle, workspace: &StoredAppleWorkspace) -> Result<(), String> {
    let path = apple_workspace_path(app)?;
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

fn load_mac_builder_config(app: &AppHandle) -> Result<Option<MacBuilderConfig>, String> {
    let path = mac_builder_config_path(app)?;

    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("The macOS builder configuration is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn save_mac_builder_config(app: &AppHandle, config: &MacBuilderConfig) -> Result<(), String> {
    let path = mac_builder_config_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "The macOS builder configuration directory is unavailable.".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    fs::write(
        path,
        serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn load_mac_guest_access(app: &AppHandle) -> Result<Option<StoredMacGuestAccess>, String> {
    let path = mac_guest_access_path(app)?;

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

fn save_mac_guest_access(app: &AppHandle, access: &StoredMacGuestAccess) -> Result<(), String> {
    let path = mac_guest_access_path(app)?;
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

async fn read_mac_builder_secrets() -> Result<Option<StoredMacBuilderSecrets>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let entry = mac_builder_credential_entry()?;

        match entry.get_password() {
            Ok(encoded) => serde_json::from_str(&encoded)
                .map(Some)
                .map_err(|error| format!("The macOS signing vault entry is invalid: {error}")),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn store_mac_builder_secrets(secrets: StoredMacBuilderSecrets) -> Result<(), String> {
    let encoded = serde_json::to_string(&secrets).map_err(|error| error.to_string())?;

    tauri::async_runtime::spawn_blocking(move || {
        mac_builder_credential_entry()?
            .set_password(&encoded)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
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
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_runner_status,
            pair_runner,
            unpair_runner,
            get_realtime_configuration,
            authorize_realtime,
            heartbeat_runner,
            run_once,
            get_mac_builder_status,
            configure_mac_builder,
            launch_mac_builder,
            stop_mac_builder,
            save_mac_builder_secrets,
            clear_mac_builder_secrets,
            verify_apple_developer_team,
            create_apple_replacement_profile,
            configure_mac_guest_access,
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

    fn empty_secret_input() -> MacBuilderSecretsInput {
        MacBuilderSecretsInput {
            app_store_connect_key_id: String::new(),
            app_store_connect_issuer_id: String::new(),
            app_store_connect_private_key_path: String::new(),
            signing_certificate_path: String::new(),
            signing_certificate_password: String::new(),
            provisioning_profile_paths: Vec::new(),
            guest_keychain_password: String::new(),
        }
    }

    #[test]
    fn app_store_connect_credentials_must_be_complete() {
        let input = MacBuilderSecretsInput {
            app_store_connect_key_id: "KEY123".to_string(),
            ..empty_secret_input()
        };

        let error = normalize_mac_builder_secrets(input).expect_err("partial key must fail");

        assert_eq!(
            error,
            "App Store Connect key ID, issuer ID, and private .p8 path must be provided together."
        );
    }

    #[test]
    fn malformed_app_store_connect_private_key_is_rejected() {
        let private_key_path = write_test_private_key("malformed", "not a private key");
        let input = MacBuilderSecretsInput {
            app_store_connect_key_id: "KEY123".to_string(),
            app_store_connect_issuer_id: "issuer-123".to_string(),
            app_store_connect_private_key_path: private_key_path.display().to_string(),
            ..empty_secret_input()
        };

        let error = normalize_mac_builder_secrets(input).expect_err("invalid PEM must fail");
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
        let input = MacBuilderSecretsInput {
            app_store_connect_issuer_id: "issuer-123".to_string(),
            app_store_connect_private_key_path: private_key_path.display().to_string(),
            ..empty_secret_input()
        };

        let secrets =
            normalize_mac_builder_secrets(input).expect("complete key should be accepted");
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
        let input = MacBuilderSecretsInput {
            app_store_connect_key_id: "DIFFERENT1".to_string(),
            app_store_connect_issuer_id: "issuer-123".to_string(),
            app_store_connect_private_key_path: private_key_path.display().to_string(),
            ..empty_secret_input()
        };

        let error = normalize_mac_builder_secrets(input).expect_err("mismatched key must fail");
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
        let secrets = StoredMacBuilderSecrets {
            app_store_connect_key_id: Some("KEY123".to_string()),
            app_store_connect_issuer_id: Some("issuer-123".to_string()),
            app_store_connect_private_key: Some("private-value".to_string()),
            signing_certificate_path: Some("/secure/signing.p12".to_string()),
            signing_certificate_password: Some("certificate-password".to_string()),
            provisioning_profile_paths: vec!["/secure/app.mobileprovision".to_string()],
            guest_keychain_password: Some("keychain-password".to_string()),
        };

        let summary = summarize_mac_builder_secrets(&secrets);
        let encoded = serde_json::to_string(&summary).expect("summary should serialize");

        assert!(summary.app_store_connect_configured);
        assert_eq!(summary.app_store_connect_key_id.as_deref(), Some("KEY123"));
        assert_eq!(
            summary.signing_certificate_name.as_deref(),
            Some("signing.p12")
        );
        assert_eq!(summary.provisioning_profile_count, 1);
        assert!(!encoded.contains("private-value"));
        assert!(!encoded.contains("certificate-password"));
        assert!(!encoded.contains("keychain-password"));
    }

    #[test]
    fn saving_one_signing_route_preserves_the_other_stored_route() {
        let existing = StoredMacBuilderSecrets {
            app_store_connect_key_id: Some("KEY123".to_string()),
            app_store_connect_issuer_id: Some("issuer-123".to_string()),
            app_store_connect_private_key: Some("private-value".to_string()),
            signing_certificate_path: None,
            signing_certificate_password: None,
            provisioning_profile_paths: Vec::new(),
            guest_keychain_password: None,
        };
        let incoming = StoredMacBuilderSecrets {
            signing_certificate_path: Some("/secure/signing.p12".to_string()),
            signing_certificate_password: Some("certificate-password".to_string()),
            provisioning_profile_paths: vec!["/secure/app.mobileprovision".to_string()],
            guest_keychain_password: Some("keychain-password".to_string()),
            ..StoredMacBuilderSecrets::default()
        };

        let merged = merge_mac_builder_secrets(existing, incoming);

        assert_eq!(merged.app_store_connect_key_id.as_deref(), Some("KEY123"));
        assert_eq!(
            merged.signing_certificate_path.as_deref(),
            Some("/secure/signing.p12")
        );
        assert_eq!(merged.provisioning_profile_paths.len(), 1);
    }

    #[test]
    fn xcode_project_settings_detect_one_team_and_prefer_the_release_bundle() {
        let project = r#"
            DEVELOPMENT_TEAM = F5QA294KSX;
            PRODUCT_BUNDLE_IDENTIFIER = nz.co.thinksolar.app.debug;
            DEVELOPMENT_TEAM = F5QA294KSX;
            PRODUCT_BUNDLE_IDENTIFIER = nz.co.thinksolar.app;
        "#;

        assert_eq!(
            one_xcode_setting(project, "DEVELOPMENT_TEAM").as_deref(),
            Some("F5QA294KSX")
        );
        assert_eq!(
            release_bundle_identifier(project).as_deref(),
            Some("nz.co.thinksolar.app")
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
