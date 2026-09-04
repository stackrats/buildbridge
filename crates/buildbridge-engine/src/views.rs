//! The views the desktop renders: one machine, and the list of machines.

use super::*;

pub(crate) fn is_live(state: ContainerState) -> bool {
    matches!(
        state,
        ContainerState::Running | ContainerState::Paused | ContainerState::Restarting
    )
}

pub(crate) fn ensure_apple_project_guest_ready(view: &MacBuilderView) -> Result<(), String> {
    if view.runtime.state != ContainerState::Running {
        return Err("Start the macOS machine first.".to_string());
    }
    if view.guest.ssh.trust != GuestTrustState::Trusted || !view.guest.diagnostics.authenticated {
        return Err("Finish the pinned macOS guest connection first.".to_string());
    }
    if !view.guest.diagnostics.xcode_selected {
        return Err("Activate Xcode before preparing an Apple project.".to_string());
    }

    Ok(())
}

pub(crate) async fn build_machine_list_view(app: &Engine) -> Result<MachineListView, String> {
    let registry = machines::load_registry(app)?;
    let mut entries = Vec::with_capacity(registry.machines.len());
    for machine in &registry.machines {
        let paths = MachinePaths::resolve(app, &machine.id)?;
        let workspace_name = load_apple_workspace(&paths)?.map(|workspace| workspace.name);
        let signing = load_signing_provisioning(&paths)?;
        entries.push((machine.clone(), paths, workspace_name, signing));
    }
    let busy = app
        .state()
        .busy_machines
        .lock()
        .map_err(|_| "The machine operation registry is poisoned.".to_string())?
        .clone();
    let template_app = app.clone();
    let (host, machines) = tokio::task::spawn_blocking(move || {
        let host = buildbridge_docker_osx::probe_host();
        let mut summaries = Vec::with_capacity(entries.len());
        for (machine, paths, workspace_name, signing) in entries {
            let runtime = buildbridge_docker_osx::status(&paths.container_name)
                .map_err(|error| error.to_string())?;
            // Signing belongs to the container it was imported into; a rebuilt container drops it.
            let signing = signing
                .filter(|stored| runtime.container_id.as_deref() == Some(&stored.container_id));
            summaries.push(MachineSummary {
                id: machine.id.clone(),
                template_name: machine
                    .template_id
                    .as_deref()
                    .and_then(|id| template_name(&template_app, id)),
                config: machine.config,
                created_at_epoch_seconds: machine.created_at_epoch_seconds,
                state: runtime.state,
                container_id: runtime.container_id,
                busy_operation: busy.get(&machine.id).map(|label| (*label).to_string()),
                guest_configured: paths.guest_access().is_file(),
                trust_pinned: paths.known_hosts().is_file(),
                workspace_name,
                signing_kit_name: None,
                signing_provisioned: signing.is_some(),
                signing_identity: signing.and_then(|stored| {
                    let result = stored.result;
                    result
                        .distribution_identity
                        .or(result.development_identity)
                        .map(|identity| identity.identity_name)
                }),
                archive_retained: paths.apple_archive_record().is_file(),
                env_set_name: None,
                usb_ready: buildbridge_docker_osx::inspect_container_layout(&paths.container_name)
                    .ok()
                    .flatten()
                    .is_some_and(|layout| {
                        layout.disk_on_host && layout.usb_access && layout.control_socket
                    }),
                device_run_retained: paths.apple_device_run_record().is_file(),
            });
        }

        Ok::<_, String>((host, summaries))
    })
    .await
    .map_err(|error| error.to_string())??;

    // Kit and env-set names come from the vault, which the blocking probe above must not touch.
    let kits = read_signing_kits().await?.kits;
    let env_sets = read_env_sets()
        .await
        .map(|stored| stored.sets)
        .unwrap_or_default();
    let attachments = machines::load_registry(app)?;
    let mut machines = machines;
    for summary in &mut machines {
        let attached = attachments
            .find(&summary.id)
            .ok()
            .and_then(|machine| machine.signing_kit_id.clone());
        // The same rule as the machine view: nothing is attached on a machine's behalf, not
        // even when the host holds exactly one kit, so the row and the page never disagree.
        summary.signing_kit_name =
            resolve_signing_kit(&kits, attached.as_deref()).map(|kit| kit.name.clone());
        summary.env_set_name = attachments
            .find(&summary.id)
            .ok()
            .and_then(|machine| machine.env_set_id.as_deref())
            .and_then(|id| env_sets.iter().find(|set| set.id == id))
            .map(|set| set.name.clone());
    }

    Ok(MachineListView { host, machines })
}

