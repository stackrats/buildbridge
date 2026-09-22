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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum StopMachineOutcome {
    Stopped,
    AlreadyStopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StopMachineResult {
    pub machine_id: String,
    pub name: String,
    pub outcome: StopMachineOutcome,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StopAllMachinesResult {
    pub results: Vec<StopMachineResult>,
}

/// Stops only the machines in this registry, preserving their containers and stored data.
/// Independent machines stop concurrently, so one protected or failed operation cannot hold
/// up the others. A machine owned by another process remains protected by its operation lock.
pub async fn stop_all_machines(app: &Engine) -> Result<StopAllMachinesResult, String> {
    let registry = machines::load_registry(app)?;
    let pending = registry
        .machines
        .into_iter()
        .map(|machine| {
            let app = app.clone();
            let machine_id = machine.id.clone();
            let task = tokio::spawn(async move {
                stop_registered_machine(&EngineMachineStop { app: &app }, &machine_id).await
            });
            (machine, task)
        })
        .collect::<Vec<_>>();
    let mut results = Vec::with_capacity(pending.len());
    for (machine, task) in pending {
        let outcome = task
            .await
            .map_err(|error| format!("The stop operation could not finish: {error}"))
            .and_then(|result| result);
        results.push(stop_machine_result(
            machine.id,
            machine.config.name,
            outcome,
        ));
    }
    app.notify_machines_changed();

    Ok(StopAllMachinesResult { results })
}

fn stop_machine_result(
    machine_id: String,
    name: String,
    result: Result<StopMachineOutcome, String>,
) -> StopMachineResult {
    let (outcome, error) = match result {
        Ok(outcome) => (outcome, None),
        Err(error) => (StopMachineOutcome::Failed, Some(error)),
    };
    StopMachineResult {
        machine_id,
        name,
        outcome,
        error,
    }
}

const STOP_CANCEL_WAIT_POLLS: usize = 100;

trait MachineStopControl: Sync {
    fn busy(&self, machine_id: &str) -> Result<Option<String>, String>;
    fn cancel(&self, machine_id: &str) -> impl Future<Output = Result<(), String>> + Send;
    fn state(
        &self,
        machine_id: &str,
    ) -> impl Future<Output = Result<ContainerState, String>> + Send;
    fn stop(&self, machine_id: &str)
    -> impl Future<Output = Result<ContainerState, String>> + Send;
    fn wait_for_cancellation(&self) -> impl Future<Output = ()> + Send;
}

struct EngineMachineStop<'a> {
    app: &'a Engine,
}

impl MachineStopControl for EngineMachineStop<'_> {
    fn busy(&self, machine_id: &str) -> Result<Option<String>, String> {
        busy_operation(self.app, machine_id)
    }

    async fn cancel(&self, machine_id: &str) -> Result<(), String> {
        cancel_machine_operation(self.app, machine_id.to_string()).await
    }

    async fn state(&self, machine_id: &str) -> Result<ContainerState, String> {
        let provider = machines::load_registry(self.app)?
            .find(machine_id)?
            .config
            .provider;
        let paths = MachinePaths::resolve(self.app, machine_id)?;
        tokio::task::spawn_blocking(move || {
            buildbridge_machines::status_for(&paths.container_name, provider)
                .map(|runtime| runtime.state)
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())?
    }

    async fn stop(&self, machine_id: &str) -> Result<ContainerState, String> {
        stop_machine(self.app, machine_id.to_string())
            .await
            .map(|view| view.runtime.state)
    }

    async fn wait_for_cancellation(&self) {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn stop_registered_machine(
    control: &impl MachineStopControl,
    machine_id: &str,
) -> Result<StopMachineOutcome, String> {
    if let Some(operation) = control.busy(machine_id)? {
        if operation == "migrating_usb" {
            return Err("The disk migration cannot be stopped; wait for it to finish.".into());
        }
        if let Err(error) = control.cancel(machine_id).await
            && control.busy(machine_id)?.is_some()
        {
            return Err(format!(
                "The {operation} operation could not be cancelled: {error}"
            ));
        }
        for attempt in 0..=STOP_CANCEL_WAIT_POLLS {
            if control.busy(machine_id)?.is_none() {
                break;
            }
            if attempt == STOP_CANCEL_WAIT_POLLS {
                return Err(
                    "The active operation is still finishing after cancellation. Try Stop all again when it has finished."
                        .into(),
                );
            }
            control.wait_for_cancellation().await;
        }
    }

    match control.state(machine_id).await? {
        ContainerState::Running | ContainerState::Paused | ContainerState::Restarting => {
            match control.stop(machine_id).await? {
                ContainerState::Missing
                | ContainerState::Created
                | ContainerState::Exited
                | ContainerState::Dead => Ok(StopMachineOutcome::Stopped),
                _ => {
                    Err("The machine did not reach a stopped state. Check it and try again.".into())
                }
            }
        }
        ContainerState::Missing
        | ContainerState::Created
        | ContainerState::Exited
        | ContainerState::Dead => Ok(StopMachineOutcome::AlreadyStopped),
        ContainerState::Unavailable | ContainerState::Unknown => Err(
            "The machine's current container state could not be read. Check Docker and try again."
                .into(),
        ),
    }
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

#[cfg(test)]
mod stop_all_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct FakeStopControl {
        operation: Option<&'static str>,
        cancellation_error: Option<&'static str>,
        cancelled: AtomicBool,
        waits: AtomicUsize,
        release_after: usize,
        runtime: Result<ContainerState, &'static str>,
        stopped: Result<ContainerState, &'static str>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl FakeStopControl {
        fn new(runtime: ContainerState) -> Self {
            Self {
                operation: None,
                cancellation_error: None,
                cancelled: AtomicBool::new(false),
                waits: AtomicUsize::new(0),
                release_after: 0,
                runtime: Ok(runtime),
                stopped: Ok(ContainerState::Exited),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn record(&self, call: &'static str) {
            self.calls.lock().unwrap().push(call);
        }
    }

    impl MachineStopControl for FakeStopControl {
        fn busy(&self, _machine_id: &str) -> Result<Option<String>, String> {
            Ok(
                if self.cancelled.load(Ordering::Acquire)
                    && self.waits.load(Ordering::Acquire) >= self.release_after
                {
                    None
                } else {
                    self.operation.map(str::to_string)
                },
            )
        }

        async fn cancel(&self, _machine_id: &str) -> Result<(), String> {
            self.record("cancel");
            if let Some(error) = self.cancellation_error {
                return Err(error.to_string());
            }
            self.cancelled.store(true, Ordering::Release);
            Ok(())
        }

        async fn state(&self, _machine_id: &str) -> Result<ContainerState, String> {
            self.record("state");
            self.runtime.map_err(str::to_string)
        }

        async fn stop(&self, _machine_id: &str) -> Result<ContainerState, String> {
            self.record("stop");
            self.stopped.map_err(str::to_string)
        }

        async fn wait_for_cancellation(&self) {
            self.record("wait");
            self.waits.fetch_add(1, Ordering::AcqRel);
        }
    }

    #[tokio::test]
    async fn active_build_finishes_cancelling_before_fresh_state_and_stop() {
        let mut control = FakeStopControl::new(ContainerState::Running);
        control.operation = Some("archiving");
        control.release_after = 2;

        assert_eq!(
            stop_registered_machine(&control, "one").await.unwrap(),
            StopMachineOutcome::Stopped
        );
        assert_eq!(
            *control.calls.lock().unwrap(),
            ["cancel", "wait", "wait", "state", "stop"]
        );
    }

    #[tokio::test]
    async fn protected_migrations_are_not_cancelled_or_stopped() {
        let mut control = FakeStopControl::new(ContainerState::Running);
        control.operation = Some("migrating_usb");

        let error = stop_registered_machine(&control, "one").await.unwrap_err();
        assert!(error.contains("disk migration cannot be stopped"));
        assert!(control.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_foreign_operation_or_cancellation_timeout_keeps_the_machine_running() {
        let mut foreign = FakeStopControl::new(ContainerState::Running);
        foreign.operation = Some("archiving");
        foreign.cancellation_error = Some("Nothing is running on this machine.");
        assert!(stop_registered_machine(&foreign, "one").await.is_err());
        assert_eq!(*foreign.calls.lock().unwrap(), ["cancel"]);

        let mut slow = FakeStopControl::new(ContainerState::Running);
        slow.operation = Some("archiving");
        slow.release_after = STOP_CANCEL_WAIT_POLLS + 1;
        let error = stop_registered_machine(&slow, "one").await.unwrap_err();
        assert!(error.contains("still finishing"));
        assert_eq!(slow.waits.load(Ordering::Acquire), STOP_CANCEL_WAIT_POLLS);
        assert!(!slow.calls.lock().unwrap().contains(&"stop"));
    }

    #[tokio::test]
    async fn only_fresh_live_states_are_stopped() {
        for state in [
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Restarting,
        ] {
            let control = FakeStopControl::new(state);
            assert_eq!(
                stop_registered_machine(&control, "one").await.unwrap(),
                StopMachineOutcome::Stopped
            );
            assert_eq!(*control.calls.lock().unwrap(), ["state", "stop"]);
        }
        for state in [
            ContainerState::Missing,
            ContainerState::Created,
            ContainerState::Exited,
            ContainerState::Dead,
        ] {
            let control = FakeStopControl::new(state);
            assert_eq!(
                stop_registered_machine(&control, "one").await.unwrap(),
                StopMachineOutcome::AlreadyStopped
            );
            assert_eq!(*control.calls.lock().unwrap(), ["state"]);
        }
        for state in [ContainerState::Unavailable, ContainerState::Unknown] {
            let control = FakeStopControl::new(state);
            assert!(stop_registered_machine(&control, "one").await.is_err());
            assert_eq!(*control.calls.lock().unwrap(), ["state"]);
        }
    }

    #[tokio::test]
    async fn failures_remain_per_machine_and_success_requires_a_stopped_state() {
        let mut failed = FakeStopControl::new(ContainerState::Running);
        failed.stopped = Err("Docker refused to stop the container.");
        let stopped = FakeStopControl::new(ContainerState::Running);
        let mut still_running = FakeStopControl::new(ContainerState::Running);
        still_running.stopped = Ok(ContainerState::Running);
        let mut results = Vec::new();
        for (id, control) in [
            ("failed", &failed),
            ("stopped", &stopped),
            ("live", &still_running),
        ] {
            results.push(stop_machine_result(
                id.to_string(),
                format!("Machine {id}"),
                stop_registered_machine(control, id).await,
            ));
        }
        assert_eq!(results[0].machine_id, "failed");
        assert_eq!(results[0].outcome, StopMachineOutcome::Failed);
        assert_eq!(
            results[0].error.as_deref(),
            Some("Docker refused to stop the container.")
        );
        assert_eq!(results[1].outcome, StopMachineOutcome::Stopped);
        assert_eq!(results[1].error, None);
        assert_eq!(results[2].outcome, StopMachineOutcome::Failed);
        assert!(results[2].error.as_ref().unwrap().contains("stopped state"));
    }
}
