//! Machine registry commands and a machine's start, stop and profile.

use super::*;
pub async fn list_machines(app: &Engine) -> Result<MachineListView, String> {
    build_machine_list_view(app).await
}
pub async fn create_machine(
    app: &Engine,
    mut profile: MachineConfig,
    template_id: Option<String>,
) -> Result<MachineListView, String> {
    profile.validate().map_err(|error| error.to_string())?;
    let template_id = match template_id.map(|id| id.trim().to_string()) {
        Some(id) if !id.is_empty() && !profile.provider.is_macos() => {
            let _ = id;
            return Err(
                "An Android toolchain has no disk to clone; templates are for macOS machines."
                    .to_string(),
            );
        }
        Some(id) if !id.is_empty() => {
            let template = load_template(app, &id)?
                .ok_or_else(|| "That template is no longer stored.".to_string())?;
            // The disk directory's layout belongs to the provider that made the template, and
            // the release names that layout for dockur/macos; a clone takes both from it.
            if template.provider != profile.provider {
                return Err(format!(
                    "The template {} was saved from a {} machine; choose that provider to clone it.",
                    template.name,
                    template.provider.label()
                ));
            }
            if let Some(release) = template.macos_release {
                profile.macos_release = release;
            }
            let files = buildbridge_machines::MachineTemplateFiles::new(
                &TemplatePaths::resolve(app, &id)?.files_dir(),
            )
            .map_err(|error| error.to_string())?;
            if !files.ready() {
                return Err(format!(
                    "The template {} is incomplete; save it again before cloning it.",
                    template.name
                ));
            }
            Some(id)
        }
        _ => None,
    };
    let mut registry = machines::load_registry(app)?;
    if registry.machines.len() >= machines::MAX_MACHINES {
        return Err(format!(
            "buildbridge manages at most {} machines on one host.",
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
        template_id,
    });
    machines::save_registry(app, &registry)?;
    app.notify_machines_changed();

    build_machine_list_view(app).await
}
pub async fn delete_machine(
    app: &Engine,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MachineListView, String> {
    if !input.confirmed {
        return Err("Confirm the machine deletion before continuing.".to_string());
    }
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let mut registry = machines::load_registry(app)?;
    let index = registry.position(&machine_id)?;
    let provider = registry.machines[index].config.provider;
    let guard = begin_machine_operation(app, &machine_id, "deleting")?;
    let container_name = paths.container_name.clone();
    let scope = guard.scope();
    let removal = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_machines::enter_operation(scope);
        let runtime = buildbridge_machines::status_for(&container_name, provider)
            .map_err(|error| error.to_string())?;
        if is_live(runtime.state) {
            return Err("Stop the machine before deleting it.".to_string());
        }
        buildbridge_machines::remove(&container_name).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    let removal = removal.and_then(|result| result.map(|_| ()));
    if let Err(error) = removal {
        drop(guard);
        return Err(error);
    }
    // Remove this machine's publishing credential while its nonsecret vault reference
    // still exists. Reusing a deleted machine's name must not reconnect its Play account.
    // The marker goes with the machine's files either way, so a vault that will not answer
    // cannot keep a machine on the host: its credential is unreachable without the marker.
    if !provider.is_macos()
        && let Err(error) = remove_google_play_credentials(app, &machine_id).await
    {
        emit_machine_progress(
            app,
            "machine-delete-warning",
            &machine_id,
            serde_json::json!({ "detail": error }),
        );
    }
    // The removal of the files runs while the operation is still held: emptying an Android
    // home goes through a container of its own, which a Stop must be able to reach.
    let files = tokio::task::spawn_blocking({
        let paths = paths.clone();
        move || paths.remove_machine_files()
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result);
    if let Err(error) = files {
        drop(guard);
        return Err(error);
    }
    registry.machines.remove(index);
    machines::save_registry(app, &registry)?;
    drop(guard);
    app.notify_machines_changed();

    build_machine_list_view(app).await
}
pub async fn discard_machine_container(
    app: &Engine,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MachineView, String> {
    if !input.confirmed {
        return Err("Confirm discarding the container before continuing.".to_string());
    }
    let paths = MachinePaths::resolve(app, &machine_id)?;
    machines::load_registry(app)?.find(&machine_id)?;
    let guard = begin_machine_operation(app, &machine_id, "discarding")?;
    let container_name = paths.container_name.clone();
    let scope = guard.scope();
    let removal_paths = paths.clone();
    let removal = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_machines::enter_operation(scope);
        buildbridge_machines::remove(&container_name).map_err(|error| error.to_string())?;
        // The disk is the macOS installation the person just agreed to discard; for a
        // toolchain it is the home with the SDK, the caches and the synchronized project.
        removal_paths.remove_container_storage()
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result);
    if let Err(error) = removal {
        drop(guard);
        return Err(error);
    }
    remove_file_if_present(&paths.known_hosts())?;
    remove_signing_provisioning_record(&paths)?;
    remove_file_if_present(&paths.apple_device_run_record())?;
    remove_android_release_record(&paths)?;
    remove_android_release_error(&paths)?;
    if let Some(mut workspace) = load_android_workspace(&paths)? {
        workspace.last_snapshot_sha256 = None;
        workspace.last_sync_file_count = None;
        workspace.last_sync_bytes = None;
        workspace.last_synced_at_epoch_seconds = None;
        workspace.last_build_succeeded = false;
        workspace.last_build = None;
        save_android_workspace(&paths, &workspace)?;
    }
    clear_usb_attach_issue(app, &machine_id);
    drop(guard);
    app.notify_machines_changed();

    build_machine_view(app, &paths).await
}
pub async fn get_machine(app: &Engine, machine_id: String) -> Result<MachineView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;

    build_machine_view(app, &paths).await
}
pub async fn configure_machine(
    app: &Engine,
    machine_id: String,
    profile: MachineConfig,
) -> Result<MachineView, String> {
    profile.validate().map_err(|error| error.to_string())?;
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let mut registry = machines::load_registry(app)?;
    let index = registry.position(&machine_id)?;
    registry.ensure_unique_ssh_port(&profile, Some(&machine_id))?;
    let change =
        ensure_machine_profile_can_change(&paths, &registry.machines[index].config, &profile)
            .await?;
    if matches!(change, ProfileChange::RecreateContainer) {
        // The container goes before the profile is saved: a removal that fails leaves the
        // machine exactly as it was, rather than storing hardware its container does not
        // have. Only the container is removed — the disk, the NVRAM, the control directory
        // and a toolchain's home are bound from this host and stay, so the next start builds
        // the new hardware around the macOS or the SDK that is already there.
        let guard = begin_machine_operation(app, &machine_id, "reconfiguring")?;
        let container_name = paths.container_name.clone();
        let scope = guard.scope();
        let removal = tokio::task::spawn_blocking(move || {
            let _operation = buildbridge_machines::enter_operation(scope);
            buildbridge_machines::remove(&container_name).map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result);
        drop(guard);
        removal?;
    }
    registry.machines[index].config = profile;
    machines::save_registry(app, &registry)?;
    app.notify_machines_changed();

    build_machine_view(app, &paths).await
}
pub async fn launch_machine(app: &Engine, machine_id: String) -> Result<MachineView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let profile = machines::load_registry(app)?
        .find(&machine_id)?
        .config
        .clone();
    let guard = begin_machine_operation(app, &machine_id, "starting")?;
    let identity_path = paths.identity();
    // A toolchain container's storage is its home; a macOS machine's is its disk.
    let disk_dir = if profile.provider.is_macos() {
        paths.disk_dir()
    } else {
        paths.android_home_dir()
    };
    let qmp_dir = paths.qmp_dir();
    let template_dir = template_dir_for(app, &machine_id)?;
    let container_name = paths.container_name.clone();
    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_machines::enter_operation(scope);
        let options = buildbridge_machines::LaunchOptions {
            identity_path: &identity_path,
            disk_dir: &disk_dir,
            qmp_dir: &qmp_dir,
            usb: buildbridge_machines::resolve_usb_options(),
            template_dir: template_dir.as_deref(),
        };
        buildbridge_machines::launch(&container_name, &profile, &options, |progress| {
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
    app.notify_machines_changed();

    build_machine_view(app, &paths).await
}
pub async fn stop_machine(app: &Engine, machine_id: String) -> Result<MachineView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let provider = machines::load_registry(app)?
        .find(&machine_id)?
        .config
        .provider;
    let guard = begin_machine_operation(app, &machine_id, "stopping")?;
    let container_name = paths.container_name.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_machines::enter_operation(scope);
        if !provider.is_macos() {
            // A desktop restart may leave a detached job without an engine operation scope.
            // Stop clears its job and staged credentials before the container is shut down.
            let _ = buildbridge_machines::stop_android_jobs(&container_name);
        }
        buildbridge_machines::stop(&container_name).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    finish_operation(&cancel_probe, joined)?;
    app.notify_machines_changed();

    build_machine_view(app, &paths).await
}

/// A machine and its container state, for a host that lists machines outside the window (the
/// desktop's tray) without building the full view.
#[derive(Debug, Clone)]
pub struct MachineRuntime {
    pub id: String,
    pub name: String,
    pub state: ContainerState,
    pub host_ready: bool,
}

pub fn machine_runtimes(app: &Engine) -> Vec<MachineRuntime> {
    let Ok(registry) = machines::load_registry(app) else {
        return Vec::new();
    };
    registry
        .machines
        .into_iter()
        .filter_map(|machine| {
            let paths = MachinePaths::resolve(app, &machine.id).ok()?;
            let runtime =
                buildbridge_machines::status_for(&paths.container_name, machine.config.provider)
                    .ok();
            Some(MachineRuntime {
                id: machine.id,
                name: machine.config.name,
                state: runtime
                    .as_ref()
                    .map(|runtime| runtime.state)
                    .unwrap_or(ContainerState::Unavailable),
                host_ready: runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.prerequisites.ready),
            })
        })
        .collect()
}
