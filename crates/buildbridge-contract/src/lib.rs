//! Versioned DTOs shared by the runner core and every native client.
//!
//! Protocol v1 is additive: existing fields and enum values are not renamed
//! or removed. Breaking wire changes require a protocol version bump.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct PairRunnerRequest {
    pub code: String,
    pub name: String,
    pub platform: String,
    pub architecture: String,
    pub version: Option<String>,
    pub capabilities: Vec<String>,
    pub protocol_version: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PairRunnerResponse {
    pub protocol_version: u32,
    pub runner: RunnerIdentity,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerIdentity {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HeartbeatRequest {
    pub version: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RealtimeConfigurationResponse {
    pub protocol_version: u32,
    pub realtime: RealtimeConfiguration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeConfiguration {
    pub key: String,
    pub host: String,
    pub port: u16,
    pub scheme: String,
    pub channel: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RealtimeAuthorizationRequest {
    pub socket_id: String,
    pub channel_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClaimBuildResponse {
    pub protocol_version: u32,
    pub build: ClaimedBuild,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClaimedBuild {
    pub id: String,
    pub kind: BuildKind,
    pub payload: serde_json::Value,
    pub lease_expires_at: String,
    pub next_log_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildKind {
    Diagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
    System,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppendLogsRequest {
    pub lines: Vec<BuildLogLine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildLogLine {
    pub sequence: u64,
    pub stream: LogStream,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompleteBuildRequest {
    pub status: CompletionStatus,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_payload_matches_the_control_plane_wire_shape() {
        let claim: ClaimBuildResponse = serde_json::from_str(
            r#"{
                "protocol_version": 1,
                "build": {
                    "id": "build-1",
                    "kind": "diagnostics",
                    "payload": {"protocol_version": 1},
                    "lease_expires_at": "2026-09-01T10:00:00Z",
                    "next_log_sequence": 3
                }
            }"#,
        )
        .expect("contract JSON should decode");

        assert_eq!(claim.protocol_version, PROTOCOL_VERSION);
        assert_eq!(claim.build.kind, BuildKind::Diagnostics);
        assert_eq!(claim.build.next_log_sequence, 3);
    }

    #[test]
    fn realtime_configuration_matches_the_control_plane_wire_shape() {
        let response: RealtimeConfigurationResponse = serde_json::from_str(
            r#"{
                "protocol_version": 1,
                "realtime": {
                    "key": "buildbridge-key",
                    "host": "127.0.0.1",
                    "port": 8080,
                    "scheme": "http",
                    "channel": "private-runners.runner-1"
                }
            }"#,
        )
        .expect("realtime JSON should decode");

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.realtime.port, 8080);
        assert_eq!(response.realtime.channel, "private-runners.runner-1");
    }
}
