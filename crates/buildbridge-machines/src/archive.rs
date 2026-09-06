//! The signed archive and IPA export, and the artifacts it returns.

use super::*;

#[allow(clippy::too_many_arguments)]
pub fn run_signed_apple_archive<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    signing: &SigningProvisioningResult,
    scheme: &str,
    keychain_password: &str,
    env: Option<&GuestEnvFiles>,
    version: Option<&ProjectVersion>,
    output_directory: &Path,
    mut on_progress: F,
) -> Result<AppleArchiveResult, ProviderError>
where
    F: FnMut(AppleArchiveProgress),
{
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    validate_signing_target(&signing.development_team, &signing.bundle_identifier)?;
    if let Some(version) = version {
        validate_apple_version(version).map_err(ProviderError::GuestBridge)?;
    }
    if !valid_apple_scheme(scheme) {
        return Err(ProviderError::GuestBridge(
            "the stored Xcode scheme is missing or invalid".to_string(),
        ));
    }
    if keychain_password.is_empty() || keychain_password.len() > 512 {
        return Err(ProviderError::GuestBridge(
            "the signing keychain credential is missing or invalid".to_string(),
        ));
    }
    let identity = signing.distribution_identity.as_ref().ok_or_else(|| {
        ProviderError::GuestBridge(
            "no distribution identity is provisioned: the credentials hold only a development identity, which signs a Debug build for a phone but not an App Store archive"
                .to_string(),
        )
    })?;
    if identity.identity_sha1.len() != 40
        || !identity
            .identity_sha1
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(ProviderError::GuestBridge(
            "the provisioned signing identity is invalid".to_string(),
        ));
    }
    let profile = select_app_store_profile(signing).ok_or_else(|| {
        ProviderError::GuestBridge(
            "no installed App Store profile matches the provisioned distribution identity and project (development and ad hoc profiles are not used for App Store archives)"
                .to_string(),
        )
    })?;
    let output_directory = validate_archive_output_directory(output_directory)?;
    let expected_keychain_path =
        format!("/Users/{username}/Library/Keychains/{SIGNING_KEYCHAIN_NAME}");
    if signing.keychain_path != expected_keychain_path {
        return Err(ProviderError::GuestBridge(
            "the provisioned signing keychain path is invalid".to_string(),
        ));
    }
    let ipa_path = output_directory.join(format!("{scheme}-AppStore.ipa"));
    let archive_path = output_directory.join(format!("{scheme}.xcarchive.zip"));
    let ipa_partial_path = ipa_path.with_extension("ipa.part");
    let archive_partial_path = archive_path.with_extension("zip.part");
    for path in [
        &ipa_path,
        &archive_path,
        &ipa_partial_path,
        &archive_partial_path,
    ] {
        if path.exists() {
            return Err(ProviderError::GuestBridge(
                "the signed artifact destination is not empty".to_string(),
            ));
        }
    }

    let started_at = Instant::now();
    let guest_home = format!("/Users/{username}");
    let guest_tools = format!("{guest_home}/.buildbridge/tools");
    let helper_source = format!("{guest_tools}/signing-helper.c");
    let helper_binary = format!("{guest_tools}/signing-helper");
    let xcodebuild =
        format!("{guest_home}/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild");
    let workspace = format!("{guest_home}/BuildBridge/workspaces/active/ios/App/App.xcworkspace");
    let operation_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let staging =
        format!("{guest_home}/Library/Caches/dev.buildbridge.desktop/archive-{operation_id}");
    let guest_archive = format!("{staging}/{scheme}.xcarchive");
    let guest_archive_zip = format!("{staging}/{scheme}.xcarchive.zip");
    let guest_derived_data = format!("{staging}/DerivedData");
    let guest_signing_settings = format!("{staging}/Signing.xcconfig");
    let guest_export_options = format!("{staging}/ExportOptions.plist");
    let guest_export = format!("{staging}/Export");
    let export_options = apple_export_options_plist(
        &signing.development_team,
        &signing.bundle_identifier,
        &profile.uuid,
        &identity.identity_sha1,
    );

    on_progress(archive_progress(
        AppleArchivePhase::Preparing,
        0,
        0,
        started_at,
        "Preparing the fixed Release and App Store Connect export recipe.",
        None,
    ));
    let prepare = format!(
        "set -eu; /bin/mkdir -p {} {}; /bin/chmod 700 {}; /bin/rm -rf {}; /bin/mkdir -p {} {}; /bin/chmod 700 {}",
        shell_single_quote(&guest_tools),
        shell_single_quote(&format!(
            "{guest_home}/Library/Caches/dev.buildbridge.desktop"
        )),
        shell_single_quote(&guest_tools),
        shell_single_quote(&staging),
        shell_single_quote(&staging),
        shell_single_quote(&guest_export),
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
        if let Some(env) = env {
            rebuild_web_assets_with_env(
                env,
                ssh_port,
                username,
                identity_path,
                known_hosts_path,
                &guest_home,
                started_at,
                &mut on_progress,
            )?;
        }
        let archive_target = resolve_archive_app_target(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &xcodebuild,
            &workspace,
            scheme,
            &signing.bundle_identifier,
        )?;
        let mut signing_settings = apple_archive_signing_xcconfig(
            &archive_target,
            &signing.development_team,
            &identity.identity_sha1,
            &profile.uuid,
        );
        // The requested version rides in the same settings file, so the archive carries it
        // whatever the synced project says; the archived app is checked against it below.
        if let Some(version) = version {
            signing_settings.push_str(&apple_version_xcconfig(version));
            on_progress(archive_progress(
                AppleArchivePhase::Preparing,
                0,
                0,
                started_at,
                "Preparing the fixed Release and App Store Connect export recipe.",
                Some(format!("Archiving as version {}.", version.display())),
            ));
        }
        install_signing_helper(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &helper_source,
            &helper_binary,
        )?;
        stream_bytes_to_guest(
            signing_settings.as_bytes(),
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_signing_settings,
            "target-scoped signing settings",
        )?;
        stream_bytes_to_guest(
            export_options.as_bytes(),
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_export_options,
            "App Store export options",
        )?;

        let mut output_tail = run_archive_helper(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &helper_binary,
            &signing.keychain_path,
            &xcodebuild,
            &workspace,
            scheme,
            &guest_archive,
            &guest_derived_data,
            &guest_signing_settings,
            &guest_export_options,
            &guest_export,
            keychain_password,
            started_at,
            &mut on_progress,
        )?;

        on_progress(archive_progress(
            AppleArchivePhase::Verifying,
            0,
            0,
            started_at,
            "Verifying the archived app signature and reading release metadata.",
            None,
        ));
        let mut inspection = inspect_apple_archive(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_archive,
            &guest_export,
        )?;
        if inspection.bundle_identifier != signing.bundle_identifier {
            return Err(ProviderError::GuestBridge(format!(
                "the exported archive uses bundle identifier {}, not {}",
                inspection.bundle_identifier, signing.bundle_identifier
            )));
        }
        if let Some(version) = version
            && (inspection.marketing_version != version.version
                || inspection.build_number != version.build)
        {
            return Err(ProviderError::GuestBridge(format!(
                "the exported archive reports version {} ({}), not the requested {}; the app's Info.plist must take CFBundleShortVersionString from MARKETING_VERSION and CFBundleVersion from CURRENT_PROJECT_VERSION for a version set here to reach it",
                inspection.marketing_version,
                inspection.build_number,
                version.display()
            )));
        }
        on_progress(archive_progress(
            AppleArchivePhase::PackagingArchive,
            0,
            0,
            started_at,
            "The signature is valid; packaging the portable Xcode archive.",
            None,
        ));
        let (archive_bytes, archive_sha256) = package_apple_archive(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_archive,
            &guest_archive_zip,
        )?;
        inspection.archive_bytes = archive_bytes;
        inspection.archive_sha256 = archive_sha256;

        let total_bytes = inspection
            .ipa_bytes
            .checked_add(inspection.archive_bytes)
            .ok_or_else(|| {
                ProviderError::GuestBridge("the signed artifact sizes are invalid".to_string())
            })?;
        if inspection.ipa_bytes == 0
            || inspection.archive_bytes == 0
            || inspection.ipa_bytes > APPLE_ARCHIVE_MAX_BYTES
            || inspection.archive_bytes > APPLE_ARCHIVE_MAX_BYTES
            || total_bytes > APPLE_ARCHIVE_MAX_BYTES
        {
            return Err(ProviderError::GuestBridge(
                "the signed artifacts exceed the 20 GiB transfer limit".to_string(),
            ));
        }
        let guest_ipa = format!("{guest_export}/{}", inspection.ipa_name);
        stream_guest_artifact(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_ipa,
            &ipa_partial_path,
            inspection.ipa_bytes,
            0,
            total_bytes,
            "Transferring the signed IPA to this host.",
            started_at,
            &mut on_progress,
        )?;
        stream_guest_artifact(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &guest_archive_zip,
            &archive_partial_path,
            inspection.archive_bytes,
            inspection.ipa_bytes,
            total_bytes,
            "Transferring the portable Xcode archive to this host.",
            started_at,
            &mut on_progress,
        )?;
        verify_local_artifact(
            &ipa_partial_path,
            inspection.ipa_bytes,
            &inspection.ipa_sha256,
        )?;
        verify_local_artifact(
            &archive_partial_path,
            inspection.archive_bytes,
            &inspection.archive_sha256,
        )?;
        fs::rename(&ipa_partial_path, &ipa_path).map_err(|error| {
            ProviderError::GuestBridge(format!("could not retain the signed IPA: {error}"))
        })?;
        fs::rename(&archive_partial_path, &archive_path).map_err(|error| {
            ProviderError::GuestBridge(format!("could not retain the Xcode archive: {error}"))
        })?;
        set_artifact_permissions(&ipa_path)?;
        set_artifact_permissions(&archive_path)?;
        output_tail.push(format!(
            "Verified signed {} ({}) and retained both local artifacts.",
            inspection.bundle_identifier, inspection.marketing_version
        ));
        if output_tail.len() > APPLE_BUILD_OUTPUT_TAIL_LINES {
            output_tail.remove(0);
        }

        Ok(AppleArchiveResult {
            scheme: scheme.to_string(),
            configuration: "Release".to_string(),
            export_method: "app-store-connect".to_string(),
            bundle_identifier: inspection.bundle_identifier,
            development_team: signing.development_team.clone(),
            marketing_version: inspection.marketing_version,
            build_number: inspection.build_number,
            provisioning_profile_uuid: profile.uuid.clone(),
            ipa: AppleArchiveArtifact {
                path: ipa_path.display().to_string(),
                bytes: inspection.ipa_bytes,
                sha256: inspection.ipa_sha256,
            },
            archive: AppleArchiveArtifact {
                path: archive_path.display().to_string(),
                bytes: inspection.archive_bytes,
                sha256: inspection.archive_sha256,
            },
            output_tail,
        })
    })();

    let _ = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!("/bin/rm -rf {}", shell_single_quote(&staging)),
    );
    if operation.is_err() {
        for path in [
            &ipa_path,
            &archive_path,
            &ipa_partial_path,
            &archive_partial_path,
        ] {
            let _ = fs::remove_file(path);
        }
    }
    let result = operation?;
    on_progress(archive_progress(
        AppleArchivePhase::Completed,
        result.ipa.bytes + result.archive.bytes,
        result.ipa.bytes + result.archive.bytes,
        started_at,
        "Signed archive and IPA verified and retained on this host.",
        None,
    ));

    Ok(result)
}

