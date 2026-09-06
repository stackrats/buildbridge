//! Versioned DTOs shared by the runner core and every native client.
//!
//! Protocol v1 is additive: existing fields and enum values are not renamed
//! or removed. Breaking wire changes require a protocol version bump.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const PROTOCOL_VERSION: u32 = 1;

mod sharing;
pub use sharing::*;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct PairRunnerRequest {
    pub code: String,
    pub name: String,
    pub platform: String,
    pub architecture: String,
    pub version: Option<String>,
    pub capabilities: Vec<String>,
    pub protocol_version: u32,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct PairRunnerResponse {
    pub protocol_version: u32,
    pub runner: RunnerIdentity,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RunnerIdentity {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct HeartbeatRequest {
    pub version: String,
    pub capabilities: Vec<String>,
    /// The machines this runner can build on, so a control plane can offer them by name.
    /// Additive in protocol v1: an older control plane ignores the field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub machines: Vec<MachineReport>,
    /// Whether the owner has paused shared builds. Additive in protocol v1 like `machines`: a
    /// runner that is not pausing sends nothing, so a strict older control plane still accepts
    /// the heartbeat.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sharing_paused: bool,
}

/// What a control plane needs to know about one machine to queue work on it. Nothing here is a
/// secret or a host path: the project is named, not located.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
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
    /// What the machine builds for: `ios` or `android`. Additive in protocol v1; a control
    /// plane that predates it reads every machine as an iOS one, which every machine was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain_version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readiness_issues: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct RenewLeaseResponse {
    pub lease_expires_at: String,
}

/// What the control plane says back to a heartbeat. `queued_builds` is the recovery path for a
/// missed queue event: the heartbeat is sent anyway, so carrying the count costs nothing, and a
/// runner that sees work waiting claims it instead of waiting for a broadcast that never came.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct HeartbeatResponse {
    pub protocol_version: u32,
    pub server_time: String,
    /// Builds queued for this runner, or running on a lease that has lapsed.
    #[serde(default)]
    #[ts(type = "number")]
    pub queued_builds: u64,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct RealtimeConfigurationResponse {
    pub protocol_version: u32,
    pub realtime: RealtimeConfiguration,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeConfiguration {
    pub key: String,
    pub host: String,
    pub port: u16,
    pub scheme: String,
    pub channel: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RealtimeAuthorizationRequest {
    pub socket_id: String,
    pub channel_name: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ClaimBuildResponse {
    pub protocol_version: u32,
    pub build: ClaimedBuild,
}

#[derive(Clone, Deserialize, TS)]
#[ts(export)]
pub struct ClaimedBuild {
    pub id: String,
    pub kind: BuildKind,
    pub payload: serde_json::Value,
    pub lease_expires_at: String,
    #[ts(type = "number")]
    pub next_log_sequence: u64,
    #[serde(default)]
    pub authorization: Option<BuildAuthorization>,
    #[serde(default)]
    pub lease_token: Option<String>,
}

/// The lease token binds writes to one claim; it is a secret and is never printed, so a claim
/// that ends up in a log or an error message shows only that a token is present.
impl std::fmt::Debug for ClaimedBuild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimedBuild")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("payload", &self.payload)
            .field("lease_expires_at", &self.lease_expires_at)
            .field("next_log_sequence", &self.next_log_sequence)
            .field("authorization", &self.authorization)
            .field(
                "lease_token",
                &self.lease_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum BuildKind {
    Diagnostics,
    /// A signed Release archive and App Store export on one managed macOS machine.
    AppleArchive,
    /// A signed release app bundle and APK on one Android toolchain machine.
    AndroidRelease,
}

/// The payload of an `apple_archive` build. The machine is the runner's own; `git_ref` names a
/// revision of the project already approved on that machine, and `None` builds the approved
/// folder as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
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

/// The payload of an `android_release` build: the same shape as an Apple archive's, on a
/// machine whose provider is the Android toolchain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AndroidReleasePayload {
    pub machine_id: String,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub env_set: Option<String>,
}

impl AndroidReleasePayload {
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(value.clone())
            .map_err(|error| format!("the android_release payload is not valid: {error}"))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
    System,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AppendLogsRequest {
    pub lines: Vec<BuildLogLine>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct BuildLogLine {
    #[ts(type = "number")]
    pub sequence: u64,
    pub stream: LogStream,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CompletionStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
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
    fn an_android_release_claim_decodes_beside_the_apple_one() {
        let claim: ClaimBuildResponse = serde_json::from_str(
            r#"{
                "protocol_version": 1,
                "build": {
                    "id": "build-3",
                    "kind": "android_release",
                    "payload": {"machine_id": "droid", "env_set": "production"},
                    "lease_expires_at": "2026-09-01T10:00:00Z",
                    "next_log_sequence": 1
                }
            }"#,
        )
        .expect("contract JSON should decode");

        assert_eq!(claim.build.kind, BuildKind::AndroidRelease);
        let payload = AndroidReleasePayload::from_value(&claim.build.payload).expect("payload");
        assert_eq!(payload.machine_id, "droid");
        assert_eq!(payload.env_set.as_deref(), Some("production"));
        assert_eq!(payload.git_ref, None);
    }

    #[test]
    fn a_machine_report_without_a_platform_keeps_the_original_wire_shape() {
        let report = MachineReport {
            id: "m".to_string(),
            name: "Mac".to_string(),
            ready: true,
            project: None,
            bundle_identifier: None,
            repository: None,
            env_set: None,
            env_sets: Vec::new(),
            platform: None,
            executor: None,
            toolchain_version: None,
            readiness_issues: Vec::new(),
            architecture: None,
        };
        let json = serde_json::to_value(&report).expect("serializes");
        assert!(json.get("platform").is_none());
        let android: MachineReport = serde_json::from_value(serde_json::json!({
            "id": "d", "name": "Droid", "ready": false, "project": null,
            "bundle_identifier": null, "repository": null, "env_set": null, "platform": "android"
        }))
        .expect("decodes");
        assert_eq!(android.platform.as_deref(), Some("android"));
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
            sharing_paused: false,
        };
        let json = serde_json::to_value(&request).expect("serializes");

        assert!(json.get("machines").is_none());
        assert!(
            json.get("sharing_paused").is_none(),
            "a runner that is not pausing sends the original heartbeat shape"
        );

        let paused = HeartbeatRequest {
            sharing_paused: true,
            ..request
        };
        let json = serde_json::to_value(&paused).expect("serializes");
        assert_eq!(json["sharing_paused"], serde_json::json!(true));
    }

    #[test]
    fn a_claimed_build_never_prints_its_lease_token() {
        let claim: ClaimBuildResponse = serde_json::from_str(
            r#"{
                "protocol_version": 1,
                "build": {
                    "id": "build-4",
                    "kind": "diagnostics",
                    "payload": {},
                    "lease_expires_at": "2026-09-01T10:00:00Z",
                    "next_log_sequence": 1,
                    "lease_token": "lease-secret-value"
                }
            }"#,
        )
        .expect("contract JSON should decode");

        let shown = format!("{:?}", claim.build);
        assert!(!shown.contains("lease-secret-value"), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
        assert!(shown.contains("build-4"), "{shown}");
        assert_eq!(
            claim.build.lease_token.as_deref(),
            Some("lease-secret-value")
        );

        let bare: ClaimedBuild = serde_json::from_value(serde_json::json!({
            "id": "build-5", "kind": "diagnostics", "payload": {},
            "lease_expires_at": "2026-09-01T10:00:00Z", "next_log_sequence": 1
        }))
        .expect("decodes");
        assert!(format!("{bare:?}").contains("lease_token: None"));
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
