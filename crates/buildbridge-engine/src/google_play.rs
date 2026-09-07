//! Explicit upload of a reviewed Android bundle to an internal-testing draft in Google Play.
//! Credentials stay in the OS vault and all network destinations are fixed Google origins.

use super::*;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Client, RequestBuilder, Response, StatusCode, Url};
use serde_json::{Value, json};
use std::future::Future;
use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

const VAULT_SERVICE: &str = "dev.buildbridge.desktop.google-play";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const API_ORIGIN: &str = "https://androidpublisher.googleapis.com";
const SCOPE: &str = "https://www.googleapis.com/auth/androidpublisher";
const MAX_CREDENTIAL_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const UPLOAD_CHUNK_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GooglePlayConnection {
    pub configured: bool,
    pub client_email: Option<String>,
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GooglePlayUploadResult {
    pub package_name: String,
    pub version_code: String,
    pub track: String,
    #[ts(type = "\"draft\"")]
    pub status: String,
    pub sha256: String,
}

// Deliberately has no Debug implementation. Unknown JSON fields (including token_uri) are
// discarded; an imported file can never choose where its private-key assertion is sent.
#[derive(Serialize, Deserialize)]
struct ServiceAccount {
    #[serde(rename = "type")]
    kind: String,
    client_email: String,
    project_id: String,
    private_key_id: String,
    private_key: String,
}

impl ServiceAccount {
    fn summary(&self) -> GooglePlayConnection {
        GooglePlayConnection {
            configured: true,
            client_email: Some(self.client_email.clone()),
            project_id: Some(self.project_id.clone()),
        }
    }

    /// A portable key file reconstructed from the fields retained in the vault. Never reuse
    /// an imported token URI: external clients must send assertions only to Google too.
    fn export_json(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec_pretty(&json!({
            "type": self.kind,
            "client_email": self.client_email,
            "project_id": self.project_id,
            "private_key_id": self.private_key_id,
            "private_key": self.private_key,
            "token_uri": TOKEN_URL,
            "auth_uri": "https://accounts.google.com/o/oauth2/auth",
        }))
        .map_err(|_| "The service-account key could not be prepared for export.".to_string())
    }
}

fn parse_account(encoded: &[u8]) -> Result<ServiceAccount, String> {
    if encoded.len() > MAX_CREDENTIAL_BYTES {
        return Err("The service-account JSON file is too large.".into());
    }
    let account: ServiceAccount = serde_json::from_slice(encoded)
        .map_err(|_| "Choose a Google service-account JSON key file.".to_string())?;
    let safe = |value: &str, max: usize| {
        !value.is_empty()
            && value.len() <= max
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'@')
            })
    };
    if account.kind != "service_account"
        || !safe(&account.client_email, 254)
        || !account.client_email.ends_with(".gserviceaccount.com")
        || account.client_email.matches('@').count() != 1
        || !safe(&account.project_id, 128)
        || !safe(&account.private_key_id, 128)
    {
        return Err("The JSON file is missing valid Google service-account details.".into());
    }
    // Signing also validates RSA key parameters, before anything reaches the vault/network.
    account_assertion(&account, 1)?;
    Ok(account)
}

fn account_assertion(account: &ServiceAccount, now: u64) -> Result<String, String> {
    let key = EncodingKey::from_rsa_pem(account.private_key.as_bytes()).map_err(|_| {
        "The service-account file does not contain a valid RSA private key.".to_string()
    })?;
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(account.private_key_id.clone());
    encode(
        &header,
        &json!({"iss": account.client_email, "scope": SCOPE, "aud": TOKEN_URL,
            "iat": now, "exp": now + 3600}),
        &key,
    )
    .map_err(|_| "buildbridge could not sign with this service-account RSA key.".to_string())
}

fn require_android(app: &Engine, machine_id: &str) -> Result<(), String> {
    MachinePaths::resolve(app, machine_id)?;
    if machines::load_registry(app)?
        .find(machine_id)?
        .config
        .provider
        .is_macos()
    {
        return Err("Google Play uploads require an Android machine.".into());
    }
    Ok(())
}

fn vault_entry(account_id: &str) -> Result<Entry, String> {
    Entry::new(VAULT_SERVICE, account_id)
        .map_err(|_| "The operating-system credential vault is unavailable.".to_string())
}

