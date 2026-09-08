//! The Android machine's project work: approving a Capacitor project with its Android
//! platform, synchronizing it into the toolchain container, the debug build, and the signed
//! release with its retained artifacts. The shape follows the Apple commands step for step.

use super::*;

pub async fn approve_android_workspace(
    app: &Engine,
    machine_id: String,
    input: ApproveAndroidWorkspaceInput,
) -> Result<MachineView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    ensure_android_machine(app, &machine_id)?;
    let approved = inspect_android_workspace(input.path.trim(), input.module.as_deref())?;
    let guard = begin_machine_operation(app, &machine_id, "approving_android_workspace")?;
    let workspace = match load_android_workspace(&paths)? {
        Some(existing) if existing.local_path == approved.local_path => StoredAndroidWorkspace {
            name: approved.name,
            layout: approved.layout,
            application_id: approved.application_id,
            ..existing
        },
        _ => approved,
    };
    save_android_workspace(&paths, &workspace)?;

    drop(guard);
    build_machine_view(app, &paths).await
}

pub async fn clear_android_workspace(
    app: &Engine,
    machine_id: String,
) -> Result<MachineView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let guard = begin_machine_operation(app, &machine_id, "clearing_android_workspace")?;
    remove_file_if_present(&paths.android_workspace())?;

    drop(guard);
    build_machine_view(app, &paths).await
}

pub async fn sync_android_workspace(
    app: &Engine,
    machine_id: String,
) -> Result<SyncAndroidWorkspaceResult, String> {
    sync_android_workspace_from(app, &machine_id, None).await
}

/// Synchronizes a source tree into the container: the approved folder as it is, or a
/// checked-out revision of the same project when a remote build names a ref.
pub(crate) async fn sync_android_workspace_from(
    app: &Engine,
    machine_id: &str,
    source: Option<(PathBuf, WorkspaceSource)>,
) -> Result<SyncAndroidWorkspaceResult, String> {
    sync_android_workspace_with_env(app, machine_id, source, None).await
}

