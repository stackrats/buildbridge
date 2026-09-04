//! The project workspace: snapshot, transfer, sync, web assets, and progress events.

use super::*;

/// A build's environment, rendered twice because its two readers quote differently: Vite's
/// dotenv loader reads `.env.production.local`, and the guest build shell sources `env.sh`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestEnvFiles {
    pub dotenv: String,
    pub shell: String,
}

pub fn sync_apple_workspace<F>(
    workspace_path: &Path,
    env: Option<&GuestEnvFiles>,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    mut on_progress: F,
) -> Result<AppleWorkspaceSyncResult, ProviderError>
where
    F: FnMut(AppleProjectProgress),
{
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    let workspace_path = fs::canonicalize(workspace_path).map_err(|error| {
        ProviderError::GuestBridge(format!("the approved project is unavailable: {error}"))
    })?;
    if !workspace_path.is_dir()
        || !workspace_path.join("package.json").is_file()
        || !workspace_path.join("ios/App/Podfile").is_file()
        || !workspace_path.join("ios/App/Podfile.lock").is_file()
        || !workspace_path.join("ios/App/App.xcworkspace").is_dir()
    {
        return Err(ProviderError::GuestBridge(
            "the approved project must contain package.json and ios/App/App.xcworkspace with a locked Podfile"
                .to_string(),
        ));
    }

    let started_at = Instant::now();
    on_progress(apple_progress(
        AppleProjectPhase::Snapshotting,
        0,
        0,
        started_at,
        "Inspecting the approved project and excluding local dependencies and secrets.",
        None,
    ));
    let (source_file_count, source_bytes) = inspect_snapshot_tree(&workspace_path)?;
    let temporary_archive = TemporaryArchive::new();
    create_workspace_archive(&workspace_path, &temporary_archive.0)?;
    let archive_bytes = fs::metadata(&temporary_archive.0)
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not inspect the source snapshot: {error}"))
        })?
        .len();
    let snapshot_sha256 = snapshot_sha256(&temporary_archive.0)?;

    let guest_root = format!("/Users/{username}/BuildBridge/workspaces");
    let guest_workspace = format!("{guest_root}/active");
    let guest_staging = format!("{guest_root}/active.incoming");
    let guest_archive = format!("{guest_root}/active.tar.gz");
    let guest_env = format!("{guest_root}/active.env");
    let guest_env_shell = format!("{guest_root}/active.env.sh");
    let prepare = format!(
        "/bin/mkdir -p '{guest_root}'; /bin/rm -rf '{guest_staging}'; /bin/rm -f '{guest_archive}' '{guest_env}' '{guest_env_shell}'"
    );
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &prepare,
    )?;
    stream_workspace_archive(
        &temporary_archive.0,
        archive_bytes,
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &guest_archive,
        started_at,
        &mut on_progress,
    )?;

    // The environment travels the same pinned channel as the source, as owner-only files that
    // land next to it: one for Vite, one for the build shell. Never as command arguments.
    if let Some(env) = env {
        stream_bytes_to_guest(
            env.dotenv.as_bytes(),
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_env,
            "env file",
        )?;
        stream_bytes_to_guest(
            env.shell.as_bytes(),
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_env_shell,
            "env script",
        )?;
    }

    on_progress(apple_progress(
        AppleProjectPhase::Extracting,
        archive_bytes,
        archive_bytes,
        started_at,
        "Extracting the bounded snapshot into the BuildBridge guest workspace.",
        None,
    ));
    let extract = format!(
        "set -eu; /bin/mkdir -p '{guest_staging}'; /usr/bin/tar -xzf '{guest_archive}' -C '{guest_staging}'; /bin/test -f '{guest_staging}/package.json'; /bin/test -d '{guest_staging}/ios/App/App.xcworkspace'; /bin/rm -rf '{guest_workspace}.previous'; if /bin/test -d '{guest_workspace}'; then /bin/mv '{guest_workspace}' '{guest_workspace}.previous'; fi; /bin/mv '{guest_staging}' '{guest_workspace}'; if /bin/test -f '{guest_env}'; then /bin/mv '{guest_env}' '{guest_workspace}/.env.production.local'; /bin/chmod 600 '{guest_workspace}/.env.production.local'; fi; if /bin/test -f '{guest_env_shell}'; then /bin/mkdir -p '{guest_workspace}/.buildbridge'; /bin/mv '{guest_env_shell}' '{guest_workspace}/.buildbridge/env.sh'; /bin/chmod 600 '{guest_workspace}/.buildbridge/env.sh'; fi; /bin/rm -f '{guest_archive}'; /bin/rm -rf '{guest_workspace}.previous'"
    );
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &extract,
    )?;

    Ok(AppleWorkspaceSyncResult {
        guest_path: guest_workspace,
        snapshot_sha256,
        source_file_count,
        source_bytes,
        archive_bytes,
    })
}

