//! Running a Debug build on a physical iPhone from the guest: what `devicectl` sees once a
//! phone is passed through, and building, installing and launching on one of them.
//!
//! `devicectl` is Xcode's CoreDevice tool. Its JSON is read strictly into a small model; any
//! value it prints that this crate does not know becomes `Unknown` rather than an error, so a
//! new Xcode cannot break the listing, while identifiers and UDIDs are validated because they
//! later become command arguments.

use std::io::Read;
use std::path::Path;
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::{
    ProviderError, TrackedCommand, clean_output, guest_ssh_command, shell_single_quote,
    valid_device_udid, valid_profile_uuid, validate_guest_operation,
};

/// One `devicectl` entry is a few kilobytes of capabilities; a handful of phones fits well
/// inside a mebibyte, and anything larger is not a device list.
const DEVICE_LIST_MAX_BYTES: usize = 1024 * 1024;
const DEVICE_LIST_TIMEOUT_SECONDS: u32 = 10;
const NAME_MAX_CHARS: usize = 128;
const MODEL_MAX_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperModeState {
    Enabled,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingState {
    Paired,
    Unpaired,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    Connected,
    Disconnected,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportType {
    Wired,
    LocalNetwork,
    Unknown,
}

impl DeveloperModeState {
    fn from_devicectl(value: Option<&str>) -> Self {
        match value {
            Some("enabled") => Self::Enabled,
            Some("disabled") => Self::Disabled,
            _ => Self::Unknown,
        }
    }
}

impl PairingState {
    fn from_devicectl(value: Option<&str>) -> Self {
        match value {
            Some("paired") => Self::Paired,
            Some("unpaired") => Self::Unpaired,
            _ => Self::Unknown,
        }
    }
}

impl TunnelState {
    fn from_devicectl(value: Option<&str>) -> Self {
        match value {
            Some("connected") => Self::Connected,
            Some("disconnected") => Self::Disconnected,
            Some("unavailable") => Self::Unavailable,
            _ => Self::Unknown,
        }
    }
}

impl TransportType {
    fn from_devicectl(value: Option<&str>) -> Self {
        match value {
            Some("wired") => Self::Wired,
            Some("localNetwork") => Self::LocalNetwork,
            _ => Self::Unknown,
        }
    }
}

/// A phone as the guest reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestDevice {
    /// CoreDevice's UUID for the device; what `devicectl` commands take as `--device`.
    pub identifier: String,
    /// The hardware UDID, what Apple's developer portal registers.
    pub udid: Option<String>,
    pub name: String,
    pub os_version: Option<String>,
    pub model: Option<String>,
    pub developer_mode: DeveloperModeState,
    pub pairing_state: PairingState,
    pub tunnel_state: TunnelState,
    pub transport_type: TransportType,
    /// Wired, paired, and in Developer Mode: a build can be installed and launched.
    pub ready: bool,
    /// The first thing standing in the way, in the words the person needs on the phone.
    pub issue: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DevicectlOutput {
    #[serde(default)]
    result: Option<DevicectlResult>,
}

#[derive(Debug, Default, Deserialize)]
struct DevicectlResult {
    #[serde(default)]
    devices: Vec<DevicectlDevice>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevicectlDevice {
    identifier: String,
    #[serde(default)]
    hardware_properties: Option<HardwareProperties>,
    #[serde(default)]
    device_properties: Option<DeviceProperties>,
    #[serde(default)]
    connection_properties: Option<ConnectionProperties>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HardwareProperties {
    #[serde(default)]
    udid: Option<String>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    marketing_name: Option<String>,
    #[serde(default)]
    product_type: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceProperties {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    os_version_number: Option<String>,
    #[serde(default)]
    developer_mode_status: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionProperties {
    #[serde(default)]
    pairing_state: Option<String>,
    #[serde(default)]
    tunnel_state: Option<String>,
    #[serde(default)]
    transport_type: Option<String>,
}

/// Printable text only, bounded; `devicectl` repeats whatever the phone is named.
fn sanitize_text(value: Option<String>, max_chars: usize) -> Option<String> {
    let cleaned: String = value?
        .chars()
        .filter(|character| !character.is_control())
        .take(max_chars)
        .collect();
    let trimmed = cleaned.trim();

    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn sanitize_version(value: Option<String>) -> Option<String> {
    value
        .map(|version| version.trim().to_string())
        .filter(|version| {
            !version.is_empty()
                && version.len() <= 16
                && version
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'.')
        })
}

/// What stands between the phone and a build, if anything. Wired first: a phone seen over the
/// network cannot be the one plugged into this host.
fn device_issue(
    transport: TransportType,
    pairing: PairingState,
    developer_mode: DeveloperModeState,
) -> Option<&'static str> {
    match (transport, pairing, developer_mode) {
        (TransportType::LocalNetwork, _, _) => {
            Some("The phone is connected over the network, not USB.")
        }
        (_, PairingState::Paired, DeveloperModeState::Enabled) => None,
        (_, PairingState::Paired, _) => Some(
            "Turn on Developer Mode on the phone (Settings › Privacy & Security), then restart it if asked.",
        ),
        (_, _, _) => Some("Unlock the phone and tap Trust when it asks about this computer."),
    }
}

/// Reads `devicectl list devices --json-output` into devices. An empty document is an empty
/// list — `devicectl` writes nothing when it fails, and a failed listing is not an error the
/// person can act on beyond refreshing.
pub(crate) fn parse_devicectl_devices(json: &str) -> Result<Vec<GuestDevice>, ProviderError> {
    if json.trim().is_empty() {
        return Ok(Vec::new());
    }
    let output: DevicectlOutput = serde_json::from_str(json).map_err(|error| {
        ProviderError::GuestBridge(format!(
            "the guest returned an unreadable device list: {error}"
        ))
    })?;
    let mut devices = Vec::new();

    for raw in output.result.unwrap_or_default().devices {
        let hardware = raw.hardware_properties.unwrap_or_default();
        if hardware
            .platform
            .as_deref()
            .is_some_and(|platform| !platform.eq_ignore_ascii_case("ios"))
        {
            continue;
        }
        if !valid_profile_uuid(&raw.identifier) {
            return Err(ProviderError::GuestBridge(
                "the guest returned an invalid device identifier".to_string(),
            ));
        }
        let properties = raw.device_properties.unwrap_or_default();
        let connection = raw.connection_properties.unwrap_or_default();
        let developer_mode =
            DeveloperModeState::from_devicectl(properties.developer_mode_status.as_deref());
        let pairing_state = PairingState::from_devicectl(connection.pairing_state.as_deref());
        let tunnel_state = TunnelState::from_devicectl(connection.tunnel_state.as_deref());
        let transport_type = TransportType::from_devicectl(connection.transport_type.as_deref());
        let issue = device_issue(transport_type, pairing_state, developer_mode);
        let model = sanitize_text(hardware.marketing_name.clone(), MODEL_MAX_CHARS)
            .or_else(|| sanitize_text(hardware.product_type.clone(), MODEL_MAX_CHARS));

        devices.push(GuestDevice {
            identifier: raw.identifier,
            udid: hardware
                .udid
                .filter(|udid| valid_device_udid(udid))
                .map(|udid| udid.to_ascii_uppercase()),
            name: sanitize_text(properties.name, NAME_MAX_CHARS)
                .or_else(|| model.clone())
                .unwrap_or_else(|| "iPhone".to_string()),
            os_version: sanitize_version(properties.os_version_number),
            model,
            developer_mode,
            pairing_state,
            tunnel_state,
            transport_type,
            ready: issue.is_none(),
            issue: issue.map(str::to_string),
        });
    }

    Ok(devices)
}

/// Runs one guest command and returns its whole standard output, up to `max_bytes`, unlike
/// `run_guest_command`, whose output is trimmed for a diagnostic.
pub(crate) fn run_guest_command_capped(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    command: &str,
    max_bytes: usize,
) -> Result<String, ProviderError> {
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!(
                "could not run ssh; install OpenSSH client tools: {error}"
            ))
        })?;
    let mut stdout = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        pipe.take(max_bytes as u64 + 1)
            .read_to_end(&mut stdout)
            .map_err(|error| ProviderError::GuestBridge(error.to_string()))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| ProviderError::GuestBridge(error.to_string()))?;
    if !output.status.success() {
        let message = clean_output(&output.stderr);
        return Err(ProviderError::GuestBridge(if message.is_empty() {
            "the guest command failed".to_string()
        } else {
            message
        }));
    }
    if stdout.len() > max_bytes {
        return Err(ProviderError::GuestBridge(
            "the guest returned more output than expected".to_string(),
        ));
    }

    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

pub(crate) fn device_list_script(username: &str) -> String {
    let developer_dir = format!("/Users/{username}/Applications/Xcode.app/Contents/Developer");

    format!(
        "set -u; export DEVELOPER_DIR={}; out=$(/usr/bin/mktemp /tmp/buildbridge-devices.XXXXXX) || exit 1; /usr/bin/xcrun devicectl list devices --json-output \"$out\" --timeout {DEVICE_LIST_TIMEOUT_SECONDS} >/dev/null 2>&1 || /usr/bin/true; /bin/cat \"$out\"; /bin/rm -f \"$out\"",
        shell_single_quote(&developer_dir)
    )
}

/// The phones the guest can see right now.
pub fn list_guest_devices(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<Vec<GuestDevice>, ProviderError> {
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    let output = run_guest_command_capped(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &device_list_script(username),
        DEVICE_LIST_MAX_BYTES,
    )?;

    parse_devicectl_devices(&output)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"{
      "info": {"arguments": ["devicectl", "list", "devices"], "outcome": "success"},
      "result": {
        "devices": [
          {
            "capabilities": [{"featureIdentifier": "com.apple.coredevice.feature.launchapplication", "name": "Launch Application"}],
            "connectionProperties": {"pairingState": "paired", "potentialHostnames": ["00008030-001A2B3C4D5E6F00.coredevice.local"], "transportType": "wired", "tunnelState": "connected"},
            "deviceProperties": {"bootState": "booted", "developerModeStatus": "enabled", "name": "Matt’s iPhone", "osVersionNumber": "18.6"},
            "hardwareProperties": {"deviceType": "iPhone", "marketingName": "iPhone 15 Pro", "platform": "iOS", "productType": "iPhone16,1", "udid": "00008030-001a2b3c4d5e6f00"},
            "identifier": "E3F1A2B4-5C6D-4E7F-8A9B-0C1D2E3F4A5B"
          },
          {
            "connectionProperties": {"pairingState": "unpaired", "transportType": "localNetwork", "tunnelState": "unavailable"},
            "deviceProperties": {"name": "Spare\n phone"},
            "hardwareProperties": {"platform": "iOS", "udid": "not-a-udid"},
            "identifier": "0A1B2C3D-4E5F-4061-8273-8495A6B7C8D9"
          },
          {
            "connectionProperties": {"pairingState": "paired", "transportType": "wired", "tunnelState": "connected"},
            "deviceProperties": {"name": "Studio", "developerModeStatus": "enabled"},
            "hardwareProperties": {"platform": "macOS", "udid": "0123456789ABCDEF0123456789ABCDEF01234567"},
            "identifier": "1B2C3D4E-5F60-4718-8293-A4B5C6D7E8F9"
          }
        ]
      }
    }"#;

    #[test]
    fn devicectl_output_parses_into_bounded_guest_devices() {
        let devices = parse_devicectl_devices(LISTING).expect("parses");
        assert_eq!(devices.len(), 2, "the Mac is dropped");

        let phone = &devices[0];
        assert_eq!(phone.identifier, "E3F1A2B4-5C6D-4E7F-8A9B-0C1D2E3F4A5B");
        assert_eq!(phone.udid.as_deref(), Some("00008030-001A2B3C4D5E6F00"));
        assert_eq!(phone.name, "Matt’s iPhone");
        assert_eq!(phone.os_version.as_deref(), Some("18.6"));
        assert_eq!(phone.model.as_deref(), Some("iPhone 15 Pro"));
        assert_eq!(phone.developer_mode, DeveloperModeState::Enabled);
        assert_eq!(phone.pairing_state, PairingState::Paired);
        assert_eq!(phone.tunnel_state, TunnelState::Connected);
        assert_eq!(phone.transport_type, TransportType::Wired);
        assert!(phone.ready);
        assert_eq!(phone.issue, None);

        let spare = &devices[1];
        assert_eq!(spare.udid, None, "an invalid UDID is not carried");
        assert_eq!(spare.name, "Spare phone", "control characters are dropped");
        assert_eq!(spare.transport_type, TransportType::LocalNetwork);
        assert!(!spare.ready);
        assert!(spare.issue.as_deref().unwrap().contains("network"));
    }

    #[test]
    fn devicectl_output_without_devices_is_empty_not_an_error() {
        assert!(parse_devicectl_devices("").expect("empty").is_empty());
        assert!(parse_devicectl_devices("   \n").expect("blank").is_empty());
        assert!(
            parse_devicectl_devices(r#"{"result": {"devices": []}}"#)
                .expect("no devices")
                .is_empty()
        );
        assert!(
            parse_devicectl_devices(r#"{"info": {}}"#)
                .expect("no result")
                .is_empty()
        );
        assert!(parse_devicectl_devices("not json").is_err());
        assert!(
            parse_devicectl_devices(r#"{"result": {"devices": [{"identifier": "../x"}]}}"#)
                .is_err()
        );
    }

    #[test]
    fn guest_device_readiness_explains_trust_developer_mode_and_transport() {
        assert_eq!(
            device_issue(
                TransportType::Wired,
                PairingState::Paired,
                DeveloperModeState::Enabled
            ),
            None
        );
        assert!(
            device_issue(
                TransportType::Wired,
                PairingState::Unpaired,
                DeveloperModeState::Unknown
            )
            .unwrap()
            .contains("Trust")
        );
        assert!(
            device_issue(
                TransportType::Wired,
                PairingState::Paired,
                DeveloperModeState::Disabled
            )
            .unwrap()
            .contains("Developer Mode")
        );
        assert!(
            device_issue(
                TransportType::LocalNetwork,
                PairingState::Paired,
                DeveloperModeState::Enabled
            )
            .unwrap()
            .contains("network")
        );
        assert_eq!(
            DeveloperModeState::from_devicectl(Some("something-new")),
            DeveloperModeState::Unknown
        );
        assert_eq!(TunnelState::from_devicectl(None), TunnelState::Unknown);
    }

    #[test]
    fn the_device_list_script_is_fixed_and_quotes_the_developer_directory() {
        let script = device_list_script("builder");
        assert!(script.contains(
            "export DEVELOPER_DIR='/Users/builder/Applications/Xcode.app/Contents/Developer'"
        ));
        assert!(script.contains("devicectl list devices --json-output"));
        assert!(script.contains("--timeout 10"));
        assert!(script.contains("/bin/rm -f"));
    }
}
