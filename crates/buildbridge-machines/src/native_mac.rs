//! Native Apple builds on the Mac that owns Xcode and its signing identities.
//! Project code runs as the owner; a separate checkout is not a security sandbox.

use super::*;
use std::ffi::OsString;
use std::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NativeMacIdentity {
    pub sha1: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NativeMacToolchain {
    pub architecture: String,
    pub developer_directory: Option<String>,
    pub xcode_version: Option<String>,
    pub ios_sdk: Option<String>,
    pub available_sdks: Option<String>,
    pub node_version: Option<String>,
    pub pnpm_version: Option<String>,
    pub cocoapods_version: Option<String>,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NativeMacProfile {
    pub uuid: String,
    pub team_identifier: String,
    pub application_identifier: String,
    pub expires_at: String,
    #[ts(type = "number")]
    pub expires_at_epoch_seconds: u64,
    pub sha256: String,
    pub certificate_sha1s: Vec<String>,
}

pub fn require_native_mac() -> Result<(), String> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err("Native Apple builds run on a Mac with Xcode installed.".to_string())
    }
}

/// A complete object ID, never a branch, option, revision expression, or remote command.
pub fn valid_native_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64) && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Only network transports whose repository is explicitly approved by the Mac owner.
/// Credential-bearing HTTP URLs, local paths, and Git's executable remote helpers are refused.
pub fn valid_native_repository(repository: &str) -> bool {
    if repository.is_empty()
        || repository.len() > 2048
        || repository
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        || repository.contains("::")
    {
        return false;
    }
    if let Some(rest) = repository.strip_prefix("https://") {
        let authority = rest.split('/').next().unwrap_or_default();
        return !authority.is_empty()
            && !authority.contains('@')
            && !repository.contains(['?', '#']);
    }
    let value = repository.strip_prefix("ssh://").unwrap_or(repository);
    (if repository.starts_with("ssh://") {
        value.contains('/')
    } else {
        value.contains(':')
    }) && !value.starts_with('-')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@.:/_-~".contains(c))
        && (repository.starts_with("ssh://") || value.contains('@'))
}

const SYSTEM_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

fn host_path() -> OsString {
    host_path_for(
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
        std::env::var_os("PATH").as_deref(),
    )
}

/// App bundles usually receive only /usr/bin:/bin. Include the standard owner-installed
/// Homebrew locations, whatever the process inherited, and then the per-user tool managers'
/// directories, without sourcing a shell startup file.
fn host_path_for(home: Option<&Path>, inherited: Option<&std::ffi::OsStr>) -> OsString {
    let mut path = OsString::from(SYSTEM_PATH);
    if let Some(existing) = inherited {
        path.push(":");
        path.push(existing);
    }
    for directory in home.map(user_tool_directories).unwrap_or_default() {
        if std::env::split_paths(&path).any(|entry| entry == directory) {
            continue;
        }
        path.push(":");
        path.push(directory);
    }
    path
}

/// Node.js and pnpm installed through Volta, fnm, nvm, or corepack live under the owner's
/// home directory, where a desktop launched from Finder cannot see them: the terminal's
/// shell startup file put them on PATH. These are the managers' default locations, resolved
/// without running their shell functions. Only directories that exist are returned.
fn user_tool_directories(home: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![home.join(".volta/bin")];
    candidates.extend(
        [
            "Library/Application Support/fnm/aliases/default/bin",
            ".local/share/fnm/aliases/default/bin",
            ".fnm/aliases/default/bin",
        ]
        .into_iter()
        .map(|relative| home.join(relative)),
    );
    candidates.extend(nvm_default_bin(&home.join(".nvm")));
    candidates.push(home.join(".local/bin"));
    candidates
        .into_iter()
        .filter(|path| path.is_dir())
        .collect()
}

/// nvm's `default` alias names a version (`v22.1.0`), a prefix of one (`22`), or another
/// alias (`lts/*`, whose file names the current LTS line). The highest installed version
/// stands in when the alias is missing or names nothing installed.
fn nvm_default_bin(nvm: &Path) -> Option<PathBuf> {
    let mut installed = fs::read_dir(nvm.join("versions/node"))
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            Some((version_numbers(name.strip_prefix('v')?)?, entry.path()))
        })
        .collect::<Vec<_>>();
    installed.sort();
    let (_, highest) = installed.last()?;
    let selected = nvm_alias_version(nvm, "default", 0).and_then(|wanted| {
        installed
            .iter()
            .rev()
            .find(|(version, _)| version.starts_with(&wanted))
            .map(|(_, path)| path)
    });
    Some(selected.unwrap_or(highest).join("bin"))
}

fn nvm_alias_version(nvm: &Path, alias: &str, depth: usize) -> Option<Vec<u64>> {
    if depth > 8
        || alias.is_empty()
        || alias.len() > 64
        || alias.starts_with('/')
        || alias.split('/').any(|part| part.is_empty() || part == "..")
        || !alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "./*-_".contains(c))
    {
        return None;
    }
    let value = fs::read_to_string(nvm.join("alias").join(alias)).ok()?;
    let value = value.trim();
    match version_numbers(value.strip_prefix('v').unwrap_or(value)) {
        Some(version) => Some(version),
        None => nvm_alias_version(nvm, value, depth + 1),
    }
}

fn version_numbers(value: &str) -> Option<Vec<u64>> {
    if value.is_empty() || value.len() > 32 {
        return None;
    }
    value
        .split('.')
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                None
            } else {
                part.parse().ok()
            }
        })
        .collect()
}

fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command.env_clear();
    for name in ["HOME", "USER", "LOGNAME", "TMPDIR", "DEVELOPER_DIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .current_dir(if cfg!(windows) {
            std::env::temp_dir()
        } else {
            PathBuf::from("/")
        })
        .env("PATH", host_path())
        .env("LANG", "en_US.UTF-8");
    command
}

fn capture(mut command: Command, label: &str) -> Result<String, String> {
    capture_with_timeout(&mut command, label, Duration::from_secs(30))
}

fn capture_with_timeout(
    command: &mut Command,
    label: &str,
    timeout: Duration,
) -> Result<String, String> {
    let stdout = capture_output(command, None, label, timeout)?;
    String::from_utf8(stdout)
        .map(|value| value.trim().to_string())
        .map_err(|_| format!("The response while trying to {label} is not text."))
}

