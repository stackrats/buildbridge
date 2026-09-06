//! Exercise the real runner HTTP client against a local service, without a new test runtime
//! dependency in the transport crate or access to any actual control plane.

use super::*;
use buildbridge_contract::CreateArtifactRequest;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::AtomicU64;
use std::thread;
use std::time::Duration;

const CHUNK: usize = 4 * 1024 * 1024;
static NEXT_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &TcpStream) -> Request {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap().to_string();
    let path = parts.next().unwrap().to_string();
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
    }
    let length = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    assert!(length <= CHUNK, "the client sent an unbounded chunk");
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    Request {
        method,
        path,
        headers,
        body,
    }
}

struct Server {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn start(reply: impl Fn(usize, &Request) -> (u16, Value) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stored = Arc::clone(&requests);
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopped);
        let worker = thread::spawn(move || {
            let mut index = 0;
            while !stop.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("test service failed: {error}"),
                };
                let request = read_request(&stream);
                let (status, response) = reply(index, &request);
                stored.lock().unwrap().push(request);
                index += 1;
                // A status of zero is the connection going away before any answer.
                if status == 0 {
                    drop(stream);
                    continue;
                }
                let body = serde_json::to_vec(&response).unwrap();
                let header = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        Self {
            url,
            requests,
            stopped,
            worker: Some(worker),
        }
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

struct ArtifactFile {
    path: PathBuf,
    input: CreateArtifactRequest,
}

impl ArtifactFile {
    fn new(bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "buildbridge-upload-test-{}-{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::AcqRel)
        ));
        fs::write(&path, bytes).unwrap();
        Self {
            path,
            input: CreateArtifactRequest {
                name: "App.ipa".into(),
                bytes: bytes.len() as u64,
                sha256: "a".repeat(64),
            },
        }
    }
}

impl Drop for ArtifactFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn artifact(input: &CreateArtifactRequest, uploaded: u64, ready: bool) -> Value {
    json!({ "protocol_version": PROTOCOL_VERSION, "artifact": {
        "id": "artifact-1", "name": input.name, "bytes": input.bytes, "sha256": input.sha256,
        "uploaded_bytes": uploaded, "status": if ready { "ready" } else { "uploading" }, "download_url": null,
    } })
}

fn client(server: &Server) -> ApiClient {
    let claimed = ClaimedBuild {
        id: "build-1".into(),
        kind: BuildKind::AppleArchive,
        payload: json!({}),
        lease_expires_at: "2999-01-01T00:00:00Z".into(),
        next_log_sequence: 1,
        authorization: None,
        lease_token: Some("current-claim".into()),
    };
    ApiClient::new(&server.url, "runner-secret")
        .unwrap()
        .for_build(&claimed)
}

#[tokio::test]
async fn private_artifact_chunks_keep_the_claim_header_and_exact_offsets() {
    let contents = (0..CHUNK + 17)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let file = ArtifactFile::new(&contents);
    let input = file.input.clone();
    let server = Server::start(move |index, _request| {
        (
            200,
            match index {
                0 => artifact(&input, 0, false),
                1 => artifact(&input, CHUNK as u64, false),
                2 => artifact(&input, input.bytes, false),
                3 => artifact(&input, input.bytes, true),
                _ => panic!("unexpected extra request"),
            },
        )
    });
    let result = client(&server)
        .upload_artifact("build-1", &file.path, &file.input, || false)
        .await
        .unwrap();
    assert_eq!(result.status, "ready");
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    for request in &requests {
        assert_eq!(
            request.headers.get("x-build-lease").map(String::as_str),
            Some("current-claim")
        );
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer runner-secret")
        );
    }
    assert_eq!(requests[0].path, "/api/runner/builds/build-1/artifacts");
    assert_eq!(requests[0].method, "POST");
    assert!(!String::from_utf8_lossy(&requests[0].body).contains(file.path.to_str().unwrap()));
    assert_eq!(
        requests[1].path,
        "/api/runner/builds/build-1/artifacts/artifact-1?offset=0"
    );
    assert_eq!(requests[1].method, "PUT");
    assert_eq!(requests[1].body, contents[..CHUNK]);
    assert_eq!(
        requests[2].path,
        format!("/api/runner/builds/build-1/artifacts/artifact-1?offset={CHUNK}")
    );
    assert_eq!(requests[2].body, contents[CHUNK..]);
    assert_eq!(
        requests[3].path,
        "/api/runner/builds/build-1/artifacts/artifact-1/complete"
    );
}

#[tokio::test]
async fn resumed_upload_starts_at_the_verified_server_offset() {
    let file = ArtifactFile::new(b"abcdefgh");
    let input = file.input.clone();
    let server = Server::start(move |index, _| {
        (
            200,
            artifact(&input, if index == 0 { 3 } else { input.bytes }, index == 2),
        )
    });
    client(&server)
        .upload_artifact("build-1", &file.path, &file.input, || false)
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].path.ends_with("?offset=3"));
    assert_eq!(requests[1].body, b"defgh");
}

