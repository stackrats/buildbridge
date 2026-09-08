use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

const SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const TOKEN: &str = "test-access-token-that-must-not-be-displayed";

#[derive(Clone, Debug)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Reply {
    fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            headers: vec![],
            body,
        }
    }
}

struct Server {
    origin: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn start(reply: impl Fn(&str, usize, &RecordedRequest) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server_origin = origin.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let saved = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut index = 0;
            while !stopped.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                // macOS and the BSDs give an accepted socket the listener's non-blocking
                // flag; Linux always hands back a blocking one. Without this the first read
                // returns WouldBlock before the client has sent anything, and the read
                // timeout below would never apply either.
                stream.set_nonblocking(false).unwrap();
                let request = read_request(&stream);
                let response = reply(&server_origin, index, &request);
                saved.lock().unwrap().push(request);
                index += 1;
                let body = serde_json::to_vec(&response.body).unwrap();
                let mut head = format!(
                    "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n",
                    response.status,
                    body.len()
                );
                for (name, value) in response.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        Self {
            origin,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn client(&self) -> PlayClient {
        PlayClient {
            http: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            origin: self.origin.clone(),
            token_url: format!("{}/token", self.origin),
            retry_delay: Duration::from_millis(1),
        }
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn read_request(stream: &TcpStream) -> RecordedRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut first = line.split_whitespace();
    let method = first.next().unwrap().to_string();
    let path = first.next().unwrap().to_string();
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line.trim().is_empty() {
            break;
        }
        let (key, value) = line.split_once(':').unwrap();
        headers.insert(key.to_ascii_lowercase(), value.trim().into());
    }
    let length = headers
        .get("content-length")
        .and_then(|length: &String| length.parse().ok())
        .unwrap_or(0);
    assert!(length <= UPLOAD_CHUNK_BYTES);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    RecordedRequest {
        method,
        path,
        headers,
        body,
    }
}

struct Fixture {
    root: PathBuf,
    app: Engine,
    paths: MachinePaths,
    release: AndroidReleaseResult,
}

impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "buildbridge-google-play-{}-{nonce}",
            std::process::id()
        ));
        let app = Engine::new(EngineDeps {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            events: Arc::new(NoEvents),
        });
        let paths = MachinePaths::resolve(&app, "android-upload").unwrap();
        let directory = paths.artifacts_dir().join("release-test");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("app.aab");
        fs::write(&path, b"abc").unwrap();
        let release = AndroidReleaseResult {
            application_id: "com.example.app".into(),
            version_name: "1.0".into(),
            version_code: "42".into(),
            key_alias: "upload".into(),
            certificate_sha256: "a".repeat(64),
            aab: Some(AndroidArtifact {
                path: path.to_string_lossy().into(),
                bytes: 3,
                sha256: SHA256.into(),
            }),
            apk: None,
            output_tail: vec![],
        };
        Self {
            root,
            app,
            paths,
            release,
        }
    }

    fn file(&self) -> fs::File {
        fs::File::open(&self.release.aab.as_ref().unwrap().path).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn success_reply(origin: &str, index: usize, request: &RecordedRequest) -> Reply {
    match index {
        0 => Reply::json(200, json!({"id": "edit-123"})),
        1 => Reply::json(
            200,
            json!({"tracks": [{"track": "production", "releases": [{"status": "inProgress"}]},
            {"track": "internal", "releases": [{"status": "completed", "versionCodes": ["41"], "name": "Previous", "releaseNotes": [{"language": "en-US", "text": "Original"}]}]}]}),
        ),
        2 => Reply {
            status: 200,
            body: json!({}),
            headers: vec![(
                "Location".into(),
                format!(
                    "{origin}/upload/androidpublisher/v3/applications/com.example.app/edits/edit-123/bundles?uploadType=resumable&upload_id=session-secret"
                ),
            )],
        },
        3 => Reply::json(200, json!({"versionCode": 42, "sha256": SHA256})),
        4 => Reply::json(200, serde_json::from_slice(&request.body).unwrap()),
        5 => Reply::json(200, json!({"id": "edit-123"})),
        _ => Reply::json(204, json!({})),
    }
}

#[tokio::test]
async fn uploads_exact_bytes_and_commits_only_an_internal_draft_preserving_existing_releases() {
    let fixture = Fixture::new();
    let server = Server::start(success_reply);
    let result = server
        .client()
        .upload(
            TOKEN,
            &fixture.release,
            fixture.file(),
            &OperationScope::new(),
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(result.status, "draft");
    assert_eq!(result.track, "internal");
    assert_eq!(result.version_code, "42");
    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[3].method, "PUT");
    assert_eq!(requests[3].body, b"abc");
    assert_eq!(requests[3].headers["content-range"], "bytes 0-2/3");
    let updated: Value = serde_json::from_slice(&requests[4].body).unwrap();
    assert_eq!(updated["releases"][0]["versionCodes"], json!(["41"]));
    assert_eq!(
        updated["releases"][0]["releaseNotes"][0]["text"],
        "Original"
    );
    assert_eq!(
        updated["releases"][1],
        json!({"versionCodes": ["42"], "status": "draft"})
    );
    assert!(requests[5].path.ends_with(
        ":commit?changesNotSentForReview=true&changesInReviewBehavior=ERROR_IF_IN_REVIEW"
    ));
    assert!(requests[5].body.is_empty());
    assert!(
        requests
            .iter()
            .all(|request| request.headers["authorization"] == format!("Bearer {TOKEN}"))
    );
    assert!(requests.iter().all(|request| !request.path.contains(TOKEN)));
}

#[tokio::test]
async fn hash_or_version_mismatch_cleans_up_without_updating_or_committing() {
    for bundle in [
        json!({"versionCode": 43, "sha256": SHA256}),
        json!({"versionCode": 42, "sha256": "wrong"}),
    ] {
        let fixture = Fixture::new();
        let server = Server::start(move |origin, index, request| {
            if index == 3 {
                Reply::json(200, bundle.clone())
            } else if index == 4 {
                Reply::json(204, json!({}))
            } else {
                success_reply(origin, index, request)
            }
        });
        let error = server
            .client()
            .upload(
                TOKEN,
                &fixture.release,
                fixture.file(),
                &OperationScope::new(),
                |_| {},
            )
            .await
            .unwrap_err();
        assert!(error.contains("does not match"));
        let requests = server.requests();
        assert_eq!(requests.last().unwrap().method, "DELETE");
        assert!(
            !requests
                .iter()
                .any(|request| request.path.contains(":commit"))
        );
    }
}

#[tokio::test]
async fn refuses_existing_drafts_and_does_not_leak_remote_error_details() {
    for forbidden in [false, true] {
        let fixture = Fixture::new();
        let server = Server::start(move |origin, index, request| match index {
            1 if forbidden => Reply::json(
                403,
                json!({"error": {"message": format!("{TOKEN} PRIVATE KEY session-secret")}}),
            ),
            1 => Reply::json(
                200,
                json!({"tracks": [{"track": "internal", "releases": [{"status": "draft", "versionCodes": ["40"]}]}]}),
            ),
            2 => Reply::json(204, json!({})),
            _ => success_reply(origin, index, request),
        });
        let error = server
            .client()
            .upload(
                TOKEN,
                &fixture.release,
                fixture.file(),
                &OperationScope::new(),
                |_| {},
            )
            .await
            .unwrap_err();
        assert!(!error.contains(TOKEN));
        assert!(!error.contains("PRIVATE KEY"));
        assert!(!error.contains("session-secret"));
        let requests = server.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].method, "DELETE");
    }
}

