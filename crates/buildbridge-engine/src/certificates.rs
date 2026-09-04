//! Identities created at Apple for a kit, and the OpenSSL packaging behind them.

use super::*;

/// Creates an Apple Distribution identity for a kit without a Mac anywhere: the private key is
/// generated on this host, Apple signs a CSR for it through the kit's Team key, and the result is
/// packaged as a `.p12` straight into the kit. Nothing at Apple is revoked or replaced.
pub async fn create_apple_distribution_certificate(
    app: &Engine,
    kit_id: String,
    input: CreateAppleCertificateInput,
) -> Result<CreateAppleCertificateResult, String> {
    create_apple_certificate_command(app, kit_id, input, apple_api::CertificateKind::Distribution)
        .await
}

/// The development counterpart: the identity a Debug build on a registered phone is signed
/// with. It joins the kit next to the distribution one and never replaces it.
pub async fn create_apple_development_certificate(
    app: &Engine,
    kit_id: String,
    input: CreateAppleCertificateInput,
) -> Result<CreateAppleCertificateResult, String> {
    create_apple_certificate_command(app, kit_id, input, apple_api::CertificateKind::Development)
        .await
}

pub(crate) async fn create_apple_certificate_command(
    app: &Engine,
    kit_id: String,
    input: CreateAppleCertificateInput,
    kind: apple_api::CertificateKind,
) -> Result<CreateAppleCertificateResult, String> {
    if !input.confirmed {
        return Err("Confirm the Apple certificate creation before continuing.".to_string());
    }
    let kits = read_signing_kits().await?.kits;
    let mut kit = kits
        .iter()
        .find(|kit| kit.id == kit_id)
        .cloned()
        .ok_or_else(|| "This signing kit is no longer stored.".to_string())?;
    let (certificate, saved_path) = create_apple_certificate_for_kit(app, &mut kit, kind)
        .await
        .map_err(|error| {
            with_other_kit_hint(
                error,
                kind,
                &kits_holding_distribution_identity(&kits, &kit_id),
            )
        })?;

    Ok(CreateAppleCertificateResult {
        certificate,
        saved_path,
        kit: summarize_signing_kit(&kit),
    })
}

/// The other kits on this host that already hold a distribution identity. When Apple refuses a
/// second distribution certificate, the one it counts is usually in one of these.
pub(crate) fn kits_holding_distribution_identity(
    kits: &[StoredSigningKit],
    except_id: &str,
) -> Vec<String> {
    kits.iter()
        .filter(|kit| kit.id != except_id && kit.signing_certificate_path.is_some())
        .map(|kit| kit.name.clone())
        .collect()
}

/// Apple's refusal says to revoke or export; if another kit here already holds the identity
/// Apple is counting, the right move is to attach that kit instead, so the message says so.
pub(crate) fn with_other_kit_hint(
    error: String,
    kind: apple_api::CertificateKind,
    holders: &[String],
) -> String {
    if kind != apple_api::CertificateKind::Distribution
        || holders.is_empty()
        || !error.contains("refused to issue another distribution certificate")
    {
        return error;
    }
    let holders = holders.join(", ");
    let verb = if holders.contains(", ") {
        "already hold"
    } else {
        "already holds"
    };
    format!(
        "{error} On this host, {holders} {verb} a distribution identity for this team; attach that kit to the machine instead of creating a second certificate."
    )
}