const CAPTURE_LIMIT: usize = 2 * 1024 * 1024;

/// Runs a metadata helper in its own process group with a deadline, bounded output, and the
/// operation's cancellation. `input` reaches its standard input from a separate thread so a
/// helper that never reads cannot stall the caller.
fn capture_output(
    command: &mut Command,
    input: Option<&[u8]>,
    label: &str,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| format!("Could not {label}: {error}"))?;
    let writer = match input {
        Some(input) => {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| "Command input is unavailable.".to_string())?;
            let input = input.to_vec();
            Some(thread::spawn(move || stdin.write_all(&input)))
        }
        None => None,
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Command output is unavailable.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Command errors are unavailable.".to_string())?;
    let read = |pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.take(CAPTURE_LIMIT as u64 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        })
    };
    let out = read(Box::new(stdout));
    let err = read(Box::new(stderr));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        if Instant::now() >= deadline || current_scope().is_some_and(|scope| scope.is_cancelled()) {
            timed_out = true;
            // The group is stopped while its leader is still ours to reap: until then the
            // group ID cannot be handed to an unrelated process.
            #[cfg(unix)]
            kill_process_group(child.id());
            let _ = child.kill();
            break child.wait().map_err(|error| error.to_string())?;
        }
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        thread::sleep(Duration::from_millis(25));
    };
    // A metadata helper may exit before a child that inherited its output pipes. Those
    // children must not defeat the probe's deadline by keeping the readers open.
    #[cfg(unix)]
    kill_process_group_survivors(child.id());
    let stdout = out
        .join()
        .map_err(|_| "Command output reader stopped.".to_string())?
        .map_err(|e| e.to_string())?;
    let stderr = err
        .join()
        .map_err(|_| "Command error reader stopped.".to_string())?
        .map_err(|e| e.to_string())?;
    if let Some(writer) = writer {
        // A helper that exits without reading its input is judged by its status instead.
        let _ = writer.join();
    }
    if timed_out {
        return Err(format!(
            "Could not {label}: the command stopped or exceeded its time limit."
        ));
    }
    if !status.success() {
        return Err(format!(
            "Could not {label}: {}",
            sanitize_build_log_line(&String::from_utf8_lossy(&stderr))
        ));
    }
    if stdout.len() > CAPTURE_LIMIT {
        return Err(format!(
            "The response while trying to {label} is too large."
        ));
    }
    Ok(stdout)
}

/// Stops every process in the group a native command leads. Sound only while the leader is
/// still unreaped or the group still has members; once a group has emptied, its ID may
/// already belong to an unrelated process.
#[cfg(unix)]
fn kill_process_group(pid: u32) {
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Whether the group led by `pid` still has members, without reaping anything: signal 0 is
/// delivered to nobody but fails with "no such process" for an emptied group.
#[cfg(unix)]
fn process_group_alive(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// After the leader has been reaped, stops the members that outlived it. Those members are
/// what keeps the group ID reserved, so nothing is signalled when the check finds none.
#[cfg(unix)]
fn kill_process_group_survivors(pid: u32) {
    if process_group_alive(pid) {
        kill_process_group(pid);
    }
}

fn captured(program: &str, args: &[&str], label: &str) -> Result<String, String> {
    let mut cmd = command(program);
    cmd.args(args);
    capture(cmd, label)
}

pub fn probe_native_mac() -> NativeMacToolchain {
    let mut toolchain = NativeMacToolchain {
        architecture: std::env::consts::ARCH.to_string(),
        ..Default::default()
    };
    if let Err(error) = require_native_mac() {
        toolchain.issues.push(error);
        return toolchain;
    }
    let mut probe = |program: &str, args: &[&str], label: &str| {
        let mut cmd = command(program);
        cmd.args(args);
        match capture_with_timeout(&mut cmd, label, Duration::from_secs(6)) {
            Ok(value) => Some(value),
            Err(error) => {
                toolchain.issues.push(error);
                None
            }
        }
    };
    toolchain.developer_directory = std::env::var("DEVELOPER_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| probe("/usr/bin/xcode-select", &["-p"], "find the selected Xcode"));
    toolchain.xcode_version = probe(
        "/usr/bin/xcodebuild",
        &["-version"],
        "read Xcode's version (open Xcode and finish its setup)",
    );
    toolchain.ios_sdk = probe(
        "/usr/bin/xcrun",
        &["--sdk", "iphoneos", "--show-sdk-version"],
        "find the iOS SDK (install it in Xcode Settings)",
    );
    toolchain.available_sdks = probe(
        "/usr/bin/xcodebuild",
        &["-showsdks"],
        "list the installed Xcode SDKs",
    );
    toolchain.node_version = probe("node", &["--version"], "find Node.js on this Mac");
    toolchain.pnpm_version = probe("pnpm", &["--version"], "find pnpm on this Mac");
    toolchain.cocoapods_version = probe("pod", &["--version"], "find CocoaPods on this Mac");
    toolchain
}

pub fn native_mac_identities() -> Result<Vec<NativeMacIdentity>, String> {
    require_native_mac()?;
    let mut cmd = command("/usr/bin/security");
    cmd.args(["find-identity", "-v", "-p", "codesigning"]);
    Ok(parse_identities(&capture_with_timeout(
        &mut cmd,
        "list available signing identities",
        Duration::from_secs(6),
    )?))
}

fn parse_identities(output: &str) -> Vec<NativeMacIdentity> {
    output
        .lines()
        .filter_map(|line| {
            let (_, rest) = line.trim().split_once(") ")?;
            let (hash, label) = rest.split_once(' ')?;
            if hash.len() != 40 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            let name = label.strip_prefix('"')?.strip_suffix('"')?;
            if !name.starts_with("Apple Distribution:") && !name.starts_with("iPhone Distribution:")
            {
                return None;
            }
            Some(NativeMacIdentity {
                sha1: hash.to_ascii_uppercase(),
                name: name.to_string(),
            })
        })
        .collect()
}

pub fn native_repository_for(path: &Path) -> Result<String, String> {
    require_native_mac()?;
    let mut cmd = command("/usr/bin/git");
    cmd.arg("-C")
        .arg(path)
        .args(["remote", "get-url", "origin"]);
    let remote = capture(cmd, "read the project's origin repository")?;
    if !valid_native_repository(&remote) {
        return Err("Use an HTTPS repository without embedded credentials, or an SSH repository, as this project's origin.".to_string());
    }
    Ok(remote)
}

/// The crate-wide file checksum, reached as `buildbridge_machines::native_sha256`: the host's
/// own digest tool, run with the same bounded, cancellable helper as every native command.
pub fn native_sha256(path: &Path) -> Result<String, String> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut cmd = command("/usr/bin/shasum");
        cmd.args(["-a", "256"]);
        cmd
    } else {
        command("sha256sum")
    };
    cmd.arg(path);
    let output = capture_with_timeout(
        &mut cmd,
        "checksum the native build file",
        Duration::from_secs(300),
    )?;
    let hash = output.split_whitespace().next().unwrap_or_default();
    if !valid_sha256(hash) {
        return Err("The native build checksum is invalid.".to_string());
    }
    Ok(hash.to_ascii_lowercase())
}

fn native_sha1(path: &Path) -> Result<String, String> {
    let mut cmd = command("/usr/bin/shasum");
    cmd.args(["-a", "1"]).arg(path);
    let output = capture(cmd, "fingerprint the signed certificate")?;
    let hash = output.split_whitespace().next().unwrap_or_default();
    if hash.len() != 40 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Invalid signing certificate fingerprint.".to_string());
    }
    Ok(hash.to_ascii_uppercase())
}