#[tokio::test]
async fn untrusted_resumable_destination_is_not_contacted() {
    let fixture = Fixture::new();
    let server = Server::start(|origin, index, request| {
        if index == 2 {
            Reply {
                status: 200,
                headers: vec![(
                    "Location".into(),
                    "https://evil.example/upload?upload_id=steal".into(),
                )],
                body: json!({}),
            }
        } else if index == 3 {
            Reply::json(204, json!({}))
        } else {
            success_reply(origin, index, request)
        }
    });
    let error = server
        .client()
        .upload(
            TOKEN,
            &fixture.release,
            fixture.file(),
            &OperationScope::new(),
            |_| {},
        )
        .await
        .unwrap_err();
    assert!(error.contains("untrusted upload destination"));
    assert_eq!(server.requests().last().unwrap().method, "DELETE");
}

#[tokio::test]
async fn cancellation_after_edit_creation_deletes_edit_without_uploading() {
    let fixture = Fixture::new();
    let scope = OperationScope::new();
    let stop = Arc::clone(&scope);
    let server = Server::start(move |origin, index, request| {
        if index == 1 {
            stop.cancel();
        }
        if request.method == "DELETE" {
            Reply::json(204, json!({}))
        } else {
            success_reply(origin, index, request)
        }
    });
    let error = server
        .client()
        .upload(TOKEN, &fixture.release, fixture.file(), &scope, |_| {})
        .await
        .unwrap_err();
    assert_eq!(error, CANCELLED_MESSAGE);
    assert_eq!(server.requests().last().unwrap().method, "DELETE");
    assert!(
        !server
            .requests()
            .iter()
            .any(|request| request.path.contains(":commit"))
    );
}

