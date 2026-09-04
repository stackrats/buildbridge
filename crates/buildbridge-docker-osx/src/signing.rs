//! Signing provisioning: identities and profiles into the guest keychain, and their verification.

use super::*;

/// Streams the fixed native signing helper's source into the guest and compiles it there with
/// Xcode's own clang, owner-only. Every signing operation runs the helper from the same
/// recipe; the recipe lives here once.
pub(crate) fn install_signing_helper(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    helper_source: &str,
    helper_binary: &str,
) -> Result<(), ProviderError> {
    stream_bytes_to_guest(
        SIGNING_HELPER_SOURCE,
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        helper_source,
        "signing helper",
    )?;
    let compile = format!(
        "set -eu; /usr/bin/xcrun --sdk macosx clang -std=c11 -O2 -Wno-deprecated-declarations {} -framework Security -framework CoreFoundation -o {}; /bin/chmod 700 {}",
        shell_single_quote(helper_source),
        shell_single_quote(helper_binary),
        shell_single_quote(helper_binary),
    );
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &compile,
    )
    .map(|_| ())
}

/// What one identity import needs beyond the identity itself: the guest helper, the keychain,
/// the pinned Apple intermediate, and the bridge to reach them.
pub(crate) struct IdentityImportContext<'a> {
    pub(crate) ssh_port: u16,
    pub(crate) username: &'a str,
    pub(crate) identity_path: &'a Path,
    pub(crate) known_hosts_path: &'a Path,
    pub(crate) helper_binary: &'a str,
    pub(crate) keychain_path: &'a str,
    pub(crate) staging: &'a str,
    pub(crate) xcodebuild: &'a str,
    pub(crate) keychain_password: &'a str,
    pub(crate) development_team: &'a str,
    pub(crate) wwdr_g3_pem: &'a str,
    pub(crate) wwdr_g3_der: &'a str,
    pub(crate) total_bytes: u64,
    pub(crate) started_at: Instant,
}

