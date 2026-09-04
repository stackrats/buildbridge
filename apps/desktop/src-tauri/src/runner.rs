//! The control-plane runner: pairing, heartbeat, and remote job execution.

use super::*;
use ts_rs::TS;

#[tauri::command]
pub(crate) async fn get_runner_status(app: AppHandle) -> Result<DesktopStatus, String> {
    let config = load_config(&app)?;
    let paired = match config.as_ref() {
        Some(config) => read_token(config.runner_id.clone()).await.is_ok(),
        None => false,
    };

    Ok(DesktopStatus {
        paired,
        // Configuration without a token is the shape a cleared keyring leaves behind. Saying
        // "not paired" there would send someone looking for a pairing code they already used.
        credentials_missing: config.is_some() && !paired,
        server_url: config.as_ref().map(|value| value.server_url.clone()),
        runner_id: config.as_ref().map(|value| value.runner_id.clone()),
        runner_name: config.as_ref().map(|value| value.runner_name.clone()),
        platform: std::env::consts::OS.to_string(),
        architecture: std::env::consts::ARCH.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[tauri::command]
pub(crate) async fn pair_runner(app: AppHandle, input: PairInput) -> Result<DesktopStatus, String> {
    let request = PairRunnerRequest {
        code: input.code,
        name: input.runner_name.clone(),
        platform: std::env::consts::OS.to_string(),
        architecture: std::env::consts::ARCH.to_string(),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        capabilities: runner_capabilities(),
        protocol_version: PROTOCOL_VERSION,
    };
    let paired = ApiClient::pair(&input.server_url, &request)
        .await
        .map_err(|error| error.to_string())?;

    write_token(paired.runner.id.clone(), paired.token).await?;

    let config = StoredConfig {
        server_url: input.server_url.trim_end_matches('/').to_string(),
        runner_id: paired.runner.id,
        runner_name: paired.runner.name,
    };
    save_config(&app, &config)?;

    get_runner_status(app).await
}

#[tauri::command]
pub(crate) async fn unpair_runner(app: AppHandle) -> Result<(), String> {
    if let Some(config) = load_config(&app)? {
        delete_token(config.runner_id).await?;
    }

    let path = config_path(&app)?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub(crate) async fn get_realtime_configuration(
    app: AppHandle,
) -> Result<RealtimeConfiguration, String> {
    let (_, client) = paired_client(&app).await?;

    client
        .realtime_configuration()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn authorize_realtime(
    app: AppHandle,
    input: AuthorizeRealtimeInput,
) -> Result<serde_json::Value, String> {
    let (config, client) = paired_client(&app).await?;
    let expected_channel = format!("private-runners.{}", config.runner_id);

    if input.channel_name != expected_channel {
        return Err("The desktop client refused an unexpected realtime channel.".to_string());
    }

    client
        .authorize_realtime(&RealtimeAuthorizationRequest {
            socket_id: input.socket_id,
            channel_name: input.channel_name,
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn heartbeat_runner(app: AppHandle) -> Result<HeartbeatSummary, String> {
    let (_, client) = paired_client(&app).await?;

    let response = client
        .heartbeat(&heartbeat_request(&app).await)
        .await
        .map_err(|error| error.to_string())?;

    Ok(HeartbeatSummary {
        queued_builds: response.queued_builds,
    })
}

/// The part of a heartbeat reply the interface acts on.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HeartbeatSummary {
    /// Work waiting for this runner. Non-zero means a queue event was missed; claim now.
    #[ts(type = "number")]
    pub(crate) queued_builds: u64,
}

#[tauri::command]
pub(crate) async fn run_once(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<RunOnceResult, String> {
    if state.runner_running.swap(true, Ordering::AcqRel) {
        return Ok(RunOnceResult {
            state: RunState::Busy,
            build_id: None,
            message: "The runner is already checking for work.".to_string(),
        });
    }

    let result = run_once_inner(&app).await;
    state.runner_running.store(false, Ordering::Release);

    result
}

pub(crate) async fn run_once_inner(app: &AppHandle) -> Result<RunOnceResult, String> {
    let (_, client) = paired_client(app).await?;

    client
        .heartbeat(&heartbeat_request(app).await)
        .await
        .map_err(|error| error.to_string())?;

    let Some(build) = client.claim().await.map_err(|error| error.to_string())? else {
        return Ok(RunOnceResult {
            state: RunState::Idle,
            build_id: None,
            message: "Connected. No queued builds.".to_string(),
        });
    };

    // The runner crate runs what it can by itself; anything that needs a managed machine is
    // executed here, where the machines live.
    let Some(execution) = execute(&build) else {
        return execute_apple_archive_build(app, &client, &build).await;
    };
    client
        .append_logs(&build.id, execution.logs)
        .await
        .map_err(|error| error.to_string())?;
    let succeeded = execution.status == CompletionStatus::Succeeded;
    client
        .complete(
            &build.id,
            &CompleteBuildRequest {
                status: execution.status,
                exit_code: Some(execution.exit_code),
                error: execution.error,
                result: None,
            },
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(RunOnceResult {
        state: if succeeded {
            RunState::Completed
        } else {
            RunState::Failed
        },
        build_id: Some(build.id),
        message: "Build completed and reported to the control plane.".to_string(),
    })
}

/// How often buffered log lines are sent, and how many of those intervals pass between lease
/// renewals. Two minutes is the lease; renewing every minute leaves room for a slow request.
pub(crate) const LOG_PUMP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

pub(crate) const LEASE_RENEWAL_TICKS: u32 = 30;

/// Turns what a machine reports to the desktop's own interface into control-plane log lines.
/// Every progress event the archive pipeline emits is observed here, so the remote log is the
/// same story the Build tab tells, in the same order.
pub(crate) struct LogForwarder {
    pub(crate) pending: Vec<BuildLogLine>,
    pub(crate) next_sequence: u64,
    pub(crate) last_phase: HashMap<String, String>,
}

impl LogForwarder {
    pub(crate) fn new(next_sequence: u64) -> Self {
        Self {
            pending: Vec::new(),
            next_sequence,
            last_phase: HashMap::new(),
        }
    }

    pub(crate) fn push(&mut self, stream: LogStream, message: impl Into<String>) {
        self.pending.push(BuildLogLine {
            sequence: self.next_sequence,
            stream,
            message: message.into(),
        });
        self.next_sequence += 1;
    }

    pub(crate) fn observe(&mut self, event: &str, payload: &serde_json::Value, machine_id: &str) {
        if payload.get("machineId").and_then(serde_json::Value::as_str) != Some(machine_id) {
            return;
        }
        let Some(progress) = payload.get("progress") else {
            return;
        };
        let phase = progress
            .get("phase")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !phase.is_empty() && self.last_phase.get(event) != Some(&phase) {
            self.last_phase.insert(event.to_string(), phase.clone());
            let detail = progress
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            self.push(
                LogStream::System,
                format!("[{}] {detail}", phase.replace('_', " "))
                    .trim_end()
                    .to_string(),
            );
        }
        let line = progress
            .get("logLine")
            .or_else(|| progress.get("log_line"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !line.trim().is_empty() {
            self.push(LogStream::Stdout, line);
        }
    }

    pub(crate) fn take(&mut self) -> Vec<BuildLogLine> {
        std::mem::take(&mut self.pending)
    }
}

/// Sends buffered lines every couple of seconds and keeps the lease alive, on a plain thread so
/// it runs regardless of what the archive pipeline is blocking on. Returns the first failure.
pub(crate) fn spawn_log_pump(
    client: ApiClient,
    build_id: String,
    forwarder: Arc<Mutex<LogForwarder>>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<Option<String>> {
    std::thread::spawn(move || {
        let mut ticks: u32 = 0;
        let mut failure: Option<String> = None;
        loop {
            let stopping = stop.load(Ordering::Acquire);
            let lines = forwarder
                .lock()
                .map(|mut forwarder| forwarder.take())
                .unwrap_or_default();
            if !lines.is_empty()
                && let Err(error) =
                    tauri::async_runtime::block_on(client.append_logs(&build_id, lines))
            {
                failure.get_or_insert(format!("log forwarding failed: {error}"));
            }
            if stopping {
                break;
            }
            ticks += 1;
            if ticks.is_multiple_of(LEASE_RENEWAL_TICKS)
                && let Err(error) = tauri::async_runtime::block_on(client.renew_lease(&build_id))
            {
                failure.get_or_insert(format!("lease renewal failed: {error}"));
            }
            std::thread::sleep(LOG_PUMP_INTERVAL);
        }
        failure
    })
}

/// Runs a claimed `apple_archive` build on one of this host's machines and reports it.
pub(crate) async fn execute_apple_archive_build(
    app: &AppHandle,
    client: &ApiClient,
    build: &ClaimedBuild,
) -> Result<RunOnceResult, String> {
    let payload = AppleArchivePayload::from_value(&build.payload)?;
    let forwarder = Arc::new(Mutex::new(LogForwarder::new(build.next_log_sequence)));
    let stop = Arc::new(AtomicBool::new(false));
    let pump = spawn_log_pump(
        client.clone(),
        build.id.clone(),
        Arc::clone(&forwarder),
        Arc::clone(&stop),
    );

    // The pipeline emits the same events the Build tab listens to; forward them as the log.
    let listeners: Vec<tauri::EventId> = [PROJECT_PROGRESS_EVENT, ARCHIVE_PROGRESS_EVENT]
        .into_iter()
        .map(|event| {
            let forwarder = Arc::clone(&forwarder);
            let machine_id = payload.machine_id.clone();
            app.listen(event, move |emitted| {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(emitted.payload())
                    && let Ok(mut forwarder) = forwarder.lock()
                {
                    forwarder.observe(event, &value, &machine_id);
                }
            })
        })
        .collect();

    let outcome = run_remote_apple_archive(app, &payload, &forwarder).await;
    let env_set_name = match payload.env_set.clone() {
        Some(name) => Some(name),
        None => match attached_env_set_id(app, &payload.machine_id) {
            Ok(Some(id)) => read_env_sets()
                .await
                .ok()
                .and_then(|stored| stored.sets.into_iter().find(|set| set.id == id))
                .map(|set| set.name),
            _ => None,
        },
    };

    for id in listeners {
        app.unlisten(id);
    }
    if let Err(error) = &outcome
        && let Ok(mut forwarder) = forwarder.lock()
    {
        forwarder.push(LogStream::Stderr, error.clone());
    }
    stop.store(true, Ordering::Release);
    let pump_failure = pump.join().ok().flatten();

    let request = match &outcome {
        Ok(archive) => CompleteBuildRequest {
            status: CompletionStatus::Succeeded,
            exit_code: Some(0),
            error: pump_failure,
            result: Some(archive_result_json(archive, env_set_name.as_deref())),
        },
        Err(error) => CompleteBuildRequest {
            status: CompletionStatus::Failed,
            exit_code: Some(1),
            error: Some(error.clone()),
            result: None,
        },
    };
    client
        .complete(&build.id, &request)
        .await
        .map_err(|error| error.to_string())?;

    Ok(match outcome {
        Ok(archive) => RunOnceResult {
            state: RunState::Completed,
            build_id: Some(build.id.clone()),
            message: format!(
                "Signed archive {} ({}) built and reported to the control plane.",
                archive.marketing_version, archive.build_number
            ),
        },
        Err(error) => RunOnceResult {
            state: RunState::Failed,
            build_id: Some(build.id.clone()),
            message: format!("Signed archive failed: {error}"),
        },
    })
}

/// The remote pipeline is the Build tab's own steps in order — synchronize, test build, signed
/// archive — on the approved folder or on a fetched revision of the same project.
pub(crate) async fn run_remote_apple_archive(
    app: &AppHandle,
    payload: &AppleArchivePayload,
    forwarder: &Arc<Mutex<LogForwarder>>,
) -> Result<AppleArchiveResult, String> {
    let log = |stream: LogStream, message: String| {
        if let Ok(mut forwarder) = forwarder.lock() {
            forwarder.push(stream, message);
        }
    };
    let machine = machines::load_registry(app)?
        .find(&payload.machine_id)?
        .clone();
    let paths = MachinePaths::resolve(app, &payload.machine_id)?;
    let approved = load_apple_workspace(&paths)?.ok_or_else(|| {
        "No project is approved on this machine. Approve one in the desktop first.".to_string()
    })?;
    log(
        LogStream::System,
        format!(
            "BuildBridge {} · signed archive of {} on {}",
            env!("CARGO_PKG_VERSION"),
            approved.name,
            machine.config.name
        ),
    );

    let source = match payload.git_ref.as_deref() {
        Some(git_ref) => {
            let remote = project_remote_url(&approved.local_path).ok_or_else(|| {
                "The approved project has no git remote, so only its folder as-is can be built."
                    .to_string()
            })?;
            log(
                LogStream::System,
                format!("$ git fetch {} {git_ref}", redact_remote(&remote)),
            );
            let checkout = paths.checkout_dir();
            let fetch_ref = git_ref.to_string();
            let fetch_dir = checkout.clone();
            let commit = tauri::async_runtime::spawn_blocking(move || {
                checkout_project_ref(&remote, &fetch_ref, &fetch_dir)
            })
            .await
            .map_err(|error| error.to_string())??;
            log(
                LogStream::System,
                format!("Checked out {git_ref} at {commit}"),
            );

            let inspected = inspect_apple_workspace(&checkout.to_string_lossy())?;
            if inspected.bundle_identifier != approved.bundle_identifier
                || inspected.development_team != approved.development_team
            {
                return Err(
                    "That revision targets a different bundle identifier or team than the approved project, so the provisioned signing would not match it."
                        .to_string(),
                );
            }
            Some((
                checkout,
                WorkspaceSource {
                    kind: "git".to_string(),
                    git_ref: Some(git_ref.to_string()),
                    commit: Some(commit),
                },
            ))
        }
        None => None,
    };

    // A remote build names a set; the runner only ever builds with a set it already holds.
    let env_set_id = match payload.env_set.as_deref() {
        Some(name) => Some(
            read_env_sets()
                .await?
                .sets
                .into_iter()
                .find(|set| set.name == name)
                .map(|set| set.id)
                .ok_or_else(|| format!("No env set named {name} is stored on this host."))?,
        ),
        None => attached_env_set_id(app, &payload.machine_id)?,
    };
    log(
        LogStream::System,
        match &env_set_id {
            Some(_) => format!(
                "Env set: {}",
                payload
                    .env_set
                    .clone()
                    .unwrap_or_else(|| "attached to the machine".to_string())
            ),
            None => "Env set: none".to_string(),
        },
    );

    sync_apple_workspace_from(app, &payload.machine_id, source).await?;
    run_apple_smoke_build(app.clone(), payload.machine_id.clone(), None).await?;
    let archived =
        run_apple_signed_archive(app.clone(), payload.machine_id.clone(), env_set_id).await?;

    Ok(archived.archive)
}

/// What the control plane keeps about a finished archive: names, sizes and checksums. The files
/// themselves stay on this host.
pub(crate) fn archive_result_json(
    archive: &AppleArchiveResult,
    env_set: Option<&str>,
) -> serde_json::Value {
    let artifact = |artifact: &AppleArchiveArtifact| {
        serde_json::json!({
            "name": std::path::Path::new(&artifact.path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| artifact.path.clone()),
            "bytes": artifact.bytes,
            "sha256": artifact.sha256,
        })
    };

    serde_json::json!({
        "version": archive.marketing_version,
        "build_number": archive.build_number,
        "bundle_identifier": archive.bundle_identifier,
        "scheme": archive.scheme,
        "export_method": archive.export_method,
        "env_set": env_set,
        "artifacts": [artifact(&archive.ipa), artifact(&archive.archive)],
    })
}

pub(crate) fn runner_capabilities() -> Vec<String> {
    let mut capabilities = vec!["diagnostics".to_string()];

    match std::env::consts::OS {
        "macos" => capabilities.push("xcode".to_string()),
        "linux" | "windows" => capabilities.push("docker".to_string()),
        _ => {}
    }

    capabilities
}

pub(crate) async fn heartbeat_request(app: &AppHandle) -> HeartbeatRequest {
    HeartbeatRequest {
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: runner_capabilities(),
        machines: machine_reports(app).await,
    }
}

/// The machines as a control plane needs to see them: named, and either ready for a signed
/// archive or not. A failure to inspect them leaves the list empty rather than failing the
/// heartbeat, so a Docker hiccup never reads as a runner going offline.
pub(crate) async fn machine_reports(app: &AppHandle) -> Vec<MachineReport> {
    let Ok(view) = build_machine_list_view(app).await else {
        return Vec::new();
    };
    let env_set_names: Vec<String> = read_env_sets()
        .await
        .map(|stored| stored.sets.into_iter().map(|set| set.name).collect())
        .unwrap_or_default();

    view.machines
        .into_iter()
        .map(|summary| {
            let workspace = MachinePaths::resolve(app, &summary.id)
                .ok()
                .and_then(|paths| load_apple_workspace(&paths).ok().flatten());
            let ready = summary.state == ContainerState::Running
                && summary.trust_pinned
                && summary.signing_provisioned
                && workspace.is_some();

            MachineReport {
                id: summary.id,
                name: summary.config.name,
                ready,
                project: workspace.as_ref().map(|workspace| workspace.name.clone()),
                bundle_identifier: workspace
                    .as_ref()
                    .and_then(|workspace| workspace.bundle_identifier.clone()),
                repository: workspace
                    .as_ref()
                    .and_then(|workspace| project_remote_url(&workspace.local_path))
                    .map(|remote| redact_remote(&remote)),
                env_set: summary.env_set_name,
                env_sets: env_set_names.clone(),
            }
        })
        .collect()
}

/// The approved project's `origin` remote, if the folder is a git checkout. This is the only
/// repository a remote build may fetch from; the control plane never supplies one.
pub(crate) fn project_remote_url(local_path: &str) -> Option<String> {
    if !std::path::Path::new(local_path).join(".git").exists() {
        return None;
    }
    let output = Command::new("git")
        .args(["-C", local_path, "remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let remote = String::from_utf8_lossy(&output.stdout).trim().to_string();

    (!remote.is_empty()).then_some(remote)
}

/// A remote URL without any credentials that may be embedded in it.
pub(crate) fn redact_remote(remote: &str) -> String {
    match (remote.find("://"), remote.find('@')) {
        (Some(scheme_end), Some(at)) if at > scheme_end => {
            format!("{}{}", &remote[..scheme_end + 3], &remote[at + 1..])
        }
        _ => remote.to_string(),
    }
}

/// Checks out one revision of the approved project into the machine's checkout directory and
/// returns the commit it resolved to. Fixed argv, no shell; the ref was validated by both the
/// control plane and the contract crate, and is validated once more here.
pub(crate) fn checkout_project_ref(
    remote: &str,
    git_ref: &str,
    checkout: &std::path::Path,
) -> Result<String, String> {
    if !valid_git_ref(git_ref) {
        return Err("That ref is not a branch, tag, or commit as git names them.".to_string());
    }
    if !checkout.join(".git").is_dir() {
        if checkout.exists() {
            fs::remove_dir_all(checkout)
                .map_err(|error| format!("Could not reset the checkout directory: {error}"))?;
        }
        if let Some(parent) = checkout.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create the checkout directory: {error}"))?;
        }
        git(&[
            "clone",
            "--quiet",
            "--no-checkout",
            "--",
            remote,
            &checkout.to_string_lossy(),
        ])?;
    }
    let dir = checkout.to_string_lossy().to_string();
    git(&["-C", &dir, "remote", "set-url", "origin", remote])?;
    git(&["-C", &dir, "fetch", "--quiet", "--force", "origin", git_ref])?;
    git(&[
        "-C",
        &dir,
        "checkout",
        "--quiet",
        "--force",
        "--detach",
        "FETCH_HEAD",
    ])?;
    git(&["-C", &dir, "clean", "--quiet", "-fdx"])?;
    let commit = git(&["-C", &dir, "rev-parse", "HEAD"])?.trim().to_string();

    Ok(commit)
}

pub(crate) fn git(args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| format!("git is not available on this host: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "git {} failed: {}",
            args.iter()
                .find(|arg| !arg.starts_with('-') && **arg != "-C")
                .copied()
                .unwrap_or("command"),
            stderr.trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub(crate) async fn paired_client(app: &AppHandle) -> Result<(StoredConfig, ApiClient), String> {
    let config = load_config(app)?.ok_or_else(|| "Pair this runner first.".to_string())?;
    let token = read_token(config.runner_id.clone()).await?;
    let client = ApiClient::new(&config.server_url, token).map_err(|error| error.to_string())?;

    Ok((config, client))
}

pub(crate) fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("runner.json"))
        .map_err(|error| error.to_string())
}

pub(crate) fn load_config(app: &AppHandle) -> Result<Option<StoredConfig>, String> {
    let path = config_path(app)?;

    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("The runner configuration is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn save_config(app: &AppHandle, config: &StoredConfig) -> Result<(), String> {
    let path = config_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "The runner configuration directory is unavailable.".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    fs::write(
        path,
        serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}