pub(crate) fn valid_apple_scheme(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, ' ' | '-' | '_' | '.')
        })
}

pub(crate) fn validate_archive_output_directory(path: &Path) -> Result<PathBuf, ProviderError> {
    if !path.is_absolute() {
        return Err(ProviderError::GuestBridge(
            "the signed artifact destination must be an absolute directory".to_string(),
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ProviderError::GuestBridge(format!(
            "the signed artifact destination is unavailable: {error}"
        ))
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ProviderError::GuestBridge(
            "the signed artifact destination must be a real directory".to_string(),
        ));
    }

    fs::canonicalize(path).map_err(|error| {
        ProviderError::GuestBridge(format!(
            "could not resolve the signed artifact destination: {error}"
        ))
    })
}

pub(crate) fn apple_export_options_plist(
    development_team: &str,
    bundle_identifier: &str,
    profile_uuid: &str,
    identity_sha1: &str,
) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>destination</key>
    <string>export</string>
    <key>manageAppVersionAndBuildNumber</key>
    <false/>
    <key>method</key>
    <string>app-store-connect</string>
    <key>provisioningProfiles</key>
    <dict>
        <key>{bundle_identifier}</key>
        <string>{profile_uuid}</string>
    </dict>
    <key>signingCertificate</key>
    <string>{identity_sha1}</string>
    <key>signingStyle</key>
    <string>manual</string>
    <key>stripSwiftSymbols</key>
    <true/>
    <key>teamID</key>
    <string>{development_team}</string>
    <key>uploadSymbols</key>
    <true/>
</dict>
</plist>
"#
    )
}

