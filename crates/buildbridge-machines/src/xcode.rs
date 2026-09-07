//! Xcode: importing the package, activating it, and authorizing the access key.

use super::*;

/// Installs the buildbridge public key into the guest user's `authorized_keys` over one
/// password-authenticated SSH session — the `ssh-copy-id` route. The session is pinned to the
/// already-trusted host key, so the password only ever reaches the machine whose fingerprint
/// was verified. The password is handed to `ssh` through the environment of that one process
/// and read back by a fixed askpass helper; it is never an argument, never written to disk,
/// and never part of an error. Every later guest operation authenticates with the key.
pub fn authorize_guest_key(
    ssh_port: u16,
    username: &str,
    public_key: &str,
    password: &str,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
    if !valid_guest_username(username) {
        return Err(ProviderError::GuestBridge(
            "the macOS short username is invalid".to_string(),
        ));
    }
    if ssh_port < 1024 {
        return Err(ProviderError::GuestBridge(
            "guest SSH port must be between 1024 and 65535".to_string(),
        ));
    }
    if !known_hosts_path.is_file() {
        return Err(ProviderError::GuestBridge(
            "pin the guest SSH fingerprint before sending it a password".to_string(),
        ));
    }
    if !valid_guest_public_key(public_key) {
        return Err(ProviderError::GuestBridge(
            "the buildbridge guest public key is invalid".to_string(),
        ));
    }
    if !valid_guest_password(password) {
        return Err(ProviderError::GuestBridge(
            "enter the local macOS login password: up to 512 characters on one line".to_string(),
        ));
    }

    let askpass = GuestAskpassHelper::create()?;
    let output =
        guest_password_ssh_command(ssh_port, username, known_hosts_path, &askpass.script())
            .env(GUEST_PASSWORD_ENV, password)
            .arg(guest_key_install_command(public_key))
            .tracked_output()
            .map_err(|error| {
                ProviderError::GuestBridge(format!(
                    "could not run ssh; install OpenSSH client tools: {error}"
                ))
            })?;
    drop(askpass);

    if !output.status.success() {
        return Err(ProviderError::GuestBridge(
            describe_password_session_failure(username, &clean_output(&output.stderr)),
        ));
    }
    match clean_output(&output.stdout).as_str() {
        "authorized" => Ok(()),
        other => Err(ProviderError::GuestBridge(format!(
            "the guest did not confirm the key install: {}",
            if other.is_empty() { "no result" } else { other }
        ))),
    }
}

pub(crate) fn describe_password_session_failure(username: &str, stderr: &str) -> String {
    if stderr.contains("Permission denied") {
        format!(
            "macOS did not accept the password for {username}. Check the short username and the local macOS login password, or add the key from the guest Terminal instead."
        )
    } else if stderr.contains("Host key verification failed")
        || stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
    {
        "the guest SSH identity no longer matches the pinned fingerprint; forget the pin only after verifying this is the expected machine".to_string()
    } else if stderr.is_empty() {
        "the password-authenticated SSH session failed without a message".to_string()
    } else {
        stderr.to_string()
    }
}

pub(crate) fn guest_xcode_application_path(username: &str) -> String {
    format!("/Users/{username}/Applications/Xcode.app")
}

