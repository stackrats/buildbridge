use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use ts_rs::TS;

const APP_STORE_CONNECT_BUNDLE_IDS_URL: &str = "https://api.appstoreconnect.apple.com/v1/bundleIds";
const APP_STORE_CONNECT_APPS_URL: &str = "https://api.appstoreconnect.apple.com/v1/apps";
const APP_STORE_CONNECT_CERTIFICATES_URL: &str =
    "https://api.appstoreconnect.apple.com/v1/certificates";
const APP_STORE_CONNECT_PROFILES_URL: &str = "https://api.appstoreconnect.apple.com/v1/profiles";
const APP_STORE_CONNECT_DEVICES_URL: &str = "https://api.appstoreconnect.apple.com/v1/devices";
const APP_STORE_CONNECT_AUDIENCE: &str = "appstoreconnect-v1";
const TOKEN_LIFETIME_SECONDS: u64 = 5 * 60;
const DEVICE_FIELDS: &str = "name,udid,platform,status,deviceClass,model,addedDate";
/// Apple's yearly allowance per device class; a registration counts against it for the
/// membership year even after the device is disabled.
const DEVICE_LIMIT: usize = 100;

/// The two identities a kit can hold. Apple's modern type names cover every platform; the
/// older iOS-specific ones still appear on certificates issued years ago.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CertificateKind {
    Distribution,
    Development,
}

impl CertificateKind {
    pub(crate) fn api_type(self) -> &'static str {
        match self {
            Self::Distribution => "DISTRIBUTION",
            Self::Development => "DEVELOPMENT",
        }
    }

    pub(crate) fn matches(self, certificate: &AppleCertificateSummary) -> bool {
        match self {
            Self::Distribution => matches!(
                certificate.certificate_type.as_str(),
                "DISTRIBUTION" | "IOS_DISTRIBUTION"
            ),
            Self::Development => matches!(
                certificate.certificate_type.as_str(),
                "DEVELOPMENT" | "IOS_DEVELOPMENT"
            ),
        }
    }

    /// The resource name Apple's refusals are phrased around.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Distribution => "distribution certificate",
            Self::Development => "development certificate",
        }
    }

    pub(crate) fn common_name(self) -> &'static str {
        match self {
            Self::Distribution => "BuildBridge Distribution",
            Self::Development => "BuildBridge Development",
        }
    }

    pub(crate) fn file_stem(self) -> &'static str {
        match self {
            Self::Distribution => "distribution",
            Self::Development => "development",
        }
    }
}

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppleDeviceSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) udid: String,
    pub(crate) platform: String,
    pub(crate) status: String,
    pub(crate) device_class: String,
    pub(crate) model: String,
    pub(crate) added_date: String,
}

/// A device that is on the team after `register_device`: created just now, or found already.
#[derive(Debug)]
pub(crate) struct RegisteredAppleDevice {
    pub(crate) device: AppleDeviceSummary,
    pub(crate) already_registered: bool,
}