pub(crate) fn validate_guest_operation(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
    if !valid_guest_username(username) {
        return Err(ProviderError::GuestBridge(
            "the macOS short username is invalid".to_string(),
        ));
    }
    if ssh_port < 1024 || !identity_path.is_file() || !known_hosts_path.is_file() {
        return Err(ProviderError::GuestBridge(
            "the pinned macOS guest connection is not configured".to_string(),
        ));
    }

    Ok(())
}

pub(crate) fn inspect_snapshot_tree(root: &Path) -> Result<(u64, u64), ProviderError> {
    fn visit(
        root: &Path,
        directory: &Path,
        files: &mut u64,
        bytes: &mut u64,
    ) -> Result<(), ProviderError> {
        for entry in fs::read_dir(directory).map_err(|error| {
            ProviderError::GuestBridge(format!("could not inspect the approved project: {error}"))
        })? {
            let entry = entry.map_err(|error| {
                ProviderError::GuestBridge(format!("could not inspect a project entry: {error}"))
            })?;
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap_or(&path);
            if snapshot_path_excluded(relative) {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                ProviderError::GuestBridge(format!("could not inspect {}: {error}", path.display()))
            })?;
            if metadata.file_type().is_symlink() {
                return Err(ProviderError::GuestBridge(format!(
                    "the project contains a symbolic link outside excluded dependency folders: {}",
                    relative.display()
                )));
            }
            if metadata.is_dir() {
                visit(root, &path, files, bytes)?;
            } else if metadata.is_file() {
                *files += 1;
                *bytes = bytes.saturating_add(metadata.len());
                if *files > APPLE_WORKSPACE_MAX_FILES || *bytes > APPLE_WORKSPACE_MAX_BYTES {
                    return Err(ProviderError::GuestBridge(
                        "the project snapshot exceeds the 50,000 file or 2 GiB safety limit"
                            .to_string(),
                    ));
                }
            } else {
                return Err(ProviderError::GuestBridge(format!(
                    "the project contains an unsupported filesystem entry: {}",
                    relative.display()
                )));
            }
        }

        Ok(())
    }

    let mut files = 0;
    let mut bytes = 0;
    visit(root, root, &mut files, &mut bytes)?;
    Ok((files, bytes))
}

pub(crate) fn snapshot_path_excluded(relative: &Path) -> bool {
    if [
        "android/build",
        "android/app/build",
        "android/capacitor-cordova-android-plugins/build",
        "ios/App/build",
        "ios/App/Pods",
    ]
    .iter()
    .any(|path| relative.starts_with(path))
    {
        return true;
    }

    let excluded_directory = relative.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(
                ".git"
                    | ".ssh"
                    | ".buildbridge"
                    | ".pnpm-store"
                    | "node_modules"
                    | "dist"
                    | "coverage"
                    | "DerivedData"
                    | "xcuserdata"
                    | ".idea"
                    | ".vscode"
                    | ".gradle"
            )
        )
    });
    if excluded_directory {
        return true;
    }

    let Some(name) = relative.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name == ".env"
        || name.starts_with(".env.")
        || matches!(
            name,
            ".npmrc"
                | ".yarnrc"
                | ".pypirc"
                | ".netrc"
                | "id_rsa"
                | "id_dsa"
                | "id_ecdsa"
                | "id_ed25519"
        )
    {
        return true;
    }

    relative
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "p8" | "p12" | "pfx" | "pem" | "key" | "mobileprovision"
            )
        })
}

