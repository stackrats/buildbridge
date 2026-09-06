//! Explicit delivery of a retained App Store IPA through the managed Mac's Transporter.

use super::*;

/// Uploads precisely the IPA the caller reviewed. Apple processes the delivery separately;
/// this operation neither publishes a release nor changes the successful archive record.
pub async fn upload_apple_archive(
    app: &Engine,
    machine_id: String,
    expected_sha256: String,
) -> Result<(), String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let profile = machines::load_registry(app)?
        .find(&machine_id)?
        .config
        .clone();
    if !profile.provider.is_macos() {
        return Err("Transporter uploads require a macOS machine.".to_string());
    }

    // Claim the machine before reading its retained archive, so rebuilding or deleting it
    // cannot change what this explicit upload refers to.
    let guard = begin_machine_operation(app, &machine_id, "uploading_archive")?;
    let stored = load_apple_archive(&paths)?
        .ok_or_else(|| "Build a signed App Store IPA before uploading.".to_string())?;
    let ipa_path = reviewed_ipa_path(&paths, &stored.result, &expected_sha256)?;
    let access = load_mac_guest_access(&paths)?
        .ok_or_else(|| "Configure the macOS short username before uploading.".to_string())?;
    let current = build_machine_view(app, &paths).await?;
    if current.runtime.state != ContainerState::Running {
        return Err("Start the macOS machine before uploading with Transporter.".to_string());
    }
    if current.guest.ssh.trust != GuestTrustState::Trusted
        || !current.guest.diagnostics.authenticated
    {
        return Err("Finish the pinned macOS guest connection before uploading.".to_string());
    }
    let credentials = resolve_signing_kit_for(app, &machine_id).await?;
    let (key_id, issuer_id, private_key) = match (
        credentials.app_store_connect_key_id,
        credentials.app_store_connect_issuer_id,
        credentials.app_store_connect_private_key,
    ) {
        (Some(key), Some(issuer), Some(private)) => (key, issuer, private),
        _ => {
            return Err(
                "Add an App Store Connect Team API key (Key ID, Issuer ID and .p8 private key) to this machine's signing credentials before uploading."
                    .to_string(),
            );
        }
    };
    let identity_path = paths.guest_identity();
    let known_hosts_path = paths.known_hosts();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let event_app = app.clone();
    let bytes = stored.result.ipa.bytes;
    let joined = tokio::task::spawn_blocking(move || {
        let _operation = buildbridge_machines::enter_operation(scope);
        verify_reviewed_ipa(&ipa_path, bytes, &expected_sha256)?;
        buildbridge_machines::upload_apple_ipa(
            profile.ssh_port,
            &access.username,
            &identity_path,
            &known_hosts_path,
            &ipa_path,
            &expected_sha256,
            &key_id,
            &issuer_id,
            &private_key,
            |detail| {
                emit_machine_progress(
                    &event_app,
                    "machine-upload-progress",
                    &machine_id,
                    serde_json::json!({ "detail": detail }),
                );
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    let result = finish_operation(&cancel_probe, joined);
    drop(guard);
    result
}

fn reviewed_ipa_path(
    paths: &MachinePaths,
    archive: &AppleArchiveResult,
    expected_sha256: &str,
) -> Result<PathBuf, String> {
    if expected_sha256.len() != 64
        || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !archive.ipa.sha256.eq_ignore_ascii_case(expected_sha256)
    {
        return Err(
            "The retained IPA changed. Review the current release before uploading.".into(),
        );
    }
    if !matches!(
        archive.export_method.as_str(),
        "app-store" | "app-store-connect"
    ) {
        return Err("Only an App Store exported IPA can be uploaded with Transporter.".into());
    }
    validated_apple_archive_directory(paths, archive)?;
    let path = fs::canonicalize(&archive.ipa.path)
        .map_err(|error| format!("The retained IPA is unavailable: {error}"))?;
    if path.extension().and_then(|extension| extension.to_str()) != Some("ipa") {
        return Err("The retained upload artifact must be an IPA file.".into());
    }
    Ok(path)
}

fn verify_reviewed_ipa(path: &std::path::Path, bytes: u64, sha256: &str) -> Result<(), String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("The retained IPA is unavailable: {error}"))?;
    if !metadata.is_file() || bytes == 0 || metadata.len() != bytes {
        return Err(
            "The retained IPA's size changed. Build a new release before uploading.".into(),
        );
    }
    if !buildbridge_machines::native_sha256(path)?.eq_ignore_ascii_case(sha256) {
        return Err(
            "The retained IPA's checksum changed. Build a new release before uploading.".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const IPA_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    struct Fixture {
        root: PathBuf,
        app: Engine,
        paths: MachinePaths,
        archive: AppleArchiveResult,
    }

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "buildbridge-publishing-{}-{nonce}",
                std::process::id()
            ));
            let app = Engine::new(EngineDeps {
                config_dir: root.join("config"),
                data_dir: root.join("data"),
                events: Arc::new(NoEvents),
            });
            let paths = MachinePaths::resolve(&app, "upload-test").unwrap();
            let directory = paths.artifacts_dir().join("archive-test");
            fs::create_dir_all(&directory).unwrap();
            let ipa_path = directory.join("App-AppStore.ipa");
            let archive_path = directory.join("App.xcarchive.zip");
            fs::write(&ipa_path, b"abc").unwrap();
            fs::write(&archive_path, b"archive").unwrap();
            let archive = AppleArchiveResult {
                scheme: "App".into(),
                configuration: "Release".into(),
                export_method: "app-store-connect".into(),
                bundle_identifier: "com.example.app".into(),
                development_team: "ABCDEFGHIJ".into(),
                marketing_version: "1.0".into(),
                build_number: "1".into(),
                provisioning_profile_uuid: "profile".into(),
                ipa: AppleArchiveArtifact {
                    path: ipa_path.to_string_lossy().into_owned(),
                    bytes: 3,
                    sha256: IPA_SHA256.into(),
                },
                archive: AppleArchiveArtifact {
                    path: archive_path.to_string_lossy().into_owned(),
                    bytes: 7,
                    sha256: "a".repeat(64),
                },
                output_tail: vec![],
            };
            Self {
                root,
                app,
                paths,
                archive,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn both_app_store_export_names_accept_the_reviewed_ipa() {
        let mut fixture = Fixture::new();
        for method in ["app-store", "app-store-connect"] {
            fixture.archive.export_method = method.into();
            let expected = IPA_SHA256.to_ascii_uppercase();
            let path = reviewed_ipa_path(&fixture.paths, &fixture.archive, &expected).unwrap();
            assert_eq!(path, fs::canonicalize(&fixture.archive.ipa.path).unwrap());
            verify_reviewed_ipa(&path, fixture.archive.ipa.bytes, &expected).unwrap();
        }
    }

    #[test]
    fn stale_or_malformed_reviewed_hashes_are_refused() {
        let fixture = Fixture::new();
        for expected in [
            String::new(),
            "a".repeat(63),
            "a".repeat(65),
            "g".repeat(64),
            "a".repeat(64),
        ] {
            let error = reviewed_ipa_path(&fixture.paths, &fixture.archive, &expected).unwrap_err();
            assert!(error.contains("Review the current release"), "{error}");
        }
    }

    #[test]
    fn same_size_content_changes_are_refused() {
        let fixture = Fixture::new();
        let path = reviewed_ipa_path(&fixture.paths, &fixture.archive, IPA_SHA256).unwrap();
        fs::write(&path, b"abd").unwrap();
        let error = verify_reviewed_ipa(&path, fixture.archive.ipa.bytes, IPA_SHA256).unwrap_err();
        assert!(error.contains("checksum changed"), "{error}");
    }

    #[test]
    fn empty_missing_and_changed_size_ipas_are_refused() {
        let fixture = Fixture::new();
        let path = reviewed_ipa_path(&fixture.paths, &fixture.archive, IPA_SHA256).unwrap();
        for recorded_size in [0, 2, 4] {
            let error = verify_reviewed_ipa(&path, recorded_size, IPA_SHA256).unwrap_err();
            assert!(error.contains("size changed"), "{error}");
        }
        fs::write(&path, b"").unwrap();
        assert!(verify_reviewed_ipa(&path, 0, IPA_SHA256).is_err());
        fs::remove_file(&path).unwrap();
        assert!(verify_reviewed_ipa(&path, 3, IPA_SHA256).is_err());
        assert!(reviewed_ipa_path(&fixture.paths, &fixture.archive, IPA_SHA256).is_err());
        fs::create_dir(&path).unwrap();
        assert!(verify_reviewed_ipa(&path, 3, IPA_SHA256).is_err());
        assert!(reviewed_ipa_path(&fixture.paths, &fixture.archive, IPA_SHA256).is_err());
    }

    #[test]
    fn artifacts_outside_the_machine_directory_are_refused() {
        let mut fixture = Fixture::new();
        let outside = fixture.root.join("archive-outside");
        fs::create_dir(&outside).unwrap();
        for artifact in [&mut fixture.archive.ipa, &mut fixture.archive.archive] {
            let destination =
                outside.join(std::path::Path::new(&artifact.path).file_name().unwrap());
            fs::rename(&artifact.path, &destination).unwrap();
            artifact.path = destination.to_string_lossy().into_owned();
        }
        let error = reviewed_ipa_path(&fixture.paths, &fixture.archive, IPA_SHA256).unwrap_err();
        assert!(
            error.contains("outside BuildBridge's managed directory"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_escape_the_managed_archive_directory() {
        let fixture = Fixture::new();
        let outside = fixture.root.join("outside.ipa");
        fs::write(&outside, b"abc").unwrap();
        fs::remove_file(&fixture.archive.ipa.path).unwrap();
        std::os::unix::fs::symlink(&outside, &fixture.archive.ipa.path).unwrap();
        let error = reviewed_ipa_path(&fixture.paths, &fixture.archive, IPA_SHA256).unwrap_err();
        assert!(
            error.contains("outside BuildBridge's managed directory"),
            "{error}"
        );
    }

    #[test]
    fn non_app_store_exports_and_non_ipa_files_are_refused() {
        let mut fixture = Fixture::new();
        for method in ["development", "ad-hoc", "enterprise", "debugging"] {
            fixture.archive.export_method = method.into();
            let error =
                reviewed_ipa_path(&fixture.paths, &fixture.archive, IPA_SHA256).unwrap_err();
            assert!(error.contains("Only an App Store exported IPA"), "{error}");
        }
        fixture.archive.export_method = "app-store-connect".into();
        let renamed = std::path::Path::new(&fixture.archive.ipa.path).with_extension("zip");
        fs::rename(&fixture.archive.ipa.path, &renamed).unwrap();
        fixture.archive.ipa.path = renamed.to_string_lossy().into_owned();
        let error = reviewed_ipa_path(&fixture.paths, &fixture.archive, IPA_SHA256).unwrap_err();
        assert!(error.contains("must be an IPA file"), "{error}");
    }

    #[tokio::test]
    async fn clearing_an_archive_cannot_remove_an_in_flight_upload() {
        let fixture = Fixture::new();
        save_apple_archive(
            &fixture.paths,
            &StoredAppleArchive {
                container_id: "container".into(),
                snapshot_sha256: "b".repeat(64),
                signing_certificate_sha256: "c".repeat(64),
                result: fixture.archive.clone(),
                env_set_name: None,
            },
        )
        .unwrap();
        save_apple_archive_error(&fixture.paths, "previous archive error").unwrap();
        let guard =
            begin_machine_operation(&fixture.app, &fixture.paths.id, "uploading_archive").unwrap();

        let error = clear_apple_archive(&fixture.app, fixture.paths.id.clone())
            .await
            .unwrap_err();
        assert!(
            error.contains("Another operation is still running"),
            "{error}"
        );
        assert_eq!(
            load_apple_archive(&fixture.paths).unwrap().unwrap().result,
            fixture.archive
        );
        assert_eq!(fs::read(&fixture.archive.ipa.path).unwrap(), b"abc");
        assert_eq!(fs::read(&fixture.archive.archive.path).unwrap(), b"archive");
        assert!(fixture.paths.apple_archive_error().is_file());
        assert_eq!(
            busy_operation(&fixture.app, &fixture.paths.id)
                .unwrap()
                .as_deref(),
            Some("uploading_archive")
        );

        drop(guard);
        assert!(
            begin_machine_operation(&fixture.app, &fixture.paths.id, "clearing_archive").is_ok()
        );
    }
}
