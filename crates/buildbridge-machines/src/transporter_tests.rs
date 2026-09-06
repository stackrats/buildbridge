use super::*;

const KEY_ID: &str = "A1B2C3D4E5";
const ISSUER_ID: &str = "11223344-5566-7788-99aa-bbccddeeff00";
const PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\ncredential_material_that_must_not_appear\n-----END PRIVATE KEY-----\n";
const IPA: &[u8] = b"retained IPA\0\nwith binary bytes\xff";

struct UploadFixture {
    directory: PathBuf,
    ipa: PathBuf,
    digest: String,
}

impl UploadFixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!(
            "buildbridge-upload-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let ipa = directory.join("retained ' $(shell) IPA.ipa");
        fs::write(&ipa, IPA).unwrap();
        let mock = directory.join("iTMSTransporter");
        fs::write(&mock, MOCK_TRANSPORTER).unwrap();
        fs::set_permissions(&mock, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            directory,
            digest: native_sha256(&ipa).unwrap(),
            ipa,
        }
    }

    fn command(&self, mode: &str) -> Command {
        let script = format!(
            "__name__ = 'test_helper'\n{TRANSPORTER_HELPER}\nfind_transporter = lambda: os.environ['TEST_TRANSPORTER']\nsys.exit(main())"
        );
        let mut command = Command::new("/usr/bin/python3");
        // Model SSH: stopping its host process closes the stream, while the
        // remote helper has its own session and must clean up independently.
        command.args(["-u", "-c", "import os, subprocess, sys; child = subprocess.Popen([sys.executable, '-u', '-c', os.environ['TEST_HELPER']], start_new_session=True); sys.exit(child.wait())"])
            .env("TEST_HELPER", script)
            .env("TEST_TRANSPORTER", self.directory.join("iTMSTransporter"))
            .env("TEST_REPORT", self.directory.join("report.json"))
            .env("TEST_MODE", mode);
        command
    }

    fn run(
        &self,
        mode: &str,
        digest: &str,
        timeout: Duration,
    ) -> (Result<(), ProviderError>, Vec<String>) {
        let mut progress = Vec::new();
        let result = run_upload(
            &mut self.command(mode),
            File::open(&self.ipa).unwrap(),
            IPA.len() as u64,
            [KEY_ID, ISSUER_ID, PRIVATE_KEY, digest],
            timeout,
            &mut |message| progress.push(message),
        );
        (result, progress)
    }

    /// The helper's report, waited for: a host timeout as short as the tests use can fire
    /// before a loaded machine has even started the helper's Python, which still writes the
    /// report on its own session's way out.
    fn report(&self) -> serde_json::Value {
        let path = self.directory.join("report.json");
        let started = Instant::now();
        loop {
            if let Ok(bytes) = fs::read(&path)
                && let Ok(report) = serde_json::from_slice(&bytes)
            {
                return report;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the helper never wrote its report: {path:?}"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn assert_cleaned(&self) {
        let report = self.report();
        let directory = Path::new(report["cwd"].as_str().unwrap());
        let started = Instant::now();
        while directory.exists() && started.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !directory.exists(),
            "guest upload directory was not cleaned: {directory:?}"
        );
        if let Some(child) = report["child"].as_u64() {
            let child = child.to_string();
            let state = |_: ()| {
                let output = Command::new("ps")
                    .args(["-o", "stat=", "-p", &child])
                    .output()
                    .unwrap();
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            };
            let gone = |state: &str| state.is_empty() || state.starts_with('Z');
            let mut current = state(());
            while !gone(&current) && started.elapsed() < Duration::from_secs(5) {
                thread::sleep(Duration::from_millis(50));
                current = state(());
            }
            assert!(
                gone(&current),
                "Transporter's child is still running: {current}"
            );
        }
    }
}

impl Drop for UploadFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

const MOCK_TRANSPORTER: &str = r#"#!/usr/bin/python3
import json, os, pathlib, signal, stat, subprocess, sys, time
assert sys.argv[1:4] == ['-m', 'upload', '-assetFile']
assert sys.argv[5:] == ['-apiKey', 'A1B2C3D4E5', '-apiIssuer', '11223344-5566-7788-99aa-bbccddeeff00', '-v', 'critical']
assert pathlib.Path(sys.argv[4]).read_bytes() == b'retained IPA\0\nwith binary bytes\xff'
key_path = pathlib.Path('private_keys/AuthKey_A1B2C3D4E5.p8')
assert stat.S_IMODE(key_path.stat().st_mode) == 0o600
assert stat.S_IMODE(key_path.parent.stat().st_mode) == 0o700
assert stat.S_IMODE(pathlib.Path.cwd().stat().st_mode) == 0o700
key = key_path.read_text()
assert key == '-----BEGIN PRIVATE KEY-----\ncredential_material_that_must_not_appear\n-----END PRIVATE KEY-----\n'
mode = os.environ['TEST_MODE']
descendant = None
if mode == 'hang':
    descendant = subprocess.Popen(['/bin/sleep', '30'])
