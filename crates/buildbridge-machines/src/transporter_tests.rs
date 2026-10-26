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
        crate::test_scripts::write_runnable(&mock, MOCK_TRANSPORTER);
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
            fixture.run("hang", &fixture.digest, Duration::from_secs(30))
        });
        let start = Instant::now();
        let startup_timeout = Duration::from_secs(20);
        while !fixture.directory.join("report.json").exists()
            && !worker.is_finished()
            && start.elapsed() < startup_timeout
        {
            thread::sleep(Duration::from_millis(50));
        }
        let ready = fixture.directory.join("report.json").exists();
        let finished_before_cancellation = worker.is_finished();
        // Cancel and join even when startup fails, so asserting readiness cannot leave the
        // upload running until its separate timeout or hide the worker's actual failure.
        scope.cancel();
        let (result, progress) = worker.join().expect("upload worker should not panic");
        assert!(
            ready,
            "mock Transporter did not start within {startup_timeout:?}; worker finished before cancellation: {finished_before_cancellation}; result: {result:?}; progress: {progress:?}"
        );
        let error = result.unwrap_err().to_string();
        assert!(error.contains("stopped"), "{error}");
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
    assert!(error.contains("Use Install Transporter"), "{error}");
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

/// The readiness probe runs the upload's own search, so it finds buildbridge's install and
/// reports nothing else as installed.
#[test]
fn the_readiness_probe_shares_the_upload_search() {
    let home = crate::test_scripts::fixture_dir("transporter-locate");
    let locate = |prelude: &str| {
        let output = Command::new("/usr/bin/python3")
            .args(["-c", &format!("{prelude}\n{TRANSPORTER_HELPER}"), "locate"])
            .env("HOME", &home)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    // Never the developer's own Transporter, even on a Mac that has one.
    assert_eq!(
        locate("import os\nos.path.isfile = lambda _: False"),
        "missing"
    );
    let installed = home.join(".buildbridge/tools/itms/bin");
    fs::create_dir_all(&installed).unwrap();
    crate::test_scripts::write_runnable(&installed.join("iTMSTransporter"), "#!/bin/sh\n");
    assert_eq!(locate(""), "installed");
    let _ = fs::remove_dir_all(&home);
}

const FAKE_CURL: &str = r#"#!/bin/sh
output=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output) output=$2; shift 2 ;;
        *) shift ;;
    esac
done
if [ "$FIXTURE_MODE" = offline ]; then
    printf 'curl: (6) Could not resolve host: itunesconnect.apple.com\n' >&2
    exit 6
fi
printf 'package\n' > "$output"
"#;

/// Answers as `pkgutil` does: the signature report, then a flat package expanded into its
/// component directories, each with a gzip payload.
const FAKE_PKGUTIL: &str = r#"#!/bin/sh
case "$1" in
    --check-signature)
        status='signed by a developer certificate issued by Apple for distribution'
        leaf='Developer ID Installer: Apple Inc. - MZCD Automation (4U568XBT3J)'
        case "$FIXTURE_MODE" in
            unsigned) printf 'Package "iTMSTransporter.pkg":\n   Status: no signature\n'; exit 1 ;;
            untrusted) status='signed by untrusted certificate' ;;
            other_team) leaf='Developer ID Installer: Apple Inc. - Someone Else (ABCDE12345)' ;;
        esac
        printf 'Package "iTMSTransporter.pkg":\n   Status: %s\n   Notarization: trusted by the Apple notary service\n   Certificate Chain:\n    1. %s\n       Expires: 2029-10-18 17:06:11 +0000\n       ------------------------------------------------------------------------\n    2. Developer ID Certification Authority\n    3. Apple Root CA\n\n' "$status" "$leaf"
        ;;
    --expand)
        mkdir -p "$3/payload/bin"
        printf '#!/bin/sh\nexit 0\n' > "$3/payload/bin/iTMSTransporter"
        chmod 755 "$3/payload/bin/iTMSTransporter"
        for component in iTMSTransporter-9.9.0.1.pkg $( [ "$FIXTURE_MODE" = two_components ] && echo Extra.pkg ); do
            mkdir -p "$3/$component"
            tar -czf "$3/$component/Payload" -C "$3/payload" bin
        done
        rm -rf "$3/payload"
        ;;
esac
"#;