pub(crate) fn archive_progress(
    phase: AppleArchivePhase,
    completed_bytes: u64,
    total_bytes: u64,
    started_at: Instant,
    detail: &str,
    log_line: Option<String>,
) -> AppleArchiveProgress {
    AppleArchiveProgress {
        phase,
        completed_bytes,
        total_bytes,
        elapsed_seconds: started_at.elapsed().as_secs(),
        detail: detail.to_string(),
        log_line,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_archive_app_target(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    xcodebuild_path: &str,
    workspace_path: &str,
    scheme: &str,
    expected_bundle_identifier: &str,
) -> Result<String, ProviderError> {
    let command = format!(
        "set -o pipefail; {} -workspace {} -scheme {} -configuration Release -destination 'generic/platform=iOS' -showBuildSettings | /usr/bin/awk '$1 == \"TARGET_NAME\" || $1 == \"PRODUCT_BUNDLE_IDENTIFIER\" {{ print }}'",
        shell_single_quote(xcodebuild_path),
        shell_single_quote(workspace_path),
        shell_single_quote(scheme),
    );
    let output = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &command,
    )?;
    let target = build_setting_value(&output, "TARGET_NAME").ok_or_else(|| {
        ProviderError::GuestBridge(
            "Xcode did not return the application target for the selected scheme".to_string(),
        )
    })?;
    let bundle_identifier =
        build_setting_value(&output, "PRODUCT_BUNDLE_IDENTIFIER").ok_or_else(|| {
            ProviderError::GuestBridge(
                "Xcode did not return the application bundle identifier for the selected scheme"
                    .to_string(),
            )
        })?;
    if bundle_identifier != expected_bundle_identifier {
        return Err(ProviderError::GuestBridge(format!(
            "the selected Xcode scheme resolves to bundle identifier {bundle_identifier}, not {expected_bundle_identifier}"
        )));
    }
    if target.is_empty()
        || target.len() > 128
        || !target
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(ProviderError::GuestBridge(
            "the application target name cannot be represented safely in target-scoped signing settings"
                .to_string(),
        ));
    }
    Ok(target.to_string())
}