#[tokio::test]
async fn chunked_upload_uses_acknowledged_ranges_and_bounded_requests() {
    let fixture = Fixture::new();
    let path = &fixture.release.aab.as_ref().unwrap().path;
    let size = UPLOAD_CHUNK_BYTES + 3;
    fs::write(path, vec![b'x'; size]).unwrap();
    let server = Server::start(move |_, index, _| {
        if index == 0 {
            Reply {
                status: 308,
                headers: vec![(
                    "Range".into(),
                    format!("bytes=0-{}", UPLOAD_CHUNK_BYTES - 1),
                )],
                body: json!({}),
            }
        } else {
            Reply::json(200, json!({"versionCode": 42, "sha256": SHA256}))
        }
    });
    let session = Url::parse(&format!("{}/session", server.origin)).unwrap();
    server
        .client()
        .upload_chunks(
            TOKEN,
            &mut fixture.file(),
            size as u64,
            &session,
            &OperationScope::new(),
            &|_| {},
        )
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].body.len(), UPLOAD_CHUNK_BYTES);
    assert_eq!(requests[1].body, b"xxx");
    assert_eq!(
        requests[1].headers["content-range"],
        format!("bytes {}-{}/{size}", UPLOAD_CHUNK_BYTES, size - 1)
    );
}

#[test]
fn retained_artifacts_require_the_reviewed_hash_size_and_managed_aab_path() {
    let mut fixture = Fixture::new();
    let path = reviewed_aab_path(&fixture.paths, &fixture.release, SHA256).unwrap();
    verify_aab(&path, 3, SHA256).unwrap();
    assert!(reviewed_aab_path(&fixture.paths, &fixture.release, &"a".repeat(64)).is_err());
    assert!(verify_aab(&path, 2, SHA256).is_err());
    fs::write(&path, b"abd").unwrap();
    assert!(
        verify_aab(&path, 3, SHA256)
            .unwrap_err()
            .contains("checksum changed")
    );
    let outside = fixture.root.join("outside.aab");
    fs::write(&outside, b"abc").unwrap();
    fixture.release.aab.as_mut().unwrap().path = outside.to_string_lossy().into_owned();
    assert!(reviewed_aab_path(&fixture.paths, &fixture.release, SHA256).is_err());
    fixture.release.aab = None;
    assert!(reviewed_aab_path(&fixture.paths, &fixture.release, SHA256).is_err());
}

#[test]
fn track_selection_accepts_documented_internal_identifiers_and_refuses_ambiguity_or_rollouts() {
    for name in ["internal", "qa"] {
        let track = internal_track(&json!({"tracks": [{"track": name}]})).unwrap();
        assert_eq!(track.0, name);
        for status in ["draft", "inProgress", "halted", "unknown"] {
            assert!(
                internal_track(
                    &json!({"tracks": [{"track": name, "releases": [{"status": status}]}]})
                )
                .is_err()
            );
        }
    }
    assert!(internal_track(&json!({"tracks": [{"track": "internal"}, {"track": "qa"}]})).is_err());
    assert!(internal_track(&json!({"tracks": [{"track": "production"}]})).is_err());
}