#[derive(Serialize, Deserialize)]
struct ConnectionMarker {
    created_at: u64,
    nonce: u128,
}

fn marker_path(app: &Engine, machine_id: &str) -> Result<PathBuf, String> {
    Ok(MachinePaths::resolve(app, machine_id)?
        .android_workspace()
        .with_file_name("google-play-connection.json"))
}

/// The vault account a marker names. The account id is derived, never stored: it binds the
/// credential to this data directory, this machine id and the machine's creation time.
fn account_id_for(app: &Engine, machine_id: &str, created_at: u64, nonce: u128) -> String {
    format!(
        "{}:{machine_id}:{created_at}:{nonce}",
        app.data_dir().display()
    )
}

/// The marker on disk, as read for the machine created at `created_at`. A marker written for
/// an earlier machine of the same name, or one that is not a marker at all, is not a
/// connection: the credential it once pointed at is unreachable through it either way.
fn read_marker(
    path: &std::path::Path,
    created_at: u64,
) -> Result<Option<ConnectionMarker>, String> {
    match fs::read(path) {
        Ok(encoded) => Ok(serde_json::from_slice::<ConnectionMarker>(&encoded)
            .ok()
            .filter(|marker| marker.created_at == created_at)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("The local Google Play connection record could not be read.".into()),
    }
}

/// Whatever account ids a marker file still names, whole or not, for removing their vault
/// entries. A marker is two integers; a torn one may still carry both, and a stale one from a
/// previous machine of this name names an entry nothing else will ever remove.
fn recoverable_account_ids(app: &Engine, machine_id: &str, path: &std::path::Path) -> Vec<String> {
    let Ok(encoded) = fs::read(path) else {
        return Vec::new();
    };
    if let Ok(marker) = serde_json::from_slice::<ConnectionMarker>(&encoded) {
        return vec![account_id_for(
            app,
            machine_id,
            marker.created_at,
            marker.nonce,
        )];
    }
    let text = String::from_utf8_lossy(&encoded);
    let field = |name: &str| -> Option<u128> {
        let start = text.find(&format!("\"{name}\""))? + name.len() + 2;
        let rest = text[start..].trim_start().strip_prefix(':')?.trim_start();
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    match (field("created_at"), field("nonce")) {
        (Some(created_at), Some(nonce)) => {
            vec![account_id_for(app, machine_id, created_at as u64, nonce)]
        }
        _ => Vec::new(),
    }
}

fn connection_account_id(
    app: &Engine,
    machine_id: &str,
    create: bool,
) -> Result<Option<String>, String> {
    let created_at = machines::load_registry(app)?
        .find(machine_id)?
        .created_at_epoch_seconds;
    let path = marker_path(app, machine_id)?;
    let marker = match read_marker(&path, created_at)? {
        Some(marker) => marker,
        None if create => {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| {
                    "Correct the host clock before connecting to Google Play.".to_string()
                })?
                .as_nanos();
            let marker = ConnectionMarker { created_at, nonce };
            write_restricted_file(
                &path,
                &serde_json::to_vec(&marker).map_err(|_| {
                    "Could not encode the Google Play connection record.".to_string()
                })?,
            )?;
            marker
        }
        None => return Ok(None),
    };
    Ok(Some(account_id_for(
        app,
        machine_id,
        marker.created_at,
        marker.nonce,
    )))
}

/// Deletes the named vault entries as far as the vault allows. Removal is cleanup: an entry
/// that is already gone, or a vault that will not answer, must not keep a machine from being
/// disconnected or deleted.
fn remove_vault_entries(account_ids: Vec<String>) -> Result<(), String> {
    let mut failure = None;
    for account_id in account_ids {
        match vault_entry(&account_id).and_then(|entry| match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => {
                Err("The Google Play credential could not be removed from the OS vault.".into())
            }
        }) {
            Ok(()) => {}
            Err(error) => failure = Some(error),
        }
    }
    failure.map_or(Ok(()), Err)
}

async fn read_account(app: &Engine, machine_id: &str) -> Result<Option<ServiceAccount>, String> {
    let Some(account_id) = connection_account_id(app, machine_id, false)? else {
        return Ok(None);
    };
    tokio::task::spawn_blocking(move || match vault_entry(&account_id)?.get_password() {
        Ok(encoded) => parse_account(encoded.as_bytes()).map(Some),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err("The Google Play credential could not be read from the OS vault.".into()),
    })
    .await
    .map_err(|_| "The Google Play credential operation failed.".to_string())?
}

