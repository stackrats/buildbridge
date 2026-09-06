//! Explicit local access to saved signing material. Ordinary kit summaries never reveal it.

use std::io::{Read, Write};
use std::path::Path;

use super::*;

const MAX_CREDENTIAL_EXPORT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    Secret,
    File,
    Value,
}

/// A recoverable credential, without its secret contents or its source path.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SigningCredential {
    pub id: String,
    pub label: String,
    pub kind: CredentialKind,
    /// Only public identifiers and aliases are included here.
    pub value: Option<String>,
    /// Present when this item can be exported, including a private key held in the vault.
    pub file_name: Option<String>,
    pub available: bool,
}

pub async fn list_signing_credentials(kit_id: String) -> Result<Vec<SigningCredential>, String> {
    let kit = credential_kit(&kit_id).await?;
    Ok(signing_credentials(&kit))
}

pub async fn reveal_signing_credential(
    kit_id: String,
    credential_id: String,
) -> Result<String, String> {
    let id = CredentialId::parse(&credential_id)?;
    let kit = credential_kit(&kit_id).await?;
    credential_text(&kit, id).map(str::to_owned)
}

pub async fn export_signing_credential(
    kit_id: String,
    credential_id: String,
    path: String,
) -> Result<(), String> {
    let id = CredentialId::parse(&credential_id)?;
    let kit = credential_kit(&kit_id).await?;
    tokio::task::spawn_blocking(move || export_kit_credential(&kit, id, Path::new(&path)))
        .await
        .map_err(|_| "The credential could not be exported.".to_string())?
}

async fn credential_kit(kit_id: &str) -> Result<StoredSigningKit, String> {
    read_signing_kits()
        .await
        .map_err(|_| "The operating-system credential vault could not be read.".to_string())?
        .kits
        .into_iter()
        .find(|kit| kit.id == kit_id)
        .ok_or_else(|| "These signing credentials are no longer stored.".to_string())
}

#[derive(Clone, Copy)]
enum CredentialId {
    AppStoreConnectKeyId,
    AppStoreConnectIssuerId,
    AppStoreConnectPrivateKey,
    SigningCertificate,
    SigningCertificatePassword,
    DevelopmentCertificate,
    DevelopmentCertificatePassword,
    GuestKeychainPassword,
    AndroidKeystore,
    AndroidKeystorePassword,
    AndroidKeyAlias,
    AndroidKeyPassword,
    ProvisioningProfile(usize),
}

impl CredentialId {
    fn parse(id: &str) -> Result<Self, String> {
        Ok(match id {
            "app_store_connect_key_id" => Self::AppStoreConnectKeyId,
            "app_store_connect_issuer_id" => Self::AppStoreConnectIssuerId,
            "app_store_connect_private_key" => Self::AppStoreConnectPrivateKey,
            "signing_certificate" => Self::SigningCertificate,
            "signing_certificate_password" => Self::SigningCertificatePassword,
            "development_certificate" => Self::DevelopmentCertificate,
            "development_certificate_password" => Self::DevelopmentCertificatePassword,
            "guest_keychain_password" => Self::GuestKeychainPassword,
            "android_keystore" => Self::AndroidKeystore,
            "android_keystore_password" => Self::AndroidKeystorePassword,
            "android_key_alias" => Self::AndroidKeyAlias,
            "android_key_password" => Self::AndroidKeyPassword,
            _ => {
                let index = id
                    .strip_prefix("provisioning_profile_")
                    .and_then(|index| index.parse::<usize>().ok())
                    .filter(|index| {
                        *index < MAX_PROVISIONING_PROFILES
                            && id == format!("provisioning_profile_{index}")
                    })
                    .ok_or_else(|| "This credential is not available for review.".to_string())?;
                Self::ProvisioningProfile(index)
            }
        })
    }
}

fn signing_credentials(kit: &StoredSigningKit) -> Vec<SigningCredential> {
    let mut credentials = Vec::new();
    for (id, label, value) in [
        (
            "app_store_connect_key_id",
            "App Store Connect key ID",
            &kit.app_store_connect_key_id,
        ),
        (
            "app_store_connect_issuer_id",
            "App Store Connect issuer ID",
            &kit.app_store_connect_issuer_id,
        ),
        (
            "android_key_alias",
            "Android key alias",
            &kit.android_key_alias,
        ),
    ] {
        if let Some(value) = value {
            credentials.push(SigningCredential {
                id: id.to_string(),
                label: label.to_string(),
                kind: CredentialKind::Value,
                value: Some(value.clone()),
                file_name: None,
                available: true,
            });
        }
    }
    for (id, label, value) in [
        (
            "app_store_connect_private_key",
            "App Store Connect private key",
            kit.app_store_connect_private_key.as_ref(),
        ),
        (
            "signing_certificate_password",
            "Distribution certificate password",
            kit.signing_certificate_password.as_ref(),
        ),
        (
            "development_certificate_password",
            "Development certificate password",
            kit.development_certificate_password.as_ref(),
        ),
        (
            "guest_keychain_password",
            "Guest keychain password",
            kit.guest_keychain_password.as_ref(),
        ),
        (
            "android_keystore_password",
            "Android keystore password",
            kit.android_keystore_password.as_ref(),
        ),
        (
            "android_key_password",
            if kit.android_key_password.is_some() {
                "Android key password"
            } else {
                "Android key password (same as keystore password)"
            },
            kit.android_key_password
                .as_ref()
                .or(kit.android_keystore_password.as_ref()),
        ),
    ] {
        if value.is_some() {
            credentials.push(SigningCredential {
                id: id.to_string(),
                label: label.to_string(),
                kind: CredentialKind::Secret,
                value: None,
                file_name: (id == "app_store_connect_private_key").then(|| {
                    kit.app_store_connect_key_id
                        .as_deref()
                        .filter(|id| {
                            id.chars()
                                .all(|character| character.is_ascii_alphanumeric())
                        })
                        .map(|id| format!("AuthKey_{id}.p8"))
                        .unwrap_or_else(|| "AuthKey.p8".to_string())
                }),
                available: true,
            });
        }
    }
    for (id, label, path) in [
        (
            "signing_certificate",
            "Distribution certificate",
            &kit.signing_certificate_path,
        ),
        (
            "development_certificate",
            "Development certificate",
            &kit.development_certificate_path,
        ),
        (
            "android_keystore",
            "Android keystore",
            &kit.android_keystore_path,
        ),
    ] {
        if let Some(path) = path {
            credentials.push(file_credential(id.to_string(), label.to_string(), path));
        }
    }
    for (index, path) in kit.provisioning_profile_paths.iter().enumerate() {
        credentials.push(file_credential(
            format!("provisioning_profile_{index}"),
            format!("Provisioning profile {}", index + 1),
            path,
        ));
    }
    credentials
}