pub fn import_xcode_package<F>(
    package_path: &Path,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    mut on_progress: F,
) -> Result<XcodeImportResult, ProviderError>
where
    F: FnMut(XcodeImportProgress),
{
    if !valid_guest_username(username) {
        return Err(ProviderError::GuestBridge(
            "the macOS short username is invalid".to_string(),
        ));
    }
    if ssh_port < 1024 {
        return Err(ProviderError::GuestBridge(
            "guest SSH port must be between 1024 and 65535".to_string(),
        ));
    }
    if !identity_path.is_file() || !known_hosts_path.is_file() {
        return Err(ProviderError::GuestBridge(
            "configure a guest key and trust its SSH fingerprint before importing Xcode"
                .to_string(),
        ));
    }

    let (validated_path, total_bytes) = validate_xcode_package(package_path)?;
    let started_at = Instant::now();
    let guest_home = format!("/Users/{username}");
    let guest_downloads = format!("{guest_home}/Downloads");
    let guest_applications = format!("{guest_home}/Applications");
    let guest_archive = format!("{guest_downloads}/{XCODE_ARCHIVE_NAME}");
    let installed_path = guest_xcode_application_path(username);

    on_progress(XcodeImportProgress {
        phase: XcodeImportPhase::Preparing,
        transferred_bytes: 0,
        total_bytes,
        elapsed_seconds: 0,
        detail: "Checking the pinned guest and preparing its import directory.".to_string(),
    });

    let prepare_directories = format!("/bin/mkdir -p '{guest_downloads}' '{guest_applications}'");
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &prepare_directories,
    )?;

    let inspect_destination = format!(
        "if /bin/test -e '{installed_path}' || /bin/test -e '{guest_downloads}/Xcode.app'; then /usr/bin/printf occupied; else /usr/bin/printf clear; fi"
    );
    let destination_status = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &inspect_destination,
    )?;
    if destination_status != "clear" {
        return Err(ProviderError::GuestBridge(
            "Xcode.app already exists in the guest user’s Applications or Downloads directory. Activate or move that copy instead of overwriting it."
                .to_string(),
        ));
    }

    stream_xcode_package(
        &validated_path,
        total_bytes,
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &guest_archive,
        started_at,
        &mut on_progress,
    )?;
    expand_xcode_package(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &guest_archive,
        &guest_downloads,
        &installed_path,
        total_bytes,
        started_at,
        &mut on_progress,
    )?;

    let activation_commands = vec![
        format!("sudo xcode-select --switch '{installed_path}'"),
        "sudo xcodebuild -license accept".to_string(),
        "sudo xcodebuild -runFirstLaunch".to_string(),
        "xcodebuild -version".to_string(),
    ];
    on_progress(XcodeImportProgress {
        phase: XcodeImportPhase::AwaitingActivation,
        transferred_bytes: total_bytes,
        total_bytes,
        elapsed_seconds: started_at.elapsed().as_secs(),
        detail: "Xcode is expanded and ready for buildbridge activation.".to_string(),
    });

    Ok(XcodeImportResult {
        installed_path,
        activation_commands,
    })
}