/// Creates an identity of one kind for a kit without a Mac anywhere: the private key is
/// generated on this host, Apple signs a CSR for it through the kit's Team key, and the result
/// is packaged as a `.p12` straight into the kit. Nothing at Apple is revoked or replaced.
pub(crate) async fn create_apple_certificate_for_kit(
    app: &Engine,
    kit: &mut StoredSigningKit,
    kind: apple_api::CertificateKind,
) -> Result<(apple_api::AppleCertificateSummary, String), String> {
    let key_id = kit.app_store_connect_key_id.clone().ok_or_else(|| {
        "This kit has no App Store Connect key. Add one to the kit first; creating a certificate needs a Team key with the Admin role."
            .to_string()
    })?;
    let issuer_id = kit
        .app_store_connect_issuer_id
        .clone()
        .ok_or_else(|| "This kit has no App Store Connect Issuer ID.".to_string())?;
    let private_key = kit
        .app_store_connect_private_key
        .clone()
        .ok_or_else(|| "This kit has no App Store Connect .p8 key.".to_string())?;

    let directory = managed_apple_certificates_dir(app)?.join(format!(
        "{}-{}",
        kind.file_stem(),
        machines::now_epoch_seconds()
    ));
    let work = tokio::task::spawn_blocking({
        let directory = directory.clone();
        move || prepare_certificate_request(&directory, kind.common_name())
    })
    .await
    .map_err(|error| error.to_string())??;

    let created =
        match apple_api::create_certificate(&key_id, &issuer_id, &private_key, &work.csr_pem, kind)
            .await
        {
            Ok(created) => created,
            Err(error) => {
                let _ = fs::remove_dir_all(&directory);
                return Err(error);
            }
        };

    let packaged = tokio::task::spawn_blocking({
        let directory = directory.clone();
        let display_name = created.certificate.name.clone();
        let content = created.content.clone();
        move || package_certificate(&directory, &display_name, &content, kind.file_stem())
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result)
    .map_err(|error| {
        let _ = fs::remove_dir_all(&directory);
        format!(
            "Apple issued certificate {}, but BuildBridge could not package it: {error}. Nothing at Apple was revoked; download it from the developer portal, or revoke it there and try again.",
            created.certificate.name
        )
    })?;

    match kind {
        apple_api::CertificateKind::Distribution => {
            kit.signing_certificate_path = Some(packaged.p12_path.clone());
            kit.signing_certificate_password = Some(packaged.password);
        }
        apple_api::CertificateKind::Development => {
            kit.development_certificate_path = Some(packaged.p12_path.clone());
            kit.development_certificate_password = Some(packaged.password);
            kit.development_certificate_serial_number =
                Some(created.certificate.serial_number.clone());
        }
    }
    save_signing_kit_record(kit.clone()).await.map_err(|error| {
        format!(
            "Apple issued certificate {} and it was packaged at {}, but BuildBridge could not update the kit in the OS vault: {error}. Nothing at Apple was revoked.",
            created.certificate.name, packaged.p12_path
        )
    })?;

    Ok((created.certificate, packaged.p12_path))
}

/// Apple's serial number for the kit's development `.p12`: recorded when BuildBridge created
/// it, read out of the file with OpenSSL otherwise.
pub(crate) fn development_certificate_serial(kit: &StoredSigningKit) -> Result<String, String> {
    if let Some(serial) = &kit.development_certificate_serial_number {
        return Ok(serial.clone());
    }
    let path = kit
        .development_certificate_path
        .as_deref()
        .ok_or_else(|| "This kit has no development certificate.".to_string())?;
    let password = kit
        .development_certificate_password
        .as_deref()
        .ok_or_else(|| {
            "Store the development certificate passphrase in the operating-system vault first."
                .to_string()
        })?;
    certificate_serial_from_p12(path, password, "development")
}

/// The serial of the certificate inside a `.p12`, as Apple lists it. The passphrase goes
/// through the environment, never the command line, and OpenSSL 3 is retried in legacy mode for
/// files exported by older tooling.
pub(crate) fn certificate_serial_from_p12(
    path: &str,
    password: &str,
    what: &str,
) -> Result<String, String> {
    let env = Some(("BUILDBRIDGE_P12_PASSWORD", password));
    let pem = openssl(
        &[
            "pkcs12",
            "-in",
            path,
            "-nokeys",
            "-clcerts",
            "-passin",
            "env:BUILDBRIDGE_P12_PASSWORD",
        ],
        None,
        env,
    )
    .or_else(|_| {
        openssl(
            &[
                "pkcs12",
                "-legacy",
                "-in",
                path,
                "-nokeys",
                "-clcerts",
                "-passin",
                "env:BUILDBRIDGE_P12_PASSWORD",
            ],
            None,
            env,
        )
    })?;
    let serial = openssl(&["x509", "-noout", "-serial"], Some(&pem), None)?;
    let serial = String::from_utf8_lossy(&serial);
    let serial = serial.trim();
    let serial = serial
        .strip_prefix("serial=")
        .unwrap_or(serial)
        .trim()
        .to_ascii_uppercase();
    if serial.is_empty()
        || !serial
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(format!(
            "OpenSSL could not read the {what} certificate's serial number."
        ));
    }

    Ok(serial)
}

