//! Per-machine records on disk: workspace, signing, archive, and the guest key material.

use super::*;
use ts_rs::TS;

pub(crate) fn prepare_apple_archive_output_dir(paths: &MachinePaths) -> Result<PathBuf, String> {
    prepare_artifact_output_dir(paths, "archive")
}

pub(crate) fn prepare_android_release_output_dir(paths: &MachinePaths) -> Result<PathBuf, String> {
    prepare_artifact_output_dir(paths, "release")
}

pub(crate) fn prepare_android_debug_output_dir(paths: &MachinePaths) -> Result<PathBuf, String> {
    prepare_artifact_output_dir(paths, "debug")
}

/// Removes every retained debug build of a machine: the directories this engine named
/// `debug-…` directly under its artifacts, and nothing else there.
pub(crate) fn remove_android_debug_outputs(paths: &MachinePaths) -> Result<(), String> {
    let root = paths.artifacts_dir();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let is_debug = name.to_str().is_some_and(|name| name.starts_with("debug-"));
        if is_debug
            && entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
        {
            fs::remove_dir_all(entry.path())
                .map_err(|error| format!("Could not remove the previous debug APK: {error}"))?;
        }
    }

    Ok(())
}

/// The directory holding one retained artifact, once it is proven to be a regular file directly
/// under a `prefix`-named directory of this machine's managed artifacts.
pub(crate) fn validated_android_artifact_directory(
    paths: &MachinePaths,
    artifact_path: &str,
    prefix: &str,
) -> Result<PathBuf, String> {
    let root = fs::canonicalize(paths.artifacts_dir())
        .map_err(|error| format!("The managed artifact directory is unavailable: {error}"))?;
    let artifact = fs::canonicalize(artifact_path)
        .map_err(|error| format!("The retained artifact is unavailable: {error}"))?;
    if !artifact.is_file() {
        return Err("The retained artifact is no longer a regular file.".to_string());
    }
    let directory = artifact
        .parent()
        .ok_or_else(|| "The retained artifact has no parent directory.".to_string())?;
    let named = directory
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(prefix));
    if directory.parent() != Some(root.as_path()) || !named {
        return Err("The artifact record is outside BuildBridge's managed directory.".to_string());
    }

    Ok(directory.to_path_buf())
}

/// A fresh owner-only directory under the machine's artifacts for one build's outputs.
pub(crate) fn prepare_artifact_output_dir(
    paths: &MachinePaths,
    prefix: &str,
) -> Result<PathBuf, String> {
    let root = paths.artifacts_dir();
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    set_restricted_directory_permissions(&root)?;
    let operation_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "The system clock is earlier than the Unix epoch.".to_string())?
        .as_millis();
    let directory = root.join(format!("{prefix}-{operation_id}-{}", std::process::id()));
    fs::create_dir(&directory).map_err(|error| error.to_string())?;
    set_restricted_directory_permissions(&directory)?;

    Ok(directory)
}

