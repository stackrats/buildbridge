//! Deliver the exact retained IPA through Transporter inside the pinned macOS guest.

use super::*;
use std::sync::mpsc;

const TRANSPORTER_HELPER: &str = include_str!("transporter.py");
const TRANSPORTER_PREFIX: &str = "__BUILDBRIDGE_TRANSPORTER__:";
const TRANSPORTER_COMPLETED: &str =
    "Upload delivered. Apple must process the build before it appears in TestFlight.";
const TRANSPORTER_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60 + 15);

/// Uses an installed Transporter with a team App Store Connect API key. The key is
/// carried through framed stdin and removed from the guest on completion, failure,
/// timeout, or disconnection. The retained IPA must match `expected_sha256` after
/// transfer before any delivery to Apple can begin.
#[allow(clippy::too_many_arguments)]
pub fn upload_apple_ipa<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    ipa_path: &Path,
    expected_sha256: &str,
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
    mut on_progress: F,
) -> Result<(), ProviderError>
where
    F: FnMut(String),
{
    validate_upload_input(username, expected_sha256, key_id, issuer_id, private_key)?;
    let ipa = File::open(ipa_path).map_err(|error| {
        upload_error(format!(
            "could not open the retained App Store IPA: {error}"
        ))
    })?;
    let metadata = ipa.metadata().map_err(|error| {
        upload_error(format!(
            "could not inspect the retained App Store IPA: {error}"
        ))
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 16 * 1024 * 1024 * 1024 {
        return Err(upload_error(
            "the retained App Store IPA is empty or exceeds 16 GiB",
        ));
    }
    let ssh = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path);
    let mut command = Command::new(ssh.get_program());
    // All guest source is fixed; paths, identifiers and the private key are data on
    // stdin, never shell text. Keepalives bound a severed network connection.
    command
        .args([
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
        ])
        .args(ssh.get_args())
        .arg(transporter_command());
    run_upload(
        &mut command,
        ipa,
        metadata.len(),
        [key_id, issuer_id, private_key, expected_sha256],
        TRANSPORTER_TIMEOUT,
        &mut on_progress,
    )
}

fn transporter_command() -> String {
    format!(
        "exec /usr/bin/python3 -u -c {}",
        shell_single_quote(TRANSPORTER_HELPER)
    )
}

fn upload_error(message: impl Into<String>) -> ProviderError {
    ProviderError::GuestBridge(message.into())
}

fn validate_upload_input(
    username: &str,
    expected_sha256: &str,
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
) -> Result<(), ProviderError> {
    let uuid_parts = issuer_id.split('-').collect::<Vec<_>>();
    if !valid_guest_username(username) {
        return Err(upload_error("the macOS guest username is invalid"));
    }
    if expected_sha256.len() != 64
        || !expected_sha256
            .bytes()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(upload_error(
            "the retained App Store IPA checksum is invalid",
        ));
    }
    if key_id.len() != 10
        || !key_id
            .bytes()
            .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
    {
        return Err(upload_error(
            "enter the 10-character App Store Connect API Key ID",
        ));
    }
    if uuid_parts.len() != 5
        || uuid_parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .any(|(part, length)| {
                part.len() != length || !part.bytes().all(|character| character.is_ascii_hexdigit())
            })
    {
        return Err(upload_error(
            "enter the App Store Connect team API key's Issuer ID",
        ));
    }
    if private_key.len() > 16_384
        || !private_key.starts_with("-----BEGIN PRIVATE KEY-----")
        || !private_key
            .trim_end()
            .ends_with("-----END PRIVATE KEY-----")
        || private_key.contains('\0')
    {
        return Err(upload_error(
            "choose the App Store Connect API private key (.p8)",
        ));
    }
    Ok(())
}

enum UploadEvent {
    Output(String),
    Transfer(Result<(), ProviderError>),
}

fn write_upload_payload(
    writer: &mut impl Write,
    ipa: &mut impl Read,
    size: u64,
    frames: [&str; 4],
) -> Result<(), ProviderError> {
    for frame in frames {
        write_secret_frame(writer, frame)?;
    }
    writer.write_all(&size.to_be_bytes()).map_err(|error| {
        upload_error(format!("could not send the retained IPA length: {error}"))
    })?;
    let copied = std::io::copy(&mut ipa.take(size), writer).map_err(|error| {
        upload_error(format!(
            "the retained IPA transfer was interrupted: {error}"
        ))
    })?;
    if copied != size {
        return Err(upload_error(
            "the retained IPA changed while it was being transferred",
        ));
    }
    writer.flush().map_err(|error| {
        upload_error(format!(
            "could not finish the retained IPA transfer: {error}"
        ))
    })
}