/// Keeps a profile Apple returned: the managed copy on this host, and its path in the kit.
pub(crate) async fn retain_profile_in_kit(
    app: &Engine,
    kit: &mut StoredSigningKit,
    profile: &apple_api::AppleProvisioningProfileSummary,
    content: &[u8],
) -> Result<(), String> {
    if kit.provisioning_profile_paths.len() >= MAX_PROVISIONING_PROFILES {
        return Err(format!(
            "This kit already holds {MAX_PROVISIONING_PROFILES} profiles. Remove obsolete paths before adding another."
        ));
    }
    let saved_path = save_managed_apple_profile(app, profile, content)?;
    let saved_path = saved_path
        .to_str()
        .ok_or_else(|| "The managed profile path is not valid UTF-8.".to_string())?
        .to_string();
    if !kit
        .provisioning_profile_paths
        .iter()
        .any(|path| path == &saved_path)
    {
        kit.provisioning_profile_paths.push(saved_path);
    }
    save_signing_kit_record(kit.clone()).await
}

/// What the Team key route creates before a kit without stored distribution files can
/// provision: an Apple Distribution certificate packaged into the kit, and an App Store profile
/// for the project's bundle identifier that lists it, downloaded into the kit. Each step persists
/// before the next, so a retry resumes rather than repeats; nothing at Apple is revoked or
/// replaced.
pub(crate) async fn ensure_distribution_set(
    app: &Engine,
    kit: &mut StoredSigningKit,
    bundle_identifier: &str,
    workspace_name: &str,
    report: &(dyn Fn(buildbridge_docker_osx::SigningProvisioningPhase, &str) + Sync),
) -> Result<(), String> {
    use buildbridge_docker_osx::SigningProvisioningPhase as Phase;

    let (key_id, issuer_id, private_key) = match (
        kit.app_store_connect_key_id.clone(),
        kit.app_store_connect_issuer_id.clone(),
        kit.app_store_connect_private_key.clone(),
    ) {
        (Some(key_id), Some(issuer_id), Some(private_key)) => (key_id, issuer_id, private_key),
        _ => {
            return Err(
                "This kit has no App Store Connect key, so BuildBridge cannot create signing material for it. Store a distribution identity with its profile, or add a Team key."
                    .to_string(),
            );
        }
    };

    if kit.signing_certificate_path.is_none() {
        report(
            Phase::CreatingCertificate,
            "Creating an Apple Distribution certificate for a key generated on this host",
        );
        let kits = read_signing_kits().await?.kits;
        let holders = kits_holding_distribution_identity(&kits, &kit.id);
        create_apple_certificate_for_kit(app, kit, apple_api::CertificateKind::Distribution)
            .await
            .map_err(|error| {
                with_other_kit_hint(error, apple_api::CertificateKind::Distribution, &holders)
            })?;
    }
    let path = kit
        .signing_certificate_path
        .clone()
        .ok_or_else(|| "This kit has no distribution certificate.".to_string())?;
    let password = kit.signing_certificate_password.clone().ok_or_else(|| {
        "Store the certificate export password in the operating-system vault first.".to_string()
    })?;
    let serial = tokio::task::spawn_blocking(move || {
        certificate_serial_from_p12(&path, &password, "distribution")
    })
    .await
    .map_err(|error| error.to_string())??;
    let certificate = apple_api::find_certificate_by_serial(
        &key_id,
        &issuer_id,
        &private_key,
        &serial,
        apple_api::CertificateKind::Distribution,
    )
    .await?
    .ok_or_else(|| {
        format!(
            "The kit's distribution certificate (serial {serial}) is not an unexpired Apple Distribution certificate on this team. Store the .p12 of a current certificate in the kit, or store the kit again with only the Team key so BuildBridge creates one."
        )
    })?;

    report(
        Phase::CreatingProfile,
        &format!("Checking the team's App Store profiles for {bundle_identifier}"),
    );
    apple_api::ensure_bundle_id(
        &key_id,
        &issuer_id,
        &private_key,
        bundle_identifier,
        workspace_name,
    )
    .await?;
    let search = apple_api::find_app_store_profile(
        &key_id,
        &issuer_id,
        &private_key,
        bundle_identifier,
        &certificate.id,
    )
    .await?;
    let (profile, content) = match search.matching {
        Some(profile) if kit_holds_profile_uuid(kit, &profile.uuid) => return Ok(()),
        Some(profile) => {
            report(
                Phase::CreatingProfile,
                "Downloading the App Store profile that already lists this certificate",
            );
            apple_api::download_profile(&key_id, &issuer_id, &private_key, &profile.id).await?
        }
        None if !search.other_usable.is_empty() => {
            return Err(format!(
                "An active App Store profile for {bundle_identifier} exists at Apple ({}) but is for a different certificate, and BuildBridge does not replace a live profile it did not create. Store that certificate's .p12 in the kit, or let the profile expire or delete it in the developer portal, then provision again.",
                search.other_usable.join(", ")
            ));
        }
        None => {
            report(
                Phase::CreatingProfile,
                &format!("Creating an App Store profile for {bundle_identifier}"),
            );
            let created = apple_api::create_replacement_profile(
                &key_id,
                &issuer_id,
                &private_key,
                bundle_identifier,
                &certificate.id,
            )
            .await?;
            (created.profile, created.content)
        }
    };

    retain_profile_in_kit(app, kit, &profile, &content).await
}

