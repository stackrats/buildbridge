//! iPhone passthrough: the host rule, the container's USB access, attach and detach.

use super::*;

/// After QEMU holds the phone: macOS enumerating it, then the CoreDevice pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum UsbAttachPhase {
    WaitingForMacos,
    Pairing,
    Completed,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct UsbAttachProgress {
    pub(crate) phase: UsbAttachPhase,
    #[ts(type = "number")]
    pub(crate) elapsed_seconds: u64,
    pub(crate) detail: String,
}

/// Installs the udev rule that stops usbmuxd from claiming iPhones on this host, through one
/// authorization prompt. Host-level, so it holds no machine.
pub async fn install_usb_release_rule(
    app: &Engine,
) -> Result<buildbridge_docker_osx::HostUsbStatus, String> {
    let guard = begin_host_usb_operation(app)?;
    let staging = app.data_dir().join("usb");
    let installed = tokio::task::spawn_blocking(move || {
        buildbridge_docker_osx::install_iphone_udev_rule(&staging)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?;
    drop(guard);

    installed
}
pub async fn remove_usb_release_rule(
    app: &Engine,
) -> Result<buildbridge_docker_osx::HostUsbStatus, String> {
    let guard = begin_host_usb_operation(app)?;
    let removed = tokio::task::spawn_blocking(|| {
        buildbridge_docker_osx::remove_iphone_udev_rule().map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?;
    drop(guard);

    removed
}

/// Moves a container's macOS disk onto this host and recreates the container with the disk
/// bound in, the control socket, and USB access. Nothing on the disk changes; the signing
/// record is rebound to the new container because the keychain it describes moved with it.
pub async fn migrate_machine_for_usb(
    app: &Engine,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MacBuilderView, String> {
    if !input.confirmed {
        return Err("Confirm the container migration before continuing.".to_string());
    }
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let profile = machines::load_registry(app)?
        .find(&machine_id)?
        .config
        .clone();
    let guard = begin_machine_operation(app, &machine_id, "migrating_usb")?;
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
        buildbridge_docker_osx::migrate_disk_to_host(
            &container_name,
            &profile,
            &options,
            |progress| {
                emit_machine_progress(
                    &event_app,
                    USB_MIGRATION_PROGRESS_EVENT,
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
    let runtime = finish_operation(&cancel_probe, joined)?;
    if let (Some(container_id), Some(mut stored)) =
        (runtime.container_id, load_signing_provisioning(&paths)?)
    {
        stored.container_id = container_id;
        save_signing_provisioning(&paths, &stored)?;
    }
    app.notify_machines_changed();

    build_mac_builder_view(app, &paths).await
}

/// Recreates the container from the machine's current profile so it picks up an option it was
/// created without — the phone's USB controller — with the disk, identity and signing kept.
/// macOS restarts once, which is the whole cost.
pub async fn rebuild_machine_container(
    app: &Engine,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MacBuilderView, String> {
    if !input.confirmed {
        return Err("Confirm the machine restart before continuing.".to_string());
    }
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let profile = machines::load_registry(app)?
        .find(&machine_id)?
        .config
        .clone();
    let guard = begin_machine_operation(app, &machine_id, "rebuilding_container")?;
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
        buildbridge_docker_osx::rebuild_container(&container_name, &profile, &options, |progress| {
            emit_machine_progress(
                &event_app,
                CONTAINER_REBUILD_PROGRESS_EVENT,
                &event_machine_id,
                progress,
            );
        })
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let runtime = finish_operation(&cancel_probe, joined)?;
    // The container is new, so the keychain record has to point at it or signing reads as lost.
    if let (Some(container_id), Some(mut stored)) =
        (runtime.container_id, load_signing_provisioning(&paths)?)
    {
        stored.container_id = container_id;
        save_signing_provisioning(&paths, &stored)?;
    }
    clear_usb_attach_issue(app, &machine_id);
    app.notify_machines_changed();

    build_mac_builder_view(app, &paths).await
}

/// Hands one host port to the running guest. The phone leaves this host until detached, or
/// until the machine stops.
pub async fn attach_usb_device(
    app: &Engine,
    machine_id: String,
    input: AttachUsbDeviceInput,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let profile = machines::load_registry(app)?
        .find(&machine_id)?
        .config
        .clone();
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    if !buildbridge_docker_osx::valid_usb_port_path(&input.port) {
        return Err("The USB port is not valid.".to_string());
    }
    let current = build_mac_builder_view(app, &paths).await?;
    let guest_reset = buildbridge_docker_osx::guest_reset_for_macos(
        current.guest.diagnostics.macos_version.as_deref(),
    );
    let guard = begin_machine_operation(app, &machine_id, "attaching_usb")?;
    let container_name = paths.container_name.clone();
    let socket = paths.qmp_endpoint(profile.provider);
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        let runtime =
            buildbridge_docker_osx::status(&container_name).map_err(|error| error.to_string())?;
        if runtime.state != ContainerState::Running {
            return Err("Start the machine before attaching a phone.".to_string());
        }
        let device = buildbridge_docker_osx::host_usb_status(None)
            .devices
            .into_iter()
            .find(|device| device.bus == input.bus && device.port == input.port)
            .ok_or_else(|| {
                "No Apple device is plugged into that port. Plug the phone in and refresh."
                    .to_string()
            })?;
        buildbridge_docker_osx::attach_usb_device(&socket, &container_name, &device, guest_reset)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let attached = finish_operation(&cancel_probe, joined)?;
    match attached.issue {
        Some(issue) if !attached.enumerated => {
            if let Ok(mut issues) = app.state().usb_attach_issues.lock() {
                issues.insert(machine_id.clone(), issue);
            }
        }
        _ => clear_usb_attach_issue(app, &machine_id),
    }
    if attached.enumerated {
        settle_attached_phone(app, &machine_id, &paths, profile.ssh_port, &access.username).await?;
    }

    build_mac_builder_view(app, &paths).await
}

/// After QEMU holds the phone: wait for macOS to register it, then pair with it, which raises
/// the Trust prompt on the phone. Attaching is one click from the person's side; the phases are
/// reported so the wait reads as a wait.
pub(crate) async fn settle_attached_phone(
    app: &Engine,
    machine_id: &str,
    paths: &MachinePaths,
    ssh_port: u16,
    username: &str,
) -> Result<(), String> {
    let guard = begin_machine_operation(app, machine_id, "settling_phone")?;
    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();
    let event_app = app.clone();
    let event_machine_id = machine_id.to_string();
    let username = username.to_string();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        let started = std::time::Instant::now();
        let report = |phase: UsbAttachPhase, detail: &str| {
            emit_machine_progress(
                &event_app,
                USB_ATTACH_PROGRESS_EVENT,
                &event_machine_id,
                UsbAttachProgress {
                    phase,
                    elapsed_seconds: started.elapsed().as_secs(),
                    detail: detail.to_string(),
                },
            );
        };
        report(
            UsbAttachPhase::WaitingForMacos,
            "macOS is enumerating the phone; this can take a minute and a half",
        );
        let mut devices = Vec::new();
        while started.elapsed() < std::time::Duration::from_secs(90) {
            devices = buildbridge_docker_osx::list_guest_devices(
                ssh_port,
                &username,
                &identity_path,
                &known_hosts_path,
            )
            .unwrap_or_default();
            if devices
                .iter()
                .any(|device| device.transport_type == buildbridge_docker_osx::TransportType::Wired)
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
        let wired = devices
            .iter()
            .find(|device| device.transport_type == buildbridge_docker_osx::TransportType::Wired);
        match wired {
            Some(device)
                if device.pairing_state != buildbridge_docker_osx::PairingState::Paired =>
            {
                if let Some(udid) = &device.udid {
                    report(
                        UsbAttachPhase::Pairing,
                        "Unlock the phone and tap Trust when it asks about this computer",
                    );
                    if let Ok(paired) = buildbridge_docker_osx::pair_guest_device(
                        ssh_port,
                        &username,
                        &identity_path,
                        &known_hosts_path,
                        udid,
                    ) {
                        devices = paired;
                    }
                }
            }
            _ => {}
        }
        report(UsbAttachPhase::Completed, "Done");
        Ok::<_, String>(devices)
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let devices = finish_operation(&cancel_probe, joined)?;
    if let Ok(mut cache) = app.state().guest_devices.lock() {
        cache.insert(machine_id.to_string(), devices);
    }

    Ok(())
}
pub async fn detach_usb_device(app: &Engine, machine_id: String) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let provider = machines::load_registry(app)?
        .find(&machine_id)?
        .config
        .provider;
    let guard = begin_machine_operation(app, &machine_id, "detaching_usb")?;
    let socket = paths.qmp_endpoint(provider);
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        buildbridge_docker_osx::detach_usb_device(&socket).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    finish_operation(&cancel_probe, joined)?;
    clear_usb_attach_issue(app, &machine_id);

    build_mac_builder_view(app, &paths).await
}

pub(crate) fn clear_usb_attach_issue(app: &Engine, machine_id: &str) {
    if let Ok(mut issues) = app.state().usb_attach_issues.lock() {
        issues.remove(machine_id);
    }
}
