//! Xcode import and activation, signing provisioning, workspace sync, and the builds.

use super::*;

#[tauri::command]
pub(crate) async fn import_mac_xcode_package(
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
pub(crate) async fn activate_mac_xcode(
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
pub(crate) async fn provision_mac_signing(
    app: AppHandle,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    let secrets = resolve_signing_kit_for(&app, &machine_id).await?;
    provision_with_kit(&app, &machine_id, secrets).await
}

/// Provisions one kit into a machine's guest keychain: the distribution identity, the
/// development identity when the kit holds one, and every profile, replacing what the previous
/// provisioning installed. A kit that holds a Team key but no distribution files gets them
/// created at Apple first, so a Team key and a keychain password are all a kit needs.
pub(crate) async fn provision_with_kit(
    app: &AppHandle,
    machine_id: &str,
    mut secrets: StoredSigningKit,
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

    let guard = begin_machine_operation(app, machine_id, "provisioning_signing")?;
    if !kit_has_distribution_set(&secrets) && kit_has_team_key(&secrets) {
        let started = std::time::Instant::now();
        let report = |phase: buildbridge_docker_osx::SigningProvisioningPhase, detail: &str| {
            emit_machine_progress(
                app,
                SIGNING_PROGRESS_EVENT,
                machine_id,
                SigningProvisioningProgress {
                    phase,
                    completed_bytes: 0,
                    total_bytes: 0,
                    elapsed_seconds: started.elapsed().as_secs(),
                    detail: detail.to_string(),
                },
            );
        };
        ensure_distribution_set(
            app,
            &mut secrets,
            &bundle_identifier,
            &workspace.name,
            &report,
        )
        .await?;
    }

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

#[tauri::command]
pub(crate) async fn clear_mac_guest_signing(
    app: AppHandle,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    let guest = guest_context(&app, &machine_id).await?;
    let paths = guest.paths.clone();
    let profile_uuids = guest
        .current
        .signing
        .as_ref()
        .ok_or_else(|| "No provisioned signing state is recorded for this guest.".to_string())?
        .profiles
        .iter()
        .map(|profile| profile.uuid.clone())
        .collect::<Vec<_>>();
    run_machine_operation(&app, &machine_id, "clearing_signing", move || {
        buildbridge_docker_osx::clear_signing(
            &profile_uuids,
            guest.ssh_port(),
            &guest.username,
            &guest.identity_path,
            &guest.known_hosts_path,
        )
        .map_err(|error| error.to_string())
    })
    .await?;
    remove_signing_provisioning_record(&paths)?;

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
pub(crate) async fn approve_apple_workspace(
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
pub(crate) async fn clear_apple_workspace(
    app: AppHandle,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    remove_file_if_present(&paths.apple_workspace())?;

    build_mac_builder_view(&app, &paths).await
}

#[tauri::command]
pub(crate) async fn sync_apple_workspace(
    app: AppHandle,
    machine_id: String,
) -> Result<SyncAppleWorkspaceResult, String> {
    sync_apple_workspace_from(&app, &machine_id, None).await
}

/// Synchronizes a source tree into the guest: the approved folder as it is, or a checked-out
/// revision of the same project when a remote build names a ref.
pub(crate) async fn sync_apple_workspace_from(
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
pub(crate) async fn run_apple_smoke_build(
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
pub(crate) async fn run_apple_signed_archive(
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
pub(crate) async fn reveal_apple_archive(app: AppHandle, machine_id: String) -> Result<(), String> {
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
pub(crate) async fn clear_apple_archive(
    app: AppHandle,
    machine_id: String,
) -> Result<MacBuilderView, String> {
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