pub(crate) fn build_setting_value<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output.lines().find_map(|line| {
        let (candidate, value) = line.trim().split_once(" = ")?;
        (candidate == key).then_some(value.trim())
    })
}

pub(crate) fn apple_archive_signing_xcconfig(
    target: &str,
    development_team: &str,
    identity_sha1: &str,
    profile_uuid: &str,
) -> String {
    format!(
        "BUILDBRIDGE_TEAM_{target} = {development_team}\n\
BUILDBRIDGE_IDENTITY_{target} = {identity_sha1}\n\
BUILDBRIDGE_PROFILE_{target} = {profile_uuid}\n\
BUILDBRIDGE_STYLE_{target} = Manual\n\
DEVELOPMENT_TEAM = $(BUILDBRIDGE_TEAM_$(TARGET_NAME))\n\
CODE_SIGN_IDENTITY = $(BUILDBRIDGE_IDENTITY_$(TARGET_NAME))\n\
PROVISIONING_PROFILE_SPECIFIER = $(BUILDBRIDGE_PROFILE_$(TARGET_NAME))\n\
CODE_SIGN_STYLE = $(BUILDBRIDGE_STYLE_$(TARGET_NAME))\n"
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_archive_helper<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    helper_path: &str,
    keychain_path: &str,
    xcodebuild_path: &str,
    workspace_path: &str,
    scheme: &str,
    archive_path: &str,
    derived_data_path: &str,
    signing_settings_path: &str,
    export_options_path: &str,
    export_path: &str,
    keychain_password: &str,
    started_at: Instant,
    on_progress: &mut F,
) -> Result<Vec<String>, ProviderError>
where
    F: FnMut(AppleArchiveProgress),
{
    let remote_command = format!(
        "{} --archive {} {} {} {} {} {} {} {} {} 2>&1",
        shell_single_quote(helper_path),
        shell_single_quote(keychain_path),
        shell_single_quote(xcodebuild_path),
        shell_single_quote(workspace_path),
        shell_single_quote(scheme),
        shell_single_quote(archive_path),
        shell_single_quote(derived_data_path),
        shell_single_quote(signing_settings_path),
        shell_single_quote(export_options_path),
        shell_single_quote(export_path),
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start the signed archive: {error}"))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not open protected archive input".to_string())
    })?;
    write_secret_frame(&mut stdin, keychain_password)?;
    drop(stdin);
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture signed archive output".to_string())
    })?;
    let mut phase = AppleArchivePhase::Archiving;
    let mut output_tail = Vec::new();
    let mut diagnostic_lines = Vec::new();

    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| {
            ProviderError::GuestBridge(format!("could not read signed archive output: {error}"))
        })?;
        if let Some(marker) = line.strip_prefix("__BUILDBRIDGE_ARCHIVE__:") {
            phase = match marker {
                "archiving" => AppleArchivePhase::Archiving,
                "exporting" => AppleArchivePhase::Exporting,
                "complete" => AppleArchivePhase::Verifying,
                _ => {
                    return Err(ProviderError::GuestBridge(
                        "macOS returned an unknown archive phase".to_string(),
                    ));
                }
            };
            on_progress(archive_progress(
                phase,
                0,
                0,
                started_at,
                archive_phase_detail(phase),
                None,
            ));
            continue;
        }

        let line = sanitize_build_log_line(&line);
        if line.is_empty() {
            continue;
        }
        if apple_archive_log_is_diagnostic(&line) {
            diagnostic_lines.push(line.clone());
            if diagnostic_lines.len() > APPLE_BUILD_DIAGNOSTIC_LINES {
                diagnostic_lines.remove(0);
            }
        }
        output_tail.push(line.clone());
        if output_tail.len() > APPLE_BUILD_OUTPUT_TAIL_LINES {
            output_tail.remove(0);
        }
        on_progress(archive_progress(
            phase,
            0,
            0,
            started_at,
            archive_phase_detail(phase),
            Some(line),
        ));
    }

    let status = child.wait().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish the signed archive: {error}"))
    })?;
    if !status.success() {
        let context = build_failure_context(&diagnostic_lines, &output_tail);
        return Err(ProviderError::GuestBridge(if context.is_empty() {
            format!(
                "the signed archive failed during {}",
                archive_phase_detail(phase)
            )
        } else {
            format!(
                "the signed archive failed during {}:\n{context}",
                archive_phase_detail(phase)
            )
        }));
    }

    Ok(output_tail)
}

