//! The SSH bridge to the guest: commands, key installation, host keys, and quoting.

use super::*;

pub fn valid_guest_username(username: &str) -> bool {
    let mut chars = username.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    username.len() <= 32
        && (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

pub(crate) fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub fn fingerprint_host_key_line(line: &str) -> Result<String, ProviderError> {
    let parsed = parse_host_key_line(line)?;
    let mut child = Command::new("ssh-keygen")
        .args(["-lf", "-", "-E", "sha256"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not run ssh-keygen; install OpenSSH client tools: {error}"
            ))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not send the host key to ssh-keygen".to_string())
    })?;
    writeln!(stdin, "{} {}", parsed.algorithm, parsed.key).map_err(|error| {
        ProviderError::GuestBridge(format!("could not fingerprint the SSH host key: {error}"))
    })?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not fingerprint the SSH host key: {error}"))
    })?;

    if !output.status.success() {
        return Err(ProviderError::GuestBridge(
            "the SSH host key is malformed".to_string(),
        ));
    }

    clean_output(&output.stdout)
        .split_whitespace()
        .find(|field| field.starts_with("SHA256:"))
        .map(str::to_string)
        .ok_or_else(|| {
            ProviderError::GuestBridge("ssh-keygen returned an invalid fingerprint".to_string())
        })
}

#[derive(Debug)]
pub(crate) struct ParsedHostKey<'a> {
    pub(crate) host: &'a str,
    pub(crate) algorithm: &'a str,
    pub(crate) key: &'a str,
}

pub(crate) fn parse_host_key_line(line: &str) -> Result<ParsedHostKey<'_>, ProviderError> {
    let mut fields = line.split_whitespace();
    let host = fields.next();
    let algorithm = fields.next();
    let key = fields.next();

    if host.is_none()
        || algorithm != Some("ssh-ed25519")
        || key.is_none()
        || fields.next().is_some()
    {
        return Err(ProviderError::GuestBridge(
            "the SSH host key pin is invalid".to_string(),
        ));
    }

    Ok(ParsedHostKey {
        host: host.expect("checked above"),
        algorithm: algorithm.expect("checked above"),
        key: key.expect("checked above"),
    })
}

pub(crate) fn guest_port_reachable(ssh_port: u16) -> bool {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), ssh_port);

    TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok()
}

pub(crate) fn run_guest_command(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    command: &str,
) -> Result<String, ProviderError> {
    let output = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(command)
        .tracked_output()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not run ssh; install OpenSSH client tools: {error}"
            ))
        })?;

    if output.status.success() {
        Ok(clean_output(&output.stdout))
    } else {
        let message = clean_output(&output.stderr);
        Err(ProviderError::GuestBridge(if message.is_empty() {
            "SSH authentication failed; add the BuildBridge public key to the guest user"
                .to_string()
        } else {
            message
        }))
    }
}

pub(crate) fn guest_ssh_command(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Command {
    let destination = format!("{username}@127.0.0.1");
    let mut command = Command::new("ssh");
    command
        .arg("-F")
        .arg("/dev/null")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=3",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "PasswordAuthentication=no",
            "-o",
            "KbdInteractiveAuthentication=no",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "LogLevel=ERROR",
            "-p",
            &ssh_port.to_string(),
            "-i",
        ])
        .arg(identity_path)
        .arg("-o")
        .arg(format!("UserKnownHostsFile={}", known_hosts_path.display()))
        .arg(destination);

    command
}

/// Environment variable the one-time key install hands the macOS password to `ssh` through.
/// Only the askpass helper reads it.
pub(crate) const GUEST_PASSWORD_ENV: &str = "BUILDBRIDGE_GUEST_PASSWORD";

