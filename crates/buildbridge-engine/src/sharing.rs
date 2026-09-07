//! Owner-approved sharing policy, enforced again on the host before every shared build.
use super::*;
use buildbridge_contract::{
    BuildAuthorization, CreateSharingInvitationRequest, SharingGrant, SharingInvitation,
    SharingState, valid_commit,
};
use serde_json::{Value, json};

static POLICY_LOCK: Mutex<()> = Mutex::new(());

fn ensure_shareable_target(machine_id: &str) -> Result<(), String> {
    if machine_id == NATIVE_MAC_TARGET_ID {
        Ok(())
    } else {
        Err("Sharing with other people supports the native Mac destination in this first release. Your own remote builds on managed macOS and Android machines remain available.".to_string())
    }
}

/// How long a writer waits for another buildbridge process to finish with the policy file.
/// A holder rewrites one small JSON file, so contention lasts milliseconds; waiting this long
/// lets a window refresh, the command line and the runner's tick pass each other instead of
/// one of them failing.
const POLICY_LOCK_WAIT: std::time::Duration = std::time::Duration::from_millis(500);
const POLICY_LOCK_POLL: std::time::Duration = std::time::Duration::from_millis(25);

fn policy_file_lock(app: &Engine) -> Result<fs::File, String> {
    policy_file_lock_within(app, POLICY_LOCK_WAIT)
}