pub fn activate_xcode<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    mut on_progress: F,
) -> Result<(), ProviderError>
where
    F: FnMut(XcodeImportProgress),
{
    let installed_path =
        ensure_imported_xcode(ssh_port, username, identity_path, known_hosts_path)?;

    let detail = "A macOS Terminal window is opening. Enter the local macOS login password there; buildbridge does not receive or store it.";
    on_progress(XcodeImportProgress {
        phase: XcodeImportPhase::AwaitingAuthorization,
        transferred_bytes: 0,
        total_bytes: 0,
        elapsed_seconds: 0,
        detail: detail.to_string(),
    });

    // The no-password route. A bare SSH process cannot display Authorization Services UI in
    // the console audit session, so open a fixed, short-lived command file in the guest's
    // Terminal: sudo reads the password from its macOS TTY and it never leaves the guest. The
    // bridge route below is the alternative when the password is typed into the desktop.
    let activation_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let guest_cache = format!("/Users/{username}/Library/Caches/dev.buildbridge.desktop");
    let status_path = format!("{guest_cache}/xcode-activation-{activation_id}.status");
    let script_path = format!("{guest_cache}/xcode-activation-{activation_id}.command");
    let terminal_script = format!(
        r#"#!/bin/zsh
/usr/bin/clear
/usr/bin/printf "buildbridge Xcode activation\n\n"
/usr/bin/printf "Enter the local macOS login password when sudo asks.\n"
/usr/bin/printf "The password remains inside this macOS Terminal.\n"
/usr/bin/printf "Continuing selects Xcode, accepts its license, and installs required components.\n\n"
trap "/usr/bin/printf \"failed:interrupted\\n\" > {status_path}" EXIT
if /usr/bin/sudo /usr/bin/xcode-select --switch {installed_path} && \
   /usr/bin/sudo /usr/bin/xcodebuild -license accept && \
   /usr/bin/sudo /usr/bin/xcodebuild -runFirstLaunch; then
    trap - EXIT
    /usr/bin/printf "success\n" > {status_path}
    /usr/bin/printf "\nXcode is ready. You can close this window.\n"
else
    result=$?
    trap - EXIT
    /usr/bin/printf "failed:%s\n" "$result" > {status_path}
    /usr/bin/printf "\nActivation did not complete. Return to buildbridge and retry.\n"
fi
read -k 1 "?Press any key to close this window."
"#
    );
    let run = run_guest_terminal_script(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &GuestTerminalScript {
            guest_cache: &guest_cache,
            status_path: &status_path,
            script_path: &script_path,
            script: &terminal_script,
            timeout_seconds: 1800,
            spawn_failure: "could not start macOS Xcode activation",
        },
        &mut |elapsed| {
            on_progress(XcodeImportProgress {
                phase: XcodeImportPhase::AwaitingAuthorization,
                transferred_bytes: 0,
                total_bytes: 0,
                elapsed_seconds: elapsed,
                detail: detail.to_string(),
            })
        },
    )?;
    let stdout = run.stdout;
    let stderr = run.stderr;

    if !run.status_success {
        let stderr = clean_output(&stderr);
        let stdout = clean_output(&stdout);
        let message = if !stderr.is_empty() { stderr } else { stdout };
        let message = if message.contains("User canceled") || message.contains("-128") {
            "Xcode activation was canceled in macOS. Select Activate Xcode when you are ready."
                .to_string()
        } else if message.is_empty() {
            "macOS could not complete Xcode activation. Use the manual recovery commands if Terminal did not appear."
                .to_string()
        } else {
            format!(
                "macOS could not complete Xcode activation: {message}. Use the manual recovery commands if Terminal did not appear."
            )
        };

        return Err(ProviderError::GuestBridge(message));
    }

    match clean_output(&stdout).as_str() {
        "success" => {}
        "failed:interrupted" => {
            return Err(ProviderError::GuestBridge(
                "Xcode activation was interrupted in the macOS Terminal. Retry when you are ready."
                    .to_string(),
            ));
        }
        result if result.starts_with("failed:") => {
            return Err(ProviderError::GuestBridge(
                "Xcode activation did not complete in macOS. Retry and use the local macOS login password shown during account setup."
                    .to_string(),
            ));
        }
        "launch_failed" => {
            return Err(ProviderError::GuestBridge(
                "macOS could not open the activation Terminal window. Use the manual recovery commands."
                    .to_string(),
            ));
        }
        "timeout" => {
            return Err(ProviderError::GuestBridge(
                "Xcode activation timed out after 30 minutes. Close any activation Terminal window and retry."
                    .to_string(),
            ));
        }
        result => {
            return Err(ProviderError::GuestBridge(format!(
                "macOS returned an unexpected Xcode activation result: {result}"
            )));
        }
    }

    verify_xcode_activation(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &installed_path,
    )
}

/// Marker the bridge route prints between its commands so progress can name the one running.
pub(crate) const XCODE_ACTIVATION_MARKER: &str = "__BUILDBRIDGE_ACTIVATION__:";