pub(crate) fn archive_phase_detail(phase: AppleArchivePhase) -> &'static str {
    match phase {
        AppleArchivePhase::Preparing => "Preparing the signed Release recipe",
        AppleArchivePhase::BuildingWebAssets => {
            "Rebuilding the web assets with the chosen environment"
        }
        AppleArchivePhase::Archiving => "Compiling and signing the Release archive",
        AppleArchivePhase::Exporting => "Exporting the App Store Connect IPA",
        AppleArchivePhase::Verifying => "Verifying the archived app signature",
        AppleArchivePhase::PackagingArchive => "Packaging the portable Xcode archive",
        AppleArchivePhase::Transferring => "Transferring verified artifacts to this host",
        AppleArchivePhase::Completed => "Signed archive and IPA complete",
    }
}

pub(crate) fn apple_archive_log_is_diagnostic(line: &str) -> bool {
    let normalized = line.to_ascii_lowercase();

    apple_build_log_is_diagnostic(line)
        || normalized.contains("provisioning profile")
        || normalized.contains("requires a provisioning profile")
        || normalized.contains("requires a development team")
        || normalized.contains("code signing")
        || normalized.contains("codesign")
        || normalized.contains("xcode_archive_failed")
        || normalized.contains("xcode_export_failed")
        || normalized.starts_with("error: exportarchive")
        || normalized.starts_with("** archive failed **")
        || normalized.starts_with("** export failed **")
}

