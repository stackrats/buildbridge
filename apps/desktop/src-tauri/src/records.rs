//! Per-machine records on disk: workspace, signing, archive, and the guest key material.

use super::*;
use ts_rs::TS;

pub(crate) fn prepare_apple_archive_output_dir(paths: &MachinePaths) -> Result<PathBuf, String> {
    let root = paths.artifacts_dir();
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    set_restricted_directory_permissions(&root)?;
    let operation_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "The system clock is earlier than the Unix epoch.".to_string())?
        .as_millis();
    let directory = root.join(format!("archive-{operation_id}-{}", std::process::id()));
    fs::create_dir(&directory).map_err(|error| error.to_string())?;
    set_restricted_directory_permissions(&directory)?;

    Ok(directory)
}

pub(crate) fn remove_file_if_present(path: &std::path::Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn managed_apple_profiles_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("macos-builder").join("profiles"))
        .map_err(|error| error.to_string())
}

/// A provisioning profile this host already downloaded, kept so a kit can be rebuilt without
/// going back to Apple. Only the file name and path are exposed; the contents stay on disk.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManagedAppleProfile {
    pub(crate) file_name: String,
    pub(crate) path: String,
    #[ts(type = "number")]
    pub(crate) saved_at_epoch_seconds: u64,
}

pub(crate) fn safe_apple_resource_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

