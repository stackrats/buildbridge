//! Signing kits: the vault records, their shape, and the commands that keep them.

use super::*;
pub async fn save_signing_kit(
    app: &Engine,
    input: SigningKitInput,
) -> Result<Vec<SigningKitSummary>, String> {
    let requested_id = input.kit_id.clone();
    let incoming = normalize_signing_kit(input)?;
    let mut kits = read_signing_kits().await?;

    match requested_id {
        Some(id) => {
            let existing = kits
                .kits
                .iter()
                .find(|kit| kit.id == id)
                .cloned()
                .ok_or_else(|| "These signing credentials are no longer stored.".to_string())?;
            let mut merged = merge_signing_kit(existing, incoming);
            invent_keychain_password(&mut merged)?;
            if let Some(stored) = kits.kits.iter_mut().find(|kit| kit.id == id) {
                *stored = merged;
            }
        }
        None => {
            if kits.kits.len() >= MAX_SIGNING_KITS {
                return Err(format!(
                    "BuildBridge stores at most {MAX_SIGNING_KITS} signing credentials."
                ));
            }
            let existing_ids = kits
                .kits
                .iter()
                .map(|kit| kit.id.as_str())
                .collect::<Vec<_>>();
            let mut created = StoredSigningKit {
                id: machines::machine_id_from_name(&incoming.name, &existing_ids),
                created_at_epoch_seconds: machines::now_epoch_seconds(),
                ..incoming
            };
            invent_keychain_password(&mut created)?;
            kits.kits.push(created);
        }
    }

    write_signing_kits(kits).await?;

    list_signing_kits(app).await
}
pub async fn list_signing_kits(app: &Engine) -> Result<Vec<SigningKitSummary>, String> {
    let kits = read_signing_kits().await?.kits;
    let registry = machines::load_registry(app)?;

    Ok(kits
        .iter()
        .map(|kit| {
            let mut summary = summarize_signing_kit(kit);
            summary.attached_machines = registry
                .machines
                .iter()
                .filter(|machine| machine.signing_kit_id.as_deref() == Some(kit.id.as_str()))
                .map(|machine| machine.config.name.clone())
                .collect();

            summary
        })
        .collect())
}
pub async fn delete_signing_kit(
    app: &Engine,
    kit_id: String,
    input: ConfirmInput,
) -> Result<Vec<SigningKitSummary>, String> {
    if !input.confirmed {
        return Err("Confirm removing the signing credentials before continuing.".to_string());
    }
    let mut kits = read_signing_kits().await?;
    let index = kits
        .kits
        .iter()
        .position(|kit| kit.id == kit_id)
        .ok_or_else(|| "These signing credentials are no longer stored.".to_string())?;
    let removed = kits.kits.remove(index);
    write_signing_kits(kits).await?;
    remove_managed_profiles_for(app, &removed)?;
    remove_managed_certificate_for(app, &removed)?;
    remove_managed_keystore_for(app, &removed)?;

    let mut registry = machines::load_registry(app)?;
    let mut detached = false;
    for machine in &mut registry.machines {
        if machine.signing_kit_id.as_deref() == Some(kit_id.as_str()) {
            machine.signing_kit_id = None;
            detached = true;
        }
    }
    if detached {
        machines::save_registry(app, &registry)?;
    }

    list_signing_kits(app).await
}
pub async fn attach_signing_kit(
    app: &Engine,
    machine_id: String,
    input: AttachSigningKitInput,
) -> Result<MachineView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    if let Some(id) = input.kit_id.as_deref() {
        let stored = read_signing_kits().await?;
        if !stored.kits.iter().any(|kit| kit.id == id) {
            return Err("These signing credentials are no longer stored.".to_string());
        }
    }
    let mut registry = machines::load_registry(app)?;
    let index = registry.position(&machine_id)?;
    registry.machines[index].signing_kit_id = input.kit_id;
    machines::save_registry(app, &registry)?;

    build_machine_view(app, &paths).await
}