pub async fn google_play_connection(
    app: &Engine,
    machine_id: String,
) -> Result<GooglePlayConnection, String> {
    require_android(app, &machine_id)?;
    Ok(read_account(app, &machine_id)
        .await?
        .map(|account| account.summary())
        .unwrap_or_default())
}

/// An explicit local export; no private-key material is returned to the desktop webview.
pub async fn export_google_play_credential(
    app: &Engine,
    machine_id: String,
    path: String,
) -> Result<(), String> {
    require_android(app, &machine_id)?;
    let _guard = begin_machine_operation(app, &machine_id, "exporting_google_play_credential")?;
    let account = read_account(app, &machine_id)
        .await?
        .ok_or_else(|| "This machine's Google Play credential is no longer stored.".to_string())?;
    tokio::task::spawn_blocking(move || {
        credentials::export_credential_bytes(std::path::Path::new(&path), &account.export_json()?)
    })
    .await
    .map_err(|_| "The service-account key could not be exported.".to_string())?
}

pub async fn configure_google_play(
    app: &Engine,
    machine_id: String,
    path: String,
) -> Result<GooglePlayConnection, String> {
    require_android(app, &machine_id)?;
    let _guard = begin_machine_operation(app, &machine_id, "configuring_google_play")?;
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        let path = std::path::Path::new(&path);
        if !path.is_absolute() || !path.is_file() {
            return Err("Choose a local Google service-account JSON key file.".into());
        }
        let file = fs::File::open(path)
            .map_err(|_| "The selected service-account file could not be opened.".to_string())?;
        let mut encoded = Vec::new();
        file.take(MAX_CREDENTIAL_BYTES as u64 + 1)
            .read_to_end(&mut encoded)
            .map_err(|_| "The selected service-account file could not be read.".to_string())?;
        let account = parse_account(&encoded)?;
        let stored = serde_json::to_string(&account)
            .map_err(|_| "The service-account credential could not be encoded.".to_string())?;
        let account_id = connection_account_id(&app, &machine_id, true)?.expect("created marker");
        vault_entry(&account_id)?
            .set_password(&stored)
            .map_err(|_| {
                "The Google Play credential could not be saved in the OS vault.".to_string()
            })?;
        Ok(account.summary())
    })
    .await
    .map_err(|_| "The Google Play credential operation failed.".to_string())?
}

pub async fn disconnect_google_play(app: &Engine, machine_id: String) -> Result<(), String> {
    require_android(app, &machine_id)?;
    let _guard = begin_machine_operation(app, &machine_id, "configuring_google_play")?;
    remove_google_play_credentials(app, &machine_id).await
}

/// Lifecycle deletion already holds the machine lock; never try to acquire it again here.
///
/// The marker file goes whatever state it is in — whole, stale, torn or unreadable — and the
/// vault entry of any account id it still names goes with it. A machine is never left
/// connected to a record that cannot be read.
pub(crate) async fn remove_google_play_credentials(
    app: &Engine,
    machine_id: &str,
) -> Result<(), String> {
    let created_at = machines::load_registry(app)?
        .find(machine_id)?
        .created_at_epoch_seconds;
    let path = marker_path(app, machine_id)?;
    let (account_ids, connected) = match read_marker(&path, created_at)? {
        Some(marker) => (
            vec![account_id_for(
                app,
                machine_id,
                marker.created_at,
                marker.nonce,
            )],
            true,
        ),
        None => (recoverable_account_ids(app, machine_id, &path), false),
    };
    tokio::task::spawn_blocking(move || {
        let vault = remove_vault_entries(account_ids);
        // A live connection keeps its marker until the vault has let the credential go, so a
        // vault that is briefly unavailable can be asked again. A record that is not a
        // connection is removed regardless; the entry it may name is cleanup, not access.
        if connected {
            vault?;
        }
        remove_file_if_present(&path)
    })
    .await
    .map_err(|_| "The Google Play credential operation failed.".to_string())?
}

