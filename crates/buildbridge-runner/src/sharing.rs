use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use buildbridge_contract::{
    ArtifactResponse, ClaimedBuild, CreateArtifactRequest, CreateSharingInvitationRequest,
    RemoteArtifact, SharingGrant, SharingGrantResponse, SharingInvitation,
    SharingInvitationResponse, SharingState,
};

use super::{ApiClient, RunnerError, decode, endpoint, ensure_protocol, ensure_success};

const CHUNK_BYTES: usize = 4 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// How many times one offset may fail to transfer before the upload is given up. A chunk that
/// lands resets the budget: a slow link that keeps making progress is not a failing one.
const CHUNK_ATTEMPTS: u32 = 5;
/// A chunk request's own deadline: time to connect and get an answer, plus the bytes at a
/// floor of 256 KiB/s. The client-wide timeout is sized for small JSON exchanges, and a
/// finished archive must not fail because one 4 MiB chunk took longer than that.
const CHUNK_BASE_TIMEOUT: Duration = Duration::from_secs(60);
const CHUNK_FLOOR_BYTES_PER_SECOND: u64 = 256 * 1024;

pub(super) fn chunk_timeout(bytes: usize) -> Duration {
    CHUNK_BASE_TIMEOUT + Duration::from_secs((bytes as u64).div_ceil(CHUNK_FLOOR_BYTES_PER_SECOND))
}

/// Whether a failed request may be tried again: the control plane could not be reached or
/// did not answer whole, or it answered with a server error. A refusal is final.
fn retryable(error: &RunnerError) -> bool {
    match error {
        RunnerError::Transport(_) => true,
        RunnerError::Api { status, .. } => status.is_server_error(),
        _ => false,
    }
}

pub(super) fn checked_id(id: &str) -> Result<&str, RunnerError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(RunnerError::InvalidUrl(
            "invalid resource identifier".to_string(),
        ));
    }
    Ok(id)
}

impl ApiClient {
    /// Bind writes to this claim attempt; a stale attempt cannot complete a newer lease.
    pub fn for_build(&self, build: &ClaimedBuild) -> Self {
        let mut client = self.clone();
        client.lease_token = build.lease_token.clone();
        client
    }

    pub(super) fn lease_request(
        &self,
        request: reqwest::RequestBuilder,
    ) -> reqwest::RequestBuilder {
        match &self.lease_token {
            Some(token) => request.header("X-Build-Lease", token),
            None => request,
        }
    }

    pub async fn sharing(&self) -> Result<SharingState, RunnerError> {
        let response = self
            .http
            .get(endpoint(&self.base_url, "api/runner/sharing")?)
            .bearer_auth(&self.token)
            .send()
            .await?;
        let state: SharingState = decode(response).await?;
        ensure_protocol(state.protocol_version)?;
        Ok(state)
    }

    pub async fn create_sharing_invitation(
        &self,
        input: &CreateSharingInvitationRequest,
    ) -> Result<SharingInvitation, RunnerError> {
        let response = self
            .http
            .post(endpoint(&self.base_url, "api/runner/sharing/invitations")?)
            .bearer_auth(&self.token)
            .json(input)
            .send()
            .await?;
        let response: SharingInvitationResponse = decode(response).await?;
        ensure_protocol(response.protocol_version)?;
        Ok(response.invitation)
    }

    pub async fn approve_sharing_grant(&self, grant_id: &str) -> Result<SharingGrant, RunnerError> {
        let path = format!(
            "api/runner/sharing/grants/{}/approve",
            checked_id(grant_id)?
        );
        let response = self
            .http
            .post(endpoint(&self.base_url, &path)?)
            .bearer_auth(&self.token)
            .send()
            .await?;
        let response: SharingGrantResponse = decode(response).await?;
        ensure_protocol(response.protocol_version)?;
        Ok(response.grant)
    }

    pub async fn revoke_sharing_grant(&self, grant_id: &str) -> Result<(), RunnerError> {
        let path = format!("api/runner/sharing/grants/{}", checked_id(grant_id)?);
        let response = self
            .http
            .delete(endpoint(&self.base_url, &path)?)
            .bearer_auth(&self.token)
            .send()
            .await?;
        ensure_success(response).await
    }