/// The fixed askpass helper OpenSSH runs during the one-time key install. It only echoes the
/// password variable set on that one `ssh` process, so the password never becomes an
/// argument, a file, or a log line.
pub(crate) const GUEST_ASKPASS_SCRIPT: &str =
    "#!/bin/sh\nprintf '%s\\n' \"$BUILDBRIDGE_GUEST_PASSWORD\"\n";

/// A private, single-use directory holding the askpass helper for one key install. Dropping
/// it removes the helper again, whether or not `ssh` succeeded.
pub(crate) struct GuestAskpassHelper {
    pub(crate) dir: PathBuf,
}

impl GuestAskpassHelper {
    pub(crate) fn create() -> Result<Self, ProviderError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "buildbridge-askpass-{}-{nanos}",
            std::process::id()
        ));
        let failed = |error: std::io::Error| {
            ProviderError::GuestBridge(format!("could not prepare the password helper: {error}"))
        };

        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

            fs::DirBuilder::new()
                .mode(0o700)
                .create(&dir)
                .map_err(failed)?;
            let helper = Self { dir };
            let script = helper.script();
            fs::write(&script, GUEST_ASKPASS_SCRIPT).map_err(failed)?;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).map_err(failed)?;

            Ok(helper)
        }
        #[cfg(not(unix))]
        {
            let _ = (dir, failed);
            Err(ProviderError::GuestBridge(
                "the one-time key install needs a Unix host".to_string(),
            ))
        }
    }

    pub(crate) fn script(&self) -> PathBuf {
        self.dir.join("askpass")
    }
}

impl Drop for GuestAskpassHelper {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Returns whether a public key is the one BuildBridge generated: a single `ssh-ed25519` line
/// with base64 material and at most a plain comment, so it can be quoted into a guest command.
pub fn valid_guest_public_key(public_key: &str) -> bool {
    let mut fields = public_key.split(' ');
    let algorithm = fields.next();
    let Some(material) = fields.next() else {
        return false;
    };
    let comment_is_plain = match fields.next() {
        None => true,
        Some(comment) => {
            !comment.is_empty()
                && comment.chars().all(|character| {
                    character.is_ascii_alphanumeric()
                        || matches!(character, '-' | '_' | '.' | '@' | ':')
                })
        }
    };

    public_key.len() <= 2_048
        && algorithm == Some("ssh-ed25519")
        && !material.is_empty()
        && material.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '=')
        })
        && comment_is_plain
        && fields.next().is_none()
}

/// The askpass helper answers with one line, so the password has to be one.
pub(crate) fn valid_guest_password(password: &str) -> bool {
    !password.is_empty() && password.len() <= 512 && !password.chars().any(char::is_control)
}

/// The command the password session runs in the guest: append the key once, with the
/// permissions sshd insists on, and confirm. Idempotent, so a retry never duplicates the line.
pub(crate) fn guest_key_install_command(public_key: &str) -> String {
    let quoted = shell_single_quote(public_key);
    format!(
        "umask 077; key={quoted}; /bin/mkdir -p \"$HOME/.ssh\" && /bin/chmod 700 \"$HOME/.ssh\" && {{ /usr/bin/grep -qxF \"$key\" \"$HOME/.ssh/authorized_keys\" 2>/dev/null || /usr/bin/printf '%s\\n' \"$key\" >> \"$HOME/.ssh/authorized_keys\"; }} && /bin/chmod 600 \"$HOME/.ssh/authorized_keys\" && /usr/bin/printf authorized"
    )
}