fn plist_value(path: &Path, key: &str) -> Result<String, String> {
    let mut cmd = command("/usr/bin/plutil");
    cmd.args(["-extract", key, "raw", "-o", "-"]).arg(path);
    capture(cmd, "read the provisioning profile")
}

pub fn inspect_native_profile(path: &Path, temporary: &Path) -> Result<NativeMacProfile, String> {
    require_native_mac()?;
    if !path.is_file() || fs::metadata(path).map_err(|e| e.to_string())?.len() > 2 * 1024 * 1024 {
        return Err("Choose a provisioning profile smaller than 2 MB.".to_string());
    }
    let mut decode = command("/usr/bin/security");
    decode.args(["cms", "-D", "-i"]).arg(path);
    let decoded = capture(decode, "decode the provisioning profile")?;
    fs::write(temporary, decoded).map_err(|e| e.to_string())?;
    let result = (|| {
        let uuid = plist_value(temporary, "UUID")?;
        let team_identifier = plist_value(temporary, "TeamIdentifier.0")?;
        let application_identifier = plist_value(temporary, "Entitlements.application-identifier")?;
        let expires_at = plist_value(temporary, "ExpirationDate")?;
        if !valid_profile_uuid(&uuid) || !valid_native_team(&team_identifier) {
            return Err("This provisioning profile has invalid identifiers.".to_string());
        }
        let mut expiry = command("/bin/date");
        expiry.args(["-u", "-j", "-f", "%Y-%m-%dT%H:%M:%SZ", &expires_at, "+%s"]);
        let epoch = capture(expiry, "read the profile expiry")
            .or_else(|_| {
                captured(
                    "/bin/date",
                    &["-u", "-j", "-f", "%Y-%m-%d %H:%M:%S %z", &expires_at, "+%s"],
                    "read the profile expiry",
                )
            })?
            .parse::<u64>()
            .map_err(|_| "The profile expiry is invalid.".to_string())?;
        if epoch
            <= SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        {
            return Err("This provisioning profile has expired.".to_string());
        }
        if plist_value(temporary, "Entitlements.get-task-allow")
            .ok()
            .as_deref()
            == Some("true")
            || plist_value(temporary, "ProvisionsAllDevices")
                .ok()
                .as_deref()
                == Some("true")
            || plist_value(temporary, "ProvisionedDevices.0").is_ok()
        {
            return Err("Choose an App Store distribution profile; development, ad hoc, and enterprise profiles are not supported by this archive recipe.".to_string());
        }
        let mut certificate_sha1s = Vec::new();
        for index in 0..20 {
            let Ok(certificate) = plist_value(temporary, &format!("DeveloperCertificates.{index}"))
            else {
                break;
            };
            let mut decoder = command("/usr/bin/base64");
            decoder.arg("-D");
            let binary = capture_bytes_with_input(
                decoder,
                certificate.as_bytes(),
                "decode the profile certificate",
            )?;
            let mut digest = command("/usr/bin/shasum");
            digest.args(["-a", "1"]);
            let hash =
                capture_bytes_with_input(digest, &binary, "fingerprint the profile certificate")?;
            let hash = String::from_utf8_lossy(&hash)
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            if hash.len() != 40 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err("Invalid profile certificate fingerprint.".to_string());
            }
            certificate_sha1s.push(hash);
        }
        if certificate_sha1s.is_empty() {
            return Err("This profile has no signing certificate.".to_string());
        }
        Ok(NativeMacProfile {
            uuid,
            team_identifier,
            application_identifier,
            expires_at,
            expires_at_epoch_seconds: epoch,
            sha256: native_sha256(path)?,
            certificate_sha1s,
        })
    })();
    let _ = fs::remove_file(temporary);
    result
}

fn capture_bytes_with_input(
    mut cmd: Command,
    input: &[u8],
    label: &str,
) -> Result<Vec<u8>, String> {
    capture_output(&mut cmd, Some(input), label, Duration::from_secs(30))
}