/// What a development profile search found: the newest usable profile that already lists the
/// phone and carries the certificate, and the devices of the newest usable profile carrying the
/// certificate at all, so a new profile keeps every phone that was already provisioned.
#[derive(Debug)]
pub(crate) struct DevelopmentProfileSearch {
    pub(crate) matching: Option<AppleProvisioningProfileSummary>,
    pub(crate) superseded_device_ids: Vec<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct DevicesResponse {
    data: Vec<DeviceResource>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct DeviceResponse {
    data: DeviceResource,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct DeviceResource {
    id: String,
    #[serde(default)]
    attributes: Option<DeviceAttributes>,
}

#[derive(Debug, Default, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
struct DeviceAttributes {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    udid: Option<String>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    device_class: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    added_date: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct ProfileMembershipsResponse {
    data: Vec<ProfileWithRelationships>,
    #[serde(default)]
    included: Vec<IncludedResource>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct ProfileWithRelationships {
    id: String,
    #[serde(default)]
    relationships: Option<ProfileRelationships>,
}

#[derive(Debug, Default, Deserialize, TS)]
#[ts(export)]
struct ProfileRelationships {
    #[serde(default)]
    devices: Option<RelationshipList>,
    #[serde(default)]
    certificates: Option<RelationshipList>,
}

#[derive(Debug, Default, Deserialize, TS)]
#[ts(export)]
struct RelationshipList {
    #[serde(default)]
    data: Vec<ResourceIdentifier>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct ResourceIdentifier {
    #[serde(rename = "type")]
    kind: String,
    id: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct IncludedResource {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    #[serde(default)]
    attributes: serde_json::Value,
}

/// Which devices and certificates a profile lists, by Apple id and, for devices, by UDID.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ProfileMembership {
    pub(crate) device_ids: Vec<String>,
    pub(crate) device_udids: Vec<String>,
    pub(crate) certificate_ids: Vec<String>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppleTeamVerificationResult {
    key_id: String,
    project_development_team: String,
    bundle_identifier: String,
    app_store_record_found: bool,
    app_store_app_name: Option<String>,
    app_store_app_id: Option<String>,
    bundle_id_found: bool,
    bundle_id_name: Option<String>,
    bundle_id_platform: Option<String>,
    app_id_prefix: Option<String>,
    bundle_lookup_fallback_used: bool,
    developer_resources_accessible: bool,
    developer_resources_issue: Option<String>,
    profiles_accessible: bool,
    profiles_issue: Option<String>,
    profiles: Vec<AppleProvisioningProfileSummary>,
    certificates_accessible: bool,
    certificates_issue: Option<String>,
    certificates: Vec<AppleCertificateSummary>,
    devices_accessible: bool,
    devices_issue: Option<String>,
    devices: Vec<AppleDeviceSummary>,
    #[ts(type = "number")]
    verified_at_epoch_seconds: u64,
}

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppleProvisioningProfileSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) platform: String,
    pub(crate) profile_type: String,
    pub(crate) profile_state: String,
    pub(crate) uuid: String,
    pub(crate) created_date: String,
    pub(crate) expiration_date: String,
    /// Filled for development and ad hoc profiles when the memberships were fetched.
    pub(crate) device_udids: Vec<String>,
    pub(crate) certificate_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppleCertificateSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) display_name: String,
    pub(crate) certificate_type: String,
    pub(crate) serial_number: String,
    pub(crate) platform: String,
    pub(crate) expiration_date: String,
}

/// A certificate Apple has just issued for a CSR this host generated: its metadata and its DER
/// content. The private key never leaves the host, so this plus that key is a complete identity.
#[derive(Debug)]
pub(crate) struct CreatedAppleCertificate {
    pub(crate) certificate: AppleCertificateSummary,
    pub(crate) content: Vec<u8>,
}

pub(crate) struct CreatedAppleProfile {
    pub(crate) profile: AppleProvisioningProfileSummary,
    pub(crate) certificate: AppleCertificateSummary,
    pub(crate) content: Vec<u8>,
}

#[derive(Serialize)]
struct AppleTokenClaims<'a> {
    iss: &'a str,
    iat: u64,
    exp: u64,
    aud: &'static str,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct BundleIdsResponse {
    data: Vec<BundleIdResource>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct BundleIdResource {
    id: String,
    attributes: BundleIdAttributes,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct AppsResponse {
    data: Vec<AppResource>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct AppResource {
    id: String,
    attributes: AppAttributes,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
struct AppAttributes {
    name: String,
    bundle_id: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct ProfilesResponse {
    data: Vec<ProfileResource>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct ProfileResponse {
    data: ProfileResource,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct ProfileResource {
    id: String,
    #[serde(default)]
    attributes: Option<ProfileAttributes>,
}

#[derive(Debug, Default, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
struct ProfileAttributes {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    profile_type: Option<String>,
    #[serde(default)]
    profile_state: Option<String>,
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    created_date: Option<String>,
    #[serde(default)]
    expiration_date: Option<String>,
    #[serde(default)]
    profile_content: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct CertificatesResponse {
    data: Vec<CertificateResource>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct CertificateResource {
    id: String,
    #[serde(default)]
    attributes: Option<CertificateAttributes>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct CertificateResponse {
    data: CertificateResource,
}

#[derive(Debug, Default, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
struct CertificateAttributes {
    #[serde(default)]
    certificate_content: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    certificate_type: Option<String>,
    #[serde(default)]
    serial_number: Option<String>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    expiration_date: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
struct BundleIdAttributes {
    identifier: String,
    name: String,
    platform: String,
    seed_id: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct AppleErrorResponse {
    errors: Vec<AppleApiError>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct AppleApiError {
    title: Option<String>,
    detail: Option<String>,
}

pub(crate) async fn verify_developer_team(
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
    project_development_team: &str,
    bundle_identifier: &str,
) -> Result<AppleTeamVerificationResult, String> {
    let now = unix_timestamp()?;
    let token = create_token(key_id, issuer_id, private_key, now)?;
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("BuildBridge/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("Could not prepare the Apple API connection: {error}"))?;

    let apps_response = client
        .get(APP_STORE_CONNECT_APPS_URL)
        .bearer_auth(&token)
        .query(&[
            ("filter[bundleId]", bundle_identifier),
            ("fields[apps]", "name,bundleId"),
            ("limit", "1"),
        ])
        .send()
        .await
        .map_err(|error| connection_error("App Store app", &error))?;
    let apps_status = apps_response.status();
    let apps_body = apps_response
        .text()
        .await
        .map_err(|error| format!("Could not read Apple's app-record response: {error}"))?;
    if !apps_status.is_success() {
        return Err(apple_error_message(
            apps_status,
            &apps_body,
            "App Store app",
        ));
    }
    let app = parse_app_response(&apps_body, bundle_identifier)?;

    let (
        bundle,
        developer_resources_accessible,
        developer_resources_issue,
        bundle_lookup_fallback_used,
    ) = fetch_bundle_id(&client, &token, bundle_identifier).await?;

    let (profiles, profiles_accessible, profiles_issue) = if let Some(bundle) = &bundle {
        fetch_profiles(&client, &token, &bundle.id).await?
    } else {
        (Vec::new(), false, None)
    };
    let (certificates, certificates_accessible, certificates_issue) =
        fetch_certificates(&client, &token).await?;
    let (devices, devices_accessible, devices_issue) = fetch_devices(&client, &token).await?;
    // Which phones the development and ad hoc profiles list; a key that cannot read them
    // leaves the lists empty rather than failing the whole verification.
    let mut profiles = profiles;
    let membership_ids = profiles
        .iter()
        .filter(|profile| {
            matches!(
                profile.profile_type.as_str(),
                "IOS_APP_DEVELOPMENT" | "IOS_APP_ADHOC"
            )
        })
        .map(|profile| profile.id.clone())
        .collect::<Vec<_>>();
    if !membership_ids.is_empty()
        && let Ok(memberships) = fetch_profile_memberships(&client, &token, &membership_ids).await
    {
        for profile in &mut profiles {
            if let Some(membership) = memberships.get(&profile.id) {
                profile.device_udids = membership.device_udids.clone();
                profile.certificate_ids = membership.certificate_ids.clone();
            }
        }
    }

    Ok(AppleTeamVerificationResult {
        key_id: key_id.to_string(),
        project_development_team: project_development_team.to_string(),
        bundle_identifier: bundle_identifier.to_string(),
        app_store_record_found: app.is_some(),
        app_store_app_name: app.as_ref().map(|item| item.attributes.name.clone()),
        app_store_app_id: app.map(|item| item.id),
        bundle_id_found: bundle.is_some(),
        bundle_id_name: bundle.as_ref().map(|item| item.attributes.name.clone()),
        bundle_id_platform: bundle.as_ref().map(|item| item.attributes.platform.clone()),
        app_id_prefix: bundle.and_then(|item| item.attributes.seed_id),
        bundle_lookup_fallback_used,
        developer_resources_accessible,
        developer_resources_issue,
        profiles_accessible,
        profiles_issue,
        profiles,
        certificates_accessible,
        certificates_issue,
        certificates,
        devices_accessible,
        devices_issue,
        devices,
        verified_at_epoch_seconds: now,
    })
}

fn api_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("BuildBridge/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("Could not prepare the Apple API connection: {error}"))
}

fn device_summary(resource: DeviceResource) -> AppleDeviceSummary {
    let attributes = resource.attributes.unwrap_or_default();
    AppleDeviceSummary {
        id: resource.id,
        name: attributes
            .name
            .unwrap_or_else(|| "Unnamed device".to_string()),
        udid: attributes.udid.unwrap_or_default().to_ascii_uppercase(),
        platform: attributes.platform.unwrap_or_else(|| "UNKNOWN".to_string()),
        status: attributes.status.unwrap_or_else(|| "UNKNOWN".to_string()),
        device_class: attributes
            .device_class
            .unwrap_or_else(|| "UNKNOWN".to_string()),
        model: attributes.model.unwrap_or_default(),
        added_date: attributes.added_date.unwrap_or_default(),
    }
}

async fn fetch_devices(
    client: &Client,
    token: &str,
) -> Result<(Vec<AppleDeviceSummary>, bool, Option<String>), String> {
    let response = client
        .get(APP_STORE_CONNECT_DEVICES_URL)
        .bearer_auth(token)
        .query(&[
            ("filter[platform]", "IOS"),
            ("fields[devices]", DEVICE_FIELDS),
            ("limit", "200"),
            ("sort", "name"),
        ])
        .send()
        .await
        .map_err(|error| connection_error("registered devices", &error))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Could not read Apple's devices response: {error}"))?;
    if status == StatusCode::FORBIDDEN {
        return Ok((
            Vec::new(),
            false,
            Some(apple_error_message(status, &body, "registered devices")),
        ));
    }
    if !status.is_success() {
        return Err(apple_error_message(status, &body, "registered devices"));
    }
    let response: DevicesResponse = serde_json::from_str(&body)
        .map_err(|_| "Apple returned an unreadable devices response.".to_string())?;

    Ok((
        response.data.into_iter().map(device_summary).collect(),
        true,
        None,
    ))
}

/// The exact filter first, then the inventory: Apple's filters have proved unreliable for
/// bundle identifiers, and a UDID's case differs between what phones print and what Apple stores.
async fn fetch_device_by_udid(
    client: &Client,
    token: &str,
    udid: &str,
) -> Result<Option<AppleDeviceSummary>, String> {
    let response = client
        .get(APP_STORE_CONNECT_DEVICES_URL)
        .bearer_auth(token)
        .query(&[
            ("filter[udid]", udid),
            ("fields[devices]", DEVICE_FIELDS),
            ("limit", "1"),
        ])
        .send()
        .await
        .map_err(|error| connection_error("registered device", &error))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Could not read Apple's device response: {error}"))?;
    if !status.is_success() {
        return Err(apple_error_message(status, &body, "registered device"));
    }
    let response: DevicesResponse = serde_json::from_str(&body)
        .map_err(|_| "Apple returned an unreadable device response.".to_string())?;
    if let Some(device) = response.data.into_iter().map(device_summary).next() {
        return Ok(Some(device));
    }
    let (devices, _, _) = fetch_devices(client, token).await?;

    Ok(devices
        .into_iter()
        .find(|device| device.udid.eq_ignore_ascii_case(udid)))
}

fn device_registration_request(name: &str, udid: &str) -> serde_json::Value {
    serde_json::json!({
        "data": {
            "type": "devices",
            "attributes": {
                "name": name,
                "platform": "IOS",
                "udid": udid,
            }
        }
    })
}

pub(crate) fn validate_device_udid(udid: &str) -> Result<(), String> {
    if !buildbridge_docker_osx::valid_device_udid(udid) {
        return Err(
            "The iPhone's UDID must be 40 hexadecimal characters, or 8 and 16 with a hyphen."
                .to_string(),
        );
    }

    Ok(())
}

pub(crate) fn validate_device_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 50 || name.chars().any(char::is_control) {
        return Err("Give the iPhone a name of 1 to 50 characters.".to_string());
    }

    Ok(name.to_string())
}

/// Registers a phone with the team, or finds it if it is already there. Registration counts
/// against Apple's yearly allowance and cannot be undone here, so the caller confirms first.
const APP_STORE_CONNECT_BUNDLE_ID_CAPABILITIES_URL: &str =
    "https://api.appstoreconnect.apple.com/v1/bundleIdCapabilities";

/// A Developer Bundle ID the device step made sure exists, and whether it made it.
#[derive(Debug, Clone)]
pub(crate) struct EnsuredBundleId {
    pub(crate) id: String,
    pub(crate) created: bool,
}

/// A bundle identifier as Apple accepts one: reverse-DNS characters only. Anything else is
/// refused before it becomes a request.
pub(crate) fn validate_bundle_identifier(identifier: &str) -> Result<(), String> {
    let valid = !identifier.is_empty()
        && identifier.len() <= 255
        && identifier
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'));
    if valid {
        Ok(())
    } else {
        Err(format!("{identifier} is not a bundle identifier."))
    }
}

/// Finds the Developer Bundle ID for an exact identifier, or registers it. Registration is a
/// new resource at Apple and revokes nothing; the caller confirms it, and it happens once.
pub(crate) async fn ensure_bundle_id(
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
    identifier: &str,
    name: &str,
) -> Result<EnsuredBundleId, String> {
    validate_bundle_identifier(identifier)?;
    let name = validate_device_name(name)?;
    let now = unix_timestamp()?;
    let token = create_token(key_id, issuer_id, private_key, now)?;
    let client = api_client()?;

    let (bundle, accessible, issue, _) = fetch_bundle_id(&client, &token, identifier).await?;
    if !accessible {
        return Err(issue.unwrap_or_else(|| {
            "The Team key cannot access Developer provisioning resources.".to_string()
        }));
    }
    // The lookup falls back to any bundle ID for an app; only an exact match counts here.
    if let Some(bundle) = bundle.filter(|bundle| bundle.attributes.identifier == identifier) {
        return Ok(EnsuredBundleId {
            id: bundle.id,
            created: false,
        });
    }

    let response = client
        .post(APP_STORE_CONNECT_BUNDLE_IDS_URL)
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "data": {
                "type": "bundleIds",
                "attributes": { "identifier": identifier, "name": name, "platform": "IOS" }
            }
        }))
        .send()
        .await
        .map_err(|error| connection_error("bundle ID registration", &error))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Could not read Apple's bundle ID response: {error}"))?;
    if status == StatusCode::CREATED {
        #[derive(Deserialize)]
        struct Created {
            data: BundleIdResource,
        }
        let created: Created = serde_json::from_str(&body).map_err(|_| {
            "Apple registered the bundle ID but returned an unreadable response.".to_string()
        })?;
        if created.data.attributes.identifier != identifier {
            return Err(
                "Apple registered a bundle ID but returned a different identifier; check Identifiers in the developer portal."
                    .to_string(),
            );
        }
        return Ok(EnsuredBundleId {
            id: created.data.id,
            created: true,
        });
    }
    if matches!(
        status,
        StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY
    ) && let (Some(bundle), _, _, _) = fetch_bundle_id(&client, &token, identifier).await?
        && bundle.attributes.identifier == identifier
    {
        return Ok(EnsuredBundleId {
            id: bundle.id,
            created: false,
        });
    }

    Err(apple_error_message(status, &body, "bundle ID registration"))
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct BundleIdCapabilitiesResponse {
    data: Vec<BundleIdCapabilityResource>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct BundleIdCapabilityResource {
    attributes: BundleIdCapabilityAttributes,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
struct BundleIdCapabilityAttributes {
    capability_type: Option<String>,
    #[serde(default)]
    settings: Option<serde_json::Value>,
}

/// Enables on one bundle ID every capability another has, with the same settings, so a Debug
/// build carries the entitlements the main app does. Capabilities Apple grants every App ID by
/// default answer with a conflict, which is not a failure. Returns what was enabled.
pub(crate) async fn copy_bundle_id_capabilities(
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
    from_bundle_id: &str,
    to_bundle_id: &str,
) -> Result<Vec<String>, String> {
    validate_resource_id(from_bundle_id, "The source bundle ID is not valid.")?;
    validate_resource_id(to_bundle_id, "The target bundle ID is not valid.")?;
    let now = unix_timestamp()?;
    let token = create_token(key_id, issuer_id, private_key, now)?;
    let client = api_client()?;

    let response = client
        .get(format!(
            "{APP_STORE_CONNECT_BUNDLE_IDS_URL}/{from_bundle_id}/bundleIdCapabilities"
        ))
        .bearer_auth(&token)
        .query(&[("limit", "200")])
        .send()
        .await
        .map_err(|error| connection_error("bundle ID capabilities", &error))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Could not read Apple's capabilities response: {error}"))?;
    if !status.is_success() {
        return Err(apple_error_message(status, &body, "bundle ID capabilities"));
    }
    let listed: BundleIdCapabilitiesResponse = serde_json::from_str(&body)
        .map_err(|_| "Apple returned unreadable bundle ID capabilities.".to_string())?;

    let mut enabled = Vec::new();
    for capability in listed.data {
        let Some(capability_type) = capability.attributes.capability_type else {
            continue;
        };
        let mut attributes = serde_json::json!({ "capabilityType": capability_type });
        if let Some(settings) = capability
            .attributes
            .settings
            .filter(|value| !value.is_null())
        {
            attributes["settings"] = settings;
        }
        let response = client
            .post(APP_STORE_CONNECT_BUNDLE_ID_CAPABILITIES_URL)
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "data": {
                    "type": "bundleIdCapabilities",
                    "attributes": attributes,
                    "relationships": {
                        "bundleId": { "data": { "type": "bundleIds", "id": to_bundle_id } }
                    }
                }
            }))
            .send()
            .await
            .map_err(|error| connection_error("bundle ID capability", &error))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        match status {
            StatusCode::CREATED | StatusCode::OK => enabled.push(capability_type),
            StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY => {}
            _ => {
                return Err(apple_error_message(status, &body, "bundle ID capability"));
            }
        }
    }

    Ok(enabled)
}

pub(crate) async fn register_device(
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
    udid: &str,
    name: &str,
) -> Result<RegisteredAppleDevice, String> {
    validate_device_udid(udid)?;
    let name = validate_device_name(name)?;
    let now = unix_timestamp()?;
    let token = create_token(key_id, issuer_id, private_key, now)?;
    let client = api_client()?;

    if let Some(device) = fetch_device_by_udid(&client, &token, udid).await? {
        if device.status.eq_ignore_ascii_case("DISABLED") {
            return Err(
                "This iPhone is registered but disabled in the developer portal. Enable it under Devices, then try again."
                    .to_string(),
            );
        }
        return Ok(RegisteredAppleDevice {
            device,
            already_registered: true,
        });
    }

    let response = client
        .post(APP_STORE_CONNECT_DEVICES_URL)
        .bearer_auth(&token)
        .json(&device_registration_request(&name, udid))
        .send()
        .await
        .map_err(|error| connection_error("device registration", &error))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Could not read Apple's device registration response: {error}"))?;
    if status == StatusCode::CREATED {
        let response: DeviceResponse = serde_json::from_str(&body).map_err(|_| {
            "Apple registered the device but returned an unreadable response.".to_string()
        })?;
        let device = device_summary(response.data);
        if !device.udid.eq_ignore_ascii_case(udid) {
            return Err(
                "Apple registered a device but returned a different UDID; verify the team's devices in the developer portal."
                    .to_string(),
            );
        }
        return Ok(RegisteredAppleDevice {
            device,
            already_registered: false,
        });
    }
    if matches!(
        status,
        StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY
    ) && let Some(device) = fetch_device_by_udid(&client, &token, udid).await?
    {
        // A race or an "already exists" refusal both mean the phone is on the team.
        return Ok(RegisteredAppleDevice {
            device,
            already_registered: true,
        });
    }

    Err(apple_error_message(status, &body, "device registration"))
}

/// The certificate on the team whose serial matches the kit's `.p12`, of the given kind.
pub(crate) async fn find_certificate_by_serial(
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
    serial: &str,
    kind: CertificateKind,
) -> Result<Option<AppleCertificateSummary>, String> {
    let now = unix_timestamp()?;
    let token = create_token(key_id, issuer_id, private_key, now)?;
    let client = api_client()?;
    let (certificates, accessible, issue) = fetch_certificates(&client, &token).await?;
    if !accessible {
        return Err(issue
            .unwrap_or_else(|| "The Team key cannot access the team's certificates.".to_string()));
    }

    Ok(certificates.into_iter().find(|certificate| {
        kind.matches(certificate)
            && certificate_serial_matches(serial, &certificate.serial_number)
            && rfc3339_is_after(&certificate.expiration_date, now)
    }))
}

/// OpenSSL prints `serial=00AB…`; Apple stores the bare hex without leading zeros.
pub(crate) fn certificate_serial_matches(local: &str, apple: &str) -> bool {
    let normalize = |value: &str| {
        let value = value.trim();
        let value = value.strip_prefix("serial=").unwrap_or(value);
        let value = value.trim().trim_start_matches('0');
        value.to_ascii_uppercase()
    };
    let (local, apple) = (normalize(local), normalize(apple));

    !local.is_empty() && local == apple
}

fn profile_is_usable_development(profile: &AppleProvisioningProfileSummary, now: u64) -> bool {
    profile.profile_type == "IOS_APP_DEVELOPMENT"
        && profile.profile_state == "ACTIVE"
        && rfc3339_is_after(&profile.expiration_date, now)
}

/// The devices and certificates each profile lists, in one call per twenty profiles.
async fn fetch_profile_memberships(
    client: &Client,
    token: &str,
    profile_ids: &[String],
) -> Result<std::collections::HashMap<String, ProfileMembership>, String> {
    let mut memberships = std::collections::HashMap::new();
    for chunk in profile_ids.chunks(20) {
        for id in chunk {
            validate_profile_id(id)?;
        }
        let response = client
            .get(APP_STORE_CONNECT_PROFILES_URL)
            .bearer_auth(token)
            .query(&[
                ("filter[id]", chunk.join(",").as_str()),
                ("include", "devices,certificates"),
                ("fields[profiles]", "name"),
                ("fields[devices]", "udid,status"),
                ("fields[certificates]", "certificateType"),
                ("limit", "200"),
            ])
            .send()
            .await
            .map_err(|error| connection_error("profile devices", &error))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| format!("Could not read Apple's profile devices response: {error}"))?;
        if !status.is_success() {
            return Err(apple_error_message(status, &body, "profile devices"));
        }
        memberships.extend(parse_profile_memberships(&body)?);
    }

    Ok(memberships)
}

pub(crate) fn parse_profile_memberships(
    body: &str,
) -> Result<std::collections::HashMap<String, ProfileMembership>, String> {
    let response: ProfileMembershipsResponse = serde_json::from_str(body)
        .map_err(|_| "Apple returned an unreadable profile devices response.".to_string())?;
    let udids: std::collections::HashMap<&str, String> = response
        .included
        .iter()
        .filter(|item| item.kind == "devices")
        .filter_map(|item| {
            item.attributes
                .get("udid")
                .and_then(serde_json::Value::as_str)
                .map(|udid| (item.id.as_str(), udid.to_ascii_uppercase()))
        })
        .collect();
    let mut memberships = std::collections::HashMap::new();
    for profile in response.data {
        let relationships = profile.relationships.unwrap_or_default();
        let device_ids = relationships
            .devices
            .map(|list| {
                list.data
                    .into_iter()
                    .filter(|item| item.kind == "devices")
                    .map(|item| item.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let device_udids = device_ids
            .iter()
            .filter_map(|id| udids.get(id.as_str()).cloned())
            .collect();
        let certificate_ids = relationships
            .certificates
            .map(|list| {
                list.data
                    .into_iter()
                    .filter(|item| item.kind == "certificates")
                    .map(|item| item.id)
                    .collect()
            })
            .unwrap_or_default();
        memberships.insert(
            profile.id,
            ProfileMembership {
                device_ids,
                device_udids,
                certificate_ids,
            },
        );
    }

    Ok(memberships)
}

/// Looks for a usable development profile for the bundle that carries the kit's development
/// certificate and lists the phone, and remembers the devices of the newest such profile
/// without the phone so a replacement keeps them.
pub(crate) async fn find_development_profile(
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
    bundle_identifier: &str,
    certificate_id: &str,
    udid: &str,
) -> Result<DevelopmentProfileSearch, String> {
    validate_certificate_id(certificate_id)?;
    validate_device_udid(udid)?;
    let now = unix_timestamp()?;
    let token = create_token(key_id, issuer_id, private_key, now)?;
    let client = api_client()?;

    let (bundle, accessible, issue, _) =
        fetch_bundle_id(&client, &token, bundle_identifier).await?;
    if !accessible {
        return Err(issue.unwrap_or_else(|| {
            "The Team key cannot access Developer provisioning resources.".to_string()
        }));
    }
    let bundle = bundle.ok_or_else(|| {
        format!(
            "Apple did not return the existing Developer Bundle ID for {bundle_identifier}. Verify the team before preparing device signing."
        )
    })?;
    let (profiles, profiles_accessible, profiles_issue) =
        fetch_profiles(&client, &token, &bundle.id).await?;
    if !profiles_accessible {
        return Err(profiles_issue.unwrap_or_else(|| {
            "The Team key cannot inspect the existing provisioning profiles.".to_string()
        }));
    }
    let candidates = profiles
        .into_iter()
        .filter(|profile| profile_is_usable_development(profile, now))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(DevelopmentProfileSearch {
            matching: None,
            superseded_device_ids: Vec::new(),
        });
    }
    let ids = candidates
        .iter()
        .map(|profile| profile.id.clone())
        .collect::<Vec<_>>();
    let memberships = fetch_profile_memberships(&client, &token, &ids).await?;
    let with_certificate = candidates
        .into_iter()
        .filter_map(|mut profile| {
            let membership = memberships.get(&profile.id)?.clone();
            membership
                .certificate_ids
                .iter()
                .any(|id| id == certificate_id)
                .then(|| {
                    profile.device_udids = membership.device_udids.clone();
                    profile.certificate_ids = membership.certificate_ids.clone();
                    (profile, membership)
                })
        })
        .collect::<Vec<_>>();
    // `fetch_profiles` sorts newest expiry first, so the first hit is the newest.
    let matching = with_certificate
        .iter()
        .find(|(profile, _)| {
            profile
                .device_udids
                .iter()
                .any(|listed| listed.eq_ignore_ascii_case(udid))
        })
        .map(|(profile, _)| profile.clone());
    let superseded_device_ids = with_certificate
        .first()
        .map(|(_, membership)| membership.device_ids.clone())
        .unwrap_or_default();

    Ok(DevelopmentProfileSearch {
        matching,
        superseded_device_ids,
    })
}

/// The team's usable App Store profiles for a bundle, split by whether they list one
/// certificate: `matching` is what provisioning downloads; `other_usable` names the live
/// profiles for other certificates, which are why a new one cannot simply be created (Apple keeps
/// one active App Store profile per bundle, and BuildBridge never replaces a live profile it did
/// not make).
#[derive(Debug, Default)]
pub(crate) struct AppStoreProfileSearch {
    pub(crate) matching: Option<AppleProvisioningProfileSummary>,
    pub(crate) other_usable: Vec<String>,
}

pub(crate) async fn find_app_store_profile(
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
    bundle_identifier: &str,
    certificate_id: &str,
) -> Result<AppStoreProfileSearch, String> {
    validate_certificate_id(certificate_id)?;
    let now = unix_timestamp()?;
    let token = create_token(key_id, issuer_id, private_key, now)?;
    let client = api_client()?;

    let (bundle, accessible, issue, _) =
        fetch_bundle_id(&client, &token, bundle_identifier).await?;
    if !accessible {
        return Err(issue.unwrap_or_else(|| {
            "The Team key cannot access Developer provisioning resources.".to_string()
        }));
    }
    let Some(bundle) = bundle else {
        return Ok(AppStoreProfileSearch::default());
    };
    let (profiles, profiles_accessible, profiles_issue) =
        fetch_profiles(&client, &token, &bundle.id).await?;
    if !profiles_accessible {
        return Err(profiles_issue.unwrap_or_else(|| {
            "The Team key cannot inspect the existing provisioning profiles.".to_string()
        }));
    }
    let candidates = profiles
        .into_iter()
        .filter(|profile| profile_is_usable_app_store(profile, now))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(AppStoreProfileSearch::default());
    }
    let ids = candidates
        .iter()
        .map(|profile| profile.id.clone())
        .collect::<Vec<_>>();
    let memberships = fetch_profile_memberships(&client, &token, &ids).await?;
    let mut search = AppStoreProfileSearch::default();
    // `fetch_profiles` sorts newest expiry first, so the first hit is the newest.
    for mut profile in candidates {
        let listed = memberships
            .get(&profile.id)
            .map(|membership| membership.certificate_ids.clone())
            .unwrap_or_default();
        if search.matching.is_none() && listed.iter().any(|id| id == certificate_id) {
            profile.certificate_ids = listed;
            search.matching = Some(profile);
        } else {
            search.other_usable.push(profile.name);
        }
    }

    Ok(search)
}

/// Creates a development profile for the bundle listing the given devices. Unlike the App
/// Store replacement it never refuses because a usable profile exists: usable here depends on
/// which phones a profile lists, which the caller has already checked.
pub(crate) async fn create_development_profile(
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
    bundle_identifier: &str,
    certificate_id: &str,
    device_ids: &[String],
) -> Result<CreatedAppleProfile, String> {
    validate_certificate_id(certificate_id)?;
    if device_ids.is_empty() || device_ids.len() > DEVICE_LIMIT {
        return Err("A development profile needs between one and one hundred devices.".to_string());
    }
    for id in device_ids {
        validate_resource_id(id, "Choose a valid registered device.")?;
    }
    let now = unix_timestamp()?;
    let token = create_token(key_id, issuer_id, private_key, now)?;
    let client = api_client()?;

    let (bundle, accessible, issue, _) =
        fetch_bundle_id(&client, &token, bundle_identifier).await?;
    if !accessible {
        return Err(issue.unwrap_or_else(|| {
            "The Team key cannot access Developer provisioning resources.".to_string()
        }));
    }
    let bundle = bundle.ok_or_else(|| {
        format!(
            "Apple did not return the existing Developer Bundle ID for {bundle_identifier}. Verify the team before creating a profile."
        )
    })?;
    let (certificates, certificates_accessible, certificates_issue) =
        fetch_certificates(&client, &token).await?;
    if !certificates_accessible {
        return Err(certificates_issue.unwrap_or_else(|| {
            "The Team key cannot inspect the team's certificates.".to_string()
        }));
    }
    let certificate = certificates
        .into_iter()
        .find(|certificate| certificate.id == certificate_id)
        .ok_or_else(|| {
            "The development certificate is no longer on the Apple team. Create one from the kit."
                .to_string()
        })?;
    if !CertificateKind::Development.matches(&certificate)
        || !rfc3339_is_after(&certificate.expiration_date, now)
    {
        return Err(
            "The kit's development certificate is not an unexpired Apple Development certificate; create a new one from the kit."
                .to_string(),
        );
    }

    let profile_name = format!("BuildBridge Development {now}");
    let request =
        development_profile_request(&profile_name, &bundle.id, &certificate.id, device_ids);
    let response = client
        .post(APP_STORE_CONNECT_PROFILES_URL)
        .bearer_auth(&token)
        .json(&request)
        .send()
        .await
        .map_err(|error| connection_error("development provisioning profile", &error))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Could not read Apple's created-profile response: {error}"))?;
    if status != StatusCode::CREATED {
        return Err(apple_error_message(
            status,
            &body,
            "development provisioning profile",
        ));
    }
    let (profile, content) = finish_created_profile(
        &client,
        &token,
        &body,
        &profile_name,
        "IOS_APP_DEVELOPMENT",
        now,
    )
    .await?;

    Ok(CreatedAppleProfile {
        profile,
        certificate,
        content,
    })
}

/// The part of profile creation after Apple says 201: fetch what the create response left out,
/// decode the content, and refuse a profile of the wrong type or one that is not active.
async fn finish_created_profile(
    client: &Client,
    token: &str,
    body: &str,
    profile_name: &str,
    expected_type: &str,
    now: u64,
) -> Result<(AppleProvisioningProfileSummary, Vec<u8>), String> {
    let response: ProfileResponse = serde_json::from_str(body).map_err(|_| {
        "Apple created the profile but returned an unreadable response.".to_string()
    })?;
    let mut profile_resource = response.data;
    let response_is_incomplete = match profile_resource.attributes.as_ref() {
        Some(attributes) => {
            attributes.profile_content.is_none()
                || attributes.profile_type.is_none()
                || attributes.profile_state.is_none()
                || attributes.expiration_date.is_none()
                || attributes.uuid.is_none()
        }
        None => true,
    };
    if response_is_incomplete {
        profile_resource = fetch_created_profile(client, token, &profile_resource.id)
            .await
            .map_err(|error| {
                format!(
                    "Apple created profile {profile_name}, but BuildBridge could not download it: {error}. It was not revoked; verify again before retrying."
                )
            })?;
    }
    let profile_id = profile_resource.id;
    let mut attributes = profile_resource.attributes.unwrap_or_default();
    let encoded_content = attributes.profile_content.take().ok_or_else(|| {
        "Apple created the profile but did not return its downloadable content.".to_string()
    })?;
    let content = decode_profile_content(&encoded_content)?;
    let profile = profile_summary(profile_id, attributes);
    if profile.profile_type != expected_type {
        return Err(
            "Apple created a profile with an unexpected type; it was not retained locally."
                .to_string(),
        );
    }
    if profile.profile_state != "ACTIVE" || !rfc3339_is_after(&profile.expiration_date, now) {
        return Err(
            "Apple created the profile, but it is not active with a future expiry. It was not retained locally or revoked; verify the Apple inventory before retrying."
                .to_string(),
        );
    }

    Ok((profile, content))
}

fn validate_resource_id(value: &str, message: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err(message.to_string());
    }

    Ok(())
}

/// The typed profile create request. App Store profiles name no devices; development ones
/// list exactly the registered devices they are for.
fn profile_request(
    profile_name: &str,
    profile_type: &str,
    bundle_id: &str,
    certificate_id: &str,
    device_ids: Option<&[String]>,
) -> serde_json::Value {
    let mut relationships = serde_json::json!({
        "bundleId": {
            "data": { "type": "bundleIds", "id": bundle_id }
        },
        "certificates": {
            "data": [{ "type": "certificates", "id": certificate_id }]
        }
    });
    if let Some(device_ids) = device_ids {
        relationships["devices"] = serde_json::json!({
            "data": device_ids
                .iter()
                .map(|id| serde_json::json!({ "type": "devices", "id": id }))
                .collect::<Vec<_>>()
        });
    }

    serde_json::json!({
        "data": {
            "type": "profiles",
            "attributes": {
                "name": profile_name,
                "profileType": profile_type
            },
            "relationships": relationships
        }
    })
}

fn development_profile_request(
    profile_name: &str,
    bundle_id: &str,
    certificate_id: &str,
    device_ids: &[String],
) -> serde_json::Value {
    profile_request(
        profile_name,
        "IOS_APP_DEVELOPMENT",
        bundle_id,
        certificate_id,
        Some(device_ids),
    )
}

/// Asks Apple to issue a certificate of one kind for a CSR whose private key was generated on
/// this host, so no Mac is involved. Nothing at Apple is revoked or replaced: if the team is at
/// its limit, Apple refuses and that refusal is shown as is.
pub(crate) async fn create_certificate(
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
    csr_pem: &str,
    kind: CertificateKind,
) -> Result<CreatedAppleCertificate, String> {
    if !valid_csr_pem(csr_pem) {
        return Err("BuildBridge generated an unreadable certificate signing request.".to_string());
    }
    let now = unix_timestamp()?;
    let token = create_token(key_id, issuer_id, private_key, now)?;
    let client = api_client()?;

    // A key that cannot read certificates cannot create them either; say so before trying.
    let (_, accessible, issue) = fetch_certificates(&client, &token).await?;
    if !accessible {
        return Err(issue
            .unwrap_or_else(|| "The Team key cannot access the team's certificates.".to_string()));
    }

    let response = client
        .post(APP_STORE_CONNECT_CERTIFICATES_URL)
        .bearer_auth(&token)
        .json(&certificate_request_for(csr_pem, kind))
        .send()
        .await
        .map_err(|error| connection_error(kind.label(), &error))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Could not read Apple's created-certificate response: {error}"))?;
    if status != StatusCode::CREATED {
        return Err(apple_error_message(status, &body, kind.label()));
    }

    parse_created_certificate_for(&body, kind)
}

async fn fetch_bundle_id(
    client: &Client,
    token: &str,
    bundle_identifier: &str,
) -> Result<(Option<BundleIdResource>, bool, Option<String>, bool), String> {
    let response = client
        .get(APP_STORE_CONNECT_BUNDLE_IDS_URL)
        .bearer_auth(token)
        .query(&[
            ("filter[identifier]", bundle_identifier),
            ("fields[bundleIds]", "name,identifier,platform,seedId"),
            ("limit", "1"),
        ])
        .send()
        .await
        .map_err(|error| connection_error("Developer bundle ID", &error))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Could not read Apple's bundle-ID response: {error}"))?;
    let (mut bundle, mut accessible, mut issue) = if status.is_success() {
        (
            parse_bundle_id_response(&body, bundle_identifier)?,
            true,
            None,
        )
    } else if status == StatusCode::FORBIDDEN {
        (
            None,
            false,
            Some(apple_error_message(
                status,
                &body,
                "Developer provisioning resources",
            )),
        )
    } else {
        return Err(apple_error_message(status, &body, "Developer bundle ID"));
    };

    let mut fallback_used = false;
    if accessible && bundle.is_none() {
        fallback_used = true;
        let inventory_response = client
            .get(APP_STORE_CONNECT_BUNDLE_IDS_URL)
            .bearer_auth(token)
            .query(&[
                ("fields[bundleIds]", "name,identifier,platform,seedId"),
                ("limit", "200"),
                ("sort", "identifier"),
            ])
            .send()
            .await
            .map_err(|error| connection_error("Developer bundle-ID inventory", &error))?;
        let inventory_status = inventory_response.status();
        let inventory_body = inventory_response.text().await.map_err(|error| {
            format!("Could not read Apple's bundle-ID inventory response: {error}")
        })?;

        if inventory_status.is_success() {
            bundle = parse_bundle_id_response(&inventory_body, bundle_identifier)?;
        } else if inventory_status == StatusCode::FORBIDDEN {
            accessible = false;
            issue = Some(apple_error_message(
                inventory_status,
                &inventory_body,
                "Developer provisioning resources",
            ));
        } else {
            return Err(apple_error_message(
                inventory_status,
                &inventory_body,
                "Developer bundle-ID inventory",
            ));
        }
    }

    Ok((bundle, accessible, issue, fallback_used))
}

async fn fetch_profiles(
    client: &Client,
    token: &str,
    bundle_resource_id: &str,
) -> Result<(Vec<AppleProvisioningProfileSummary>, bool, Option<String>), String> {
    let mut profiles_url = reqwest::Url::parse(APP_STORE_CONNECT_BUNDLE_IDS_URL)
        .map_err(|error| format!("Could not prepare Apple's profiles URL: {error}"))?;
    profiles_url
        .path_segments_mut()
        .map_err(|_| "Could not prepare Apple's profiles URL.".to_string())?
        .push(bundle_resource_id)
        .push("profiles");

    let response = client
        .get(profiles_url)
        .bearer_auth(token)
        .query(&[
            (
                "fields[profiles]",
                "name,platform,profileType,profileState,uuid,createdDate,expirationDate",
            ),
            ("limit", "200"),
        ])
        .send()
        .await
        .map_err(|error| connection_error("provisioning profiles", &error))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Could not read Apple's profiles response: {error}"))?;

    if status == StatusCode::FORBIDDEN {
        return Ok((
            Vec::new(),
            false,
            Some(apple_error_message(status, &body, "provisioning profiles")),
        ));
    }
    if !status.is_success() {
        return Err(apple_error_message(status, &body, "provisioning profiles"));
    }

    let mut profiles = parse_profiles_response(&body)?;
    profiles.sort_by(|left, right| right.expiration_date.cmp(&left.expiration_date));

    Ok((profiles, true, None))
}

async fn fetch_certificates(
    client: &Client,
    token: &str,
) -> Result<(Vec<AppleCertificateSummary>, bool, Option<String>), String> {
    let response = client
        .get(APP_STORE_CONNECT_CERTIFICATES_URL)
        .bearer_auth(token)
        .query(&[
            (
                "fields[certificates]",
                "name,certificateType,displayName,serialNumber,platform,expirationDate",
            ),
            ("limit", "200"),
            ("sort", "-id"),
        ])
        .send()
        .await
        .map_err(|error| connection_error("distribution certificates", &error))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Could not read Apple's certificates response: {error}"))?;

    if status == StatusCode::FORBIDDEN {
        return Ok((
            Vec::new(),
            false,
            Some(apple_error_message(
                status,
                &body,
                "distribution certificates",
            )),
        ));
    }
    if !status.is_success() {
        return Err(apple_error_message(
            status,
            &body,
            "distribution certificates",
        ));
    }

    let mut certificates = parse_certificates_response(&body)?;
    certificates.sort_by(|left, right| {
        is_distribution_certificate(right)
            .cmp(&is_distribution_certificate(left))
            .then_with(|| right.expiration_date.cmp(&left.expiration_date))
    });

    Ok((certificates, true, None))
}

pub(crate) async fn create_replacement_profile(
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
    bundle_identifier: &str,
    certificate_id: &str,
) -> Result<CreatedAppleProfile, String> {
    validate_certificate_id(certificate_id)?;

    let now = unix_timestamp()?;
    let token = create_token(key_id, issuer_id, private_key, now)?;
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("BuildBridge/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("Could not prepare the Apple API connection: {error}"))?;

    let (bundle, accessible, issue, _) =
        fetch_bundle_id(&client, &token, bundle_identifier).await?;
    if !accessible {
        return Err(issue.unwrap_or_else(|| {
            "The Team key cannot access Developer provisioning resources.".to_string()
        }));
    }
    let bundle = bundle.ok_or_else(|| {
        format!(
            "Apple did not return the existing Developer Bundle ID for {bundle_identifier}. Verify the team before creating a profile."
        )
    })?;

    let (certificates, certificates_accessible, certificates_issue) =
        fetch_certificates(&client, &token).await?;
    if !certificates_accessible {
        return Err(certificates_issue.unwrap_or_else(|| {
            "The Team key cannot inspect distribution certificates.".to_string()
        }));
    }
    let certificate = certificates
        .into_iter()
        .find(|certificate| certificate.id == certificate_id)
        .ok_or_else(|| {
            "The selected certificate is no longer available. Verify the developer team again."
                .to_string()
        })?;
    if !certificate_is_usable(&certificate, now) {
        return Err(
            "The selected certificate is not an unexpired Apple Distribution certificate. Verify again and choose another certificate."
                .to_string(),
        );
    }

    let (profiles, profiles_accessible, profiles_issue) =
        fetch_profiles(&client, &token, &bundle.id).await?;
    if !profiles_accessible {
        return Err(profiles_issue.unwrap_or_else(|| {
            "The Team key cannot inspect the existing provisioning profiles.".to_string()
        }));
    }
    if profiles
        .iter()
        .any(|profile| profile_is_usable_app_store(profile, now))
    {
        return Err(
            "An active iOS App Store profile now exists. Verify again before creating another one."
                .to_string(),
        );
    }

    let profile_name = format!("BuildBridge App Store {now}");
    let request = replacement_profile_request(&profile_name, &bundle.id, &certificate.id);
    let response = client
        .post(APP_STORE_CONNECT_PROFILES_URL)
        .bearer_auth(&token)
        .json(&request)
        .send()
        .await
        .map_err(|error| connection_error("replacement provisioning profile", &error))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Could not read Apple's created-profile response: {error}"))?;
    if status != StatusCode::CREATED {
        return Err(apple_error_message(
            status,
            &body,
            "replacement provisioning profile",
        ));
    }

    let response: ProfileResponse = serde_json::from_str(&body).map_err(|_| {
        "Apple created the profile but returned an unreadable response.".to_string()
    })?;
    let mut profile_resource = response.data;
    let response_is_incomplete = match profile_resource.attributes.as_ref() {
        Some(attributes) => {
            attributes.profile_content.is_none()
                || attributes.profile_type.is_none()
                || attributes.profile_state.is_none()
                || attributes.expiration_date.is_none()
                || attributes.uuid.is_none()
        }
        None => true,
    };
    if response_is_incomplete {
        profile_resource = fetch_created_profile(&client, &token, &profile_resource.id)
            .await
            .map_err(|error| {
                format!(
                    "Apple created profile {profile_name}, but BuildBridge could not download it: {error}. It was not revoked; verify again before retrying."
                )
            })?;
    }
    let profile_id = profile_resource.id;
    let mut attributes = profile_resource.attributes.unwrap_or_default();
    let encoded_content = attributes.profile_content.take().ok_or_else(|| {
        "Apple created the profile but did not return its downloadable content.".to_string()
    })?;
    let content = decode_profile_content(&encoded_content)?;

    let profile = profile_summary(profile_id, attributes);
    if profile.profile_type != "IOS_APP_STORE" {
        return Err(
            "Apple created a profile with an unexpected distribution type; it was not retained locally."
                .to_string(),
        );
    }
    if !profile_is_usable_app_store(&profile, now) {
        return Err(
            "Apple created the replacement profile, but it is not active with a future expiry. It was not retained locally or revoked; verify the Apple inventory before retrying."
                .to_string(),
        );
    }

    Ok(CreatedAppleProfile {
        profile,
        certificate,
        content,
    })
}

/// The typed create request: one certificate type, one CSR, nothing else Apple could act on.
fn certificate_request_for(csr_pem: &str, kind: CertificateKind) -> serde_json::Value {
    serde_json::json!({
        "data": {
            "type": "certificates",
            "attributes": {
                "certificateType": kind.api_type(),
                "csrContent": csr_pem,
            }
        }
    })
}

fn parse_created_certificate_for(
    body: &str,
    kind: CertificateKind,
) -> Result<CreatedAppleCertificate, String> {
    let response: CertificateResponse = serde_json::from_str(body).map_err(|_| {
        "Apple created the certificate but returned an unreadable response.".to_string()
    })?;
    let mut resource = response.data;
    let encoded = resource
        .attributes
        .as_mut()
        .and_then(|attributes| attributes.certificate_content.take())
        .ok_or_else(|| {
            "Apple created the certificate but did not return its content. Nothing was revoked; it can be downloaded from the developer portal."
                .to_string()
        })?;
    let content = decode_certificate_content(&encoded)?;
    let certificate = certificate_summary(resource);
    if !kind.matches(&certificate) {
        return Err(format!(
            "Apple returned a {} certificate instead of a {}; it was not retained.",
            certificate.certificate_type,
            kind.label()
        ));
    }

    Ok(CreatedAppleCertificate {
        certificate,
        content,
    })
}

fn decode_certificate_content(encoded: &str) -> Result<Vec<u8>, String> {
    if encoded.is_empty() || encoded.len() > 64 * 1024 {
        return Err("Apple returned certificate content with an unsafe size.".to_string());
    }
    let content = BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| "Apple returned invalid encoded certificate content.".to_string())?;
    if content.is_empty() || content.len() > 32 * 1024 {
        return Err("Apple returned certificate content with an unsafe size.".to_string());
    }

    Ok(content)
}

/// A PEM certificate signing request as OpenSSL writes it, with nothing else in it.
pub(crate) fn valid_csr_pem(csr: &str) -> bool {
    let trimmed = csr.trim();
    trimmed.starts_with("-----BEGIN CERTIFICATE REQUEST-----")
        && trimmed.ends_with("-----END CERTIFICATE REQUEST-----")
        && trimmed.len() < 16 * 1024
        && trimmed.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | ' ' | '\n' | '\r')
        })
}

/// Downloads one existing profile by its opaque Apple id, including its content.
///
/// The same request serves a freshly created profile and one that already existed: a kit that
/// lost its local copy can take it back from Apple rather than hunting for the file.
pub(crate) async fn download_profile(
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
    profile_id: &str,
) -> Result<(AppleProvisioningProfileSummary, Vec<u8>), String> {
    validate_profile_id(profile_id)?;

    let now = unix_timestamp()?;
    let token = create_token(key_id, issuer_id, private_key, now)?;
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("BuildBridge/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("Could not prepare the Apple API connection: {error}"))?;
    let resource = fetch_created_profile(&client, &token, profile_id).await?;
    let mut attributes = resource.attributes.ok_or_else(|| {
        "Apple returned the profile without any metadata. Verify again and retry.".to_string()
    })?;
    let encoded = attributes.profile_content.take().ok_or_else(|| {
        "Apple did not return the profile's content. Verify again and retry.".to_string()
    })?;
    let content = decode_profile_content(&encoded)?;
    let summary = profile_summary(resource.id, attributes);

    Ok((summary, content))
}

async fn fetch_created_profile(
    client: &Client,
    token: &str,
    profile_id: &str,
) -> Result<ProfileResource, String> {
    let mut profile_url = reqwest::Url::parse(APP_STORE_CONNECT_PROFILES_URL)
        .map_err(|error| format!("Could not prepare Apple's profile URL: {error}"))?;
    profile_url
        .path_segments_mut()
        .map_err(|_| "Could not prepare Apple's profile URL.".to_string())?
        .push(profile_id);
    let response = client
        .get(profile_url)
        .bearer_auth(token)
        .query(&[(
            "fields[profiles]",
            "name,platform,profileType,profileState,profileContent,uuid,createdDate,expirationDate",
        )])
        .send()
        .await
        .map_err(|error| connection_error("created provisioning profile", &error))?;
    let status = response.status();
    let body = response.text().await.map_err(|error| {
        format!("Could not read Apple's created-profile download response: {error}")
    })?;
    if !status.is_success() {
        return Err(apple_error_message(
            status,
            &body,
            "created provisioning profile",
        ));
    }

    serde_json::from_str::<ProfileResponse>(&body)
        .map(|response| response.data)
        .map_err(|_| {
            "Apple created the profile but returned an unreadable download response.".to_string()
        })
}

fn replacement_profile_request(
    profile_name: &str,
    bundle_id: &str,
    certificate_id: &str,
) -> serde_json::Value {
    profile_request(
        profile_name,
        "IOS_APP_STORE",
        bundle_id,
        certificate_id,
        None,
    )
}

fn decode_profile_content(encoded_content: &str) -> Result<Vec<u8>, String> {
    if encoded_content.is_empty() || encoded_content.len() > 4 * 1024 * 1024 {
        return Err(
            "Apple created the profile but returned content with an unsafe size.".to_string(),
        );
    }
    let content = BASE64_STANDARD.decode(encoded_content).map_err(|_| {
        "Apple created the profile but returned invalid encoded content.".to_string()
    })?;
    if content.is_empty() || content.len() > 2 * 1024 * 1024 {
        return Err(
            "Apple created the profile but returned content with an unsafe size.".to_string(),
        );
    }

    Ok(content)
}

fn is_distribution_certificate(certificate: &AppleCertificateSummary) -> bool {
    matches!(
        certificate.certificate_type.as_str(),
        "DISTRIBUTION" | "IOS_DISTRIBUTION"
    )
}

fn certificate_is_usable(certificate: &AppleCertificateSummary, now: u64) -> bool {
    is_distribution_certificate(certificate) && rfc3339_is_after(&certificate.expiration_date, now)
}

fn profile_is_usable_app_store(profile: &AppleProvisioningProfileSummary, now: u64) -> bool {
    profile.profile_type == "IOS_APP_STORE"
        && profile.profile_state == "ACTIVE"
        && rfc3339_is_after(&profile.expiration_date, now)
}

fn rfc3339_is_after(value: &str, unix_seconds: u64) -> bool {
    let Ok(timestamp) = OffsetDateTime::parse(value, &Rfc3339) else {
        return false;
    };
    let Ok(now) = OffsetDateTime::from_unix_timestamp(unix_seconds as i64) else {
        return false;
    };

    timestamp > now
}

fn create_token(
    key_id: &str,
    issuer_id: &str,
    private_key: &str,
    now: u64,
) -> Result<String, String> {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(key_id.to_string());
    let claims = AppleTokenClaims {
        iss: issuer_id,
        iat: now.saturating_sub(5),
        exp: now + TOKEN_LIFETIME_SECONDS,
        aud: APP_STORE_CONNECT_AUDIENCE,
    };
    let encoding_key = EncodingKey::from_ec_pem(private_key.as_bytes()).map_err(|_| {
        "The stored .p8 file is not a valid App Store Connect ES256 private key.".to_string()
    })?;

    encode(&header, &claims, &encoding_key)
        .map_err(|_| "BuildBridge could not sign the Apple API verification token.".to_string())
}

fn parse_app_response(body: &str, bundle_identifier: &str) -> Result<Option<AppResource>, String> {
    let response: AppsResponse = serde_json::from_str(body)
        .map_err(|_| "Apple returned an unreadable app-record response.".to_string())?;

    Ok(response
        .data
        .into_iter()
        .find(|item| item.attributes.bundle_id == bundle_identifier))
}

fn parse_bundle_id_response(
    body: &str,
    bundle_identifier: &str,
) -> Result<Option<BundleIdResource>, String> {
    let response: BundleIdsResponse = serde_json::from_str(body)
        .map_err(|_| "Apple returned an unreadable bundle-ID response.".to_string())?;

    Ok(response
        .data
        .into_iter()
        .find(|item| item.attributes.identifier == bundle_identifier))
}

fn parse_profiles_response(body: &str) -> Result<Vec<AppleProvisioningProfileSummary>, String> {
    let response: ProfilesResponse = serde_json::from_str(body)
        .map_err(|_| "Apple returned an unreadable profiles response.".to_string())?;

    Ok(response
        .data
        .into_iter()
        .map(|profile| {
            let attributes = profile.attributes.unwrap_or_default();
            profile_summary(profile.id, attributes)
        })
        .collect())
}

fn profile_summary(id: String, attributes: ProfileAttributes) -> AppleProvisioningProfileSummary {
    AppleProvisioningProfileSummary {
        id,
        name: attributes
            .name
            .unwrap_or_else(|| "Unnamed provisioning profile".to_string()),
        platform: attributes.platform.unwrap_or_else(|| "UNKNOWN".to_string()),
        profile_type: attributes
            .profile_type
            .unwrap_or_else(|| "UNKNOWN".to_string()),
        profile_state: attributes
            .profile_state
            .unwrap_or_else(|| "UNKNOWN".to_string()),
        uuid: attributes.uuid.unwrap_or_default(),
        created_date: attributes.created_date.unwrap_or_default(),
        expiration_date: attributes.expiration_date.unwrap_or_default(),
        device_udids: Vec::new(),
        certificate_ids: Vec::new(),
    }
}

fn parse_certificates_response(body: &str) -> Result<Vec<AppleCertificateSummary>, String> {
    let response: CertificatesResponse = serde_json::from_str(body)
        .map_err(|_| "Apple returned an unreadable certificates response.".to_string())?;

    Ok(response.data.into_iter().map(certificate_summary).collect())
}

fn certificate_summary(certificate: CertificateResource) -> AppleCertificateSummary {
    let attributes = certificate.attributes.unwrap_or_default();
    AppleCertificateSummary {
        id: certificate.id,
        name: attributes
            .name
            .unwrap_or_else(|| "Unnamed signing certificate".to_string()),
        display_name: attributes.display_name.unwrap_or_default(),
        certificate_type: attributes
            .certificate_type
            .unwrap_or_else(|| "UNKNOWN".to_string()),
        serial_number: attributes.serial_number.unwrap_or_default(),
        platform: attributes.platform.unwrap_or_else(|| "UNKNOWN".to_string()),
        expiration_date: attributes.expiration_date.unwrap_or_default(),
    }
}

fn validate_profile_id(profile_id: &str) -> Result<(), String> {
    if profile_id.is_empty()
        || profile_id.len() > 128
        || !profile_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("That provisioning profile identifier is not valid.".to_string());
    }

    Ok(())
}

fn validate_certificate_id(certificate_id: &str) -> Result<(), String> {
    if certificate_id.is_empty()
        || certificate_id.len() > 128
        || !certificate_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("Choose a valid Apple Distribution certificate.".to_string());
    }

    Ok(())
}

fn connection_error(resource: &str, error: &reqwest::Error) -> String {
    if error.is_timeout() {
        format!(
            "Apple did not respond to the {resource} check within 20 seconds. Check this host's network connection and try again."
        )
    } else {
        format!("Could not reach Apple for the {resource} check: {error}")
    }
}

fn apple_error_message(status: StatusCode, body: &str, resource: &str) -> String {
    let detail = serde_json::from_str::<AppleErrorResponse>(body)
        .ok()
        .and_then(|response| response.errors.into_iter().next())
        .and_then(|error| error.detail.or(error.title))
        .map(|value| truncate(&value, 240));

    let guidance = match status {
        StatusCode::UNAUTHORIZED => format!(
            "Apple rejected the Team API key while checking the {resource}. Check the Key ID, Issuer ID, and matching .p8 file."
        ),
        StatusCode::FORBIDDEN => format!(
            "Apple accepted the key but it cannot access {resource}. Use a Team key with the required role; individual API keys cannot use provisioning endpoints."
        ),
        StatusCode::TOO_MANY_REQUESTS => format!(
            "Apple temporarily rate-limited the {resource} check. Wait a moment and try again."
        ),
        StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY
            if resource == "distribution certificate" =>
        {
            "Apple refused to issue another distribution certificate. Apple allows only a few active ones per team: revoke an unused one in the developer portal, or export an existing one from the Mac that holds its key."
                .to_string()
        }
        StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY
            if resource == "development certificate" =>
        {
            "Apple refused to issue another development certificate. Apple allows only a few active ones per team: revoke an unused one in the developer portal, or store the .p12 of an existing one in the kit."
                .to_string()
        }
        StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY
            if resource == "device registration" =>
        {
            "Apple refused to register the iPhone. A team may register up to 100 iPhones per membership year, and removed devices still count until the membership renews; check Devices in the developer portal."
                .to_string()
        }
        StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY => format!(
            "Apple rejected the requested {resource}. Verify the Bundle ID and selected distribution certificate, then try again."
        ),
        _ => format!("Apple could not complete the {resource} check right now."),
    };

    match detail {
        Some(detail) if !detail.is_empty() => format!("{guidance} Apple says: {detail}"),
        _ => guidance,
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn unix_timestamp() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| {
            "This host's clock is invalid. Correct it before contacting Apple.".to_string()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_created_certificate_is_read_with_its_content() {
        let created = parse_created_certificate_for(
            r#"{
                "data": {
                    "type": "certificates",
                    "id": "CERT123",
                    "attributes": {
                        "name": "Apple Distribution: Example Developer (TEAM123456)",
                        "displayName": "Example Developer",
                        "certificateType": "DISTRIBUTION",
                        "serialNumber": "0123456789ABCDEF",
                        "platform": "IOS",
                        "expirationDate": "2027-09-03T10:00:00.000+00:00",
                        "certificateContent": "MIIBAQ=="
                    }
                }
            }"#,
            CertificateKind::Distribution,
        )
        .expect("a created certificate decodes");

        assert_eq!(created.certificate.id, "CERT123");
        assert_eq!(created.certificate.certificate_type, "DISTRIBUTION");
        assert_eq!(created.content, vec![0x30, 0x82, 0x01, 0x01]);
    }

    #[test]
    fn a_certificate_of_the_wrong_kind_or_without_content_is_refused() {
        let development = parse_created_certificate_for(
            r#"{"data": {"type": "certificates", "id": "C1", "attributes": {"certificateType": "DEVELOPMENT", "certificateContent": "MIIBAQ=="}}}"#,
            CertificateKind::Distribution,
        );
        assert!(development.is_err());

        let empty = parse_created_certificate_for(
            r#"{"data": {"type": "certificates", "id": "C1", "attributes": {"certificateType": "DISTRIBUTION"}}}"#,
            CertificateKind::Distribution,
        );
        assert!(empty.unwrap_err().contains("did not return its content"));
    }

    #[test]
    fn the_certificate_request_carries_only_a_type_and_the_csr() {
        let csr = "-----BEGIN CERTIFICATE REQUEST-----\nMIIB\n-----END CERTIFICATE REQUEST-----";
        let request = certificate_request_for(csr, CertificateKind::Distribution);

        assert_eq!(request["data"]["type"], "certificates");
        assert_eq!(
            request["data"]["attributes"]["certificateType"],
            "DISTRIBUTION"
        );
        assert_eq!(request["data"]["attributes"]["csrContent"], csr);
        assert!(request["data"].get("relationships").is_none());
        assert!(valid_csr_pem(csr));
        assert!(!valid_csr_pem(
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----"
        ));
        assert!(!valid_csr_pem(
            "-----BEGIN CERTIFICATE REQUEST-----\n$(rm)\n-----END CERTIFICATE REQUEST-----"
        ));
    }

    #[test]
    fn accepts_the_opaque_identifiers_apple_issues_for_profiles() {
        assert!(validate_profile_id("ABCD1234EF").is_ok());
        assert!(validate_profile_id("2f3d9c10-0a6b-4f4e-9c2e-1d0b7a5e6c11").is_ok());
    }

    #[test]
    fn rejects_a_profile_identifier_that_could_reshape_the_request_path() {
        assert!(validate_profile_id("").is_err());
        assert!(validate_profile_id("../v1/users").is_err());
        assert!(validate_profile_id("id?include=bundleId").is_err());
        assert!(validate_profile_id(&"a".repeat(129)).is_err());
    }

    #[test]
    fn parses_an_exact_bundle_identifier_match() {
        let bundle = parse_bundle_id_response(
            r#"{
                "data": [
                    {
                        "type": "bundleIds",
                        "id": "unrelated-id",
                        "attributes": {
                            "name": "Another app",
                            "identifier": "com.example.other",
                            "platform": "IOS",
                            "seedId": "TEAM123456"
                        }
                    },
                    {
                        "type": "bundleIds",
                        "id": "opaque-id",
                        "attributes": {
                            "name": "Example app",
                            "identifier": "com.example.app",
                            "platform": "IOS",
                            "seedId": "TEAM123456"
                        }
                    }
                ]
            }"#,
            "com.example.app",
        )
        .expect("response should parse")
        .expect("bundle should match");

        assert_eq!(bundle.attributes.name, "Example app");
        assert_eq!(bundle.attributes.platform, "IOS");
        assert_eq!(bundle.attributes.seed_id.as_deref(), Some("TEAM123456"));
    }

    #[test]
    fn parses_profile_expiry_metadata_without_profile_contents() {
        let profiles = parse_profiles_response(
            r#"{
                "data": [
                    {
                        "type": "profiles",
                        "id": "profile-resource-id",
                        "attributes": {
                            "name": "Example App Store",
                            "platform": "IOS",
                            "profileType": "IOS_APP_STORE",
                            "profileState": "INVALID",
                            "uuid": "00000000-0000-0000-0000-000000000001",
                            "createdDate": "2025-01-01T00:00:00Z",
                            "expirationDate": "2026-01-01T00:00:00Z"
                        }
                    },
                    {
                        "type": "profiles",
                        "id": "legacy-profile-resource-id",
                        "attributes": {
                            "name": "Legacy profile",
                            "expirationDate": null
                        }
                    }
                ]
            }"#,
        )
        .expect("profiles should parse");

        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].profile_state, "INVALID");
        assert_eq!(profiles[0].profile_type, "IOS_APP_STORE");
        assert_eq!(profiles[0].expiration_date, "2026-01-01T00:00:00Z");
        assert_eq!(profiles[1].profile_state, "UNKNOWN");
        assert!(profiles[1].expiration_date.is_empty());
    }

    #[test]
    fn parses_and_classifies_distribution_certificate_metadata() {
        let certificates = parse_certificates_response(
            r#"{
                "data": [{
                    "type": "certificates",
                    "id": "certificate-resource-id",
                    "attributes": {
                        "name": "Apple Distribution",
                        "displayName": "Apple Distribution: Example",
                        "certificateType": "DISTRIBUTION",
                        "serialNumber": "ABC123",
                        "platform": "IOS",
                        "expirationDate": "2027-01-01T00:00:00Z"
                    }
                }]
            }"#,
        )
        .expect("certificates should parse");

        assert_eq!(certificates.len(), 1);
        assert!(is_distribution_certificate(&certificates[0]));
        assert!(certificate_is_usable(&certificates[0], 1_788_220_800));
        assert!(!certificate_is_usable(&certificates[0], 1_830_297_600));
    }

    #[test]
    fn preserves_non_distribution_certificates_for_inventory_diagnostics() {
        let certificates = parse_certificates_response(
            r#"{
                "data": [{
                    "type": "certificates",
                    "id": "development-certificate-resource-id",
                    "attributes": {
                        "name": "Apple Development",
                        "displayName": "Apple Development: Example",
                        "certificateType": "DEVELOPMENT",
                        "serialNumber": "DEF456",
                        "platform": "IOS",
                        "expirationDate": "2027-01-01T00:00:00Z"
                    }
                }]
            }"#,
        )
        .expect("certificate inventory should parse");

        assert_eq!(certificates.len(), 1);
        assert_eq!(certificates[0].certificate_type, "DEVELOPMENT");
        assert!(!is_distribution_certificate(&certificates[0]));
        assert!(!certificate_is_usable(&certificates[0], 1_788_220_800));
    }

    #[test]
    fn replacement_profile_request_uses_only_the_confirmed_relationships() {
        let request = replacement_profile_request(
            "BuildBridge App Store 1788324836",
            "bundle-resource-id",
            "certificate-resource-id",
        );

        assert_eq!(request["data"]["type"], "profiles");
        assert_eq!(
            request["data"]["attributes"]["profileType"],
            "IOS_APP_STORE"
        );
        assert_eq!(
            request["data"]["relationships"]["bundleId"]["data"]["id"],
            "bundle-resource-id"
        );
        assert_eq!(
            request["data"]["relationships"]["certificates"]["data"][0]["id"],
            "certificate-resource-id"
        );
        assert!(request["data"]["relationships"].get("devices").is_none());
    }

    #[test]
    fn created_profile_content_is_bounded_and_base64_decoded() {
        assert_eq!(
            decode_profile_content("YnBsaXN0MDDRAQJYdmVyc2lvbgkICw==")
                .expect("small encoded profile should decode"),
            b"bplist00\xd1\x01\x02Xversion\t\x08\x0b"
        );
        assert!(decode_profile_content("").is_err());
        assert!(decode_profile_content("not base64").is_err());
    }

    #[test]
    fn parses_an_exact_app_store_record_match() {
        let app = parse_app_response(
            r#"{
                "data": [{
                    "type": "apps",
                    "id": "1234567890",
                    "attributes": {
                        "name": "Example app",
                        "bundleId": "com.example.app"
                    }
                }]
            }"#,
            "com.example.app",
        )
        .expect("response should parse")
        .expect("app should match");

        assert_eq!(app.id, "1234567890");
        assert_eq!(app.attributes.name, "Example app");
    }

    #[test]
    fn successful_empty_responses_are_distinct_from_api_errors() {
        let app = parse_app_response(r#"{"data": []}"#, "com.example.app")
            .expect("app response should parse");
        let bundle = parse_bundle_id_response(r#"{"data": []}"#, "com.example.app")
            .expect("bundle response should parse");

        assert!(app.is_none());
        assert!(bundle.is_none());
    }

    #[test]
    fn maps_apple_authentication_errors_without_echoing_the_request() {
        let message = apple_error_message(
            StatusCode::UNAUTHORIZED,
            r#"{"errors":[{"title":"NOT_AUTHORIZED","detail":"Authentication credentials are missing or invalid."}]}"#,
            "App Store app",
        );

        assert!(message.contains("while checking the App Store app"));
        assert!(message.contains("Authentication credentials are missing or invalid"));
    }

    #[test]
    fn rejects_a_non_ec_private_key_before_network_access() {
        let result = create_token(
            "837B3VAM6Z",
            "00000000-0000-0000-0000-000000000000",
            "-----BEGIN PRIVATE KEY-----\nnot-a-key\n-----END PRIVATE KEY-----",
            123,
        );

        assert_eq!(
            result.expect_err("invalid key should fail"),
            "The stored .p8 file is not a valid App Store Connect ES256 private key."
        );
    }
}
