//! Scoped invitations and private artifact delivery. Runner credentials never leave their host.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SharingInvitation {
    pub id: String,
    #[serde(default)]
    pub code: Option<String>,
    pub policy_id: String,
    pub machine_id: String,
    pub project: Option<String>,
    pub repository: String,
    pub bundle_identifier: Option<String>,
    pub kinds: Vec<super::BuildKind>,
    pub env_sets: Vec<String>,
    pub expires_at: String,
    pub access_hours: u32,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SharingGrant {
    pub id: String,
    pub invitation_id: String,
    pub policy_id: String,
    pub machine_id: String,
    pub project: Option<String>,
    pub repository: String,
    pub bundle_identifier: Option<String>,
    pub kinds: Vec<super::BuildKind>,
    pub env_sets: Vec<String>,
    pub requester_id: String,
    pub requester_name: String,
    pub requester_email: String,
    pub status: String,
    pub expires_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SharingState {
    pub protocol_version: u32,
    pub invitations: Vec<SharingInvitation>,
    pub grants: Vec<SharingGrant>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateSharingInvitationRequest {
    pub machine_id: String,
    pub kinds: Vec<super::BuildKind>,
    /// An empty string explicitly permits building without an environment.
    pub env_sets: Vec<String>,
    pub expires_in_minutes: u32,
    pub access_hours: u32,
    pub policy_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SharingInvitationResponse {
    pub protocol_version: u32,
    pub invitation: SharingInvitation,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SharingGrantResponse {
    pub protocol_version: u32,
    pub grant: SharingGrant,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BuildAuthorization {
    pub grant_id: String,
    pub requester_id: String,
    pub policy_id: String,
    pub machine_id: String,
    pub project: Option<String>,
    pub repository: String,
    pub bundle_identifier: Option<String>,
    pub kinds: Vec<super::BuildKind>,
    pub env_sets: Vec<String>,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateArtifactRequest {
    pub name: String,
    #[ts(type = "number")]
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RemoteArtifact {
    pub id: String,
    pub name: String,
    #[ts(type = "number")]
    pub bytes: u64,
    pub sha256: String,
    #[ts(type = "number")]
    pub uploaded_bytes: u64,
    pub status: String,
    pub download_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ArtifactResponse {
    pub protocol_version: u32,
    pub artifact: RemoteArtifact,
}

/// Shared builds name immutable commits, never a moving branch or the owner's dirty folder.
pub fn valid_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64) && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    #[test]
    fn shared_source_requires_a_full_immutable_commit() {
        assert!(super::valid_commit(&"a".repeat(40)));
        assert!(super::valid_commit(&"A".repeat(64)));
        for value in ["main", "abcdef0", "HEAD", "--upload-pack=bad", ""] {
            assert!(!super::valid_commit(value));
        }
        assert!(!super::valid_commit(&"g".repeat(40)));
    }

    #[test]
    fn commit_validation_is_exact_about_length_and_alphabet() {
        for length in [39, 41, 63, 65] {
            assert!(!super::valid_commit(&"0".repeat(length)), "{length}");
        }
        assert!(super::valid_commit(&"aB3d".repeat(10)));
        assert!(super::valid_commit(&format!(
            "{}{}",
            "0".repeat(20),
            "F".repeat(20)
        )));
        // Twenty two-byte digits are forty bytes long but not ASCII hex.
        assert!(!super::valid_commit(&"\u{0661}".repeat(20)));
        assert!(!super::valid_commit(&format!("{} ", "a".repeat(39))));
    }

    #[test]
    fn sharing_records_decode_when_a_lean_server_omits_optional_fields_or_adds_new_ones() {
        let invitation: super::SharingInvitation = serde_json::from_str(
            r#"{
                "id": "inv-1", "policy_id": "pol-1", "machine_id": "default",
                "repository": "https://example.com/app.git",
                "kinds": ["apple_archive", "android_release"], "env_sets": [""],
                "expires_at": "2026-09-08T00:00:00Z", "access_hours": 24, "status": "pending",
                "introduced_later": {"nested": true}
            }"#,
        )
        .expect("an invitation without code, project, or bundle identifier decodes");
        assert_eq!(invitation.code, None);
        assert_eq!(invitation.project, None);
        assert_eq!(invitation.bundle_identifier, None);
        assert_eq!(
            invitation.kinds,
            [
                crate::BuildKind::AppleArchive,
                crate::BuildKind::AndroidRelease
            ]
        );
        assert_eq!(invitation.env_sets, [""]);

        let grant: super::SharingGrant = serde_json::from_str(
            r#"{
                "id": "grant-1", "invitation_id": "inv-1", "policy_id": "pol-1",
                "machine_id": "default", "repository": "https://example.com/app.git",
                "kinds": [], "env_sets": [], "requester_id": "user-2",
                "requester_name": "Sam", "requester_email": "sam@example.com",
                "status": "approved", "created_at": "2026-09-07T00:00:00Z"
            }"#,
        )
        .expect("a grant without an expiry decodes");
        assert_eq!(grant.expires_at, None);
        assert!(grant.kinds.is_empty());

        let artifact: super::RemoteArtifact = serde_json::from_str(
            r#"{
                "id": "art-1", "name": "App.ipa", "bytes": 9007199254740993, "sha256": "ab",
                "uploaded_bytes": 0, "status": "uploading"
            }"#,
        )
        .expect("an artifact without a download URL decodes");
        assert_eq!(artifact.download_url, None);
        assert_eq!(
            artifact.bytes, 9_007_199_254_740_993,
            "sizes past 2^53 stay exact"
        );

        let unknown_kind = serde_json::from_str::<super::SharingInvitation>(
            r#"{
                "id": "inv-2", "policy_id": "pol-1", "machine_id": "default",
                "repository": "r", "kinds": ["windows_msix"], "env_sets": [],
                "expires_at": "2026-09-08T00:00:00Z", "access_hours": 1, "status": "pending"
            }"#,
        );
        assert!(
            unknown_kind.is_err(),
            "a build kind this runner cannot do is refused"
        );
    }

    #[test]
    fn a_claim_with_an_authorization_block_round_trips_its_scope() {
        let claim: crate::ClaimedBuild = serde_json::from_str(
            r#"{
                "id": "build-9", "kind": "apple_archive", "payload": {"machine_id": "default"},
                "lease_expires_at": "2026-09-07T10:00:00Z", "next_log_sequence": 1,
                "lease_token": "lease-abc",
                "authorization": {
                    "grant_id": "grant-1", "requester_id": "user-2", "policy_id": "pol-1",
                    "machine_id": "default", "repository": "https://example.com/app.git",
                    "bundle_identifier": "com.example.app",
                    "kinds": ["apple_archive"], "env_sets": ["staging"],
                    "expires_at": "2026-09-08T00:00:00Z"
                }
            }"#,
        )
        .expect("a shared claim decodes");
        let authorization = claim.authorization.as_ref().expect("authorization present");
        assert_eq!(authorization.project, None);
        assert_eq!(authorization.kinds, [crate::BuildKind::AppleArchive]);
        assert_eq!(claim.lease_token.as_deref(), Some("lease-abc"));

        let json = serde_json::to_value(authorization).expect("serializes");
        assert_eq!(json["kinds"], serde_json::json!(["apple_archive"]));
        assert_eq!(json["project"], serde_json::Value::Null);
        assert_eq!(json["bundle_identifier"], "com.example.app");
        let again: super::BuildAuthorization = serde_json::from_value(json).expect("round trips");
        assert_eq!(again.grant_id, "grant-1");
        assert_eq!(again.env_sets, ["staging"]);
    }
}
