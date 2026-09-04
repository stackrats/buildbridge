//! Provisioning profiles in the guest: inspection, classification, selection, install.

use super::*;

/// The fixed shell for reading one profile's metadata in the guest: decode, read the UUID,
/// team, application identifier and expiry, hash each developer certificate, then the
/// entitlement flags and provisioned devices. `plutil` prints a missing key's error on stdout
/// and exits 1, so each boolean is filtered to a bare `true`/`false` and defaults to `false`.
pub(crate) fn profile_inspection_command(guest_profile: &str) -> String {
    let profile = shell_single_quote(guest_profile);
    let plist = shell_single_quote(&format!("{guest_profile}.plist"));
    let certificate = shell_single_quote(&format!("{guest_profile}.certificate.der"));
    format!(
        "; /usr/bin/security cms -D -i {profile} > {plist}; uuid=$(/usr/bin/plutil -extract UUID raw -o - {plist}); team=$(/usr/bin/plutil -extract TeamIdentifier.0 raw -o - {plist}); app_id=$(/usr/bin/plutil -extract Entitlements.application-identifier raw -o - {plist}); expires_at=$(/usr/bin/plutil -extract ExpirationDate raw -o - {plist}); if ! expiry_epoch=$(/bin/date -j -f '%Y-%m-%d %H:%M:%S %z' \"$expires_at\" +%s 2>/dev/null || /bin/date -j -f '%Y-%m-%dT%H:%M:%SZ' \"$expires_at\" +%s 2>/dev/null); then /usr/bin/printf 'invalid_profile_expiry:%s' \"$uuid\" >&2; exit 1; fi; if /bin/test \"$expiry_epoch\" -le \"$(/bin/date +%s)\"; then /usr/bin/printf 'expired_profile:%s' \"$uuid\" >&2; exit 1; fi; /usr/bin/printf '__BUILDBRIDGE_PROFILE__\\t%s\\t%s\\t%s\\t%s\\n' \"$uuid\" \"$team\" \"$app_id\" \"$expires_at\"; certificate_count=$(/usr/bin/plutil -extract DeveloperCertificates xml1 -o - {plist} | /usr/bin/grep -c '<data>'); certificate_index=0; while /bin/test \"$certificate_index\" -lt \"$certificate_count\"; do /usr/bin/plutil -extract \"DeveloperCertificates.$certificate_index\" raw -o - {plist} | /usr/bin/base64 -D > {certificate}; certificate_sha256=$(/usr/bin/openssl dgst -sha256 {certificate} | /usr/bin/awk '{{print $NF}}'); /usr/bin/printf '__BUILDBRIDGE_PROFILE_CERT__\\t%s\\t%s\\n' \"$uuid\" \"$certificate_sha256\"; certificate_index=$((certificate_index + 1)); done; get_task_allow=$(/usr/bin/plutil -extract Entitlements.get-task-allow raw -o - {plist} 2>/dev/null | /usr/bin/grep -x -E 'true|false' || /bin/echo false); provisions_all=$(/usr/bin/plutil -extract ProvisionsAllDevices raw -o - {plist} 2>/dev/null | /usr/bin/grep -x -E 'true|false' || /bin/echo false); /usr/bin/printf '__BUILDBRIDGE_PROFILE_FLAGS__\\t%s\\t%s\\t%s\\n' \"$uuid\" \"$get_task_allow\" \"$provisions_all\"; device_count=$(/usr/bin/plutil -extract ProvisionedDevices xml1 -o - {plist} 2>/dev/null | /usr/bin/grep -c '<string>' || /usr/bin/true); device_index=0; while /bin/test \"${{device_count:-0}}\" -gt \"$device_index\"; do device_udid=$(/usr/bin/plutil -extract \"ProvisionedDevices.$device_index\" raw -o - {plist}); /usr/bin/printf '__BUILDBRIDGE_PROFILE_DEVICE__\\t%s\\t%s\\n' \"$uuid\" \"$device_udid\"; device_index=$((device_index + 1)); done; /bin/rm -f {certificate} {plist}"
    )
}