fn policy_file_lock_within(app: &Engine, wait: std::time::Duration) -> Result<fs::File, String> {
    fs::create_dir_all(app.config_dir()).map_err(|e| e.to_string())?;
    let path = app.config_dir().join("sharing.lock");
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| error.to_string())?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(POLICY_LOCK_POLL);
            }
            Err(_) => {
                return Err(
                    "Another buildbridge process is updating sharing permissions. Try again shortly."
                        .to_string(),
                );
            }
        }
    }
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SharingOverview {
    pub paused: bool,
    pub state: SharingState,
    pub targets: Vec<MachineReport>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ShareMachineInput {
    pub machine_id: String,
    pub env_sets: Vec<String>,
    pub access_hours: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LocalSharing {
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    policies: Vec<LocalPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalPolicy {
    id: String,
    runner_id: String,
    server_url: String,
    machine_id: String,
    repository: String,
    bundle_identifier: Option<String>,
    kinds: Vec<BuildKind>,
    env_sets: Vec<String>,
    /// Owner-local project and identity configuration. No credentials or paths go to the server.
    pin: Value,
    approval: Option<LocalApproval>,
    #[serde(default)]
    created_at_epoch_seconds: u64,
    #[serde(default)]
    access_hours: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalApproval {
    grant_id: String,
    requester_id: String,
    expires_at: String,
}

fn policy_path(app: &Engine) -> PathBuf {
    app.config_dir().join("sharing.json")
}

fn read_local(app: &Engine) -> Result<LocalSharing, String> {
    match fs::read(policy_path(app)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "The local sharing permissions could not be read. Sharing is disabled until they are repaired.".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(LocalSharing::default()),
        Err(error) => Err(error.to_string()),
    }
}

fn update_local(
    app: &Engine,
    update: impl FnOnce(&mut LocalSharing) -> Result<(), String>,
) -> Result<(), String> {
    let _guard = POLICY_LOCK
        .lock()
        .map_err(|_| "The sharing permissions are busy.".to_string())?;
    let _file_guard = policy_file_lock(app)?;
    let mut state = read_local(app)?;
    update(&mut state)?;
    fs::create_dir_all(app.config_dir()).map_err(|error| error.to_string())?;
    let temp = app
        .config_dir()
        .join(format!("sharing-{}.tmp", generated_password()?));
    let result = (|| {
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|error| error.to_string())?;
        file.write_all(&serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temp, policy_path(app)).map_err(|error| error.to_string())
    })();
    let _ = fs::remove_file(temp);
    result
}

pub(crate) fn sharing_paused(app: &Engine) -> bool {
    read_local(app).map(|state| state.paused).unwrap_or(true)
}

pub async fn get_sharing(app: &Engine) -> Result<SharingOverview, String> {
    let (_, client) = paired_client(app).await?;
    let mut remote = client.sharing().await.map_err(|error| error.to_string())?;
    // A read is a read: the file is rewritten only when the server's state has retired a
    // local policy, so refreshing the window beside the runner's tick or the command line
    // does not contend for the writer's lock.
    let mut local = read_local(app)?;
    if prune_policies(&mut local, &remote) {
        update_local(app, |state| {
            prune_policies(state, &remote);
            Ok(())
        })?;
        local = read_local(app)?;
    }
    for grant in &mut remote.grants {
        if grant.status != "approved" {
            continue;
        }
        match local
            .policies
            .iter()
            .find(|policy| policy.id == grant.policy_id)
        {
            None => grant.status = "revoked".to_string(),
            Some(policy)
                if !policy.approval.as_ref().is_some_and(|approval| {
                    approval.grant_id == grant.id
                        && approval.requester_id == grant.requester_id
                        && grant.expires_at.as_deref() == Some(&approval.expires_at)
                        && future_expiry(&approval.expires_at)
                }) =>
            {
                grant.status = "pending".to_string()
            }
            Some(_) => {}
        }
    }
    Ok(SharingOverview {
        paused: sharing_paused(app),
        state: remote,
        targets: machine_reports(app)
            .await
            .into_iter()
            .filter(|target| target.id == NATIVE_MAC_TARGET_ID)
            .collect(),
    })
}

pub async fn set_sharing_paused(app: &Engine, paused: bool) -> Result<(), String> {
    update_local(app, |state| {
        state.paused = paused;
        Ok(())
    })?;
    // Local policy is authoritative even while the server cannot be reached.
    let _ = heartbeat_runner(app).await;
    Ok(())
}

pub async fn create_sharing_invitation(
    app: &Engine,
    input: ShareMachineInput,
) -> Result<SharingInvitation, String> {
    crate::settings::require_remote_builds(app)?;
    ensure_shareable_target(&input.machine_id)?;
    if !(1..=720).contains(&input.access_hours)
        || input.env_sets.is_empty()
        || input.env_sets.len() > MAX_ENV_SETS + 1
    {
        return Err(
            "Choose at least one environment and an access duration between 1 hour and 30 days."
                .to_string(),
        );
    }
    if sharing_paused(app) {
        return Err("Resume sharing before creating an invitation.".to_string());
    }
    let (config, client) = paired_client(app).await?;
    let targets = machine_reports(app).await;
    let target = targets
        .iter()
        .find(|target| target.id == input.machine_id)
        .ok_or("This build destination is unavailable.")?;
    // A report's readiness folds in whether the Mac is busy; a Mac in the middle of a build is
    // set up fine and only needs to finish, so it is told apart from one missing its setup.
    if target.id == NATIVE_MAC_TARGET_ID
        && native_mac_status(app).await.is_ok_and(|status| status.busy)
    {
        return Err(
            "This Mac is busy with another operation. Wait for it to finish before sharing it."
                .to_string(),
        );
    }
    if !target.ready {
        return Err(
            "Complete this destination's project and signing setup before sharing it.".to_string(),
        );
    }
    let repository = target
        .repository
        .clone()
        .filter(|value| !value.is_empty())
        .ok_or("Approve a project with a Git repository before sharing this destination.")?;
    let mut env_sets = input.env_sets;
    env_sets.sort();
    env_sets.dedup();
    if env_sets
        .iter()
        .any(|name| !name.is_empty() && !target.env_sets.contains(name))
    {
        return Err("An environment is no longer available on this host.".to_string());
    }
    let kind = if target.platform.as_deref() == Some("android") {
        BuildKind::AndroidRelease
    } else {
        BuildKind::AppleArchive
    };
    let remote = client.sharing().await.map_err(|error| error.to_string())?;
    let policy = LocalPolicy {
        id: generated_password()?,
        runner_id: config.runner_id,
        server_url: config.server_url,
        machine_id: target.id.clone(),
        repository,
        bundle_identifier: target.bundle_identifier.clone(),
        kinds: vec![kind],
        env_sets,
        pin: target_pin(app, &target.id).await?,
        approval: None,
        created_at_epoch_seconds: machines::now_epoch_seconds(),
        access_hours: input.access_hours,
    };
    update_local(app, |state| {
        prune_policies(state, &remote);
        if state.policies.len() >= 128 {
            return Err("This host already has 128 active invitations or access requests. Revoke unused access, or wait for unused invitation codes to expire.".to_string());
        }
        state.policies.push(policy.clone());
        Ok(())
    })?;
    let result = async {
        client
            .heartbeat(&heartbeat_request(app).await)
            .await
            .map_err(|error| error.to_string())?;
        let invitation = client
            .create_sharing_invitation(&CreateSharingInvitationRequest {
                machine_id: policy.machine_id.clone(),
                kinds: policy.kinds.clone(),
                env_sets: policy.env_sets.clone(),
                expires_in_minutes: 10,
                access_hours: input.access_hours,
                policy_id: policy.id.clone(),
            })
            .await
            .map_err(|error| error.to_string())?;
        if invitation.policy_id != policy.id
            || invitation.machine_id != policy.machine_id
            || invitation.repository != policy.repository
            || invitation.bundle_identifier != policy.bundle_identifier
            || invitation.kinds != policy.kinds
            || !same_values(&invitation.env_sets, &policy.env_sets)
            || invitation.access_hours != policy.access_hours
            || !future_expiry(&invitation.expires_at)
        {
            return Err(
                "The returned invitation does not match the owner's requested permissions."
                    .to_string(),
            );
        }
        Ok(invitation)
    }
    .await;
    if result.is_err() {
        update_local(app, |state| {
            state.policies.retain(|stored| stored.id != policy.id);
            Ok(())
        })?;
    }
    result
}

/// Drops the local policies the server no longer backs, and says whether any went.
fn prune_policies(state: &mut LocalSharing, remote: &SharingState) -> bool {
    let now = machines::now_epoch_seconds();
    let before = state.policies.len();
    state.policies.retain(|policy| {
        if ensure_shareable_target(&policy.machine_id).is_err() {
            return false;
        }
        if let Some(approval) = &policy.approval {
            return future_expiry(&approval.expires_at);
        }
        // A newly-created local policy can precede the server response. Do not race that
        // in-flight request; older unused codes are removable once the server drops them.
        if now.saturating_sub(policy.created_at_epoch_seconds) < 60 {
            return true;
        }
        remote.grants.iter().any(|grant| {
            grant.policy_id == policy.id && matches!(grant.status.as_str(), "pending" | "approved")
        }) || remote.invitations.iter().any(|invite| {
            invite.policy_id == policy.id
                && future_expiry(&invite.expires_at)
                && !matches!(invite.status.as_str(), "revoked" | "expired" | "cancelled")
        })
    });
    state.policies.len() != before
}

fn matching_policy<'a>(
    state: &'a LocalSharing,
    grant: &SharingGrant,
    config: &StoredConfig,
) -> Result<&'a LocalPolicy, String> {
    ensure_shareable_target(&grant.machine_id)?;
    let policy = state
        .policies
        .iter()
        .find(|policy| policy.id == grant.policy_id)
        .ok_or("This invitation was not created on this host. Create a new invitation here.")?;
    if policy.runner_id != config.runner_id
        || policy.server_url != config.server_url
        || policy.machine_id != grant.machine_id
        || policy.repository != grant.repository
        || policy.bundle_identifier != grant.bundle_identifier
        || policy.kinds != grant.kinds
        || !same_values(&policy.env_sets, &grant.env_sets)
    {
        return Err(
            "The invitation's permissions changed. Create a new invitation on this host."
                .to_string(),
        );
    }
    if !(1..=720).contains(&policy.access_hours)
        || policy.approval.as_ref().is_some_and(|approval| {
            approval.grant_id != grant.id || approval.requester_id != grant.requester_id
        })
    {
        return Err("This invitation was already used, or its original access duration is unavailable. Create a new invitation.".to_string());
    }
    Ok(policy)
}

pub async fn approve_sharing_grant(app: &Engine, grant_id: String) -> Result<(), String> {
    let (config, client) = paired_client(app).await?;
    let remote = client.sharing().await.map_err(|error| error.to_string())?;
    let pending = remote
        .grants
        .iter()
        .find(|grant| grant.id == grant_id)
        .ok_or("This access request is no longer available.")?;
    if !matches!(pending.status.as_str(), "pending" | "approved") {
        return Err("This access request has been revoked.".to_string());
    }
    let policy = matching_policy(&read_local(app)?, pending, &config)?.clone();
    if target_pin(app, &policy.machine_id).await? != policy.pin {
        return Err("The project or signing identity changed since this invitation was created. Create a new invitation.".to_string());
    }
    let approved = client
        .approve_sharing_grant(&grant_id)
        .await
        .map_err(|error| error.to_string())?;
    if approved.id != pending.id
        || approved.requester_id != pending.requester_id
        || approved.status != "approved"
    {
        return Err(
            "The access request changed while it was being approved. Refresh and review it again."
                .to_string(),
        );
    }
    matching_policy(&read_local(app)?, &approved, &config)?;
    let expiry = approved
        .expires_at
        .clone()
        .filter(|expiry| future_expiry(expiry))
        .ok_or("The server returned an invalid access expiry.")?;
    if !expiry_within_owner_limit(&expiry, policy.access_hours)
        || policy
            .approval
            .as_ref()
            .is_some_and(|existing| existing.expires_at != expiry)
    {
        return Err("The server's access expiry exceeds or changes the duration approved by the owner. Create a new invitation.".to_string());
    }
    update_local(app, |state| {
        let policy = state
            .policies
            .iter_mut()
            .find(|p| p.id == approved.policy_id)
            .ok_or("The local invitation is no longer available.")?;
        policy.approval = Some(LocalApproval {
            grant_id: approved.id,
            requester_id: approved.requester_id,
            expires_at: expiry,
        });
        Ok(())
    })
}

pub async fn revoke_sharing_grant(app: &Engine, grant_id: String) -> Result<(), String> {
    // Deny locally first; a failed network request must never leave local access enabled.
    update_local(app, |state| {
        state.policies.retain(|policy| {
            policy
                .approval
                .as_ref()
                .is_none_or(|approval| approval.grant_id != grant_id)
        });
        Ok(())
    })?;
    let (_, client) = paired_client(app).await?;
    client
        .revoke_sharing_grant(&grant_id)
        .await
        .map_err(|error| {
            format!("Access is disabled on this host, but the server could not be updated: {error}")
        })
}

fn same_values(left: &[String], right: &[String]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .all(|value| right.iter().filter(|other| *other == value).count() == 1)
        && right
            .iter()
            .all(|value| left.iter().filter(|other| *other == value).count() == 1)
}

fn expiry_within_owner_limit(expiry: &str, hours: u32) -> bool {
    (1..=720).contains(&hours)
        && time::OffsetDateTime::parse(expiry, &time::format_description::well_known::Rfc3339)
            .is_ok_and(|expiry| {
                let now = machines::now_epoch_seconds() as i64;
                expiry.unix_timestamp() > now
                    && expiry.unix_timestamp() <= now + i64::from(hours) * 3600
            })
}

fn future_expiry(value: &str) -> bool {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).is_ok_and(
        |expiry| {
            expiry.unix_timestamp()
                > SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|v| v.as_secs() as i64)
                    .unwrap_or(i64::MAX)
        },
    )
}