#[tokio::test]
async fn a_chunk_that_fails_once_is_resumed_from_the_offset_the_server_reports() {
    let contents = (0..CHUNK + 17)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let file = ArtifactFile::new(&contents);
    let input = file.input.clone();
    let server = Server::start(move |index, _| match index {
        0 => (200, artifact(&input, 0, false)),
        // The first chunk meets a server error and is asked about: nothing landed.
        1 => (503, json!({"message": "storage is busy"})),
        2 => (200, artifact(&input, 0, false)),
        3 => (200, artifact(&input, CHUNK as u64, false)),
        // The second chunk's connection drops after the bytes went out: they had landed.
        4 => (0, Value::Null),
        5 => (200, artifact(&input, input.bytes, false)),
        6 => (200, artifact(&input, input.bytes, true)),
        _ => panic!("unexpected extra request"),
    });
    let result = client(&server)
        .upload_artifact("build-1", &file.path, &file.input, || false)
        .await
        .unwrap();
    assert_eq!(result.status, "ready");
    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    let describe = |request: &Request| (request.method.clone(), request.path.clone());
    assert_eq!(
        requests.iter().map(describe).collect::<Vec<_>>(),
        [
            ("POST", "/api/runner/builds/build-1/artifacts"),
            (
                "PUT",
                "/api/runner/builds/build-1/artifacts/artifact-1?offset=0"
            ),
            ("POST", "/api/runner/builds/build-1/artifacts"),
            (
                "PUT",
                "/api/runner/builds/build-1/artifacts/artifact-1?offset=0"
            ),
            (
                "PUT",
                "/api/runner/builds/build-1/artifacts/artifact-1?offset=4194304"
            ),
            ("POST", "/api/runner/builds/build-1/artifacts"),
            (
                "POST",
                "/api/runner/builds/build-1/artifacts/artifact-1/complete"
            ),
        ]
        .map(|(method, path)| (method.to_string(), path.to_string()))
    );
    assert_eq!(requests[1].body, contents[..CHUNK]);
    assert_eq!(requests[3].body, contents[..CHUNK]);
    assert_eq!(requests[4].body, contents[CHUNK..]);
    for request in &requests {
        assert_eq!(
            request.headers.get("x-build-lease").map(String::as_str),
            Some("current-claim")
        );
    }
}

#[tokio::test]
async fn the_retry_budget_is_per_offset_and_a_stuck_offset_exhausts_it() {
    // Four failures before each of two chunks lands: eight in all, never five at one offset.
    let contents = vec![9_u8; CHUNK + 2];
    let file = ArtifactFile::new(&contents);
    let input = file.input.clone();
    let server = Server::start(move |index, request| {
        let uploaded = if index < 9 { 0 } else { CHUNK as u64 };
        match index {
            0 => (200, artifact(&input, 0, false)),
            9 => (200, artifact(&input, CHUNK as u64, false)),
            18 => (200, artifact(&input, input.bytes, false)),
            19 => (200, artifact(&input, input.bytes, true)),
            _ if request.method == "PUT" => (502, json!({"message": "bad gateway"})),
            _ => (200, artifact(&input, uploaded, false)),
        }
    });
    let result = client(&server)
        .upload_artifact("build-1", &file.path, &file.input, || false)
        .await
        .unwrap();
    assert_eq!(result.status, "ready");
    let requests = server.requests();
    assert_eq!(requests.len(), 20);
    assert_eq!(
        requests.iter().filter(|r| r.method == "PUT").count(),
        10,
        "five tries for each chunk"
    );
    assert!(requests[18].path.ends_with("?offset=4194304"));

    // The same server that never accepts a chunk is given up on after five tries at it.
    let file = ArtifactFile::new(b"abc");
    let input = file.input.clone();
    let server = Server::start(move |index, request| {
        if index == 0 || request.method == "POST" {
            (200, artifact(&input, 0, false))
        } else {
            (503, json!({"message": "storage is busy"}))
        }
    });
    let error = client(&server)
        .upload_artifact("build-1", &file.path, &file.input, || false)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("503"), "{error}");
    assert!(!error.to_string().contains(file.path.to_str().unwrap()));
    let requests = server.requests();
    assert_eq!(
        requests.iter().filter(|r| r.method == "PUT").count(),
        5,
        "the budget is spent"
    );
    assert_eq!(
        requests.len(),
        1 + 5 + 4,
        "create, five chunks, four offset checks"
    );
}

#[tokio::test]
async fn upload_rejects_metadata_changes_and_artifact_id_swaps() {
    for (changed_request, key, value) in [
        (0, "sha256", json!("b".repeat(64))),
        (1, "id", json!("other-artifact")),
        (2, "id", json!("other-artifact")),
    ] {
        let file = ArtifactFile::new(b"signed file");
        let input = file.input.clone();
        let server = Server::start(move |index, _| {
            let mut response =
                artifact(&input, if index == 0 { 0 } else { input.bytes }, index == 2);
            if index == changed_request {
                response["artifact"][key] = value.clone();
            }
            (200, response)
        });
        assert!(
            client(&server)
                .upload_artifact("build-1", &file.path, &file.input, || false)
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), changed_request + 1);
    }
}