pub(crate) fn load_signing_provisioning(
    paths: &MachinePaths,
) -> Result<Option<StoredSigningProvisioning>, String> {
    let path = paths.signing_provisioning();

    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("The guest signing record is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn load_apple_archive(
    paths: &MachinePaths,
) -> Result<Option<StoredAppleArchive>, String> {
    let path = paths.apple_archive_record();

    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("The signed archive record is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn save_apple_archive(
    paths: &MachinePaths,
    archive: &StoredAppleArchive,
) -> Result<(), String> {
    let path = paths.apple_archive_record();
    let encoded = serde_json::to_vec_pretty(archive).map_err(|error| error.to_string())?;

    write_restricted_file(&path, &encoded)
}

pub(crate) fn remove_apple_archive_record(paths: &MachinePaths) -> Result<(), String> {
    remove_file_if_present(&paths.apple_archive_record())
}

pub(crate) fn save_apple_archive_error(paths: &MachinePaths, error: &str) -> Result<(), String> {
    let error = error
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .take(8_000)
        .collect::<String>();
    write_restricted_file(&paths.apple_archive_error(), error.as_bytes())
}

pub(crate) fn remove_apple_archive_error(paths: &MachinePaths) -> Result<(), String> {
    remove_file_if_present(&paths.apple_archive_error())
}

pub(crate) fn validated_apple_archive_directory(
    paths: &MachinePaths,
    result: &AppleArchiveResult,
) -> Result<PathBuf, String> {
    let root = paths.artifacts_dir();
    let root = fs::canonicalize(root)
        .map_err(|error| format!("The managed artifact directory is unavailable: {error}"))?;
    let ipa = fs::canonicalize(&result.ipa.path)
        .map_err(|error| format!("The retained IPA is unavailable: {error}"))?;
    let archive = fs::canonicalize(&result.archive.path)
        .map_err(|error| format!("The retained Xcode archive is unavailable: {error}"))?;
    if !ipa.is_file() || !archive.is_file() {
        return Err("The retained signed artifacts are no longer regular files.".to_string());
    }
    let directory = ipa
        .parent()
        .ok_or_else(|| "The retained IPA has no parent directory.".to_string())?;
    if archive.parent() != Some(directory) || directory.parent() != Some(root.as_path()) {
        return Err(
            "The signed artifact record is outside BuildBridge's managed directory.".to_string(),
        );
    }

    Ok(directory.to_path_buf())
}

pub(crate) fn save_signing_provisioning(
    paths: &MachinePaths,
    signing: &StoredSigningProvisioning,
) -> Result<(), String> {
    let path = paths.signing_provisioning();
    let encoded = serde_json::to_vec_pretty(signing).map_err(|error| error.to_string())?;

    write_restricted_file(&path, &encoded)
}

pub(crate) fn remove_signing_provisioning_record(paths: &MachinePaths) -> Result<(), String> {
    remove_file_if_present(&paths.signing_provisioning())
}

pub(crate) fn load_apple_workspace(
    paths: &MachinePaths,
) -> Result<Option<StoredAppleWorkspace>, String> {
    let path = paths.apple_workspace();

    match fs::read(path) {
        Ok(bytes) => {
            let mut workspace: StoredAppleWorkspace =
                serde_json::from_slice(&bytes).map_err(|error| {
                    format!("The approved Apple project configuration is invalid: {error}")
                })?;
            if workspace.development_team.is_none() || workspace.bundle_identifier.is_none() {
                let project_path = PathBuf::from(&workspace.local_path)
                    .join("ios/App/App.xcodeproj/project.pbxproj");
                if let Ok(project) = fs::read_to_string(project_path) {
                    workspace.development_team = one_xcode_setting(&project, "DEVELOPMENT_TEAM");
                    workspace.bundle_identifier = release_bundle_identifier(&project);
                }
            }

            Ok(Some(workspace))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn save_apple_workspace(
    paths: &MachinePaths,
    workspace: &StoredAppleWorkspace,
) -> Result<(), String> {
    let path = paths.apple_workspace();
    let encoded = serde_json::to_vec_pretty(workspace).map_err(|error| error.to_string())?;

    write_restricted_file(&path, &encoded)
}

pub(crate) fn inspect_apple_workspace(path: &str) -> Result<StoredAppleWorkspace, String> {
    if path.is_empty() {
        return Err("Choose an absolute local project directory.".to_string());
    }
    let requested = std::path::Path::new(path);
    if !requested.is_absolute() {
        return Err("The approved project path must be absolute.".to_string());
    }
    let canonical = fs::canonicalize(requested)
        .map_err(|error| format!("The selected project directory is unavailable: {error}"))?;
    if !canonical.is_dir() {
        return Err("The selected project path is not a directory.".to_string());
    }
    for required in [
        "package.json",
        "pnpm-lock.yaml",
        "capacitor.config.ts",
        "ios/App/Podfile",
        "ios/App/Podfile.lock",
        "ios/App/App.xcodeproj/project.pbxproj",
    ] {
        if !canonical.join(required).is_file() {
            return Err(format!("This project is missing {required}."));
        }
    }
    if !canonical.join("ios/App/App.xcworkspace").is_dir() {
        return Err("This project is missing ios/App/App.xcworkspace.".to_string());
    }

    let package: serde_json::Value = serde_json::from_slice(
        &fs::read(canonical.join("package.json"))
            .map_err(|error| format!("Could not read package.json: {error}"))?,
    )
    .map_err(|error| format!("package.json is invalid: {error}"))?;
    let name = package
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.trim().is_empty() && name.len() <= 120)
        .map(str::to_string)
        .or_else(|| {
            canonical
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .ok_or_else(|| "The selected project name is invalid.".to_string())?;
    let local_path = canonical
        .to_str()
        .filter(|path| path.len() <= 4_096)
        .ok_or_else(|| "The selected project path is not valid UTF-8.".to_string())?
        .to_string();
    let project_file = fs::read_to_string(canonical.join("ios/App/App.xcodeproj/project.pbxproj"))
        .map_err(|error| format!("Could not read the Xcode project settings: {error}"))?;
    let development_team = one_xcode_setting(&project_file, "DEVELOPMENT_TEAM");
    let bundle_identifier = release_bundle_identifier(&project_file);

    Ok(StoredAppleWorkspace {
        local_path,
        name,
        ios_workspace: "ios/App/App.xcworkspace".to_string(),
        scheme: "App".to_string(),
        development_team,
        bundle_identifier,
        last_snapshot_sha256: None,
        last_sync_file_count: None,
        last_sync_bytes: None,
        last_build_succeeded: false,
        last_xcode_version: None,
        last_native_lock_updated: false,
        last_build_target: None,
        debug_bundle_identifier: None,
        last_source: None,
    })
}

pub(crate) fn xcode_setting_values(project: &str, key: &str) -> Vec<String> {
    let prefix = format!("{key} = ");
    let mut values = Vec::new();
    for line in project.lines() {
        let line = line.trim();
        let Some(value) = line
            .strip_prefix(&prefix)
            .and_then(|value| value.strip_suffix(';'))
        else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        let valid = !value.is_empty()
            && value.len() <= 255
            && value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '-')
            });
        if valid && !values.iter().any(|stored| stored == value) {
            values.push(value.to_string());
        }
    }

    values
}

pub(crate) fn one_xcode_setting(project: &str, key: &str) -> Option<String> {
    let mut values = xcode_setting_values(project, key);

    (values.len() == 1).then(|| values.remove(0))
}

pub(crate) fn release_bundle_identifier(project: &str) -> Option<String> {
    let mut values = xcode_setting_values(project, "PRODUCT_BUNDLE_IDENTIFIER");
    if values.len() == 1 {
        return values.pop();
    }
    let release_values = values
        .into_iter()
        .filter(|value| !value.ends_with(".debug") && !value.ends_with(".Debug"))
        .collect::<Vec<_>>();

    (release_values.len() == 1).then(|| release_values[0].clone())
}

pub(crate) fn load_mac_guest_access(
    paths: &MachinePaths,
) -> Result<Option<StoredMacGuestAccess>, String> {
    let path = paths.guest_access();

    match fs::read(path) {
        Ok(bytes) => {
            let access: StoredMacGuestAccess = serde_json::from_slice(&bytes).map_err(|error| {
                format!("The macOS guest access configuration is invalid: {error}")
            })?;
            if !buildbridge_docker_osx::valid_guest_username(&access.username) {
                return Err("The stored macOS guest username is invalid.".to_string());
            }
            Ok(Some(access))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn save_mac_guest_access(
    paths: &MachinePaths,
    access: &StoredMacGuestAccess,
) -> Result<(), String> {
    let path = paths.guest_access();
    let encoded = serde_json::to_vec_pretty(access).map_err(|error| error.to_string())?;

    write_restricted_file(&path, &encoded)
}

pub(crate) fn ensure_mac_guest_keypair(identity_path: &std::path::Path) -> Result<String, String> {
    let public_key_path = identity_path.with_file_name("guest_ed25519.pub");
    match (identity_path.is_file(), public_key_path.is_file()) {
        (true, true) => return read_mac_guest_public_key(&public_key_path),
        (true, false) | (false, true) => {
            return Err(
                "The BuildBridge guest SSH keypair is incomplete. Restore the missing key before continuing."
                    .to_string(),
            );
        }
        (false, false) => {}
    }

    let parent = identity_path
        .parent()
        .ok_or_else(|| "The macOS guest key directory is unavailable.".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let output = Command::new("ssh-keygen")
        .args([
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            "buildbridge-guest",
            "-f",
        ])
        .arg(identity_path)
        .output()
        .map_err(|error| {
            format!("Could not run ssh-keygen; install OpenSSH client tools: {error}")
        })?;

    if !output.status.success() {
        return Err(format!(
            "Could not create the guest SSH key: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    set_restricted_permissions(identity_path)?;
    read_mac_guest_public_key(&public_key_path)
}

pub(crate) fn read_mac_guest_public_key(path: &std::path::Path) -> Result<String, String> {
    let public_key = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let public_key = public_key.trim();
    let fields: Vec<_> = public_key.split_whitespace().collect();

    if public_key.len() > 2_048
        || fields.len() < 2
        || fields.len() > 3
        || fields.first() != Some(&"ssh-ed25519")
    {
        return Err("The BuildBridge guest SSH public key is invalid.".to_string());
    }

    Ok(public_key.to_string())
}

pub(crate) fn read_optional_text(path: &std::path::Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value.trim().to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn write_restricted_file(path: &std::path::Path, contents: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "The macOS guest configuration directory is unavailable.".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    fs::write(path, contents).map_err(|error| error.to_string())?;
    set_restricted_permissions(path)
}

pub(crate) fn set_restricted_permissions(path: &std::path::Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}

pub(crate) fn set_restricted_directory_permissions(path: &std::path::Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}