pub(crate) async fn sync_android_workspace_with_env(
    app: &Engine,
    machine_id: &str,
    source: Option<(PathBuf, WorkspaceSource)>,
    env_override: Option<Option<String>>,
) -> Result<SyncAndroidWorkspaceResult, String> {
    let paths = MachinePaths::resolve(app, machine_id)?;
    ensure_android_machine(app, machine_id)?;
    let guard = begin_machine_operation(app, machine_id, "synchronizing")?;
    let mut workspace = load_android_workspace(&paths)?
        .ok_or_else(|| "Approve a local Android project first.".to_string())?;
    let current = build_machine_view(app, &paths).await?;
    ensure_android_container_ready(&current)?;
    let (workspace_path, source) = source.unwrap_or_else(|| {
        (
            PathBuf::from(&workspace.local_path),
            WorkspaceSource::folder(),
        )
    });
    let env_files = match env_override {
        Some(id) => guest_env_files_for_set(id.as_deref())
            .await?
            .map(|(_, files)| files),
        None => guest_env_files_for(app, machine_id).await?,
    };
    let event_app = app.clone();
    let event_machine_id = machine_id.to_string();
    let container_name = paths.container_name.clone();
    let layout = workspace.layout.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_machines::enter_operation(scope);
        buildbridge_machines::sync_android_workspace(
            &container_name,
            &workspace_path,
            &layout,
            env_files.as_ref(),
            |progress: AndroidBuildProgress| {
                emit_machine_progress(
                    &event_app,
                    ANDROID_BUILD_PROGRESS_EVENT,
                    &event_machine_id,
                    progress,
                );
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    let sync = finish_operation(&cancel_probe, joined)?;

    workspace.last_snapshot_sha256 = Some(sync.snapshot_sha256.clone());
    workspace.last_sync_file_count = Some(sync.source_file_count);
    workspace.last_sync_bytes = Some(sync.source_bytes);
    workspace.last_synced_at_epoch_seconds = Some(crate::machines::now_epoch_seconds());
    workspace.last_build_succeeded = false;
    workspace.last_build = None;
    workspace.last_source = Some(source);
    save_android_workspace(&paths, &workspace)?;
    drop(guard);
    let view = build_machine_view(app, &paths).await?;

    Ok(SyncAndroidWorkspaceResult { view, sync })
}

/// The debug build: the Android counterpart of the unsigned test build. The first run also
/// prepares the toolchain, which is where its download shows its progress. `allow_http` applies
/// only to this APK; neither the workspace nor a subsequent release inherits the override.
/// The debug build. A requested version is written into the project on the host and into
/// the synced copy in the container first, so the project and the APK say the same thing.
pub async fn run_android_debug_build(
    app: &Engine,
    machine_id: String,
    allow_http: bool,
    version: Option<ProjectVersionInput>,
) -> Result<RunAndroidBuildResult, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    ensure_android_machine(app, &machine_id)?;
    let guard = begin_machine_operation(app, &machine_id, "test_building")?;
    let mut workspace = load_android_workspace(&paths)?
        .ok_or_else(|| "Approve and synchronize a local Android project first.".to_string())?;
    if workspace.last_snapshot_sha256.is_none() {
        return Err("Synchronize the approved project before running a debug build.".to_string());
    }
    let requested_version =
        resolve_android_project_version(&workspace.local_path, &workspace.layout, version)?;
    let current = build_machine_view(app, &paths).await?;
    ensure_android_container_ready(&current)?;
    // Hold the operation lock before removing the previous APK so an in-flight device
    // installation can finish reading it. A new debug build replaces this retained output.
    remove_android_debug_outputs(&paths)?;
    let output_directory = prepare_android_debug_output_dir(&paths)?;
    // The project takes the version before the build does, so it is never behind an APK.
    if let Some(version) = &requested_version
        && let Err(error) =
            write_android_project_version(&workspace.local_path, &workspace.layout, version)
    {
        drop(guard);
        let _ = fs::remove_dir(&output_directory);
        return Err(error);
    }
    workspace.last_build_succeeded = false;
    workspace.last_build = None;
    if let Err(error) = save_android_workspace(&paths, &workspace) {
        drop(guard);
        let _ = fs::remove_dir(&output_directory);
        return Err(error);
    }

    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let container_name = paths.container_name.clone();
    let operation_output_directory = output_directory.clone();
    let layout = workspace.layout.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_machines::enter_operation(scope);
        buildbridge_machines::run_android_debug_build(
            &container_name,
            &layout,
            &operation_output_directory,
            allow_http,
            requested_version.as_ref(),
            |progress: AndroidBuildProgress| {
                emit_machine_progress(
                    &event_app,
                    ANDROID_BUILD_PROGRESS_EVENT,
                    &event_machine_id,
                    progress,
                );
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    let build = match finish_operation(&cancel_probe, joined) {
        Ok(build) => build,
        Err(error) => {
            let _ = fs::remove_dir_all(&output_directory);
            return Err(error);
        }
    };

    workspace.last_build_succeeded = true;
    workspace.last_build = Some(build.clone());
    save_android_workspace(&paths, &workspace)?;
    drop(guard);
    let view = build_machine_view(app, &paths).await?;

    Ok(RunAndroidBuildResult { view, build })
}

/// The signed release: the attached kit's upload key signs the release bundle and APK, and
/// both are retained under the machine's artifacts with their checksums. A requested version
/// is written into the project on the host and into the synced copy in the container, so
/// the project and the release say the same thing without a new sync.
pub async fn run_android_signed_release(
    app: &Engine,
    machine_id: String,
    env_set_id: Option<String>,
    outputs: Option<AndroidReleaseOutputs>,
    version: Option<ProjectVersionInput>,
) -> Result<RunAndroidReleaseResult, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    ensure_android_machine(app, &machine_id)?;
    let guard = begin_machine_operation(app, &machine_id, "releasing")?;
    remove_android_release_error(&paths)?;
    let chosen_env = guest_env_files_for_set(env_set_id.as_deref()).await?;
    let workspace = load_android_workspace(&paths)?
        .ok_or_else(|| "Approve and synchronize a local Android project first.".to_string())?;
    if !workspace.last_build_succeeded || workspace.last_snapshot_sha256.is_none() {
        return Err("Complete the debug build first.".to_string());
    }
    let requested_version =
        resolve_android_project_version(&workspace.local_path, &workspace.layout, version)?;
    let current = build_machine_view(app, &paths).await?;
    ensure_android_container_ready(&current)?;
    let kit = resolve_signing_kit_for(app, &machine_id).await?;
    let signing = android_signing_material(&kit)?;
    let container_id = current
        .runtime
        .container_id
        .clone()
        .ok_or_else(|| "The toolchain container identity is unavailable.".to_string())?;
    let snapshot_sha256 = workspace
        .last_snapshot_sha256
        .clone()
        .expect("checked above");
    let output_directory = prepare_android_release_output_dir(&paths)?;
    // The project takes the version before the release does, so it is never behind an APK.
    if let Some(version) = &requested_version
        && let Err(error) =
            write_android_project_version(&workspace.local_path, &workspace.layout, version)
    {
        let _ = fs::remove_dir_all(&output_directory);
        return Err(error);
    }

    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let container_name = paths.container_name.clone();
    let operation_output_directory = output_directory.clone();
    let env_set_name = chosen_env.as_ref().map(|(name, _)| name.clone());
    let layout = workspace.layout.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_machines::enter_operation(scope);
        buildbridge_machines::run_signed_android_release(
            &container_name,
            &layout,
            &signing,
            chosen_env.as_ref().map(|(_, files)| files),
            outputs.unwrap_or_default(),
            requested_version.as_ref(),
            &operation_output_directory,
            |progress: AndroidReleaseProgress| {
                emit_machine_progress(
                    &event_app,
                    ANDROID_RELEASE_PROGRESS_EVENT,
                    &event_machine_id,
                    progress,
                );
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    let release = match finish_operation(&cancel_probe, joined) {
        Ok(release) => release,
        Err(error) => {
            let _ = fs::remove_dir_all(&output_directory);
            if error != CANCELLED_MESSAGE {
                let _ = save_android_release_error(&paths, &error);
            }
            return Err(error);
        }
    };
    if let Err(error) = save_android_release(
        &paths,
        &StoredAndroidRelease {
            container_id,
            snapshot_sha256,
            result: release.clone(),
            env_set_name,
        },
    ) {
        let _ = fs::remove_dir_all(&output_directory);
        let _ = save_android_release_error(&paths, &error);
        return Err(error);
    }
    remove_android_release_error(&paths)?;
    drop(guard);
    let view = build_machine_view(app, &paths).await?;

    Ok(RunAndroidReleaseResult { view, release })
}

/// Opens the folder holding the last debug APK, the one a phone takes over `adb install`.
pub async fn reveal_android_debug_apk(app: &Engine, machine_id: String) -> Result<(), String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let apk = load_android_workspace(&paths)?
        .and_then(|workspace| workspace.last_build)
        .and_then(|build| build.apk)
        .ok_or_else(|| "No debug APK is retained for this machine.".to_string())?;
    let directory = validated_android_artifact_directory(&paths, &apk.path, "debug-")?;
    reveal_directory(&directory, "the debug APK")
}

pub async fn reveal_android_release(app: &Engine, machine_id: String) -> Result<(), String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let stored = load_android_release(&paths)?
        .ok_or_else(|| "No retained signed release is available.".to_string())?;
    let directory = validated_android_release_directory(&paths, &stored.result)?;
    reveal_directory(&directory, "the signed artifacts")
}

pub async fn clear_android_release(
    app: &Engine,
    machine_id: String,
) -> Result<MachineView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let guard = begin_machine_operation(app, &machine_id, "clearing_android_release")?;
    if let Some(stored) = load_android_release(&paths)? {
        let directory = validated_android_release_directory(&paths, &stored.result)?;
        fs::remove_dir_all(directory)
            .map_err(|error| format!("Could not remove the signed artifacts: {error}"))?;
    }
    remove_android_release_record(&paths)?;
    remove_android_release_error(&paths)?;

    drop(guard);
    build_machine_view(app, &paths).await
}

fn ensure_android_machine(app: &Engine, machine_id: &str) -> Result<(), String> {
    if machines::load_registry(app)?
        .find(machine_id)?
        .config
        .provider
        .is_macos()
    {
        return Err(
            "This is a macOS machine; approve the project as an Apple project.".to_string(),
        );
    }

    Ok(())
}

#[cfg(test)]
mod operation_tests {
    use super::*;

    #[tokio::test]
    async fn retained_android_files_cannot_change_during_installation_or_upload() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "buildbridge-android-artifact-lock-{}-{nonce}",
            std::process::id()
        ));
        let app = Engine::new(EngineDeps {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            events: Arc::new(NoEvents),
        });
        let id = "android-artifact-lock".to_string();
        machines::save_registry(
            &app,
            &machines::MachineRegistry {
                machines: vec![StoredMachine {
                    id: id.clone(),
                    config: MachineConfig {
                        provider: MachineProvider::AndroidToolchain,
                        ..MachineConfig::default()
                    },
                    created_at_epoch_seconds: 0,
                    signing_kit_id: None,
                    env_set_id: None,
                    template_id: None,
                }],
            },
        )
        .unwrap();
        let paths = MachinePaths::resolve(&app, &id).unwrap();
        let debug = paths.artifacts_dir().join("debug-retained");
        fs::create_dir_all(&debug).unwrap();
        let apk = debug.join("app-debug.apk");
        fs::write(&apk, b"retained APK").unwrap();
        write_restricted_file(&paths.android_workspace(), b"retained workspace").unwrap();
        for label in ["running_android_device", "uploading_google_play"] {
            let guard = begin_machine_operation(&app, &id, label).unwrap();
            let errors = [
                clear_android_workspace(&app, id.clone()).await.unwrap_err(),
                clear_android_release(&app, id.clone()).await.unwrap_err(),
                run_android_debug_build(&app, id.clone(), false, None)
                    .await
                    .unwrap_err(),
                run_android_signed_release(&app, id.clone(), None, None, None)
                    .await
                    .unwrap_err(),
                sync_android_workspace(&app, id.clone()).await.unwrap_err(),
            ];
            for error in errors {
                assert!(
                    error.contains("Another operation is still running"),
                    "{error}"
                );
            }
            assert_eq!(fs::read(&apk).unwrap(), b"retained APK");
            assert_eq!(
                fs::read(paths.android_workspace()).unwrap(),
                b"retained workspace"
            );
            drop(guard);
        }
        fs::remove_dir_all(root).unwrap();
    }
}