pathlib.Path(os.environ['TEST_REPORT']).write_text(json.dumps({'cwd': str(pathlib.Path.cwd()), 'child': descendant.pid if descendant else None}))
print(key, flush=True)
if mode == 'authentication':
    print('Authentication failed: ' + key)
    sys.exit(1)
if mode == 'duplicate':
    print('ERROR ITMS-90189: Redundant Binary Upload ' + key)
    sys.exit(1)
if mode == 'rejected':
    print('ERROR ITMS-99999: rejected ' + key)
    sys.exit(1)
if mode == 'hang':
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    time.sleep(30)
"#;

#[test]
fn upload_rejects_ids_that_could_escape_the_private_key_directory() {
    for key_id in ["../KEY0000", "ABCDE'FGHI", "$(command)", "abcde12345"] {
        assert!(
            validate_upload_input(
                "buildbridge",
                &"a".repeat(64),
                key_id,
                ISSUER_ID,
                PRIVATE_KEY
            )
            .is_err()
        );
    }
    assert!(
        validate_upload_input(
            "buildbridge",
            &"a".repeat(64),
            KEY_ID,
            ISSUER_ID,
            PRIVATE_KEY
        )
        .is_ok()
    );
    assert!(!transporter_command().contains(PRIVATE_KEY));
    assert!(!transporter_command().contains(ISSUER_ID));
}

#[test]
fn payload_frames_preserve_multiline_credentials_and_exact_binary_ipa() {
    let mut payload = Vec::new();
    let mut ipa = IPA;
    write_upload_payload(
        &mut payload,
        &mut ipa,
        IPA.len() as u64,
        [KEY_ID, ISSUER_ID, PRIVATE_KEY, &"a".repeat(64)],
    )
    .unwrap();
    let mut input = payload.as_slice();
    for expected in [KEY_ID, ISSUER_ID, PRIVATE_KEY, &"a".repeat(64)] {
        let mut length = [0; 4];
        input.read_exact(&mut length).unwrap();
        let length = u32::from_be_bytes(length) as usize;
        let mut contents = vec![0; length];
        input.read_exact(&mut contents).unwrap();
        assert_eq!(contents, expected.as_bytes());
    }
    let mut length = [0; 8];
    input.read_exact(&mut length).unwrap();
    assert_eq!(u64::from_be_bytes(length), IPA.len() as u64);
    assert_eq!(input, IPA);
}

#[test]
fn guest_upload_uses_exact_argv_and_cleans_key_without_forwarding_tool_secrets() {
    let fixture = UploadFixture::new();
    let (result, progress) = fixture.run("success", &fixture.digest, Duration::from_secs(10));
    result.unwrap();
    assert!(
        progress
            .iter()
            .any(|message| message == TRANSPORTER_COMPLETED)
    );
    assert!(!progress.join("\n").contains("credential_material"));
    fixture.assert_cleaned();
}

#[test]
fn guest_upload_failure_is_actionable_redacted_and_cleans_key() {
    for (mode, expected) in [
        ("authentication", "could not authenticate"),
        ("duplicate", "already received"),
        ("rejected", "ITMS-99999"),
    ] {
        let fixture = UploadFixture::new();
        let (result, progress) = fixture.run(mode, &fixture.digest, Duration::from_secs(10));
        let error = result.unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("credential_material"));
        assert!(!progress.join("\n").contains("credential_material"));
        fixture.assert_cleaned();
    }
}

#[test]
fn changed_ipa_is_rejected_before_transporter_is_invoked() {
    let fixture = UploadFixture::new();
    let (result, _) = fixture.run("success", &"0".repeat(64), Duration::from_secs(10));
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("checksum does not match")
    );
    assert!(!fixture.directory.join("report.json").exists());
}

