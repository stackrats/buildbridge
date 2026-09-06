use super::*;
use std::sync::atomic::AtomicU64;

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "buildbridge-credential-export-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&directory).unwrap();
        Self(directory)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn saved_kit() -> StoredSigningKit {
    StoredSigningKit {
        id: "team".to_string(),
        app_store_connect_key_id: Some("ABCDE12345".to_string()),
        app_store_connect_issuer_id: Some("issuer-id".to_string()),
        app_store_connect_private_key: Some("private-key-contents".to_string()),
        signing_certificate_password: Some("distribution-password".to_string()),
        development_certificate_password: Some("development-password".to_string()),
        guest_keychain_password: Some("generated-guest-password".to_string()),
        android_keystore_password: Some(" keystore-password ".to_string()),
        android_key_password: Some(" separate-key-password ".to_string()),
        android_key_alias: Some("upload".to_string()),
        ..StoredSigningKit::default()
    }
}

#[test]
fn metadata_omits_secret_values_and_source_paths() {
    let directory = TestDirectory::new();
    let source = directory.path("distribution.p12");
    fs::write(&source, b"certificate-contents").unwrap();
    let mut kit = saved_kit();
    kit.signing_certificate_path = Some(source.to_string_lossy().to_string());
    let credentials = signing_credentials(&kit);
    let encoded = serde_json::to_string(&credentials).unwrap();
    for secret in [
        "private-key-contents",
        "distribution-password",
        "development-password",
        "generated-guest-password",
        "keystore-password",
        "separate-key-password",
        "certificate-contents",
    ] {
        assert!(!encoded.contains(secret));
    }
    assert!(!encoded.contains(directory.0.to_str().unwrap()));
    assert_eq!(
        credentials
            .iter()
            .filter(|item| item.kind == CredentialKind::Secret)
            .count(),
        6,
    );
    assert!(
        credentials
            .iter()
            .all(|item| item.kind == CredentialKind::Value || item.value.is_none())
    );
    let private_key = credentials
        .iter()
        .find(|item| item.id == "app_store_connect_private_key")
        .unwrap();
    assert_eq!(
        private_key.file_name.as_deref(),
        Some("AuthKey_ABCDE12345.p8")
    );
    let file = credentials
        .iter()
        .find(|item| item.id == "signing_certificate")
        .unwrap();
    assert_eq!(file.file_name.as_deref(), Some("distribution.p12"));
    assert!(file.available);
    assert_eq!(
        credential_text(&kit, CredentialId::GuestKeychainPassword).unwrap(),
        "generated-guest-password"
    );
}

#[test]
fn android_key_password_reveals_effective_password_without_trimming() {
    let mut kit = saved_kit();
    assert_eq!(
        credential_text(&kit, CredentialId::AndroidKeyPassword).unwrap(),
        " separate-key-password "
    );
    kit.android_key_password = None;
    assert_eq!(
        credential_text(&kit, CredentialId::AndroidKeyPassword).unwrap(),
        " keystore-password "
    );
    let credentials = signing_credentials(&kit);
    let inherited = credentials
        .iter()
        .find(|item| item.id == "android_key_password")
        .unwrap();
    assert!(inherited.label.contains("same as keystore password"));
    assert!(inherited.available);
    kit.android_keystore_password = None;
    assert!(credential_text(&kit, CredentialId::AndroidKeyPassword).is_err());
    assert!(
        !signing_credentials(&kit)
            .iter()
            .any(|item| item.id == "android_key_password")
    );
}

#[test]
fn credential_ids_are_allowlisted_and_profiles_belong_to_the_selected_kit() {
    for id in [
        "/etc/passwd",
        "../../private.p12",
        "signing_certificate_path",
        "provisioning_profile_-1",
        "provisioning_profile_00",
        "provisioning_profile_+1",
        "provisioning_profile_20",
        "provisioning_profile_18446744073709551616",
        "provisioning_profile_0/../../private.p12",
    ] {
        assert!(CredentialId::parse(id).is_err());
    }
    let directory = TestDirectory::new();
    let kit = saved_kit();
    assert!(
        export_kit_credential(
            &kit,
            CredentialId::parse("provisioning_profile_0").unwrap(),
            &directory.path("profile.mobileprovision"),
        )
        .is_err()
    );
    assert!(!directory.path("profile.mobileprovision").exists());
    assert!(credential_text(&kit, CredentialId::SigningCertificate).is_err());
    assert!(
        export_kit_credential(
            &kit,
            CredentialId::GuestKeychainPassword,
            &directory.path("password.txt")
        )
        .is_err()
    );
}