fn read_upload_output(reader: impl Read, events: mpsc::Sender<UploadEvent>) {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        // Bound each line before parsing. Only our helper's progress protocol is
        // exposed; Transporter stdout and stderr never reach the host logs.
        match reader.by_ref().take(4097).read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) if line.len() > 4096 => return,
            Ok(_) => {
                let value = String::from_utf8_lossy(&line);
                if let Some(message) = value.trim().strip_prefix(TRANSPORTER_PREFIX) {
                    let _ = events.send(UploadEvent::Output(message.to_string()));
                }
            }
        }
    }
}

fn run_upload(
    command: &mut Command,
    mut ipa: File,
    size: u64,
    frames: [&str; 4],
    timeout: Duration,
    on_progress: &mut impl FnMut(String),
) -> Result<(), ProviderError> {
    let operation = current_scope();
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .tracked_spawn()
        .map_err(|error| {
            upload_error(format!(
                "could not start the Transporter SSH bridge: {error}"
            ))
        })?;
    let mut stdin = child.stdin.take().expect("the upload stdin is piped");
    let stdout = child.stdout.take().expect("the upload stdout is piped");
    let started = Instant::now();
    on_progress("Transferring the retained App Store IPA to the macOS machine.".to_string());
    let (events, receiver) = mpsc::channel();
    let (release, hold_input) = mpsc::channel::<()>();
    let mut release = Some(release);
    thread::scope(|threads| {
        let transfer_events = events.clone();
        threads.spawn(move || {
            let result = write_upload_payload(&mut stdin, &mut ipa, size, frames);
            let failed = result.is_err();
            let _ = transfer_events.send(UploadEvent::Transfer(result));
            if !failed {
                // EOF is the guest's cancellation signal. Retain the pipe until
                // the guest has finished, or the host cancels/times out.
                let _ = hold_input.recv();
            }
        });
        let output_reader = threads.spawn(move || read_upload_output(stdout, events));
        let mut output_reader = Some(output_reader);
        let mut completed = false;
        let mut error = None;
        let result = loop {
            if operation.as_ref().is_some_and(|scope| scope.is_cancelled()) {
                break Err(upload_error(
                    "upload stopped; check App Store Connect before retrying because Apple may have received the build",
                ));
            }
            if started.elapsed() > timeout {
                break Err(upload_error(
                    "the upload timed out; check App Store Connect before retrying because Apple may have received the build",
                ));
            }
            for event in receiver.try_iter() {
                match event {
                    UploadEvent::Output(message) => {
                        if let Some(message) = message.strip_prefix("ERROR: ") {
                            error = Some(upload_error(message));
                        } else {
                            completed |= message == TRANSPORTER_COMPLETED;
                            on_progress(message);
                        }
                    }
                    UploadEvent::Transfer(Err(transfer_error)) => {
                        // A missing guest tool may close stdin before transfer
                        // finishes. Prefer the helper's actionable error.
                        if error.is_none() {
                            error = Some(transfer_error);
                        }
                    }
                    UploadEvent::Transfer(Ok(())) => {}
                }
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    // An ended SSH process can no longer confirm delivery. Stop
                    // retaining stdin before joining output so a surviving guest
                    // helper can observe EOF and remove its temporary key.
                    drop(release.take());
                    // Readers can still have the final output buffered after
                    // the process exits. Drain their final events before deciding.
                    let _ = output_reader
                        .take()
                        .expect("the output reader is joined once")
                        .join();
                    for event in receiver.try_iter() {
                        if let UploadEvent::Output(message) = event {
                            if let Some(message) = message.strip_prefix("ERROR: ") {
                                error = Some(upload_error(message));
                            } else {
                                completed |= message == TRANSPORTER_COMPLETED;
                                on_progress(message);
                            }
                        }
                    }
                    break if let Some(error) = error {
                        Err(error)
                    } else if status.success() && completed {
                        Ok(())
                    } else {
                        Err(upload_error(
                            "the Transporter connection ended without confirming delivery; verify macOS SSH and Xcode's Python tools, and check App Store Connect before retrying",
                        ))
                    };
                }
                Ok(None) => thread::sleep(Duration::from_millis(100)),
                Err(error) => {
                    break Err(upload_error(format!(
                        "could not monitor Transporter: {error}"
                    )));
                }
            }
        };
        drop(release);
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
        if operation.as_ref().is_some_and(|scope| scope.is_cancelled()) {
            return Err(upload_error(
                "upload stopped; check App Store Connect before retrying because Apple may have received the build",
            ));
        }
        result
    })
}

#[cfg(all(test, unix))]
#[path = "transporter_tests.rs"]
mod tests;