pub(crate) fn inspect_guest_profiles(
    profiles: &[(String, PathBuf)],
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<String, ProviderError> {
    let mut command = "set -eu; umask 077".to_string();
    for (guest_profile, _) in profiles {
        command.push_str(&profile_inspection_command(guest_profile));
    }

    match run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &command,
    ) {
        Err(ProviderError::GuestBridge(message)) if message.starts_with("expired_profile:") => {
            Err(ProviderError::GuestBridge(format!(
                "provisioning profile {} has expired",
                message.trim_start_matches("expired_profile:")
            )))
        }
        Err(ProviderError::GuestBridge(message))
            if message.starts_with("invalid_profile_expiry:") =>
        {
            Err(ProviderError::GuestBridge(format!(
                "provisioning profile {} has an unreadable expiration date",
                message.trim_start_matches("invalid_profile_expiry:")
            )))
        }
        result => result,
    }
}

pub(crate) fn parse_profile_summaries(
    output: &str,
) -> Result<Vec<ProvisioningProfileSummary>, ProviderError> {
    let mut profiles: Vec<ProvisioningProfileSummary> = Vec::new();
    // (get-task-allow, ProvisionsAllDevices) per profile, in the order the guest reported.
    let mut flags: Vec<(String, bool, bool)> = Vec::new();
    for line in output.lines() {
        if let Some(values) = line.strip_prefix("__BUILDBRIDGE_PROFILE_FLAGS__\t") {
            let fields = values.split('\t').collect::<Vec<_>>();
            let parse_flag = |value: &str| match value {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
            let (Some(&uuid), Some(get_task_allow), Some(provisions_all)) = (
                fields.first(),
                fields.get(1).and_then(|value| parse_flag(value)),
                fields.get(2).and_then(|value| parse_flag(value)),
            ) else {
                return Err(ProviderError::GuestBridge(
                    "macOS returned invalid provisioning profile entitlement metadata".to_string(),
                ));
            };
            if fields.len() != 3
                || !profiles.iter().any(|profile| profile.uuid == uuid)
                || flags.iter().any(|(seen, _, _)| seen == uuid)
            {
                return Err(ProviderError::GuestBridge(
                    "macOS returned out-of-order provisioning profile metadata".to_string(),
                ));
            }
            flags.push((uuid.to_string(), get_task_allow, provisions_all));
            continue;
        }
        if let Some(values) = line.strip_prefix("__BUILDBRIDGE_PROFILE_DEVICE__\t") {
            let Some((uuid, udid)) = values.split_once('\t') else {
                return Err(ProviderError::GuestBridge(
                    "macOS returned invalid provisioning profile device metadata".to_string(),
                ));
            };
            let Some(profile) = profiles.iter_mut().find(|profile| profile.uuid == uuid) else {
                return Err(ProviderError::GuestBridge(
                    "macOS returned out-of-order provisioning profile metadata".to_string(),
                ));
            };
            let udid = udid.to_ascii_uppercase();
            if !valid_device_udid(&udid)
                || profile.provisioned_device_udids.len() >= 400
                || profile.provisioned_device_udids.contains(&udid)
            {
                return Err(ProviderError::GuestBridge(
                    "macOS returned invalid provisioning profile device metadata".to_string(),
                ));
            }
            profile.provisioned_device_udids.push(udid);
            continue;
        }
        if let Some(values) = line.strip_prefix("__BUILDBRIDGE_PROFILE_CERT__\t") {
            let Some((uuid, fingerprint)) = values.split_once('\t') else {
                return Err(ProviderError::GuestBridge(
                    "macOS returned invalid provisioning profile certificate metadata".to_string(),
                ));
            };
            let fingerprint = fingerprint.to_ascii_uppercase();
            let Some(profile) = profiles.iter_mut().find(|profile| profile.uuid == uuid) else {
                return Err(ProviderError::GuestBridge(
                    "macOS returned out-of-order provisioning profile metadata".to_string(),
                ));
            };
            if fingerprint.len() != 64
                || !fingerprint
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
                || profile
                    .developer_certificate_sha256
                    .iter()
                    .any(|stored| stored == &fingerprint)
            {
                return Err(ProviderError::GuestBridge(
                    "macOS returned invalid provisioning profile certificate metadata".to_string(),
                ));
            }
            profile.developer_certificate_sha256.push(fingerprint);
            continue;
        }
        let Some(values) = line.strip_prefix("__BUILDBRIDGE_PROFILE__\t") else {
            continue;
        };
        let mut fields = values.splitn(4, '\t');
        let uuid = fields.next().unwrap_or_default();
        let team_identifier = fields.next().unwrap_or_default();
        let application_identifier = fields.next().unwrap_or_default();
        let expires_at = fields.next().unwrap_or_default();
        if !valid_profile_uuid(uuid)
            || team_identifier.is_empty()
            || team_identifier.len() > 64
            || !team_identifier
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
            || application_identifier.is_empty()
            || application_identifier.len() > 320
            || !application_identifier.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '*')
            })
            || application_identifier.matches('*').count() > 1
            || (application_identifier.contains('*') && !application_identifier.ends_with('*'))
            || expires_at.is_empty()
            || expires_at.len() > 96
            || !expires_at
                .chars()
                .all(|character| character.is_ascii_graphic() || character == ' ')
            || profiles
                .iter()
                .any(|profile: &ProvisioningProfileSummary| profile.uuid == uuid)
        {
            return Err(ProviderError::GuestBridge(
                "macOS returned invalid provisioning profile metadata".to_string(),
            ));
        }
        profiles.push(ProvisioningProfileSummary {
            uuid: uuid.to_string(),
            team_identifier: team_identifier.to_string(),
            application_identifier: application_identifier.to_string(),
            expires_at: expires_at.to_string(),
            developer_certificate_sha256: Vec::new(),
            kind: None,
            provisioned_device_udids: Vec::new(),
            get_task_allow: false,
        });
    }
    if profiles
        .iter()
        .any(|profile| profile.developer_certificate_sha256.is_empty())
    {
        return Err(ProviderError::GuestBridge(
            "a provisioning profile contains no developer signing certificate".to_string(),
        ));
    }
    for profile in &mut profiles {
        let Some((_, get_task_allow, provisions_all)) =
            flags.iter().find(|(uuid, _, _)| uuid == &profile.uuid)
        else {
            return Err(ProviderError::GuestBridge(
                "macOS returned incomplete provisioning profile metadata".to_string(),
            ));
        };
        profile.get_task_allow = *get_task_allow;
        profile.kind = Some(classify_profile(
            *get_task_allow,
            *provisions_all,
            profile.provisioned_device_udids.len(),
        ));
    }

    Ok(profiles)
}