pub(crate) async fn build_mac_builder_view(
    app: &Engine,
    paths: &MachinePaths,
) -> Result<MacBuilderView, String> {
    let profile = machines::load_registry(app)?
        .find(&paths.id)?
        .config
        .clone();
    let guest_access = load_mac_guest_access(paths)?;
    let busy_operation = busy_operation(app, &paths.id)?;
    let probe_paths = paths.clone();
    let probe_profile = profile.clone();
    let cached_devices = app
        .state()
        .guest_devices
        .lock()
        .ok()
        .and_then(|devices| devices.get(&paths.id).cloned())
        .unwrap_or_default();
    let (runtime, logs, guest, mut usb) = tokio::task::spawn_blocking(move || {
        let runtime = buildbridge_docker_osx::status(&probe_paths.container_name)
            .map_err(|error| error.to_string())?;
        let logs = buildbridge_docker_osx::recent_logs(&probe_paths.container_name)
            .map_err(|error| error.to_string())?;
        let guest = build_mac_guest_view(
            &probe_profile,
            &runtime,
            guest_access.as_ref(),
            &probe_paths,
            cached_devices,
        )?;
        let usb = buildbridge_docker_osx::machine_usb_status(
            &probe_paths.container_name,
            &probe_paths.qmp_socket(),
            runtime.state,
        );

        Ok::<_, String>((runtime, logs, guest, usb))
    })
    .await
    .map_err(|error| error.to_string())??;
    // The attach reports why the guest has not enumerated the phone; the probe cannot, so the
    // last reason is carried until the phone shows up or is detached.
    if let Some(attached) = usb.attached.as_mut() {
        let remembered = app
            .state()
            .usb_attach_issues
            .lock()
            .ok()
            .and_then(|issues| issues.get(&paths.id).cloned());
        if attached.enumerated {
            clear_usb_attach_issue(app, &paths.id);
        } else {
            attached.issue = remembered;
        }
    }
    let attached = attached_kit_id(app, &paths.id)?;
    let (kits, vault_issue) = match read_signing_kits().await {
        Ok(stored) => (stored.kits, None),
        Err(issue) => (Vec::new(), Some(issue)),
    };
    let resolved = resolve_signing_kit(&kits, attached.as_deref());
    let signing_kit = resolved.map(summarize_signing_kit);
    let env_set = match attached_env_set_id(app, &paths.id)? {
        Some(id) => read_env_sets()
            .await
            .ok()
            .and_then(|stored| stored.sets.into_iter().find(|set| set.id == id))
            .map(|set| summarize_env_set(&set, &[])),
        None => None,
    };
    let apple_workspace = load_apple_workspace(paths)?;
    let signing = load_signing_provisioning(paths)?
        .filter(|stored| runtime.container_id.as_deref() == Some(&stored.container_id))
        .map(|stored| stored.result);
    let (archive, archive_env_set) = load_apple_archive(paths)?
        .filter(|stored| {
            std::path::Path::new(&stored.result.ipa.path).is_file()
                && std::path::Path::new(&stored.result.archive.path).is_file()
        })
        .map(|stored| (Some(stored.result), stored.env_set_name))
        .unwrap_or((None, None));
    let archive_error = read_optional_text(&paths.apple_archive_error())?;
    let device_run = load_apple_device_run(paths)?
        .filter(|stored| runtime.container_id.as_deref() == Some(&stored.container_id))
        .map(|stored| stored.result);
    let device_run_error = read_optional_text(&paths.apple_device_run_error())?;

    Ok(MacBuilderView {
        machine_id: paths.id.clone(),
        template: template_ref_for(app, &paths.id),
        profile,
        busy_operation,
        runtime,
        signing_kit,
        env_set,
        signing_health: signing_health(vault_issue.as_deref(), resolved, signing.is_some()),
        vault_issue,
        guest,
        apple_workspace,
        signing,
        archive,
        archive_env_set,
        archive_error,
        logs,
        usb,
        device_run,
        device_run_error,
    })
}

pub(crate) fn build_mac_guest_view(
    profile: &MacBuilderConfig,
    runtime: &RuntimeStatus,
    access: Option<&StoredMacGuestAccess>,
    paths: &MachinePaths,
    devices: Vec<buildbridge_docker_osx::GuestDevice>,
) -> Result<MacGuestAccessView, String> {
    let username = access.map(|value| value.username.clone());
    let public_key = read_optional_text(&paths.guest_public_key())?;

    if runtime.state != ContainerState::Running {
        return Ok(MacGuestAccessView {
            username,
            public_key,
            ssh: GuestSshStatus {
                issue: Some("Start the macOS machine to probe guest SSH.".to_string()),
                ..GuestSshStatus::default()
            },
            diagnostics: GuestDiagnostics::default(),
            devices: Vec::new(),
        });
    }

    let known_hosts_path = paths.known_hosts();
    let pinned_host_key = read_optional_text(&known_hosts_path)?;
    let ssh =
        buildbridge_docker_osx::guest_ssh_status(profile.ssh_port, pinned_host_key.as_deref());
    let diagnostics = if ssh.trust == GuestTrustState::Trusted {
        match access {
            Some(access) => buildbridge_docker_osx::guest_diagnostics(
                profile.ssh_port,
                &access.username,
                &paths.guest_identity(),
                &known_hosts_path,
            ),
            None => GuestDiagnostics {
                issue: Some("Enter the macOS short username to configure key access.".to_string()),
                ..GuestDiagnostics::default()
            },
        }
    } else {
        GuestDiagnostics::default()
    };

    Ok(MacGuestAccessView {
        username,
        public_key,
        ssh,
        diagnostics,
        devices,
    })
}

pub(crate) async fn ensure_mac_builder_profile_can_change(
    paths: &MachinePaths,
    stored: &MacBuilderConfig,
    profile: &MacBuilderConfig,
) -> Result<(), String> {
    let unchanged_hardware = stored.macos_release == profile.macos_release
        && stored.memory_gib == profile.memory_gib
        && stored.cpu_cores == profile.cpu_cores
        && stored.ssh_port == profile.ssh_port;
    if unchanged_hardware {
        return Ok(());
    }

    let container_name = paths.container_name.clone();
    let runtime =
        tokio::task::spawn_blocking(move || buildbridge_docker_osx::status(&container_name))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;

    if !matches!(
        runtime.state,
        ContainerState::Missing | ContainerState::Unavailable
    ) {
        return Err(
            "Stop the machine and discard its container before changing its hardware profile. The name can be changed at any time."
                .to_string(),
        );
    }

    Ok(())
}