/// Streams one `.p12` into the staging directory, imports it through the helper (creating the
/// keychain for the first identity, adding to it for the second), installs the pinned Apple
/// intermediate alongside the first, checks the team, and proves the private key can sign.
#[allow(clippy::too_many_arguments)]
pub(crate) fn import_signing_identity<F>(
    context: &IdentityImportContext<'_>,
    host_path: &Path,
    password: &str,
    stem: &str,
    label: &str,
    mode: HelperImportMode,
    completed_bytes: &mut u64,
    on_progress: &mut F,
) -> Result<ProvisionedIdentity, ProviderError>
where
    F: FnMut(SigningProvisioningProgress),
{
    let guest_p12 = format!("{}/{stem}.p12", context.staging);
    let guest_der = format!("{}/{stem}.der", context.staging);
    stream_signing_file(
        host_path,
        &guest_p12,
        context.total_bytes,
        completed_bytes,
        context.started_at,
        context.ssh_port,
        context.username,
        context.identity_path,
        context.known_hosts_path,
        on_progress,
    )?;
    on_progress(signing_progress(
        SigningProvisioningPhase::ImportingCertificate,
        context.total_bytes,
        context.total_bytes,
        context.started_at,
        match mode {
            HelperImportMode::Create => {
                "Creating the dedicated keychain and importing its non-extractable identity."
            }
            HelperImportMode::Add => "Importing the second identity into the same keychain.",
        },
    ));
    run_signing_helper(
        context.ssh_port,
        context.username,
        context.identity_path,
        context.known_hosts_path,
        context.helper_binary,
        context.keychain_path,
        &guest_p12,
        &guest_der,
        context.xcodebuild,
        context.keychain_password,
        password,
        mode,
    )?;

    on_progress(signing_progress(
        SigningProvisioningPhase::Verifying,
        context.total_bytes,
        context.total_bytes,
        context.started_at,
        "Verifying the imported certificate and proving that its private key can sign code.",
    ));
    let output = run_guest_command(
        context.ssh_port,
        context.username,
        context.identity_path,
        context.known_hosts_path,
        &certificate_metadata_command(&guest_der),
    )?;
    let (certificate_expires_at, identity_sha1, certificate_sha256, team, identity_name) =
        parse_certificate_metadata(&output)?;
    if team != context.development_team {
        return Err(ProviderError::GuestBridge(format!(
            "the {label} belongs to team {team}, but the project uses team {}",
            context.development_team
        )));
    }
    if mode == HelperImportMode::Create {
        install_apple_wwdr_g3_intermediate(
            context.wwdr_g3_pem,
            context.wwdr_g3_der,
            context.keychain_path,
            context.ssh_port,
            context.username,
            context.identity_path,
            context.known_hosts_path,
        )?;
    }
    verify_code_signing_identity(
        &identity_sha1,
        context.helper_binary,
        context.keychain_path,
        context.staging,
        context.keychain_password,
        context.ssh_port,
        context.username,
        context.identity_path,
        context.known_hosts_path,
    )?;

    Ok(ProvisionedIdentity {
        identity_name,
        identity_sha1,
        certificate_sha256,
        certificate_expires_at,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn provision_signing<F>(
    material: &SigningMaterial<'_>,
    development_team: &str,
    bundle_identifier: &str,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    mut on_progress: F,
) -> Result<SigningProvisioningResult, ProviderError>
where
    F: FnMut(SigningProvisioningProgress),
{
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    validate_signing_target(development_team, bundle_identifier)?;
    let keychain_password = material.keychain_password;
    let distribution = material
        .distribution_certificate
        .map(|(path, password)| {
            validate_identity_material(path, password, "signing certificate")
                .map(|(file, bytes)| (file, password, bytes))
        })
        .transpose()?;
    let development = material
        .development_certificate
        .map(|(path, password)| {
            validate_identity_material(path, password, "development certificate")
                .map(|(file, bytes)| (file, password, bytes))
        })
        .transpose()?;
    if distribution.is_none() && development.is_none() {
        return Err(ProviderError::GuestBridge(
            "the signing kit holds no identity: store a distribution identity or a development identity first"
                .to_string(),
        ));
    }
    let (profile_paths, profile_bytes) = validate_signing_material(
        material.profile_paths,
        keychain_password,
        distribution.is_some(),
    )?;
    let total_bytes = profile_bytes
        + distribution
            .as_ref()
            .map(|(_, _, bytes)| *bytes)
            .unwrap_or(0)
        + development
            .as_ref()
            .map(|(_, _, bytes)| *bytes)
            .unwrap_or(0);
    if total_bytes > SIGNING_MATERIAL_MAX_BYTES {
        return Err(ProviderError::GuestBridge(
            "the signing kit is larger than the material a provisioning run accepts".to_string(),
        ));
    }
    let started_at = Instant::now();
    let guest_home = format!("/Users/{username}");
    let guest_tools = format!("{guest_home}/.buildbridge/tools");
    let helper_source = format!("{guest_tools}/signing-helper.c");
    let helper_binary = format!("{guest_tools}/signing-helper");
    let xcodebuild =
        format!("{guest_home}/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild");
    let operation_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let staging =
        format!("{guest_home}/Library/Caches/dev.buildbridge.desktop/signing-{operation_id}");
    let guest_wwdr_g3_pem = format!("{staging}/AppleWWDRCAG3.pem");
    let guest_wwdr_g3_der = format!("{staging}/AppleWWDRCAG3.cer");
    let keychain_path = format!("{guest_home}/Library/Keychains/{SIGNING_KEYCHAIN_NAME}");

    on_progress(signing_progress(
        SigningProvisioningPhase::Preparing,
        0,
        total_bytes,
        started_at,
        "Validating the signing kit and preparing the password-safe guest helper.",
    ));
    let prepare = format!(
        "set -eu; /bin/mkdir -p {} {}; /bin/chmod 700 {}; /usr/bin/security delete-keychain {} >/dev/null 2>&1 || /bin/rm -f {}; /bin/rm -rf {}; /bin/mkdir -p {}; /bin/chmod 700 {}",
        shell_single_quote(&guest_tools),
        shell_single_quote(&format!("{guest_home}/Library/Keychains")),
        shell_single_quote(&guest_tools),
        shell_single_quote(&keychain_path),
        shell_single_quote(&keychain_path),
        shell_single_quote(&staging),
        shell_single_quote(&staging),
        shell_single_quote(&staging),
    );
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &prepare,
    )?;

    let operation = (|| {
        install_signing_helper(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &helper_source,
            &helper_binary,
        )?;
        stream_bytes_to_guest(
            APPLE_WWDR_G3_PEM,
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_wwdr_g3_pem,
            "pinned Apple WWDR G3 intermediate",
        )?;

        let mut completed_bytes = 0_u64;
        let guest_profiles = profile_paths
            .iter()
            .enumerate()
            .map(|(index, profile_path)| {
                let guest_path = format!("{staging}/profile-{index:03}.mobileprovision");
                stream_signing_file(
                    profile_path,
                    &guest_path,
                    total_bytes,
                    &mut completed_bytes,
                    started_at,
                    ssh_port,
                    username,
                    identity_path,
                    known_hosts_path,
                    &mut on_progress,
                )?;
                Ok((guest_path, profile_path.clone()))
            })
            .collect::<Result<Vec<_>, ProviderError>>()?;

        // The first identity creates the keychain and the second joins it, so one unlock serves
        // both the archive and a device build. Each gets the team check and the signing probe.
        let context = IdentityImportContext {
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            helper_binary: &helper_binary,
            keychain_path: &keychain_path,
            staging: &staging,
            xcodebuild: &xcodebuild,
            keychain_password,
            development_team,
            wwdr_g3_pem: &guest_wwdr_g3_pem,
            wwdr_g3_der: &guest_wwdr_g3_der,
            total_bytes,
            started_at,
        };
        let mut mode = HelperImportMode::Create;
        let distribution_identity = match &distribution {
            Some((path, password, _)) => {
                let identity = import_signing_identity(
                    &context,
                    path,
                    password,
                    "identity",
                    "signing certificate",
                    mode,
                    &mut completed_bytes,
                    &mut on_progress,
                )?;
                mode = HelperImportMode::Add;
                Some(identity)
            }
            None => None,
        };
        let development_identity = match &development {
            Some((path, password, _)) => Some(import_signing_identity(
                &context,
                path,
                password,
                "development",
                "development certificate",
                mode,
                &mut completed_bytes,
                &mut on_progress,
            )?),
            None => None,
        };

        on_progress(signing_progress(
            SigningProvisioningPhase::InspectingProfiles,
            total_bytes,
            total_bytes,
            started_at,
            "Inspecting profile expiry, team, bundle, and included signing certificates.",
        ));
        let profile_output = inspect_guest_profiles(
            &guest_profiles,
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
        )?;
        let profiles = parse_profile_summaries(&profile_output)?;
        if profiles.len() != guest_profiles.len() {
            return Err(ProviderError::GuestBridge(
                "macOS returned incomplete provisioning profile metadata".to_string(),
            ));
        }
        for profile in &profiles {
            if profile.team_identifier != development_team {
                return Err(ProviderError::GuestBridge(format!(
                    "provisioning profile {} belongs to team {}, but the project uses team {}",
                    profile.uuid, profile.team_identifier, development_team
                )));
            }
            let accepted = std::iter::once(bundle_identifier)
                .chain(material.extra_bundle_identifiers.iter().map(String::as_str));
            if !accepted
                .clone()
                .any(|bundle| profile_allows_bundle(&profile.application_identifier, bundle))
            {
                return Err(ProviderError::GuestBridge(format!(
                    "provisioning profile {} allows {}, not the project bundle identifier {}",
                    profile.uuid,
                    profile.application_identifier,
                    accepted.collect::<Vec<_>>().join(" or ")
                )));
            }
            // Development profiles carry the development certificate; App Store, ad hoc and
            // enterprise profiles all carry the distribution one.
            let (required_fingerprint, identity_label) =
                if profile.kind == Some(ProfileKind::Development) {
                    (
                        development_identity
                            .as_ref()
                            .map(|identity| identity.certificate_sha256.as_str()),
                        "development",
                    )
                } else {
                    (
                        distribution_identity
                            .as_ref()
                            .map(|identity| identity.certificate_sha256.as_str()),
                        "distribution",
                    )
                };
            let Some(required_fingerprint) = required_fingerprint else {
                return Err(ProviderError::GuestBridge(format!(
                    "provisioning profile {} is a {identity_label} profile, but the kit has no {identity_label} identity",
                    profile.uuid
                )));
            };
            if !profile
                .developer_certificate_sha256
                .iter()
                .any(|fingerprint| fingerprint == required_fingerprint)
            {
                return Err(ProviderError::GuestBridge(format!(
                    "provisioning profile {} does not include the kit's {identity_label} certificate",
                    profile.uuid
                )));
            }
        }

        on_progress(signing_progress(
            SigningProvisioningPhase::InstallingProfiles,
            total_bytes,
            total_bytes,
            started_at,
            "Installing the verified profiles under their Apple UUIDs.",
        ));
        install_guest_profiles(
            &guest_profiles,
            &profiles,
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
        )?;
        run_guest_command(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &format!(
                "/usr/bin/security lock-keychain {}",
                shell_single_quote(&keychain_path)
            ),
        )?;

        Ok(SigningProvisioningResult {
            keychain_path: keychain_path.clone(),
            distribution_identity,
            development_team: development_team.to_string(),
            bundle_identifier: bundle_identifier.to_string(),
            profiles,
            development_identity,
        })
    })();

    let cleanup = if operation.is_ok() {
        format!("/bin/rm -rf {}", shell_single_quote(&staging))
    } else {
        format!(
            "/usr/bin/security delete-keychain {} >/dev/null 2>&1 || /bin/rm -f {}; /bin/rm -rf {}",
            shell_single_quote(&keychain_path),
            shell_single_quote(&keychain_path),
            shell_single_quote(&staging),
        )
    };
    let _ = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &cleanup,
    );
    let result = operation?;
    on_progress(signing_progress(
        SigningProvisioningPhase::Completed,
        total_bytes,
        total_bytes,
        started_at,
        "Signing identities and provisioning profiles are ready in macOS.",
    ));

    Ok(result)
}

pub fn clear_signing(
    profile_uuids: &[String],
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    if profile_uuids.len() > SIGNING_PROFILE_MAX_COUNT {
        return Err(ProviderError::GuestBridge(
            "too many stored signing profile identifiers".to_string(),
        ));
    }
    for uuid in profile_uuids {
        if !valid_profile_uuid(uuid) {
            return Err(ProviderError::GuestBridge(
                "stored provisioning profile metadata is invalid".to_string(),
            ));
        }
    }

    let guest_home = format!("/Users/{username}");
    let keychain_path = format!("{guest_home}/Library/Keychains/{SIGNING_KEYCHAIN_NAME}");
    let mut command = format!(
        "/usr/bin/security delete-keychain {} >/dev/null 2>&1 || /bin/rm -f {}",
        shell_single_quote(&keychain_path),
        shell_single_quote(&keychain_path),
    );
    for uuid in profile_uuids {
        for directory in [
            format!("{guest_home}/Library/MobileDevice/Provisioning Profiles"),
            format!("{guest_home}/Library/Developer/Xcode/UserData/Provisioning Profiles"),
        ] {
            command.push_str(&format!(
                "; /bin/rm -f {}",
                shell_single_quote(&format!("{directory}/{uuid}.mobileprovision"))
            ));
        }
    }

    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &command,
    )?;

    Ok(())
}

