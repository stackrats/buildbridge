//! The control-plane runner: pairing, heartbeat, and remote job execution.

use super::*;
use ts_rs::TS;

/// Recovery polling runs in the native process, including while its window is hidden. The
/// realtime client can still wake the same runner immediately; run_once serializes claims.
pub async fn run_runner_service(app: Engine) {
    loop {
        if let Ok(summary) = heartbeat_runner(&app).await
            && summary.queued_builds > 0
        {
            let result = run_once(&app).await;
            match result {
                Ok(result) => {
                    let _ = app.emit("runner-activity", result);
                }
                Err(error) => {
                    let _ = app.emit(
                        "runner-activity",
                        RunOnceResult {
                            state: RunState::Failed,
                            build_id: None,
                            message: error,
                        },
                    );
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
    }
}

pub async fn get_runner_status(app: &Engine) -> Result<DesktopStatus, String> {
    let config = load_config(app)?;
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
pub async fn pair_runner(app: &Engine, input: PairInput) -> Result<DesktopStatus, String> {
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
    save_config(app, &config)?;

    get_runner_status(app).await
}
pub async fn unpair_runner(app: &Engine) -> Result<(), String> {
    if let Some(config) = load_config(app)? {
        delete_token(config.runner_id).await?;
    }

    let path = config_path(app)?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }

    Ok(())
}
pub async fn get_realtime_configuration(app: &Engine) -> Result<RealtimeConfiguration, String> {
    let (_, client) = paired_client(app).await?;

    client
        .realtime_configuration()
        .await
        .map_err(|error| error.to_string())
}
pub async fn authorize_realtime(
    app: &Engine,
    input: AuthorizeRealtimeInput,
) -> Result<serde_json::Value, String> {
    let (config, client) = paired_client(app).await?;
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
pub async fn heartbeat_runner(app: &Engine) -> Result<HeartbeatSummary, String> {
    let (_, client) = paired_client(app).await?;

    let response = client
        .heartbeat(&heartbeat_request(app).await)
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
pub struct HeartbeatSummary {
    /// Work waiting for this runner. Non-zero means a queue event was missed; claim now.
    #[ts(type = "number")]
    pub(crate) queued_builds: u64,
}
pub async fn run_once(app: &Engine) -> Result<RunOnceResult, String> {
    let state = app.state();
    if state.runner_running.swap(true, Ordering::AcqRel) {
        return Ok(RunOnceResult {
            state: RunState::Busy,
            build_id: None,
            message: "The runner is already checking for work.".to_string(),
        });
    }

    let result = run_once_inner(app).await;
    state.runner_running.store(false, Ordering::Release);

    result
}

pub(crate) async fn run_once_inner(app: &Engine) -> Result<RunOnceResult, String> {
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
    let client = client.for_build(&build);
    if let Err(error) = crate::sharing::authorize_shared_build(app, &build).await {
        client
            .complete(
                &build.id,
                &CompleteBuildRequest {
                    status: CompletionStatus::Failed,
                    exit_code: Some(1),
                    error: Some(error.clone()),
                    result: None,
                },
            )
            .await
            .map_err(|failure| failure.to_string())?;
        return Ok(RunOnceResult {
            state: RunState::Failed,
            build_id: Some(build.id),
            message: error,
        });
    }

    // The runner crate runs what it can by itself; anything that needs a managed machine is
    // executed here, where the machines live.
    let Some(execution) = execute(&build) else {
        return match build.kind {
            BuildKind::AppleArchive => execute_apple_archive_build(app, &client, &build).await,
            BuildKind::AndroidRelease => execute_android_release_build(app, &client, &build).await,
            BuildKind::Diagnostics => Err("the runner declined a diagnostics build".to_string()),
        };
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
/// renewals. The lease lasts two minutes; thirty-second permission checks leave room for a slow request.
pub(crate) const LOG_PUMP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

pub(crate) const LEASE_RENEWAL_TICKS: u32 = 15;

/// How many renewal ticks in a row may fail to reach the control plane before a build whose
/// lease expiry cannot be read is stopped. Three ticks are ninety seconds: inside one lease.
pub(crate) const MAX_UNREACHABLE_RENEWALS: u32 = 3;

/// Why a renewal tick did not extend the lease.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RenewalFailure {
    /// This host's sharing policy, or the control plane, said no. Final.
    Refused(String),
    /// The control plane could not be asked, or could not answer. Worth another tick.
    Unreachable(String),
}

impl RenewalFailure {
    /// A lease the control plane refuses — lapsed or replaced, its grant revoked, its build
    /// gone — is gone for good; anything else is the network or the server having a moment.
    pub(crate) fn from_lease_error(error: buildbridge_runner::RunnerError) -> Self {
        match &error {
            buildbridge_runner::RunnerError::Api { status, .. }
                if matches!(status.as_u16(), 401 | 403 | 404 | 409 | 410) =>
            {
                Self::Refused(error.to_string())
            }
            _ => Self::Unreachable(error.to_string()),
        }
    }
}

/// What the pump does after one renewal tick.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LeaseVerdict {
    /// The lease now runs until the returned time.
    Renewed(String),
    /// Not renewed this tick, but the lease already held still stands: ask again next tick.
    Retry,
    /// The build stops now, for the returned reason.
    Abort(String),
}

/// Decides whether a running build goes on after one renewal tick. A refusal stops it at once.
/// A transport or server failure is retried while the lease already held is still in the
/// future — the lease, not any one request, is what the control plane granted — and, when its
/// expiry cannot be read, for a bounded run of consecutive failures.
pub(crate) fn lease_verdict(
    outcome: Result<String, RenewalFailure>,
    lease_expires_at: &str,
    unreachable_ticks: u32,
    now_epoch_seconds: i64,
) -> LeaseVerdict {
    match outcome {
        Ok(expires_at) => LeaseVerdict::Renewed(expires_at),
        Err(RenewalFailure::Refused(error)) => LeaseVerdict::Abort(format!(
            "The build stopped because its lease or sharing permission is no longer valid: {error}"
        )),
        Err(RenewalFailure::Unreachable(error)) => {
            let lapsed = match lease_expiry_epoch_seconds(lease_expires_at) {
                Some(expiry) => now_epoch_seconds >= expiry,
                None => unreachable_ticks >= MAX_UNREACHABLE_RENEWALS,
            };
            if lapsed {
                LeaseVerdict::Abort(format!(
                    "The build stopped because its lease could not be renewed before it ran out: {error}"
                ))
            } else {
                LeaseVerdict::Retry
            }
        }
    }
}

fn lease_expiry_epoch_seconds(value: &str) -> Option<i64> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|expiry| expiry.unix_timestamp())
}

/// Turns what a machine reports to the desktop's own interface into control-plane log lines.
/// Every progress event the archive pipeline emits is observed here, so the remote log is the
/// same story the Build tab tells, in the same order.
pub struct LogForwarder {
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
        // The progress is flattened beside `machineId`; older payloads nested it.
        let progress = payload.get("progress").unwrap_or(payload);
        let phase = progress
            .get("phase")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !phase.is_empty() && self.last_phase.get(event) != Some(&phase) {
            self.last_phase.insert(event.to_string(), phase.clone());
            let detail = progress
                .get("detail")
                .or_else(|| progress.get("label"))
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
    handle: tokio::runtime::Handle,
    client: ApiClient,
    app: Engine,
    build: ClaimedBuild,
    forwarder: Arc<Mutex<LogForwarder>>,
    stop: Arc<AtomicBool>,
    aborted: Arc<AtomicBool>,
) -> std::thread::JoinHandle<Option<String>> {
    std::thread::spawn(move || {
        let mut ticks: u32 = 0;
        let mut failure: Option<String> = None;
        let mut lease_expires_at = build.lease_expires_at.clone();
        let mut unreachable_ticks: u32 = 0;
        loop {
            let stopping = stop.load(Ordering::Acquire);
            let lines = forwarder
                .lock()
                .map(|mut forwarder| forwarder.take())
                .unwrap_or_default();
            if !lines.is_empty()
                && let Err(error) = handle.block_on(client.append_logs(&build.id, lines))
            {
                failure.get_or_insert(format!("log forwarding failed: {error}"));
            }
            if stopping {
                break;
            }
            ticks += 1;
            if ticks.is_multiple_of(LEASE_RENEWAL_TICKS) {
                let outcome = match handle.block_on(authorize_shared_build(&app, &build)) {
                    Err(refusal) => Err(RenewalFailure::Refused(refusal)),
                    Ok(()) => handle
                        .block_on(client.renew_lease(&build.id))
                        .map(|lease| lease.lease_expires_at)
                        .map_err(RenewalFailure::from_lease_error),
                };
                if matches!(outcome, Err(RenewalFailure::Unreachable(_))) {
                    unreachable_ticks += 1;
                }
                let now = i64::try_from(machines::now_epoch_seconds()).unwrap_or(i64::MAX);
                match lease_verdict(outcome, &lease_expires_at, unreachable_ticks, now) {
                    LeaseVerdict::Renewed(expires_at) => {
                        lease_expires_at = expires_at;
                        unreachable_ticks = 0;
                    }
                    LeaseVerdict::Retry => {}
                    LeaseVerdict::Abort(reason) => {
                        aborted.store(true, Ordering::Release);
                        if let Some(id) = build
                            .payload
                            .get("machine_id")
                            .and_then(serde_json::Value::as_str)
                            && let Ok(scopes) = app.state().operation_scopes.lock()
                            && let Some(scope) = scopes.get(id)
                        {
                            scope.cancel();
                        }
                        failure.get_or_insert(reason);
                        break;
                    }
                }
            }
            std::thread::sleep(LOG_PUMP_INTERVAL);
        }
        failure
    })
}

fn ensure_remote_active(aborted: &AtomicBool) -> Result<(), String> {
    if aborted.load(Ordering::Acquire) {
        Err("The build was stopped because its lease or sharing permission ended.".to_string())
    } else {
        Ok(())
    }
}

async fn upload_outputs(
    client: &ApiClient,
    build: &ClaimedBuild,
    artifacts: &[AppleArchiveArtifact],
    aborted: &AtomicBool,
) -> Result<(), String> {
    for artifact in artifacts {
        ensure_remote_active(aborted)?;
        let path = std::path::Path::new(&artifact.path);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("An artifact has an invalid filename.")?;
        client
            .upload_artifact(
                &build.id,
                path,
                &buildbridge_contract::CreateArtifactRequest {
                    name: name.to_string(),
                    bytes: artifact.bytes,
                    sha256: artifact.sha256.clone(),
                },
                || aborted.load(Ordering::Acquire),
            )
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Streams a finished build's files with the machine claimed for the whole delivery, so a
/// clear or a rebuild started meanwhile waits instead of removing a file that is half sent.
async fn upload_outputs_from_machine(
    app: &Engine,
    client: &ApiClient,
    build: &ClaimedBuild,
    machine_id: &str,
    artifacts: &[AppleArchiveArtifact],
    aborted: &AtomicBool,
) -> Result<(), String> {
    let guard = begin_machine_operation(app, machine_id, "uploading_artifacts")?;
    let result = upload_outputs(client, build, artifacts, aborted).await;
    drop(guard);
    result
}

/// The same for this Mac: its operation lock keeps a local build or a cleanup off the retained
/// artifacts until they have been delivered.
async fn upload_outputs_from_native_mac(
    app: &Engine,
    client: &ApiClient,
    build: &ClaimedBuild,
    artifacts: &[AppleArchiveArtifact],
    aborted: &AtomicBool,
) -> Result<(), String> {
    let operation = begin_native_operation(app)?;
    let result = upload_outputs(client, build, artifacts, aborted).await;
    drop(operation);
    result
}

async fn execute_native_mac_archive(
    app: &Engine,
    client: &ApiClient,
    build: &ClaimedBuild,
    payload: AppleArchivePayload,
) -> Result<RunOnceResult, String> {
    let forwarder = Arc::new(Mutex::new(LogForwarder::new(build.next_log_sequence)));
    let stop = Arc::new(AtomicBool::new(false));
    let aborted = Arc::new(AtomicBool::new(false));
    let pump = spawn_log_pump(
        tokio::runtime::Handle::current(),
        client.clone(),
        app.clone(),
        build.clone(),
        Arc::clone(&forwarder),
        Arc::clone(&stop),
        Arc::clone(&aborted),
    );
    let listener_forwarder = Arc::clone(&forwarder);
    let listener = app.listen(NATIVE_MAC_PROGRESS_EVENT, move |value| {
        if let Ok(mut logs) = listener_forwarder.lock() {
            logs.observe(NATIVE_MAC_PROGRESS_EVENT, value, NATIVE_MAC_TARGET_ID);
        }
    });
    let mut outcome = if build.lease_token.is_none() {
        Err(
            "Native Mac remote builds require an updated control plane with artifact delivery."
                .to_string(),
        )
    } else {
        match payload.git_ref {
            Some(commit) => {
                run_authorized_native_mac_build(
                    app,
                    NativeMacBuildInput {
                        commit,
                        outcome: NativeMacBuildOutcome::Archive,
                        env_set: payload.env_set,
                    },
                    |config| authorize_native_snapshot(app, build, config),
                )
                .await
            }
            None => Err("Native Mac remote builds require a full Git commit hash.".to_string()),
        }
    };
    if let Ok(result) = &outcome
        && let Err(error) =
            upload_outputs_from_native_mac(app, client, build, &result.artifacts, &aborted).await
    {
        outcome = Err(error);
    }
    app.unlisten(listener);
    if let Err(error) = &outcome
        && let Ok(mut logs) = forwarder.lock()
    {
        logs.push(LogStream::Stderr, error.clone());
    }
    stop.store(true, Ordering::Release);
    let pump_failure = pump.join().ok().flatten();
    if aborted.load(Ordering::Acquire) {
        outcome = Err(pump_failure
            .clone()
            .unwrap_or_else(|| "The shared build was stopped.".to_string()));
    }
    let request = match &outcome {
        Ok(result) => CompleteBuildRequest {
            status: CompletionStatus::Succeeded,
            exit_code: Some(0),
            error: pump_failure,
            result: Some(serde_json::json!({
                "commit":result.commit,"repository":redact_remote(&result.repository),"xcode_version":result.xcode_version,
                "bundle_identifier":result.bundle_identifier,"env_set":result.env_set,
                "artifacts":result.artifacts.iter().map(|artifact| serde_json::json!({"name":std::path::Path::new(&artifact.path).file_name().and_then(|name| name.to_str()),"bytes":artifact.bytes,"sha256":artifact.sha256})).collect::<Vec<_>>()
            })),
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
    Ok(RunOnceResult {
        state: if outcome.is_ok() {
            RunState::Completed
        } else {
            RunState::Failed
        },
        build_id: Some(build.id.clone()),
        message: outcome
            .map(|_| {
                "The Mac build finished and its verified artifacts are ready to download."
                    .to_string()
            })
            .unwrap_or_else(|error| error),
    })
}

/// Runs a claimed `apple_archive` build on one of this host's machines and reports it.
pub(crate) async fn execute_apple_archive_build(
    app: &Engine,
    client: &ApiClient,
    build: &ClaimedBuild,
) -> Result<RunOnceResult, String> {
    let payload = AppleArchivePayload::from_value(&build.payload)?;
    if payload.machine_id == NATIVE_MAC_TARGET_ID {
        return execute_native_mac_archive(app, client, build, payload).await;
    }
    let forwarder = Arc::new(Mutex::new(LogForwarder::new(build.next_log_sequence)));
    let stop = Arc::new(AtomicBool::new(false));
    let aborted = Arc::new(AtomicBool::new(false));
    let pump = spawn_log_pump(
        tokio::runtime::Handle::current(),
        client.clone(),
        app.clone(),
        build.clone(),
        Arc::clone(&forwarder),
        Arc::clone(&stop),
        Arc::clone(&aborted),
    );

    // The pipeline emits the same events the Build tab listens to; forward them as the log.
    let listeners: Vec<ListenerId> = [PROJECT_PROGRESS_EVENT, ARCHIVE_PROGRESS_EVENT]
        .into_iter()
        .map(|event| {
            let forwarder = Arc::clone(&forwarder);
            let machine_id = payload.machine_id.clone();
            app.listen(event, move |value| {
                if let Ok(mut forwarder) = forwarder.lock() {
                    forwarder.observe(event, value, &machine_id);
                }
            })
        })
        .collect();

    let mut outcome = run_remote_apple_archive(app, &payload, &forwarder, &aborted).await;
    if let Ok(archive) = &outcome
        && build.lease_token.is_some()
        && let Err(error) = upload_outputs_from_machine(
            app,
            client,
            build,
            &payload.machine_id,
            &[archive.ipa.clone(), archive.archive.clone()],
            &aborted,
        )
        .await
    {
        outcome = Err(error);
    }
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
    if aborted.load(Ordering::Acquire) {
        outcome = Err(pump_failure
            .clone()
            .unwrap_or_else(|| "The remote build was stopped.".to_string()));
    }

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

/// Runs a claimed `android_release` build on one of this host's Android machines and reports
/// it, the same way an Apple archive is.
pub(crate) async fn execute_android_release_build(
    app: &Engine,
    client: &ApiClient,
    build: &ClaimedBuild,
) -> Result<RunOnceResult, String> {
    let payload = AndroidReleasePayload::from_value(&build.payload)?;
    let forwarder = Arc::new(Mutex::new(LogForwarder::new(build.next_log_sequence)));
    let stop = Arc::new(AtomicBool::new(false));
    let aborted = Arc::new(AtomicBool::new(false));
    let pump = spawn_log_pump(
        tokio::runtime::Handle::current(),
        client.clone(),
        app.clone(),
        build.clone(),
        Arc::clone(&forwarder),
        Arc::clone(&stop),
        Arc::clone(&aborted),
    );
    let listeners: Vec<ListenerId> = [ANDROID_BUILD_PROGRESS_EVENT, ANDROID_RELEASE_PROGRESS_EVENT]
        .into_iter()
        .map(|event| {
            let forwarder = Arc::clone(&forwarder);
            let machine_id = payload.machine_id.clone();
            app.listen(event, move |value| {
                if let Ok(mut forwarder) = forwarder.lock() {
                    forwarder.observe(event, value, &machine_id);
                }
            })
        })
        .collect();

    let mut outcome = run_remote_android_release(app, &payload, &forwarder, &aborted).await;
    if let Ok(release) = &outcome
        && build.lease_token.is_some()
    {
        let artifacts = release
            .aab
            .iter()
            .chain(release.apk.iter())
            .map(|artifact| AppleArchiveArtifact {
                path: artifact.path.clone(),
                bytes: artifact.bytes,
                sha256: artifact.sha256.clone(),
            })
            .collect::<Vec<_>>();
        if let Err(error) = upload_outputs_from_machine(
            app,
            client,
            build,
            &payload.machine_id,
            &artifacts,
            &aborted,
        )
        .await
        {
            outcome = Err(error);
        }
    }
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
    if aborted.load(Ordering::Acquire) {
        outcome = Err(pump_failure
            .clone()
            .unwrap_or_else(|| "The remote build was stopped.".to_string()));
    }

    let request = match &outcome {
        Ok(release) => CompleteBuildRequest {
            status: CompletionStatus::Succeeded,
            exit_code: Some(0),
            error: pump_failure,
            result: Some(android_result_json(release, env_set_name.as_deref())),
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
        Ok(release) => RunOnceResult {
            state: RunState::Completed,
            build_id: Some(build.id.clone()),
            message: format!(
                "Signed Android release {} ({}) built and reported to the control plane.",
                release.version_name, release.version_code
            ),
        },
        Err(error) => RunOnceResult {
            state: RunState::Failed,
            build_id: Some(build.id.clone()),
            message: format!("Signed Android release failed: {error}"),
        },
    })
}

/// The Android pipeline: synchronize, debug build, signed release — on the approved folder or
/// on a fetched revision of the same project.
pub(crate) async fn run_remote_android_release(
    app: &Engine,
    payload: &AndroidReleasePayload,
    forwarder: &Arc<Mutex<LogForwarder>>,
    aborted: &AtomicBool,
) -> Result<AndroidReleaseResult, String> {
    let log = |stream: LogStream, message: String| {
        if let Ok(mut forwarder) = forwarder.lock() {
            forwarder.push(stream, message);
        }
    };
    let machine = machines::load_registry(app)?
        .find(&payload.machine_id)?
        .clone();
    if machine.config.provider.is_macos() {
        return Err(
            "That machine is a macOS machine; queue an Apple archive on it instead.".to_string(),
        );
    }
    let paths = MachinePaths::resolve(app, &payload.machine_id)?;
    let approved = load_android_workspace(&paths)?.ok_or_else(|| {
        "No project is approved on this machine. Approve one in the desktop first.".to_string()
    })?;
    log(
        LogStream::System,
        format!(
            "BuildBridge {} · signed Android release of {} on {}",
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
            let commit = tokio::task::spawn_blocking(move || {
                checkout_project_ref(&remote, &fetch_ref, &fetch_dir)
            })
            .await
            .map_err(|error| error.to_string())??;
            log(
                LogStream::System,
                format!("Checked out {git_ref} at {commit}"),
            );
            let inspected = inspect_android_workspace(&checkout.to_string_lossy())?;
            if inspected.application_id != approved.application_id {
                return Err(
                    "That revision declares a different application identifier than the approved project."
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

    let env_set_id = match payload.env_set.as_deref() {
        Some(name) => Some(
            read_env_sets()
                .await?
                .sets
                .into_iter()
                .find(|set| set.name == name)
                .map(|set| set.id)
                .ok_or_else(|| format!("No environment named {name} is stored on this host."))?,
        ),
        None => attached_env_set_id(app, &payload.machine_id)?,
    };
    log(
        LogStream::System,
        match &env_set_id {
            Some(_) => format!(
                "Environment: {}",
                payload
                    .env_set
                    .clone()
                    .unwrap_or_else(|| "attached to the machine".to_string())
            ),
            None => "Environment: none".to_string(),
        },
    );

    ensure_remote_active(aborted)?;
    sync_android_workspace_with_env(app, &payload.machine_id, source, Some(env_set_id.clone()))
        .await?;
    ensure_remote_active(aborted)?;
    run_android_debug_build(app, payload.machine_id.clone(), false).await?;
    ensure_remote_active(aborted)?;
    let released =
        run_android_signed_release(app, payload.machine_id.clone(), env_set_id, None, None).await?;

    Ok(released.release)
}

/// What the control plane keeps about a finished Android release: names, sizes and checksums.
pub(crate) fn android_result_json(
    release: &AndroidReleaseResult,
    env_set: Option<&str>,
) -> serde_json::Value {
    let artifact = |artifact: &AndroidArtifact| {
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
        "version": release.version_name,
        "build_number": release.version_code,
        "application_id": release.application_id,
        "key_alias": release.key_alias,
        "certificate_sha256": release.certificate_sha256,
        "env_set": env_set,
        "artifacts": release.aab.iter().chain(release.apk.iter()).map(artifact).collect::<Vec<_>>(),
    })
}

/// The remote pipeline is the Build tab's own steps in order — synchronize, test build, signed
/// archive — on the approved folder or on a fetched revision of the same project.
pub(crate) async fn run_remote_apple_archive(
    app: &Engine,
    payload: &AppleArchivePayload,
    forwarder: &Arc<Mutex<LogForwarder>>,
    aborted: &AtomicBool,
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
            let commit = tokio::task::spawn_blocking(move || {
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
                .ok_or_else(|| format!("No environment named {name} is stored on this host."))?,
        ),
        None => attached_env_set_id(app, &payload.machine_id)?,
    };
    log(
        LogStream::System,
        match &env_set_id {
            Some(_) => format!(
                "Environment: {}",
                payload
                    .env_set
                    .clone()
                    .unwrap_or_else(|| "attached to the machine".to_string())
            ),
            None => "Environment: none".to_string(),
        },
    );

    ensure_remote_active(aborted)?;
    sync_apple_workspace_with_env(app, &payload.machine_id, source, Some(env_set_id.clone()))
        .await?;
    ensure_remote_active(aborted)?;
    run_apple_smoke_build(app, payload.machine_id.clone(), None).await?;
    ensure_remote_active(aborted)?;
    let archived =
        run_apple_signed_archive(app, payload.machine_id.clone(), env_set_id, None).await?;

    Ok(archived.archive)
}

/// Safe completion metadata: names, sizes and checksums. Capable servers receive the files
/// separately through the lease-scoped artifact upload; host paths stay local.
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
    let mut capabilities = vec![
        "diagnostics".to_string(),
        "sharing_v1".to_string(),
        "artifacts_v1".to_string(),
    ];

    match std::env::consts::OS {
        "macos" => {
            capabilities.push("docker".to_string());
            capabilities.push("android".to_string());
        }
        "linux" | "windows" => {
            capabilities.push("docker".to_string());
            // The Android toolchain needs nothing of the host but Docker.
            capabilities.push("android".to_string());
        }
        _ => {}
    }

    capabilities
}

pub(crate) async fn heartbeat_request(app: &Engine) -> HeartbeatRequest {
    let machines = machine_reports(app).await;
    let mut capabilities = runner_capabilities();
    if machines.iter().any(|target| {
        target.executor.as_deref() == Some("native_macos") && target.toolchain_version.is_some()
    }) {
        capabilities.push("xcode".to_string());
        capabilities.push("native_macos_v1".to_string());
    }
    HeartbeatRequest {
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities,
        machines,
        sharing_paused: sharing_paused(app),
    }
}

/// The machines as a control plane needs to see them: named, and either ready for a signed
/// archive or not. A failure to inspect them leaves the list empty rather than failing the
/// heartbeat, so a Docker hiccup never reads as a runner going offline.
pub(crate) async fn machine_reports(app: &Engine) -> Vec<MachineReport> {
    let summaries = build_machine_list_view(app)
        .await
        .map(|view| view.machines)
        .unwrap_or_default();
    let env_set_names: Vec<String> = read_env_sets()
        .await
        .map(|stored| stored.sets.into_iter().map(|set| set.name).collect())
        .unwrap_or_default();

    let mut reports: Vec<MachineReport> = summaries
        .into_iter()
        .map(|summary| {
            let paths = MachinePaths::resolve(app, &summary.id).ok();
            // The project's name, identifier and folder, whichever platform it is for.
            let project = match summary.platform {
                MachinePlatform::Ios => paths
                    .as_ref()
                    .and_then(|paths| load_apple_workspace(paths).ok().flatten())
                    .map(|workspace| {
                        (
                            workspace.name,
                            workspace.bundle_identifier,
                            workspace.local_path,
                        )
                    }),
                MachinePlatform::Android => paths
                    .as_ref()
                    .and_then(|paths| load_android_workspace(paths).ok().flatten())
                    .map(|workspace| {
                        (
                            workspace.name,
                            workspace.application_id,
                            workspace.local_path,
                        )
                    }),
            };
            let ready = summary.state == ContainerState::Running
                && (summary.platform == MachinePlatform::Android || summary.trust_pinned)
                && summary.signing_provisioned
                && project.is_some();

            MachineReport {
                executor: None,
                toolchain_version: None,
                readiness_issues: Vec::new(),
                architecture: None,
                id: summary.id,
                name: summary.config.name,
                ready,
                project: project.as_ref().map(|(name, _, _)| name.clone()),
                bundle_identifier: project
                    .as_ref()
                    .and_then(|(_, identifier, _)| identifier.clone()),
                repository: project
                    .as_ref()
                    .and_then(|(_, _, local_path)| project_remote_url(local_path))
                    .map(|remote| redact_remote(&remote)),
                env_set: summary.env_set_name,
                env_sets: env_set_names.clone(),
                platform: Some(
                    match summary.platform {
                        MachinePlatform::Ios => "ios",
                        MachinePlatform::Android => "android",
                    }
                    .to_string(),
                ),
            }
        })
        .collect();
    if cfg!(target_os = "macos")
        && let Ok(status) = native_mac_status(app).await
    {
        let project = status.config.project;
        reports.push(MachineReport {
            id: NATIVE_MAC_TARGET_ID.to_string(),
            name: "This Mac".to_string(),
            ready: status.archive_ready && !status.busy,
            project: project.as_ref().map(|project| project.name.clone()),
            bundle_identifier: project
                .as_ref()
                .map(|project| project.bundle_identifier.clone()),
            repository: project
                .as_ref()
                .map(|project| redact_remote(&project.repository)),
            env_set: None,
            env_sets: env_set_names,
            platform: Some("ios".to_string()),
            executor: Some("native_macos".to_string()),
            toolchain_version: status.toolchain.xcode_version,
            architecture: Some(status.toolchain.architecture),
            readiness_issues: status.issues,
        });
    }
    reports
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
        return Err(git_failure_message(
            args,
            &String::from_utf8_lossy(&output.stderr),
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// The failure of one git invocation as a remote build's log may carry it: the subcommand by
/// name, and git's own words with the local paths and remotes this host passed it taken out.
/// The checkout lives under this host's data directory and the remote may carry a token;
/// neither belongs on the control plane.
pub(crate) fn git_failure_message(args: &[&str], stderr: &str) -> String {
    let mut rest = args.iter().copied();
    let mut subcommand = "command";
    while let Some(arg) = rest.next() {
        match arg {
            // Global options that take a value of their own.
            "-C" | "-c" => {
                rest.next();
            }
            _ if arg.starts_with('-') => {}
            _ => {
                subcommand = arg;
                break;
            }
        }
    }
    let mut message = stderr.trim().to_string();
    for arg in args {
        if *arg == subcommand || arg.is_empty() {
            continue;
        }
        if std::path::Path::new(arg).is_absolute() {
            message = message.replace(arg, "<local path>");
        } else if arg.contains("://") || (arg.contains('@') && arg.contains(':')) {
            message = message.replace(arg, &redact_remote(arg));
        }
    }
    // git may print a remote in a form of its own; whatever still looks like one is redacted.
    let message = message
        .split_inclusive(char::is_whitespace)
        .map(|token| {
            if token.contains("://") {
                redact_remote(token)
            } else {
                token.to_string()
            }
        })
        .collect::<String>();

    format!("git {subcommand} failed: {message}")
}

pub(crate) async fn paired_client(app: &Engine) -> Result<(StoredConfig, ApiClient), String> {
    let config = load_config(app)?.ok_or_else(|| "Pair this runner first.".to_string())?;
    let token = read_token(config.runner_id.clone()).await?;
    let client = ApiClient::new(&config.server_url, token).map_err(|error| error.to_string())?;

    Ok((config, client))
}

pub(crate) fn config_path(app: &Engine) -> Result<PathBuf, String> {
    Ok(app.config_dir().join("runner.json"))
}

pub(crate) fn load_config(app: &Engine) -> Result<Option<StoredConfig>, String> {
    let path = config_path(app)?;

    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("The runner configuration is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn save_config(app: &Engine, config: &StoredConfig) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_credentials_are_redacted_before_they_reach_the_control_plane() {
        assert_eq!(
            redact_remote("https://user:ghp_secret@github.com/team/app.git"),
            "https://github.com/team/app.git"
        );
        assert_eq!(
            redact_remote("https://x-access-token:ghp_secret@github.com/a/b"),
            "https://github.com/a/b"
        );
        assert_eq!(
            redact_remote("ssh://git@example.com/team/app.git"),
            "ssh://example.com/team/app.git"
        );
        assert_eq!(
            redact_remote("https://github.com/team/app.git"),
            "https://github.com/team/app.git"
        );
        assert_eq!(
            redact_remote("git@github.com:team/app.git"),
            "git@github.com:team/app.git"
        );
        assert_eq!(redact_remote(""), "");
    }

    #[test]
    fn git_failures_name_the_subcommand_and_carry_no_local_path_or_remote_secret() {
        let checkout = "/home/me/.local/share/buildbridge/machines/m/checkout";
        let message = git_failure_message(
            &[
                "-C",
                checkout,
                "fetch",
                "--quiet",
                "--force",
                "origin",
                "release/1.4",
            ],
            &format!("fatal: cannot change to '{checkout}': No such file or directory"),
        );
        assert!(message.starts_with("git fetch failed: "), "{message}");
        assert!(!message.contains("/home/me"), "{message}");
        assert!(message.contains("<local path>"), "{message}");
        assert!(message.contains("release/1.4") || !message.contains("release"));

        let remote = "https://x-access-token:ghp_secret@github.com/team/app.git";
        let message = git_failure_message(
            &[
                "clone",
                "--quiet",
                "--no-checkout",
                "--",
                remote,
                "/tmp/bb/checkout",
            ],
            &format!(
                "fatal: unable to access '{remote}/': Could not resolve host: github.com\nfatal: destination path '/tmp/bb/checkout' already exists"
            ),
        );
        assert!(message.starts_with("git clone failed: "), "{message}");
        assert!(!message.contains("ghp_secret"), "{message}");
        assert!(!message.contains("x-access-token"), "{message}");
        assert!(!message.contains("/tmp/bb"), "{message}");
        assert!(message.contains("github.com/team/app.git"), "{message}");

        // A remote git prints in its own words, unlike any argument, is still redacted.
        let message = git_failure_message(
            &["-C", "/x", "fetch", "origin", "main"],
            "fatal: could not read from 'https://user:pw@example.test/app.git/': timed out",
        );
        assert!(!message.contains("user:pw"), "{message}");
        assert!(
            message.contains("https://example.test/app.git/"),
            "{message}"
        );

        assert_eq!(
            git_failure_message(&["-C", "/x", "rev-parse", "HEAD"], "  "),
            "git rev-parse failed: "
        );
        assert_eq!(git_failure_message(&[], "boom"), "git command failed: boom");
        assert_eq!(
            git_failure_message(&["-c", "core.x=1", "-C", "/x", "clean", "-fdx"], "boom"),
            "git clean failed: boom"
        );
    }

    #[test]
    fn a_lost_renewal_keeps_the_build_while_the_lease_it_holds_still_stands() {
        let future = "2999-01-01T00:00:00Z";
        let now = 1_000_000_000;
        let unreachable = || Err(RenewalFailure::Unreachable("request timed out".into()));
        assert_eq!(
            lease_verdict(Ok("2999-06-01T00:00:00Z".into()), future, 0, now),
            LeaseVerdict::Renewed("2999-06-01T00:00:00Z".into())
        );
        for ticks in [1, 3, 100] {
            assert_eq!(
                lease_verdict(unreachable(), future, ticks, now),
                LeaseVerdict::Retry,
                "the lease, not a failure count, governs while the expiry is readable"
            );
        }
        // Past the lease, the build has no claim on the machine any more.
        let expiry = "2001-09-09T01:46:40Z";
        assert!(matches!(
            lease_verdict(unreachable(), expiry, 1, now),
            LeaseVerdict::Abort(reason) if reason.contains("ran out") && reason.contains("timed out")
        ));
        assert_eq!(
            lease_verdict(unreachable(), expiry, 1, now - 1),
            LeaseVerdict::Retry
        );
        // An unreadable expiry falls back to a bounded run of failures.
        for (ticks, retried) in [(1, true), (2, true), (3, false), (4, false)] {
            assert_eq!(
                lease_verdict(unreachable(), "soon", ticks, now) == LeaseVerdict::Retry,
                retried,
                "{ticks} unreachable ticks"
            );
        }
        // A refusal is final at once, however fresh the lease.
        assert!(matches!(
            lease_verdict(Err(RenewalFailure::Refused("access revoked".into())), future, 0, now),
            LeaseVerdict::Abort(reason)
                if reason.contains("access revoked") && reason.contains("no longer valid")
        ));
    }

    #[test]
    fn only_a_definite_no_from_the_control_plane_is_a_refusal() {
        use buildbridge_runner::RunnerError;
        use reqwest::StatusCode;
        let api = |status: u16| RunnerError::Api {
            status: StatusCode::from_u16(status).unwrap(),
            message: "no".into(),
        };
        for status in [401, 403, 404, 409, 410] {
            assert!(
                matches!(
                    RenewalFailure::from_lease_error(api(status)),
                    RenewalFailure::Refused(_)
                ),
                "{status}"
            );
        }
        for status in [408, 429, 500, 502, 503, 504] {
            assert!(
                matches!(
                    RenewalFailure::from_lease_error(api(status)),
                    RenewalFailure::Unreachable(_)
                ),
                "{status}"
            );
        }
        assert!(matches!(
            RenewalFailure::from_lease_error(RunnerError::InvalidUrl("x".into())),
            RenewalFailure::Unreachable(_)
        ));
        assert_eq!(
            lease_expiry_epoch_seconds("2001-09-09T01:46:40Z"),
            Some(1_000_000_000)
        );
        assert_eq!(lease_expiry_epoch_seconds("soon"), None);
    }

    #[test]
    fn progress_events_become_one_phase_line_per_change_and_only_this_machines_log_lines() {
        let mut forwarder = LogForwarder::new(7);
        let event = "apple-archive-progress";
        forwarder.observe(
            event,
            &serde_json::json!({"machineId": "other", "phase": "archiving", "logLine": "leak"}),
            "default",
        );
        forwarder.observe(
            event,
            &serde_json::json!({"phase": "archiving", "logLine": "leak"}),
            "default",
        );
        assert!(forwarder.pending.is_empty());
        forwarder.observe(
            event,
            &serde_json::json!({
                "machineId": "default", "phase": "building_web_assets",
                "detail": "Rebuilding web assets", "logLine": "  "
            }),
            "default",
        );
        forwarder.observe(
            event,
            &serde_json::json!({
                "machineId": "default", "phase": "building_web_assets", "logLine": "vite v6 building"
            }),
            "default",
        );
        forwarder.observe(
            event,
            &serde_json::json!({
                "machineId": "default",
                "progress": {"phase": "archiving", "label": "Archiving", "log_line": "xcodebuild archive"}
            }),
            "default",
        );
        forwarder.observe(
            event,
            &serde_json::json!({"machineId": "default", "phase": "completed"}),
            "default",
        );
        forwarder.observe(
            "device-progress",
            &serde_json::json!({"machineId": "default", "phase": "completed", "detail": "Done"}),
            "default",
        );
        let lines = forwarder.take();
        assert!(forwarder.pending.is_empty());
        assert_eq!(
            lines.iter().map(|line| line.sequence).collect::<Vec<_>>(),
            [7, 8, 9, 10, 11, 12]
        );
        assert_eq!(
            lines
                .iter()
                .map(|line| (line.stream, line.message.as_str()))
                .collect::<Vec<_>>(),
            [
                (
                    LogStream::System,
                    "[building web assets] Rebuilding web assets"
                ),
                (LogStream::Stdout, "vite v6 building"),
                (LogStream::System, "[archiving] Archiving"),
                (LogStream::Stdout, "xcodebuild archive"),
                (LogStream::System, "[completed]"),
                (LogStream::System, "[completed] Done"),
            ]
        );
        assert_eq!(forwarder.next_sequence, 13);
        assert!(!serde_json::to_string(&lines).unwrap().contains("leak"));
    }
}
