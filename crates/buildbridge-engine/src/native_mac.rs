//! Owner-approved native Mac target. Configuration and permissions are separate from managed
//! virtual machines; the desktop, CLI, and runner use the same operations.

use super::*;
use buildbridge_machines::native_mac as native;
use std::path::Path;

pub use buildbridge_machines::native_mac::{
    NativeMacIdentity, NativeMacProfile, NativeMacToolchain,
};

pub const NATIVE_MAC_TARGET_ID: &str = "native-mac";
pub const NATIVE_MAC_PROGRESS_EVENT: &str = "native-mac-progress";
/// Retained build artifacts, under the data directory.
const NATIVE_MAC_BUILDS_DIR: &str = "native-mac/builds";

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NativeMacConfig {
    pub project: Option<NativeMacProject>,
    pub signing: Option<NativeMacSigning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NativeMacProject {
    pub path: String,
    pub name: String,
    pub repository: String,
    pub bundle_identifier: String,
    pub development_team: Option<String>,
    pub scheme: String,
    pub min_xcode_version: Option<String>,
    pub min_ios_sdk_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NativeMacSigning {
    pub identity_sha1: String,
    pub identity_name: String,
    /// An owner-selected local profile, copied into buildbridge's private configuration.
    pub profile_path: String,
    pub profile: NativeMacProfile,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NativeMacProjectInput {
    pub path: String,
    #[serde(default)]
    pub min_xcode_version: Option<String>,
    #[serde(default)]
    pub min_ios_sdk_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NativeMacSigningInput {
    pub identity_sha1: String,
    pub profile_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum NativeMacBuildOutcome {
    Test,
    Archive,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NativeMacBuildInput {
    pub commit: String,
    pub outcome: NativeMacBuildOutcome,
    #[serde(default)]
    pub env_set: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NativeMacBuildResult {
    pub outcome: NativeMacBuildOutcome,
    pub commit: String,
    pub repository: String,
    pub xcode_version: String,
    pub bundle_identifier: String,
    pub env_set: Option<String>,
    pub artifacts: Vec<AppleArchiveArtifact>,
    pub output_tail: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NativeMacStatus {
    pub supported: bool,
    pub toolchain: NativeMacToolchain,
    pub config: NativeMacConfig,
    pub identities: Vec<NativeMacIdentity>,
    pub test_ready: bool,
    pub archive_ready: bool,
    pub issues: Vec<String>,
    pub compatibility_warnings: Vec<String>,
    pub busy: bool,
    pub launch_at_login: bool,
    pub last_build: Option<NativeMacBuildResult>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NativeMacProgress {
    pub machine_id: String,
    pub phase: String,
    pub label: String,
    pub log_line: Option<String>,
}

fn root(app: &Engine) -> PathBuf {
    app.config_dir().join("native-mac")
}
fn config_path(app: &Engine) -> PathBuf {
    root(app).join("config.json")
}
fn builds_root(app: &Engine) -> PathBuf {
    app.data_dir().join(NATIVE_MAC_BUILDS_DIR)
}

#[derive(Clone)]
struct CachedProbe {
    at: std::time::Instant,
    toolchain: NativeMacToolchain,
    identities: Vec<NativeMacIdentity>,
    identity_issue: Option<String>,
}

static PROBE_CACHE: std::sync::OnceLock<Mutex<HashMap<PathBuf, CachedProbe>>> =
    std::sync::OnceLock::new();

fn cached_probe(key: PathBuf) -> CachedProbe {
    let cache = PROBE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(value) = cache
        .get(&key)
        .filter(|value| value.at.elapsed() < std::time::Duration::from_secs(60))
    {
        return value.clone();
    }
    let toolchain = native::probe_native_mac();
    let (identities, identity_issue) = if cfg!(target_os = "macos") {
        match native::native_mac_identities() {
            Ok(values) => (values, None),
            Err(error) => (Vec::new(), Some(error)),
        }
    } else {
        (Vec::new(), None)
    };
    let value = CachedProbe {
        at: std::time::Instant::now(),
        toolchain,
        identities,
        identity_issue,
    };
    if cache.len() >= 8 {
        cache.clear();
    }
    cache.insert(key, value.clone());
    value
}

/// What a profile file looked like when its hash was taken. Status is polled by the desktop
/// and by the runner's heartbeat; the digest tool runs again only when the file changes.
#[derive(Clone, PartialEq, Eq)]
struct ProfileFileKey {
    modified: Option<SystemTime>,
    len: u64,
}

static PROFILE_HASH_CACHE: std::sync::OnceLock<Mutex<HashMap<PathBuf, (ProfileFileKey, String)>>> =
    std::sync::OnceLock::new();

fn profile_file_key(path: &Path) -> Option<ProfileFileKey> {
    let metadata = fs::metadata(path).ok()?;
    Some(ProfileFileKey {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    })
}

/// The profile's SHA-256, keyed by its path, modification time, and length. None when the
/// file is missing or cannot be hashed; a failure is not cached, so the next poll tries again.
fn cached_profile_sha256(path: &Path) -> Option<String> {
    let key = profile_file_key(path)?;
    let cache = PROFILE_HASH_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached, hash)) = cache.get(path)
        && *cached == key
    {
        return Some(hash.clone());
    }
    let hash = buildbridge_machines::native_sha256(path).ok()?;
    if cache.len() >= 8 {
        cache.clear();
    }
    cache.insert(path.to_path_buf(), (key, hash.clone()));
    Some(hash)
}

pub fn load_native_mac_config(app: &Engine) -> Result<NativeMacConfig, String> {
    match fs::read(config_path(app)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("The native Mac configuration is invalid: {e}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(NativeMacConfig::default())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn save_config(app: &Engine, config: &NativeMacConfig) -> Result<(), String> {
    let encoded = serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?;
    let temporary = root(app).join("config.json.pending");
    write_restricted_file(&temporary, &encoded)?;
    fs::rename(temporary, config_path(app)).map_err(|e| e.to_string())?;
    if let Some(cache) = PROBE_CACHE.get()
        && let Ok(mut cache) = cache.lock()
    {
        cache.remove(&config_path(app));
    }
    app.notify_machines_changed();
    Ok(())
}

/// Holds this Mac for one operation; the runner holds it across an artifact upload too.
pub(crate) struct NativeOperation {
    app: Engine,
    scope: Arc<OperationScope>,
    lock: fs::File,
}

impl Drop for NativeOperation {
    fn drop(&mut self) {
        if let Ok(mut busy) = self.app.state().busy_machines.lock() {
            busy.remove(NATIVE_MAC_TARGET_ID);
        }
        if let Ok(mut scopes) = self.app.state().operation_scopes.lock() {
            scopes.remove(NATIVE_MAC_TARGET_ID);
        }
        let _ = self.lock.unlock();
        self.app.notify_machines_changed();
    }
}

/// Like `begin_machine_operation`: the filesystem work happens before the registry lock, which
/// is held only for the in-process check and the non-blocking file lock, and released before
/// the scope registry is touched.
pub(crate) fn begin_native_operation(app: &Engine) -> Result<NativeOperation, String> {
    fs::create_dir_all(root(app)).map_err(|e| e.to_string())?;
    set_restricted_directory_permissions(&root(app))?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root(app).join("operation.lock"))
        .map_err(|e| e.to_string())?;
    let mut busy = app
        .state()
        .busy_machines
        .lock()
        .map_err(|_| "The build registry is unavailable.".to_string())?;
    if busy.contains_key(NATIVE_MAC_TARGET_ID) {
        return Err("Another native Mac operation is still running.".to_string());
    }
    lock.try_lock().map_err(|_| {
        "Another buildbridge process is using this Mac. Wait for it to finish.".to_string()
    })?;
    busy.insert(NATIVE_MAC_TARGET_ID.to_string(), "native_building");
    drop(busy);
    // From here the guard's drop releases the busy entry and the file lock on any failure.
    let operation = NativeOperation {
        app: app.clone(),
        scope: OperationScope::new(),
        lock,
    };
    app.state()
        .operation_scopes
        .lock()
        .map_err(|_| "The build registry is unavailable.".to_string())?
        .insert(
            NATIVE_MAC_TARGET_ID.to_string(),
            Arc::clone(&operation.scope),
        );
    Ok(operation)
}

fn native_lock_alive(lock: &Path) -> bool {
    if !lock.exists() {
        return false;
    }
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock)
        .map_or(true, |file| file.try_lock().is_err())
}

pub async fn cancel_native_mac_build(app: &Engine) -> Result<(), String> {
    let scope = app
        .state()
        .operation_scopes
        .lock()
        .map_err(|_| "The build registry is unavailable.".to_string())?
        .get(NATIVE_MAC_TARGET_ID)
        .cloned()
        .ok_or_else(|| "No native Mac build is running.".to_string())?;
    scope.cancel();
    Ok(())
}

pub async fn native_mac_status(app: &Engine) -> Result<NativeMacStatus, String> {
    let config = load_native_mac_config(app)?;
    let busy = app
        .state()
        .busy_machines
        .lock()
        .map_err(|_| "The build registry is unavailable.".to_string())?
        .contains_key(NATIVE_MAC_TARGET_ID)
        || native_lock_alive(&root(app).join("operation.lock"));
    let launch_at_login = native_mac_login_enabled(app).await?;
    let last_build = match fs::read(root(app).join("last-build.json")) {
        Ok(bytes) => serde_json::from_slice::<NativeMacBuildResult>(&bytes).ok(),
        Err(_) => None,
    };
    let probe_key = config_path(app);
    tokio::task::spawn_blocking(move || {
        let probe = cached_probe(probe_key);
        let toolchain = probe.toolchain;
        let mut issues = toolchain.issues.clone();
        let mut compatibility_warnings = Vec::new();
        let supported = cfg!(target_os = "macos");
        let identities = probe.identities;
        // A keychain failure blocks signing; unsigned compilation remains usable.
        let identity_issue = probe.identity_issue;
        if let Some(project) = &config.project {
            let minimums = check_owner_minimums(project, &toolchain);
            issues.extend(minimums.0);
            compatibility_warnings.extend(minimums.1);
            match inspect_apple_workspace(&project.path) {
                Ok(workspace) => {
                    if workspace.bundle_identifier.as_deref() != Some(&project.bundle_identifier) || workspace.development_team != project.development_team { compatibility_warnings.push("Local checkout: its signing settings differ from the approved project. Requested commits must still match the saved approval; approve the changed project to build its new identity.".to_string()); }
                    let compatibility = check_compatibility(project, Path::new(&project.path), &toolchain);
                    for issue in compatibility.0 { if !issues.contains(&issue) { compatibility_warnings.push(format!("Local checkout: {issue}")); } }
                    for warning in compatibility.1 { if !compatibility_warnings.contains(&warning) { compatibility_warnings.push(format!("Local checkout: {warning}")); } }
                }
                Err(error) => compatibility_warnings.push(format!("Local checkout: {error} Requested commits are fetched and checked separately.")),
            }
        } else { issues.push("Approve a local Capacitor project on this Mac.".to_string()); }
        let test_ready = supported && issues.is_empty() && !busy;
        let mut archive_ready = test_ready;
        if let Some(error) = identity_issue { issues.push(error); archive_ready = false; }
        match (&config.project, &config.signing) {
            (Some(project), Some(signing)) => {
                if !identities.iter().any(|identity| identity.sha1 == signing.identity_sha1) { issues.push("The approved signing identity is unavailable or its keychain is locked.".to_string()); archive_ready = false; }
                if let Err(error) = signing_matches(project, signing) { issues.push(error); archive_ready = false; }
                if supported && cached_profile_sha256(Path::new(&signing.profile_path)).as_deref() != Some(&signing.profile.sha256) { issues.push("The approved provisioning profile is missing or changed. Select it again on this Mac.".to_string()); archive_ready = false; }
                if signing.profile.expires_at_epoch_seconds <= machines::now_epoch_seconds() { issues.push("The approved App Store profile has expired. Select a renewed profile on this Mac.".to_string()); archive_ready = false; }
            }
            _ => { issues.push("Select an existing distribution identity and App Store profile to archive.".to_string()); archive_ready = false; }
        }
        Ok(NativeMacStatus { supported, toolchain, config, identities, test_ready, archive_ready, issues, compatibility_warnings, busy, launch_at_login, last_build })
    }).await.map_err(|e| e.to_string())?
}

/// An explicit owner recheck bypasses the heartbeat's short-lived toolchain cache.
pub async fn refresh_native_mac_status(app: &Engine) -> Result<NativeMacStatus, String> {
    if let Some(cache) = PROBE_CACHE.get()
        && let Ok(mut cache) = cache.lock()
    {
        cache.remove(&config_path(app));
    }
    native_mac_status(app).await
}

pub async fn approve_native_mac_project(
    app: &Engine,
    input: NativeMacProjectInput,
) -> Result<NativeMacConfig, String> {
    native::require_native_mac()?;
    let operation = begin_native_operation(app)?;
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        let _operation = operation;
        validate_minimum(input.min_xcode_version.as_deref())?;
        validate_minimum(input.min_ios_sdk_version.as_deref())?;
        let workspace = inspect_apple_workspace(&input.path)?;
        let repository = native::native_repository_for(Path::new(&workspace.local_path))?;
        let bundle_identifier = workspace.bundle_identifier.ok_or_else(|| {
            "Set a concrete Release bundle identifier in this project's Xcode settings first."
                .to_string()
        })?;
        let project = NativeMacProject {
            path: workspace.local_path,
            name: workspace.name,
            repository,
            bundle_identifier,
            development_team: workspace.development_team,
            scheme: workspace.scheme,
            min_xcode_version: input.min_xcode_version,
            min_ios_sdk_version: input.min_ios_sdk_version,
        };
        let config = NativeMacConfig {
            project: Some(project),
            signing: None,
        };
        save_config(&app, &config)?;
        Ok(config)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub async fn configure_native_mac_signing(
    app: &Engine,
    input: NativeMacSigningInput,
) -> Result<NativeMacConfig, String> {
    native::require_native_mac()?;
    let operation = begin_native_operation(app)?;
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        let _entered = buildbridge_machines::enter_operation(Arc::clone(&operation.scope));
        let _operation = operation;
        let mut config = load_native_mac_config(&app)?;
        let project = config
            .project
            .as_ref()
            .ok_or_else(|| "Approve a project on this Mac first.".to_string())?;
        let identity = native::native_mac_identities()?
            .into_iter()
            .find(|identity| identity.sha1.eq_ignore_ascii_case(&input.identity_sha1))
            .ok_or_else(|| {
                "Choose an available distribution identity from this Mac's keychain.".to_string()
            })?;
        let requested = Path::new(&input.profile_path);
        if !requested.is_absolute() {
            return Err("Choose an absolute local provisioning profile path.".to_string());
        }
        let profile = native::inspect_native_profile(
            requested,
            &root(&app).join("profile-inspection.plist"),
        )?;
        let profile_path = root(&app).join(format!("{}.mobileprovision", profile.sha256));
        let signing = NativeMacSigning {
            identity_sha1: identity.sha1,
            identity_name: identity.name,
            profile_path: profile_path.to_string_lossy().into_owned(),
            profile,
        };
        signing_matches(project, &signing)?;
        let bytes = fs::read(requested).map_err(|e| e.to_string())?;
        write_restricted_file(&profile_path, &bytes)?;
        if buildbridge_machines::native_sha256(&profile_path)? != signing.profile.sha256 {
            let _ = fs::remove_file(&profile_path);
            return Err(
                "The profile changed while it was being approved. Select it again.".to_string(),
            );
        }
        config.signing = Some(signing);
        save_config(&app, &config)?;
        Ok(config)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn signing_matches(project: &NativeMacProject, signing: &NativeMacSigning) -> Result<(), String> {
    if project
        .development_team
        .as_deref()
        .is_some_and(|team| team != signing.profile.team_identifier)
        || signing.profile.application_identifier
            != format!(
                "{}.{}",
                signing.profile.team_identifier, project.bundle_identifier
            )
        || !signing
            .profile
            .certificate_sha1s
            .iter()
            .any(|sha| sha.eq_ignore_ascii_case(&signing.identity_sha1))
    {
        return Err("The selected identity, App Store profile, and approved project must belong to the same team and bundle identifier.".to_string());
    }
    Ok(())
}

fn progress(app: &Engine, phase: &str, label: &str, log_line: Option<String>) {
    let _ = app.emit(
        NATIVE_MAC_PROGRESS_EVENT,
        NativeMacProgress {
            machine_id: NATIVE_MAC_TARGET_ID.to_string(),
            phase: phase.to_string(),
            label: label.to_string(),
            log_line,
        },
    );
}

struct RemoveDirectory(PathBuf);
impl Drop for RemoveDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct InstalledProfiles(Vec<PathBuf>);
impl Drop for InstalledProfiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

fn install_profile(signing: &NativeMacSigning) -> Result<InstalledProfiles, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "The Mac account's home directory is unavailable.".to_string())?;
    let bytes = fs::read(&signing.profile_path).map_err(|e| e.to_string())?;
    let mut installed = InstalledProfiles(Vec::new());
    // Xcode 16 moved its profile store; support both layouts without replacing owner files.
    for relative in [
        "Library/MobileDevice/Provisioning Profiles",
        "Library/Developer/Xcode/UserData/Provisioning Profiles",
    ] {
        let directory = home.join(relative);
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let path = directory.join(format!("{}.mobileprovision", signing.profile.uuid));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                installed.0.push(path.clone());
                file.write_all(&bytes).map_err(|e| e.to_string())?;
                set_restricted_permissions(&path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::read(&path).map_err(|e| e.to_string())? != bytes {
                    return Err("An installed profile has the same UUID with different contents. Resolve it in Xcode before building.".to_string());
                }
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(installed)
}

/// Build inputs select only a saved environment by name. They never supply host paths,
/// signing material, repository URLs, shell commands, or arbitrary environment values.
pub async fn run_native_mac_build(
    app: &Engine,
    input: NativeMacBuildInput,
) -> Result<NativeMacBuildResult, String> {
    run_authorized_native_mac_build(app, input, |_| Ok(())).await
}

/// The runner uses this check to bind its approved grant to the exact local configuration
/// while the native operation lock prevents another client from changing it.
pub(crate) async fn run_authorized_native_mac_build(
    app: &Engine,
    input: NativeMacBuildInput,
    check: impl FnOnce(&NativeMacConfig) -> Result<(), String>,
) -> Result<NativeMacBuildResult, String> {
    native::require_native_mac()?;
    if !native::valid_native_commit(&input.commit) {
        return Err(
            "Enter the full Git commit ID to build (40 or 64 hexadecimal characters).".to_string(),
        );
    }
    let operation = begin_native_operation(app)?;
    let config = load_native_mac_config(app)?;
    check(&config)?;
    let project = config
        .project
        .ok_or_else(|| "Approve a local project on this Mac first.".to_string())?;
    let mut environment = Vec::new();
    let mut secrets = Vec::new();
    let mut dotenv = None;
    if let Some(name) = &input.env_set {
        let stored = read_env_sets().await?;
        let set = stored
            .sets
            .iter()
            .find(|set| &set.name == name)
            .ok_or_else(|| "That environment is not stored on this Mac.".to_string())?;
        for variable in &set.variables {
            if !valid_env_key(&variable.key) || env_value_issue(&variable.value).is_some() {
                return Err("The saved environment contains an invalid variable.".to_string());
            }
            if matches!(
                variable.key.as_str(),
                "PATH"
                    | "HOME"
                    | "USER"
                    | "LOGNAME"
                    | "TMPDIR"
                    | "DEVELOPER_DIR"
                    | "GIT_SSH_COMMAND"
                    | "GIT_CONFIG"
                    | "BASH_ENV"
                    | "ENV"
            ) || variable.key.starts_with("DYLD_")
                || variable.key.starts_with("LD_")
            {
                return Err(format!(
                    "{} changes the native build process itself and cannot be supplied by a build environment.",
                    variable.key
                ));
            }
            environment.push((variable.key.clone(), variable.value.clone()));
            if variable.secret && !variable.value.is_empty() {
                secrets.push(variable.value.clone());
            }
        }
        dotenv = Some(render_dotenv(&set.variables));
    }
    let app = app.clone();
    let redactions = secrets.clone();
    let scope = Arc::clone(&operation.scope);
    let result = tokio::task::spawn_blocking(move || {
        let _entered = buildbridge_machines::enter_operation(Arc::clone(&operation.scope));
        let _operation = operation;
        let toolchain = native::probe_native_mac();
        if !toolchain.issues.is_empty() { return Err(toolchain.issues.join("\n")); }
        let compatible = check_owner_minimums(&project, &toolchain);
        if !compatible.0.is_empty() { return Err(compatible.0.join("\n")); }
        let parent = builds_root(&app);
        fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
        set_restricted_directory_permissions(&parent)?;
        let build_id = format!("{}-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos(), std::process::id());
        let staging = parent.join(format!("{build_id}.working"));
        fs::create_dir(&staging).map_err(|e| e.to_string())?;
        set_restricted_directory_permissions(&staging)?;
        let _cleanup = RemoveDirectory(staging.clone());
        progress(&app, "source", "Fetch the approved source commit", None);
        let checkout = staging.join("Source");
        native::checkout_native_commit(&project.repository, &input.commit, &checkout, |line| progress(&app, "source", "Fetch the approved source commit", Some(line)))?;
        validate_source_links(&checkout)?;
        let fetched = inspect_apple_workspace(checkout.to_str().ok_or_else(|| "The native build directory is not UTF-8.".to_string())?)?;
        if fetched.bundle_identifier.as_deref() != Some(&project.bundle_identifier) || fetched.development_team != project.development_team { return Err("This commit changes the approved project's bundle identifier or team. The Mac owner must approve the updated project first.".to_string()); }
        let compatibility = check_compatibility(&project, &checkout, &toolchain);
        if !compatibility.0.is_empty() { return Err(compatibility.0.join("\n")); }
        for warning in compatibility.1 { progress(&app, "compatibility", "Review project compatibility", Some(warning)); }
        // Checked-in .env files are source and remain so; the explicit saved environment uses
        // the same Vite local production override as the existing Apple preparation recipe.
        if let Some(dotenv) = dotenv {
            let path = checkout.join(".env.production.local");
            if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) { return Err("The saved environment cannot replace a source symlink at .env.production.local.".to_string()); }
            write_restricted_file(&path, dotenv.as_bytes())?;
        }
        let mut signing = if input.outcome == NativeMacBuildOutcome::Archive { Some(config.signing.ok_or_else(|| "Select an existing distribution identity and App Store profile on this Mac first.".to_string())?) } else { None };
        let _profiles = if let Some(signing) = &mut signing {
            let current = native::inspect_native_profile(Path::new(&signing.profile_path), &staging.join("profile.plist"))?;
            if current.sha256 != signing.profile.sha256 { return Err("The approved provisioning profile changed. The Mac owner must approve it again.".to_string()); }
            signing.profile = current;
            signing_matches(&project, signing)?;
            if !native::native_mac_identities()?.iter().any(|identity| identity.sha1 == signing.identity_sha1) { return Err("The approved distribution identity is unavailable. Unlock its keychain on the Mac before building.".to_string()); }
            Some(install_profile(signing)?)
        } else { None };
        let output = staging.join("Artifacts");
        let recipe = native::NativeBuildRecipe { project: &checkout, staging: &staging, output: &output, developer_directory: toolchain.developer_directory.as_deref().ok_or_else(|| "Select Xcode on this Mac first.".to_string())?, bundle_identifier: &project.bundle_identifier, team: signing.as_ref().map(|s| s.profile.team_identifier.as_str()).or(project.development_team.as_deref()).unwrap_or_default(), signing: signing.as_ref().map(|s| (s.identity_sha1.as_str(), &s.profile)), environment: &environment, secrets: &secrets };
        let (mut artifacts, output_tail) = native::run_native_recipe(recipe, |phase, label, line| progress(&app, phase, label, line))?;
        if _operation.scope.is_cancelled() { return Err(CANCELLED_MESSAGE.to_string()); }
        if !artifacts.is_empty() {
            let retained = parent.join(&build_id);
            fs::rename(&output, &retained).map_err(|e| e.to_string())?;
            for artifact in &mut artifacts {
                let name = Path::new(&artifact.path).file_name().ok_or_else(|| "Invalid artifact filename.".to_string())?;
                artifact.path = retained.join(name).to_string_lossy().into_owned();
            }
        }
        let result = NativeMacBuildResult { outcome: input.outcome, commit: input.commit.to_ascii_lowercase(), repository: project.repository, xcode_version: toolchain.xcode_version.unwrap_or_default(), bundle_identifier: project.bundle_identifier, env_set: input.env_set, artifacts, output_tail };
        write_restricted_file(&root(&app).join("last-build.json"), &serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?)?;
        progress(&app, "completed", if result.outcome == NativeMacBuildOutcome::Archive { "Signed native Mac archive complete" } else { "Native Mac compile complete; this does not launch a preview" }, None);
        Ok(result)
    }).await.map_err(|e| e.to_string());
    finish_operation(&scope, result).map_err(|error| {
        error
            .lines()
            .map(|line| native::redact_native_log(line, &redactions))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn validate_source_links(root: &Path) -> Result<(), String> {
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut pending = vec![(root.clone(), 0usize)];
    let mut files = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        if depth > 64 {
            return Err("The source directory nesting is too deep.".to_string());
        }
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_name() == ".git" {
                continue;
            }
            files += 1;
            if files > 100_000 {
                return Err("The source contains more than 100,000 entries.".to_string());
            }
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                let target = fs::canonicalize(entry.path()).map_err(|_| "A source symlink has no existing target; shared builds require links within the checkout.".to_string())?;
                if !target.starts_with(&root) {
                    return Err(
                        "A source symlink points outside the approved checkout.".to_string()
                    );
                }
            } else if kind.is_dir() {
                pending.push((entry.path(), depth + 1));
            }
        }
    }
    Ok(())
}

pub async fn reveal_native_mac_artifacts(app: &Engine, path: String) -> Result<(), String> {
    let artifact = fs::canonicalize(&path)
        .map_err(|_| "This native build artifact is no longer available.".to_string())?;
    let builds = fs::canonicalize(builds_root(app))
        .map_err(|_| "No native Mac artifacts have been retained.".to_string())?;
    let parent = artifact
        .parent()
        .ok_or_else(|| "The artifact folder is invalid.".to_string())?;
    if !artifact.is_file()
        || !parent.starts_with(&builds)
        || parent.parent() != Some(builds.as_path())
        || parent
            .file_name()
            .and_then(|v| v.to_str())
            .is_none_or(|v| v.is_empty() || !v.bytes().all(|c| c.is_ascii_digit() || c == b'-'))
        || !matches!(
            artifact.file_name().and_then(|v| v.to_str()),
            Some("App-AppStore.ipa" | "App.xcarchive.zip")
        )
    {
        return Err("Choose a retained native Mac build artifact.".to_string());
    }
    let directory = parent.to_path_buf();
    tokio::task::spawn_blocking(move || reveal_directory(&directory, "native Mac build artifacts"))
        .await
        .map_err(|e| e.to_string())?
}

const LOGIN_MARKER: &str = "<!-- Managed by buildbridge: open the desktop at login. -->";

fn login_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "The Mac account's home directory is unavailable.".to_string())?;
    Ok(home.join("Library/LaunchAgents/dev.buildbridge.desktop.runner.plist"))
}

pub async fn native_mac_login_enabled(_app: &Engine) -> Result<bool, String> {
    if !cfg!(target_os = "macos") {
        return Ok(false);
    }
    match fs::read_to_string(login_path()?) {
        Ok(contents) => Ok(contents.contains(LOGIN_MARKER)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "Could not read the buildbridge login setting: {error}"
        )),
    }
}

fn login_plist(executable: &str) -> String {
    let executable = executable
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
{LOGIN_MARKER}
<plist version="1.0"><dict>
<key>Label</key><string>dev.buildbridge.desktop.runner</string>
<key>ProgramArguments</key><array><string>{executable}</string></array>
<key>RunAtLoad</key><true/>
<key>LimitLoadToSessionType</key><string>Aqua</string>
</dict></plist>
"#
    )
}

/// The desktop is already running. Persist its next-login launch without starting a second
/// copy now, and without a KeepAlive job that would undo the owner's explicit Quit action.
pub async fn set_native_mac_login(_app: &Engine, enabled: bool) -> Result<(), String> {
    native::require_native_mac()?;
    let path = login_path()?;
    if path.exists()
        && !fs::read_to_string(&path)
            .map_err(|e| e.to_string())?
            .contains(LOGIN_MARKER)
    {
        return Err(
            "Another login item uses buildbridge's filename. It was left unchanged.".to_string(),
        );
    }
    if !enabled {
        if path.exists() {
            fs::remove_file(path).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    let executable = fs::canonicalize(std::env::current_exe().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let bundled = executable
        .parent()
        .is_some_and(|p| p.file_name().is_some_and(|name| name == "MacOS"))
        && executable
            .parent()
            .and_then(Path::parent)
            .is_some_and(|p| p.file_name().is_some_and(|name| name == "Contents"))
        && executable
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .is_some_and(|p| p.extension().is_some_and(|ext| ext == "app"));
    if !bundled {
        return Err("Open an installed buildbridge.app to enable opening at login. A development or CLI executable cannot be registered as the desktop.".to_string());
    }
    let executable = executable
        .to_str()
        .ok_or_else(|| "The desktop executable path is not UTF-8.".to_string())?;
    let temporary = path.with_extension(format!("{}.pending", std::process::id()));
    write_restricted_file(&temporary, login_plist(executable).as_bytes())?;
    fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
    Ok(())
}

fn version_parts(value: &str) -> Option<Vec<u64>> {
    if value.is_empty() || value.len() > 32 {
        return None;
    }
    value
        .split('.')
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                None
            } else {
                part.parse().ok()
            }
        })
        .collect()
}

fn validate_minimum(value: Option<&str>) -> Result<(), String> {
    if value.is_some_and(|v| version_parts(v).is_none()) {
        return Err("Enter a version such as 16, 16.4, or 26.0.".to_string());
    }
    Ok(())
}

fn below(actual: &str, required: &str) -> Option<bool> {
    let mut actual = version_parts(actual)?;
    let mut required = version_parts(required)?;
    let len = actual.len().max(required.len());
    actual.resize(len, 0);
    required.resize(len, 0);
    Some(actual < required)
}

fn check_owner_minimums(
    project: &NativeMacProject,
    toolchain: &NativeMacToolchain,
) -> (Vec<String>, Vec<String>) {
    let mut issues = Vec::new();
    let mut warnings = Vec::new();
    let xcode = toolchain
        .xcode_version
        .as_deref()
        .and_then(|version| version.lines().next())
        .and_then(|line| line.strip_prefix("Xcode "));
    for (label, actual, minimum) in [
        ("Xcode", xcode, project.min_xcode_version.as_deref()),
        (
            "iOS SDK",
            toolchain.ios_sdk.as_deref(),
            project.min_ios_sdk_version.as_deref(),
        ),
    ] {
        if let Some(minimum) = minimum {
            match actual.and_then(|actual| below(actual, minimum)) {
                Some(false) => {}
                Some(true) => issues.push(format!(
                    "This project requires {label} {minimum} or later; this Mac has {}.",
                    actual.unwrap_or("unknown")
                )),
                None => issues.push(format!(
                    "Could not verify this project's minimum {label} {minimum}."
                )),
            }
        } else {
            warnings.push(format!("No minimum {label} was declared by the project owner; compatibility must still be confirmed by a build."));
        }
    }
    (issues, warnings)
}

fn check_compatibility(
    project: &NativeMacProject,
    source: &Path,
    toolchain: &NativeMacToolchain,
) -> (Vec<String>, Vec<String>) {
    let (mut issues, mut warnings) = check_owner_minimums(project, toolchain);
    let xcode = toolchain
        .xcode_version
        .as_deref()
        .and_then(|version| version.lines().next())
        .and_then(|line| line.strip_prefix("Xcode "));
    if let Ok(required) = fs::read_to_string(source.join(".xcode-version")) {
        let required = required.trim();
        if version_parts(required).is_none() {
            issues.push(
                "The committed .xcode-version is not a supported dotted version.".to_string(),
            );
        } else if !xcode.is_some_and(|actual| {
            actual == required
                || actual
                    .strip_prefix(required)
                    .is_some_and(|suffix| suffix.starts_with('.'))
        }) {
            issues.push(format!(
                "The committed .xcode-version selects {required}; the active Xcode is {}.",
                xcode.unwrap_or("unknown")
            ));
        }
    }
    if let Ok(settings) = fs::read_to_string(source.join("ios/App/App.xcodeproj/project.pbxproj")) {
        for deployment in xcode_setting_values(&settings, "IPHONEOS_DEPLOYMENT_TARGET") {
            if toolchain
                .ios_sdk
                .as_deref()
                .and_then(|sdk| below(sdk, &deployment))
                == Some(true)
            {
                issues.push(format!("The committed project targets iOS {deployment}, newer than this Mac's iOS SDK {}.", toolchain.ios_sdk.as_deref().unwrap_or("unknown")));
            }
        }
    }
    match fs::read(source.join("package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    {
        Some(package) => {
            if let Some(required) = package
                .get("packageManager")
                .and_then(serde_json::Value::as_str)
            {
                if let Some(pnpm) = required
                    .strip_prefix("pnpm@")
                    .map(|v| v.split('+').next().unwrap_or(v))
                {
                    if version_parts(pnpm).is_some()
                        && toolchain.pnpm_version.as_deref() != Some(pnpm)
                    {
                        issues.push(format!(
                            "This commit selects pnpm {pnpm}; this Mac has {}.",
                            toolchain.pnpm_version.as_deref().unwrap_or("none")
                        ));
                    }
                } else {
                    issues.push("This native recipe requires pnpm; the commit selects another package manager.".to_string());
                }
            }
            if let Some(required) = package
                .get("engines")
                .and_then(|engines| engines.get("node"))
                .and_then(serde_json::Value::as_str)
            {
                let minimum = required.strip_prefix(">=").map(str::trim);
                if let Some(minimum) = minimum.filter(|v| version_parts(v).is_some()) {
                    if toolchain
                        .node_version
                        .as_deref()
                        .map(|v| v.trim_start_matches('v'))
                        .and_then(|v| below(v, minimum))
                        != Some(false)
                    {
                        issues.push(format!(
                            "This commit requires Node.js {required}; this Mac has {}.",
                            toolchain.node_version.as_deref().unwrap_or("none")
                        ));
                    }
                } else {
                    warnings.push(format!("Node.js requirement {required} must be confirmed by dependency installation; this range is not evaluated automatically."));
                }
            }
        }
        None => issues.push(
            "Could not inspect the commit's package.json before running its build.".to_string(),
        ),
    }
    (issues, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> NativeMacProject {
        NativeMacProject {
            path: "/project".into(),
            name: "test".into(),
            repository: "https://example.com/app.git".into(),
            bundle_identifier: "com.example.app".into(),
            development_team: Some("TEAM123456".into()),
            scheme: "App".into(),
            min_xcode_version: Some("16.4".into()),
            min_ios_sdk_version: Some("18.5".into()),
        }
    }

    #[test]
    fn minimum_versions_compare_numerically_and_reject_expressions() {
        assert_eq!(below("16.10", "16.4"), Some(false));
        assert_eq!(below("16", "16.0.0"), Some(false));
        assert_eq!(below("16.3.9", "16.4"), Some(true));
        assert!(validate_minimum(Some(">=16")).is_err());
        assert!(validate_minimum(Some("16;evil")).is_err());
    }

    #[test]
    fn signing_cannot_cross_approved_project_or_certificate() {
        let mut signing = NativeMacSigning {
            identity_sha1: "A".repeat(40),
            identity_name: "Owner".into(),
            profile_path: "/profile".into(),
            profile: NativeMacProfile {
                uuid: "UUID".into(),
                team_identifier: "TEAM123456".into(),
                application_identifier: "TEAM123456.com.example.app".into(),
                expires_at: "future".into(),
                expires_at_epoch_seconds: u64::MAX,
                sha256: "B".repeat(64),
                certificate_sha1s: vec!["A".repeat(40)],
            },
        };
        assert!(signing_matches(&project(), &signing).is_ok());
        signing.profile.application_identifier = "TEAM123456.com.other.app".into();
        assert!(signing_matches(&project(), &signing).is_err());
        signing.profile.application_identifier = "TEAM123456.com.example.app".into();
        signing.identity_sha1 = "C".repeat(40);
        assert!(signing_matches(&project(), &signing).is_err());
    }

    #[test]
    fn exact_commit_compatibility_blocks_old_xcode_before_scripts() {
        let dir =
            std::env::temp_dir().join(format!("buildbridge-native-compat-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"packageManager":"pnpm@10.0.0","engines":{"node":">=22"}}"#,
        )
        .unwrap();
        let tools = NativeMacToolchain {
            xcode_version: Some("Xcode 16.3\nBuild version example".into()),
            ios_sdk: Some("18.5".into()),
            pnpm_version: Some("10.0.0".into()),
            node_version: Some("v20.0.0".into()),
            ..Default::default()
        };
        let (issues, _) = check_compatibility(&project(), &dir, &tools);
        assert_eq!(issues.len(), 2);
        assert!(issues.iter().any(|v| v.contains("Xcode 16.4")));
        assert!(issues.iter().any(|v| v.contains("Node.js >=22")));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn older_requested_commit_uses_its_own_requirements_not_the_owner_working_tree() {
        let dir = std::env::temp_dir().join(format!(
            "buildbridge-native-commit-requirements-{}",
            std::process::id()
        ));
        let local = dir.join("owner");
        let fetched = dir.join("fetched");
        fs::create_dir_all(&local).unwrap();
        fs::create_dir_all(&fetched).unwrap();
        fs::write(
            local.join("package.json"),
            r#"{"packageManager":"pnpm@11.0.0","engines":{"node":">=24"}}"#,
        )
        .unwrap();
        fs::write(local.join(".xcode-version"), "26").unwrap();
        fs::write(
            fetched.join("package.json"),
            r#"{"packageManager":"pnpm@10.0.0","engines":{"node":">=22"}}"#,
        )
        .unwrap();
        fs::write(fetched.join(".xcode-version"), "16.4").unwrap();
        let tools = NativeMacToolchain {
            xcode_version: Some("Xcode 16.4\nBuild version example".into()),
            ios_sdk: Some("18.5".into()),
            pnpm_version: Some("10.0.0".into()),
            node_version: Some("v22.12.0".into()),
            ..Default::default()
        };
        assert!(check_owner_minimums(&project(), &tools).0.is_empty());
        assert!(!check_compatibility(&project(), &local, &tools).0.is_empty());
        assert!(
            check_compatibility(&project(), &fetched, &tools)
                .0
                .is_empty()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn login_item_escapes_the_executable_and_does_not_restart_after_quit() {
        let plist = login_plist("/Applications/Matt's <Build&Bridge>.app/Contents/MacOS/bridge");
        assert!(plist.contains("Matt&apos;s &lt;Build&amp;Bridge&gt;.app"));
        assert!(plist.contains("<key>ProgramArguments</key><array><string>"));
        assert!(!plist.contains("KeepAlive"));
        assert!(!plist.contains("/bin/sh"));
    }

    #[cfg(unix)]
    #[test]
    fn committed_symlinks_cannot_escape_the_build_checkout() {
        use std::os::unix::fs::symlink;
        let dir =
            std::env::temp_dir().join(format!("buildbridge-native-links-{}", std::process::id()));
        fs::create_dir_all(dir.join("Source")).unwrap();
        fs::write(dir.join("outside"), "private").unwrap();
        fs::write(dir.join("Source/inside"), "source").unwrap();
        symlink("inside", dir.join("Source/link")).unwrap();
        assert!(validate_source_links(&dir.join("Source")).is_ok());
        symlink("../outside", dir.join("Source/escape")).unwrap();
        assert!(validate_source_links(&dir.join("Source")).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_target_lock_is_shared_by_separate_engine_clients_and_recovers_on_drop() {
        let dir =
            std::env::temp_dir().join(format!("buildbridge-native-lock-{}", std::process::id()));
        let engine = || {
            Engine::new(EngineDeps {
                config_dir: dir.join("config"),
                data_dir: dir.join("data"),
                events: Arc::new(NoEvents),
            })
        };
        let first = engine();
        let second = engine();
        let operation = begin_native_operation(&first).unwrap();
        assert!(begin_native_operation(&second).is_err());
        assert!(native_lock_alive(&root(&second).join("operation.lock")));
        drop(operation);
        assert!(!native_lock_alive(&root(&second).join("operation.lock")));
        drop(begin_native_operation(&second).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_operation_registers_busy_and_scope_together_and_refuses_a_second_in_process() {
        let dir = std::env::temp_dir().join(format!(
            "buildbridge-native-registry-{}",
            std::process::id()
        ));
        let engine = Engine::new(EngineDeps {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            events: Arc::new(NoEvents),
        });
        let operation = begin_native_operation(&engine).unwrap();
        assert_eq!(
            begin_native_operation(&engine).err().as_deref(),
            Some("Another native Mac operation is still running.")
        );
        let state = engine.state();
        assert_eq!(
            state
                .busy_machines
                .lock()
                .unwrap()
                .get(NATIVE_MAC_TARGET_ID),
            Some(&"native_building")
        );
        assert!(
            state
                .operation_scopes
                .lock()
                .unwrap()
                .get(NATIVE_MAC_TARGET_ID)
                .is_some_and(|scope| Arc::ptr_eq(scope, &operation.scope))
        );
        drop(operation);
        assert!(
            !state
                .busy_machines
                .lock()
                .unwrap()
                .contains_key(NATIVE_MAC_TARGET_ID)
        );
        assert!(
            !state
                .operation_scopes
                .lock()
                .unwrap()
                .contains_key(NATIVE_MAC_TARGET_ID)
        );
        assert!(!native_lock_alive(&root(&engine).join("operation.lock")));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn profile_hash_is_recomputed_only_when_the_file_changes_size_or_modification_time() {
        use std::time::Duration;
        let dir = std::env::temp_dir().join(format!(
            "buildbridge-native-profile-hash-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let profile = dir.join("profile.mobileprovision");
        let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let write = |bytes: &[u8], at: SystemTime| {
            fs::write(&profile, bytes).unwrap();
            fs::File::options()
                .write(true)
                .open(&profile)
                .unwrap()
                .set_modified(at)
                .unwrap();
        };
        write(b"first", stamp);
        let first = cached_profile_sha256(&profile).unwrap();
        assert_eq!(
            first,
            buildbridge_machines::native_sha256(&profile).unwrap()
        );
        // Same path, length, and modification time: the cached digest stands.
        write(b"other", stamp);
        assert_eq!(cached_profile_sha256(&profile).unwrap(), first);
        // A later modification time reads the file again.
        write(b"other", stamp + Duration::from_secs(1));
        let second = cached_profile_sha256(&profile).unwrap();
        assert_ne!(second, first);
        assert_eq!(
            second,
            buildbridge_machines::native_sha256(&profile).unwrap()
        );
        // So does a different length at the same modification time.
        write(b"longer!", stamp + Duration::from_secs(1));
        assert_ne!(cached_profile_sha256(&profile).unwrap(), second);
        // A missing profile is reported as such, never from the cache.
        assert_eq!(cached_profile_sha256(&dir.join("missing")), None);
        fs::remove_file(&profile).unwrap();
        assert_eq!(cached_profile_sha256(&profile), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