#[test]
fn unsafe_package_names_and_upload_destinations_are_refused() {
    for value in ["com.example.app", "com.example.App_1"] {
        assert!(valid_package_name(value));
    }
    for value in [
        "app",
        "../app",
        "com.app?token=secret",
        "com..app",
        "com.1app",
        "com.app/other",
    ] {
        assert!(!valid_package_name(value));
    }
    let expected = "https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications/com.example.app/edits/123/bundles";
    assert!(validated_session_url(&format!("{expected}?upload_id=abc"), expected).is_ok());
    for value in [
        expected.into(),
        expected.replace("https:", "http:") + "?upload_id=x",
        expected.replace("googleapis.com", "googleapis.com.evil.example") + "?upload_id=x",
        expected.replace("/bundles", "/apks") + "?upload_id=x",
        format!("{expected}?upload_id=x#fragment"),
    ] {
        assert!(validated_session_url(&value, expected).is_err());
    }
}

#[test]
fn malformed_credentials_never_echo_private_material() {
    for encoded in [b"PRIVATE KEY SECRET".to_vec(), serde_json::to_vec(&json!({"type": "service_account", "client_email": "upload@example.iam.gserviceaccount.com", "project_id": "example", "private_key_id": "key123", "private_key": "PRIVATE KEY SECRET", "token_uri": "https://evil.example"})).unwrap()] {
        let error = parse_account(&encoded).err().unwrap();
        assert!(!error.contains("SECRET"));
        assert!(!error.contains("evil.example"));
    }
}

// Public, disposable test fixture. Never used with a Google account.
const TEST_RSA_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCUWZ3L87qiHbXS
cdUPOCyFGVAAHL5IRzcgHpMwILXo2c3aqLla3VS/lN4KuYDh4iPtgJtLT25H9gmH
ebxer/dFAcKXRfido1CsdXSFNhrhhSEewI8g1FF1O3ksxqPiHAlwCLqAJ8VbcGUG
pv4RTIPDDmuDUAaf1phQfU38Sc0pPdrRQksyXUCYLH4/cxN68hkMj7VPI2mjgyTE
xLR/wRrWaps4ftbufkHTw5bitamraFHhxO6km1hgtkIeHdq0PlJ3CODUv0ouSKYh
pRgkHEeXsu+CRJMtpzI6wdYo5fwi6AQ7PmquGQ+2jkmZDOizUjd7z3PjplbXQEQ7
+c+WaFW1AgMBAAECggEAFO/u+uV7vjymODzTnrHFasWPSmzEGExgxdo62MyND/+J
c6ZjfqUFPILEscvLmlXBz1fa0w6zWFmrr6CpSs3X/rfIYHRCKfFuReDP4KspsRGK
gp4XtUDx/DM4H28rmxJs7JB2zfaO/qBGyeEQs51SbVmgJ+jH/pAZcCa3NneuCndW
0eU0+Q27NkY4g9y52uFfQcAothw/80DO+bmA9dKIv+4hHC0fPYrGvJRWFe4gkm0w
QysCEo2/uXBpelOwHIyiuoBTupEocgHDbof1Uz3niYFt9pe6PFKLBtvzfF40XJNu
hMgvL1H1wURza9R6DZerl1YqndZtsgl3MyKSx/MdAwKBgQDNAVZAXdIomWv2dd2X
qkPEA2lSZjKba2KWkeOh4K39kfjeMH9zyuB/T04ZvDz6NGi4m4VzRQp0HnVyxjyj
V+OZYdQF71jtf34Qv/pZirBdXixq88cIJIQ/kiQSUn6rsriplZ+fJzJDV92HQNu9
gJZ+hWcOhtrDPGKUkoUDW/24+wKBgQC5QH+JoEFo5TU5I/UBYdr1MzxEJzOWwQ2F
ZYBV/hAYc9hEcnqmXMhxtD3W3066DMQoIlRQ8UcDeTPRW2r3nRkpcsgMu1VsW3ou
22wBHagecK/eTJmAdi1GkJC/Rpn/pAs7llY9gF2g0EWaDXduM+O5iybQ8YuAxYae
/1vOvwVNDwKBgQCEBIRi1whrlMFt6eFVthQFupysr5uPcsv+YtzQdjwVu1ck3t50
1wVTduK4t/wctHtrxttdq+xbcvH3g6mxFvw+3j0HxWbjKuMoLjkuSJ3iwq6gAXT+
zWVM+vO3yOBB+cnpi61LdJZtv7utShs4IgLIX2hKdpWSfOSPAPwfebIe2QKBgGUN
BsTm8ucqKHcr4wjG/S5FrXkrvRtd4WdDr9a4iMUd4/mqTLcU444KmLTuCL66GgIe
f8nLY0ZExfxMlrPNMR2H7BHt2jIKUELhFDAjokJAi96CADWvwRC96Qc9luF49Vui
rRZNQEVpdp4K/HvTuEPM4PaW29b5aG6wsr67OkQHAoGBAJOM9i8Ujt9KQlgWO4ZJ
szp2nPex7Ai9P9rhwNe1XSKAyGInHK3pg/t4NE72XOaUWXGLW7suxhQlWAX4nkbO
xByCKRMp5q2Nr6q2ExE6SIbozF3E+NBXEfiZaR1m228uz6k3JbUaO74xUjKOrN82
8vCBOdKXMp5ImRY4m42vOnWR
-----END PRIVATE KEY-----
"#;