pub(crate) fn create_workspace_archive(
    root: &Path,
    archive_path: &Path,
) -> Result<(), ProviderError> {
    let mut command = Command::new("tar");
    command.args(["-czf"]).arg(archive_path);
    for pattern in [
        ".git",
        ".ssh",
        ".buildbridge",
        ".pnpm-store",
        "node_modules",
        "dist",
        "coverage",
        "DerivedData",
        "xcuserdata",
        ".idea",
        ".vscode",
        ".gradle",
        "android/build",
        "android/app/build",
        "android/capacitor-cordova-android-plugins/build",
        "ios/App/build",
        "ios/App/Pods",
        ".env",
        ".env.*",
        ".npmrc",
        ".yarnrc",
        ".pypirc",
        ".netrc",
        "id_rsa",
        "id_dsa",
        "id_ecdsa",
        "id_ed25519",
        "*.p8",
        "*.p12",
        "*.pfx",
        "*.pem",
        "*.key",
        "*.mobileprovision",
    ] {
        command.arg(format!("--exclude={pattern}"));
    }
    let output = command
        .arg("-C")
        .arg(root)
        .arg(".")
        .tracked_output()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not run tar for the source snapshot: {error}"
            ))
        })?;
    if !output.status.success() {
        return Err(ProviderError::GuestBridge(format!(
            "could not create the source snapshot: {}",
            clean_output(&output.stderr)
        )));
    }

    Ok(())
}

pub(crate) fn snapshot_sha256(path: &Path) -> Result<String, ProviderError> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not checksum the source snapshot: {error}"))
        })?;
    if !output.status.success() {
        return Err(ProviderError::GuestBridge(
            "sha256sum could not checksum the source snapshot".to_string(),
        ));
    }
    let checksum = clean_output(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    if checksum.len() != 64
        || !checksum
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(ProviderError::GuestBridge(
            "sha256sum returned an invalid source snapshot checksum".to_string(),
        ));
    }

    Ok(checksum)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn stream_workspace_archive<F>(
    archive_path: &Path,
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
    F: FnMut(AppleProjectProgress),
{
    let mut archive = File::open(archive_path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not open the source snapshot: {error}"))
    })?;
    let remote_command = format!("/bin/cat > '{guest_archive}'");
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start source synchronization: {error}"))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not open source synchronization".to_string())
    })?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut completed_bytes = 0;
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    loop {
        let read = archive.read(&mut buffer).map_err(|error| {
            ProviderError::GuestBridge(format!("could not read the source snapshot: {error}"))
        })?;
        if read == 0 {
            break;
        }
        stdin.write_all(&buffer[..read]).map_err(|error| {
            ProviderError::GuestBridge(format!("source synchronization was interrupted: {error}"))
        })?;
        completed_bytes += read as u64;
        if last_progress.elapsed() >= Duration::from_millis(250) || completed_bytes == total_bytes {
            on_progress(apple_progress(
                AppleProjectPhase::Transferring,
                completed_bytes,
                total_bytes,
                started_at,
                "Copying the secret-filtered snapshot through the pinned SSH bridge.",
                None,
            ));
            last_progress = Instant::now();
        }
    }
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish source synchronization: {error}"))
    })?;
    if !output.status.success() {
        return Err(ProviderError::GuestBridge(format!(
            "the guest rejected the source snapshot: {}",
            clean_output(&output.stderr)
        )));
    }

    Ok(())
}