pub async fn upload_google_play(
    app: &Engine,
    machine_id: String,
    expected_sha256: String,
) -> Result<GooglePlayUploadResult, String> {
    require_android(app, &machine_id)?;
    let guard = begin_machine_operation(app, &machine_id, "uploading_google_play")?;
    let scope = guard.scope();
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let release = load_android_release(&paths)?
        .ok_or_else(|| "Build a signed Android App Bundle before uploading.".to_string())?
        .result;
    let path = reviewed_aab_path(&paths, &release, &expected_sha256)?;
    let artifact = release.aab.as_ref().expect("reviewed bundle exists");
    let bytes = artifact.bytes;
    let sha256 = artifact.sha256.clone();
    let verify_scope = Arc::clone(&scope);
    let file = tokio::task::spawn_blocking(move || {
        let _scope = buildbridge_machines::enter_operation(verify_scope);
        verify_aab(&path, bytes, &sha256)?;
        fs::File::open(path).map_err(|_| "The retained AAB is unavailable.".to_string())
    })
    .await
    .map_err(|_| "Could not verify the retained AAB.".to_string())??;
    let account = read_account(app, &machine_id).await?.ok_or_else(|| {
        "Import a Google Play service-account JSON key before uploading.".to_string()
    })?;
    let client = PlayClient::new()?;
    let progress = |detail: String| {
        emit_machine_progress(
            app,
            "machine-upload-progress",
            &machine_id,
            json!({"detail": detail}),
        )
    };
    progress("Authenticating with Google Play…".into());
    let token = client.authenticate(&account, &scope).await?;
    client
        .upload(&token, &release, file, &scope, progress)
        .await
}

/// A version code Google Play already holds for the app, with the release it belongs to when
/// a track names one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayVersionCode {
    pub code: u64,
    pub release_name: Option<String>,
}

/// Every version code Google Play has received for the app: its bundles, its APKs and the
/// releases on its tracks, read through a throwaway edit that is deleted afterwards. A read
/// takes no machine lock; nothing is committed.
pub(crate) async fn fetch_version_codes(
    app: &Engine,
    machine_id: &str,
    package_name: &str,
) -> Result<Vec<PlayVersionCode>, String> {
    if !valid_package_name(package_name) {
        return Err("The project's application identifier is not a valid package name.".into());
    }
    let account = read_account(app, machine_id).await?.ok_or_else(|| {
        "Import a Google Play service-account JSON key before asking Google Play about builds."
            .to_string()
    })?;
    let client = PlayClient::new()?;
    let scope = OperationScope::new();
    let token = client.authenticate(&account, &scope).await?;
    client.version_codes(&token, package_name, &scope).await
}

fn reviewed_aab_path(
    paths: &MachinePaths,
    release: &AndroidReleaseResult,
    expected_sha256: &str,
) -> Result<PathBuf, String> {
    let aab = release
        .aab
        .as_ref()
        .ok_or_else(|| "Build an AAB release before uploading to Google Play.".to_string())?;
    if expected_sha256.len() != 64
        || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !aab.sha256.eq_ignore_ascii_case(expected_sha256)
    {
        return Err(
            "The retained AAB changed. Review the current release before uploading.".into(),
        );
    }
    if !valid_package_name(&release.application_id)
        || !release
            .version_code
            .parse::<u32>()
            .is_ok_and(|code| code > 0 && code <= 2_100_000_000)
    {
        return Err("The retained release has an invalid package name or version code. Build a new release.".into());
    }
    validated_android_release_directory(paths, release)?;
    let path =
        fs::canonicalize(&aab.path).map_err(|_| "The retained AAB is unavailable.".to_string())?;
    if path.extension().and_then(|extension| extension.to_str()) != Some("aab") {
        return Err("The retained upload artifact must be an AAB file.".into());
    }
    Ok(path)
}