fn test_account() -> ServiceAccount {
    let input = json!({"type": "service_account", "client_email": "upload@example.iam.gserviceaccount.com", "project_id": "example", "private_key_id": "key123", "private_key": TEST_RSA_KEY, "token_uri": "https://evil.example/token"});
    parse_account(&serde_json::to_vec(&input).unwrap()).unwrap()
}

#[test]
fn exported_service_account_retains_its_key_and_uses_google_auth_endpoints() {
    let account = test_account();
    let encoded = account.export_json().unwrap();
    let value: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["token_uri"], TOKEN_URL);
    assert_eq!(
        value["auth_uri"],
        "https://accounts.google.com/o/oauth2/auth"
    );
    assert!(
        !String::from_utf8(encoded.clone())
            .unwrap()
            .contains("evil.example")
    );
    let restored = parse_account(&encoded).unwrap();
    assert_eq!(restored.private_key, account.private_key);
    assert_eq!(restored.private_key_id, account.private_key_id);
    assert_eq!(restored.client_email, account.client_email);
    assert_eq!(restored.project_id, account.project_id);
    assert_eq!(
        account_assertion(&restored, 1).unwrap(),
        account_assertion(&account, 1).unwrap()
    );
}

#[tokio::test]
async fn authentication_uses_a_signed_google_scoped_assertion_and_returns_only_the_safe_summary() {
    use base64::Engine as _;
    let account = test_account();
    let encoded = serde_json::to_value(&account).unwrap();
    assert!(encoded.get("token_uri").is_none());
    let summary = serde_json::to_value(account.summary()).unwrap();
    assert_eq!(summary.as_object().unwrap().len(), 3);
    assert_eq!(
        summary["clientEmail"],
        "upload@example.iam.gserviceaccount.com"
    );
    assert!(!summary.to_string().contains("PRIVATE KEY"));
    let server = Server::start(|_, _, request| {
        assert_eq!(request.path, "/token");
        assert!(!request.headers.contains_key("authorization"));
        let form = Url::parse(&format!(
            "https://example.invalid/?{}",
            String::from_utf8(request.body.clone()).unwrap()
        ))
        .unwrap();
        let values: HashMap<_, _> = form
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert_eq!(
            values["grant_type"],
            "urn:ietf:params:oauth:grant-type:jwt-bearer"
        );
        let assertion = &values["assertion"];
        assert_eq!(
            jsonwebtoken::decode_header(assertion).unwrap().alg,
            Algorithm::RS256
        );
        let claims: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(assertion.split('.').nth(1).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["aud"], TOKEN_URL);
        assert_eq!(claims["scope"], SCOPE);
        assert_eq!(claims["iss"], "upload@example.iam.gserviceaccount.com");
        assert_eq!(
            claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap(),
            3600
        );
        assert!(!String::from_utf8_lossy(&request.body).contains("PRIVATE KEY"));
        Reply::json(200, json!({"access_token": TOKEN, "token_type": "Bearer"}))
    });
    assert_eq!(
        server
            .client()
            .authenticate(&account, &OperationScope::new())
            .await
            .unwrap(),
        TOKEN
    );
}