/// Activates Xcode over the pinned bridge instead of the guest Terminal. The local macOS login
/// password is written once to the SSH session's stdin, where a single `sudo -S` reads it and
/// runs the same fixed `xcode-select`, license, and first-launch commands the Terminal route
/// runs. The password is never an argument on either side, never a file, and is gone when the
/// session ends. Output streams back so the interface can say which command is running.
pub fn activate_xcode_with_password<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    password: &str,
    mut on_progress: F,
) -> Result<(), ProviderError>
where
    F: FnMut(XcodeImportProgress),
{
    if !valid_guest_password(password) {
        return Err(ProviderError::GuestBridge(
            "enter the local macOS login password: up to 512 characters on one line".to_string(),
        ));
    }
    let installed_path =
        ensure_imported_xcode(ssh_port, username, identity_path, known_hosts_path)?;

    let started_at = Instant::now();
    let mut step = "authorizing".to_string();
    let mut detail = xcode_activation_step_detail(&step).to_string();
    on_progress(xcode_activation_progress(&detail, 0));

    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(xcode_activation_sudo_command(&installed_path))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not start Xcode activation over the bridge: {error}"
            ))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge(
            "could not hand the password to the activation session".to_string(),
        )
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture Xcode activation output".to_string())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture Xcode activation errors".to_string())
    })?;
    // sudo reads exactly one line. Closing stdin right after is what stops it asking again, and
    // a session that died before reading explains itself through its exit status below.
    let _ = stdin
        .write_all(password.as_bytes())
        .and_then(|()| stdin.write_all(b"\n"))
        .and_then(|()| stdin.flush());
    drop(stdin);

    let (lines, received) = std::sync::mpsc::channel::<String>();
    let stdout_reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut stderr = stderr;
        let _ = stderr.read_to_end(&mut output);
        output
    });
    loop {
        match received.recv_timeout(Duration::from_secs(1)) {
            Ok(line) => {
                if let Some(reached) = line.strip_prefix(XCODE_ACTIVATION_MARKER) {
                    step = reached.trim().to_string();
                    detail = xcode_activation_step_detail(&step).to_string();
                } else {
                    let line = sanitize_build_log_line(&line);
                    if !line.is_empty() {
                        detail = format!("{} · {line}", xcode_activation_step_detail(&step));
                    }
                }
                on_progress(xcode_activation_progress(
                    &detail,
                    started_at.elapsed().as_secs(),
                ));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                on_progress(xcode_activation_progress(
                    &detail,
                    started_at.elapsed().as_secs(),
                ));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = stdout_reader.join();
    let status = child.wait().map_err(|error| {
        ProviderError::GuestBridge(format!("could not monitor Xcode activation: {error}"))
    })?;
    let stderr = clean_output(&stderr_reader.join().unwrap_or_default());

    if !status.success() {
        return Err(ProviderError::GuestBridge(
            describe_bridge_activation_failure(username, &step, &stderr),
        ));
    }

    verify_xcode_activation(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &installed_path,
    )
}

/// The one remote command of the bridge route: `sudo` takes the password from stdin, ignores any
/// cached credential, prints nothing as a prompt, and runs the fixed activation script, whose
/// own stdin is `/dev/null` so nothing downstream can read the session again.
pub(crate) fn xcode_activation_sudo_command(installed_path: &str) -> String {
    let script = format!(
        "exec </dev/null; /usr/bin/printf '{marker}select\\n'; /usr/bin/xcode-select --switch '{installed_path}' && /usr/bin/printf '{marker}license\\n' && /usr/bin/xcodebuild -license accept && /usr/bin/printf '{marker}first-launch\\n' && /usr/bin/xcodebuild -runFirstLaunch && /usr/bin/printf '{marker}done\\n'",
        marker = XCODE_ACTIVATION_MARKER
    );

    format!(
        "/usr/bin/sudo -S -k -p '' /bin/sh -c {}",
        shell_single_quote(&script)
    )
}

pub(crate) fn xcode_activation_step_detail(step: &str) -> &'static str {
    match step {
        "authorizing" => "Authorizing with sudo over the pinned bridge",
        "select" => "Selecting the developer directory",
        "license" => "Accepting Apple's license",
        "first-launch" => {
            "Running Xcode's first-launch tasks and installing required components. This can take several minutes."
        }
        "done" => "Activation finished; verifying",
        _ => "Activating Xcode",
    }
}

pub(crate) fn xcode_activation_progress(detail: &str, elapsed_seconds: u64) -> XcodeImportProgress {
    XcodeImportProgress {
        phase: XcodeImportPhase::Activating,
        transferred_bytes: 0,
        total_bytes: 0,
        elapsed_seconds,
        detail: detail.to_string(),
    }
}

pub(crate) fn describe_bridge_activation_failure(
    username: &str,
    step: &str,
    stderr: &str,
) -> String {
    if stderr.contains("incorrect password attempt") || stderr.contains("Sorry, try again") {
        return format!(
            "macOS did not accept the password for {username}; nothing was changed. Check the local macOS login password, or leave it blank to type it in the guest Terminal."
        );
    }
    if stderr.contains("not in the sudoers") || stderr.contains("not allowed to") {
        return format!(
            "{username} is not an administrator on the guest, so sudo refused. Activate with an administrator account or use the manual commands."
        );
    }

    let stage = match step {
        "authorizing" => " before any command ran",
        "select" => " while selecting the developer directory",
        "license" => " while accepting the license",
        "first-launch" => " during Xcode's first-launch tasks",
        _ => "",
    };
    if stderr.is_empty() {
        format!("macOS could not complete Xcode activation{stage}; no message was returned")
    } else {
        format!("macOS could not complete Xcode activation{stage}: {stderr}")
    }
}

/// Confirms the imported Xcode application is where the import left it and returns its path.
pub(crate) fn ensure_imported_xcode(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<String, ProviderError> {
    if !valid_guest_username(username) {
        return Err(ProviderError::GuestBridge(
            "the macOS short username is invalid".to_string(),
        ));
    }
    if ssh_port < 1024 {
        return Err(ProviderError::GuestBridge(
            "guest SSH port must be between 1024 and 65535".to_string(),
        ));
    }
    if !identity_path.is_file() || !known_hosts_path.is_file() {
        return Err(ProviderError::GuestBridge(
            "configure a guest key and trust its SSH fingerprint before activating Xcode"
                .to_string(),
        ));
    }

    let installed_path = guest_xcode_application_path(username);
    let inspect_xcode = format!(
        "if /bin/test -x '{installed_path}/Contents/Developer/usr/bin/xcodebuild'; then /usr/bin/printf ready; else /usr/bin/printf missing; fi"
    );
    if run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &inspect_xcode,
    )? != "ready"
    {
        return Err(ProviderError::GuestBridge(
            "the imported Xcode application could not be found; import it again or use the recovery commands"
                .to_string(),
        ));
    }

    Ok(installed_path)
}

/// Checks that activation, whichever route ran it, left the expected developer directory
/// selected and first launch complete.
pub(crate) fn verify_xcode_activation(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    installed_path: &str,
) -> Result<(), ProviderError> {
    let selected_path = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        "/usr/bin/xcode-select --print-path",
    )
    .map_err(|_| {
        ProviderError::GuestBridge(
            "Xcode remains inactive. Retry activation with the local macOS login password, not the Apple Account password."
                .to_string(),
        )
    })?;
    let expected_path = format!("{installed_path}/Contents/Developer");
    if selected_path != expected_path {
        return Err(ProviderError::GuestBridge(format!(
            "macOS selected an unexpected developer directory: {selected_path}"
        )));
    }
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        "/usr/bin/xcodebuild -version",
    )?;
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        "/usr/bin/xcodebuild -checkFirstLaunchStatus",
    )
    .map_err(|_| {
        ProviderError::GuestBridge(
            "Xcode was selected, but macOS still reports incomplete first-launch setup. Retry activation and let it finish."
                .to_string(),
        )
    })?;

    Ok(())
}