/// Whether the kit already holds a copy of the profile with this UUID, by managed file name.
pub(crate) fn kit_holds_profile_uuid(kit: &StoredSigningKit, uuid: &str) -> bool {
    kit.provisioning_profile_paths.iter().any(|path| {
        std::path::Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.to_ascii_uppercase()
                    .starts_with(&uuid.to_ascii_uppercase())
            })
    })
}

/// Where identities created here are kept: the `.p12` and Apple's `.cer`, owner-only.
pub(crate) fn managed_apple_certificates_dir(app: &Engine) -> Result<PathBuf, String> {
    Ok(app.config_dir().join("macos-builder").join("certificates"))
}

pub struct CertificateRequestFiles {
    pub(crate) csr_pem: String,
}

pub struct PackagedCertificate {
    pub(crate) p12_path: String,
    pub(crate) password: String,
}

/// Generates the private key and CSR on this host with fixed-argv OpenSSL. The key is written
/// owner-only from OpenSSL's standard output rather than by OpenSSL itself, so it never exists
/// with looser permissions even for an instant.
pub(crate) fn prepare_certificate_request(
    directory: &std::path::Path,
    common_name: &str,
) -> Result<CertificateRequestFiles, String> {
    openssl(&["version"], None, None).map_err(|_| {
        "OpenSSL is not installed on this host. Install the openssl package and try again."
            .to_string()
    })?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create the certificate directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Could not protect the certificate directory: {error}"))?;
    }

    let key_pem = openssl(&certificate_key_args(), None, None)?;
    let key_path = directory.join("key.pem");
    write_restricted_file(&key_path, &key_pem)?;

    let csr_pem = openssl(
        &certificate_csr_args(&key_path.to_string_lossy(), common_name),
        None,
        None,
    )?;
    let csr_pem = String::from_utf8(csr_pem)
        .map_err(|_| "OpenSSL produced an unreadable certificate request.".to_string())?;
    if !apple_api::valid_csr_pem(&csr_pem) {
        return Err("OpenSSL produced an unreadable certificate request.".to_string());
    }

    Ok(CertificateRequestFiles { csr_pem })
}

