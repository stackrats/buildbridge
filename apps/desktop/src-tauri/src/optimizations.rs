//! Guest optimizations: the catalogue with this guest's state, and applying one.

use super::*;
use ts_rs::TS;

/// One optimization as the interface shows it: the catalogue entry plus whether the guest
/// already has it, when the guest can be asked.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GuestOptimizationView {
    #[serde(flatten)]
    pub(crate) optimization: &'static GuestOptimization,
    /// `None` when the guest cannot be asked right now, or the check could not tell.
    pub(crate) applied: Option<bool>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GuestOptimizationsView {
    /// Whether the guest is reachable enough to check or apply anything.
    pub(crate) available: bool,
    pub(crate) reason: Option<String>,
    pub(crate) items: Vec<GuestOptimizationView>,
}

/// Lists the catalogue with each item's current state on this machine's guest.
#[tauri::command]
pub(crate) async fn list_guest_optimizations(
    app: AppHandle,
    machine_id: String,
) -> Result<GuestOptimizationsView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let view = build_mac_builder_view(&app, &paths).await?;
    let guest_ready = view.runtime.state == ContainerState::Running
        && view.guest.ssh.trust == GuestTrustState::Trusted
        && view.guest.diagnostics.authenticated;
    let catalogue = buildbridge_docker_osx::guest_optimizations();
    if !guest_ready {
        return Ok(GuestOptimizationsView {
            available: false,
            reason: Some(
                "Start the machine, pin its identity and authorize the access key first."
                    .to_string(),
            ),
            items: catalogue
                .iter()
                .map(|optimization| GuestOptimizationView {
                    optimization,
                    applied: None,
                })
                .collect(),
        });
    }

    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let ssh_port = view.profile.ssh_port;
    let identity = paths.guest_identity();
    let known_hosts = paths.known_hosts();
    let states = tauri::async_runtime::spawn_blocking(move || {
        buildbridge_docker_osx::check_guest_optimizations(
            ssh_port,
            &access.username,
            &identity,
            &known_hosts,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;

    Ok(GuestOptimizationsView {
        available: true,
        reason: None,
        items: catalogue
            .iter()
            .map(|optimization| GuestOptimizationView {
                optimization,
                applied: states
                    .iter()
                    .find(|(id, _)| *id == optimization.id)
                    .and_then(|(_, applied)| *applied),
            })
            .collect(),
    })
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplyOptimizationInput {
    pub(crate) optimization_id: String,
    pub(crate) confirmed: bool,
}

/// Applies one catalogue optimization to the guest. Admin items open the guest Terminal for
/// the password; the operation is busy — and stoppable — until it returns.
#[tauri::command]
pub(crate) async fn apply_guest_optimization(
    app: AppHandle,
    machine_id: String,
    input: ApplyOptimizationInput,
) -> Result<GuestOptimizationsView, String> {
    if !input.confirmed {
        return Err("Confirm the optimization before applying it.".to_string());
    }
    let optimization = buildbridge_docker_osx::guest_optimization(&input.optimization_id)
        .ok_or_else(|| "That optimization is not in the catalogue.".to_string())?;
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let view = build_mac_builder_view(&app, &paths).await?;
    if view.runtime.state != ContainerState::Running {
        return Err("Start the macOS machine first.".to_string());
    }
    if view.guest.ssh.trust != GuestTrustState::Trusted || !view.guest.diagnostics.authenticated {
        return Err("Finish the pinned macOS guest connection first.".to_string());
    }
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let guard = begin_machine_operation(&app, &machine_id, "optimizing")?;
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let ssh_port = view.profile.ssh_port;
    let identity = paths.guest_identity();
    let known_hosts = paths.known_hosts();
    let optimization_id = optimization.id.to_string();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::apply_guest_optimization(
            ssh_port,
            &access.username,
            &identity,
            &known_hosts,
            &optimization_id,
            |_elapsed| {},
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    finish_operation(&cancel_probe, joined)?;

    list_guest_optimizations(app, machine_id).await
}