#[test]
fn export_preserves_exact_vault_key_and_each_stored_file() {
    let directory = TestDirectory::new();
    let mut kit = saved_kit();
    let pem = "-----BEGIN PRIVATE KEY-----\r\nkey-material\r\n-----END PRIVATE KEY-----\r\n";
    kit.app_store_connect_private_key = Some(pem.to_string());
    let key_destination = directory.path("AuthKey.p8");
    export_kit_credential(
        &kit,
        CredentialId::AppStoreConnectPrivateKey,
        &key_destination,
    )
    .unwrap();
    assert_eq!(fs::read(&key_destination).unwrap(), pem.as_bytes());

    let bytes = [0, 255, 13, 10, 42, 128];
    let source = directory.path("saved.p12");
    fs::write(&source, bytes).unwrap();
    let path = source.to_string_lossy().to_string();
    kit.signing_certificate_path = Some(path.clone());
    kit.development_certificate_path = Some(path.clone());
    kit.android_keystore_path = Some(path.clone());
    kit.provisioning_profile_paths = vec![path];
    for (index, id) in [
        CredentialId::SigningCertificate,
        CredentialId::DevelopmentCertificate,
        CredentialId::AndroidKeystore,
        CredentialId::ProvisioningProfile(0),
    ]
    .into_iter()
    .enumerate()
    {
        let destination = directory.path(&format!("export-{index}"));
        export_kit_credential(&kit, id, &destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

#[test]
fn missing_files_are_unavailable_and_export_errors_omit_sensitive_paths() {
    let directory = TestDirectory::new();
    let mut kit = saved_kit();
    kit.android_keystore_path = Some(
        directory
            .path("sensitive-missing-file.jks")
            .to_string_lossy()
            .to_string(),
    );
    let credentials = signing_credentials(&kit);
    let file = credentials
        .iter()
        .find(|item| item.id == "android_keystore")
        .unwrap();
    assert!(!file.available);
    let destination = directory.path("backup.jks");
    let error =
        export_kit_credential(&kit, CredentialId::AndroidKeystore, &destination).unwrap_err();
    assert!(!error.contains("sensitive-missing-file"));
    assert!(!destination.exists());
}

#[test]
fn oversized_credential_file_is_refused_before_destination_is_created() {
    let directory = TestDirectory::new();
    let source = directory.path("oversized.jks");
    fs::File::create(&source)
        .unwrap()
        .set_len(MAX_CREDENTIAL_EXPORT_BYTES + 1)
        .unwrap();
    let kit = StoredSigningKit {
        android_keystore_path: Some(source.to_string_lossy().to_string()),
        ..StoredSigningKit::default()
    };
    let destination = directory.path("backup.jks");
    let error =
        export_kit_credential(&kit, CredentialId::AndroidKeystore, &destination).unwrap_err();
    assert!(error.contains("64 MiB"));
    assert!(!destination.exists());
}

#[test]
fn exports_refuse_overwrites_and_remove_partial_writes() {
    let directory = TestDirectory::new();
    let destination = directory.path("existing.p8");
    fs::write(&destination, b"original").unwrap();
    assert!(export_credential_bytes(&destination, b"replacement").is_err());
    assert_eq!(fs::read(&destination).unwrap(), b"original");
    assert!(export_credential_bytes(Path::new("relative.p8"), b"secret").is_err());

    let partial = directory.path("partial.p8");
    let error = write_credential_export(&partial, |file| {
        file.write_all(b"partial secret")?;
        Err(std::io::Error::other("sensitive write failure"))
    })
    .unwrap_err();
    assert!(!partial.exists());
    assert!(!error.contains("sensitive write failure"));
}

#[cfg(unix)]
#[test]
fn exports_refuse_existing_and_dangling_symlink_destinations() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new();
    let target = directory.path("original.p8");
    fs::write(&target, b"original").unwrap();
    let existing = directory.path("link.p8");
    symlink(&target, &existing).unwrap();
    assert!(export_credential_bytes(&existing, b"replacement").is_err());
    assert_eq!(fs::read(&target).unwrap(), b"original");

    let missing = directory.path("missing.p8");
    let dangling = directory.path("dangling.p8");
    symlink(&missing, &dangling).unwrap();
    assert!(export_credential_bytes(&dangling, b"replacement").is_err());
    assert!(!missing.exists());
}

#[test]
fn private_key_download_names_fall_back_when_the_key_id_is_not_a_plain_token() {
    for key_id in [
        Some("../../AuthKey"),
        Some("A B"),
        Some("a/b"),
        Some("ABCDE-1234"),
        Some("AUTH\0KEY"),
        None,
    ] {
        let mut kit = saved_kit();
        kit.app_store_connect_key_id = key_id.map(str::to_string);
        let credentials = signing_credentials(&kit);
        let key = credentials
            .iter()
            .find(|item| item.id == "app_store_connect_private_key")
            .unwrap();
        assert_eq!(key.file_name.as_deref(), Some("AuthKey.p8"), "{key_id:?}");
    }
    let credentials = signing_credentials(&saved_kit());
    let key = credentials
        .iter()
        .find(|item| item.id == "app_store_connect_private_key")
        .unwrap();
    assert_eq!(key.file_name.as_deref(), Some("AuthKey_ABCDE12345.p8"));
}
