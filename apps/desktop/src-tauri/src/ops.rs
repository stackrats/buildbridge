//! Machine operations: the per-machine busy marker, cancellation scopes, and progress events.

use super::*;

#[derive(Default)]
pub(crate) struct AppState {
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
pub(crate) struct HostUsbGuard {
    pub(crate) app: AppHandle,
}

impl Drop for HostUsbGuard {
    fn drop(&mut self) {
        self.app
            .state::<AppState>()
            .host_usb_busy
            .store(false, Ordering::Release);
    }
}

pub(crate) fn begin_host_usb_operation(app: &AppHandle) -> Result<HostUsbGuard, String> {
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
pub(crate) struct MachineOperationGuard {
    pub(crate) app: AppHandle,
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

pub(crate) fn begin_machine_operation(
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
pub(crate) async fn cancel_machine_operation(
    app: AppHandle,
    machine_id: String,
) -> Result<(), String> {
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

pub(crate) fn busy_operation(app: &AppHandle, machine_id: &str) -> Result<Option<String>, String> {
    let state = app.state::<AppState>();
    let busy = state
        .busy_machines
        .lock()
        .map_err(|_| "The machine operation registry is poisoned.".to_string())?;

    Ok(busy.get(machine_id).map(|label| (*label).to_string()))
}

pub(crate) fn emit_machine_progress<T: Serialize + Clone>(
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
