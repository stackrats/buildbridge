//! Host-level preferences: the few things about this computer buildbridge does not decide for
//! itself. They live in `settings.json` beside the machine registry, so the desktop and the
//! command line read the same ones, and nothing in the file is secret.

use super::*;

const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct HostSettings {
    /// The browser that opens pages outside buildbridge — a provider's repository, Apple's
    /// downloads, a store console — as a command name, a path, or on macOS an application
    /// bundle such as `Firefox.app`. `None` is the desktop's own default browser.
    #[serde(default)]
    pub browser: Option<String>,
    /// Whether this host offers remote builds: pairing with a control plane, claiming queued
    /// work, and sharing a Mac with someone else. They need a buildbridge server to pair with,
    /// and that service is self-hosted, so a host that has not stood one up is better off not
    /// being shown the feature at all. Off, the desktop hides the pane, the sidebar item and
    /// the control-plane chip, and nothing polls a queue. Turn it on with
    /// `buildbridge settings set --remote-builds on`.
    #[serde(default)]
    pub remote_builds: bool,
}

/// Where buildbridge keeps its files on this host: the small records in the configuration
/// directory, and the large ones — machine disks, artifacts, downloads — in the data directory.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StorageLocations {
    pub config_dir: String,
    pub data_dir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum StorageDirectory {
    Config,
    Data,
}

pub(crate) fn load_settings(app: &Engine) -> Result<HostSettings, String> {
    match fs::read(app.config_dir().join(SETTINGS_FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("The settings file is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(HostSettings::default()),
        Err(error) => Err(format!("The settings file could not be read: {error}")),
    }
}

/// Whether remote builds are on for this host. A settings file that cannot be read counts as
/// off, so a damaged file leaves the queue alone rather than starting it by accident.
pub(crate) fn remote_builds_enabled(app: &Engine) -> bool {
    load_settings(app).is_ok_and(|settings| settings.remote_builds)
}

/// The refusal every control-plane entry point gives while remote builds are off, so a client
/// that reaches one anyway — the command line, an old window — is told how to turn them on
/// rather than shown a connection error for a server it was never going to reach.
pub(crate) fn require_remote_builds(app: &Engine) -> Result<(), String> {
    if remote_builds_enabled(app) {
        Ok(())
    } else {
        Err("Remote builds are off on this host. Turn them on with: buildbridge settings set --remote-builds on".to_string())
    }
}

fn store_settings(app: &Engine, settings: &HostSettings) -> Result<(), String> {
    let config_dir = app.config_dir();
    fs::create_dir_all(&config_dir).map_err(|error| error.to_string())?;
    fs::write(
        config_dir.join(SETTINGS_FILE),
        serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("The settings could not be saved: {error}"))
}

/// A browser field as typed: trimmed, and blank means the default browser.
fn normalize_browser(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

pub async fn get_host_settings(app: &Engine) -> Result<HostSettings, String> {
    load_settings(app)
}

/// Saves the settings after checking what can be checked: a browser that is named must exist
/// on this computer now, so the mistake is found here and not on the next link.
pub async fn save_host_settings(app: &Engine, input: HostSettings) -> Result<HostSettings, String> {
    let settings = HostSettings {
        browser: normalize_browser(input.browser),
        remote_builds: input.remote_builds,
    };
    if let Some(browser) = &settings.browser {
        crate::opener::resolve_browser(browser)?;
    }
    store_settings(app, &settings)?;
    Ok(settings)
}

pub async fn get_storage_locations(app: &Engine) -> Result<StorageLocations, String> {
    Ok(StorageLocations {
        config_dir: app.config_dir().to_string_lossy().into_owned(),
        data_dir: app.data_dir().to_string_lossy().into_owned(),
    })
}

/// Shows one of buildbridge's own directories in the file manager, creating it first so the
/// window has somewhere to land on a fresh installation.
pub async fn reveal_storage_directory(
    app: &Engine,
    directory: StorageDirectory,
) -> Result<(), String> {
    let (path, what) = match directory {
        StorageDirectory::Config => (app.config_dir(), "the configuration directory"),
        StorageDirectory::Data => (app.data_dir(), "the data directory"),
    };
    fs::create_dir_all(&path).map_err(|error| format!("Could not create {what}: {error}"))?;
    crate::opener::reveal_directory(&path, what)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_browser_means_the_default_one() {
        assert_eq!(normalize_browser(None), None);
        assert_eq!(normalize_browser(Some("   ".to_string())), None);
        assert_eq!(
            normalize_browser(Some("  firefox ".to_string())),
            Some("firefox".to_string())
        );
    }

    #[test]
    fn settings_written_before_a_field_existed_still_load() {
        let settings: HostSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, HostSettings::default());
        let settings: HostSettings = serde_json::from_str(r#"{"browser":"vivaldi"}"#).unwrap();
        assert_eq!(settings.browser.as_deref(), Some("vivaldi"));
    }

    #[test]
    fn remote_builds_are_off_until_a_host_asks_for_them() {
        assert!(!HostSettings::default().remote_builds);
        let settings: HostSettings = serde_json::from_str(r#"{"browser":"vivaldi"}"#).unwrap();
        assert!(!settings.remote_builds);
        let settings: HostSettings = serde_json::from_str(r#"{"remoteBuilds":true}"#).unwrap();
        assert!(settings.remote_builds);
    }
}