pub(crate) fn validate_signing_target(
    development_team: &str,
    bundle_identifier: &str,
) -> Result<(), ProviderError> {
    if development_team.is_empty()
        || development_team.len() > 64
        || !development_team
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(ProviderError::GuestBridge(
            "the project development team is missing or invalid".to_string(),
        ));
    }
    if bundle_identifier.is_empty()
        || bundle_identifier.len() > 255
        || bundle_identifier.starts_with('.')
        || bundle_identifier.ends_with('.')
        || bundle_identifier.contains("..")
        || !bundle_identifier
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
    {
        return Err(ProviderError::GuestBridge(
            "the project bundle identifier is missing or invalid".to_string(),
        ));
    }

    Ok(())
}

/// One `.p12` and its passphrase, checked before anything is streamed to the guest.
pub(crate) fn validate_identity_material(
    path: &Path,
    password: &str,
    label: &str,
) -> Result<(PathBuf, u64), ProviderError> {
    if password.is_empty() || password.len() > 512 {
        return Err(ProviderError::GuestBridge(format!(
            "store the {label} passphrase in the operating-system vault first"
        )));
    }
    let file = validate_signing_file(path, &["p12", "pfx"], SIGNING_CERTIFICATE_MAX_BYTES, label)?;
    let bytes = fs::metadata(&file)
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not inspect the {label}: {error}"))
        })?
        .len();
    Ok((file, bytes))
}