/// What a profile is for. In-house profiles provision every device; App Store profiles name
/// none and cannot be debugged; development profiles can be debugged; ad hoc ones name devices
/// but cannot.
pub fn classify_profile(
    get_task_allow: bool,
    provisions_all_devices: bool,
    device_count: usize,
) -> ProfileKind {
    if provisions_all_devices {
        ProfileKind::Enterprise
    } else if get_task_allow {
        ProfileKind::Development
    } else if device_count == 0 {
        ProfileKind::AppStore
    } else {
        ProfileKind::AdHoc
    }
}

pub(crate) fn profile_matches_project(
    profile: &ProvisioningProfileSummary,
    signing: &SigningProvisioningResult,
    bundle_identifier: &str,
) -> bool {
    valid_profile_uuid(&profile.uuid)
        && profile.team_identifier == signing.development_team
        && profile_allows_bundle(&profile.application_identifier, bundle_identifier)
}

/// The profile an App Store archive signs with: the project's, carrying the distribution
/// certificate, and not a development or ad hoc profile that happens to match too. A record
/// written before kinds were read counts as App Store, which is all a kit could hold then.
pub fn select_app_store_profile(
    signing: &SigningProvisioningResult,
) -> Option<&ProvisioningProfileSummary> {
    let identity = signing.distribution_identity.as_ref()?;
    signing.profiles.iter().find(|profile| {
        matches!(profile.kind, Some(ProfileKind::AppStore) | None)
            && profile_matches_project(profile, signing, &signing.bundle_identifier)
            && profile
                .developer_certificate_sha256
                .iter()
                .any(|fingerprint| fingerprint == &identity.certificate_sha256)
    })
}