pub(crate) fn normalize_signing_kit(input: SigningKitInput) -> Result<StoredSigningKit, String> {
    let name = input.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 60 {
        return Err("Give the signing credentials a name of 1 to 60 characters.".to_string());
    }
    let app_store_connect_private_key_path =
        optional_trim(input.app_store_connect_private_key_path);
    let inferred_key_id = app_store_connect_private_key_path
        .as_deref()
        .and_then(infer_app_store_connect_key_id);
    let supplied_key_id = optional_trim(input.app_store_connect_key_id);
    if let (Some(supplied), Some(inferred)) = (&supplied_key_id, &inferred_key_id)
        && !supplied.eq_ignore_ascii_case(inferred)
    {
        return Err(format!(
            "The App Store Connect key ID does not match the selected AuthKey_{inferred}.p8 file."
        ));
    }
    let app_store_connect_private_key = app_store_connect_private_key_path
        .as_deref()
        .map(read_app_store_connect_private_key)
        .transpose()?;
    let secrets = StoredSigningKit {
        id: input.kit_id.unwrap_or_default(),
        name,
        created_at_epoch_seconds: 0,
        app_store_connect_key_id: supplied_key_id.or(inferred_key_id),
        app_store_connect_issuer_id: optional_trim(input.app_store_connect_issuer_id),
        app_store_connect_private_key,
        signing_certificate_path: optional_trim(input.signing_certificate_path),
        signing_certificate_password: optional_trim(input.signing_certificate_password),
        provisioning_profile_paths: input
            .provisioning_profile_paths
            .into_iter()
            .filter_map(optional_trim)
            .collect(),
        guest_keychain_password: optional_trim(input.guest_keychain_password),
        development_certificate_path: optional_trim(input.development_certificate_path),
        development_certificate_password: optional_trim(input.development_certificate_password),
        development_certificate_serial_number: None,
        android_keystore_path: optional_trim(input.android_keystore_path),
        android_keystore_password: optional_android_password(input.android_keystore_password),
        android_key_alias: optional_trim(input.android_key_alias),
        android_key_password: optional_android_password(input.android_key_password),
    };
    let app_store_connect_values = [
        secrets.app_store_connect_key_id.is_some(),
        secrets.app_store_connect_issuer_id.is_some(),
        secrets.app_store_connect_private_key.is_some(),
    ];

    if app_store_connect_values.iter().any(|value| *value)
        && !app_store_connect_values.iter().all(|value| *value)
    {
        return Err(
            "App Store Connect key ID, issuer ID, and private .p8 path must be provided together."
                .to_string(),
        );
    }

    if let Some(private_key) = &secrets.app_store_connect_private_key
        && (private_key.len() > 16_384
            || !private_key.contains("-----BEGIN PRIVATE KEY-----")
            || !private_key.contains("-----END PRIVATE KEY-----"))
    {
        return Err("The App Store Connect private key is not a valid PEM key.".to_string());
    }

    if secrets.provisioning_profile_paths.len() > MAX_PROVISIONING_PROFILES {
        return Err(format!(
            "At most {MAX_PROVISIONING_PROFILES} provisioning profiles can be stored."
        ));
    }

    if let Some(path) = &secrets.signing_certificate_path {
        validate_secret_file(path, &["p12", "pfx"], "signing certificate")?;
    }
    if let Some(path) = &secrets.development_certificate_path {
        validate_secret_file(path, &["p12", "pfx"], "development certificate")?;
    }

    for path in &secrets.provisioning_profile_paths {
        validate_secret_file(path, &["mobileprovision"], "provisioning profile")?;
    }
    if let Some(path) = &secrets.android_keystore_path {
        validate_secret_file(path, &["jks", "keystore", "p12", "pfx"], "Android keystore")?;
    }
    if let Some(alias) = &secrets.android_key_alias
        && !valid_android_key_alias(alias)
    {
        return Err(
            "The Android key alias may only contain letters, digits, dots, underscores and dashes."
                .to_string(),
        );
    }
    for password in [
        secrets.android_keystore_password.as_deref(),
        secrets.android_key_password.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if password.chars().count() < 6 || password.chars().any(char::is_control) {
            return Err(
                "An Android keystore password is one line of at least six characters.".to_string(),
            );
        }
    }

    for value in [
        secrets.app_store_connect_key_id.as_deref(),
        secrets.app_store_connect_issuer_id.as_deref(),
        secrets.signing_certificate_password.as_deref(),
        secrets.guest_keychain_password.as_deref(),
        secrets.development_certificate_password.as_deref(),
        secrets.android_keystore_password.as_deref(),
        secrets.android_key_password.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if value.len() > 512 {
            return Err("A credential field exceeds the 512 character limit.".to_string());
        }
    }

    Ok(secrets)
}