#[derive(Debug)]
pub(crate) struct AppleArchiveInspection {
    pub(crate) bundle_identifier: String,
    pub(crate) marketing_version: String,
    pub(crate) build_number: String,
    pub(crate) ipa_name: String,
    pub(crate) ipa_bytes: u64,
    pub(crate) ipa_sha256: String,
    pub(crate) archive_bytes: u64,
    pub(crate) archive_sha256: String,
}

pub(crate) fn inspect_apple_archive(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    archive_path: &str,
    export_path: &str,
) -> Result<AppleArchiveInspection, ProviderError> {
    let archive = shell_single_quote(archive_path);
    let export = shell_single_quote(export_path);
    let command = format!(
        "set -eu; app_relative=$(/usr/bin/plutil -extract ApplicationProperties.ApplicationPath raw -o - {archive}/Info.plist); app_name=${{app_relative#Applications/}}; case \"$app_relative\" in Applications/*.app) ;; *) /usr/bin/printf 'invalid_archive_app_path' >&2; exit 1;; esac; case \"$app_name\" in ''|*/*|*..*) /usr/bin/printf 'invalid_archive_app_name' >&2; exit 1;; esac; app_path={archive}/Products/\"$app_relative\"; /usr/bin/codesign --verify --deep --strict --verbose=2 \"$app_path\"; bundle=$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - \"$app_path/Info.plist\"); version=$(/usr/bin/plutil -extract CFBundleShortVersionString raw -o - \"$app_path/Info.plist\"); build=$(/usr/bin/plutil -extract CFBundleVersion raw -o - \"$app_path/Info.plist\"); ipa_count=$(/usr/bin/find {export} -maxdepth 1 -type f -name '*.ipa' | /usr/bin/wc -l | /usr/bin/tr -d ' '); /bin/test \"$ipa_count\" -eq 1; ipa=$(/usr/bin/find {export} -maxdepth 1 -type f -name '*.ipa'); ipa_name=$(/usr/bin/basename \"$ipa\"); case \"$ipa_name\" in ''|*/*|*..*) /usr/bin/printf 'invalid_ipa_name' >&2; exit 1;; esac; ipa_bytes=$(/usr/bin/stat -f %z \"$ipa\"); ipa_sha=$(/usr/bin/shasum -a 256 \"$ipa\" | /usr/bin/awk '{{print $1}}'); /usr/bin/printf '__BUILDBRIDGE_APP__\\t%s\\t%s\\t%s\\n__BUILDBRIDGE_IPA__\\t%s\\t%s\\t%s\\n' \"$bundle\" \"$version\" \"$build\" \"$ipa_name\" \"$ipa_bytes\" \"$ipa_sha\""
    );
    let output = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &command,
    )?;
    parse_apple_archive_inspection(&output)
}