fn valid_package_name(value: &str) -> bool {
    value.len() <= 255
        && value.contains('.')
        && value.split('.').all(|part| {
            part.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}

fn verify_aab(path: &std::path::Path, bytes: u64, sha256: &str) -> Result<(), String> {
    let metadata =
        fs::metadata(path).map_err(|_| "The retained AAB is unavailable.".to_string())?;
    if !metadata.is_file() || bytes == 0 || metadata.len() != bytes {
        return Err(
            "The retained AAB's size changed. Build a new release before uploading.".into(),
        );
    }
    if !buildbridge_machines::native_sha256(path)?.eq_ignore_ascii_case(sha256) {
        return Err(
            "The retained AAB's checksum changed. Build a new release before uploading.".into(),
        );
    }
    Ok(())
}

struct PlayClient {
    http: Client,
    origin: String,
    token_url: String,
    /// The unit of the exponential back-off between chunk retries.
    retry_delay: Duration,
}

impl PlayClient {
    fn new() -> Result<Self, String> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|_| "Could not create the Google Play connection.".to_string())?;
        Ok(Self {
            http,
            origin: API_ORIGIN.into(),
            token_url: TOKEN_URL.into(),
            retry_delay: Duration::from_secs(1),
        })
    }

    async fn authenticate(
        &self,
        account: &ServiceAccount,
        scope: &OperationScope,
    ) -> Result<String, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "Correct the host clock before connecting to Google Play.".to_string())?
            .as_secs();
        let assertion = account_assertion(account, now)?;
        let response = request(
            self.http.post(&self.token_url).form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &assertion),
            ]),
            scope,
        )
        .await?;
        let value = response_json(response, scope).await?;
        let token = value["access_token"]
            .as_str()
            .filter(|token| !token.is_empty() && token.len() <= 16384)
            .ok_or_else(|| "Google did not return a valid access token.".to_string())?;
        if value["token_type"]
            .as_str()
            .is_none_or(|kind| !kind.eq_ignore_ascii_case("Bearer"))
        {
            return Err("Google returned an unsupported access token.".into());
        }
        Ok(token.into())
    }

    async fn upload(
        &self,
        token: &str,
        release: &AndroidReleaseResult,
        mut file: fs::File,
        scope: &OperationScope,
        progress: impl Fn(String),
    ) -> Result<GooglePlayUploadResult, String> {
        let edits_url = format!(
            "{}/androidpublisher/v3/applications/{}/edits",
            self.origin, release.application_id
        );
        progress("Preparing the Google Play edit…".into());
        let edit = response_json(
            request(
                self.http
                    .post(&edits_url)
                    .bearer_auth(token)
                    .json(&json!({})),
                scope,
            )
            .await?,
            scope,
        )
        .await?;
        let edit_id = edit["id"]
            .as_str()
            .filter(|id| valid_remote_id(id))
            .ok_or_else(|| "Google returned an invalid edit identifier.".to_string())?;
        let edit_url = format!("{edits_url}/{edit_id}");
        let result = self
            .upload_in_edit(token, release, &mut file, scope, &progress, &edit_url)
            .await;
        if result.is_err() {
            // Cleanup must still run after cancellation. A timeout/connection loss during commit
            // has an uncertain outcome; deleting only this edit cannot delete a committed release.
            let _ = self
                .http
                .delete(&edit_url)
                .bearer_auth(token)
                .timeout(Duration::from_secs(15))
                .send()
                .await;
        }
        result
    }

    async fn upload_in_edit(
        &self,
        token: &str,
        release: &AndroidReleaseResult,
        file: &mut fs::File,
        scope: &OperationScope,
        progress: &impl Fn(String),
        edit_url: &str,
    ) -> Result<GooglePlayUploadResult, String> {
        let tracks = response_json(
            request(
                self.http
                    .get(format!("{edit_url}/tracks"))
                    .bearer_auth(token),
                scope,
            )
            .await?,
            scope,
        )
        .await?;
        let (track_name, releases) = internal_track(&tracks)?;
        let aab = release
            .aab
            .as_ref()
            .ok_or_else(|| "The retained release has no AAB.".to_string())?;
        let upload_url = format!(
            "{}/upload{}",
            self.origin,
            edit_url.strip_prefix(&self.origin).expect("fixed origin")
        );
        let upload_url = format!("{upload_url}/bundles");
        progress("Uploading the reviewed Android App Bundle…".into());
        let started = request(
            self.http
                .post(&upload_url)
                .query(&[("uploadType", "resumable")])
                .bearer_auth(token)
                .header("X-Upload-Content-Type", "application/octet-stream")
                .header("X-Upload-Content-Length", aab.bytes)
                .body(Vec::new()),
            scope,
        )
        .await?;
        check_status(started.status())?;
        let session = started
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| "Google did not return an upload session.".to_string())?;
        let session = validated_session_url(session, &upload_url)?;
        let bundle = self
            .upload_chunks(token, file, aab.bytes, &session, scope, progress)
            .await?;
        if bundle["versionCode"].as_u64() != release.version_code.parse::<u64>().ok()
            || bundle["sha256"]
                .as_str()
                .is_none_or(|sha| !sha.eq_ignore_ascii_case(&aab.sha256))
        {
            return Err("Google's uploaded bundle does not match the reviewed version and checksum. The edit was not committed.".into());
        }
        let draft = json!({"versionCodes": [release.version_code], "status": "draft"});
        let mut next_releases = releases;
        next_releases.push(draft.clone());
        let next_track = json!({"track": track_name, "releases": next_releases});
        progress("Saving an internal-testing draft…".into());
        let updated = response_json(
            request(
                self.http
                    .put(format!("{edit_url}/tracks/{track_name}"))
                    .bearer_auth(token)
                    .json(&next_track),
                scope,
            )
            .await?,
            scope,
        )
        .await?;
        verify_draft_track(&updated, &track_name, &next_releases, &release.version_code)?;
        check_cancelled(scope)?;
        // Do not interrupt a commit once sent: its outcome matters even if Stop is clicked.
        // Never retry a commit automatically, or fall back to submitting/cancelling reviews.
        let committed = self.http.post(format!("{edit_url}:commit")).bearer_auth(token)
            .query(&[("changesNotSentForReview", "true"), ("changesInReviewBehavior", "ERROR_IF_IN_REVIEW")])
            .send().await.map_err(|_| "Google did not confirm the draft commit. Check Play Console before uploading again.".to_string())?;
        check_status(committed.status())?;
        progress("Uploaded to an internal-testing draft. Review it in Google Play Console.".into());
        Ok(GooglePlayUploadResult {
            package_name: release.application_id.clone(),
            version_code: release.version_code.clone(),
            track: track_name,
            status: "draft".into(),
            sha256: aab.sha256.clone(),
        })
    }

    async fn version_codes(
        &self,
        token: &str,
        package_name: &str,
        scope: &OperationScope,
    ) -> Result<Vec<PlayVersionCode>, String> {
        let edits_url = format!(
            "{}/androidpublisher/v3/applications/{}/edits",
            self.origin, package_name
        );
        let edit = response_json(
            request(
                self.http
                    .post(&edits_url)
                    .bearer_auth(token)
                    .json(&json!({})),
                scope,
            )
            .await?,
            scope,
        )
        .await?;
        let edit_id = edit["id"]
            .as_str()
            .filter(|id| valid_remote_id(id))
            .ok_or_else(|| "Google returned an invalid edit identifier.".to_string())?;
        let edit_url = format!("{edits_url}/{edit_id}");
        let listed = self.version_codes_in_edit(token, &edit_url, scope).await;
        // A read needs no commit; the edit goes whichever way the read went.
        let _ = self
            .http
            .delete(&edit_url)
            .bearer_auth(token)
            .timeout(Duration::from_secs(15))
            .send()
            .await;
        listed
    }

    async fn version_codes_in_edit(
        &self,
        token: &str,
        edit_url: &str,
        scope: &OperationScope,
    ) -> Result<Vec<PlayVersionCode>, String> {
        let mut codes: Vec<PlayVersionCode> = Vec::new();
        let mut note = |code: u64, name: Option<String>| match codes
            .iter_mut()
            .find(|known| known.code == code)
        {
            Some(known) => {
                if known.release_name.is_none() {
                    known.release_name = name;
                }
            }
            None => codes.push(PlayVersionCode {
                code,
                release_name: name,
            }),
        };
        for resource in ["bundles", "apks"] {
            let value = response_json(
                request(
                    self.http
                        .get(format!("{edit_url}/{resource}"))
                        .bearer_auth(token),
                    scope,
                )
                .await?,
                scope,
            )
            .await?;
            for item in value[resource].as_array().into_iter().flatten() {
                if let Some(code) = item["versionCode"].as_u64() {
                    note(code, None);
                }
            }
        }
        let tracks = response_json(
            request(
                self.http
                    .get(format!("{edit_url}/tracks"))
                    .bearer_auth(token),
                scope,
            )
            .await?,
            scope,
        )
        .await?;
        for track in tracks["tracks"].as_array().into_iter().flatten() {
            for release in track["releases"].as_array().into_iter().flatten() {
                let name = release["name"].as_str().map(str::to_string);
                for code in release["versionCodes"].as_array().into_iter().flatten() {
                    let code = code
                        .as_u64()
                        .or_else(|| code.as_str().and_then(|code| code.parse().ok()));
                    if let Some(code) = code {
                        note(code, name.clone());
                    }
                }
            }
        }
        Ok(codes)
    }

    async fn upload_chunks(
        &self,
        token: &str,
        file: &mut fs::File,
        bytes: u64,
        session: &Url,
        scope: &OperationScope,
        progress: &impl Fn(String),
    ) -> Result<Value, String> {
        let mut offset = 0;
        let mut retries = 0;
        loop {
            check_cancelled(scope)?;
            let count = (bytes - offset).min(UPLOAD_CHUNK_BYTES as u64) as usize;
            if count == 0 {
                return Err("Google did not confirm the completed AAB upload.".into());
            }
            let mut chunk = vec![0; count];
            file.seek(SeekFrom::Start(offset))
                .and_then(|_| file.read_exact(&mut chunk))
                .map_err(|_| {
                    "The retained AAB changed or could not be read during upload.".to_string()
                })?;
            let end = offset + count as u64;
            let sent = request(
                self.http
                    .put(session.clone())
                    .bearer_auth(token)
                    .header("Content-Type", "application/octet-stream")
                    .header(
                        "Content-Range",
                        format!("bytes {offset}-{}/{bytes}", end - 1),
                    )
                    .body(chunk),
                scope,
            )
            .await;
            let response = match sent {
                Ok(response) if !response.status().is_server_error() => response,
                _ => {
                    check_cancelled(scope)?;
                    if retries >= 3 {
                        return Err("The Google Play upload was interrupted. Try again when the connection is stable.".into());
                    }
                    retries += 1;
                    cancellable(tokio::time::sleep(self.retry_delay * (1 << retries)), scope)
                        .await?;
                    request(
                        self.http
                            .put(session.clone())
                            .bearer_auth(token)
                            .header("Content-Range", format!("bytes */{bytes}"))
                            .body(Vec::new()),
                        scope,
                    )
                    .await?
                }
            };
            if response.status().is_success() {
                // A resumed status query may report that the final chunk already completed.
                if end != bytes {
                    return Err(
                        "Google completed the upload before all reviewed bytes were sent.".into(),
                    );
                }
                return response_json(response, scope).await;
            }
            if response.status() != StatusCode::PERMANENT_REDIRECT {
                check_status(response.status())?;
            }
            let next = acknowledged_bytes(&response)?;
            if next < offset || next > end || next >= bytes {
                return Err("Google returned an invalid upload byte range.".into());
            }
            if next == offset {
                retries += 1;
                if retries > 3 {
                    return Err(
                        "Google stopped accepting upload bytes. Try uploading again.".into(),
                    );
                }
            } else {
                // Bytes landed, so the link is working: the budget is for a stall, not for
                // the total number of hiccups a large bundle meets on its way up.
                retries = 0;
            }
            offset = next;
            progress(format!(
                "Uploading Android App Bundle: {}%",
                offset * 100 / bytes
            ));
        }
    }
}