#[tokio::test]
async fn connection_marker_scopes_vault_keys_and_missing_marker_never_needs_the_vault() {
    let fixture = Fixture::new();
    let machine = StoredMachine {
        id: fixture.paths.id.clone(),
        config: MachineConfig {
            provider: MachineProvider::AndroidToolchain,
            ..MachineConfig::default()
        },
        created_at_epoch_seconds: 100,
        signing_kit_id: None,
        env_set_id: None,
        template_id: None,
    };
    let mut registry = machines::MachineRegistry {
        machines: vec![machine],
    };
    machines::save_registry(&fixture.app, &registry).unwrap();
    assert!(
        !google_play_connection(&fixture.app, fixture.paths.id.clone())
            .await
            .unwrap()
            .configured
    );
    remove_google_play_credentials(&fixture.app, &fixture.paths.id)
        .await
        .unwrap();
    let original = connection_account_id(&fixture.app, &fixture.paths.id, true)
        .unwrap()
        .unwrap();
    assert!(original.starts_with(fixture.app.data_dir().to_str().unwrap()));
    assert_eq!(
        connection_account_id(&fixture.app, &fixture.paths.id, false).unwrap(),
        Some(original.clone())
    );
    registry.machines[0].created_at_epoch_seconds = 101;
    machines::save_registry(&fixture.app, &registry).unwrap();
    assert!(
        connection_account_id(&fixture.app, &fixture.paths.id, false)
            .unwrap()
            .is_none()
    );
    assert!(
        !google_play_connection(&fixture.app, fixture.paths.id.clone())
            .await
            .unwrap()
            .configured
    );
    let replacement = connection_account_id(&fixture.app, &fixture.paths.id, true)
        .unwrap()
        .unwrap();
    assert_ne!(replacement, original);
}

#[tokio::test]
async fn a_torn_connection_marker_reads_as_not_connected_and_can_be_removed_and_replaced() {
    let fixture = Fixture::new();
    machines::save_registry(
        &fixture.app,
        &machines::MachineRegistry {
            machines: vec![StoredMachine {
                id: fixture.paths.id.clone(),
                config: MachineConfig {
                    provider: MachineProvider::AndroidToolchain,
                    ..MachineConfig::default()
                },
                created_at_epoch_seconds: 100,
                signing_kit_id: None,
                env_set_id: None,
                template_id: None,
            }],
        },
    )
    .unwrap();
    let path = marker_path(&fixture.app, &fixture.paths.id).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    // A write that stopped mid-way: neither field is whole, so nothing names a vault entry.
    fs::write(&path, b"{\"created_at\":1").unwrap();
    assert!(
        connection_account_id(&fixture.app, &fixture.paths.id, false)
            .unwrap()
            .is_none()
    );
    assert!(
        !google_play_connection(&fixture.app, fixture.paths.id.clone())
            .await
            .unwrap()
            .configured
    );
    assert!(recoverable_account_ids(&fixture.app, &fixture.paths.id, &path).is_empty());
    remove_google_play_credentials(&fixture.app, &fixture.paths.id)
        .await
        .unwrap();
    assert!(!path.exists(), "the torn marker is removed");
    let replacement = connection_account_id(&fixture.app, &fixture.paths.id, true)
        .unwrap()
        .expect("a fresh marker is created over a torn one");
    let marker: ConnectionMarker = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(marker.created_at, 100);
    assert_eq!(
        replacement,
        account_id_for(&fixture.app, &fixture.paths.id, 100, marker.nonce)
    );
    // Garbage in place of the marker is not a connection either, and creating over it works.
    fs::write(&path, b"not json at all").unwrap();
    assert!(
        connection_account_id(&fixture.app, &fixture.paths.id, false)
            .unwrap()
            .is_none()
    );
    assert!(
        connection_account_id(&fixture.app, &fixture.paths.id, true)
            .unwrap()
            .is_some()
    );
}