fn validate_authorization<'a>(
    state: &'a LocalSharing,
    auth: &BuildAuthorization,
    config: &StoredConfig,
    kind: BuildKind,
    machine_id: &str,
    commit: Option<&str>,
    env_set: &str,
) -> Result<&'a LocalPolicy, String> {
    ensure_shareable_target(machine_id)?;
    if state.paused {
        return Err("The owner has paused shared builds on this host.".to_string());
    }
    if !commit.is_some_and(valid_commit) {
        return Err(
            "Shared builds require the full Git commit hash of an approved repository.".to_string(),
        );
    }
    let policy = state
        .policies
        .iter()
        .find(|policy| policy.id == auth.policy_id)
        .ok_or("The sharing invitation is not approved on this host.")?;
    let approved = policy.approval.as_ref().ok_or(
        "The owner has not approved this person on this host, or has revoked their access.",
    )?;
    if policy.runner_id != config.runner_id
        || policy.server_url != config.server_url
        || approved.grant_id != auth.grant_id
        || approved.requester_id != auth.requester_id
        || approved.expires_at != auth.expires_at
        || !future_expiry(&approved.expires_at)
        || machine_id != policy.machine_id
        || auth.machine_id != policy.machine_id
        || policy.repository != auth.repository
        || policy.bundle_identifier != auth.bundle_identifier
        || !policy.kinds.contains(&kind)
        || policy.kinds != auth.kinds
        || !same_values(&policy.env_sets, &auth.env_sets)
        || !policy.env_sets.iter().any(|name| name == env_set)
    {
        return Err("This build is outside the owner's approved sharing permissions, or access has expired.".to_string());
    }
    Ok(policy)
}

