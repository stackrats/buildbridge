//! Transport and typed job execution for a BuildBridge runner.
//!
//! This crate has no Tauri dependency. Desktop, headless, and future macOS
//! guest agents can all use the same API and executor.

use std::net::IpAddr;
use std::process::Command;
use std::time::Duration;

use buildbridge_contract::{
    AppendLogsRequest, BuildKind, BuildLogLine, ClaimBuildResponse, ClaimedBuild,
    CompleteBuildRequest, CompletionStatus, HeartbeatRequest, LogStream, PROTOCOL_VERSION,
    PairRunnerRequest, PairRunnerResponse, RealtimeAuthorizationRequest, RealtimeConfiguration,
    RealtimeConfigurationResponse,
};
use reqwest::{Client, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("invalid control-plane URL: {0}")]
    InvalidUrl(String),
    #[error("control-plane request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("control plane returned {status}: {message}")]
    Api { status: StatusCode, message: String },
    #[error("runner protocol {actual} is incompatible with protocol {expected}")]
    ProtocolMismatch { expected: u32, actual: u32 },
}

#[derive(Debug, Clone)]
pub struct ApiClient {
    base_url: Url,
    token: String,
    http: Client,
}

impl ApiClient {
    pub fn new(server_url: &str, token: impl Into<String>) -> Result<Self, RunnerError> {
        Ok(Self {
            base_url: normalize_base_url(server_url)?,
            token: token.into(),
            http: http_client()?,
        })
    }

    pub async fn pair(
        server_url: &str,
        request: &PairRunnerRequest,
    ) -> Result<PairRunnerResponse, RunnerError> {
        let base_url = normalize_base_url(server_url)?;
        let response = http_client()?
            .post(endpoint(&base_url, "api/runner/pair")?)
            .json(request)
            .send()
            .await?;
        let paired: PairRunnerResponse = decode(response).await?;
        ensure_protocol(paired.protocol_version)?;

        Ok(paired)
    }

    pub async fn heartbeat(&self, request: &HeartbeatRequest) -> Result<(), RunnerError> {
        self.post_empty("api/runner/heartbeat", request).await
    }

    pub async fn realtime_configuration(&self) -> Result<RealtimeConfiguration, RunnerError> {
        let response = self
            .http
            .get(endpoint(&self.base_url, "api/runner/realtime")?)
            .bearer_auth(&self.token)
            .send()
            .await?;
        let configuration: RealtimeConfigurationResponse = decode(response).await?;
        ensure_protocol(configuration.protocol_version)?;

        Ok(configuration.realtime)
    }

    pub async fn authorize_realtime(
        &self,
        request: &RealtimeAuthorizationRequest,
    ) -> Result<serde_json::Value, RunnerError> {
        let response = self
            .http
            .post(endpoint(&self.base_url, "api/broadcasting/auth")?)
            .bearer_auth(&self.token)
            .json(request)
            .send()
            .await?;

        decode(response).await
    }

    pub async fn claim(&self) -> Result<Option<ClaimedBuild>, RunnerError> {
        let response = self
            .http
            .post(endpoint(&self.base_url, "api/runner/builds/claim")?)
            .bearer_auth(&self.token)
            .send()
            .await?;

        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }

        let claim: ClaimBuildResponse = decode(response).await?;
        ensure_protocol(claim.protocol_version)?;

        Ok(Some(claim.build))
    }

    pub async fn append_logs(
        &self,
        build_id: &str,
        lines: Vec<BuildLogLine>,
    ) -> Result<(), RunnerError> {
        for chunk in lines.chunks(100) {
            self.post_empty(
                &format!("api/runner/builds/{build_id}/logs"),
                &AppendLogsRequest {
                    lines: chunk.to_vec(),
                },
            )
            .await?;
        }

        Ok(())
    }

    pub async fn complete(
        &self,
        build_id: &str,
        request: &CompleteBuildRequest,
    ) -> Result<(), RunnerError> {
        self.post_empty(&format!("api/runner/builds/{build_id}/complete"), request)
            .await
    }

    async fn post_empty<T: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<(), RunnerError> {
        let response = self
            .http
            .post(endpoint(&self.base_url, path)?)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await?;

        ensure_success(response).await
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub status: CompletionStatus,
    pub exit_code: i32,
    pub error: Option<String>,
    pub logs: Vec<BuildLogLine>,
}

pub fn execute(build: &ClaimedBuild) -> ExecutionResult {
    match build.kind {
        BuildKind::Diagnostics => execute_diagnostics(build.next_log_sequence),
    }
}

