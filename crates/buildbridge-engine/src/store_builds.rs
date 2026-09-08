//! What the stores already hold for the approved app, so a build number can be chosen that
//! they will accept. App Store Connect and TestFlight take an upload only when its build
//! number is higher than the last one for that marketing version; Google Play only when its
//! version code is higher than any it has ever received. buildbridge reads and reports; it
//! never bumps a number on its own.

use std::cmp::Ordering;

use super::*;
use ts_rs::TS;

/// One build a store holds: its number, the version it was uploaded under when the store says,
/// and when and in what state it arrived when the store says that too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StoreBuild {
    pub version: Option<String>,
    pub build: String,
    pub uploaded_at: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StoreBuildsCheck {
    /// `app_store_connect` or `google_play`.
    pub store: String,
    /// The version the store was asked about; Apple's rule is scoped to it.
    pub version: String,
    /// The build the next upload must beat: Apple's highest for this version, Google Play's
    /// highest of all. None when the store holds nothing that binds this version.
    pub must_exceed: Option<StoreBuild>,
    /// The highest build the store holds for any version.
    pub latest: Option<StoreBuild>,
    /// The lowest build number the store would accept now, when the numbers are whole.
    pub next_build: Option<String>,
    /// How many builds were read; Apple returns at most its latest 200.
    #[ts(type = "number")]
    pub builds_seen: u64,
    #[ts(type = "number")]
    pub checked_at_epoch_seconds: u64,
}

#[derive(Debug, Default, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CheckStoreBuildsInput {
    /// The version to ask about; the project's own when absent.
    #[serde(default)]
    pub version: Option<String>,
}

/// Dotted whole numbers compared part by part, `15` against `16` or `1.0.3` against `1.0.10`;
/// None when either is not that shape, since the stores' own comparison is then unknowable.
pub(crate) fn compare_build_numbers(a: &str, b: &str) -> Option<Ordering> {
    let parts = |value: &str| -> Option<Vec<u64>> {
        value
            .split('.')
            .map(|part| {
                (!part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
                    .then(|| part.parse().ok())
                    .flatten()
            })
            .collect()
    };
    let (a, b) = (parts(a)?, parts(b)?);
    let length = a.len().max(b.len());
    let at = |values: &[u64], index: usize| values.get(index).copied().unwrap_or(0);
    Some(
        (0..length)
            .map(|index| at(&a, index).cmp(&at(&b, index)))
            .find(|ordering| *ordering != Ordering::Equal)
            .unwrap_or(Ordering::Equal),
    )
}

/// The build after this one: its last part plus one.
pub(crate) fn next_build_number(build: &str) -> Option<String> {
    compare_build_numbers(build, build)?;
    let mut parts: Vec<u64> = build
        .split('.')
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    let last = parts.last_mut()?;
    *last = last.checked_add(1)?;
    Some(
        parts
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join("."),
    )
}

fn highest<'a>(builds: impl Iterator<Item = &'a StoreBuild>) -> Option<StoreBuild> {
    builds
        .max_by(|a, b| compare_build_numbers(&a.build, &b.build).unwrap_or(Ordering::Equal))
        .cloned()
}

fn requested_version(
    input: Option<CheckStoreBuildsInput>,
    project: Option<String>,
) -> Result<String, String> {
    let requested = input
        .and_then(|input| input.version)
        .map(|version| version.trim().to_string())
        .filter(|version| !version.is_empty());
    let version = match requested.or(project) {
        Some(version) => version,
        None => {
            return Err(
                "Give the version to ask about, or declare one in the project first.".to_string(),
            );
        }
    };
    if !buildbridge_machines::valid_project_version(&version) {
        return Err(
            "The version must be 1 to 64 letters, digits, periods or hyphens, such as 3.2.1."
                .to_string(),
        );
    }
    Ok(version)
}