pub(crate) async fn authorize_shared_build(
    app: &Engine,
    build: &ClaimedBuild,
) -> Result<(), String> {
    let Some(auth) = &build.authorization else {
        return Ok(());
    };
    if build.lease_token.is_none() {
        return Err("Shared builds require a current claim token.".to_string());
    }
    let payload = AppleArchivePayload::from_value(&build.payload)?;
    let env = match payload.env_set.as_deref() {
        Some(name) => name.to_string(),
        None if payload.machine_id == NATIVE_MAC_TARGET_ID => String::new(),
        None => match attached_env_set_id(app, &payload.machine_id)? {
            Some(id) => read_env_sets()
                .await?
                .sets
                .into_iter()
                .find(|set| set.id == id)
                .map(|set| set.name)
                .ok_or("The attached environment no longer exists.")?,
            None => String::new(),
        },
    };
    let config = load_config(app)?.ok_or("This host is no longer paired.")?;
    let state = read_local(app)?;
    let policy = validate_authorization(
        &state,
        auth,
        &config,
        build.kind,
        &payload.machine_id,
        payload.git_ref.as_deref(),
        &env,
    )?;
    if target_pin(app, &payload.machine_id).await? != policy.pin {
        return Err("The approved project or signing identity changed. The owner must create a new sharing invitation.".to_string());
    }
    Ok(())
}

