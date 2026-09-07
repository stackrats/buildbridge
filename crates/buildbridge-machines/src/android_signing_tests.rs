use super::*;

fn material(path: &Path) -> AndroidSigningMaterial {
    AndroidSigningMaterial {
        keystore_path: path.to_path_buf(),
        keystore_password: " store fixture password ".to_string(),
        key_alias: "upload".to_string(),
        key_password: " store fixture password ".to_string(),
    }
}

#[test]
fn credentials_use_bounded_frames_and_never_container_arguments() {
    let signing = material(Path::new("/private/key location.jks"));
    let args = signing_check_args("buildbridge-check-fixture");
    assert!(args.iter().any(|arg| arg == "--network=none"));
    assert!(args.iter().any(|arg| arg == "--read-only"));
    assert!(args.iter().any(|arg| arg == "--user=65534:65534"));
    assert!(args.iter().any(|arg| arg == ANDROID_IMAGE));
    assert!(args.iter().any(|arg| arg == ANDROID_PLATFORM_ARG));
    assert!(
        !args
            .iter()
            .any(|arg| arg.contains("--volume") || arg.contains("--mount"))
    );
    let args = args.join("\n");
    assert!(!args.contains(&signing.keystore_password));
    assert!(!args.contains("/private/key location"));
    let encoded = signing_input(&signing, &[0, 10, 255, 13]);
    let mut encoded = encoded.as_slice();
    for expected in [
        &[0_u8, 10, 255, 13][..],
        signing.keystore_password.as_bytes(),
        signing.key_password.as_bytes(),
        signing.key_alias.as_bytes(),
    ] {
        let (size, remaining) = encoded.split_at(4);
        let size = u32::from_be_bytes(size.try_into().unwrap()) as usize;
        let (field, remaining) = remaining.split_at(size);
        assert_eq!(field, expected);
        encoded = remaining;
    }
    assert!(encoded.is_empty());
}

#[test]
fn malformed_public_metadata_and_provider_messages_are_rejected_without_echoing_secrets() {
    let valid = serde_json::json!({
        "keyAlias": "upload", "certificateSha256": "a".repeat(64),
        "certificateSha1": "b".repeat(40), "algorithm": "EC", "keyBits": 256,
        "validFromEpochSeconds": 10, "validUntilEpochSeconds": 20,
    });
    assert!(parse_signing_certificate(&serde_json::to_vec(&valid).unwrap(), "upload").is_ok());
    for (field, value) in [
        ("keyAlias", serde_json::json!("different")),
        ("algorithm", serde_json::json!("DSA")),
        ("keyBits", serde_json::json!(128)),
        ("certificateSha256", serde_json::json!("bad")),
        ("validUntilEpochSeconds", serde_json::json!(5)),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        assert!(
            parse_signing_certificate(&serde_json::to_vec(&invalid).unwrap(), "upload").is_err()
        );
    }
    assert!(
        !signing_check_failure(b"secret password from third-party diagnostic")
            .contains("secret password")
    );
    assert!(
        signing_check_failure(b"BUILDBRIDGE_SIGNING_ERROR:PRIVATE_KEY")
            .contains("private signing key")
    );
    assert_eq!(
        AndroidSigningAlgorithm::Ec.jarsigner_algorithm(),
        "SHA256withECDSA"
    );
}

struct JdkFixture {
    directory: PathBuf,
    store_password: PathBuf,
    key_password: PathBuf,
}

impl JdkFixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "buildbridge-android-check-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let store_password = directory.join("store.pass");
        let key_password = directory.join("key.pass");
        fs::write(&store_password, " store fixture password ").unwrap();
        fs::write(&key_password, " separate key fixture password ").unwrap();
        fs::write(
            directory.join("AndroidSigningCheck.java"),
            SIGNING_CHECK_SOURCE,
        )
        .unwrap();
        let fixture = Self {
            directory,
            store_password,
            key_password,
        };
        fixture
            .command("javac")
            .args(["--release", "17"])
            .arg(fixture.directory.join("AndroidSigningCheck.java"))
            .output()
            .map(assert_success)
            .expect("local JDK fixture tests require javac");
        fixture
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command.current_dir(&self.directory);
        command
    }

    fn generate(
        &self,
        name: &str,
        algorithm: &str,
        separate_password: bool,
        extras: &[&str],
    ) -> AndroidSigningMaterial {
        let path = self.directory.join(format!("{name}.keystore"));
        let mut command = self.command("keytool");
        command
            .args([
                "-genkeypair",
                "-alias",
                "upload",
                "-keyalg",
                algorithm,
                "-dname",
                "CN=buildbridge disposable test",
                "-validity",
                "365",
                "-storetype",
                if separate_password { "JKS" } else { "PKCS12" },
                "-keystore",
            ])
            .arg(&path)
            .arg("-storepass:file")
            .arg(&self.store_password)
            .arg("-keypass:file")
            .arg(if separate_password {
                &self.key_password
            } else {
                &self.store_password
            });
        if algorithm == "EC" {
            command.args(["-groupname", "secp256r1"]);
        } else {
            command.args(["-keysize", "2048"]);
        }
        command.args(extras).output().map(assert_success).unwrap();
        let mut signing = material(&path);
        if separate_password {
            signing.key_password = " separate key fixture password ".to_string();
        }
        signing
    }

    fn verify(&self, signing: &AndroidSigningMaterial) -> Output {
        self.verify_bytes(signing, &fs::read(&signing.keystore_path).unwrap())
    }

    fn verify_bytes(&self, signing: &AndroidSigningMaterial, bytes: &[u8]) -> Output {
        let mut command = self.command("java");
        command.args(["-cp", ".", "AndroidSigningCheck"]);
        signing_command(
            command,
            signing_input(signing, bytes),
            Duration::from_secs(10),
            8192,
            None,
        )
        .unwrap()
    }

    fn jar_check(&self, jar: &Path, fingerprint: &str) -> Output {
        self.command("java")
            .args(["-cp", ".", "AndroidSigningCheck", "verify-jar"])
            .arg(jar)
            .arg(fingerprint)
            .output()
            .unwrap()
    }
}