#[tokio::test]
async fn local_revocation_stops_before_the_next_artifact_chunk() {
    let file = ArtifactFile::new(&vec![7; CHUNK + 1]);
    let input = file.input.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let remote_cancel = Arc::clone(&cancelled);
    let server = Server::start(move |index, _| {
        if index == 1 {
            remote_cancel.store(true, Ordering::Release);
        }
        (
            200,
            artifact(&input, if index == 0 { 0 } else { CHUNK as u64 }, false),
        )
    });
    let error = client(&server)
        .upload_artifact("build-1", &file.path, &file.input, || {
            cancelled.load(Ordering::Acquire)
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("stopped"));
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn expired_or_revoked_server_lease_prevents_further_upload_requests() {
    for rejected_status in [403, 409] {
        let file = ArtifactFile::new(&vec![7; CHUNK + 1]);
        let input = file.input.clone();
        let server = Server::start(move |index, _| {
            if index == 0 {
                (200, artifact(&input, 0, false))
            } else {
                (
                    rejected_status,
                    json!({"message":"the build lease is no longer valid"}),
                )
            }
        });
        let error = client(&server)
            .upload_artifact("build-1", &file.path, &file.input, || false)
            .await
            .unwrap_err();
        assert!(error.to_string().contains(&rejected_status.to_string()));
        assert_eq!(server.requests().len(), 2);
    }
}

#[tokio::test]
async fn local_artifact_checks_fail_before_any_request_without_naming_the_file() {
    let file = ArtifactFile::new(b"retained");
    let server = Server::start(|_, _| (500, json!({})));
    let client = client(&server);
    let missing = file.path.with_extension("missing");
    let directory = file.path.parent().unwrap().to_path_buf();
    let oversized = 2 * 1024 * 1024 * 1024 + 1;
    let cases: [(&str, &PathBuf, CreateArtifactRequest, bool); 6] = [
        ("upload stopped", &file.path, file.input.clone(), true),
        (
            "between 1 byte and 2 GiB",
            &file.path,
            CreateArtifactRequest {
                bytes: 0,
                ..file.input.clone()
            },
            false,
        ),
        (
            "between 1 byte and 2 GiB",
            &file.path,
            CreateArtifactRequest {
                bytes: oversized,
                ..file.input.clone()
            },
            false,
        ),
        ("cannot be opened", &missing, file.input.clone(), false),
        (
            "changed since the build finished",
            &file.path,
            CreateArtifactRequest {
                bytes: 9,
                ..file.input.clone()
            },
            false,
        ),
        (
            "changed since the build finished",
            &directory,
            file.input.clone(),
            false,
        ),
    ];
    for (expected, path, input, cancelled) in cases {
        let error = client
            .upload_artifact("build-1", path, &input, || cancelled)
            .await
            .expect_err(expected);
        for shown in [error.to_string(), format!("{error:?}")] {
            assert!(shown.contains(expected), "{shown}");
            assert!(!shown.contains(file.path.to_str().unwrap()), "{shown}");
            assert!(!shown.contains(directory.to_str().unwrap()), "{shown}");
            assert!(!shown.contains("runner-secret"), "{shown}");
            assert!(!shown.contains("current-claim"), "{shown}");
        }
    }
    assert!(
        server.requests().is_empty(),
        "the local checks run before any request"
    );
}

#[tokio::test]
async fn chunk_boundaries_send_one_request_per_chunk_and_a_stalled_server_stops_the_upload() {
    for contents in [vec![7_u8; CHUNK], b"x".to_vec()] {
        let file = ArtifactFile::new(&contents);
        let input = file.input.clone();
        let server = Server::start(move |index, _| {
            (
                200,
                artifact(&input, if index == 0 { 0 } else { input.bytes }, index == 2),
            )
        });
        client(&server)
            .upload_artifact("build-1", &file.path, &file.input, || false)
            .await
            .unwrap();
        let requests = server.requests();
        assert_eq!(requests.len(), 3, "create, one chunk, complete");
        assert!(requests[1].path.ends_with("?offset=0"));
        assert_eq!(requests[1].body, contents);
        assert!(requests[2].path.ends_with("/complete"));
    }

    let file = ArtifactFile::new(b"abcdefgh");
    let input = file.input.clone();
    let server = Server::start(move |_, _| (200, artifact(&input, 0, false)));
    let error = client(&server)
        .upload_artifact("build-1", &file.path, &file.input, || false)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("unexpected upload offset"),
        "{error}"
    );
    assert_eq!(
        server.requests().len(),
        2,
        "one chunk is sent, then the stall is refused"
    );

    let input = file.input.clone();
    let server = Server::start(move |index, _| {
        (
            200,
            artifact(&input, if index == 0 { 0 } else { input.bytes }, false),
        )
    });
    let error = client(&server)
        .upload_artifact("build-1", &file.path, &file.input, || false)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("could not verify the artifact"),
        "{error}"
    );
    assert_eq!(server.requests().len(), 3);
}