async fn target_pin(app: &Engine, machine_id: &str) -> Result<Value, String> {
    if machine_id == NATIVE_MAC_TARGET_ID {
        return serde_json::to_value(load_native_mac_config(app)?)
            .map_err(|error| error.to_string());
    }
    let machine = machines::load_registry(app)?.find(machine_id)?.clone();
    let paths = MachinePaths::resolve(app, machine_id)?;
    let kit = resolve_signing_kit_for(app, machine_id).await?;
    if machine.config.provider == MachineProvider::AndroidToolchain {
        let project = load_android_workspace(&paths)?.ok_or("No Android project is approved.")?;
        let path = kit
            .android_keystore_path
            .as_deref()
            .ok_or("An Android upload key is required.")?;
        let hash = buildbridge_machines::native_mac::native_sha256(std::path::Path::new(path))?;
        Ok(
            json!({"provider":machine.config.provider,"project":project.local_path,"repository":project_remote_url(&project.local_path),"bundle":project.application_id,"kit":kit.id,"alias":kit.android_key_alias,"identity":hash}),
        )
    } else {
        let project = load_apple_workspace(&paths)?.ok_or("No Apple project is approved.")?;
        Ok(
            json!({"provider":machine.config.provider,"project":project.local_path,"repository":project_remote_url(&project.local_path),"bundle":project.bundle_identifier,"team":project.development_team,"kit":kit.id,"signing":load_signing_provisioning(&paths)?}),
        )
    }
}