/// The profile a device build signs with: a development profile for the identifier the Debug
/// build carries — a project's Debug configuration often has its own, suffixed one — carrying
/// the development certificate and listing the phone.
pub fn select_development_profile<'a>(
    signing: &'a SigningProvisioningResult,
    bundle_identifier: &str,
    udid: &str,
) -> Option<&'a ProvisioningProfileSummary> {
    let identity = signing.development_identity.as_ref()?;
    signing.profiles.iter().find(|profile| {
        profile.kind == Some(ProfileKind::Development)
            && profile_matches_project(profile, signing, bundle_identifier)
            && profile
                .developer_certificate_sha256
                .iter()
                .any(|fingerprint| fingerprint == &identity.certificate_sha256)
            && profile
                .provisioned_device_udids
                .iter()
                .any(|listed| listed.eq_ignore_ascii_case(udid))
    })
}

/// A device UDID as Apple prints it: 40 hex characters on older phones, or 8 hex characters,
/// a hyphen and 16 more on phones since the iPhone XS. Case does not matter.
pub fn valid_device_udid(value: &str) -> bool {
    let bytes = value.as_bytes();

    match bytes.len() {
        40 => bytes.iter().all(u8::is_ascii_hexdigit),
        25 => {
            bytes[8] == b'-'
                && bytes[..8].iter().all(u8::is_ascii_hexdigit)
                && bytes[9..].iter().all(u8::is_ascii_hexdigit)
        }
        _ => false,
    }
}

pub(crate) fn valid_profile_uuid(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(index, character)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                character == '-'
            } else {
                character.is_ascii_hexdigit()
            }
        })
}

pub(crate) fn profile_allows_bundle(application_identifier: &str, bundle_identifier: &str) -> bool {
    let Some((_, profile_bundle)) = application_identifier.split_once('.') else {
        return false;
    };

    profile_bundle == bundle_identifier
        || profile_bundle == "*"
        || profile_bundle
            .strip_suffix('*')
            .is_some_and(|prefix| bundle_identifier.starts_with(prefix))
}

pub(crate) fn install_guest_profiles(
    guest_profiles: &[(String, PathBuf)],
    profiles: &[ProvisioningProfileSummary],
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
    let guest_home = format!("/Users/{username}");
    let directories = [
        format!("{guest_home}/Library/MobileDevice/Provisioning Profiles"),
        format!("{guest_home}/Library/Developer/Xcode/UserData/Provisioning Profiles"),
    ];
    let mut command = format!(
        "set -eu; umask 077; /bin/mkdir -p {} {}",
        shell_single_quote(&directories[0]),
        shell_single_quote(&directories[1]),
    );
    let mut staged_profiles = Vec::new();
    for ((guest_profile, _), profile) in guest_profiles.iter().zip(profiles) {
        for directory in &directories {
            let destination = format!("{directory}/{}.mobileprovision", profile.uuid);
            let pending = format!("{destination}.buildbridge-incoming");
            command.push_str(&format!(
                "; /bin/rm -f {}; /bin/cp {} {}; /bin/chmod 600 {}",
                shell_single_quote(&pending),
                shell_single_quote(guest_profile),
                shell_single_quote(&pending),
                shell_single_quote(&pending),
            ));
            staged_profiles.push((pending, destination));
        }
    }
    for (pending, destination) in staged_profiles {
        command.push_str(&format!(
            "; /bin/mv -f {} {}",
            shell_single_quote(&pending),
            shell_single_quote(&destination),
        ));
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