pub(crate) fn parse_apple_archive_inspection(
    output: &str,
) -> Result<AppleArchiveInspection, ProviderError> {
    let mut app = None;
    let mut ipa = None;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("__BUILDBRIDGE_APP__\t") {
            let fields = value.split('\t').collect::<Vec<_>>();
            if fields.len() == 3 {
                app = Some((fields[0], fields[1], fields[2]));
            }
        } else if let Some(value) = line.strip_prefix("__BUILDBRIDGE_IPA__\t") {
            let fields = value.split('\t').collect::<Vec<_>>();
            if fields.len() == 3 {
                ipa = Some((fields[0], fields[1], fields[2]));
            }
        }
    }
    let (bundle_identifier, marketing_version, build_number) = app.ok_or_else(|| {
        ProviderError::GuestBridge("macOS returned incomplete archive metadata".to_string())
    })?;
    validate_signing_target("TEAM", bundle_identifier)?;
    if !valid_release_value(marketing_version) || !valid_release_value(build_number) {
        return Err(ProviderError::GuestBridge(
            "macOS returned invalid release version metadata".to_string(),
        ));
    }
    let (ipa_name, ipa_bytes, ipa_sha256) = ipa.ok_or_else(|| {
        ProviderError::GuestBridge("macOS did not return exactly one exported IPA".to_string())
    })?;
    if !valid_artifact_name(ipa_name) || !valid_sha256(ipa_sha256) {
        return Err(ProviderError::GuestBridge(
            "macOS returned invalid IPA metadata".to_string(),
        ));
    }
    let ipa_bytes = ipa_bytes.parse::<u64>().map_err(|_| {
        ProviderError::GuestBridge("macOS returned an invalid IPA size".to_string())
    })?;

    Ok(AppleArchiveInspection {
        bundle_identifier: bundle_identifier.to_string(),
        marketing_version: marketing_version.to_string(),
        build_number: build_number.to_string(),
        ipa_name: ipa_name.to_string(),
        ipa_bytes,
        ipa_sha256: ipa_sha256.to_ascii_uppercase(),
        archive_bytes: 0,
        archive_sha256: String::new(),
    })
}

pub(crate) fn package_apple_archive(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    archive_path: &str,
    archive_zip_path: &str,
) -> Result<(u64, String), ProviderError> {
    let archive = shell_single_quote(archive_path);
    let archive_zip = shell_single_quote(archive_zip_path);
    let command = format!(
        "set -eu; /bin/rm -f {archive_zip}; /usr/bin/ditto -c -k --sequesterRsrc --keepParent {archive} {archive_zip}; archive_bytes=$(/usr/bin/stat -f %z {archive_zip}); archive_sha=$(/usr/bin/shasum -a 256 {archive_zip} | /usr/bin/awk '{{print $1}}'); /usr/bin/printf '%s\\t%s\\n' \"$archive_bytes\" \"$archive_sha\""
    );
    let output = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &command,
    )?;
    let mut fields = output.split('\t');
    let bytes = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            ProviderError::GuestBridge("macOS returned an invalid archive size".to_string())
        })?;
    let sha256 = fields.next().unwrap_or_default();
    if fields.next().is_some() || !valid_sha256(sha256) {
        return Err(ProviderError::GuestBridge(
            "macOS returned invalid archive metadata".to_string(),
        ));
    }

    Ok((bytes, sha256.to_ascii_uppercase()))
}

pub(crate) fn valid_release_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
}