fn internal_track(value: &Value) -> Result<(String, Vec<Value>), String> {
    let tracks = value["tracks"]
        .as_array()
        .ok_or_else(|| "Google returned unreadable release tracks.".to_string())?;
    let candidates: Vec<_> = tracks
        .iter()
        .filter(|track| matches!(track["track"].as_str(), Some("internal" | "qa")))
        .collect();
    if candidates.len() != 1 {
        return Err("Google Play did not identify one internal-testing track. Set up internal testing in Play Console first.".into());
    }
    let track = candidates[0];
    let releases = match track.get("releases") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or_else(|| "Google returned unreadable internal releases.".to_string())?
            .clone(),
    };
    if releases
        .iter()
        .any(|release| release["status"] != "completed")
    {
        return Err("Internal testing already has a draft or unfinished release. Review it in Play Console before uploading another draft.".into());
    }
    Ok((
        track["track"].as_str().expect("matched track").into(),
        releases,
    ))
}

fn verify_draft_track(
    value: &Value,
    track: &str,
    expected: &[Value],
    version: &str,
) -> Result<(), String> {
    let actual = value["releases"].as_array();
    let valid = value["track"] == track
        && actual.is_some_and(|actual| {
            actual.len() == expected.len()
                && expected[..expected.len() - 1]
                    .iter()
                    .all(|release| actual.contains(release))
                && actual
                    .iter()
                    .filter(|release| {
                        release["status"] == "draft" && release["versionCodes"] == json!([version])
                    })
                    .count()
                    == 1
        });
    if !valid {
        return Err("Google did not confirm the draft while preserving existing releases. The edit was not committed.".into());
    }
    Ok(())
}