/// Packages Apple's certificate with the host key as a password-protected `.p12`, then removes
/// the loose key and PEM. The password is generated here and handed to OpenSSL through the
/// environment, never as an argument.
pub(crate) fn package_certificate(
    directory: &std::path::Path,
    display_name: &str,
    certificate_der: &[u8],
    file_stem: &str,
) -> Result<PackagedCertificate, String> {
    let key_path = directory.join("key.pem");
    let cer_path = directory.join("certificate.cer");
    let pem_path = directory.join("certificate.pem");
    let p12_path = directory.join(format!("{file_stem}.p12"));
    write_restricted_file(&cer_path, certificate_der)?;

    let certificate_pem = openssl(
        &["x509", "-inform", "DER", "-in", &cer_path.to_string_lossy()],
        None,
        None,
    )?;
    write_restricted_file(&pem_path, &certificate_pem)?;

    let password = String::from_utf8(openssl(&["rand", "-base64", "24"], None, None)?)
        .map_err(|_| "OpenSSL produced an unreadable password.".to_string())?
        .trim()
        .to_string();
    if password.len() < 24 {
        return Err("OpenSSL produced an unusable password.".to_string());
    }

    let p12 = openssl(
        &certificate_p12_args(
            &key_path.to_string_lossy(),
            &pem_path.to_string_lossy(),
            display_name,
        ),
        None,
        Some(("BUILDBRIDGE_P12_PASSWORD", &password)),
    )?;
    if p12.is_empty() {
        return Err("OpenSSL produced an empty .p12.".to_string());
    }
    write_restricted_file(&p12_path, &p12)?;
    remove_file_if_present(&key_path)?;
    remove_file_if_present(&pem_path)?;

    Ok(PackagedCertificate {
        p12_path: p12_path.to_string_lossy().to_string(),
        password,
    })
}