pub(crate) fn validate_xcode_package(package_path: &Path) -> Result<(PathBuf, u64), ProviderError> {
    if !package_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xip"))
    {
        return Err(ProviderError::GuestBridge(
            "select an Apple Xcode .xip archive".to_string(),
        ));
    }

    let canonical_path = fs::canonicalize(package_path).map_err(|error| {
        ProviderError::GuestBridge(format!(
            "the selected Xcode package is unavailable: {error}"
        ))
    })?;
    let metadata = fs::metadata(&canonical_path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not inspect the Xcode package: {error}"))
    })?;

    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > XCODE_PACKAGE_MAX_BYTES {
        return Err(ProviderError::GuestBridge(
            "the selected Xcode package must be a non-empty file no larger than 20 GiB".to_string(),
        ));
    }

    let mut archive = File::open(&canonical_path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not open the Xcode package: {error}"))
    })?;
    let mut signature = [0_u8; 4];
    archive.read_exact(&mut signature).map_err(|error| {
        ProviderError::GuestBridge(format!("could not read the Xcode package: {error}"))
    })?;
    if signature != *b"xar!" {
        return Err(ProviderError::GuestBridge(
            "the selected file is not a complete Apple XIP archive".to_string(),
        ));
    }

    Ok((canonical_path, metadata.len()))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn stream_xcode_package<F>(
    package_path: &Path,
    total_bytes: u64,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    guest_archive: &str,
    started_at: Instant,
    on_progress: &mut F,
) -> Result<(), ProviderError>
where
    F: FnMut(XcodeImportProgress),
{
    let mut package = File::open(package_path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not open the Xcode package: {error}"))
    })?;
    let remote_command = format!(
        "/bin/cat > '{guest_archive}.part' && /bin/mv -f '{guest_archive}.part' '{guest_archive}'"
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not start the secure Xcode transfer: {error}"
            ))
        })?;
    let mut child_stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not open the secure Xcode transfer".to_string())
    })?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut transferred_bytes = 0_u64;
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    let transfer_result = loop {
        let read = match package.read(&mut buffer) {
            Ok(read) => read,
            Err(error) => {
                break Err(ProviderError::GuestBridge(format!(
                    "could not read the Xcode package: {error}"
                )));
            }
        };
        if read == 0 {
            break Ok(());
        }
        if let Err(error) = child_stdin.write_all(&buffer[..read]) {
            break Err(ProviderError::GuestBridge(format!(
                "the secure Xcode transfer was interrupted: {error}"
            )));
        }

        transferred_bytes += read as u64;
        if last_progress.elapsed() >= Duration::from_millis(250) || transferred_bytes == total_bytes
        {
            on_progress(XcodeImportProgress {
                phase: XcodeImportPhase::Transferring,
                transferred_bytes,
                total_bytes,
                elapsed_seconds: started_at.elapsed().as_secs(),
                detail: "Copying the signed XIP archive through the pinned SSH bridge.".to_string(),
            });
            last_progress = Instant::now();
        }
    };
    drop(child_stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish the Xcode transfer: {error}"))
    })?;
    transfer_result?;

    if !output.status.success() {
        let message = clean_output(&output.stderr);
        return Err(ProviderError::GuestBridge(if message.is_empty() {
            "the guest rejected the Xcode package transfer".to_string()
        } else {
            message
        }));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn expand_xcode_package<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    guest_archive: &str,
    guest_downloads: &str,
    installed_path: &str,
    total_bytes: u64,
    started_at: Instant,
    on_progress: &mut F,
) -> Result<(), ProviderError>
where
    F: FnMut(XcodeImportProgress),
{
    let expand_command = format!(
        "set -eu; staging=$(/usr/bin/mktemp -d '{guest_downloads}/.buildbridge-xcode.XXXXXX'); cd \"$staging\"; /usr/bin/xip --expand '{guest_archive}'; /bin/mv \"$staging/Xcode.app\" '{installed_path}'; /bin/rmdir \"$staging\"; /bin/rm -f '{guest_archive}'"
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(expand_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start Xcode expansion: {error}"))
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture Xcode expansion output".to_string())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture Xcode expansion errors".to_string())
    })?;
    let stdout_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut stdout = stdout;
        let _ = stdout.read_to_end(&mut output);
        output
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut stderr = stderr;
        let _ = stderr.read_to_end(&mut output);
        output
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                on_progress(XcodeImportProgress {
                    phase: XcodeImportPhase::Expanding,
                    transferred_bytes: total_bytes,
                    total_bytes,
                    elapsed_seconds: started_at.elapsed().as_secs(),
                    detail:
                        "Verifying and expanding Xcode inside macOS. This can take several minutes."
                            .to_string(),
                });
                thread::sleep(Duration::from_secs(1));
            }
            Err(error) => {
                return Err(ProviderError::GuestBridge(format!(
                    "could not monitor Xcode expansion: {error}"
                )));
            }
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();

    if !status.success() {
        let stderr = clean_output(&stderr);
        let stdout = clean_output(&stdout);
        let message = if !stderr.is_empty() { stderr } else { stdout };
        return Err(ProviderError::GuestBridge(if message.is_empty() {
            "macOS could not verify or expand the Xcode package".to_string()
        } else {
            format!("macOS could not expand the Xcode package: {message}")
        }));
    }

    Ok(())
}