pub(crate) fn valid_artifact_name(value: &str) -> bool {
    value.len() > 4
        && value.len() <= 255
        && value.to_ascii_lowercase().ends_with(".ipa")
        && !value.contains("..")
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, ' ' | '.' | '-' | '_')
        })
}

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn stream_guest_artifact<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    guest_path: &str,
    local_path: &Path,
    expected_bytes: u64,
    completed_offset: u64,
    total_bytes: u64,
    detail: &str,
    started_at: Instant,
    on_progress: &mut F,
) -> Result<(), ProviderError>
where
    F: FnMut(AppleArchiveProgress),
{
    let remote_command = format!("set -eu; /bin/cat {}", shell_single_quote(guest_path));
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start artifact transfer: {error}"))
        })?;
    let mut stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not read the signed artifact transfer".to_string())
    })?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(local_path)
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not create the local artifact: {error}"))
        })?;
    set_artifact_permissions(local_path)?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut copied = 0_u64;
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    loop {
        let count = stdout.read(&mut buffer).map_err(|error| {
            ProviderError::GuestBridge(format!("could not read the signed artifact: {error}"))
        })?;
        if count == 0 {
            break;
        }
        copied = copied.checked_add(count as u64).ok_or_else(|| {
            ProviderError::GuestBridge("the signed artifact size overflowed".to_string())
        })?;
        if copied > expected_bytes {
            return Err(ProviderError::GuestBridge(
                "macOS sent more artifact data than declared".to_string(),
            ));
        }
        file.write_all(&buffer[..count]).map_err(|error| {
            ProviderError::GuestBridge(format!("could not retain the signed artifact: {error}"))
        })?;
        if last_progress.elapsed() >= Duration::from_millis(200) || copied == expected_bytes {
            on_progress(archive_progress(
                AppleArchivePhase::Transferring,
                completed_offset + copied,
                total_bytes,
                started_at,
                detail,
                None,
            ));
            last_progress = Instant::now();
        }
    }
    file.sync_all().map_err(|error| {
        ProviderError::GuestBridge(format!("could not flush the signed artifact: {error}"))
    })?;
    drop(file);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish artifact transfer: {error}"))
    })?;
    if !output.status.success() {
        return Err(ProviderError::GuestBridge(format!(
            "the signed artifact transfer failed: {}",
            clean_output(&output.stderr)
        )));
    }
    if copied != expected_bytes {
        return Err(ProviderError::GuestBridge(format!(
            "the signed artifact transfer was incomplete ({copied} of {expected_bytes} bytes)"
        )));
    }

    Ok(())
}

pub(crate) fn verify_local_artifact(
    path: &Path,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<(), ProviderError> {
    let metadata = fs::metadata(path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not inspect the local artifact: {error}"))
    })?;
    if !metadata.is_file() || metadata.len() != expected_bytes {
        return Err(ProviderError::GuestBridge(
            "the local artifact size does not match macOS".to_string(),
        ));
    }
    let mut file = File::open(path).map_err(|error| {
        ProviderError::GuestBridge(format!("could not open the local artifact: {error}"))
    })?;
    let mut signature = [0_u8; 4];
    file.read_exact(&mut signature).map_err(|error| {
        ProviderError::GuestBridge(format!("could not read the local artifact: {error}"))
    })?;
    if signature != *b"PK\x03\x04" {
        return Err(ProviderError::GuestBridge(
            "the transferred artifact is not a valid ZIP container".to_string(),
        ));
    }
    let actual_sha256 = snapshot_sha256(path)?;
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err(ProviderError::GuestBridge(
            "the transferred artifact checksum does not match macOS".to_string(),
        ));
    }

    Ok(())
}

pub(crate) fn set_artifact_permissions(path: &Path) -> Result<(), ProviderError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not restrict signed artifact permissions: {error}"
            ))
        })?;
    }

    Ok(())
}

pub(crate) struct TemporaryArchive(pub(crate) PathBuf);

impl TemporaryArchive {
    pub(crate) fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "buildbridge-workspace-{}-{nonce}.tar.gz",
            std::process::id()
        )))
    }
}

impl Drop for TemporaryArchive {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
