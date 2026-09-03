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
    /// The machines this runner can build on, so a control plane can offer them by name.
    /// Additive in protocol v1: an older control plane ignores the field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub machines: Vec<MachineReport>,
}

/// What a control plane needs to know about one machine to queue work on it. Nothing here is a
/// secret or a host path: the project is named, not located.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineReport {
    pub id: String,
    pub name: String,
    /// Setup complete, a project approved, signing provisioned: a signed archive can be queued.
    pub ready: bool,
    pub project: Option<String>,
    pub bundle_identifier: Option<String>,
    /// The approved project's git remote, when it has one. A remote build names a ref of this
    /// repository; it never supplies a repository of its own.
    pub repository: Option<String>,
    /// The name of the environment set attached to the machine, if any.
    pub env_set: Option<String>,
    /// Every env set this host holds, by name, so a build can choose one.
    #[serde(default)]
    pub env_sets: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RenewLeaseResponse {
    pub lease_expires_at: String,
}

/// What the control plane says back to a heartbeat. `queued_builds` is the recovery path for a
/// missed queue event: the heartbeat is sent anyway, so carrying the count costs nothing, and a
/// runner that sees work waiting claims it instead of waiting for a broadcast that never came.
#[derive(Debug, Clone, Deserialize)]
pub struct HeartbeatResponse {
    pub protocol_version: u32,
    pub server_time: String,
    /// Builds queued for this runner, or running on a lease that has lapsed.
    #[serde(default)]
    pub queued_builds: u64,
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
    /// A signed Release archive and App Store export on one managed macOS machine.
    AppleArchive,
}

/// The payload of an `apple_archive` build. The machine is the runner's own; `git_ref` names a
/// revision of the project already approved on that machine, and `None` builds the approved
/// folder as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppleArchivePayload {
    pub machine_id: String,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    /// The env set to build the web assets with, by name. `None` uses the machine's attached
    /// set; the runner never accepts a value, only a name it already holds.
    #[serde(default)]
    pub env_set: Option<String>,
}

impl AppleArchivePayload {
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(value.clone())
            .map_err(|error| format!("the apple_archive payload is not valid: {error}"))
    }
}

/// A ref a runner is willing to fetch: a branch, tag, or commit as git names them, without
/// anything that could reshape a command line.
pub fn valid_git_ref(git_ref: &str) -> bool {
    !git_ref.is_empty()
        && git_ref.len() <= 200
        && !git_ref.starts_with('-')
        && !git_ref.starts_with('/')
        && !git_ref.ends_with('/')
        && !git_ref.contains("..")
        && !git_ref.contains("@{")
        && !git_ref.ends_with(".lock")
        && git_ref
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
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
    /// Kind-specific outcome, such as the artifacts of an archive. Additive in protocol v1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
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
    fn an_apple_archive_claim_carries_the_machine_and_an_optional_ref() {
        let claim: ClaimBuildResponse = serde_json::from_str(
            r#"{
                "protocol_version": 1,
                "build": {
                    "id": "build-2",
                    "kind": "apple_archive",
                    "payload": {"protocol_version": 1, "machine_id": "default", "ref": "release/1.4"},
                    "lease_expires_at": "2026-09-01T10:00:00Z",
                    "next_log_sequence": 1
                }
            }"#,
        )
        .expect("contract JSON should decode");

        assert_eq!(claim.build.kind, BuildKind::AppleArchive);
        let payload = AppleArchivePayload::from_value(&claim.build.payload).expect("payload");
        assert_eq!(payload.machine_id, "default");
        assert_eq!(payload.git_ref.as_deref(), Some("release/1.4"));

        let bare = AppleArchivePayload::from_value(&serde_json::json!({"machine_id": "m1"}))
            .expect("ref is optional");
        assert_eq!(bare.git_ref, None);
    }

    #[test]
    fn a_heartbeat_response_reports_waiting_work_and_tolerates_its_absence() {
        let with: HeartbeatResponse = serde_json::from_str(
            r#"{"protocol_version": 1, "server_time": "2026-09-03T10:00:00Z", "queued_builds": 2}"#,
        )
        .expect("decodes");
        assert_eq!(with.queued_builds, 2);

        let older: HeartbeatResponse = serde_json::from_str(
            r#"{"protocol_version": 1, "server_time": "2026-09-03T10:00:00Z"}"#,
        )
        .expect("an older control plane omits the count");
        assert_eq!(older.queued_builds, 0);
    }

    #[test]
    fn a_heartbeat_without_machines_keeps_the_original_wire_shape() {
        let request = HeartbeatRequest {
            version: "0.1.0".to_string(),
            capabilities: vec!["diagnostics".to_string()],
            machines: Vec::new(),
        };
        let json = serde_json::to_value(&request).expect("serializes");

        assert!(json.get("machines").is_none());
    }

    #[test]
    fn git_refs_are_limited_to_what_git_itself_names() {
        for ok in ["main", "release/1.4", "v2.0.1", "feature_x-2", "3f9c2ab"] {
            assert!(valid_git_ref(ok), "{ok} should be accepted");
        }
        for bad in [
            "", "-force", "/main", "main/", "a..b", "a@{1}", "x.lock", "a b", "$(x)", "a;b",
        ] {
            assert!(!valid_git_ref(bad), "{bad} should be rejected");
        }
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