/// Rebuilds the web assets inside the guest with a chosen env set, in place, and re-syncs them
/// into the iOS project. This is what makes an env a per-build choice: the source snapshot and
/// the installed dependencies stay, only the assets that read the environment are produced again.
/// Refuses to continue if the native lockfile moves, exactly as the test build would.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rebuild_web_assets_with_env<F>(
    env: &GuestEnvFiles,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    guest_home: &str,
    started_at: Instant,
    on_progress: &mut F,
) -> Result<(), ProviderError>
where
    F: FnMut(AppleArchiveProgress),
{
    on_progress(archive_progress(
        AppleArchivePhase::BuildingWebAssets,
        0,
        0,
        started_at,
        "Applying the chosen env set and rebuilding the web assets.",
        None,
    ));
    let root = format!("{guest_home}/BuildBridge/workspaces/active");
    let toolchain = guest_toolchain(guest_home);
    let prepare_tools = guest_tools_preparation(&toolchain);
    let GuestToolchain {
        gem_home,
        developer_dir,
        path,
        ..
    } = &toolchain;

    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!(
            "set -eu; /bin/test -d {}; /bin/mkdir -p {}",
            shell_single_quote(&root),
            shell_single_quote(&format!("{root}/.buildbridge"))
        ),
    )?;
    stream_bytes_to_guest(
        env.dotenv.as_bytes(),
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!("{root}/.env.production.local"),
        "env file",
    )?;
    stream_bytes_to_guest(
        env.shell.as_bytes(),
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!("{root}/.buildbridge/env.sh"),
        "env script",
    )?;

    let script = format!(
        r#"set -u
exec 2>&1
set -e
export PATH="{path}"
export GEM_HOME="{gem_home}"
export GEM_PATH="{gem_home}"
export DEVELOPER_DIR="{developer_dir}"
export LANG="en_US.UTF-8"
export CYPRESS_INSTALL_BINARY=0
{prepare_tools}
/bin/test -f "{root}/package.json"
/bin/test -x "{root}/node_modules/.bin/vp"
/bin/test -x "{root}/node_modules/.bin/cap"
. "{root}/.buildbridge/env.sh"
cd "{root}"
"{root}/node_modules/.bin/vp" build
lock_before=$(/usr/bin/shasum -a 256 "{root}/ios/App/Podfile.lock" | /usr/bin/cut -d ' ' -f 1)
"{root}/node_modules/.bin/cap" sync ios
lock_after=$(/usr/bin/shasum -a 256 "{root}/ios/App/Podfile.lock" | /usr/bin/cut -d ' ' -f 1)
if /bin/test "$lock_before" != "$lock_after"; then
    /usr/bin/printf '__BUILDBRIDGE_NATIVE_LOCK_UPDATED__:yes\n'
    exit 3
fi
"#
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start the web asset rebuild: {error}"))
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture the web asset rebuild output".to_string())
    })?;
    let mut tail: Vec<String> = Vec::new();
    let mut lock_moved = false;
    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| {
            ProviderError::GuestBridge(format!("could not read the web asset rebuild: {error}"))
        })?;
        if line == "__BUILDBRIDGE_NATIVE_LOCK_UPDATED__:yes" {
            lock_moved = true;
            continue;
        }
        if tail.len() >= 40 {
            tail.remove(0);
        }
        tail.push(line.clone());
        on_progress(archive_progress(
            AppleArchivePhase::BuildingWebAssets,
            0,
            0,
            started_at,
            "Applying the chosen env set and rebuilding the web assets.",
            Some(line),
        ));
    }
    let status = child.wait().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish the web asset rebuild: {error}"))
    })?;
    if lock_moved {
        return Err(ProviderError::GuestBridge(
            "rebuilding with this env set changed Podfile.lock; synchronize and run the test build again before a signed archive"
                .to_string(),
        ));
    }
    if !status.success() {
        return Err(ProviderError::GuestBridge(format!(
            "the web asset rebuild failed:\n{}",
            tail.join("\n")
        )));
    }

    Ok(())
}

pub(crate) fn apple_progress(
    phase: AppleProjectPhase,
    completed_bytes: u64,
    total_bytes: u64,
    started_at: Instant,
    detail: &str,
    log_line: Option<String>,
) -> AppleProjectProgress {
    AppleProjectProgress {
        phase,
        completed_bytes,
        total_bytes,
        elapsed_seconds: started_at.elapsed().as_secs(),
        detail: detail.to_string(),
        log_line,
    }
}

pub(crate) fn signing_progress(
    phase: SigningProvisioningPhase,
    completed_bytes: u64,
    total_bytes: u64,
    started_at: Instant,
    detail: &str,
) -> SigningProvisioningProgress {
    SigningProvisioningProgress {
        phase,
        completed_bytes,
        total_bytes,
        elapsed_seconds: started_at.elapsed().as_secs(),
        detail: detail.to_string(),
    }
}