pub(crate) fn merge_signing_kit(
    existing: StoredSigningKit,
    mut incoming: StoredSigningKit,
) -> StoredSigningKit {
    incoming.id = existing.id.clone();
    incoming.created_at_epoch_seconds = existing.created_at_epoch_seconds;
    if incoming.app_store_connect_key_id.is_none() {
        incoming.app_store_connect_key_id = existing.app_store_connect_key_id;
        incoming.app_store_connect_issuer_id = existing.app_store_connect_issuer_id;
        incoming.app_store_connect_private_key = existing.app_store_connect_private_key;
    }
    incoming.signing_certificate_path = incoming
        .signing_certificate_path
        .or(existing.signing_certificate_path);
    incoming.signing_certificate_password = incoming
        .signing_certificate_password
        .or(existing.signing_certificate_password);
    if incoming.provisioning_profile_paths.is_empty() {
        incoming.provisioning_profile_paths = existing.provisioning_profile_paths;
    }
    incoming.guest_keychain_password = incoming
        .guest_keychain_password
        .or(existing.guest_keychain_password);
    // A newly supplied development file invalidates the serial recorded for the old one.
    let new_development_file = incoming.development_certificate_path.is_some();
    incoming.development_certificate_path = incoming
        .development_certificate_path
        .or(existing.development_certificate_path);
    incoming.development_certificate_password = incoming
        .development_certificate_password
        .or(existing.development_certificate_password);
    if !new_development_file {
        incoming.development_certificate_serial_number =
            existing.development_certificate_serial_number;
    }
    incoming.android_keystore_path = incoming
        .android_keystore_path
        .or(existing.android_keystore_path);
    incoming.android_keystore_password = incoming
        .android_keystore_password
        .or(existing.android_keystore_password);
    incoming.android_key_alias = incoming.android_key_alias.or(existing.android_key_alias);
    incoming.android_key_password = incoming
        .android_key_password
        .or(existing.android_key_password);

    incoming
}

