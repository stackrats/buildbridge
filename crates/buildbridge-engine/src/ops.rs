//! Machine operations: the per-machine busy marker, cancellation scopes, and progress events.

use super::*;

#[derive(Default)]
pub struct AppState {
    pub(crate) runner_running: AtomicBool,
    /// Machines with a long-running operation in flight, keyed by machine id. The value is a
    /// stable snake_case operation key that the frontend maps back to a step.
    pub(crate) busy_machines: Mutex<HashMap<String, &'static str>>,
    /// The cancellable scope of each in-flight operation, keyed by machine id.
    pub(crate) operation_scopes: Mutex<HashMap<String, Arc<OperationScope>>>,
    /// A privileged host change (the USB udev rule) is in flight, so a second authorization
    /// prompt cannot stack on the first.
    pub(crate) host_usb_busy: AtomicBool,
    /// Why the phone a machine holds has not shown up in the guest yet, from the last attach.
    /// Cleared once the guest enumerates it or the phone is detached.
    pub(crate) usb_attach_issues: Mutex<HashMap<String, String>>,
    /// The phones each guest reported at its last listing. Probing `devicectl` costs seconds,
    /// so the view serves this and the listing command refreshes it.
    pub(crate) guest_devices: Mutex<HashMap<String, Vec<buildbridge_docker_osx::GuestDevice>>>,
}

/// Marks the host busy with a privileged USB change until dropped.
pub struct HostUsbGuard {
    pub(crate) app: Engine,
}

impl Drop for HostUsbGuard {
    fn drop(&mut self) {
        self.app
            .state()
            .host_usb_busy
            .store(false, Ordering::Release);
    }
}

pub(crate) fn begin_host_usb_operation(app: &Engine) -> Result<HostUsbGuard, String> {
    let claimed = app
        .state()
        .host_usb_busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok();
    if !claimed {
        return Err("A USB rule change is already waiting for authorization.".to_string());
    }

    Ok(HostUsbGuard { app: app.clone() })
}

/// Marks one machine busy until dropped so concurrent operations cannot interleave.
pub struct MachineOperationGuard {
    pub(crate) app: Engine,
    pub(crate) machine_id: String,
    pub(crate) scope: Arc<OperationScope>,
}

impl MachineOperationGuard {
    /// The scope a blocking closure enters so its child processes can be stopped.
    pub(crate) fn scope(&self) -> Arc<OperationScope> {
        Arc::clone(&self.scope)
    }
}

impl Drop for MachineOperationGuard {
    fn drop(&mut self) {
        if let Ok(mut busy) = self.app.state().busy_machines.lock() {
            busy.remove(&self.machine_id);
        }
        if let Ok(mut scopes) = self.app.state().operation_scopes.lock() {
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

pub(crate) fn begin_machine_operation(
    app: &Engine,
    machine_id: &str,
    label: &'static str,
) -> Result<MachineOperationGuard, String> {
    let state = app.state();
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
pub async fn cancel_machine_operation(app: &Engine, machine_id: String) -> Result<(), String> {
    let scope = app
        .state()
        .operation_scopes
        .lock()
        .map_err(|_| "The machine operation registry is poisoned.".to_string())?
        .get(&machine_id)
        .cloned();
    let label = busy_operation(app, &machine_id)?;
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
        let paths = MachinePaths::resolve(app, &machine_id)?;
        if let Some(access) = load_mac_guest_access(&paths)?
            && let Ok(registry) = machines::load_registry(app)
            && let Ok(machine) = registry.find(&machine_id)
        {
            let ssh_port = machine.config.ssh_port;
            let identity = paths.guest_identity();
            let known_hosts = paths.known_hosts();
            let _ = tokio::task::spawn_blocking(move || {
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
pub(crate) fn finish_operation<T>(
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

pub(crate) const CANCELLED_MESSAGE: &str = "Stopped.";

pub(crate) fn busy_operation(app: &Engine, machine_id: &str) -> Result<Option<String>, String> {
    let state = app.state();
    let busy = state
        .busy_machines
        .lock()
        .map_err(|_| "The machine operation registry is poisoned.".to_string())?;

    Ok(busy.get(machine_id).map(|label| (*label).to_string()))
}

pub(crate) fn emit_machine_progress<T: Serialize + Clone>(
    app: &Engine,
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

/// Everything a command that talks to a machine's guest resolves first: the machine's paths
/// and profile, the guest username, the key files, and a fresh view that proves the guest is
/// ready for project work. One place for the checks, so every command refuses the same way.
pub struct GuestContext {
    pub(crate) paths: MachinePaths,
    pub(crate) profile: MacBuilderConfig,
    pub(crate) username: String,
    pub(crate) identity_path: PathBuf,
    pub(crate) known_hosts_path: PathBuf,
    pub(crate) current: MacBuilderView,
}

impl GuestContext {
    pub(crate) fn ssh_port(&self) -> u16 {
        self.profile.ssh_port
    }
}

pub(crate) async fn guest_context(app: &Engine, machine_id: &str) -> Result<GuestContext, String> {
    let paths = MachinePaths::resolve(app, machine_id)?;
    let profile = machines::load_registry(app)?
        .find(machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let current = build_mac_builder_view(app, &paths).await?;
    ensure_apple_project_guest_ready(&current)?;
    Ok(GuestContext {
        identity_path: paths.guest_identity(),
        known_hosts_path: paths.known_hosts(),
        paths,
        profile,
        username: access.username,
        current,
    })
}

/// Runs blocking provider work as the machine's one operation: the busy marker is held for
/// its duration, the work enters a cancellation scope so a Stop reaches its processes, and a
/// cancelled run reports as such rather than with whatever error the kill produced.
pub(crate) async fn run_machine_operation<T, W>(
    app: &Engine,
    machine_id: &str,
    label: &'static str,
    work: W,
) -> Result<T, String>
where
    T: Send + 'static,
    W: FnOnce() -> Result<T, String> + Send + 'static,
{
    let guard = begin_machine_operation(app, machine_id, label)?;
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        work()
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    finish_operation(&cancel_probe, joined)
}
