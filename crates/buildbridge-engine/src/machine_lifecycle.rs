//! Machine registry commands and the builder's start, stop and profile.

use super::*;
pub async fn list_machines(app: &Engine) -> Result<MachineListView, String> {
    build_machine_list_view(app).await
}
pub async fn create_machine(
    app: &Engine,
    profile: MacBuilderConfig,
    template_id: Option<String>,
) -> Result<MachineListView, String> {
    profile.validate().map_err(|error| error.to_string())?;
    let template_id = match template_id.map(|id| id.trim().to_string()) {
        Some(id) if !id.is_empty() => {
            if profile.provider == MachineProvider::DockurMacos {
                return Err(
                    "Templates clone Docker-OSX disks; a dockur/macos machine installs macOS itself."
                        .to_string(),
                );
            }
            let template = load_template(app, &id)?
                .ok_or_else(|| "That template is no longer stored.".to_string())?;
            let files = buildbridge_docker_osx::MachineTemplateFiles::new(
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
    let guard = begin_machine_operation(app, &machine_id, "deleting")?;
    let container_name = paths.container_name.clone();
    let scope = guard.scope();
    let removal = tokio::task::spawn_blocking(move || {
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
    machines::save_registry(app, &registry)?;
    drop(guard);
    app.notify_machines_changed();

    build_machine_list_view(app).await
}
pub async fn discard_machine_container(
    app: &Engine,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MacBuilderView, String> {
    if !input.confirmed {
        return Err("Confirm discarding the macOS disk before continuing.".to_string());
    }
    let paths = MachinePaths::resolve(app, &machine_id)?;
    machines::load_registry(app)?.find(&machine_id)?;
    let guard = begin_machine_operation(app, &machine_id, "discarding")?;
    let container_name = paths.container_name.clone();
    let scope = guard.scope();
    let removal = tokio::task::spawn_blocking(move || {
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
    clear_usb_attach_issue(app, &machine_id);
    drop(guard);
    app.notify_machines_changed();

    build_mac_builder_view(app, &paths).await
}
pub async fn get_mac_builder_status(
    app: &Engine,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;

    build_mac_builder_view(app, &paths).await
}
pub async fn configure_mac_builder(
    app: &Engine,
    machine_id: String,
    profile: MacBuilderConfig,
) -> Result<MacBuilderView, String> {
    profile.validate().map_err(|error| error.to_string())?;
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let mut registry = machines::load_registry(app)?;
    let index = registry.position(&machine_id)?;
    registry.ensure_unique_ssh_port(&profile, Some(&machine_id))?;
    ensure_mac_builder_profile_can_change(&paths, &registry.machines[index].config, &profile)
        .await?;
    registry.machines[index].config = profile;
    machines::save_registry(app, &registry)?;
    app.notify_machines_changed();

    build_mac_builder_view(app, &paths).await
}
pub async fn launch_mac_builder(
    app: &Engine,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let profile = machines::load_registry(app)?
        .find(&machine_id)?
        .config
        .clone();
    let guard = begin_machine_operation(app, &machine_id, "starting")?;
    let identity_path = paths.identity();
    let disk_dir = paths.disk_dir();
    let qmp_dir = paths.qmp_dir();
    let template_dir = template_dir_for(app, &machine_id)?;
    let container_name = paths.container_name.clone();
    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        let options = buildbridge_docker_osx::LaunchOptions {
            identity_path: &identity_path,
            disk_dir: &disk_dir,
            qmp_dir: &qmp_dir,
            usb: buildbridge_docker_osx::resolve_usb_options(),
            template_dir: template_dir.as_deref(),
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
    app.notify_machines_changed();

    build_mac_builder_view(app, &paths).await
}
pub async fn stop_mac_builder(app: &Engine, machine_id: String) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    machines::load_registry(app)?.find(&machine_id)?;
    let guard = begin_machine_operation(app, &machine_id, "stopping")?;
    let container_name = paths.container_name.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::stop(&container_name).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    finish_operation(&cancel_probe, joined)?;
    app.notify_machines_changed();

    build_mac_builder_view(app, &paths).await
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
            let runtime = buildbridge_docker_osx::status(&paths.container_name).ok();
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