fn file_credential(id: String, label: String, path: &str) -> SigningCredential {
    let path = Path::new(path);
    SigningCredential {
        id,
        label,
        kind: CredentialKind::File,
        value: None,
        file_name: path
            .file_name()
            .map(|name| name.to_string_lossy().to_string()),
        available: path.is_absolute() && path.is_file(),
    }
}

fn credential_text(kit: &StoredSigningKit, id: CredentialId) -> Result<&str, String> {
    let value = match id {
        CredentialId::AppStoreConnectKeyId => &kit.app_store_connect_key_id,
        CredentialId::AppStoreConnectIssuerId => &kit.app_store_connect_issuer_id,
        CredentialId::AppStoreConnectPrivateKey => &kit.app_store_connect_private_key,
        CredentialId::SigningCertificatePassword => &kit.signing_certificate_password,
        CredentialId::DevelopmentCertificatePassword => &kit.development_certificate_password,
        CredentialId::GuestKeychainPassword => &kit.guest_keychain_password,
        CredentialId::AndroidKeystorePassword => &kit.android_keystore_password,
        CredentialId::AndroidKeyAlias => &kit.android_key_alias,
        CredentialId::AndroidKeyPassword => {
            return kit
                .android_key_password
                .as_deref()
                .or(kit.android_keystore_password.as_deref())
                .ok_or_else(|| "This credential is no longer stored.".to_string());
        }
        _ => return Err("Export this credential as a file.".to_string()),
    };
    value
        .as_deref()
        .ok_or_else(|| "This credential is no longer stored.".to_string())
}

fn export_kit_credential(
    kit: &StoredSigningKit,
    id: CredentialId,
    destination: &Path,
) -> Result<(), String> {
    if matches!(id, CredentialId::AppStoreConnectPrivateKey) {
        return export_credential_bytes(destination, credential_text(kit, id)?.as_bytes());
    }
    let path = match id {
        CredentialId::SigningCertificate => kit.signing_certificate_path.as_deref(),
        CredentialId::DevelopmentCertificate => kit.development_certificate_path.as_deref(),
        CredentialId::AndroidKeystore => kit.android_keystore_path.as_deref(),
        CredentialId::ProvisioningProfile(index) => kit
            .provisioning_profile_paths
            .get(index)
            .map(String::as_str),
        _ => return Err("This credential cannot be exported as a file.".to_string()),
    }
    .ok_or_else(|| "This credential is no longer stored.".to_string())?;
    let source = Path::new(path);
    if !source.is_absolute() || !source.is_file() {
        return Err("The saved credential file is missing or cannot be read.".to_string());
    }
    let unreadable = || "The saved credential file is missing or cannot be read.".to_string();
    let too_large = || "The saved credential file exceeds the 64 MiB export limit.".to_string();
    let file = fs::File::open(source).map_err(|_| unreadable())?;
    let metadata = file.metadata().map_err(|_| unreadable())?;
    if !metadata.is_file() {
        return Err(unreadable());
    }
    if metadata.len() > MAX_CREDENTIAL_EXPORT_BYTES {
        return Err(too_large());
    }
    // Bound the actual read as well: the file may grow after metadata was inspected.
    let mut bytes = Vec::new();
    file.take(MAX_CREDENTIAL_EXPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unreadable())?;
    if bytes.len() as u64 > MAX_CREDENTIAL_EXPORT_BYTES {
        return Err(too_large());
    }
    export_credential_bytes(destination, &bytes)
}

/// Exports exact bytes to a new file with private permissions from its first byte.
/// Existing files (including symlinks) are refused, and incomplete writes are removed.
pub(crate) fn export_credential_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    write_credential_export(path, |file| {
        file.write_all(bytes)?;
        file.sync_all()
    })
}

fn write_credential_export(
    path: &Path,
    write: impl FnOnce(&mut fs::File) -> std::io::Result<()>,
) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Choose an absolute destination path for the credential export.".to_string());
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    return Err(
        "Private credential exports are not supported on this operating system.".to_string(),
    );

    let mut file = options.open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            "A file already exists at this destination. Choose a new file name.".to_string()
        } else {
            "The credential export could not be created at this destination.".to_string()
        }
    })?;
    if write(&mut file).is_err() {
        drop(file);
        let _ = fs::remove_file(path);
        return Err("The credential export could not be completed.".to_string());
    }
    Ok(())
}

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;