pub(crate) fn certificate_key_args() -> Vec<String> {
    [
        "genpkey",
        "-algorithm",
        "RSA",
        "-pkeyopt",
        "rsa_keygen_bits:2048",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub(crate) fn certificate_csr_args(key_path: &str, common_name: &str) -> Vec<String> {
    let subject = format!("/CN={common_name}");
    ["req", "-new", "-batch", "-key", key_path, "-subj", &subject]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The algorithms macOS's Security framework can read. OpenSSL 3 writes PBES2 with AES and a
/// SHA-256 MAC by default, and `SecPKCS12Import` on macOS 26 answers that with
/// `errSecPkcs12VerifyFailure` (-25264, "MAC verification failed"), which reads like a wrong
/// password and is not. SHA-1 for the MAC and 3DES for the bags is what Keychain Access itself
/// exports, imports on every macOS, and needs no legacy provider on the host.
pub(crate) const MACOS_PKCS12_ALGORITHMS: [&str; 6] = [
    "-keypbe",
    "PBE-SHA1-3DES",
    "-certpbe",
    "PBE-SHA1-3DES",
    "-macalg",
    "sha1",
];

pub(crate) fn certificate_p12_args(
    key_path: &str,
    certificate_pem: &str,
    display_name: &str,
) -> Vec<String> {
    [
        "pkcs12",
        "-export",
        "-inkey",
        key_path,
        "-in",
        certificate_pem,
        "-name",
        display_name,
        "-passout",
        "env:BUILDBRIDGE_P12_PASSWORD",
    ]
    .into_iter()
    .chain(MACOS_PKCS12_ALGORITHMS)
    .map(str::to_string)
    .collect()
}

/// Whether a `.p12`, as `openssl pkcs12 -info` describes it, was written with algorithms macOS
/// cannot read: a SHA-256 MAC or PBES2 bags.
pub(crate) fn pkcs12_needs_repackaging(info: &str) -> bool {
    info.lines().any(|line| {
        let line = line.trim();
        (line.starts_with("MAC:") && !line.contains("sha1")) || line.contains("PBES2")
    })
}

/// A kit file BuildBridge packaged before it knew what macOS reads is rewritten in place with
/// the same key, certificate and passphrase, so nothing at Apple is touched and the kit keeps
/// its path. The key crosses only a pipe between two OpenSSL processes; the passphrase rides the
/// environment, never an argument. Files macOS can already read — every Mac export — are left
/// exactly as they are.
pub(crate) fn ensure_macos_importable_pkcs12(path: &str, password: &str) -> Result<String, String> {
    let env = Some(("BUILDBRIDGE_P12_PASSWORD", password));
    // `pkcs12 -info` writes its report to stderr, so it is read from there. A file OpenSSL
    // cannot describe at all is left for the guest import to name precisely.
    let info = openssl_report(
        &[
            "pkcs12",
            "-info",
            "-noout",
            "-in",
            path,
            "-passin",
            "env:BUILDBRIDGE_P12_PASSWORD",
        ],
        env,
    )
    .unwrap_or_default();
    if !pkcs12_needs_repackaging(&info) {
        return Ok(path.to_string());
    }
    let pem = openssl(
        &[
            "pkcs12",
            "-in",
            path,
            "-nodes",
            "-passin",
            "env:BUILDBRIDGE_P12_PASSWORD",
        ],
        None,
        env,
    )?;
    // `pkcs12 -export` reads its input twice, once for certificates and once for the key, so
    // a pipe cannot serve it. The unencrypted PEM exists for the length of that one command,
    // owner-only, beside the file it came from in the kit's owner-only directory.
    let pem_path = format!("{path}.repack.pem");
    write_owner_only(&pem_path, &pem)?;
    let display_name = std::path::Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("identity")
        .to_string();
    let repacked = format!("{path}.repacked");
    let args: Vec<String> = [
        "pkcs12",
        "-export",
        "-in",
        pem_path.as_str(),
        "-name",
        display_name.as_str(),
        "-passout",
        "env:BUILDBRIDGE_P12_PASSWORD",
        "-out",
        repacked.as_str(),
    ]
    .into_iter()
    .chain(MACOS_PKCS12_ALGORITHMS)
    .map(str::to_string)
    .collect();
    let exported = openssl(&args, None, env);
    let _ = fs::remove_file(&pem_path);
    exported.map_err(|error| {
        let _ = fs::remove_file(&repacked);
        format!("The identity at {path} could not be repackaged for macOS: {error}")
    })?;
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&repacked, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("The repackaged identity could not be protected: {error}"))?;
    }
    fs::rename(&repacked, path).map_err(|error| {
        let _ = fs::remove_file(&repacked);
        format!("The repackaged identity could not replace {path}: {error}")
    })?;

    Ok(path.to_string())
}

/// Runs OpenSSL for what it says rather than what it outputs: `pkcs12 -info` reports on
/// stderr. Both streams, as text, on success.
pub(crate) fn openssl_report(
    args: &[impl AsRef<str>],
    env: Option<(&str, &str)>,
) -> Result<String, String> {
    let mut command = Command::new("openssl");
    command.args(args.iter().map(AsRef::as_ref));
    command.stdin(std::process::Stdio::null());
    if let Some((name, value)) = env {
        command.env(name, value);
    }
    let output = command
        .output()
        .map_err(|error| format!("Could not run openssl: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));

    Ok(text)
}

/// A file only this user can read, from the first byte: created with the mode, not chmodded
/// after the write.
pub(crate) fn write_owner_only(path: &str, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("Could not create {path}: {error}"))?;
    file.write_all(bytes)
        .map_err(|error| format!("Could not write {path}: {error}"))
}

pub(crate) fn openssl(
    args: &[impl AsRef<str>],
    stdin: Option<&[u8]>,
    env: Option<(&str, &str)>,
) -> Result<Vec<u8>, String> {
    let mut command = Command::new("openssl");
    command.args(args.iter().map(AsRef::as_ref));
    command.stdin(if stdin.is_some() {
        std::process::Stdio::piped()
    } else {
        std::process::Stdio::null()
    });
    if let Some((name, value)) = env {
        command.env(name, value);
    }
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not run openssl: {error}"))?;
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        use std::io::Write;
        pipe.write_all(bytes)
            .map_err(|error| format!("Could not feed openssl: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("Could not finish openssl: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "openssl {} failed: {}",
            args.first().map(AsRef::as_ref).unwrap_or("command"),
            stderr.trim().lines().last().unwrap_or("no output")
        ));
    }

    Ok(output.stdout)
}
