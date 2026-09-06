//! Host ADB previews from the machine's reviewed, retained APK; no running container needed.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AndroidDeviceBuildKind {
    Debug,
    Release,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidDeviceRunInput {
    pub kind: AndroidDeviceBuildKind,
    pub serial: String,
    pub expected_sha256: String,
}

/// What host ADB sees. The listing reads nothing of the machine's and changes nothing, so it
/// takes no machine operation: the device list refreshes while a build runs, and a slow
/// `adb devices` never holds a build up. Installing does claim the machine, below.
pub async fn list_android_devices(
    app: &Engine,
    machine_id: String,
) -> Result<AndroidDevices, String> {
    ensure_android_device_machine(app, &machine_id)?;
    tokio::task::spawn_blocking(buildbridge_machines::list_host_android_devices)
        .await
        .map_err(|error| error.to_string())?
}

pub async fn run_android_device(
    app: &Engine,
    machine_id: String,
    input: AndroidDeviceRunInput,
) -> Result<AndroidDeviceRunResult, String> {
    ensure_android_device_machine(app, &machine_id)?;
    let paths = MachinePaths::resolve(app, &machine_id)?;
    run_machine_operation(app, &machine_id, "running_android_device", move || {
        // Read the record after claiming the machine; every hash/path comes from that record.
        let (application_id, artifact) = match input.kind {
            AndroidDeviceBuildKind::Debug => {
                let build = load_android_workspace(&paths)?.and_then(|workspace| workspace.last_build)
                    .ok_or_else(|| "Build and retain a debug APK before running it on a device.".to_string())?;
                (build.application_id, build.apk.ok_or_else(|| "No debug APK is retained. Run a new debug build.".to_string())?)
            }
            AndroidDeviceBuildKind::Release => {
                let release = load_android_release(&paths)?
                    .ok_or_else(|| "Build and retain a release APK before running it on a device.".to_string())?.result;
                (release.application_id, release.apk.ok_or_else(|| "This release contains only an app bundle. Build a release APK to run it on a device.".to_string())?)
            }
        };
        let apk = reviewed_android_apk_path(&paths, &artifact, input.kind, &input.expected_sha256)?;
        verify_android_apk(&apk, artifact.bytes, &input.expected_sha256)?;
        buildbridge_machines::run_host_android_device(
            &input.serial, &application_id, &apk, &input.expected_sha256,
        )
    }).await
}

fn ensure_android_device_machine(app: &Engine, machine_id: &str) -> Result<(), String> {
    if machines::load_registry(app)?
        .find(machine_id)?
        .config
        .provider
        != MachineProvider::AndroidToolchain
    {
        return Err("Choose an Android machine to install an Android APK.".into());
    }
    Ok(())
}

fn reviewed_android_apk_path(
    paths: &MachinePaths,
    artifact: &AndroidArtifact,
    kind: AndroidDeviceBuildKind,
    expected_sha256: &str,
) -> Result<PathBuf, String> {
    if expected_sha256.len() != 64
        || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !artifact.sha256.eq_ignore_ascii_case(expected_sha256)
    {
        return Err("The retained APK changed. Review the current build before installing.".into());
    }
    let prefix = match kind {
        AndroidDeviceBuildKind::Debug => "debug-",
        AndroidDeviceBuildKind::Release => "release-",
    };
    validated_android_artifact_directory(paths, &artifact.path, prefix)?;
    let path = fs::canonicalize(&artifact.path)
        .map_err(|error| format!("The retained APK is unavailable: {error}"))?;
    if path.extension().and_then(|extension| extension.to_str()) != Some("apk") {
        return Err("Only a retained APK can be installed on an Android device.".into());
    }
    Ok(path)
}