/// The install body against stand-ins for curl and pkgutil: only a package whose signature
/// names Apple's Transporter team is unpacked, it replaces the earlier install and its caches,
/// and every refusal leaves the earlier install exactly as it was.
#[test]
fn transporter_installs_only_a_package_signed_by_apples_transporter_team() {
    let root = crate::test_scripts::fixture_dir("transporter-install");
    let tools = root.join(".buildbridge/tools");
    let fake_curl = root.join("curl");
    let fake_pkgutil = root.join("pkgutil");
    crate::test_scripts::write_runnable(&fake_curl, FAKE_CURL);
    crate::test_scripts::write_runnable(&fake_pkgutil, FAKE_PKGUTIL);
    let body = transporter_install_body(tools.to_str().unwrap());
    assert!(
        body.contains(&format!("'{TRANSPORTER_PACKAGE_URL}'")),
        "{body}"
    );
    let body = body
        .replace("/usr/bin/curl", fake_curl.to_str().unwrap())
        .replace("/usr/sbin/pkgutil", fake_pkgutil.to_str().unwrap());
    let earlier = tools.join("itms/bin/iTMSTransporter");
    // The body runs inside the guest job wrapper, under the guest's login shell: zsh on any
    // macOS since Catalina. Every shell this host has stands in for it.
    let shells = ["/bin/sh", "/bin/bash", "/bin/zsh"]
        .into_iter()
        .filter(|shell| Path::new(shell).is_file())
        .collect::<Vec<_>>();
    let run = |shell: &str, mode: &str| {
        let _ = fs::remove_dir_all(&tools);
        fs::create_dir_all(earlier.parent().unwrap()).unwrap();
        fs::write(&earlier, "earlier\n").unwrap();
        fs::create_dir_all(root.join(".itmstransporter")).unwrap();
        let output = Command::new(shell)
            .args(["-c", &format!("exec 2>&1\nset -eu\n{body}")])
            .env("HOME", &root)
            .env("FIXTURE_MODE", mode)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let phases = stdout
            .lines()
            .filter_map(|line| line.strip_prefix(TRANSPORTER_INSTALL_PREFIX))
            .map(str::to_string)
            .collect::<Vec<_>>();
        (output.status.code(), stdout, phases)
    };

    for shell in shells {
        let (status, stdout, phases) = run(shell, "signed");
        assert_eq!(status, Some(0), "{shell}: {stdout}");
        assert_eq!(
            phases,
            ["downloading", "verifying", "unpacking", "completed"],
            "{shell}"
        );
        assert_eq!(fs::read_to_string(&earlier).unwrap(), "#!/bin/sh\nexit 0\n");
        assert!(!tools.join("itms-staging").exists(), "{shell}");
        assert!(!root.join(".itmstransporter").exists(), "{shell}");

        for mode in ["unsigned", "untrusted", "other_team"] {
            let (status, stdout, phases) = run(shell, mode);
            assert_ne!(status, Some(0), "{shell}: {mode}: {stdout}");
            assert_eq!(
                phases.last().map(String::as_str),
                Some("verifying"),
                "{shell}: {mode}"
            );
            assert_eq!(
                fs::read_to_string(&earlier).unwrap(),
                "earlier\n",
                "{shell}: {mode}"
            );
            assert!(root.join(".itmstransporter").exists(), "{shell}: {mode}");
        }

        let (status, stdout, phases) = run(shell, "offline");
        assert_eq!(status, Some(6), "{shell}: {stdout}");
        assert_eq!(phases, ["downloading"], "{shell}");
        assert!(stdout.contains("Could not resolve host"), "{stdout}");
        assert_eq!(
            fs::read_to_string(&earlier).unwrap(),
            "earlier\n",
            "{shell}"
        );

        let (status, stdout, phases) = run(shell, "two_components");
        assert_ne!(status, Some(0), "{shell}: {stdout}");
        assert_eq!(
            phases.last().map(String::as_str),
            Some("unpacking"),
            "{shell}"
        );
        assert_eq!(
            fs::read_to_string(&earlier).unwrap(),
            "earlier\n",
            "{shell}"
        );
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_failed_install_says_which_step_failed_and_only_passes_on_actionable_output() {
    let offline = transporter_install_failure(
        Some("downloading"),
        "curl: (6) Could not resolve host: itunesconnect.apple.com",
    );
    assert!(
        offline.starts_with("could not download Transporter from Apple"),
        "{offline}"
    );
    assert!(
        offline.ends_with(": curl: (6) Could not resolve host: itunesconnect.apple.com"),
        "{offline}"
    );

    let refused = transporter_install_failure(Some("verifying"), "Status: no signature");
    assert!(
        refused.contains("not signed by Apple's Transporter team"),
        "{refused}"
    );
    assert!(!refused.contains("Status"), "{refused}");

    let unpacking = transporter_install_failure(Some("unpacking"), "");
    assert_eq!(
        unpacking,
        "could not unpack Transporter; check the macOS machine's free storage"
    );
    assert!(transporter_install_failure(None, "ssh: connect").contains("before it started"));
    assert!(transporter_install_phase("anything else").is_none());
}
