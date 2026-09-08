//! Phones the guest sees: listing, pairing, device signing, and the device run.

use super::*;
use ts_rs::TS;

/// Asks the guest which phones it sees and serves the answer from the view until the next
/// listing. Holds the machine so the probe cannot interleave with a build.
/// Pairs the guest with the phone. The command raises Trust on the phone if needed and waits
/// for the answer, so this is what the trust rung's button does.
pub async fn pair_guest_device(
    app: &Engine,
    machine_id: String,
    input: PairGuestDeviceInput,
) -> Result<MachineView, String> {
    if !buildbridge_machines::valid_device_udid(&input.udid) {
        return Err("The device identifier is not a UDID.".to_string());
    }
    let guest = guest_context(app, &machine_id).await?;
    let paths = guest.paths.clone();
    let devices = run_machine_operation(app, &machine_id, "pairing_device", move || {
        buildbridge_machines::pair_guest_device(
            guest.ssh_port(),
            &guest.username,
            &guest.identity_path,
            &guest.known_hosts_path,
            &input.udid,
        )
        .map_err(|error| error.to_string())
    })
    .await?;
    if let Ok(mut cache) = app.state().guest_devices.lock() {
        cache.insert(machine_id.clone(), devices);
    }

    build_machine_view(app, &paths).await
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct OpenSafariInspectorResult {
    pub(crate) view: MachineView,
    pub(crate) inspector: buildbridge_machines::SafariInspectorResult,
}

/// Opens Safari in the guest for Web Inspector on the app running on the phone: the Develop
/// menu is turned on where macOS lets it be set over SSH, and the result says whether it took.
/// This takes no machine operation: it is one short guest command, and the moment it is wanted
/// is while a device run holds the operation and streams the app's console.
pub async fn open_safari_web_inspector(
    app: &Engine,
    machine_id: String,
) -> Result<OpenSafariInspectorResult, String> {
    let guest = guest_context(app, &machine_id).await?;
    let paths = guest.paths.clone();
    let inspector = tokio::task::spawn_blocking(move || {
        buildbridge_machines::open_safari_web_inspector(
            guest.ssh_port(),
            &guest.username,
            &guest.identity_path,
            &guest.known_hosts_path,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;

    Ok(OpenSafariInspectorResult {
        view: build_machine_view(app, &paths).await?,
        inspector,
    })
}
pub async fn list_guest_devices(app: &Engine, machine_id: String) -> Result<MachineView, String> {
    let guest = guest_context(app, &machine_id).await?;
    let paths = guest.paths.clone();
    let devices = run_machine_operation(app, &machine_id, "listing_devices", move || {
        buildbridge_machines::list_guest_devices(
            guest.ssh_port(),
            &guest.username,
            &guest.identity_path,
            &guest.known_hosts_path,
        )
        .map_err(|error| error.to_string())
    })
    .await?;
    if let Ok(mut cache) = app.state().guest_devices.lock() {
        cache.insert(machine_id.clone(), devices);
    }

    build_machine_view(app, &paths).await
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PrepareDeviceSigningInput {
    pub(crate) udid: String,
    pub(crate) device_name: String,
    pub(crate) confirmed: bool,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PrepareDeviceSigningResult {
    pub(crate) view: MachineView,
    pub(crate) certificate_created: bool,
    pub(crate) device_already_registered: bool,
    pub(crate) profile_created: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSigningPhase {
    CheckingKit,
    CreatingCertificate,
    RegisteringDevice,
    CheckingProfiles,
    CreatingProfile,
    DownloadingProfile,
    Provisioning,
    Completed,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSigningProgress {
    pub(crate) phase: DeviceSigningPhase,
    #[ts(type = "number")]
    pub(crate) elapsed_seconds: u64,
    pub(crate) detail: String,
}

/// Makes one phone buildable: a development identity in the kit (created at Apple if needed),
/// the phone registered with the team, a development profile that lists it, and both
/// identities provisioned into the guest keychain. Each step persists before the next, so a
/// retry resumes rather than repeats, and nothing at Apple is revoked.
pub async fn prepare_apple_device_signing(
    app: &Engine,
    machine_id: String,
    input: PrepareDeviceSigningInput,
) -> Result<PrepareDeviceSigningResult, String> {
    if !input.confirmed {
        return Err("Confirm the device registration before continuing.".to_string());
    }
    let udid = input.udid.trim().to_ascii_uppercase();
    apple_api::validate_device_udid(&udid)?;
    let device_name = apple_api::validate_device_name(&input.device_name)?;
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let mut workspace = load_apple_workspace(&paths)?
        .ok_or_else(|| "Approve and verify an Apple project first.".to_string())?;
    if !workspace.last_build_succeeded {
        return Err("Complete the unsigned project test build first.".to_string());
    }
    let bundle_identifier = workspace.bundle_identifier.clone().ok_or_else(|| {
        "buildbridge could not detect one release bundle identifier. Re-approve the project after setting PRODUCT_BUNDLE_IDENTIFIER in Xcode."
            .to_string()
    })?;
    let current = build_machine_view(app, &paths).await?;
    ensure_apple_project_guest_ready(&current)?;
    let mut kit = resolve_signing_kit_for(app, &machine_id).await?;
    let (key_id, issuer_id, private_key) = match (
        kit.app_store_connect_key_id.clone(),
        kit.app_store_connect_issuer_id.clone(),
        kit.app_store_connect_private_key.clone(),
    ) {
        (Some(key_id), Some(issuer_id), Some(private_key)) => (key_id, issuer_id, private_key),
        _ => {
            return Err(
                "These credentials have no App Store Connect key. Device signing needs a Team key with the Admin role to register the iPhone and create a development profile."
                    .to_string(),
            );
        }
    };
    if kit.guest_keychain_password.is_none() {
        return Err(
            "Store a dedicated guest keychain password in the operating-system vault first."
                .to_string(),
        );
    }

    // The identifier the Debug build carries, which is what its profile must be for. A project
    // often gives Debug its own suffixed identifier so both builds fit on one phone.
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username first.".to_string())?;
    let profile = machines::load_registry(app)?
        .find(&machine_id)?
        .config
        .clone();
    let device_bundle_identifier = {
        let identity_path = paths.guest_identity();
        let known_hosts_path = paths.known_hosts();
        let layout = workspace.layout.clone();
        let ssh_port = profile.ssh_port;
        let username = access.username.clone();
        tokio::task::spawn_blocking(move || {
            buildbridge_machines::resolve_debug_bundle_identifier(
                ssh_port,
                &username,
                &identity_path,
                &known_hosts_path,
                &layout,
            )
            .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())??
    };
    if device_bundle_identifier != bundle_identifier {
        apple_api::validate_bundle_identifier(&device_bundle_identifier)?;
    }

    // Already prepared for this phone: nothing to do beyond confirming the registration.
    if kit.development_certificate_path.is_some()
        && current.signing.as_ref().is_some_and(|signing| {
            buildbridge_machines::select_development_profile(
                signing,
                &device_bundle_identifier,
                &udid,
            )
            .is_some()
        })
    {
        let registered =
            apple_api::register_device(&key_id, &issuer_id, &private_key, &udid, &device_name)
                .await?;
        return Ok(PrepareDeviceSigningResult {
            view: current,
            certificate_created: false,
            device_already_registered: registered.already_registered,
            profile_created: false,
        });
    }

    let guard = begin_machine_operation(app, &machine_id, "preparing_device_signing")?;
    let started = std::time::Instant::now();
    let report = |phase: DeviceSigningPhase, detail: &str| {
        emit_machine_progress(
            app,
            DEVICE_SIGNING_PROGRESS_EVENT,
            &machine_id,
            DeviceSigningProgress {
                phase,
                elapsed_seconds: started.elapsed().as_secs(),
                detail: detail.to_string(),
            },
        );
    };
    let outcome: Result<(bool, bool, bool), String> = async {
        report(
            DeviceSigningPhase::CheckingKit,
            "Checking the credentials for a development identity",
        );
        let mut certificate_created = false;
        if kit.development_certificate_path.is_none() {
            report(
                DeviceSigningPhase::CreatingCertificate,
                "Creating an Apple Development certificate for a key generated on this host",
            );
            create_apple_certificate_for_kit(app, &mut kit, apple_api::CertificateKind::Development)
                .await?;
            certificate_created = true;
        }
        let serial = tokio::task::spawn_blocking({
            let kit = kit.clone();
            move || development_certificate_serial(&kit)
        })
        .await
        .map_err(|error| error.to_string())??;
        if kit.development_certificate_serial_number.is_none() {
            kit.development_certificate_serial_number = Some(serial.clone());
            save_signing_kit_record(kit.clone()).await?;
        }
        let found = apple_api::find_certificate_by_serial(
            &key_id,
            &issuer_id,
            &private_key,
            &serial,
            apple_api::CertificateKind::Development,
        )
        .await?;
        let certificate = match found {
            Some(certificate) => certificate,
            None => {
                // Expired, revoked, or from another team: a development certificate is cheap
                // and per-team, so a new one is created rather than the kit sent back to edit.
                report(
                    DeviceSigningPhase::CreatingCertificate,
                    "The stored development certificate is no longer valid at Apple; creating a new one",
                );
                create_apple_certificate_for_kit(
                    app,
                    &mut kit,
                    apple_api::CertificateKind::Development,
                )
                .await?;
                certificate_created = true;
                let serial = kit.development_certificate_serial_number.clone().ok_or_else(|| {
                    "buildbridge created a development certificate but recorded no serial for it."
                        .to_string()
                })?;
                apple_api::find_certificate_by_serial(
                    &key_id,
                    &issuer_id,
                    &private_key,
                    &serial,
                    apple_api::CertificateKind::Development,
                )
                .await?
                .ok_or_else(|| {
                    format!(
                        "Apple issued development certificate {serial} but does not list it yet. Try again in a moment."
                    )
                })?
            }
        };

        report(
            DeviceSigningPhase::RegisteringDevice,
            &format!("Registering {device_name} with the team"),
        );
        let registered =
            apple_api::register_device(&key_id, &issuer_id, &private_key, &udid, &device_name)
                .await?;

        if device_bundle_identifier != bundle_identifier {
            report(
                DeviceSigningPhase::CheckingProfiles,
                &format!(
                    "Registering the Debug identifier {device_bundle_identifier} at Apple with the main app's capabilities"
                ),
            );
            let ensured = apple_api::ensure_bundle_id(
                &key_id,
                &issuer_id,
                &private_key,
                &device_bundle_identifier,
                &format!("{} Debug", workspace.name),
            )
            .await?;
            if ensured.created {
                let main = apple_api::ensure_bundle_id(
                    &key_id,
                    &issuer_id,
                    &private_key,
                    &bundle_identifier,
                    &workspace.name,
                )
                .await?;
                apple_api::copy_bundle_id_capabilities(
                    &key_id,
                    &issuer_id,
                    &private_key,
                    &main.id,
                    &ensured.id,
                )
                .await?;
            }
            if workspace.debug_bundle_identifier.as_deref() != Some(&device_bundle_identifier) {
                workspace.debug_bundle_identifier = Some(device_bundle_identifier.clone());
                save_apple_workspace(&paths, &workspace)?;
            }
        }
        report(
            DeviceSigningPhase::CheckingProfiles,
            "Checking the team's development profiles for this phone",
        );
        let search = apple_api::find_development_profile(
            &key_id,
            &issuer_id,
            &private_key,
            &device_bundle_identifier,
            &certificate.id,
            &udid,
        )
        .await?;
        let mut profile_created = false;
        let (profile, content) = match search.matching {
            Some(profile) if kit_holds_profile_uuid(&kit, &profile.uuid) => (profile, None),
            Some(profile) => {
                report(
                    DeviceSigningPhase::DownloadingProfile,
                    "Downloading the development profile that already lists this phone",
                );
                let (profile, content) =
                    apple_api::download_profile(&key_id, &issuer_id, &private_key, &profile.id)
                        .await?;
                (profile, Some(content))
            }
            None => {
                report(
                    DeviceSigningPhase::CreatingProfile,
                    "Creating a development profile listing this phone",
                );
                let mut device_ids = search.superseded_device_ids;
                if !device_ids.contains(&registered.device.id) {
                    device_ids.push(registered.device.id.clone());
                }
                let created = apple_api::create_development_profile(
                    &key_id,
                    &issuer_id,
                    &private_key,
                    &device_bundle_identifier,
                    &certificate.id,
                    &device_ids,
                )
                .await?;
                profile_created = true;
                (created.profile, Some(created.content))
            }
        };
        if let Some(content) = content {
            if kit.provisioning_profile_paths.len() >= MAX_PROVISIONING_PROFILES {
                return Err(format!(
                    "These credentials already hold {MAX_PROVISIONING_PROFILES} profiles. Remove obsolete paths before adding another."
                ));
            }
            let saved_path = save_managed_apple_profile(app, &profile, &content)?;
            let saved_path = saved_path
                .to_str()
                .ok_or_else(|| "The managed profile path is not valid UTF-8.".to_string())?
                .to_string();
            if !kit
                .provisioning_profile_paths
                .iter()
                .any(|path| path == &saved_path)
            {
                kit.provisioning_profile_paths.push(saved_path);
            }
            save_signing_kit_record(kit.clone()).await?;
        }

        Ok((
            certificate_created,
            registered.already_registered,
            profile_created,
        ))
    }
    .await;
    drop(guard);
    let (certificate_created, device_already_registered, profile_created) = outcome?;

    report(
        DeviceSigningPhase::Provisioning,
        "Provisioning both identities and every profile into the guest keychain",
    );
    let view = provision_with_kit(app, &machine_id, kit).await?;
    let ready = view.signing.as_ref().is_some_and(|signing| {
        buildbridge_machines::select_development_profile(signing, &device_bundle_identifier, &udid)
            .is_some()
    });
    if !ready {
        return Err(
            "Provisioning finished, but the installed development profile does not list this iPhone. Verify the team and try again."
                .to_string(),
        );
    }
    report(
        DeviceSigningPhase::Completed,
        "Ready to sign for this iPhone",
    );

    Ok(PrepareDeviceSigningResult {
        view,
        certificate_created,
        device_already_registered,
        profile_created,
    })
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RunAppleDeviceBuildInput {
    pub(crate) udid: String,
    #[serde(default)]
    pub(crate) env_set_id: Option<String>,
    /// A version to build with, written into the project first; none builds it as synced.
    #[serde(default)]
    pub(crate) version: Option<ProjectVersionInput>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RunAppleDeviceResult {
    pub(crate) view: MachineView,
    pub(crate) run: AppleDeviceRunResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StoredAppleDeviceRun {
    pub(crate) container_id: String,
    pub(crate) snapshot_sha256: String,
    pub(crate) device_identifier: String,
    pub(crate) result: AppleDeviceRunResult,
    #[ts(type = "number")]
    pub(crate) finished_at_epoch_seconds: u64,
}

/// The phone as the view last heard of it from the guest, by UDID.
fn listed_guest_device(
    view: &MachineView,
    udid: &str,
) -> Option<buildbridge_machines::GuestDevice> {
    view.guest
        .devices
        .iter()
        .find(|device| {
            device
                .udid
                .as_deref()
                .is_some_and(|listed| listed.eq_ignore_ascii_case(udid))
        })
        .cloned()
}

/// Builds the Debug configuration for one phone, installs and launches it, and streams its
/// console until the session ends. A Stop while the app runs is the normal end and the run is
/// retained; a failure before launch is kept as the step's diagnostic. A process that has not
/// listed the guest's phones yet asks once here, so the terminal can run without listing first.
pub async fn run_apple_device_build(
    app: &Engine,
    machine_id: String,
    input: RunAppleDeviceBuildInput,
) -> Result<RunAppleDeviceResult, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let udid = input.udid.trim().to_ascii_uppercase();
    apple_api::validate_device_udid(&udid)?;
    remove_apple_device_run_error(&paths)?;
    let chosen_env = guest_env_files_for_set(input.env_set_id.as_deref()).await?;
    let profile = machines::load_registry(app)?
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
    let requested_version =
        resolve_apple_project_version(&workspace.local_path, &workspace.layout, input.version)?;
    let current = build_machine_view(app, &paths).await?;
    ensure_apple_project_guest_ready(&current)?;
    let signing = current
        .signing
        .clone()
        .ok_or_else(|| "Provision and verify signing in macOS first.".to_string())?;
    let device_profile = buildbridge_machines::select_development_profile(
        &signing,
        workspace
            .debug_bundle_identifier
            .as_deref()
            .unwrap_or(&signing.bundle_identifier),
        &udid,
    )
    .cloned()
    .ok_or_else(|| "Prepare signing for this iPhone first.".to_string())?;
    let identity = signing
        .development_identity
        .clone()
        .ok_or_else(|| "The credentials have no development identity provisioned.".to_string())?;
    let device = match listed_guest_device(&current, &udid) {
        Some(device) => device,
        None => {
            let refreshed = list_guest_devices(app, machine_id.clone()).await?;
            listed_guest_device(&refreshed, &udid).ok_or_else(|| {
                "The guest does not list that iPhone. Refresh the devices and try again."
                    .to_string()
            })?
        }
    };
    if !device.ready {
        return Err(device
            .issue
            .clone()
            .unwrap_or_else(|| "The iPhone is not ready for a build.".to_string()));
    }
    let secrets = resolve_signing_kit_for(app, &machine_id).await?;
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
    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();
    let layout = workspace.layout.clone();
    let guard = begin_machine_operation(app, &machine_id, "running_on_device")?;
    // The project takes the version before the build does, so it is never behind the phone.
    if let Some(version) = &requested_version
        && let Err(error) =
            write_apple_project_version(&workspace.local_path, &workspace.layout, version)
    {
        drop(guard);
        return Err(error);
    }

    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_machines::enter_operation(scope);
        let device_signing = buildbridge_machines::DeviceSigning {
            keychain_path: &signing.keychain_path,
            identity_sha1: &identity.identity_sha1,
            development_team: &signing.development_team,
            bundle_identifier: &signing.bundle_identifier,
            profile: &device_profile,
        };
        buildbridge_machines::run_apple_device_build(
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            &device_signing,
            &layout,
            &device,
            &keychain_password,
            chosen_env.as_ref().map(|(_, files)| files),
            requested_version.as_ref(),
            |progress: AppleDeviceRunProgress| {
                emit_machine_progress(
                    &event_app,
                    DEVICE_RUN_PROGRESS_EVENT,
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
    let run = match finish_operation(&cancel_probe, joined) {
        Ok(run) => run,
        Err(error) => {
            if error != CANCELLED_MESSAGE {
                let _ = save_apple_device_run_error(&paths, &error);
            }
            return Err(error);
        }
    };
    save_apple_device_run(
        &paths,
        &StoredAppleDeviceRun {
            container_id,
            snapshot_sha256,
            device_identifier: run.device.identifier.clone(),
            result: run.clone(),
            finished_at_epoch_seconds: machines::now_epoch_seconds(),
        },
    )?;
    remove_apple_device_run_error(&paths)?;
    let view = build_machine_view(app, &paths).await?;

    Ok(RunAppleDeviceResult { view, run })
}
pub async fn clear_apple_device_run(
    app: &Engine,
    machine_id: String,
) -> Result<MachineView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    remove_file_if_present(&paths.apple_device_run_record())?;
    remove_apple_device_run_error(&paths)?;

    build_machine_view(app, &paths).await
}

/// Copies the Podfile.lock CocoaPods wrote in the guest into the approved host project, so a
/// drifted lock can be adopted without a Mac. The host copy it replaces is kept beside the
/// machine's records, the change is reported pod by pod, and the archive's drift block lifts:
/// the guest workspace already compiled with exactly this lock. Committing it stays the user's.
pub async fn adopt_guest_podfile_lock(
    app: &Engine,
    machine_id: String,
) -> Result<AdoptPodfileLockResult, String> {
    let guest = guest_context(app, &machine_id).await?;
    let paths = guest.paths.clone();
    let mut workspace = load_apple_workspace(&paths)?
        .ok_or_else(|| "Approve and synchronize a local Apple project first.".to_string())?;
    if !workspace.last_native_lock_updated {
        return Err(
            "The guest did not refresh Podfile.lock in its last test build, so there is nothing to adopt."
                .to_string(),
        );
    }
    let podfile_dir = workspace
        .layout
        .ios
        .as_ref()
        .and_then(|ios| ios.podfile_dir.clone())
        .ok_or_else(|| {
            "The approved project has no Podfile, so there is no lock to adopt.".to_string()
        })?;
    let host_lock = PathBuf::from(&workspace.local_path)
        .join(&podfile_dir)
        .join("Podfile.lock");
    let layout = workspace.layout.clone();
    let guest_lock = run_machine_operation(app, &machine_id, "adopting_lock", move || {
        buildbridge_machines::read_guest_podfile_lock(
            guest.ssh_port(),
            &guest.username,
            &guest.identity_path,
            &guest.known_hosts_path,
            &layout,
        )
        .map_err(|error| error.to_string())
    })
    .await?;

    let before = match fs::read_to_string(&host_lock) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(format!(
                "The project's Podfile.lock could not be read: {error}"
            ));
        }
    };
    let changes = buildbridge_machines::podfile_lock_changes(&before, &guest_lock);
    let backup = paths.podfile_lock_backup();
    if !before.is_empty() {
        write_restricted_file(&backup, before.as_bytes())?;
    }
    let incoming = host_lock.with_extension("lock.buildbridge-incoming");
    fs::write(&incoming, guest_lock.as_bytes())
        .and_then(|()| fs::rename(&incoming, &host_lock))
        .map_err(|error| {
            let _ = fs::remove_file(&incoming);
            format!("The project's Podfile.lock could not be replaced: {error}")
        })?;

    workspace.last_native_lock_updated = false;
    save_apple_workspace(&paths, &workspace)?;
    let view = build_machine_view(app, &paths).await?;

    Ok(AdoptPodfileLockResult {
        view,
        changes,
        host_path: host_lock.to_string_lossy().into_owned(),
        backup_path: backup.to_string_lossy().into_owned(),
    })
}

pub(crate) fn load_apple_device_run(
    paths: &MachinePaths,
) -> Result<Option<StoredAppleDeviceRun>, String> {
    match fs::read(paths.apple_device_run_record()) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("The device run record is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn save_apple_device_run(
    paths: &MachinePaths,
    run: &StoredAppleDeviceRun,
) -> Result<(), String> {
    let encoded = serde_json::to_vec_pretty(run).map_err(|error| error.to_string())?;

    write_restricted_file(&paths.apple_device_run_record(), &encoded)
}

pub(crate) fn save_apple_device_run_error(paths: &MachinePaths, error: &str) -> Result<(), String> {
    let error = error
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .take(8_000)
        .collect::<String>();
    write_restricted_file(&paths.apple_device_run_error(), error.as_bytes())
}

pub(crate) fn remove_apple_device_run_error(paths: &MachinePaths) -> Result<(), String> {
    remove_file_if_present(&paths.apple_device_run_error())
}
