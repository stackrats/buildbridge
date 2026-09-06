//! Provisioning profiles from Apple: verification, replacement, download, and the managed copies on this host.

use super::*;
pub async fn verify_apple_developer_team(
    app: &Engine,
    machine_id: String,
) -> Result<apple_api::AppleTeamVerificationResult, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let workspace = load_apple_workspace(&paths)?.ok_or_else(|| {
        "Approve an Apple project before verifying its developer team.".to_string()
    })?;
    let development_team = workspace.development_team.ok_or_else(|| {
        "BuildBridge could not detect DEVELOPMENT_TEAM in the approved project.".to_string()
    })?;
    let bundle_identifier = workspace.bundle_identifier.ok_or_else(|| {
        "BuildBridge could not detect PRODUCT_BUNDLE_IDENTIFIER in the approved project."
            .to_string()
    })?;
    let secrets = resolve_signing_kit_for(app, &machine_id).await?;
    let key_id = secrets.app_store_connect_key_id.ok_or_else(|| {
        "The stored signing credentials do not include an App Store Connect Key ID.".to_string()
    })?;
    let issuer_id = secrets.app_store_connect_issuer_id.ok_or_else(|| {
        "The stored signing credentials do not include an App Store Connect Issuer ID.".to_string()
    })?;
    let private_key = secrets.app_store_connect_private_key.ok_or_else(|| {
        "The stored signing credentials do not include an App Store Connect .p8 key.".to_string()
    })?;

    apple_api::verify_developer_team(
        &key_id,
        &issuer_id,
        &private_key,
        &development_team,
        &bundle_identifier,
    )
    .await
}
pub async fn create_apple_replacement_profile(
    app: &Engine,
    machine_id: String,
    input: CreateAppleProfileInput,
) -> Result<CreateAppleProfileResult, String> {
    if !input.confirmed {
        return Err(
            "Confirm the Apple provisioning-profile creation before continuing.".to_string(),
        );
    }

    let paths = MachinePaths::resolve(app, &machine_id)?;
    let workspace = load_apple_workspace(&paths)?.ok_or_else(|| {
        "Approve an Apple project before creating a provisioning profile.".to_string()
    })?;
    let bundle_identifier = workspace.bundle_identifier.ok_or_else(|| {
        "BuildBridge could not detect PRODUCT_BUNDLE_IDENTIFIER in the approved project."
            .to_string()
    })?;
    let mut secrets = resolve_signing_kit_for(app, &machine_id).await?;
    if secrets.provisioning_profile_paths.len() >= MAX_PROVISIONING_PROFILES {
        return Err(format!(
            "The signing credentials already retain {MAX_PROVISIONING_PROFILES} profiles. Remove obsolete local profile paths before creating another one."
        ));
    }
    let key_id = secrets.app_store_connect_key_id.as_deref().ok_or_else(|| {
        "The stored signing credentials do not include an App Store Connect Key ID.".to_string()
    })?;
    let issuer_id = secrets
        .app_store_connect_issuer_id
        .as_deref()
        .ok_or_else(|| {
            "The stored signing credentials do not include an App Store Connect Issuer ID."
                .to_string()
        })?;
    let private_key = secrets
        .app_store_connect_private_key
        .as_deref()
        .ok_or_else(|| {
            "The stored signing credentials do not include an App Store Connect .p8 key."
                .to_string()
        })?;

    let created = apple_api::create_replacement_profile(
        key_id,
        issuer_id,
        private_key,
        &bundle_identifier,
        input.certificate_id.trim(),
    )
    .await?;
    let saved_path = save_managed_apple_profile(app, &created.profile, &created.content).map_err(
        |error| {
            format!(
                "Apple created profile {}, but BuildBridge could not retain it locally: {error}. The Apple profile was not revoked; verify again before retrying.",
                created.profile.name
            )
        },
    )?;
    let saved_path_string = saved_path
        .to_str()
        .ok_or_else(|| {
            format!(
                "Apple created profile {}, but its managed local path is not valid UTF-8. The Apple profile was not revoked; verify again before retrying.",
                created.profile.name
            )
        })?
        .to_string();
    if !secrets
        .provisioning_profile_paths
        .iter()
        .any(|path| path == &saved_path_string)
    {
        secrets
            .provisioning_profile_paths
            .push(saved_path_string.clone());
    }
    save_signing_kit_record(secrets.clone())
        .await
        .map_err(|error| {
            format!(
                "Apple created profile {} and saved it at {}, but BuildBridge could not add it to the OS vault: {error}. The Apple profile was not revoked; verify again before retrying.",
                created.profile.name, saved_path_string
            )
        })?;
    let summary = summarize_signing_kit(&secrets);

    Ok(CreateAppleProfileResult {
        profile: created.profile,
        certificate: created.certificate,
        saved_path: saved_path_string,
        kit: summary,
    })
}