    /// Files travel in bounded chunks, independent of their total size and without putting
    /// host paths in requests. Completion on the server verifies the size and SHA-256.
    pub async fn upload_artifact(
        &self,
        build_id: &str,
        path: &Path,
        input: &CreateArtifactRequest,
        cancelled: impl Fn() -> bool,
    ) -> Result<RemoteArtifact, RunnerError> {
        let check_cancelled = || {
            if cancelled() {
                Err(RunnerError::Artifact("upload stopped".to_string()))
            } else {
                Ok(())
            }
        };
        check_cancelled()?;
        if input.bytes == 0 || input.bytes > MAX_ARTIFACT_BYTES {
            return Err(RunnerError::Artifact(
                "artifacts must be between 1 byte and 2 GiB".to_string(),
            ));
        }
        let mut file = std::fs::File::open(path).map_err(|_| {
            RunnerError::Artifact("the local artifact cannot be opened".to_string())
        })?;
        let metadata = file.metadata().map_err(|_| {
            RunnerError::Artifact("the local artifact cannot be inspected".to_string())
        })?;
        if !metadata.is_file() || metadata.len() != input.bytes {
            return Err(RunnerError::Artifact(
                "the local artifact changed since the build finished".to_string(),
            ));
        }
        let base = format!("api/runner/builds/{}/artifacts", checked_id(build_id)?);
        let mut artifact = self.register_artifact(&base, input).await?;
        let path = format!("{base}/{}", checked_id(&artifact.id)?);
        let mut attempts: u32 = 0;
        while artifact.uploaded_bytes < input.bytes {
            check_cancelled()?;
            let offset = artifact.uploaded_bytes;
            let count = (input.bytes - offset).min(CHUNK_BYTES as u64) as usize;
            let mut bytes = vec![0; count];
            file.seek(SeekFrom::Start(offset))
                .and_then(|_| file.read_exact(&mut bytes))
                .map_err(|_| {
                    RunnerError::Artifact("the local artifact changed during upload".to_string())
                })?;
            let mut url = endpoint(&self.base_url, &path)?;
            url.query_pairs_mut()
                .append_pair("offset", &offset.to_string());
            let sent = async {
                let response = self
                    .lease_request(
                        self.http
                            .put(url)
                            .bearer_auth(&self.token)
                            .header("Content-Type", "application/octet-stream")
                            .timeout(chunk_timeout(count))
                            .body(bytes),
                    )
                    .send()
                    .await?;
                let response: ArtifactResponse = decode(response).await?;
                ensure_protocol(response.protocol_version)?;
                Ok::<_, RunnerError>(response.artifact)
            }
            .await;
            match sent {
                Ok(current) => {
                    if current.id != artifact.id
                        || current.name != input.name
                        || current.bytes != input.bytes
                        || current.sha256 != input.sha256
                        || current.uploaded_bytes != offset + count as u64
                    {
                        return Err(RunnerError::Artifact(
                            "the server returned an unexpected upload offset".to_string(),
                        ));
                    }
                    artifact = current;
                    attempts = 0;
                }
                Err(error) if retryable(&error) => {
                    attempts += 1;
                    if attempts >= CHUNK_ATTEMPTS {
                        return Err(error);
                    }
                    check_cancelled()?;
                    // The chunk may have landed before its answer was lost, so ask the
                    // control plane where it stands rather than resending blindly.
                    match self.register_artifact(&base, input).await {
                        Ok(current) => {
                            if current.id != artifact.id {
                                return Err(RunnerError::Artifact(
                                    "the server returned different artifact metadata".to_string(),
                                ));
                            }
                            if current.uploaded_bytes > offset {
                                attempts = 0;
                            }
                            artifact = current;
                        }
                        Err(error) if retryable(&error) => {
                            attempts += 1;
                            if attempts >= CHUNK_ATTEMPTS {
                                return Err(error);
                            }
                        }
                        Err(error) => return Err(error),
                    }
                }
                Err(error) => return Err(error),
            }
        }
        check_cancelled()?;
        let response = self
            .lease_request(
                self.http
                    .post(endpoint(&self.base_url, &format!("{path}/complete"))?)
                    .bearer_auth(&self.token),
            )
            .send()
            .await?;
        let response: ArtifactResponse = decode(response).await?;
        ensure_protocol(response.protocol_version)?;
        if response.artifact.id != artifact.id
            || response.artifact.name != input.name
            || response.artifact.status != "ready"
            || response.artifact.bytes != input.bytes
            || response.artifact.sha256 != input.sha256
            || response.artifact.uploaded_bytes != input.bytes
        {
            return Err(RunnerError::Artifact(
                "the server could not verify the artifact".to_string(),
            ));
        }
        Ok(response.artifact)
    }