pub(crate) fn valid_android_key_alias(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

// An empty field keeps a saved password; surrounding spaces are part of a nonempty secret.
fn optional_android_password(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

/// Whether a kit can sign an Android release: a keystore, the alias of the key in it, and
/// the keystore password. The key password defaults to the keystore's.
pub(crate) fn kit_has_android_signing(kit: &StoredSigningKit) -> bool {
    kit.android_keystore_path.is_some()
        && kit.android_key_alias.is_some()
        && kit.android_keystore_password.is_some()
}

/// The upload key as a release takes it, or why it cannot.
pub(crate) fn android_signing_material(
    kit: &StoredSigningKit,
) -> Result<AndroidSigningMaterial, String> {
    let keystore_path = kit.android_keystore_path.clone().ok_or_else(|| {
        "The attached credentials hold no Android keystore. Add an upload key to them, or create one there."
            .to_string()
    })?;
    let key_alias = kit.android_key_alias.clone().ok_or_else(|| {
        "The attached credentials do not name the key alias in their keystore.".to_string()
    })?;
    let keystore_password = kit
        .android_keystore_password
        .clone()
        .ok_or_else(|| "The Android keystore password is missing from the OS vault.".to_string())?;
    let key_password = kit
        .android_key_password
        .clone()
        .unwrap_or_else(|| keystore_password.clone());
    if !std::path::Path::new(&keystore_path).is_file() {
        return Err(format!(
            "The keystore is no longer at {keystore_path}. Put it back or choose another."
        ));
    }

    Ok(AndroidSigningMaterial {
        keystore_path: PathBuf::from(keystore_path),
        keystore_password,
        key_alias,
        key_password,
    })
}

/// Public metadata from an explicit local keystore check; no Google account authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidSigningVerification {
    pub kit_id: String,
    pub key_alias: String,
    pub certificate_sha256: String,
    pub certificate_sha1: String,
    pub algorithm: AndroidSigningAlgorithm,
    pub key_bits: u32,
    #[ts(type = "number")]
    pub valid_from_epoch_seconds: i64,
    #[ts(type = "number")]
    pub valid_until_epoch_seconds: i64,
    #[ts(type = "number")]
    pub verified_at_epoch_seconds: u64,
}

/// Proves local key access and reports public certificate metadata. This does not check
/// Google Play account access or whether Google accepts the certificate for an app.
pub async fn verify_android_signing_kit(
    kit_id: String,
) -> Result<AndroidSigningVerification, String> {
    let kit = read_signing_kits()
        .await?
        .kits
        .into_iter()
        .find(|kit| kit.id == kit_id)
        .ok_or_else(|| "These signing credentials are no longer stored.".to_string())?;
    let signing = android_signing_material(&kit)?;
    let certificate = tokio::task::spawn_blocking(move || {
        buildbridge_machines::verify_android_signing_material(&signing)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "The Android signing check could not finish.".to_string())??;
    Ok(AndroidSigningVerification {
        kit_id,
        key_alias: certificate.key_alias,
        certificate_sha256: certificate.certificate_sha256,
        certificate_sha1: certificate.certificate_sha1,
        algorithm: certificate.algorithm,
        key_bits: certificate.key_bits,
        valid_from_epoch_seconds: certificate.valid_from_epoch_seconds,
        valid_until_epoch_seconds: certificate.valid_until_epoch_seconds,
        verified_at_epoch_seconds: machines::now_epoch_seconds(),
    })
}

/// Where kits keep the upload keys BuildBridge created for them, one directory per key.
pub(crate) fn managed_android_keystores_dir(app: &Engine) -> Result<PathBuf, String> {
    Ok(app.config_dir().join("android-builder").join("keystores"))
}

/// Creates an upload key for a kit in a throwaway container of the toolchain image: the
/// keystore lands owner-only under BuildBridge's managed directory, and its path, alias and
/// password go into the kit. The person chooses the password and must keep it; Google Play
/// cannot recover an upload key whose password is lost, and neither can BuildBridge.
pub async fn create_android_keystore(
    app: &Engine,
    kit_id: String,
    input: CreateAndroidKeystoreInput,
) -> Result<CreateAndroidKeystoreResult, String> {
    if !input.confirmed {
        return Err("Confirm the keystore creation before continuing.".to_string());
    }
    let kits = read_signing_kits().await?.kits;
    let mut kit = kits
        .iter()
        .find(|kit| kit.id == kit_id)
        .cloned()
        .ok_or_else(|| "These signing credentials are no longer stored.".to_string())?;
    if kit.android_keystore_path.is_some() {
        return Err(
            "These credentials already hold an Android keystore. Remove it before creating another."
                .to_string(),
        );
    }
    let key_alias = optional_trim(input.key_alias).unwrap_or_else(|| "upload".to_string());
    if !valid_android_key_alias(&key_alias) {
        return Err(
            "The key alias may only contain letters, digits, dots, underscores and dashes."
                .to_string(),
        );
    }
    let certificate_name =
        optional_trim(input.certificate_name).unwrap_or_else(|| kit.name.clone());
    let password = input.password;
    if password.chars().count() < 6
        || password.len() > 512
        || password.chars().any(char::is_control)
    {
        return Err("Choose a keystore password of six to 512 characters on one line.".to_string());
    }
    let stored_password = password.clone();
    let directory = managed_android_keystores_dir(app)?
        .join(format!("upload-{}", machines::now_epoch_seconds()));
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    set_restricted_directory_permissions(&directory)?;
    let output_path = directory.join("upload.keystore");
    let creation_path = output_path.clone();
    let creation_alias = key_alias.clone();
    let created = tokio::task::spawn_blocking(move || {
        buildbridge_machines::create_android_keystore(
            &creation_path,
            &password,
            &creation_alias,
            &certificate_name,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    let keystore = match created.and_then(|result| result) {
        Ok(keystore) => keystore,
        Err(error) => {
            let _ = fs::remove_dir_all(&directory);
            return Err(error);
        }
    };
    kit.android_keystore_path = Some(keystore.path.clone());
    kit.android_key_alias = Some(key_alias);
    kit.android_keystore_password = Some(stored_password);
    kit.android_key_password = None;
    save_signing_kit_record(kit.clone()).await?;

    Ok(CreateAndroidKeystoreResult {
        keystore,
        kit: summarize_signing_kit(&kit),
    })
}

/// Removes the upload key BuildBridge created for a kit, if the kit's keystore is one of
/// those; a keystore the person pointed the kit at is theirs and stays.
pub(crate) fn remove_managed_keystore_for(
    app: &Engine,
    kit: &StoredSigningKit,
) -> Result<(), String> {
    let managed = managed_android_keystores_dir(app)?;
    if let Some(path) = &kit.android_keystore_path {
        let candidate = std::path::Path::new(path);
        if candidate.starts_with(&managed)
            && let Some(directory) = candidate.parent()
            && directory != managed
        {
            let _ = fs::remove_dir_all(directory);
        }
    }

    Ok(())
}

/// The guest keychain password is BuildBridge's to invent: it locks a keychain BuildBridge
/// creates inside a machine, and nothing else ever asks for it. A kit saved without one gets
/// one, so the simplest kit is a name and a Team key; a password someone typed is kept as is.
pub(crate) fn invent_keychain_password(kit: &mut StoredSigningKit) -> Result<(), String> {
    if kit.guest_keychain_password.is_none() {
        kit.guest_keychain_password = Some(generated_password()?);
    }

    Ok(())
}

/// Thirty-two hexadecimal characters from the operating system's random source, with no
/// dependency to carry: the same entropy OpenSSL would hand back, read directly.
pub(crate) fn generated_password() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    let mut source = fs::File::open("/dev/urandom")
        .map_err(|error| format!("The system random source is unavailable: {error}"))?;
    std::io::Read::read_exact(&mut source, &mut bytes)
        .map_err(|error| format!("The system random source could not be read: {error}"))?;

    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(crate) fn optional_trim(value: String) -> Option<String> {
    let value = value.trim().to_string();

    (!value.is_empty()).then_some(value)
}

pub(crate) fn validate_secret_file(
    path: &str,
    extensions: &[&str],
    label: &str,
) -> Result<(), String> {
    let path = std::path::Path::new(path);
    let valid_extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        });

    if !path.is_absolute() || !path.is_file() || !valid_extension {
        return Err(format!(
            "The {label} must be an existing absolute path with a supported extension."
        ));
    }

    Ok(())
}

pub(crate) fn infer_app_store_connect_key_id(path: &str) -> Option<String> {
    let file_name = std::path::Path::new(path).file_name()?.to_str()?;
    let key_id = file_name.strip_prefix("AuthKey_")?.strip_suffix(".p8")?;

    (key_id.len() == 10
        && key_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric()))
    .then(|| key_id.to_ascii_uppercase())
}

pub(crate) fn read_app_store_connect_private_key(path: &str) -> Result<String, String> {
    validate_secret_file(path, &["p8"], "App Store Connect private key")?;
    let path = std::path::Path::new(path);
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Could not inspect the App Store Connect private key: {error}"))?;
    if metadata.len() == 0 || metadata.len() > 16_384 {
        return Err(
            "The App Store Connect private key must be between 1 byte and 16 KiB.".to_string(),
        );
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(
                "Restrict the App Store Connect private key to the current user (chmod 600) before storing it."
                    .to_string(),
            );
        }
    }

    let private_key = fs::read_to_string(path)
        .map_err(|error| format!("Could not read the App Store Connect private key: {error}"))?;
    let private_key = private_key.trim().to_string();
    if !private_key.contains("-----BEGIN PRIVATE KEY-----")
        || !private_key.contains("-----END PRIVATE KEY-----")
    {
        return Err("The App Store Connect private key is not a valid PEM key.".to_string());
    }

    Ok(private_key)
}

pub(crate) fn summarize_signing_kit(secrets: &StoredSigningKit) -> SigningKitSummary {
    SigningKitSummary {
        id: secrets.id.clone(),
        name: secrets.name.clone(),
        created_at_epoch_seconds: secrets.created_at_epoch_seconds,
        attached_machines: Vec::new(),
        signing_certificate_password_stored: secrets.signing_certificate_password.is_some(),
        app_store_connect_configured: secrets.app_store_connect_key_id.is_some()
            && secrets.app_store_connect_issuer_id.is_some()
            && secrets.app_store_connect_private_key.is_some(),
        app_store_connect_key_id: secrets.app_store_connect_key_id.clone(),
        signing_certificate_configured: secrets.signing_certificate_path.is_some(),
        signing_certificate_name: secrets.signing_certificate_path.as_ref().and_then(|path| {
            std::path::Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        }),
        provisioning_profile_names: secrets
            .provisioning_profile_paths
            .iter()
            .map(|path| {
                std::path::Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(path)
                    .to_string()
            })
            .collect(),
        guest_keychain_configured: secrets.guest_keychain_password.is_some(),
        development_certificate_configured: secrets.development_certificate_path.is_some(),
        development_certificate_name: secrets.development_certificate_path.as_ref().and_then(
            |path| {
                std::path::Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            },
        ),
        development_certificate_password_stored: secrets.development_certificate_password.is_some(),
        android_keystore_configured: secrets.android_keystore_path.is_some(),
        android_keystore_name: secrets.android_keystore_path.as_ref().and_then(|path| {
            std::path::Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        }),
        android_key_alias: secrets.android_key_alias.clone(),
        android_keystore_password_stored: secrets.android_keystore_password.is_some(),
        android_key_password_stored: secrets.android_key_password.is_some(),
    }
}

/// Reads every stored kit.
///
/// The first release kept one unnamed record in this entry. That shape is recognised by the
/// absence of a `kits` array and migrated in memory to a single kit named "Signing kit", so an
/// existing vault keeps working without a separate migration step.
/// Decodes a vault entry, migrating the pre-registry single record.
///
/// Kept separate from the keyring so migration, corruption and defaulting are all testable.
pub(crate) fn parse_signing_vault(encoded: &str) -> Result<StoredSigningKits, String> {
    let value: serde_json::Value = serde_json::from_str(encoded)
        .map_err(|error| format!("The signing vault entry is invalid: {error}"))?;

    if value.get("kits").is_some() {
        let mut kits: StoredSigningKits = serde_json::from_value(value)
            .map_err(|error| format!("The signing vault entry is invalid: {error}"))?;
        for kit in &mut kits.kits {
            if kit.id.is_empty() {
                kit.id = DEFAULT_SIGNING_KIT_ID.to_string();
            }
            if kit.name.is_empty() {
                kit.name = "Signing credentials".to_string();
            }
        }
        return Ok(kits);
    }

    let legacy: StoredSigningKit = serde_json::from_value(value)
        .map_err(|error| format!("The signing vault entry is invalid: {error}"))?;

    Ok(StoredSigningKits {
        kits: vec![StoredSigningKit {
            id: DEFAULT_SIGNING_KIT_ID.to_string(),
            name: "Signing credentials".to_string(),
            ..legacy
        }],
    })
}

/// The kit a machine uses: its explicit attachment, and nothing else. Signing material is
/// never picked on a machine's behalf — not even when the host holds exactly one kit — because
/// which identity signs a build is a decision, and the interface should show it being made.
pub(crate) fn resolve_signing_kit<'a>(
    kits: &'a [StoredSigningKit],
    attached: Option<&str>,
) -> Option<&'a StoredSigningKit> {
    let id = attached?;

    kits.iter().find(|kit| kit.id == id)
}

