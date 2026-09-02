use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const APP_STORE_CONNECT_BUNDLE_IDS_URL: &str = "https://api.appstoreconnect.apple.com/v1/bundleIds";
const APP_STORE_CONNECT_APPS_URL: &str = "https://api.appstoreconnect.apple.com/v1/apps";
const APP_STORE_CONNECT_CERTIFICATES_URL: &str =
    "https://api.appstoreconnect.apple.com/v1/certificates";
const APP_STORE_CONNECT_PROFILES_URL: &str = "https://api.appstoreconnect.apple.com/v1/profiles";
const APP_STORE_CONNECT_AUDIENCE: &str = "appstoreconnect-v1";
const TOKEN_LIFETIME_SECONDS: u64 = 5 * 60;

#[derive(Debug, Serialize)]
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
    verified_at_epoch_seconds: u64,
}

#[derive(Clone, Debug, Serialize)]
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
}

#[derive(Clone, Debug, Serialize)]
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

#[derive(Debug, Deserialize)]
struct BundleIdsResponse {
    data: Vec<BundleIdResource>,
}

#[derive(Debug, Deserialize)]
struct BundleIdResource {
    id: String,
    attributes: BundleIdAttributes,
}

#[derive(Debug, Deserialize)]
struct AppsResponse {
    data: Vec<AppResource>,
}

#[derive(Debug, Deserialize)]
struct AppResource {
    id: String,
    attributes: AppAttributes,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppAttributes {
    name: String,
    bundle_id: String,
}

#[derive(Debug, Deserialize)]
struct ProfilesResponse {
    data: Vec<ProfileResource>,
}

#[derive(Debug, Deserialize)]
struct ProfileResponse {
    data: ProfileResource,
}

#[derive(Debug, Deserialize)]
struct ProfileResource {
    id: String,
    #[serde(default)]
    attributes: Option<ProfileAttributes>,
}

#[derive(Debug, Default, Deserialize)]
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

#[derive(Debug, Deserialize)]
struct CertificatesResponse {
    data: Vec<CertificateResource>,
}

#[derive(Debug, Deserialize)]
struct CertificateResource {
    id: String,
    #[serde(default)]
    attributes: Option<CertificateAttributes>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CertificateAttributes {
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BundleIdAttributes {
    identifier: String,
    name: String,
    platform: String,
    seed_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AppleErrorResponse {
    errors: Vec<AppleApiError>,
}

#[derive(Debug, Deserialize)]
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
        verified_at_epoch_seconds: now,
    })
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
    serde_json::json!({
        "data": {
            "type": "profiles",
            "attributes": {
                "name": profile_name,
                "profileType": "IOS_APP_STORE"
            },
            "relationships": {
                "bundleId": {
                    "data": { "type": "bundleIds", "id": bundle_id }
                },
                "certificates": {
                    "data": [{ "type": "certificates", "id": certificate_id }]
                }
            }
        }
    })
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
                            "name": "ThinkSolar",
                            "identifier": "nz.co.thinksolar.app",
                            "platform": "IOS",
                            "seedId": "TEAM123456"
                        }
                    }
                ]
            }"#,
            "nz.co.thinksolar.app",
        )
        .expect("response should parse")
        .expect("bundle should match");

        assert_eq!(bundle.attributes.name, "ThinkSolar");
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
                            "name": "Think Solar App Store",
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
                        "name": "ThinkSolar",
                        "bundleId": "nz.co.thinksolar.app"
                    }
                }]
            }"#,
            "nz.co.thinksolar.app",
        )
        .expect("response should parse")
        .expect("app should match");

        assert_eq!(app.id, "1234567890");
        assert_eq!(app.attributes.name, "ThinkSolar");
    }

    #[test]
    fn successful_empty_responses_are_distinct_from_api_errors() {
        let app = parse_app_response(r#"{"data": []}"#, "nz.co.thinksolar.app")
            .expect("app response should parse");
        let bundle = parse_bundle_id_response(r#"{"data": []}"#, "nz.co.thinksolar.app")
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
