//! The guest's SSH access: the short username, the access key, and the pinned identity.

use super::*;
pub async fn configure_mac_guest_access(
    app: &Engine,
    machine_id: String,
    input: MacGuestAccessInput,
) -> Result<MachineView, String> {
    let username = input.username.trim().to_string();
    if !buildbridge_machines::valid_guest_username(&username) {
        return Err(
            "Use the macOS short username: 1–32 letters, numbers, periods, underscores, or hyphens."
                .to_string(),
        );
    }

    let paths = MachinePaths::resolve(app, &machine_id)?;
    machines::load_registry(app)?.find(&machine_id)?;
    let identity_path = paths.guest_identity();
    tokio::task::spawn_blocking(move || ensure_mac_guest_keypair(&identity_path))
        .await
        .map_err(|error| error.to_string())??;
    save_mac_guest_access(&paths, &StoredMacGuestAccess { username })?;

    build_machine_view(app, &paths).await
}

/// Installs the BuildBridge key into the guest user's `authorized_keys` through one
/// password-authenticated SSH session on the pinned host key — the `ssh-copy-id` route. The
/// password exists in this request and in the environment of that single `ssh` process; it is
/// not stored, logged, or reused. Everything after this step signs in with the key. The
/// username and key are kept even when the password is rejected, so the Terminal route stays
/// available.
pub async fn authorize_mac_guest_key(
    app: &Engine,
    machine_id: String,
    input: AuthorizeMacGuestKeyInput,
) -> Result<MachineView, String> {
    let username = input.username.trim().to_string();
    if !buildbridge_machines::valid_guest_username(&username) {
        return Err(
            "Use the macOS short username: 1–32 letters, numbers, periods, underscores, or hyphens."
                .to_string(),
        );
    }
    if input.password.is_empty() {
        return Err("Enter the local macOS login password to install the key.".to_string());
    }

    let paths = MachinePaths::resolve(app, &machine_id)?;
    let profile = machines::load_registry(app)?
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
    let public_key = tokio::task::spawn_blocking(move || ensure_mac_guest_keypair(&identity_path))
        .await
        .map_err(|error| error.to_string())??;
    save_mac_guest_access(
        &paths,
        &StoredMacGuestAccess {
            username: username.clone(),
        },
    )?;

    let password = input.password;
    tokio::task::spawn_blocking(move || {
        buildbridge_machines::authorize_guest_key(
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

    build_machine_view(app, &paths).await
}
pub async fn trust_mac_guest(
    app: &Engine,
    machine_id: String,
    input: TrustMacGuestInput,
) -> Result<MachineView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let profile = machines::load_registry(app)?
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

    tokio::task::spawn_blocking(move || {
        let scanned = buildbridge_machines::scan_guest_host_key(profile.ssh_port)
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

    build_machine_view(app, &paths).await
}
pub async fn forget_mac_guest_trust(
    app: &Engine,
    machine_id: String,
) -> Result<MachineView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    remove_file_if_present(&paths.known_hosts())?;

    build_machine_view(app, &paths).await
}