fn execute_diagnostics(mut sequence: u64) -> ExecutionResult {
    let platform = std::env::consts::OS;
    let architecture = std::env::consts::ARCH;
    let mut logs = vec![BuildLogLine {
        sequence,
        stream: LogStream::System,
        message: format!(
            "BuildBridge runner {} on {platform}/{architecture}",
            env!("CARGO_PKG_VERSION")
        ),
    }];
    sequence += 1;

    let mut failed_checks = Vec::new();
    let mut exit_code = 0;

    for spec in diagnostic_specs(platform) {
        logs.push(BuildLogLine {
            sequence,
            stream: LogStream::System,
            message: format!("$ {} {}", spec.program, spec.args.join(" "))
                .trim_end()
                .to_string(),
        });
        sequence += 1;

        match Command::new(spec.program).args(spec.args).output() {
            Ok(output) => {
                push_output_lines(&mut logs, &mut sequence, LogStream::Stdout, &output.stdout);
                push_output_lines(&mut logs, &mut sequence, LogStream::Stderr, &output.stderr);

                if spec.required && !output.status.success() {
                    failed_checks.push(spec.label.to_string());
                    exit_code = output.status.code().unwrap_or(1);
                }
            }
            Err(error) => {
                logs.push(BuildLogLine {
                    sequence,
                    stream: LogStream::Stderr,
                    message: error.to_string(),
                });
                sequence += 1;

                if spec.required {
                    failed_checks.push(spec.label.to_string());
                    exit_code = 127;
                }
            }
        }
    }

    if failed_checks.is_empty() {
        logs.push(BuildLogLine {
            sequence,
            stream: LogStream::System,
            message: "Required build tools are available.".to_string(),
        });

        ExecutionResult {
            status: CompletionStatus::Succeeded,
            exit_code: 0,
            error: None,
            logs,
        }
    } else {
        let message = format!("Required checks failed: {}", failed_checks.join(", "));
        logs.push(BuildLogLine {
            sequence,
            stream: LogStream::Stderr,
            message: message.clone(),
        });

        ExecutionResult {
            status: CompletionStatus::Failed,
            exit_code,
            error: Some(message),
            logs,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CommandSpec {
    label: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    required: bool,
}

fn diagnostic_specs(platform: &str) -> Vec<CommandSpec> {
    match platform {
        "macos" => vec![
            CommandSpec {
                label: "Xcode",
                program: "xcodebuild",
                args: &["-version"],
                required: true,
            },
            CommandSpec {
                label: "Xcode command-line tools",
                program: "xcrun",
                args: &["--find", "xcodebuild"],
                required: true,
            },
            CommandSpec {
                label: "code-signing identities",
                program: "security",
                args: &["find-identity", "-v", "-p", "codesigning"],
                required: false,
            },
        ],
        "windows" => vec![
            CommandSpec {
                label: "Windows",
                program: "cmd",
                args: &["/C", "ver"],
                required: false,
            },
            docker_version_spec(),
            docker_compose_spec(),
        ],
        _ => vec![
            CommandSpec {
                label: "host system",
                program: "uname",
                args: &["-a"],
                required: false,
            },
            docker_version_spec(),
            docker_compose_spec(),
        ],
    }
}

fn docker_version_spec() -> CommandSpec {
    CommandSpec {
        label: "Docker",
        program: "docker",
        args: &["--version"],
        required: true,
    }
}

fn docker_compose_spec() -> CommandSpec {
    CommandSpec {
        label: "Docker Compose",
        program: "docker",
        args: &["compose", "version"],
        required: true,
    }
}

fn push_output_lines(
    logs: &mut Vec<BuildLogLine>,
    sequence: &mut u64,
    stream: LogStream,
    output: &[u8],
) {
    for line in String::from_utf8_lossy(output).lines() {
        logs.push(BuildLogLine {
            sequence: *sequence,
            stream,
            message: line.to_string(),
        });
        *sequence += 1;
    }
}

fn http_client() -> Result<Client, RunnerError> {
    Ok(Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(format!("buildbridge-runner/{}", env!("CARGO_PKG_VERSION")))
        .build()?)
}

fn normalize_base_url(server_url: &str) -> Result<Url, RunnerError> {
    let mut url =
        Url::parse(server_url).map_err(|error| RunnerError::InvalidUrl(error.to_string()))?;

    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(RunnerError::InvalidUrl(
            "only absolute http:// and https:// URLs are supported".to_string(),
        ));
    }

    if url.scheme() == "http" && !is_local_host(&url) {
        return Err(RunnerError::InvalidUrl(
            "https:// is required for non-local control planes".to_string(),
        ));
    }

    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }

    Ok(url)
}

fn is_local_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };

    let address = host.trim_start_matches('[').trim_end_matches(']');

    host.eq_ignore_ascii_case("localhost")
        || address
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn endpoint(base_url: &Url, path: &str) -> Result<Url, RunnerError> {
    base_url
        .join(path)
        .map_err(|error| RunnerError::InvalidUrl(error.to_string()))
}

fn ensure_protocol(actual: u32) -> Result<(), RunnerError> {
    if actual == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(RunnerError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            actual,
        })
    }
}

async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, RunnerError> {
    if !response.status().is_success() {
        return Err(api_error(response).await);
    }

    Ok(response.json().await?)
}

async fn ensure_success(response: reqwest::Response) -> Result<(), RunnerError> {
    if response.status().is_success() {
        Ok(())
    } else {
        Err(api_error(response).await)
    }
}

async fn api_error(response: reqwest::Response) -> RunnerError {
    let status = response.status();
    let message = response
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|body| {
            body.get("message")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "request was rejected".to_string());

    RunnerError::Api { status, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_platform_uses_fixed_argv_without_a_shell() {
        for platform in ["linux", "macos", "windows"] {
            let specs = diagnostic_specs(platform);
            assert!(!specs.is_empty());
            assert!(
                specs
                    .iter()
                    .all(|spec| !matches!(spec.program, "sh" | "bash" | "powershell"))
            );
        }
    }

    #[test]
    fn base_url_must_be_http_and_is_normalized() {
        assert_eq!(
            normalize_base_url("https://builds.example.test")
                .expect("valid URL")
                .as_str(),
            "https://builds.example.test/"
        );
        assert!(normalize_base_url("file:///tmp/buildbridge").is_err());
        assert!(normalize_base_url("http://builds.example.test").is_err());
        assert!(normalize_base_url("http://localhost:8000").is_ok());
        assert!(normalize_base_url("http://127.0.0.1:8000").is_ok());
        assert!(normalize_base_url("http://[::1]:8000").is_ok());
    }
}