    /// Registers the artifact with the control plane, or, for one already registered, reads
    /// it back: either way the answer says how much of it has arrived, which is where the
    /// upload starts or resumes. The answer must describe the file the build produced.
    async fn register_artifact(
        &self,
        base: &str,
        input: &CreateArtifactRequest,
    ) -> Result<RemoteArtifact, RunnerError> {
        let response = self
            .lease_request(
                self.http
                    .post(endpoint(&self.base_url, base)?)
                    .bearer_auth(&self.token)
                    .json(input),
            )
            .send()
            .await?;
        let response: ArtifactResponse = decode(response).await?;
        ensure_protocol(response.protocol_version)?;
        let artifact = response.artifact;
        if artifact.name != input.name
            || artifact.bytes != input.bytes
            || artifact.sha256 != input.sha256
        {
            return Err(RunnerError::Artifact(
                "the server returned different artifact metadata".to_string(),
            ));
        }
        if artifact.uploaded_bytes > input.bytes {
            return Err(RunnerError::Artifact(
                "invalid upload offset returned by the server".to_string(),
            ));
        }
        Ok(artifact)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    #[test]
    fn a_chunk_gets_time_for_its_bytes_at_a_slow_link_on_top_of_a_fixed_base() {
        assert_eq!(super::chunk_timeout(1), Duration::from_secs(61));
        assert_eq!(super::chunk_timeout(256 * 1024), Duration::from_secs(61));
        assert_eq!(
            super::chunk_timeout(256 * 1024 + 1),
            Duration::from_secs(62)
        );
        assert_eq!(
            super::chunk_timeout(super::CHUNK_BYTES),
            Duration::from_secs(76)
        );
        assert!(super::chunk_timeout(super::CHUNK_BYTES) > Duration::from_secs(30));
    }

    #[test]
    fn only_transport_and_server_failures_are_retried() {
        use reqwest::StatusCode;
        for status in [500, 502, 503, 504] {
            assert!(super::retryable(&super::RunnerError::Api {
                status: StatusCode::from_u16(status).unwrap(),
                message: "busy".into(),
            }));
        }
        for status in [400, 401, 403, 404, 409, 410, 429] {
            assert!(!super::retryable(&super::RunnerError::Api {
                status: StatusCode::from_u16(status).unwrap(),
                message: "no".into(),
            }));
        }
        assert!(!super::retryable(&super::RunnerError::Artifact(
            "changed".into()
        )));
        assert!(!super::retryable(&super::RunnerError::InvalidUrl(
            "bad".into()
        )));
        assert!(!super::retryable(&super::RunnerError::ProtocolMismatch {
            expected: 1,
            actual: 2
        }));
    }

    #[test]
    fn resource_ids_cannot_change_an_api_path() {
        for value in ["build-1", "01J-TEST_42"] {
            assert!(super::checked_id(value).is_ok());
        }
        for value in ["", "../sharing", "a/b", "a?x=1", "a#x", "a%2Fb"] {
            assert!(super::checked_id(value).is_err());
        }
    }

    #[test]
    fn resource_id_length_and_alphabet_are_exact_and_the_id_is_not_echoed() {
        let longest = "x".repeat(128);
        assert_eq!(super::checked_id(&longest).unwrap(), longest.as_str());
        assert!(super::checked_id(&"x".repeat(129)).is_err());
        for value in ["..", "grant ä", "a\0b", "a b", "a\nb", "a.b", "a:b"] {
            let error = super::checked_id(value).expect_err(value);
            let shown = error.to_string();
            assert!(!shown.contains(value.trim()), "{shown} echoes {value:?}");
            assert!(shown.contains("invalid resource identifier"), "{shown}");
        }
    }
}
