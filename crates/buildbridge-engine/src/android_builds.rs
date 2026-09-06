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
    let approved = inspect_android_workspace(input.path.trim())?;
    let workspace = match load_android_workspace(&paths)? {
        Some(existing) if existing.local_path == approved.local_path => StoredAndroidWorkspace {
            name: approved.name,
            application_id: approved.application_id,
            ..existing
        },
        _ => approved,
    };
    save_android_workspace(&paths, &workspace)?;

    build_machine_view(app, &paths).await
}

pub async fn clear_android_workspace(
    app: &Engine,
    machine_id: String,
) -> Result<MachineView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    remove_file_if_present(&paths.android_workspace())?;

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
    let paths = MachinePaths::resolve(app, machine_id)?;
    ensure_android_machine(app, machine_id)?;
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
    let env_files = guest_env_files_for(app, machine_id).await?;
    let guard = begin_machine_operation(app, machine_id, "synchronizing")?;

    let event_app = app.clone();
    let event_machine_id = machine_id.to_string();
    let container_name = paths.container_name.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_machines::enter_operation(scope);
        buildbridge_machines::sync_android_workspace(
            &container_name,
            &workspace_path,
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
    drop(guard);
    let sync = finish_operation(&cancel_probe, joined)?;

    workspace.last_snapshot_sha256 = Some(sync.snapshot_sha256.clone());
    workspace.last_sync_file_count = Some(sync.source_file_count);
    workspace.last_sync_bytes = Some(sync.source_bytes);
    workspace.last_build_succeeded = false;
    workspace.last_build = None;
    workspace.last_source = Some(source);
    save_android_workspace(&paths, &workspace)?;
    let view = build_machine_view(app, &paths).await?;

    Ok(SyncAndroidWorkspaceResult { view, sync })
}

/// The debug build: the Android counterpart of the unsigned test build. The first run also
/// prepares the toolchain, which is where its download shows its progress.
pub async fn run_android_debug_build(
    app: &Engine,
    machine_id: String,
) -> Result<RunAndroidBuildResult, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    ensure_android_machine(app, &machine_id)?;
    let mut workspace = load_android_workspace(&paths)?
        .ok_or_else(|| "Approve and synchronize a local Android project first.".to_string())?;
    if workspace.last_snapshot_sha256.is_none() {
        return Err("Synchronize the approved project before running a debug build.".to_string());
    }
    let current = build_machine_view(app, &paths).await?;
    ensure_android_container_ready(&current)?;
    // One debug APK is kept per machine: the previous build's directory goes before this one
    // starts, so a failed build leaves the last good APK in place until it is replaced.
    remove_android_debug_outputs(&paths)?;
    let output_directory = prepare_android_debug_output_dir(&paths)?;
    let guard = match begin_machine_operation(app, &machine_id, "test_building") {
        Ok(guard) => guard,
        Err(error) => {
            let _ = fs::remove_dir(&output_directory);
            return Err(error);
        }
    };
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
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_machines::enter_operation(scope);
        buildbridge_machines::run_android_debug_build(
            &container_name,
            &operation_output_directory,
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
    drop(guard);
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
    let view = build_machine_view(app, &paths).await?;

    Ok(RunAndroidBuildResult { view, build })
}

/// The signed release: the attached kit's upload key signs the release bundle and APK, and
/// both are retained under the machine's artifacts with their checksums.
pub async fn run_android_signed_release(
    app: &Engine,
    machine_id: String,
    env_set_id: Option<String>,
) -> Result<RunAndroidReleaseResult, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    ensure_android_machine(app, &machine_id)?;
    remove_android_release_error(&paths)?;
    let chosen_env = guest_env_files_for_set(env_set_id.as_deref()).await?;
    let workspace = load_android_workspace(&paths)?
        .ok_or_else(|| "Approve and synchronize a local Android project first.".to_string())?;
    if !workspace.last_build_succeeded || workspace.last_snapshot_sha256.is_none() {
        return Err("Complete the debug build first.".to_string());
    }
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
    let guard = match begin_machine_operation(app, &machine_id, "releasing") {
        Ok(guard) => guard,
        Err(error) => {
            let _ = fs::remove_dir(&output_directory);
            return Err(error);
        }
    };

    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let container_name = paths.container_name.clone();
    let operation_output_directory = output_directory.clone();
    let env_set_name = chosen_env.as_ref().map(|(name, _)| name.clone());
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_machines::enter_operation(scope);
        buildbridge_machines::run_signed_android_release(
            &container_name,
            &signing,
            chosen_env.as_ref().map(|(_, files)| files),
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
    drop(guard);
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

pub(crate) fn reveal_directory(directory: &std::path::Path, what: &str) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(target_os = "windows") {
        Command::new("explorer")
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(directory)
        .spawn()
        .map_err(|error| format!("Could not reveal {what}: {error}"))?;

    Ok(())
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
    if let Some(stored) = load_android_release(&paths)? {
        let directory = validated_android_release_directory(&paths, &stored.result)?;
        fs::remove_dir_all(directory)
            .map_err(|error| format!("Could not remove the signed artifacts: {error}"))?;
    }
    remove_android_release_record(&paths)?;
    remove_android_release_error(&paths)?;

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