/// The profiles and the keychain password. A kit with a distribution identity must carry at
/// least one profile, because that is what the archive signs with; a development-only kit may
/// carry none yet, since the device step adds its profile later.
pub(crate) fn validate_signing_material(
    profile_paths: &[PathBuf],
    keychain_password: &str,
    profiles_required: bool,
) -> Result<(Vec<PathBuf>, u64), ProviderError> {
    if keychain_password.is_empty() || keychain_password.len() > 512 {
        return Err(ProviderError::GuestBridge(
            "store a dedicated guest keychain password in the operating-system vault first"
                .to_string(),
        ));
    }
    if (profiles_required && profile_paths.is_empty())
        || profile_paths.len() > SIGNING_PROFILE_MAX_COUNT
    {
        return Err(ProviderError::GuestBridge(format!(
            "select between {} and {SIGNING_PROFILE_MAX_COUNT} provisioning profiles",
            u8::from(profiles_required)
        )));
    }

    let mut total_bytes = 0_u64;
    let mut profiles = Vec::with_capacity(profile_paths.len());
    for profile_path in profile_paths {
        let profile = validate_signing_file(
            profile_path,
            &["mobileprovision"],
            PROVISIONING_PROFILE_MAX_BYTES,
            "provisioning profile",
        )?;
        if profiles.contains(&profile) {
            return Err(ProviderError::GuestBridge(
                "the same provisioning profile was selected more than once".to_string(),
            ));
        }
        total_bytes = total_bytes
            .checked_add(
                fs::metadata(&profile)
                    .map_err(|error| {
                        ProviderError::GuestBridge(format!(
                            "could not inspect a provisioning profile: {error}"
                        ))
                    })?
                    .len(),
            )
            .ok_or_else(|| {
                ProviderError::GuestBridge("the signing kit size is invalid".to_string())
            })?;
        profiles.push(profile);
    }

    Ok((profiles, total_bytes))
}