/// The one `ssh` invocation that authenticates with a password instead of the key. It still
/// refuses anything but the pinned host identity, tries the password once, and gets it from
/// the askpass helper rather than a terminal or an agent.
pub(crate) fn guest_password_ssh_command(
    ssh_port: u16,
    username: &str,
    known_hosts_path: &Path,
    askpass_path: &Path,
) -> Command {
    let destination = format!("{username}@127.0.0.1");
    let mut command = Command::new("ssh");
    command
        .arg("-F")
        .arg("/dev/null")
        .args([
            "-o",
            "PubkeyAuthentication=no",
            "-o",
            "PasswordAuthentication=yes",
            "-o",
            "KbdInteractiveAuthentication=yes",
            "-o",
            "NumberOfPasswordPrompts=1",
            "-o",
            "ConnectTimeout=3",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "LogLevel=ERROR",
            "-p",
        ])
        .arg(ssh_port.to_string())
        .arg("-o")
        .arg(format!("UserKnownHostsFile={}", known_hosts_path.display()))
        .arg(destination)
        .env("SSH_ASKPASS", askpass_path)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env_remove("SSH_AUTH_SOCK");

    command
}

/// What a guest Terminal script left behind: whether the SSH session itself succeeded, and
/// everything it printed. The one-word status the script writes is in `stdout`.
pub(crate) struct GuestTerminalRun {
    pub(crate) status_success: bool,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

/// Runs a zsh script in a window of the guest's Terminal, for the things only a macOS TTY can
/// do — sudo reading a password that never leaves the guest. The script is written to a
/// short-lived command file, opened with `open -a Terminal`, and its status file polled for
/// `timeout_seconds`; `launch_failed` and `timeout` are printed as the result when the window
/// never opened or the script never finished. `on_waiting` is called once a second with the
/// elapsed time so the caller can say it is waiting.
/// A script for the guest's Terminal: where its files go, what it says, and how long to wait.
pub(crate) struct GuestTerminalScript<'a> {
    pub(crate) guest_cache: &'a str,
    pub(crate) status_path: &'a str,
    pub(crate) script_path: &'a str,
    pub(crate) script: &'a str,
    pub(crate) timeout_seconds: u64,
    /// How to describe a session that could not even be spawned.
    pub(crate) spawn_failure: &'a str,
}

pub(crate) fn run_guest_terminal_script(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    terminal: &GuestTerminalScript<'_>,
    on_waiting: &mut dyn FnMut(u64),
) -> Result<GuestTerminalRun, ProviderError> {
    let GuestTerminalScript {
        guest_cache,
        status_path,
        script_path,
        script,
        timeout_seconds,
        spawn_failure,
    } = *terminal;
    let quoted = shell_single_quote(script);
    let remote_command = format!(
        "/bin/mkdir -p '{guest_cache}'; /bin/rm -f '{status_path}' '{script_path}'; /usr/bin/printf '%s' {quoted} > '{script_path}'; /bin/chmod 700 '{script_path}'; if ! /usr/bin/open -a Terminal '{script_path}'; then /usr/bin/printf launch_failed; exit 0; fi; remaining={timeout_seconds}; while /bin/test ! -f '{status_path}' && /bin/test \"$remaining\" -gt 0; do /bin/sleep 1; remaining=$((remaining - 1)); done; if /bin/test -f '{status_path}'; then /bin/cat '{status_path}'; /bin/rm -f '{status_path}' '{script_path}'; else /usr/bin/printf timeout; fi"
    );
    let started_at = Instant::now();
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| ProviderError::GuestBridge(format!("{spawn_failure}: {error}")))?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture the guest Terminal output".to_string())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture the guest Terminal errors".to_string())
    })?;
    let stdout_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut stdout = stdout;
        let _ = std::io::Read::read_to_end(&mut stdout, &mut output);
        output
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut stderr = stderr;
        let _ = std::io::Read::read_to_end(&mut stderr, &mut output);
        output
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                on_waiting(started_at.elapsed().as_secs());
                thread::sleep(Duration::from_secs(1));
            }
            Err(error) => {
                return Err(ProviderError::GuestBridge(format!(
                    "could not monitor the guest Terminal: {error}"
                )));
            }
        }
    };

    Ok(GuestTerminalRun {
        status_success: status.success(),
        stdout: stdout_reader.join().unwrap_or_default(),
        stderr: stderr_reader.join().unwrap_or_default(),
    })
}