/// Downloads an existing Apple profile into this host's managed store and adds it to the kit.
///
/// Apple keeps the profile; BuildBridge only holds a copy. Losing that copy — a cleared vault, a
/// new host — should not mean hunting for the file, so an active profile can be taken back with
/// one action instead of being re-downloaded by hand.
pub async fn download_apple_profile(
    app: &Engine,
    machine_id: String,
    profile_id: String,
) -> Result<DownloadAppleProfileResult, String> {
    let mut secrets = resolve_signing_kit_for(app, &machine_id).await?;
    if secrets.provisioning_profile_paths.len() >= MAX_PROVISIONING_PROFILES {
        return Err(format!(
            "These credentials already hold {MAX_PROVISIONING_PROFILES} profiles. Remove obsolete paths before adding another."
        ));
    }
    let key_id = secrets.app_store_connect_key_id.as_deref().ok_or_else(|| {
        "These credentials have no App Store Connect Key ID, so Apple cannot be asked for the profile."
            .to_string()
    })?;
    let issuer_id = secrets
        .app_store_connect_issuer_id
        .as_deref()
        .ok_or_else(|| "These credentials have no App Store Connect Issuer ID.".to_string())?;
    let private_key = secrets
        .app_store_connect_private_key
        .as_deref()
        .ok_or_else(|| "These credentials have no App Store Connect .p8 key.".to_string())?;

    let (profile, content) =
        apple_api::download_profile(key_id, issuer_id, private_key, profile_id.trim()).await?;
    let saved_path = save_managed_apple_profile(app, &profile, &content)?;
    let saved_path_string = saved_path
        .to_str()
        .ok_or_else(|| "The managed profile path is not valid UTF-8.".to_string())?
        .to_string();
    if !secrets
        .provisioning_profile_paths
        .iter()
        .any(|path| path == &saved_path_string)
    {
        secrets
            .provisioning_profile_paths
            .push(saved_path_string.clone());
    }
    save_signing_kit_record(secrets.clone()).await?;

    Ok(DownloadAppleProfileResult {
        profile,
        saved_path: saved_path_string,
        kit: summarize_signing_kit(&secrets),
    })
}

/// Managed copies are always written with this extension, so anything else in the directory is
/// not ours to offer.
pub(crate) fn is_managed_profile_file(file_name: &str) -> bool {
    !file_name.starts_with('.')
        && std::path::Path::new(file_name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mobileprovision"))
}

/// Lists the profiles already on this host. A vault that loses its paths does not lose these
/// files, so they can be attached to a kit again in one step.
pub fn list_managed_apple_profiles(app: &Engine) -> Result<Vec<ManagedAppleProfile>, String> {
    let directory = managed_apple_profiles_dir(app)?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("Could not read {}: {error}", directory.display())),
    };

    let mut profiles: Vec<ManagedAppleProfile> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let file_name = entry.file_name().to_string_lossy().to_string();
            if !is_managed_profile_file(&file_name) {
                return None;
            }
            let saved_at_epoch_seconds = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |elapsed| elapsed.as_secs());

            Some(ManagedAppleProfile {
                file_name,
                path: entry.path().to_string_lossy().to_string(),
                saved_at_epoch_seconds,
            })
        })
        .collect();

    profiles.sort_by(|left, right| {
        right
            .saved_at_epoch_seconds
            .cmp(&left.saved_at_epoch_seconds)
            .then_with(|| left.file_name.cmp(&right.file_name))
    });
    profiles.truncate(MAX_PROVISIONING_PROFILES);

    Ok(profiles)
}

pub(crate) fn save_managed_apple_profile(
    app: &Engine,
    profile: &apple_api::AppleProvisioningProfileSummary,
    content: &[u8],
) -> Result<PathBuf, String> {
    let file_id = if safe_apple_resource_component(&profile.uuid) {
        &profile.uuid
    } else if safe_apple_resource_component(&profile.id) {
        &profile.id
    } else {
        return Err("Apple returned an unsafe profile identifier.".to_string());
    };
    if content.is_empty() || content.len() > 2 * 1024 * 1024 {
        return Err("Apple returned profile content with an unsafe size.".to_string());
    }

    let path = managed_apple_profiles_dir(app)?.join(format!("{file_id}.mobileprovision"));
    write_restricted_file(&path, content)?;

    Ok(path)
}

/// Removes an identity BuildBridge created for this kit. Its password lived only in the kit
/// that was just deleted, so the file could not be used again anyway.
pub(crate) fn remove_managed_certificate_for(
    app: &Engine,
    kit: &StoredSigningKit,
) -> Result<(), String> {
    let managed = managed_apple_certificates_dir(app)?;
    for path in [
        kit.signing_certificate_path.as_ref(),
        kit.development_certificate_path.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
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

/// Removes only the Apple-created profile files this kit owns, leaving other kits alone.
pub(crate) fn remove_managed_profiles_for(
    app: &Engine,
    kit: &StoredSigningKit,
) -> Result<(), String> {
    let managed = managed_apple_profiles_dir(app)?;
    for path in &kit.provisioning_profile_paths {
        let candidate = std::path::Path::new(path);
        if candidate.starts_with(&managed) {
            remove_file_if_present(candidate)?;
        }
    }

    Ok(())
}