#[test]
fn removal_recovers_whatever_account_ids_a_marker_still_names() {
    let fixture = Fixture::new();
    let path = fixture.root.join("google-play-connection.json");
    fs::create_dir_all(&fixture.root).unwrap();
    let whole = ConnectionMarker {
        created_at: 100,
        nonce: 123_456_789,
    };
    fs::write(&path, serde_json::to_vec(&whole).unwrap()).unwrap();
    assert_eq!(
        recoverable_account_ids(&fixture.app, "droid", &path),
        vec![account_id_for(&fixture.app, "droid", 100, 123_456_789)]
    );
    // Torn after both fields: the entry it names can still be removed.
    fs::write(&path, b"{\"created_at\": 100, \"nonce\": 123456789").unwrap();
    assert_eq!(
        recoverable_account_ids(&fixture.app, "droid", &path),
        vec![account_id_for(&fixture.app, "droid", 100, 123_456_789)]
    );
    for hopeless in [
        &b"{\"created_at\":100,\"non"[..],
        b"{\"created_at\":\"100\",\"nonce\":\"1\"}",
        b"",
        b"\xff\xfe",
    ] {
        fs::write(&path, hopeless).unwrap();
        assert!(
            recoverable_account_ids(&fixture.app, "droid", &path).is_empty(),
            "{hopeless:?}"
        );
    }
    fs::remove_file(&path).unwrap();
    assert!(recoverable_account_ids(&fixture.app, "droid", &path).is_empty());
}

#[tokio::test]
async fn transient_failures_are_forgiven_once_bytes_land_so_a_large_bundle_finishes() {
    let fixture = Fixture::new();
    let bytes = 4;
    let file_path = fixture.release.aab.as_ref().unwrap().path.clone();
    fs::write(&file_path, b"abcd").unwrap();
    // Every chunk request fails once; every status query shows the bytes arrived anyway. Eight
    // failures in all, never more than two at one offset: a budget that only counts failures
    // in a row lets the upload finish where a total count would have given up.
    let server = Server::start(move |_, index, request| match index {
        0 | 2 | 4 | 6 => Reply::json(503, json!({"error": "transient"})),
        1 | 3 | 5 => {
            assert!(request.body.is_empty());
            assert_eq!(request.headers["content-range"], "bytes */4");
            Reply {
                status: 308,
                headers: vec![("Range".into(), format!("bytes=0-{}", index / 2))],
                body: json!({}),
            }
        }
        7 => Reply {
            status: 308,
            headers: vec![("Range".into(), "bytes=0-2".into())],
            body: json!({}),
        },
        _ => {
            assert_eq!(request.body, b"d");
            assert_eq!(request.headers["content-range"], "bytes 3-3/4");
            Reply::json(200, json!({"versionCode": 42, "sha256": SHA256}))
        }
    });
    let session = Url::parse(&format!("{}/session", server.origin)).unwrap();
    let mut file = fs::File::open(&file_path).unwrap();
    server
        .client()
        .upload_chunks(
            TOKEN,
            &mut file,
            bytes,
            &session,
            &OperationScope::new(),
            &|_| {},
        )
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 9);
    assert_eq!(requests[0].headers["content-range"], "bytes 0-3/4");
    assert_eq!(requests[2].headers["content-range"], "bytes 1-3/4");
    assert_eq!(requests[4].headers["content-range"], "bytes 2-3/4");
    assert_eq!(requests[6].headers["content-range"], "bytes 3-3/4");

    // A stall, with no byte ever landing, still ends the upload after a few tries.
    let server = Server::start(|_, index, _| {
        if index.is_multiple_of(2) {
            Reply::json(503, json!({}))
        } else {
            Reply {
                status: 308,
                headers: vec![],
                body: json!({}),
            }
        }
    });
    let session = Url::parse(&format!("{}/session", server.origin)).unwrap();
    let mut file = fs::File::open(&file_path).unwrap();
    let error = server
        .client()
        .upload_chunks(
            TOKEN,
            &mut file,
            bytes,
            &session,
            &OperationScope::new(),
            &|_| {},
        )
        .await
        .unwrap_err();
    assert!(error.contains("stopped accepting upload bytes"), "{error}");
    assert_eq!(server.requests().len(), 4);
}