/// Whether a kit holds everything provisioning needs.
///
/// Three things make a kit usable, any one of them with the guest keychain password: a Team
/// key, from which BuildBridge creates certificates and profiles at Apple when a machine first
/// needs them; the distribution set — identity, passphrase and at least one profile — which is
/// what an archive needs; or a development identity with its passphrase, enough to run on a
/// phone and nothing more.
pub(crate) fn kit_is_complete(kit: &StoredSigningKit) -> bool {
    kit.guest_keychain_password.is_some()
        && (kit_has_team_key(kit)
            || kit_has_distribution_set(kit)
            || kit_has_development_identity(kit))
}

pub(crate) fn kit_has_team_key(kit: &StoredSigningKit) -> bool {
    kit.app_store_connect_key_id.is_some()
        && kit.app_store_connect_issuer_id.is_some()
        && kit.app_store_connect_private_key.is_some()
}

pub(crate) fn kit_has_distribution_set(kit: &StoredSigningKit) -> bool {
    kit.signing_certificate_path.is_some()
        && kit.signing_certificate_password.is_some()
        && !kit.provisioning_profile_paths.is_empty()
}

pub(crate) fn kit_has_development_identity(kit: &StoredSigningKit) -> bool {
    kit.development_certificate_path.is_some() && kit.development_certificate_password.is_some()
}