pub(crate) fn validate_signing_file(
    path: &Path,
    extensions: &[&str],
    maximum_bytes: u64,
    label: &str,
) -> Result<PathBuf, ProviderError> {
    if !path.is_absolute()
        || !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extensions
                    .iter()
                    .any(|expected| extension.eq_ignore_ascii_case(expected))
            })
    {
        return Err(ProviderError::GuestBridge(format!(
            "the {label} must be an absolute path with a supported extension"
        )));
    }
    let canonical = fs::canonicalize(path).map_err(|error| {
        ProviderError::GuestBridge(format!("the {label} is unavailable: {error}"))
    })?;
    let metadata = fs::metadata(&canonical).map_err(|error| {
        ProviderError::GuestBridge(format!("could not inspect the {label}: {error}"))
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum_bytes {
        return Err(ProviderError::GuestBridge(format!(
            "the {label} is empty, not a regular file, or exceeds its size limit"
        )));
    }

    Ok(canonical)
}

pub(crate) fn stream_bytes_to_guest(
    contents: &[u8],
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    guest_path: &str,
    label: &str,
) -> Result<(), ProviderError> {
    let remote_command = format!("umask 077; /bin/cat > {}", shell_single_quote(guest_path));
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start {label} transfer: {error}"))
        })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ProviderError::GuestBridge(format!("could not open {label} transfer")))?;
    stdin.write_all(contents).map_err(|error| {
        ProviderError::GuestBridge(format!("{label} transfer was interrupted: {error}"))
    })?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish {label} transfer: {error}"))
    })?;
    if !output.status.success() {
        return Err(ProviderError::GuestBridge(format!(
            "the guest rejected the {label}: {}",
            clean_output(&output.stderr)
        )));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn stream_signing_file<F>(
    path: &Path,
    guest_path: &str,
    total_bytes: u64,
    completed_bytes: &mut u64,
    started_at: Instant,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    on_progress: &mut F,
) -> Result<(), ProviderError>
where
    F: FnMut(SigningProvisioningProgress),
{
    let mut file = File::open(path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not open signing material: {error}"))
    })?;
    let remote_command = format!("umask 077; /bin/cat > {}", shell_single_quote(guest_path));
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not start signing material transfer: {error}"
            ))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not open signing material transfer".to_string())
    })?;
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    loop {
        let count = file.read(&mut buffer).map_err(|error| {
            ProviderError::GuestBridge(format!("could not read signing material: {error}"))
        })?;
        if count == 0 {
            break;
        }
        stdin.write_all(&buffer[..count]).map_err(|error| {
            ProviderError::GuestBridge(format!(
                "signing material transfer was interrupted: {error}"
            ))
        })?;
        *completed_bytes += count as u64;
        if last_progress.elapsed() >= Duration::from_millis(200) || *completed_bytes == total_bytes
        {
            on_progress(signing_progress(
                SigningProvisioningPhase::Transferring,
                *completed_bytes,
                total_bytes,
                started_at,
                "Transferring certificate and profiles over the pinned SSH bridge.",
            ));
            last_progress = Instant::now();
        }
    }
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!(
            "could not finish signing material transfer: {error}"
        ))
    })?;
    if !output.status.success() {
        return Err(ProviderError::GuestBridge(format!(
            "the guest rejected signing material: {}",
            clean_output(&output.stderr)
        )));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_signing_helper(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    helper_path: &str,
    keychain_path: &str,
    certificate_path: &str,
    certificate_der_path: &str,
    xcodebuild_path: &str,
    keychain_password: &str,
    certificate_password: &str,
    mode: HelperImportMode,
) -> Result<(), ProviderError> {
    let remote_command = format!(
        "{}{} {} {} {} {}",
        shell_single_quote(helper_path),
        match mode {
            HelperImportMode::Create => "",
            HelperImportMode::Add => " --add",
        },
        shell_single_quote(keychain_path),
        shell_single_quote(certificate_path),
        shell_single_quote(certificate_der_path),
        shell_single_quote(xcodebuild_path),
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start signing import: {error}"))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not open the protected signing input".to_string())
    })?;
    write_secret_frame(&mut stdin, keychain_password)?;
    write_secret_frame(&mut stdin, certificate_password)?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish signing import: {error}"))
    })?;
    if !output.status.success() {
        let detail = clean_output(&output.stderr);
        let message = if detail == "identity_count:0" {
            "the PKCS#12 archive contains no certificate/private-key identity; export the distribution certificate from Keychain Access → My Certificates with its private key expanded underneath it"
                .to_string()
        } else if let Some(count) = detail.strip_prefix("identity_count:") {
            format!(
                "the PKCS#12 archive contains {count} identities; export exactly one distribution identity from Keychain Access"
            )
        } else if detail == "partition_acl_missing" {
            "macOS imported the identity but did not expose the private-key authorization record required for unattended Apple signing"
                .to_string()
        } else if detail.starts_with("authorize_private_key:") {
            format!(
                "macOS imported the identity but could not authorize its private key for Apple signing ({detail})"
            )
        } else if detail.starts_with("import_certificate:-25293") {
            "the certificate passphrase is incorrect or the PKCS#12 archive is damaged".to_string()
        } else if detail.is_empty() {
            "macOS could not import the signing certificate".to_string()
        } else {
            format!("macOS could not provision the signing keychain ({detail})")
        };
        return Err(ProviderError::GuestBridge(message));
    }
    if clean_output(&output.stdout) != "ok" {
        return Err(ProviderError::GuestBridge(
            "the signing helper returned an unexpected result".to_string(),
        ));
    }

    Ok(())
}