/// Called under the native target's configuration lock, closing the gap between claim
/// authorization and a local owner changing the selected project or signing identity.
pub(crate) fn authorize_native_snapshot(
    app: &Engine,
    build: &ClaimedBuild,
    native: &NativeMacConfig,
) -> Result<(), String> {
    let Some(auth) = &build.authorization else {
        return Ok(());
    };
    let payload = AppleArchivePayload::from_value(&build.payload)?;
    let config = load_config(app)?.ok_or("This host is no longer paired.")?;
    let state = read_local(app)?;
    let policy = validate_authorization(
        &state,
        auth,
        &config,
        build.kind,
        &payload.machine_id,
        payload.git_ref.as_deref(),
        payload.env_set.as_deref().unwrap_or(""),
    )?;
    if serde_json::to_value(native).map_err(|error| error.to_string())? != policy.pin {
        return Err(
            "The project or signing identity changed after this sharing invitation was created."
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (LocalSharing, BuildAuthorization, StoredConfig) {
        let expiry = "2999-01-01T00:00:00Z".to_string();
        let state = LocalSharing {
            paused: false,
            policies: vec![LocalPolicy {
                id: "policy".into(),
                runner_id: "runner".into(),
                server_url: "https://example.test".into(),
                machine_id: "native-mac".into(),
                repository: "git@example.test:app.git".into(),
                bundle_identifier: Some("dev.example.app".into()),
                kinds: vec![BuildKind::AppleArchive],
                env_sets: vec!["production".into()],
                pin: Value::Null,
                approval: Some(LocalApproval {
                    grant_id: "grant".into(),
                    requester_id: "person".into(),
                    expires_at: expiry.clone(),
                }),
                created_at_epoch_seconds: 1,
                access_hours: 1,
            }],
        };
        let auth = BuildAuthorization {
            grant_id: "grant".into(),
            requester_id: "person".into(),
            policy_id: "policy".into(),
            machine_id: "native-mac".into(),
            project: None,
            repository: "git@example.test:app.git".into(),
            bundle_identifier: Some("dev.example.app".into()),
            kinds: vec![BuildKind::AppleArchive],
            env_sets: vec!["production".into()],
            expires_at: expiry,
        };
        let config = StoredConfig {
            runner_id: "runner".into(),
            runner_name: "Mac".into(),
            server_url: "https://example.test".into(),
        };
        (state, auth, config)
    }
    fn permitted(state: &LocalSharing, auth: &BuildAuthorization, config: &StoredConfig) -> bool {
        validate_authorization(
            state,
            auth,
            config,
            BuildKind::AppleArchive,
            "native-mac",
            Some(&"a".repeat(40)),
            "production",
        )
        .is_ok()
    }
    #[test]
    fn only_the_locally_approved_person_and_scope_can_build() {
        let (state, auth, config) = fixture();
        assert!(permitted(&state, &auth, &config));
        let mut changed = auth.clone();
        changed.requester_id = "other".into();
        assert!(!permitted(&state, &changed, &config));
        changed = auth.clone();
        changed.repository = "git@example.test:other.git".into();
        assert!(!permitted(&state, &changed, &config));
        changed = auth.clone();
        changed.env_sets.push("secret".into());
        assert!(!permitted(&state, &changed, &config));
        assert!(
            validate_authorization(
                &state,
                &auth,
                &config,
                BuildKind::AndroidRelease,
                "native-mac",
                Some(&"a".repeat(40)),
                "production"
            )
            .is_err()
        );
        assert!(
            validate_authorization(
                &state,
                &auth,
                &config,
                BuildKind::AppleArchive,
                "native-mac",
                Some("main"),
                "production"
            )
            .is_err()
        );
        assert!(
            validate_authorization(
                &state,
                &auth,
                &config,
                BuildKind::AppleArchive,
                "native-mac",
                Some(&"a".repeat(40)),
                ""
            )
            .is_err()
        );
    }
    #[test]
    fn pause_revocation_expiry_and_repairing_fail_closed() {
        let (mut state, mut auth, mut config) = fixture();
        state.paused = true;
        assert!(!permitted(&state, &auth, &config));
        state.paused = false;
        config.runner_id = "new-runner".into();
        assert!(!permitted(&state, &auth, &config));
        config.runner_id = "runner".into();
        auth.expires_at = "2000-01-01T00:00:00Z".into();
        state.policies[0].approval.as_mut().unwrap().expires_at = auth.expires_at.clone();
        assert!(!permitted(&state, &auth, &config));
        state.policies[0].approval = None;
        assert!(!permitted(&state, &auth, &config));
    }
    #[test]
    fn server_cannot_extend_owner_selected_access_duration() {
        let future = |seconds: i64| {
            (time::OffsetDateTime::now_utc() + time::Duration::seconds(seconds))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()
        };
        assert!(expiry_within_owner_limit(&future(3500), 1));
        assert!(!expiry_within_owner_limit(&future(7200), 1));
        assert!(!expiry_within_owner_limit(&future(10), 0));
        assert!(!same_values(
            &["a".into(), "b".into()],
            &["a".into(), "a".into()]
        ));
    }
    #[test]
    fn expired_unused_policies_are_pruned_but_pending_review_is_kept() {
        let (mut state, _, _) = fixture();
        state.policies[0].approval = None;
        let remote = SharingState {
            protocol_version: PROTOCOL_VERSION,
            invitations: vec![],
            grants: vec![],
        };
        assert!(prune_policies(&mut state, &remote), "a retired policy went");
        assert!(state.policies.is_empty());
        assert!(
            !prune_policies(&mut state, &remote),
            "nothing left to prune"
        );
        let (mut state, _, _) = fixture();
        state.policies[0].approval = None;
        state.policies[0].created_at_epoch_seconds = machines::now_epoch_seconds();
        assert!(
            !prune_policies(&mut state, &remote),
            "a kept policy is no change"
        );
        assert_eq!(state.policies.len(), 1);
        let (mut state, _, _) = fixture();
        assert!(
            !prune_policies(&mut state, &remote),
            "an approved policy stays"
        );
    }

    #[test]
    fn a_writer_waits_briefly_for_another_process_holding_the_policy_file() {
        let root = std::env::temp_dir().join(format!(
            "buildbridge-policy-lock-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let app = Engine::new(EngineDeps {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            events: Arc::new(NoEvents),
        });
        let held = policy_file_lock_within(&app, std::time::Duration::ZERO).unwrap();
        // The lock is held through a separate file description, as another process holds it.
        let error =
            policy_file_lock_within(&app, std::time::Duration::from_millis(60)).unwrap_err();
        assert!(error.contains("Another buildbridge process"), "{error}");
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(120));
            drop(held);
        });
        let started = std::time::Instant::now();
        let taken = policy_file_lock_within(&app, std::time::Duration::from_secs(5))
            .expect("the lock frees up inside the wait");
        assert!(started.elapsed() >= std::time::Duration::from_millis(100));
        release.join().unwrap();
        drop(taken);
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn managed_sharing_is_disabled_without_changing_owner_remote_builds() {
        let (mut state, mut auth, config) = fixture();
        state.policies[0].machine_id = "managed-mac".to_string();
        auth.machine_id = "managed-mac".to_string();
        let error = validate_authorization(
            &state,
            &auth,
            &config,
            BuildKind::AppleArchive,
            "managed-mac",
            Some(&"a".repeat(40)),
            "production",
        )
        .unwrap_err();
        assert!(error.contains("native Mac destination"));
        let root = std::env::temp_dir().join("buildbridge-unshared-managed-test");
        let app = Engine::new(EngineDeps {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            events: Arc::new(NoEvents),
        });
        let build = ClaimedBuild {
            id: "owner-build".into(),
            kind: BuildKind::AppleArchive,
            payload: json!({"machine_id":"managed-mac"}),
            lease_expires_at: "future".into(),
            next_log_sequence: 1,
            authorization: None,
            lease_token: None,
        };
        assert!(authorize_shared_build(&app, &build).await.is_ok());
        prune_policies(
            &mut state,
            &SharingState {
                protocol_version: PROTOCOL_VERSION,
                invitations: vec![],
                grants: vec![],
            },
        );
        assert!(state.policies.is_empty());
    }
}