/// Classifies what a machine can do about signing right now.
///
/// `KitMissing` is the state left by an operating-system keyring being cleared: the guest still
/// holds a provisioned keychain, but the material that created it is gone, so the interface must
/// ask for the kit again instead of claiming signing is configured.
#[cfg(test)]
pub(crate) fn signing_health(
    vault_issue: Option<&str>,
    kit: Option<&StoredSigningKit>,
    provisioned: bool,
) -> SigningHealth {
    signing_health_for(MachinePlatform::Ios, vault_issue, kit, provisioned)
}

/// The same classification for either platform: an Android machine is ready when its kit
/// holds an upload key, and there is nothing provisioned into it that a lost kit could
/// orphan, since the key is streamed in per release.
pub(crate) fn signing_health_for(
    platform: MachinePlatform,
    vault_issue: Option<&str>,
    kit: Option<&StoredSigningKit>,
    provisioned: bool,
) -> SigningHealth {
    let complete = |kit: &StoredSigningKit| match platform {
        MachinePlatform::Ios => kit_is_complete(kit),
        MachinePlatform::Android => kit_has_android_signing(kit),
    };
    match (vault_issue, kit) {
        (Some(_), _) => SigningHealth::VaultUnavailable,
        (None, None) if provisioned => SigningHealth::KitMissing,
        (None, None) => SigningHealth::Unconfigured,
        (None, Some(kit)) if complete(kit) => SigningHealth::Ready,
        (None, Some(_)) => SigningHealth::Incomplete,
    }
}