pub fn valid_native_team(team: &str) -> bool {
    team.len() == 10
        && team
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

fn valid_native_bundle(bundle: &str) -> bool {
    !bundle.is_empty()
        && bundle.len() <= 255
        && bundle.contains('.')
        && bundle.split('.').all(|part| {
            !part.is_empty() && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

/// Stream bounded, sanitized output and stop the entire native process group on cancellation.
fn run<F>(
    mut cmd: Command,
    label: &str,
    secrets: &[String],
    tail: &mut Vec<String>,
    on_line: &mut F,
) -> Result<(), String>
where
    F: FnMut(String),
{
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let scope = current_scope();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|e| format!("Could not {label}: {e}"))?;
    let pid = child.id();
    let (sender, receiver) = mpsc::sync_channel::<String>(128);
    let mut readers = Vec::new();
    for pipe in [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let sender = sender.clone();
        readers.push(thread::spawn(move || {
            let mut reader = BufReader::new(pipe);
            let mut oversized = false;
            loop {
                let mut buffer = Vec::new();
                // Bound an individual line before allocating; excessively long output is split.
                let read = reader
                    .by_ref()
                    .take(16 * 1024)
                    .read_until(b'\n', &mut buffer);
                match read {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if buffer.len() == 16 * 1024 && buffer.last() != Some(&b'\n') {
                    oversized = true;
                    continue;
                }
                if oversized {
                    oversized = false;
                    if sender
                        .send("[buildbridge omitted an oversized log line]".to_string())
                        .is_err()
                    {
                        break;
                    }
                    continue;
                }
                if sender
                    .send(String::from_utf8_lossy(&buffer).into_owned())
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    drop(sender);
    let mut status = None;
    let mut cancelled = false;
    let mut output_done = false;
    loop {
        if !cancelled && scope.as_ref().is_some_and(|s| s.is_cancelled()) {
            cancelled = true;
            // Stopped before the leader is reaped, while its group ID is still reserved.
            #[cfg(unix)]
            kill_process_group(pid);
            let _ = child.kill();
        }
        let received = if output_done {
            thread::sleep(Duration::from_millis(100));
            Err(mpsc::RecvTimeoutError::Timeout)
        } else {
            receiver.recv_timeout(Duration::from_millis(100))
        };
        match received {
            Ok(line) => {
                let line = redact_native_log(&line, secrets);
                if !line.is_empty() {
                    tail.push(line.clone());
                    if tail.len() > 160 {
                        tail.remove(0);
                    }
                    on_line(line);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => output_done = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if status.is_none() {
            status = child.try_wait().map_err(|e| e.to_string())?;
            if status.is_some() {
                // The leader is reaped; only members that outlived it still hold its group.
                #[cfg(unix)]
                kill_process_group_survivors(pid);
            }
        }
        if output_done && status.is_some() {
            break;
        }
    }
    let status = status
        .map(Ok)
        .unwrap_or_else(|| child.wait())
        .map_err(|e| e.to_string())?;
    for reader in readers {
        let _ = reader.join();
    }
    if cancelled {
        return Err("Stopped.".to_string());
    }
    if !status.success() {
        return Err(format!(
            "Could not {label}.\n{}",
            tail.iter()
                .rev()
                .take(25)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    Ok(())
}

pub fn redact_native_log(line: &str, secrets: &[String]) -> String {
    let mut line = line.to_string();
    let mut secrets = secrets.iter().filter(|v| !v.is_empty()).collect::<Vec<_>>();
    secrets.sort_by_key(|v| std::cmp::Reverse(v.len()));
    for secret in secrets {
        line = line.replace(secret, "[redacted]");
    }
    sanitize_build_log_line(&line)
}

pub fn checkout_native_commit<F>(
    repository: &str,
    commit: &str,
    directory: &Path,
    mut on_line: F,
) -> Result<(), String>
where
    F: FnMut(String),
{
    require_native_mac()?;
    if !valid_native_repository(repository) || !valid_native_commit(commit) {
        return Err("Choose an approved repository and a complete Git commit ID.".to_string());
    }
    fs::create_dir(directory).map_err(|e| format!("Could not create an isolated checkout: {e}"))?;
    let mut tail = Vec::new();
    let git = |args: &[&str], tail: &mut Vec<String>, on_line: &mut F| {
        let mut cmd = command("/usr/bin/git");
        cmd.current_dir(directory)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(agent) = std::env::var_os("SSH_AUTH_SOCK") {
            cmd.env("SSH_AUTH_SOCK", agent);
        }
        run(cmd, "check out the approved commit", &[], tail, on_line)
    };
    git(
        &["init", "--quiet", native_git_object_format(commit)],
        &mut tail,
        &mut on_line,
    )?;
    match git(
        &native_fetch_args(repository, Some(commit)),
        &mut tail,
        &mut on_line,
    ) {
        Ok(()) => git(&native_checkout_args("FETCH_HEAD"), &mut tail, &mut on_line)?,
        Err(error) if fetch_needs_full_history(&error) => {
            on_line(
                "This server does not serve commits by ID; fetching its branches instead."
                    .to_string(),
            );
            git(
                &native_fetch_args(repository, None),
                &mut tail,
                &mut on_line,
            )?;
            git(&native_checkout_args(commit), &mut tail, &mut on_line)?;
        }
        Err(error) => return Err(error),
    }
    let mut cmd = command("/usr/bin/git");
    cmd.current_dir(directory).args(["rev-parse", "HEAD"]);
    if !capture(cmd, "verify the source commit")?.eq_ignore_ascii_case(commit) {
        return Err("The checked-out commit does not match the requested source.".to_string());
    }
    if directory.join(".gitmodules").exists() {
        return Err("Native shared builds do not fetch submodules yet. Use a project whose approved source contains everything required.".to_string());
    }
    Ok(())
}

fn native_git_object_format(commit: &str) -> &'static str {
    if commit.len() == 64 {
        "--object-format=sha256"
    } else {
        "--object-format=sha1"
    }
}

/// A pinned fetch asks for the one commit. Servers without `uploadpack.allowReachableSHA1InWant`
/// or `allowAnySHA1InWant` refuse that, and get a fetch of every branch instead; the checkout
/// then names the commit itself and the `rev-parse` comparison keeps the result pinned.
fn native_fetch_args<'a>(repository: &'a str, commit: Option<&'a str>) -> Vec<&'a str> {
    let mut args = vec!["-c", "core.hooksPath=/dev/null", "fetch"];
    match commit {
        Some(commit) => args.extend(["--depth=1", "--no-tags", "--", repository, commit]),
        None => args.extend([
            "--no-tags",
            "--",
            repository,
            "+refs/heads/*:refs/remotes/origin/*",
        ]),
    }
    args
}

fn native_checkout_args(target: &str) -> [&str; 6] {
    [
        "-c",
        "core.hooksPath=/dev/null",
        "checkout",
        "--quiet",
        "--detach",
        target,
    ]
}

/// Git's two refusals of a fetch by object ID: `upload-pack: not our ref` from servers that
/// only serve advertised tips, and the longer text when the object is reachable but the
/// server does not allow requests for unadvertised objects.
fn fetch_needs_full_history(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("server does not allow request for unadvertised object")
        || error.contains("not our ref")
}

pub struct NativeBuildRecipe<'a> {
    pub project: &'a Path,
    pub staging: &'a Path,
    pub output: &'a Path,
    pub developer_directory: &'a str,
    pub bundle_identifier: &'a str,
    pub team: &'a str,
    pub signing: Option<(&'a str, &'a NativeMacProfile)>,
    pub environment: &'a [(String, String)],
    pub secrets: &'a [String],
}

/// Fixed Capacitor recipe: install locked JS deps, build web assets, sync iOS, enforce the
/// committed native lock, then unsigned compilation or a manually signed App Store archive.
pub fn run_native_recipe<F>(
    recipe: NativeBuildRecipe<'_>,
    mut on_progress: F,
) -> Result<(Vec<AppleArchiveArtifact>, Vec<String>), String>
where
    F: FnMut(&str, &str, Option<String>),
{
    require_native_mac()?;
    let NativeBuildRecipe {
        project,
        staging,
        output,
        developer_directory,
        bundle_identifier,
        team,
        signing,
        environment,
        secrets,
    } = recipe;
    if (signing.is_some() && !valid_native_team(team)) || !valid_native_bundle(bundle_identifier) {
        return Err("The approved Apple team or bundle identifier is invalid.".to_string());
    }
    let mut tail = Vec::new();
    let lock = project.join("ios/App/Podfile.lock");
    let before = fs::read(&lock).map_err(|e| e.to_string())?;
    let mut build_command =
        |program: OsString, args: Vec<OsString>, cwd: &Path, phase: &str, label: &str| {
            on_progress(phase, label, None);
            let mut cmd = command(program);
            cmd.current_dir(cwd)
                .args(args)
                .envs(environment.iter().map(|(k, v)| (k, v)))
                .env("DEVELOPER_DIR", developer_directory)
                .env("CYPRESS_INSTALL_BINARY", "0");
            run(cmd, label, secrets, &mut tail, &mut |line| {
                on_progress(phase, label, Some(line))
            })
        };
    let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
    build_command(
        "pnpm".into(),
        args(&["install", "--frozen-lockfile", "--prefer-offline"]),
        project,
        "dependencies",
        "Install locked JavaScript dependencies",
    )?;
    build_command(
        project.join("node_modules/.bin/vp").into_os_string(),
        args(&["build"]),
        project,
        "web",
        "Build web assets",
    )?;
    build_command(
        project.join("node_modules/.bin/cap").into_os_string(),
        args(&["sync", "ios"]),
        project,
        "sync",
        "Prepare the Capacitor iOS project",
    )?;
    build_command(
        "pod".into(),
        args(&["install", "--deployment", "--no-ansi"]),
        &project.join("ios/App"),
        "pods",
        "Install locked CocoaPods",
    )?;
    if fs::read(&lock).map_err(|e| e.to_string())? != before {
        return Err("Preparing iOS changed Podfile.lock. Commit the updated native lockfile in your project, then build that commit.".to_string());
    }
    let archive = staging.join("App.xcarchive");
    let mut xcode = args(&["-workspace"]);
    xcode.push(project.join("ios/App/App.xcworkspace").into_os_string());
    xcode.extend(args(&[
        "-scheme",
        "App",
        "-configuration",
        if signing.is_some() {
            "Release"
        } else {
            "Debug"
        },
        "-destination",
        "generic/platform=iOS",
        "-derivedDataPath",
    ]));
    xcode.push(staging.join("DerivedData").into_os_string());
    xcode.push("COMPILER_INDEX_STORE_ENABLE=NO".into());
    if let Some((identity, profile)) = signing {
        if profile.team_identifier != team
            || profile.application_identifier != format!("{team}.{bundle_identifier}")
            || !profile
                .certificate_sha1s
                .iter()
                .any(|sha| sha.eq_ignore_ascii_case(identity))
        {
            return Err(
                "The owner's signing identity and profile do not match this approved project."
                    .to_string(),
            );
        }
        xcode.extend(args(&["-archivePath"]));
        xcode.push(archive.clone().into_os_string());
        let target = native_archive_target(project, developer_directory, bundle_identifier)?;
        let settings = staging.join("Signing.xcconfig");
        fs::write(
            &settings,
            apple_archive_signing_xcconfig(&target, team, identity, &profile.uuid),
        )
        .map_err(|error| error.to_string())?;
        xcode.extend([
            "-xcconfig".into(),
            settings.into_os_string(),
            "archive".into(),
        ]);
    } else {
        xcode.extend(args(&[
            "CODE_SIGNING_ALLOWED=NO",
            "CODE_SIGNING_REQUIRED=NO",
            "build",
        ]));
    }
    build_command(
        "/usr/bin/xcodebuild".into(),
        xcode,
        project,
        "build",
        if signing.is_some() {
            "Archive the signed iOS app"
        } else {
            "Compile the unsigned iOS app"
        },
    )?;
    let Some((identity, profile)) = signing else {
        return Ok((Vec::new(), tail));
    };
    let export_options = staging.join("ExportOptions.plist");
    fs::write(
        &export_options,
        apple_export_options_plist(team, bundle_identifier, &profile.uuid, identity),
    )
    .map_err(|e| e.to_string())?;
    let export = staging.join("Export");
    build_command(
        "/usr/bin/xcodebuild".into(),
        vec![
            "-exportArchive".into(),
            "-archivePath".into(),
            archive.clone().into_os_string(),
            "-exportOptionsPlist".into(),
            export_options.into_os_string(),
            "-exportPath".into(),
            export.clone().into_os_string(),
        ],
        project,
        "export",
        "Export the App Store IPA",
    )?;
    verify_native_app(
        &native_application(&archive.join("Products/Applications"))?,
        bundle_identifier,
        identity,
        profile,
        &staging.join("archive-certificate"),
    )?;
    let ipas = fs::read_dir(&export)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "ipa") && path.is_file())
        .collect::<Vec<_>>();
    if ipas.len() != 1 {
        return Err("Xcode did not export exactly one IPA.".to_string());
    }
    let unpacked = staging.join("VerifiedIPA");
    build_command(
        "/usr/bin/ditto".into(),
        vec![
            "-x".into(),
            "-k".into(),
            ipas[0].clone().into_os_string(),
            unpacked.clone().into_os_string(),
        ],
        project,
        "verify",
        "Verify the exported IPA",
    )?;
    verify_native_app(
        &native_application(&unpacked.join("Payload"))?,
        bundle_identifier,
        identity,
        profile,
        &staging.join("ipa-certificate"),
    )?;
    fs::create_dir(output).map_err(|e| e.to_string())?;
    let ipa_path = output.join("App-AppStore.ipa");
    fs::copy(&ipas[0], &ipa_path).map_err(|e| e.to_string())?;
    let archive_path = output.join("App.xcarchive.zip");
    build_command(
        "/usr/bin/ditto".into(),
        vec![
            "-c".into(),
            "-k".into(),
            "--keepParent".into(),
            archive.into_os_string(),
            archive_path.clone().into_os_string(),
        ],
        project,
        "artifacts",
        "Package the signed archive",
    )?;
    let artifacts = [ipa_path, archive_path]
        .into_iter()
        .map(|path| {
            Ok(AppleArchiveArtifact {
                bytes: fs::metadata(&path).map_err(|e| e.to_string())?.len(),
                sha256: native_sha256(&path)?,
                path: path.to_string_lossy().into_owned(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((artifacts, tail))
}

fn verify_native_app(
    app: &Path,
    bundle: &str,
    identity: &str,
    profile: &NativeMacProfile,
    certificate: &Path,
) -> Result<(), String> {
    if !app.is_dir() {
        return Err("The signed artifact does not contain the expected app.".to_string());
    }
    let mut verify = command("/usr/bin/codesign");
    verify.args(["--verify", "--deep", "--strict"]).arg(app);
    capture(verify, "verify the app signature")?;
    if plist_value(&app.join("Info.plist"), "CFBundleIdentifier")? != bundle {
        return Err(
            "The signed artifact bundle identifier differs from the approved project.".to_string(),
        );
    }
    let mut extract = command("/usr/bin/codesign");
    extract
        .args(["-d", "--extract-certificates"])
        .arg(certificate)
        .arg(app);
    capture(extract, "inspect the actual signing certificate")?;
    let certificate_zero = PathBuf::from(format!("{}0", certificate.to_string_lossy()));
    if !native_sha1(&certificate_zero)?.eq_ignore_ascii_case(identity) {
        return Err("The artifact was not signed with the owner's approved identity.".to_string());
    }
    if native_sha256(&app.join("embedded.mobileprovision"))? != profile.sha256 {
        return Err(
            "The signed artifact does not contain the owner's approved provisioning profile."
                .to_string(),
        );
    }
    Ok(())
}

fn native_application(directory: &Path) -> Result<PathBuf, String> {
    let mut apps = Vec::new();
    for entry in fs::read_dir(directory)
        .map_err(|_| "The signed artifact has no application directory.".to_string())?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "app")
            && entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
        {
            apps.push(entry.path());
        }
    }
    if apps.len() != 1 {
        return Err("The signed artifact must contain exactly one app; its bundle and signing identity will be verified.".to_string());
    }
    Ok(apps.remove(0))
}

fn native_archive_target(project: &Path, developer: &str, bundle: &str) -> Result<String, String> {
    let mut cmd = command("/usr/bin/xcodebuild");
    cmd.current_dir(project)
        .env("DEVELOPER_DIR", developer)
        .arg("-workspace")
        .arg(project.join("ios/App/App.xcworkspace"))
        .args([
            "-scheme",
            "App",
            "-configuration",
            "Release",
            "-destination",
            "generic/platform=iOS",
            "-showBuildSettings",
            "-json",
        ]);
    let settings: serde_json::Value =
        serde_json::from_str(&capture(cmd, "inspect the selected app target")?)
            .map_err(|_| "Xcode did not return valid app build settings.".to_string())?;
    let applications = settings
        .as_array()
        .ok_or_else(|| "Xcode did not return an app target.".to_string())?
        .iter()
        .filter_map(|entry| entry.get("buildSettings"))
        .filter(|settings| {
            settings
                .get("PRODUCT_BUNDLE_IDENTIFIER")
                .and_then(serde_json::Value::as_str)
                == Some(bundle)
                && settings
                    .get("WRAPPER_EXTENSION")
                    .and_then(serde_json::Value::as_str)
                    == Some("app")
        })
        .collect::<Vec<_>>();
    if applications.len() != 1 {
        return Err("The selected scheme must resolve to exactly one app with the approved bundle identifier.".to_string());
    }
    let target = applications[0]
        .get("TARGET_NAME")
        .and_then(serde_json::Value::as_str)
        .filter(|name| {
            !name.is_empty()
                && name.len() <= 128
                && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        })
        .ok_or_else(|| {
            "The app target name cannot be represented in target-scoped signing settings."
                .to_string()
        })?;
    Ok(target.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_requires_a_complete_commit_and_safe_network_remote() {
        assert!(valid_native_commit(&"a".repeat(40)));
        assert!(valid_native_commit(&"B".repeat(64)));
        for commit in [
            "main",
            "HEAD~1",
            "--upload-pack=evil",
            "abc",
            &"g".repeat(40),
        ] {
            assert!(!valid_native_commit(commit));
        }
        for remote in [
            "https://github.com/team/app.git",
            "git@github.com:team/app.git",
            "ssh://git@example.com/team/app.git",
        ] {
            assert!(valid_native_repository(remote), "{remote}");
        }
        for remote in [
            "ext::sh -c evil",
            "file:///tmp/project",
            "/tmp/project",
            "https://token@example.com/app",
            "git@example.com:app;evil",
            "https://example.com/repo?token=secret",
            "-evil",
            "https://",
        ] {
            assert!(!valid_native_repository(remote), "{remote}");
        }
    }

    #[test]
    fn identity_probe_excludes_development_and_invalid_hashes() {
        let hash = "A".repeat(40);
        let identities = parse_identities(&format!(
            "  1) {hash} \"Apple Distribution: Owner (TEAM123456)\"\n  2) {hash} \"Apple Development: Owner\"\n 3) BAD \"Apple Distribution: Bad\"\n  2 valid identities found"
        ));
        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].sha1, hash);
    }

    #[test]
    fn source_checkout_uses_the_requested_commit_object_format() {
        assert_eq!(
            native_git_object_format(&"a".repeat(40)),
            "--object-format=sha1"
        );
        assert_eq!(
            native_git_object_format(&"b".repeat(64)),
            "--object-format=sha256"
        );
    }

    #[test]
    fn secrets_are_removed_before_log_truncation() {
        let secret = "s".repeat(1200);
        assert_eq!(
            redact_native_log(&format!("token={secret}\u{1b}\n"), &[secret]),
            "token=[redacted]"
        );
        assert_eq!(
            redact_native_log("abcdef abc", &["abc".into(), "abcdef".into()]),
            "[redacted] [redacted]"
        );
    }

    #[test]
    fn unsupported_host_is_not_advertised_as_ready() {
        if !cfg!(target_os = "macos") {
            let status = probe_native_mac();
            assert!(status.xcode_version.is_none());
            assert_eq!(status.issues.len(), 1);
        }
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_native_build_stops_child_process_group() {
        let scope = OperationScope::new();
        let entered = Arc::clone(&scope);
        let worker = thread::spawn(move || {
            let _scope = enter_operation(entered);
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "sleep 30 & wait"]);
            run(
                command,
                "test cancellation",
                &[],
                &mut Vec::new(),
                &mut |_| {},
            )
        });
        thread::sleep(Duration::from_millis(100));
        let start = Instant::now();
        scope.cancel();
        assert!(worker.join().unwrap().is_err());
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_still_kills_a_term_ignoring_child_after_its_output_closes() {
        let scope = OperationScope::new();
        let entered = Arc::clone(&scope);
        let worker = thread::spawn(move || {
            let _scope = enter_operation(entered);
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "trap '' TERM; exec >/dev/null 2>&1; sleep 30"]);
            run(
                command,
                "test closed output cancellation",
                &[],
                &mut Vec::new(),
                &mut |_| {},
            )
        });
        thread::sleep(Duration::from_millis(150));
        let start = Instant::now();
        scope.cancel();
        assert_eq!(worker.join().unwrap().unwrap_err(), "Stopped.");
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn probe_deadline_stops_hung_children() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 & wait"]);
        let start = Instant::now();
        assert!(capture_with_timeout(&mut command, "probe", Duration::from_millis(100)).is_err());
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn oversized_log_lines_are_omitted_instead_of_splitting_secrets() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf '%20000s\\n' value"]);
        let mut lines = Vec::new();
        run(
            command,
            "read build output",
            &[],
            &mut Vec::new(),
            &mut |line| lines.push(line),
        )
        .unwrap();
        assert_eq!(lines, ["[buildbridge omitted an oversized log line]"]);
    }

    #[test]
    fn renamed_product_is_found_but_multiple_top_level_apps_are_refused() {
        let directory = std::env::temp_dir().join(format!(
            "buildbridge-native-app-name-{}",
            std::process::id()
        ));
        fs::create_dir_all(directory.join("MyRenamedApp.app")).unwrap();
        assert_eq!(
            native_application(&directory).unwrap(),
            directory.join("MyRenamedApp.app")
        );
        fs::create_dir(directory.join("Another.app")).unwrap();
        assert!(native_application(&directory).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn bundle_identifiers_are_reverse_dns_labels_only() {
        for bundle in ["com.example.app", "nz.co.think-solar.App2", "a.b"] {
            assert!(valid_native_bundle(bundle), "{bundle}");
        }
        for bundle in [
            "",
            "app",
            ".app",
            "com..app",
            "com.example.app.",
            "com.example.app/../x",
            "com.example.app;rm",
            "com.example.app x",
            "com.example.äpp",
            "com.example.app$(id)",
            "com.example_app",
        ] {
            assert!(!valid_native_bundle(bundle), "{bundle:?}");
        }
        assert!(valid_native_bundle(&format!("a.{}", "b".repeat(253))));
        assert!(!valid_native_bundle(&format!("a.{}", "b".repeat(254))));
    }

    #[cfg(unix)]
    #[test]
    fn native_tools_run_from_the_root_with_an_allowlisted_environment() {
        let mut env = command("/usr/bin/env");
        assert_eq!(env.get_current_dir(), Some(std::path::Path::new("/")));
        let output = env.arg("-0").output().unwrap();
        assert!(output.status.success());
        let variables: HashMap<String, String> = output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                let entry = String::from_utf8_lossy(entry);
                let (name, value) = entry.split_once('=').unwrap();
                (name.to_string(), value.to_string())
            })
            .collect();
        // Cargo sets these for every test process; none of them may reach a native tool.
        assert!(
            !variables.contains_key("CARGO_MANIFEST_DIR"),
            "{variables:?}"
        );
        assert!(!variables.contains_key("CARGO_PKG_NAME"), "{variables:?}");
        for name in variables.keys() {
            assert!(
                [
                    "HOME",
                    "USER",
                    "LOGNAME",
                    "TMPDIR",
                    "DEVELOPER_DIR",
                    "PATH",
                    "LANG"
                ]
                .contains(&name.as_str()),
                "{name} leaked into the native tool environment"
            );
        }
        assert_eq!(variables["LANG"], "en_US.UTF-8");
        assert!(
            variables["PATH"]
                .starts_with("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
        );
    }

    struct FakeHome(PathBuf);

    impl FakeHome {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "buildbridge-native-home-{label}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn dir(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(&path).unwrap();
            path
        }

        fn alias(&self, name: &str, value: &str) {
            let path = self.0.join(".nvm/alias").join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("{value}\n")).unwrap();
        }
    }

    impl Drop for FakeHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn user_tool_directories_are_the_version_managers_existing_defaults_in_order() {
        let home = FakeHome::new("managers");
        assert!(user_tool_directories(&home.0).is_empty());
        let volta = home.dir(".volta/bin");
        let fnm = home.dir(".local/share/fnm/aliases/default/bin");
        let local = home.dir(".local/bin");
        home.dir(".nvm/versions/node/v20.19.0/bin");
        let node = home.dir(".nvm/versions/node/v22.10.0/bin");
        home.dir(".nvm/versions/node/v22.9.0/bin");
        // Files, prefixes, and other managers' missing homes are not directories on PATH.
        fs::write(home.0.join(".fnm"), "not a directory").unwrap();
        assert_eq!(user_tool_directories(&home.0), [volta, fnm, node, local]);
    }

    #[test]
    fn nvm_default_alias_selects_an_installed_version_or_the_highest_one() {
        let home = FakeHome::new("nvm");
        let nvm = home.0.join(".nvm");
        for version in ["v20.19.0", "v22.1.0", "v22.10.0", "v9.11.2"] {
            home.dir(&format!(".nvm/versions/node/{version}/bin"));
        }
        let bin = |version: &str| nvm.join("versions/node").join(version).join("bin");
        // No alias: the highest version, compared numerically rather than as text.
        assert_eq!(nvm_default_bin(&nvm), Some(bin("v22.10.0")));
        home.alias("default", "v22.1.0");
        assert_eq!(nvm_default_bin(&nvm), Some(bin("v22.1.0")));
        home.alias("default", "20");
        assert_eq!(nvm_default_bin(&nvm), Some(bin("v20.19.0")));
        home.alias("default", "22");
        assert_eq!(nvm_default_bin(&nvm), Some(bin("v22.10.0")));
        // nvm mirrors the LTS lines as chained aliases, `lts/*` being a file of that name.
        home.alias("default", "lts/*");
        home.alias("lts/*", "lts/iron");
        home.alias("lts/iron", "v20.19.0");
        assert_eq!(nvm_default_bin(&nvm), Some(bin("v20.19.0")));
        // Unresolvable, unsafe, or uninstalled aliases fall back to the highest version.
        for alias in ["v24.0.0", "node", "../../etc/passwd", "/tmp/x", ""] {
            home.alias("default", alias);
            assert_eq!(nvm_default_bin(&nvm), Some(bin("v22.10.0")), "{alias:?}");
        }
        home.alias("default", "loop");
        home.alias("loop", "loop");
        assert_eq!(nvm_default_bin(&nvm), Some(bin("v22.10.0")));
        assert_eq!(nvm_default_bin(&home.0.join(".nvm-missing")), None);
    }

    #[test]
    fn host_path_appends_existing_user_tool_directories_after_the_inherited_entries_once() {
        let home = FakeHome::new("path");
        let volta = home.dir(".volta/bin");
        let local = home.dir(".local/bin");
        let inherited = OsString::from(format!("/usr/bin:/bin:{}", local.display()));
        let path = host_path_for(Some(&home.0), Some(&inherited));
        assert_eq!(
            path.to_string_lossy(),
            format!(
                "{SYSTEM_PATH}:/usr/bin:/bin:{}:{}",
                local.display(),
                volta.display()
            )
        );
        assert_eq!(host_path_for(None, None).to_string_lossy(), SYSTEM_PATH);
    }

    #[test]
    fn fetch_by_commit_refusals_select_the_full_history_fallback() {
        for error in [
            "Could not check out the approved commit.\nerror: Server does not allow request for unadvertised object 0123abcd",
            "fatal: remote error: upload-pack: not our ref 0123abcd",
        ] {
            assert!(fetch_needs_full_history(error), "{error}");
        }
        for error in [
            "fatal: could not read Username for 'https://example.com': terminal prompts disabled",
            "ssh: Could not resolve hostname example.com",
            "Stopped.",
            "",
        ] {
            assert!(!fetch_needs_full_history(error), "{error}");
        }
        let repository = "https://example.com/team/app.git";
        let commit = "a".repeat(40);
        let pinned = native_fetch_args(repository, Some(&commit));
        assert_eq!(
            pinned,
            [
                "-c",
                "core.hooksPath=/dev/null",
                "fetch",
                "--depth=1",
                "--no-tags",
                "--",
                repository,
                &commit
            ]
        );
        let full = native_fetch_args(repository, None);
        assert!(!full.iter().any(|arg| arg.starts_with("--depth")));
        assert_eq!(
            &full[full.len() - 3..],
            ["--", repository, "+refs/heads/*:refs/remotes/origin/*"]
        );
        assert_eq!(native_checkout_args(&commit)[5], commit);
    }

    #[cfg(unix)]
    #[test]
    fn helper_exit_still_stops_group_members_that_hold_its_output() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 & exit 0"]);
        let start = Instant::now();
        assert_eq!(
            capture_with_timeout(&mut command, "probe", Duration::from_secs(5)).unwrap(),
            ""
        );
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn a_reaped_leader_has_its_group_signalled_only_while_members_remain() {
        use std::os::unix::process::CommandExt;
        let spawn = |script: &str| {
            Command::new("/bin/sh")
                .args(["-c", script])
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        };
        // Nothing outlived this leader: once reaped, its ID is free for an unrelated process.
        let mut child = spawn("exit 0");
        let pid = child.id();
        child.wait().unwrap();
        assert!(!process_group_alive(pid));
        // A member that inherited the leader's output keeps the group, and its ID, alive.
        let mut child = spawn("sleep 30 & exit 0");
        let pid = child.id();
        let stdout = child.stdout.take().unwrap();
        let reader = thread::spawn(move || std::io::read_to_string(stdout));
        child.wait().unwrap();
        assert!(process_group_alive(pid));
        let start = Instant::now();
        kill_process_group_survivors(pid);
        assert_eq!(reader.join().unwrap().unwrap(), "");
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn helper_input_is_delivered_and_still_bounded_by_the_deadline() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "cat"]);
        assert_eq!(
            capture_output(
                &mut command,
                Some(b"certificate"),
                "echo",
                Duration::from_secs(5)
            )
            .unwrap(),
            b"certificate"
        );
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec >/dev/null; sleep 30 & wait"]);
        let start = Instant::now();
        let error = capture_output(&mut command, Some(b"x"), "hang", Duration::from_millis(100))
            .unwrap_err();
        assert!(error.contains("time limit"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