fn verify_android_apk(path: &std::path::Path, bytes: u64, sha256: &str) -> Result<(), String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("The retained APK is unavailable: {error}"))?;
    if !metadata.is_file() || bytes == 0 || metadata.len() != bytes {
        return Err("The retained APK's size changed. Build a new APK before installing.".into());
    }
    if !buildbridge_machines::native_sha256(path)?.eq_ignore_ascii_case(sha256) {
        return Err(
            "The retained APK's checksum changed. Build a new APK before installing.".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    struct Fixture {
        root: PathBuf,
        paths: MachinePaths,
        artifact: AndroidArtifact,
    }

    impl Fixture {
        fn new(kind: AndroidDeviceBuildKind) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "buildbridge-android-device-{}-{nonce}",
                std::process::id()
            ));
            let app = Engine::new(EngineDeps {
                config_dir: root.join("config"),
                data_dir: root.join("data"),
                events: Arc::new(NoEvents),
            });
            let paths = MachinePaths::resolve(&app, "device-test").unwrap();
            let prefix = if kind == AndroidDeviceBuildKind::Debug {
                "debug"
            } else {
                "release"
            };
            let directory = paths.artifacts_dir().join(format!("{prefix}-test"));
            fs::create_dir_all(&directory).unwrap();
            let apk = directory.join("App.apk");
            fs::write(&apk, b"abc").unwrap();
            let artifact = AndroidArtifact {
                path: apk.to_string_lossy().into_owned(),
                bytes: 3,
                sha256: SHA256.into(),
            };
            Self {
                root,
                paths,
                artifact,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn reviewed_debug_and_release_apks_are_verified_without_a_running_container() {
        for kind in [
            AndroidDeviceBuildKind::Debug,
            AndroidDeviceBuildKind::Release,
        ] {
            let fixture = Fixture::new(kind);
            let expected = SHA256.to_ascii_uppercase();
            let path =
                reviewed_android_apk_path(&fixture.paths, &fixture.artifact, kind, &expected)
                    .unwrap();
            verify_android_apk(&path, fixture.artifact.bytes, &expected).unwrap();
        }
    }

    #[test]
    fn stale_or_malformed_hashes_and_wrong_build_kinds_are_refused() {
        let fixture = Fixture::new(AndroidDeviceBuildKind::Debug);
        for expected in [
            String::new(),
            "a".repeat(63),
            "g".repeat(64),
            "a".repeat(64),
        ] {
            let error = reviewed_android_apk_path(
                &fixture.paths,
                &fixture.artifact,
                AndroidDeviceBuildKind::Debug,
                &expected,
            )
            .unwrap_err();
            assert!(error.contains("Review the current build"), "{error}");
        }
        assert!(
            reviewed_android_apk_path(
                &fixture.paths,
                &fixture.artifact,
                AndroidDeviceBuildKind::Release,
                SHA256
            )
            .is_err()
        );
    }

    #[test]
    fn changed_content_size_and_missing_apks_are_refused() {
        let fixture = Fixture::new(AndroidDeviceBuildKind::Debug);
        let path = PathBuf::from(&fixture.artifact.path);
        fs::write(&path, b"abd").unwrap();
        assert!(
            verify_android_apk(&path, 3, SHA256)
                .unwrap_err()
                .contains("checksum changed")
        );
        fs::write(&path, b"abcd").unwrap();
        assert!(
            verify_android_apk(&path, 3, SHA256)
                .unwrap_err()
                .contains("size changed")
        );
        fs::write(&path, b"").unwrap();
        assert!(verify_android_apk(&path, 0, SHA256).is_err());
        fs::remove_file(&path).unwrap();
        assert!(verify_android_apk(&path, 3, SHA256).is_err());
    }

    #[test]
    fn outside_managed_directory_and_non_apk_records_are_refused() {
        let mut fixture = Fixture::new(AndroidDeviceBuildKind::Debug);
        let outside = fixture.root.join("App.apk");
        fs::rename(&fixture.artifact.path, &outside).unwrap();
        fixture.artifact.path = outside.to_string_lossy().into_owned();
        assert!(
            reviewed_android_apk_path(
                &fixture.paths,
                &fixture.artifact,
                AndroidDeviceBuildKind::Debug,
                SHA256
            )
            .unwrap_err()
            .contains("outside BuildBridge")
        );

        let directory = fixture.paths.artifacts_dir().join("debug-test");
        let bundle = directory.join("App.aab");
        fs::rename(&outside, &bundle).unwrap();
        fixture.artifact.path = bundle.to_string_lossy().into_owned();
        assert!(
            reviewed_android_apk_path(
                &fixture.paths,
                &fixture.artifact,
                AndroidDeviceBuildKind::Debug,
                SHA256
            )
            .unwrap_err()
            .contains("Only a retained APK")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_escape_the_managed_artifact_directory() {
        let fixture = Fixture::new(AndroidDeviceBuildKind::Debug);
        let outside = fixture.root.join("outside.apk");
        fs::write(&outside, b"abc").unwrap();
        fs::remove_file(&fixture.artifact.path).unwrap();
        std::os::unix::fs::symlink(&outside, &fixture.artifact.path).unwrap();
        let error = reviewed_android_apk_path(
            &fixture.paths,
            &fixture.artifact,
            AndroidDeviceBuildKind::Debug,
            SHA256,
        )
        .unwrap_err();
        assert!(error.contains("outside BuildBridge"), "{error}");
    }
}