#[tokio::test]
async fn interrupted_upload_queries_the_session_and_resumes_from_the_acknowledged_byte() {
    let fixture = Fixture::new();
    let server = Server::start(|_, index, request| match index {
        0 => Reply::json(503, json!({"error": "do not echo this"})),
        1 => {
            assert!(request.body.is_empty());
            assert_eq!(request.headers["content-range"], "bytes */3");
            Reply {
                status: 308,
                headers: vec![("Range".into(), "bytes=0-1".into())],
                body: json!({}),
            }
        }
        _ => {
            assert_eq!(request.body, b"c");
            assert_eq!(request.headers["content-range"], "bytes 2-2/3");
            Reply::json(200, json!({"versionCode": 42, "sha256": SHA256}))
        }
    });
    let session = Url::parse(&format!("{}/session", server.origin)).unwrap();
    server
        .client()
        .upload_chunks(
            TOKEN,
            &mut fixture.file(),
            3,
            &session,
            &OperationScope::new(),
            &|_| {},
        )
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn http_failures_give_fixed_guidance_with_the_status_and_never_the_body() {
    for (status, needle) in [
        (401, "credentials"),
        (403, "denied access"),
        (404, "could not find"),
        (400, "rejected the upload"),
        (409, "already has this version"),
        (429, "temporarily limited"),
        (500, "could not complete"),
        (503, "could not complete"),
    ] {
        let error = check_status(StatusCode::from_u16(status).unwrap()).unwrap_err();
        assert!(error.contains(needle), "{status}: {error}");
        assert!(error.ends_with(&format!("(HTTP {status})")), "{error}");
    }
    for status in [200, 201, 204] {
        assert!(check_status(StatusCode::from_u16(status).unwrap()).is_ok());
    }
    assert!(check_status(StatusCode::PERMANENT_REDIRECT).is_err());
}

#[test]
fn remote_ids_and_version_codes_are_bounded_before_they_shape_a_request() {
    for id in ["edit-1", "AbC_09-", &"x".repeat(256)] {
        assert!(valid_remote_id(id), "{id}");
    }
    for id in [
        "",
        "..",
        "a/b",
        "a?b",
        "a%2e",
        "e dit",
        "ä",
        &"x".repeat(257),
    ] {
        assert!(!valid_remote_id(id), "{id:?}");
    }
    let mut fixture = Fixture::new();
    for code in ["0", "2100000001", "1.0", "", "-1", "4294967296", " 7"] {
        fixture.release.version_code = code.to_string();
        let error = reviewed_aab_path(&fixture.paths, &fixture.release, SHA256).unwrap_err();
        assert!(
            error.contains("invalid package name or version code"),
            "{code:?}: {error}"
        );
    }
    fixture.release.version_code = "2100000000".to_string();
    assert!(reviewed_aab_path(&fixture.paths, &fixture.release, SHA256).is_ok());
    fixture.release.application_id = "com.example.app;rm".to_string();
    assert!(
        reviewed_aab_path(&fixture.paths, &fixture.release, SHA256)
            .unwrap_err()
            .contains("invalid package name")
    );
}