impl Drop for JdkFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn assert_success(output: Output) -> Output {
    assert!(
        output.status.success(),
        "fixture command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn assert_failure(output: Output, code: &str) {
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        format!("BUILDBRIDGE_SIGNING_ERROR:{code}")
    );
    assert!(output.stdout.is_empty());
}

#[cfg(unix)]
fn fixed_shell(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script]);
    command
}

#[cfg(unix)]
#[test]
fn checker_host_deadline_covers_blocked_stdin_and_closed_output() {
    let started = Instant::now();
    let error = signing_command(
        fixed_shell("trap '' TERM; exec >/dev/null 2>&1; sleep 20"),
        vec![42; MAX_KEYSTORE_BYTES],
        Duration::from_millis(150),
        1024,
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("time limit"));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[cfg(unix)]
#[test]
fn checker_cancels_term_ignoring_process_and_cleanup_ignores_cancelled_scope() {
    let scope = OperationScope::new();
    let worker_scope = Arc::clone(&scope);
    let started = Instant::now();
    let worker = thread::spawn(move || {
        signing_command(
            fixed_shell("trap '' TERM; exec >/dev/null 2>&1; sleep 20"),
            vec![42; MAX_KEYSTORE_BYTES],
            Duration::from_secs(20),
            1024,
            Some(worker_scope),
        )
    });
    thread::sleep(Duration::from_millis(100));
    scope.cancel();
    assert!(
        worker
            .join()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("stopped")
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    let _operation = enter_operation(scope);
    assert!(
        signing_command(
            fixed_shell("exit 0"),
            Vec::new(),
            Duration::from_secs(1),
            1024,
            None
        )
        .unwrap()
        .status
        .success()
    );
}

#[cfg(unix)]
#[test]
fn checker_caps_output_before_waiting_and_closes_inherited_pipes() {
    let started = Instant::now();
    let error = signing_command(
        fixed_shell("while :; do printf 'oversized output without a newline' >&2; done"),
        vec![42; MAX_KEYSTORE_BYTES],
        Duration::from_secs(5),
        1024,
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("too much output"));
    assert!(started.elapsed() < Duration::from_secs(2));
    let output = signing_command(
        fixed_shell("sleep 20 & printf done"),
        Vec::new(),
        Duration::from_secs(1),
        1024,
        None,
    )
    .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"done");
}

#[test]
#[ignore = "requires local JDK tools; uses disposable fixtures, no Docker or network"]
fn local_jdk_proves_rsa_ec_separate_passwords_and_rejects_invalid_keys() {
    let fixture = JdkFixture::new();
    let rsa = fixture.generate("rsa", "RSA", false, &[]);
    let ec = fixture.generate("ec", "EC", false, &[]);
    let jks = fixture.generate("jks", "RSA", true, &[]);
    for (signing, algorithm) in [
        (&rsa, AndroidSigningAlgorithm::Rsa),
        (&ec, AndroidSigningAlgorithm::Ec),
        (&jks, AndroidSigningAlgorithm::Rsa),
    ] {
        let output = assert_success(fixture.verify(signing));
        let certificate = parse_signing_certificate(&output.stdout, "upload").unwrap();
        assert_eq!(certificate.algorithm, algorithm);
        assert!(certificate.valid_until_epoch_seconds > certificate.valid_from_epoch_seconds);
    }
    let mut wrong = jks.clone();
    wrong.keystore_password = "incorrect store password".into();
    assert_failure(fixture.verify(&wrong), "KEYSTORE_PASSWORD");
    wrong = jks.clone();
    wrong.key_password = "incorrect key password".into();
    assert_failure(fixture.verify(&wrong), "KEY_PASSWORD");
    wrong = rsa.clone();
    wrong.key_alias = "missing".into();
    assert_failure(fixture.verify(&wrong), "ALIAS");
    assert_failure(
        fixture.verify_bytes(&rsa, b"not a keystore"),
        "KEYSTORE_PASSWORD",
    );
    let expired = fixture.generate(
        "expired",
        "RSA",
        false,
        &["-startdate", "-2d", "-validity", "1"],
    );
    assert_failure(fixture.verify(&expired), "EXPIRED");
    let future = fixture.generate("future", "EC", false, &["-startdate", "+2d"]);
    assert_failure(fixture.verify(&future), "NOT_YET_VALID");
    let dsa = fixture.generate("dsa", "DSA", false, &[]);
    assert_failure(fixture.verify(&dsa), "ALGORITHM");
    let weak = fixture.generate("weak", "RSA", false, &["-keysize", "1024"]);
    assert_failure(fixture.verify(&weak), "KEY_SIZE");
    let wrong_usage = fixture.generate("usage", "RSA", false, &["-ext", "KU=keyEncipherment"]);
    assert_failure(fixture.verify(&wrong_usage), "KEY_USAGE");

    let public_certificate = fixture.directory.join("public.cer");
    fixture
        .command("keytool")
        .args(["-exportcert", "-alias", "upload", "-keystore"])
        .arg(&rsa.keystore_path)
        .arg("-storepass:file")
        .arg(&fixture.store_password)
        .arg("-file")
        .arg(&public_certificate)
        .output()
        .map(assert_success)
        .unwrap();
    let public_only = material(&fixture.directory.join("public-only.keystore"));
    fixture
        .command("keytool")
        .args(["-importcert", "-noprompt", "-alias", "upload", "-keystore"])
        .arg(&public_only.keystore_path)
        .arg("-storepass:file")
        .arg(&fixture.store_password)
        .arg("-file")
        .arg(public_certificate)
        .output()
        .map(assert_success)
        .unwrap();
    assert_failure(fixture.verify(&public_only), "PRIVATE_KEY");
}

#[test]
#[ignore = "requires local JDK tools; uses disposable fixtures, no Docker or network"]
fn local_jdk_bundle_verification_rejects_wrong_signer_unsigned_and_modified_content() {
    let fixture = JdkFixture::new();
    for (name, algorithm) in [("rsa", "RSA"), ("ec", "EC")] {
        let signing = fixture.generate(name, algorithm, false, &[]);
        let certificate =
            parse_signing_certificate(&assert_success(fixture.verify(&signing)).stdout, "upload")
                .unwrap();
        fs::write(
            fixture.directory.join("content.txt"),
            "original app content",
        )
        .unwrap();
        let jar = fixture.directory.join(format!("{name}.aab"));
        fixture
            .command("jar")
            .args(["--create", "--file"])
            .arg(&jar)
            .arg("content.txt")
            .output()
            .map(assert_success)
            .unwrap();
        fixture
            .command("jarsigner")
            .arg("-keystore")
            .arg(&signing.keystore_path)
            .arg("-storepass:file")
            .arg(&fixture.store_password)
            .arg("-keypass:file")
            .arg(&fixture.store_password)
            .args([
                "-sigalg",
                certificate.algorithm.jarsigner_algorithm(),
                "-digestalg",
                "SHA-256",
            ])
            .arg(&jar)
            .arg("upload")
            .output()
            .map(assert_success)
            .unwrap();
        assert_success(fixture.jar_check(&jar, &certificate.certificate_sha256));
        assert_failure(fixture.jar_check(&jar, &"a".repeat(64)), "JAR_SIGNER");
        let unsigned = fixture.directory.join(format!("{name}-unsigned.aab"));
        fs::copy(&jar, &unsigned).unwrap();
        fs::write(
            fixture.directory.join("added.txt"),
            "unsigned added content",
        )
        .unwrap();
        fixture
            .command("jar")
            .args(["--update", "--file"])
            .arg(&unsigned)
            .arg("added.txt")
            .output()
            .map(assert_success)
            .unwrap();
        assert_failure(
            fixture.jar_check(&unsigned, &certificate.certificate_sha256),
            "JAR_SIGNER",
        );
        fs::write(
            fixture.directory.join("content.txt"),
            "tampered app content",
        )
        .unwrap();
        fixture
            .command("jar")
            .args(["--update", "--file"])
            .arg(&jar)
            .arg("content.txt")
            .output()
            .map(assert_success)
            .unwrap();
        assert_failure(
            fixture.jar_check(&jar, &certificate.certificate_sha256),
            "VERIFICATION",
        );
    }
}

#[test]
fn signing_algorithms_map_to_the_matching_jarsigner_digest_and_keep_their_wire_names() {
    assert_eq!(
        AndroidSigningAlgorithm::Rsa.jarsigner_algorithm(),
        "SHA256withRSA"
    );
    assert_eq!(
        AndroidSigningAlgorithm::Ec.jarsigner_algorithm(),
        "SHA256withECDSA"
    );
    assert_eq!(
        serde_json::to_string(&AndroidSigningAlgorithm::Ec).unwrap(),
        "\"EC\""
    );
    assert_eq!(
        serde_json::from_str::<AndroidSigningAlgorithm>("\"RSA\"").unwrap(),
        AndroidSigningAlgorithm::Rsa
    );
    for wire in ["\"rsa\"", "\"DSA\"", "\"Ec\""] {
        assert!(
            serde_json::from_str::<AndroidSigningAlgorithm>(wire).is_err(),
            "{wire}"
        );
    }
}