pub(crate) fn load_android_workspace(
    paths: &MachinePaths,
) -> Result<Option<StoredAndroidWorkspace>, String> {
    match fs::read(paths.android_workspace()) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            format!("The approved Android project configuration is invalid: {error}")
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn save_android_workspace(
    paths: &MachinePaths,
    workspace: &StoredAndroidWorkspace,
) -> Result<(), String> {
    let encoded = serde_json::to_vec_pretty(workspace).map_err(|error| error.to_string())?;

    write_restricted_file(&paths.android_workspace(), &encoded)
}

/// The retained release, if there is one. A record that names no artifact is not a release —
/// nothing this engine writes has that shape — so it reads as none, the way a missing file
/// does: the machine view still opens and clearing the release removes the file.
pub(crate) fn load_android_release(
    paths: &MachinePaths,
) -> Result<Option<StoredAndroidRelease>, String> {
    match fs::read(paths.android_release_record()) {
        Ok(bytes) => {
            let release: StoredAndroidRelease =
                serde_json::from_slice(&bytes).map_err(|error| {
                    format!("The retained Android release record is invalid: {error}")
                })?;
            Ok(require_android_release_artifact(&release.result)
                .is_ok()
                .then_some(release))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn save_android_release(
    paths: &MachinePaths,
    release: &StoredAndroidRelease,
) -> Result<(), String> {
    require_android_release_artifact(&release.result)?;
    let encoded = serde_json::to_vec_pretty(release).map_err(|error| error.to_string())?;

    write_restricted_file(&paths.android_release_record(), &encoded)
}

pub(crate) fn remove_android_release_record(paths: &MachinePaths) -> Result<(), String> {
    remove_file_if_present(&paths.android_release_record())
}

pub(crate) fn save_android_release_error(paths: &MachinePaths, error: &str) -> Result<(), String> {
    let error = error
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .take(8_000)
        .collect::<String>();
    write_restricted_file(&paths.android_release_error(), error.as_bytes())
}

pub(crate) fn remove_android_release_error(paths: &MachinePaths) -> Result<(), String> {
    remove_file_if_present(&paths.android_release_error())
}

fn require_android_release_artifact(result: &AndroidReleaseResult) -> Result<(), String> {
    if result.aab.is_none() && result.apk.is_none() {
        return Err("The retained Android release has no artifacts.".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod android_release_record_tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        paths: MachinePaths,
    }
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "buildbridge-release-record-{}-{nonce}",
                std::process::id()
            ));
            let app = Engine::new(EngineDeps {
                config_dir: root.join("config"),
                data_dir: root.join("data"),
                events: Arc::new(NoEvents),
            });
            let paths = MachinePaths::resolve(&app, "android-test").unwrap();
            Self { root, paths }
        }
        fn result(&self) -> AndroidReleaseResult {
            let directory = self.paths.artifacts_dir().join("release-test");
            fs::create_dir_all(&directory).unwrap();
            let artifact = |name| {
                let path = directory.join(name);
                fs::write(&path, b"fixture").unwrap();
                AndroidArtifact {
                    path: path.to_string_lossy().into_owned(),
                    bytes: 7,
                    sha256: "a".repeat(64),
                }
            };
            AndroidReleaseResult {
                application_id: "com.example.app".into(),
                version_name: "1.0".into(),
                version_code: "1".into(),
                key_alias: "upload".into(),
                certificate_sha256: "b".repeat(64),
                aab: Some(artifact("app.aab")),
                apk: Some(artifact("app.apk")),
                output_tail: vec![],
            }
        }
        fn record(&self, result: AndroidReleaseResult) -> StoredAndroidRelease {
            StoredAndroidRelease {
                container_id: "container".into(),
                snapshot_sha256: "c".repeat(64),
                result,
                env_set_name: None,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn selected_artifacts_round_trip_and_legacy_both_records_remain_readable() {
        let fixture = Fixture::new();
        for selection in [
            AndroidReleaseOutputs::Both,
            AndroidReleaseOutputs::Aab,
            AndroidReleaseOutputs::Apk,
        ] {
            let mut result = fixture.result();
            if !selection.includes_aab() {
                result.aab = None;
            }
            if !selection.includes_apk() {
                result.apk = None;
            }
            save_android_release(&fixture.paths, &fixture.record(result.clone())).unwrap();
            let read = load_android_release(&fixture.paths).unwrap().unwrap();
            assert_eq!(read.result, result);
            assert!(validated_android_release_directory(&fixture.paths, &read.result).is_ok());
            let report = android_result_json(&read.result, None);
            assert_eq!(
                report["artifacts"].as_array().unwrap().len(),
                if selection == AndroidReleaseOutputs::Both {
                    2
                } else {
                    1
                }
            );
            assert!(
                !report
                    .to_string()
                    .contains(&fixture.root.to_string_lossy().to_string())
            );
        }
    }

    #[test]
    fn an_empty_release_cannot_be_saved_or_revealed_and_loads_as_no_release() {
        let fixture = Fixture::new();
        let mut result = fixture.result();
        result.aab = None;
        result.apk = None;
        assert!(validated_android_release_directory(&fixture.paths, &result).is_err());
        let record = fixture.record(result);
        assert!(save_android_release(&fixture.paths, &record).is_err());
        let path = fixture.paths.android_release_record();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(
            load_android_release(&fixture.paths).unwrap().is_none(),
            "a torn record must not brick the machine view"
        );
        remove_android_release_record(&fixture.paths).unwrap();
        assert!(
            !path.exists(),
            "clearing the release removes the torn record"
        );
        assert!(load_android_release(&fixture.paths).unwrap().is_none());
        fs::write(&path, b"{not json").unwrap();
        assert!(
            load_android_release(&fixture.paths).is_err(),
            "unreadable JSON is still reported"
        );
    }

    #[test]
    fn selected_files_must_exist_in_one_managed_release_directory() {
        let fixture = Fixture::new();
        let mut result = fixture.result();
        let outside = fixture.root.join("outside.apk");
        fs::write(&outside, b"outside").unwrap();
        result.apk.as_mut().unwrap().path = outside.to_string_lossy().into_owned();
        assert!(validated_android_release_directory(&fixture.paths, &result).is_err());
        result.aab = None;
        assert!(validated_android_release_directory(&fixture.paths, &result).is_err());
        let other = fixture.paths.artifacts_dir().join("release-other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("app.apk"), b"other").unwrap();
        result = fixture.result();
        result.apk.as_mut().unwrap().path = other.join("app.apk").to_string_lossy().into_owned();
        assert!(validated_android_release_directory(&fixture.paths, &result).is_err());
        result.aab = None;
        assert_eq!(
            validated_android_release_directory(&fixture.paths, &result).unwrap(),
            fs::canonicalize(&other).unwrap()
        );
        fs::remove_file(other.join("app.apk")).unwrap();
        assert!(validated_android_release_directory(&fixture.paths, &result).is_err());
        let debug = fixture.paths.artifacts_dir().join("debug-test");
        fs::create_dir_all(&debug).unwrap();
        fs::write(debug.join("app.apk"), b"debug").unwrap();
        result.apk.as_mut().unwrap().path = debug.join("app.apk").to_string_lossy().into_owned();
        assert!(validated_android_release_directory(&fixture.paths, &result).is_err());
        assert!(debug.join("app.apk").is_file());
    }
}

/// The directory shared by the selected regular files directly under the managed artifact root.
pub(crate) fn validated_android_release_directory(
    paths: &MachinePaths,
    result: &AndroidReleaseResult,
) -> Result<PathBuf, String> {
    require_android_release_artifact(result)?;
    let root = fs::canonicalize(paths.artifacts_dir())
        .map_err(|error| format!("The managed artifact directory is unavailable: {error}"))?;
    let mut directory: Option<PathBuf> = None;
    for artifact in result.aab.iter().chain(result.apk.iter()) {
        let path = fs::canonicalize(&artifact.path)
            .map_err(|error| format!("A retained signed artifact is unavailable: {error}"))?;
        if !path.is_file() {
            return Err("The retained signed artifacts are no longer regular files.".to_string());
        }
        let parent = path
            .parent()
            .ok_or_else(|| "The retained signed artifact has no parent directory.".to_string())?;
        if parent.parent() != Some(root.as_path())
            || !parent
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("release-"))
            || directory
                .as_deref()
                .is_some_and(|directory| directory != parent)
        {
            return Err(
                "The signed artifact record is outside BuildBridge's managed directory."
                    .to_string(),
            );
        }
        directory = Some(parent.to_path_buf());
    }
    directory.ok_or_else(|| "The retained Android release has no artifacts.".to_string())
}

/// Approves a Capacitor project for an Android machine: the same package and lock the iOS
/// side requires, plus the Android platform with its committed Gradle wrapper.
pub(crate) fn inspect_android_workspace(path: &str) -> Result<StoredAndroidWorkspace, String> {
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
        "android/gradlew",
    ] {
        if !canonical.join(required).is_file() {
            return Err(format!("This project is missing {required}."));
        }
    }
    let app_script = ["android/app/build.gradle", "android/app/build.gradle.kts"]
        .into_iter()
        .map(|relative| canonical.join(relative))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| "This project is missing android/app/build.gradle.".to_string())?;
    if !canonical.join("android/settings.gradle").is_file()
        && !canonical.join("android/settings.gradle.kts").is_file()
    {
        return Err("This project is missing android/settings.gradle.".to_string());
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
    let script = fs::read_to_string(&app_script)
        .map_err(|error| format!("Could not read the app module's Gradle script: {error}"))?;

    Ok(StoredAndroidWorkspace {
        local_path,
        name,
        application_id: gradle_application_id(&script),
        last_snapshot_sha256: None,
        last_sync_file_count: None,
        last_sync_bytes: None,
        last_build_succeeded: false,
        last_build: None,
        last_source: None,
    })
}

/// The `applicationId` an app module's Gradle script declares as a literal, in either the
/// Groovy or the Kotlin spelling. A computed value is left unknown rather than guessed.
pub(crate) fn gradle_application_id(script: &str) -> Option<String> {
    script.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix("applicationId")?;
        let rest = rest.trim_start().strip_prefix('=').unwrap_or(rest).trim();
        let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let value = rest[1..].split(quote).next()?;
        let valid = !value.is_empty()
            && value.len() <= 255
            && value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_')
            });
        valid.then(|| value.to_string())
    })
}

