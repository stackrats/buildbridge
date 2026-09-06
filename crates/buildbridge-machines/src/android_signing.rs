//! Credential checks run in a disposable, offline JDK container before project scripts.
use super::*;

pub(super) const SIGNING_CHECK_SOURCE: &str = include_str!("AndroidSigningCheck.java");
const MAX_KEYSTORE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AndroidSigningAlgorithm {
    #[serde(rename = "RSA")]
    Rsa,
    #[serde(rename = "EC")]
    Ec,
}

impl AndroidSigningAlgorithm {
    pub(super) fn jarsigner_algorithm(self) -> &'static str {
        match self {
            Self::Rsa => "SHA256withRSA",
            Self::Ec => "SHA256withECDSA",
        }
    }
}

/// Public certificate metadata after a private-key signing challenge succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AndroidSigningCertificate {
    pub key_alias: String,
    pub certificate_sha256: String,
    pub certificate_sha1: String,
    pub algorithm: AndroidSigningAlgorithm,
    pub key_bits: u32,
    #[ts(type = "number")]
    pub valid_from_epoch_seconds: i64,
    #[ts(type = "number")]
    pub valid_until_epoch_seconds: i64,
}

pub(super) fn read_signing_keystore(
    signing: &AndroidSigningMaterial,
) -> Result<Vec<u8>, ProviderError> {
    if !valid_key_alias(&signing.key_alias) {
        return Err(signing_error(
            "The key alias must contain one to 64 letters, digits, dots, underscores or dashes.",
        ));
    }
    if !valid_keystore_password(&signing.keystore_password)
        || !valid_keystore_password(&signing.key_password)
    {
        return Err(signing_error(
            "The keystore and key passwords must be one line of six to 512 characters.",
        ));
    }
    let mut bytes = Vec::new();
    File::open(&signing.keystore_path)
        .and_then(|file| {
            file.take((MAX_KEYSTORE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|_| {
            signing_error("Could not read the Android keystore. Choose its current location.")
        })?;
    if bytes.is_empty() || bytes.len() > MAX_KEYSTORE_BYTES {
        return Err(signing_error(
            "The Android keystore must be a nonempty file no larger than 1 MiB.",
        ));
    }
    Ok(bytes)
}

fn signing_error(message: &str) -> ProviderError {
    ProviderError::AndroidToolchain(message.to_string())
}

fn signing_input(signing: &AndroidSigningMaterial, keystore: &[u8]) -> Vec<u8> {
    let mut input = Vec::new();
    for value in [
        keystore,
        signing.keystore_password.as_bytes(),
        signing.key_password.as_bytes(),
        signing.key_alias.as_bytes(),
    ] {
        input.extend_from_slice(&(value.len() as u32).to_be_bytes());
        input.extend_from_slice(value);
    }
    input
}

fn signing_check_args(name: &str) -> Vec<String> {
    let script = format!(
        "set -eu\numask 077\ncat > /tmp/AndroidSigningCheck.java <<'BUILDBRIDGE_JAVA_SOURCE'\n{SIGNING_CHECK_SOURCE}\nBUILDBRIDGE_JAVA_SOURCE\nexec /usr/bin/timeout --signal=KILL 45s {JAVA_HOME}/bin/java -Xmx192m /tmp/AndroidSigningCheck.java"
    );
    [
        "run",
        "--rm",
        ANDROID_PLATFORM_ARG,
        "--interactive",
        "--name",
        name,
        "--network=none",
        "--read-only",
        "--user=65534:65534",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "--memory=512m",
        "--pids-limit=64",
        "--cpus=1",
        "--tmpfs=/tmp:rw,noexec,nosuid,nodev,size=67108864",
        "--entrypoint=/bin/sh",
        ANDROID_IMAGE,
        "-c",
        &script,
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

struct DisposableSigningCheck(String);

impl Drop for DisposableSigningCheck {
    fn drop(&mut self) {
        // Cleanup must still run after the enclosing operation has been cancelled.
        let mut command = docker_command();
        command.args(["rm", "--force", &self.0]);
        let _ = signing_command(command, Vec::new(), Duration::from_secs(3), 8192, None);
    }
}

enum SigningPipe {
    Out(std::io::Result<Vec<u8>>),
    Err(std::io::Result<Vec<u8>>),
    Input(std::io::Result<()>),
}

fn stop_signing_process(child: &mut TrackedChild) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

/// Bound both pipes while writing stdin independently: Docker startup must not block the
/// deadline while a full credential payload waits in its input pipe. No diagnostics from
/// the child are included in errors. Cleanup passes no scope so Stop cannot prevent it.
fn signing_command(
    mut command: Command,
    mut input: Vec<u8>,
    timeout: Duration,
    output_limit: usize,
    scope: Option<Arc<OperationScope>>,
) -> Result<Output, ProviderError> {
    if scope.as_ref().is_some_and(|scope| scope.is_cancelled()) {
        input.fill(0);
        return Err(signing_error("The Android signing check was stopped."));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let spawned = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let child = match spawned {
        Ok(child) => child,
        Err(_) => {
            input.fill(0);
            return Err(signing_error(
                "Could not start Docker for the Android signing check.",
            ));
        }
    };
    if let Some(scope) = &scope {
        scope.register(child.id());
    }
    let mut child = TrackedChild {
        child: Some(child),
        scope: scope.clone(),
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    let oversized = Arc::new(AtomicBool::new(false));
    for (pipe, stderr) in [
        (
            Box::new(child.stdout.take().expect("piped output")) as Box<dyn Read + Send>,
            false,
        ),
        (
            Box::new(child.stderr.take().expect("piped errors")) as Box<dyn Read + Send>,
            true,
        ),
    ] {
        let sender = sender.clone();
        let oversized = Arc::clone(&oversized);
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe
                .take(output_limit as u64 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            if result
                .as_ref()
                .is_ok_and(|bytes| bytes.len() > output_limit)
            {
                oversized.store(true, Ordering::Release);
            }
            let _ = sender.send(if stderr {
                SigningPipe::Err(result)
            } else {
                SigningPipe::Out(result)
            });
        });
    }
    let mut stdin = child.stdin.take().expect("piped input");
    thread::spawn(move || {
        let result = stdin.write_all(&input);
        input.fill(0);
        drop(stdin);
        let _ = sender.send(SigningPipe::Input(result));
    });
    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut stdout = None;
    let mut stderr = None;
    let mut input_done = false;
    let mut input_failed = false;
    let mut failure = None;
    loop {
        for received in receiver.try_iter() {
            match received {
                SigningPipe::Out(Ok(bytes)) => stdout = Some(bytes),
                SigningPipe::Err(Ok(bytes)) => stderr = Some(bytes),
                SigningPipe::Input(result) => {
                    input_done = true;
                    input_failed = result.is_err();
                }
                _ => failure = Some("Could not read the Android signing check response."),
            }
        }
        if scope.as_ref().is_some_and(|scope| scope.is_cancelled()) {
            failure = Some("The Android signing check was stopped.");
        } else if oversized.load(Ordering::Acquire) {
            failure = Some("Docker returned too much output during the Android signing check.");
        } else if Instant::now() >= deadline {
            failure =
                Some("The Android signing check exceeded its time limit. Check Docker and retry.");
        }
        if failure.is_none() && status.is_none() {
            match child.try_wait() {
                Ok(Some(exited)) => {
                    status = Some(exited);
                    // Helpers must not hold inherited pipes open after their parent exits.
                    stop_signing_process(&mut child);
                }
                Ok(None) => {}
                Err(_) => failure = Some("Could not finish the Android signing check."),
            }
        }
        if let Some(message) = failure {
            stop_signing_process(&mut child);
            // Reap without allowing a stuck pipe or OS wait to defeat the caller's deadline.
            thread::spawn(move || {
                let _ = child.wait();
            });
            return Err(signing_error(message));
        }
        if input_done
            && let (Some(status), Some(stdout), Some(stderr)) =
                (status, stdout.as_mut(), stderr.as_mut())
        {
            if status.success() && input_failed {
                return Err(signing_error(
                    "Could not send the key to the Android signing checker.",
                ));
            }
            return Ok(Output {
                status,
                stdout: std::mem::take(stdout),
                stderr: std::mem::take(stderr),
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn ensure_signing_image() -> Result<(), ProviderError> {
    let mut inspect = docker_command();
    inspect.args(image_inspect_args(ANDROID_IMAGE));
    let inspected = signing_command(
        inspect,
        Vec::new(),
        Duration::from_secs(15),
        8192,
        current_scope(),
    )?;
    if !inspected.status.success() || clean_output(&inspected.stdout) != ANDROID_PLATFORM {
        let mut pull = docker_command();
        pull.args(image_pull_args());
        let pulled = signing_command(
            pull,
            Vec::new(),
            Duration::from_secs(300),
            4 * 1024 * 1024,
            current_scope(),
        )?;
        if !pulled.status.success() {
            return Err(signing_error(
                "Could not prepare the pinned Android signing image. Check Docker and network access, then retry.",
            ));
        }
    }
    Ok(())
}

/// Checks a saved key without exposing it to a project or sending it to Google.
pub fn verify_android_signing_material(
    signing: &AndroidSigningMaterial,
) -> Result<AndroidSigningCertificate, ProviderError> {
    let keystore = read_signing_keystore(signing)?;
    verify_signing_bytes(signing, &keystore)
}

pub(super) fn verify_signing_bytes(
    signing: &AndroidSigningMaterial,
    keystore: &[u8],
) -> Result<AndroidSigningCertificate, ProviderError> {
    ensure_signing_image()?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("buildbridge-signing-check-{}-{nonce}", std::process::id());
    let _cleanup = DisposableSigningCheck(name.clone());
    let mut command = docker_command();
    command.args(signing_check_args(&name));
    let output = signing_command(
        command,
        signing_input(signing, keystore),
        Duration::from_secs(60),
        8192,
        current_scope(),
    )?;
    if !output.status.success() {
        return Err(signing_error(signing_check_failure(&output.stderr)));
    }
    parse_signing_certificate(&output.stdout, &signing.key_alias)
}

fn signing_check_failure(stderr: &[u8]) -> &'static str {
    let text = String::from_utf8_lossy(stderr);
    let code = text
        .lines()
        .find_map(|line| line.strip_prefix("BUILDBRIDGE_SIGNING_ERROR:"));
    match code {
        Some("KEYSTORE_PASSWORD") => {
            "Could not unlock the Android keystore. Check its store password and that the file is a valid JKS or PKCS12 keystore."
        }
        Some("KEY_PASSWORD") => {
            "The key password does not unlock this alias. Enter its separate key password, or use the store password if they match."
        }
        Some("ALIAS") => "That key alias is not present in the Android keystore.",
        Some("PRIVATE_KEY") => {
            "This alias contains no private signing key. A public certificate alone cannot sign an app."
        }
        Some("CERTIFICATE") => "This key has no usable X.509 signing certificate.",
        Some("EXPIRED") => {
            "The Android signing certificate has expired. Check the upload key allowed for this app before replacing it."
        }
        Some("NOT_YET_VALID") => {
            "The Android signing certificate is not valid yet. Check this computer's date and the certificate's validity period."
        }
        Some("KEY_USAGE") => "This certificate does not permit digital signatures.",
        Some("ALGORITHM") => {
            "Android releases currently support RSA and EC signing keys. Choose a supported upload key for this app."
        }
        Some("KEY_SIZE") => {
            "Android releases require an RSA key of at least 2048 bits or an EC key of at least 256 bits."
        }
        Some("PRIVATE_KEY_MISMATCH") => "The private key does not match the signing certificate.",
        _ => {
            "Android signing verification could not finish. Check Docker, the keystore and its passwords, then retry. No Google account check was performed."
        }
    }
}

fn parse_signing_certificate(
    output: &[u8],
    alias: &str,
) -> Result<AndroidSigningCertificate, ProviderError> {
    if output.len() > 4096 {
        return Err(signing_error(
            "The signing checker returned an oversized certificate response.",
        ));
    }
    let result: AndroidSigningCertificate = serde_json::from_slice(output).map_err(|_| {
        signing_error("The signing checker returned an unexpected certificate response.")
    })?;
    if result.key_alias != alias
        || !valid_sha256(&result.certificate_sha256)
        || result.certificate_sha1.len() != 40
        || !result
            .certificate_sha1
            .chars()
            .all(|value| value.is_ascii_hexdigit())
        || result.valid_until_epoch_seconds <= result.valid_from_epoch_seconds
        || result.key_bits
            < match result.algorithm {
                AndroidSigningAlgorithm::Rsa => 2048,
                AndroidSigningAlgorithm::Ec => 256,
            }
    {
        return Err(signing_error(
            "The signing checker returned invalid certificate metadata.",
        ));
    }
    Ok(result)
}

#[cfg(test)]
#[path = "android_signing_tests.rs"]
mod tests;