/// Asks the machine's store what it already holds for the approved app. A read against the
/// store's API with the credentials the machine's kit or connection already carries; no
/// machine lock, nothing written anywhere.
pub async fn check_store_builds(
    app: &Engine,
    machine_id: String,
    input: Option<CheckStoreBuildsInput>,
) -> Result<StoreBuildsCheck, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    let provider = machines::load_registry(app)?
        .find(&machine_id)?
        .config
        .provider;
    let checked_at_epoch_seconds = machines::now_epoch_seconds();

    if provider.is_macos() {
        let workspace = load_apple_workspace(&paths)?
            .ok_or_else(|| "Approve an Apple project first.".to_string())?;
        let bundle_identifier = workspace.bundle_identifier.clone().ok_or_else(|| {
            "buildbridge could not detect PRODUCT_BUNDLE_IDENTIFIER in the approved project."
                .to_string()
        })?;
        let version = requested_version(
            input,
            read_apple_project_version(&workspace.local_path, &workspace.layout)
                .map(|version| version.version),
        )?;
        let secrets = resolve_signing_kit_for(app, &machine_id).await?;
        let key_id = secrets.app_store_connect_key_id.ok_or_else(|| {
            "The stored signing credentials do not include an App Store Connect Key ID.".to_string()
        })?;
        let issuer_id = secrets.app_store_connect_issuer_id.ok_or_else(|| {
            "The stored signing credentials do not include an App Store Connect Issuer ID."
                .to_string()
        })?;
        let private_key = secrets.app_store_connect_private_key.ok_or_else(|| {
            "The stored signing credentials do not include an App Store Connect .p8 key."
                .to_string()
        })?;
        let builds: Vec<StoreBuild> =
            apple_api::fetch_builds(&key_id, &issuer_id, &private_key, &bundle_identifier)
                .await?
                .into_iter()
                .map(|build| StoreBuild {
                    version: build.version,
                    build: build.build,
                    uploaded_at: build.uploaded_at,
                    state: build.processing_state,
                })
                .collect();
        let must_exceed = highest(
            builds
                .iter()
                .filter(|build| build.version.as_deref() == Some(version.as_str())),
        );
        return Ok(StoreBuildsCheck {
            store: "app_store_connect".to_string(),
            version,
            next_build: must_exceed
                .as_ref()
                .and_then(|build| next_build_number(&build.build)),
            must_exceed,
            latest: highest(builds.iter()),
            builds_seen: builds.len() as u64,
            checked_at_epoch_seconds,
        });
    }

    let workspace = load_android_workspace(&paths)?
        .ok_or_else(|| "Approve an Android project first.".to_string())?;
    let package_name = workspace.application_id.clone().ok_or_else(|| {
        "buildbridge could not read one applicationId from the app module's Gradle script."
            .to_string()
    })?;
    let version = requested_version(
        input,
        read_android_project_version(&workspace.local_path, &workspace.layout)
            .map(|version| version.version),
    )?;
    let codes: Vec<StoreBuild> = google_play::fetch_version_codes(app, &machine_id, &package_name)
        .await?
        .into_iter()
        .map(|code| StoreBuild {
            version: code.release_name,
            build: code.code.to_string(),
            uploaded_at: None,
            state: None,
        })
        .collect();
    let latest = highest(codes.iter());
    Ok(StoreBuildsCheck {
        store: "google_play".to_string(),
        version,
        next_build: latest
            .as_ref()
            .and_then(|build| next_build_number(&build.build)),
        must_exceed: latest.clone(),
        latest,
        builds_seen: codes.len() as u64,
        checked_at_epoch_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_numbers_compare_part_by_part_and_only_when_whole() {
        assert_eq!(compare_build_numbers("15", "16"), Some(Ordering::Less));
        assert_eq!(compare_build_numbers("16", "16"), Some(Ordering::Equal));
        assert_eq!(compare_build_numbers("100", "16"), Some(Ordering::Greater));
        assert_eq!(
            compare_build_numbers("1.0.10", "1.0.3"),
            Some(Ordering::Greater)
        );
        assert_eq!(compare_build_numbers("1.0", "1.0.0"), Some(Ordering::Equal));
        assert_eq!(compare_build_numbers("1.0-beta", "1"), None);
        assert_eq!(compare_build_numbers("", "1"), None);
    }

    #[test]
    fn the_next_build_steps_the_last_part() {
        assert_eq!(next_build_number("15").as_deref(), Some("16"));
        assert_eq!(next_build_number("1.0.3").as_deref(), Some("1.0.4"));
        assert_eq!(next_build_number("beta"), None);
    }

    #[test]
    fn the_requested_version_falls_back_to_the_project_and_is_checked() {
        let input = |version: &str| {
            Some(CheckStoreBuildsInput {
                version: Some(version.to_string()),
            })
        };
        assert_eq!(
            requested_version(input(" 3.3.0 "), Some("3.2.0".into())).unwrap(),
            "3.3.0"
        );
        assert_eq!(
            requested_version(input(""), Some("3.2.0".into())).unwrap(),
            "3.2.0"
        );
        assert_eq!(
            requested_version(None, Some("3.2.0".into())).unwrap(),
            "3.2.0"
        );
        assert!(requested_version(None, None).is_err());
        assert!(requested_version(input("3.2 beta"), None).is_err());
    }

    #[test]
    fn the_highest_build_wins_regardless_of_upload_order() {
        let build = |number: &str| StoreBuild {
            version: Some("3.2.0".into()),
            build: number.into(),
            uploaded_at: None,
            state: None,
        };
        let builds = [build("9"), build("15"), build("12")];
        assert_eq!(highest(builds.iter()).unwrap().build, "15");
        assert_eq!(highest(std::iter::empty()), None);
    }
}