pub(crate) fn remove_file_if_present(path: &std::path::Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn managed_apple_profiles_dir(app: &Engine) -> Result<PathBuf, String> {
    Ok(app.config_dir().join("macos-builder").join("profiles"))
}

/// A provisioning profile this host already downloaded, kept so a kit can be rebuilt without
/// going back to Apple. Only the file name and path are exposed; the contents stay on disk.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAppleProfile {
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
            if !buildbridge_machines::valid_guest_username(&access.username) {
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

/// Writes an owner-only file in one step: the bytes go to a sibling temporary file that is
/// created private (mode 0600 from its first byte, under any umask), synced, and renamed over
/// the target. A reader never sees a half-written record, and a crash leaves the previous
/// file whole. The temporary file is removed on any failure.
pub(crate) fn write_restricted_file(path: &std::path::Path, contents: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "The macOS guest configuration directory is unavailable.".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "The record's file name is invalid.".to_string())?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let temporary = parent.join(format!(".{name}.{}-{nonce}.tmp", std::process::id()));
    let written = (|| {
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        set_restricted_permissions(&temporary)?;
        file.write_all(contents)
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        fs::rename(&temporary, path).map_err(|error| error.to_string())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
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

#[cfg(test)]
mod restricted_file_tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "buildbridge-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn leftovers(directory: &std::path::Path) -> Vec<String> {
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn restricted_files_are_private_whole_and_leave_no_temporary_behind() {
        let directory = scratch("restricted-file");
        let path = directory.join("nested").join("record.json");
        write_restricted_file(&path, b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        write_restricted_file(&path, b"second, longer than the first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second, longer than the first");
        assert!(leftovers(path.parent().unwrap()).is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            // A file that was already readable by others becomes private once rewritten.
            let open = directory.join("open.json");
            fs::write(&open, b"public").unwrap();
            fs::set_permissions(&open, fs::Permissions::from_mode(0o644)).unwrap();
            write_restricted_file(&open, b"private").unwrap();
            assert_eq!(
                fs::metadata(&open).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(fs::read(&open).unwrap(), b"private");
        }
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_failed_write_keeps_the_previous_file_and_removes_its_temporary() {
        let directory = scratch("restricted-file-failure");
        let path = directory.join("record.json");
        write_restricted_file(&path, b"kept").unwrap();
        // The target is now a directory, so the rename over it fails after the bytes went out.
        let blocked = directory.join("blocked");
        fs::create_dir_all(blocked.join("child")).unwrap();
        fs::write(blocked.join("child").join("keep"), b"x").unwrap();
        assert!(write_restricted_file(&blocked, b"replacement").is_err());
        assert!(blocked.join("child").join("keep").is_file());
        assert!(leftovers(&directory).is_empty());
        assert_eq!(fs::read(&path).unwrap(), b"kept");
        fs::remove_dir_all(&directory).unwrap();
    }
}

#[cfg(test)]
mod source_record_tests {
    use super::*;

    #[test]
    fn gradle_application_id_reads_literals_in_either_dialect_and_leaves_computed_values_unknown() {
        assert_eq!(
            gradle_application_id(
                "android {\n  defaultConfig {\n    applicationId \"com.example.app\"\n"
            ),
            Some("com.example.app".to_string())
        );
        assert_eq!(
            gradle_application_id("    applicationId = 'nz.co.think_solar.app'"),
            Some("nz.co.think_solar.app".to_string())
        );
        assert_eq!(
            gradle_application_id("applicationId=\"com.example.app\" // release"),
            Some("com.example.app".to_string())
        );
        assert_eq!(
            gradle_application_id("applicationId \"com.first\"\napplicationId \"com.second\""),
            Some("com.first".to_string())
        );
        for script in [
            "applicationId project.ext.appId",
            "applicationId \"com.example.app;rm -rf\"",
            "applicationId \"com.example.app/../x\"",
            "applicationId \"\"",
            "applicationIdSuffix \".debug\"",
            "namespace \"com.example.app\"",
            "// applicationId \"com.example.app\"",
            &format!("applicationId \"{}\"", "a".repeat(256)),
        ] {
            assert_eq!(gradle_application_id(script), None, "{script}");
        }
    }

    #[test]
    fn guest_public_keys_must_be_a_single_ed25519_line() {
        let directory = std::env::temp_dir().join(format!(
            "buildbridge-guest-key-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("id_ed25519.pub");
        fs::write(
            &path,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample builder@host\n",
        )
        .unwrap();
        assert_eq!(
            read_mac_guest_public_key(&path).unwrap(),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample builder@host"
        );
        fs::write(&path, "  ssh-ed25519 AAAA\n").unwrap();
        assert_eq!(
            read_mac_guest_public_key(&path).unwrap(),
            "ssh-ed25519 AAAA"
        );
        for bad in [
            "ssh-rsa AAAA builder@host",
            "ssh-ed25519",
            "ssh-ed25519 AAAA builder@host extra",
            "",
            "ssh-ed25519 AAAA\nssh-ed25519 BBBB",
            &format!("ssh-ed25519 {}", "A".repeat(2_040)),
        ] {
            fs::write(&path, bad).unwrap();
            assert_eq!(
                read_mac_guest_public_key(&path).unwrap_err(),
                "The BuildBridge guest SSH public key is invalid.",
                "{bad:?}"
            );
        }
        assert!(read_mac_guest_public_key(&directory.join("missing.pub")).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