pub(crate) fn write_secret_frame(
    writer: &mut impl Write,
    secret: &str,
) -> Result<(), ProviderError> {
    let length = u32::try_from(secret.len()).map_err(|_| {
        ProviderError::GuestBridge("a signing credential exceeds its size limit".to_string())
    })?;
    writer.write_all(&length.to_be_bytes()).map_err(|error| {
        ProviderError::GuestBridge(format!("could not send protected signing input: {error}"))
    })?;
    writer.write_all(secret.as_bytes()).map_err(|error| {
        ProviderError::GuestBridge(format!("could not send protected signing input: {error}"))
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn verify_code_signing_identity(
    identity_sha1: &str,
    helper_path: &str,
    keychain_path: &str,
    staging_path: &str,
    keychain_password: &str,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
    let probe_path = format!("{staging_path}/codesign-probe");
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!(
            "set -eu; /bin/cp /usr/bin/true {probe}; /bin/chmod 700 {probe}",
            probe = shell_single_quote(&probe_path),
        ),
    )?;

    let remote_command = format!(
        "{} --probe {} {} {}",
        shell_single_quote(helper_path),
        shell_single_quote(keychain_path),
        shell_single_quote(identity_sha1),
        shell_single_quote(&probe_path),
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start the signing probe: {error}"))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not open the protected signing input".to_string())
    })?;
    write_secret_frame(&mut stdin, keychain_password)?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish the signing probe: {error}"))
    })?;
    if !output.status.success() {
        let detail = clean_output(&output.stderr);
        let detail = if detail.is_empty() {
            "macOS rejected the private-key operation".to_string()
        } else {
            detail
        };
        return Err(ProviderError::GuestBridge(format!(
            "the archive imported one certificate/private-key identity, but macOS could not use it to sign code: {detail}"
        )));
    }
    if clean_output(&output.stdout) != "ok" {
        return Err(ProviderError::GuestBridge(
            "the signing probe returned an unexpected result".to_string(),
        ));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn install_apple_wwdr_g3_intermediate(
    pem_path: &str,
    der_path: &str,
    keychain_path: &str,
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
    run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &apple_wwdr_g3_import_command(pem_path, der_path, keychain_path),
    )
    .map(|_| ())
    .map_err(|error| match error {
        ProviderError::GuestBridge(message) => ProviderError::GuestBridge(format!(
            "macOS could not install Apple's pinned WWDR G3 signing intermediate: {message}"
        )),
        error => error,
    })
}