pub(crate) async fn read_signing_kits() -> Result<StoredSigningKits, String> {
    tokio::task::spawn_blocking(move || {
        let entry = signing_kit_credential_entry()?;

        match entry.get_password() {
            Ok(encoded) => parse_signing_vault(&encoded),
            Err(keyring::Error::NoEntry) => Ok(StoredSigningKits::default()),
            Err(error) => Err(format!(
                "The operating-system credential vault could not be read: {error}"
            )),
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(crate) async fn write_signing_kits(kits: StoredSigningKits) -> Result<(), String> {
    let encoded = serde_json::to_string(&kits).map_err(|error| error.to_string())?;

    tokio::task::spawn_blocking(move || {
        signing_kit_credential_entry()?
            .set_password(&encoded)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The kit a machine will provision: its explicit attachment, or the only kit on the host.
pub(crate) async fn resolve_signing_kit_for(
    app: &Engine,
    machine_id: &str,
) -> Result<StoredSigningKit, String> {
    optional_signing_kit_for(app, machine_id)
        .await?
        .ok_or_else(|| {
            "Attach signing credentials to this machine first. They are stored once on this host and attached per machine."
                .to_string()
        })
}

/// The kit attached to a machine, if it is attached to one that is still stored.
pub(crate) async fn optional_signing_kit_for(
    app: &Engine,
    machine_id: &str,
) -> Result<Option<StoredSigningKit>, String> {
    let attached = attached_kit_id(app, machine_id)?;
    let kits = read_signing_kits().await?.kits;

    Ok(resolve_signing_kit(&kits, attached.as_deref()).cloned())
}

pub(crate) fn attached_kit_id(app: &Engine, machine_id: &str) -> Result<Option<String>, String> {
    Ok(machines::load_registry(app)?
        .find(machine_id)
        .ok()
        .and_then(|machine| machine.signing_kit_id.clone()))
}

pub(crate) async fn save_signing_kit_record(kit: StoredSigningKit) -> Result<(), String> {
    let mut kits = read_signing_kits().await?;
    match kits.kits.iter_mut().find(|stored| stored.id == kit.id) {
        Some(stored) => *stored = kit,
        None => kits.kits.push(kit),
    }

    write_signing_kits(kits).await
}