fn valid_remote_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn validated_session_url(value: &str, upload_url: &str) -> Result<Url, String> {
    let invalid = || "Google returned an untrusted upload destination.".to_string();
    let url = Url::parse(value).map_err(|_| invalid())?;
    let expected = Url::parse(upload_url).map_err(|_| invalid())?;
    if url.origin() != expected.origin()
        || url.path() != expected.path()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !url
            .query_pairs()
            .any(|(name, value)| name == "upload_id" && !value.is_empty())
    {
        return Err(invalid());
    }
    Ok(url)
}

fn acknowledged_bytes(response: &Response) -> Result<u64, String> {
    let Some(range) = response.headers().get(reqwest::header::RANGE) else {
        return Ok(0);
    };
    range
        .to_str()
        .ok()
        .and_then(|value| value.strip_prefix("bytes=0-"))
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|last| last.checked_add(1))
        .ok_or_else(|| "Google returned an invalid upload byte range.".to_string())
}

fn check_cancelled(scope: &OperationScope) -> Result<(), String> {
    if scope.is_cancelled() {
        Err(CANCELLED_MESSAGE.into())
    } else {
        Ok(())
    }
}

async fn cancellable<T>(
    future: impl Future<Output = T>,
    scope: &OperationScope,
) -> Result<T, String> {
    check_cancelled(scope)?;
    tokio::pin!(future);
    loop {
        tokio::select! {
            value = &mut future => return Ok(value),
            _ = tokio::time::sleep(Duration::from_millis(100)) => check_cancelled(scope)?,
        }
    }
}