pub(crate) fn apple_wwdr_g3_import_command(
    pem_path: &str,
    der_path: &str,
    keychain_path: &str,
) -> String {
    let pem = shell_single_quote(pem_path);
    let der = shell_single_quote(der_path);
    let keychain = shell_single_quote(keychain_path);
    format!(
        "set -eu; /usr/bin/openssl x509 -in {pem} -outform DER -out {der}; actual_sha256=$(/usr/bin/openssl dgst -sha256 {der} | /usr/bin/awk '{{print toupper($NF)}}'); if /bin/test \"$actual_sha256\" != '{APPLE_WWDR_G3_DER_SHA256}'; then /usr/bin/printf 'the bundled Apple intermediate failed its pinned SHA-256 check' >&2; exit 1; fi; /usr/bin/security import {der} -k {keychain} >/dev/null"
    )
}

pub(crate) fn certificate_metadata_command(certificate_path: &str) -> String {
    let certificate = shell_single_quote(certificate_path);
    format!(
        "set -eu; certificate_start=$(/usr/bin/openssl x509 -inform DER -in {certificate} -noout -startdate); certificate_start=${{certificate_start#notBefore=}}; certificate_end=$(/usr/bin/openssl x509 -inform DER -in {certificate} -noout -enddate); certificate_end=${{certificate_end#notAfter=}}; if ! certificate_start_epoch=$(/bin/date -j -f '%b %e %T %Y %Z' \"$certificate_start\" +%s 2>/dev/null); then /usr/bin/printf 'macOS could not parse the certificate start date: %s' \"$certificate_start\" >&2; exit 1; fi; if ! certificate_end_epoch=$(/bin/date -j -f '%b %e %T %Y %Z' \"$certificate_end\" +%s 2>/dev/null); then /usr/bin/printf 'macOS could not parse the certificate expiry date: %s' \"$certificate_end\" >&2; exit 1; fi; guest_now_epoch=$(/bin/date +%s); guest_now=$(/bin/date -u '+%Y-%m-%dT%H:%M:%SZ'); if /bin/test \"$guest_now_epoch\" -lt \"$certificate_start_epoch\"; then /usr/bin/printf 'the .p12 certificate is not valid until %s; the macOS guest clock is %s' \"$certificate_start\" \"$guest_now\" >&2; exit 1; fi; if /bin/test \"$guest_now_epoch\" -ge \"$certificate_end_epoch\"; then /usr/bin/printf 'the .p12 certificate expired at %s; the macOS guest clock is %s' \"$certificate_end\" \"$guest_now\" >&2; exit 1; fi; /usr/bin/openssl x509 -inform DER -in {certificate} -noout -enddate -fingerprint -sha256 -subject -nameopt RFC2253; /usr/bin/openssl x509 -inform DER -in {certificate} -noout -fingerprint -sha1"
    )
}