#[test]
fn guest_stops_transporter_and_cleans_key_when_the_operation_is_cancelled() {
    let fixture = UploadFixture::new();
    let scope = OperationScope::new();
    thread::scope(|threads| {
        let entered = scope.clone();
        let worker = threads.spawn(|| {
            let _operation = enter_operation(entered);
            fixture
                .run("hang", &fixture.digest, Duration::from_secs(15))
                .0
        });
        let start = Instant::now();
        while !fixture.directory.join("report.json").exists()
            && start.elapsed() < Duration::from_secs(5)
        {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            fixture.directory.join("report.json").exists(),
            "mock Transporter did not start"
        );
        scope.cancel();
        assert!(
            worker
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("stopped")
        );
    });
    fixture.assert_cleaned();
}

#[test]
fn guest_timeout_stops_transporter_and_cleans_key() {
    let fixture = UploadFixture::new();
    let mut command = fixture.command("hang");
    // Exercise the guest alarm independently of the host's longer deadline.
    let script = command
        .get_envs()
        .find(|(key, _)| *key == "TEST_HELPER")
        .unwrap()
        .1
        .unwrap()
        .to_string_lossy()
        .replace(
            "find_transporter = lambda:",
            "UPLOAD_TIMEOUT_SECONDS = 1\nfind_transporter = lambda:",
        );
    command.env("TEST_HELPER", script);
    let result = run_upload(
        &mut command,
        File::open(&fixture.ipa).unwrap(),
        IPA.len() as u64,
        [KEY_ID, ISSUER_ID, PRIVATE_KEY, &fixture.digest],
        Duration::from_secs(10),
        &mut |_| {},
    );
    let error = result.unwrap_err().to_string();
    assert!(error.contains("timed out"), "{error}");
    fixture.assert_cleaned();
}

#[test]
fn host_timeout_disconnects_guest_and_cleans_key() {
    let fixture = UploadFixture::new();
    let (result, _) = fixture.run("hang", &fixture.digest, Duration::from_millis(500));
    assert!(result.unwrap_err().to_string().contains("timed out"));
    fixture.assert_cleaned();
}

#[test]
fn missing_transporter_reports_the_required_guest_installation() {
    let fixture = UploadFixture::new();
    let mut command = fixture.command("success");
    // Exercise the real discovery paths while ensuring this test never uses an
    // installed Transporter, including when run on a developer's Mac.
    command.env("TEST_HELPER", format!("__name__ = 'test_helper'\n{TRANSPORTER_HELPER}\nos.path.isfile = lambda _: False\nsys.exit(main())"));
    let result = run_upload(
        &mut command,
        File::open(&fixture.ipa).unwrap(),
        IPA.len() as u64,
        [KEY_ID, ISSUER_ID, PRIVATE_KEY, &fixture.digest],
        Duration::from_secs(10),
        &mut |_| {},
    );
    let error = result.unwrap_err().to_string();
    assert!(error.contains("Install Apple's Transporter app"), "{error}");
    assert!(!fixture.directory.join("report.json").exists());
}

#[test]
fn only_bounded_helper_progress_lines_reach_the_host() {
    let secret = "AuthKey material that must never reach a log";
    let mut stream = format!(
        "{secret}\n{TRANSPORTER_PREFIX}uploading 10%\n  {TRANSPORTER_PREFIX}uploading 50%  \nTransporter: {secret}\n"
    )
    .into_bytes();
    stream.extend_from_slice(b"\xff binary noise\n");
    let widest = format!(
        "{TRANSPORTER_PREFIX}{}",
        "y".repeat(4096 - TRANSPORTER_PREFIX.len() - 1)
    );
    stream.extend_from_slice(format!("{widest}\n{TRANSPORTER_PREFIX}done").as_bytes());
    let (sender, receiver) = std::sync::mpsc::channel();
    read_upload_output(stream.as_slice(), sender);
    let messages = receiver
        .iter()
        .map(|event| match event {
            UploadEvent::Output(message) => message,
            UploadEvent::Transfer(_) => panic!("only output events are read from the stream"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        messages,
        [
            "uploading 10%",
            "uploading 50%",
            &widest[TRANSPORTER_PREFIX.len()..],
            "done"
        ]
    );
    assert!(!messages.iter().any(|message| message.contains(secret)));

    let oversized = format!(
        "{TRANSPORTER_PREFIX}{}\n{TRANSPORTER_PREFIX}after\n",
        "z".repeat(4_096)
    );
    let (sender, receiver) = std::sync::mpsc::channel();
    read_upload_output(oversized.as_bytes(), sender);
    assert_eq!(
        receiver.iter().count(),
        0,
        "an unbounded line abandons the stream instead of forwarding a fragment"
    );
}