async fn request(builder: RequestBuilder, scope: &OperationScope) -> Result<Response, String> {
    cancellable(builder.send(), scope)
        .await?
        .map_err(|_| "Could not reach Google Play. Check the host connection and try again.".into())
}

async fn response_json(mut response: Response, scope: &OperationScope) -> Result<Value, String> {
    check_status(response.status())?;
    let mut body = Vec::new();
    while let Some(chunk) = cancellable(response.chunk(), scope)
        .await?
        .map_err(|_| "Google's response could not be read.".to_string())?
    {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err("Google returned an oversized response.".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "Google returned an unreadable response.".into())
}

fn check_status(status: StatusCode) -> Result<(), String> {
    if status.is_success() {
        return Ok(());
    }
    // Never echo remote error bodies or reqwest errors: both may contain keys, tokens or URLs.
    let guidance = match status {
        StatusCode::UNAUTHORIZED => {
            "Google rejected the service-account credentials. Check the key and host clock."
        }
        StatusCode::FORBIDDEN => {
            "Google Play denied access. Enable the Google Play Android Developer API and grant this service account access to this app and testing releases in Play Console."
        }
        StatusCode::NOT_FOUND => {
            "Google Play could not find the app or edit. Create the app and complete its initial Play Console setup first."
        }
        StatusCode::BAD_REQUEST => {
            "Google Play rejected the upload or draft. Check Play Console for the upload key, version code, app setup, and any release already in review."
        }
        StatusCode::CONFLICT => {
            "Google Play changed during this upload or already has this version. Check Play Console before uploading again."
        }
        StatusCode::TOO_MANY_REQUESTS => {
            "Google Play temporarily limited requests. Wait before uploading again."
        }
        _ => "Google Play could not complete the upload. Check Play Console and try again.",
    };
    Err(format!("{guidance} (HTTP {})", status.as_u16()))
}

#[cfg(test)]
#[path = "google_play_tests.rs"]
mod tests;