pub(crate) fn parse_certificate_metadata(
    output: &str,
) -> Result<(String, String, String, String, String), ProviderError> {
    let mut expires_at = None;
    let mut sha1 = None;
    let mut sha256 = None;
    let mut team_identifier = None;
    let mut identity_name = None;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("notAfter=") {
            let value = value.trim();
            if !value.is_empty()
                && value.len() <= 96
                && value
                    .chars()
                    .all(|character| character.is_ascii_graphic() || character == ' ')
            {
                expires_at = Some(value.to_string());
            }
        } else if line.to_ascii_lowercase().starts_with("sha1 fingerprint=") {
            let value = line
                .split_once('=')
                .map(|(_, value)| value)
                .unwrap_or_default();
            let normalized = value.replace(':', "").to_ascii_uppercase();
            if normalized.len() == 40
                && normalized
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            {
                sha1 = Some(normalized);
            }
        } else if line.to_ascii_lowercase().starts_with("sha256 fingerprint=") {
            let value = line
                .split_once('=')
                .map(|(_, value)| value)
                .unwrap_or_default();
            let normalized = value.replace(':', "").to_ascii_uppercase();
            if normalized.len() == 64
                && normalized
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            {
                sha256 = Some(normalized);
            }
        } else if let Some(subject) = line.strip_prefix("subject=") {
            team_identifier = subject.split(',').find_map(|component| {
                let value = component.trim().strip_prefix("OU=")?;
                (!value.is_empty()
                    && value.len() <= 64
                    && value
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric()))
                .then(|| value.to_string())
            });
            identity_name = subject.split(',').find_map(|component| {
                let value = component.trim().strip_prefix("CN=")?;
                (!value.is_empty()
                    && value.len() <= 256
                    && value.chars().all(|character| !character.is_control()))
                .then(|| value.replace("\\,", ","))
            });
        }
    }

    match (expires_at, sha1, sha256, team_identifier, identity_name) {
        (
            Some(expires_at),
            Some(sha1),
            Some(sha256),
            Some(team_identifier),
            Some(identity_name),
        ) => Ok((expires_at, sha1, sha256, team_identifier, identity_name)),
        _ => Err(ProviderError::GuestBridge(
            "macOS returned invalid signing certificate metadata".to_string(),
        )),
    }
}
