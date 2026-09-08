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

use crate::signing::install_signing_helper;
use crate::{
    ProviderError, TrackedCommand, clean_output, guest_ssh_command, shell_single_quote,
    valid_device_udid, valid_profile_uuid, validate_guest_operation,
};
use ts_rs::TS;

/// One `devicectl` entry is a few kilobytes of capabilities; a handful of phones fits well
/// inside a mebibyte, and anything larger is not a device list.
const DEVICE_LIST_MAX_BYTES: usize = 1024 * 1024;
const DEVICE_LIST_TIMEOUT_SECONDS: u32 = 10;
/// Opening a phone's tunnel for its details takes a few seconds the first time.
const DEVICE_DETAILS_TIMEOUT_SECONDS: u32 = 20;
/// Long enough to unlock the phone and tap Trust.
const DEVICE_PAIR_TIMEOUT_SECONDS: u32 = 90;
const NAME_MAX_CHARS: usize = 128;
const MODEL_MAX_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperModeState {
    Enabled,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum PairingState {
    Paired,
    Unpaired,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    Connected,
    Disconnected,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
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

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct DevicectlOutput {
    #[serde(default)]
    result: Option<DevicectlResult>,
}

#[derive(Debug, Default, Deserialize, TS)]
#[ts(export)]
struct DevicectlResult {
    #[serde(default)]
    devices: Vec<DevicectlDevice>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
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

#[derive(Debug, Default, Deserialize, TS)]
#[ts(export)]
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

#[derive(Debug, Default, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
struct DeviceProperties {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    os_version_number: Option<String>,
    #[serde(default)]
    developer_mode_status: Option<String>,
}

#[derive(Debug, Default, Deserialize, TS)]
#[ts(export)]
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

/// Separates the listing from the per-phone detail reports that follow it.
pub(crate) const DEVICE_DETAILS_MARKER: &str = "__BUILDBRIDGE_DEVICE_DETAILS__";

/// One `device info details` report: the same shape as a listing entry, under `result`.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
struct DeviceDetailsEnvelope {
    #[serde(default)]
    result: Option<DevicectlDevice>,
}

/// The listing, with each phone's details merged over it where a report followed. The listing
/// reports Developer Mode as unknown until something has opened the phone's tunnel; the
/// details report is what opens it.
pub(crate) fn parse_devicectl_devices(output: &str) -> Result<Vec<GuestDevice>, ProviderError> {
    let mut chunks = output.split(DEVICE_DETAILS_MARKER);
    let listing = chunks.next().unwrap_or_default();
    let mut devices = parse_devicectl_listing(listing)?;
    for chunk in chunks {
        let Ok(envelope) = serde_json::from_str::<DeviceDetailsEnvelope>(chunk.trim()) else {
            continue;
        };
        let Some(details) = envelope.result else {
            continue;
        };
        let Some(device) = devices
            .iter_mut()
            .find(|device| device.identifier == details.identifier)
        else {
            continue;
        };
        let properties = details.device_properties.unwrap_or_default();
        let connection = details.connection_properties.unwrap_or_default();
        if properties.developer_mode_status.is_some() {
            device.developer_mode =
                DeveloperModeState::from_devicectl(properties.developer_mode_status.as_deref());
        }
        if connection.tunnel_state.is_some() {
            device.tunnel_state = TunnelState::from_devicectl(connection.tunnel_state.as_deref());
        }
        if connection.pairing_state.is_some() {
            device.pairing_state =
                PairingState::from_devicectl(connection.pairing_state.as_deref());
        }
        device.issue = device_issue(
            device.transport_type,
            device.pairing_state,
            device.developer_mode,
        )
        .map(str::to_string);
        device.ready = device.issue.is_none();
    }

    Ok(devices)
}

/// Reads `devicectl list devices --json-output` into devices. An empty document is an empty
/// list — `devicectl` writes nothing when it fails, and a failed listing is not an error the
/// person can act on beyond refreshing.
fn parse_devicectl_listing(json: &str) -> Result<Vec<GuestDevice>, ProviderError> {
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

/// Lists the phones, then asks for each one's details, which is what opens the tunnel
/// Developer Mode is read over. The identifiers come from devicectl's own JSON and are still
/// checked for shape before they are handed back to it. The Python is one line on purpose:
/// a newline cannot survive the quoting between here and the guest.
pub(crate) fn device_list_script(username: &str) -> String {
    let developer_dir = format!("/Users/{username}/Applications/Xcode.app/Contents/Developer");

    format!(
        "set -u; export DEVELOPER_DIR={}; out=$(/usr/bin/mktemp /tmp/buildbridge-devices.XXXXXX) || exit 1; /usr/bin/xcrun devicectl list devices --json-output \"$out\" --timeout {DEVICE_LIST_TIMEOUT_SECONDS} >/dev/null 2>&1 || /usr/bin/true; /bin/cat \"$out\"; for id in $(/usr/bin/python3 -c 'import json,re,sys; d=json.load(open(sys.argv[1])); [print(i) for i in (str(r.get(\"identifier\",\"\")) for r in d.get(\"result\",{{}}).get(\"devices\",[])) if re.fullmatch(r\"[0-9A-Fa-f-]{{36}}\", i)]' \"$out\" 2>/dev/null); do det=$(/usr/bin/mktemp /tmp/buildbridge-device.XXXXXX) || continue; /usr/bin/xcrun devicectl device info details --device \"$id\" --json-output \"$det\" --timeout {DEVICE_DETAILS_TIMEOUT_SECONDS} >/dev/null 2>&1 || /usr/bin/true; /usr/bin/printf '\\n%s\\n' {DEVICE_DETAILS_MARKER}; /bin/cat \"$det\"; /bin/rm -f \"$det\"; done; /bin/rm -f \"$out\"",
        shell_single_quote(&developer_dir)
    )
}

/// The phones the guest can see right now.
/// The CoreDevice pairing. Tapping Trust on the phone establishes only the classic lockdown
/// pairing; `devicectl` and Xcode need this second one, and the command raises the Trust prompt
/// on the phone itself when that has not happened yet, so it is the one action the trust rung
/// needs. Blocks until the phone answers or the timeout passes.
pub(crate) fn device_pair_script(username: &str, udid: &str) -> String {
    let developer_dir = format!("/Users/{username}/Applications/Xcode.app/Contents/Developer");

    format!(
        "set -u; export DEVELOPER_DIR={}; /usr/bin/xcrun devicectl manage pair --device {} --timeout {DEVICE_PAIR_TIMEOUT_SECONDS} 2>&1 | /usr/bin/tail -n 8",
        shell_single_quote(&developer_dir),
        shell_single_quote(udid)
    )
}

/// What opening Safari for the Web Inspector found in the guest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SafariInspectorResult {
    /// Safari's Develop menu is on, so the phone and its inspectable pages appear under it.
    pub develop_menu_enabled: bool,
    /// Safari was running with the menu off and was reopened so the menu appears.
    pub safari_restarted: bool,
}

const SAFARI_INSPECTOR_MAX_BYTES: usize = 64 * 1024;
const SAFARI_INSPECTOR_MARKER: &str = "__BUILDBRIDGE_SAFARI__";

/// Turns Safari's Develop menu on and opens Safari in the guest's graphical session. The
/// preference write is best effort: macOS protects Safari's preferences and may refuse it over
/// SSH, so the read-back afterwards is what counts. Safari is reopened only when the menu was
/// just turned on, since a running Safari shows the menu after a restart and not before.
pub(crate) fn safari_inspector_script() -> String {
    let read = "/usr/bin/defaults read com.apple.Safari IncludeDevelopMenu 2>/dev/null || /usr/bin/printf 0";
    let writes = [
        "/usr/bin/defaults write com.apple.Safari IncludeDevelopMenu -bool true",
        "/usr/bin/defaults write com.apple.Safari.SandboxBroker ShowDevelopMenu -bool true",
        "/usr/bin/defaults write com.apple.Safari WebKitDeveloperExtrasEnabledPreferenceKey -bool true",
        "/usr/bin/defaults write com.apple.Safari com.apple.Safari.ContentPageGroupIdentifier.WebKit2DeveloperExtrasEnabled -bool true",
        "/usr/bin/defaults write -g WebKitDeveloperExtras -bool true",
    ]
    .iter()
    .map(|write| format!("{write} >/dev/null 2>&1 || true; "))
    .collect::<String>();

    format!(
        "set -u; was_on=$({read}); {writes}now_on=$({read}); restarted=0; \
         if [ \"$now_on\" = 1 ] && [ \"$was_on\" != 1 ] && /usr/bin/pgrep -xq Safari; then /usr/bin/killall Safari >/dev/null 2>&1 || true; /bin/sleep 2; restarted=1; fi; \
         if /usr/bin/open -a Safari >/dev/null 2>&1; then opened=1; else opened=0; fi; \
         /usr/bin/printf '{SAFARI_INSPECTOR_MARKER} develop=%s restarted=%s opened=%s\\n' \"$now_on\" \"$restarted\" \"$opened\""
    )
}

/// Reads the script's one marker line. Anything else in the output is noise from `defaults`.
pub(crate) fn parse_safari_inspector_output(
    output: &str,
) -> Result<SafariInspectorResult, ProviderError> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(SAFARI_INSPECTOR_MARKER))
        .ok_or_else(|| {
            ProviderError::GuestBridge("the guest did not report whether Safari opened".to_string())
        })?;
    let flag = |key: &str| {
        line.split_whitespace()
            .find_map(|part| {
                part.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix('='))
            })
            .is_some_and(|value| value == "1")
    };
    if !flag("opened") {
        return Err(ProviderError::GuestBridge(
            "macOS could not open Safari; Safari needs the graphical session, so log in on the console window first".to_string(),
        ));
    }

    Ok(SafariInspectorResult {
        develop_menu_enabled: flag("develop"),
        safari_restarted: flag("restarted"),
    })
}

/// Opens Safari in the guest ready for Web Inspector: its Develop menu on where macOS allows
/// it to be set from here, and the app running on the phone then appears under Develop.
pub fn open_safari_web_inspector(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<SafariInspectorResult, ProviderError> {
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    let output = run_guest_command_capped(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &safari_inspector_script(),
        SAFARI_INSPECTOR_MAX_BYTES,
    )?;

    parse_safari_inspector_output(&output)
}

/// Pairs the guest with one phone, then lists again so the caller sees the result.
pub fn pair_guest_device(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    udid: &str,
) -> Result<Vec<GuestDevice>, ProviderError> {
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    if !crate::valid_device_udid(udid) {
        return Err(ProviderError::GuestBridge(
            "the device identifier is not a UDID".to_string(),
        ));
    }
    let output = run_guest_command_capped(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &device_pair_script(username, udid),
        DEVICE_LIST_MAX_BYTES,
    )?;
    if !output.contains("Paired with device") {
        let reason = clean_output(output.as_bytes());
        return Err(ProviderError::GuestBridge(if reason.is_empty() {
            "the phone did not accept the pairing; unlock it and tap Trust when it asks".to_string()
        } else {
            format!("the phone did not accept the pairing: {reason}")
        }));
    }

    list_guest_devices(ssh_port, username, identity_path, known_hosts_path)
}

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
    #[test]
    fn the_safari_script_reads_the_menu_back_and_restarts_safari_only_when_it_just_turned_on() {
        let script = super::safari_inspector_script();
        assert!(script.contains("defaults read com.apple.Safari IncludeDevelopMenu"));
        assert!(script.contains("IncludeDevelopMenu -bool true >/dev/null 2>&1 || true"));
        assert!(
            script.contains(
                r#"[ "$now_on" = 1 ] && [ "$was_on" != 1 ] && /usr/bin/pgrep -xq Safari"#
            )
        );
        assert!(script.contains("/usr/bin/open -a Safari"));
        assert!(script.ends_with(r#""$now_on" "$restarted" "$opened""#));
    }

    #[test]
    fn the_safari_report_is_read_from_its_marker_line_only() {
        let parsed = super::parse_safari_inspector_output(
            "2026-09-04 defaults[512:9] Could not write domain com.apple.Safari\n__BUILDBRIDGE_SAFARI__ develop=1 restarted=1 opened=1\n",
        )
        .expect("a marker line parses");
        assert_eq!(
            parsed,
            super::SafariInspectorResult {
                develop_menu_enabled: true,
                safari_restarted: true,
            }
        );

        let refused = super::parse_safari_inspector_output(
            "__BUILDBRIDGE_SAFARI__ develop=0 restarted=0 opened=1",
        )
        .expect("a refused preference still opens Safari");
        assert!(!refused.develop_menu_enabled);

        let no_session = super::parse_safari_inspector_output(
            "__BUILDBRIDGE_SAFARI__ develop=1 restarted=0 opened=0",
        )
        .expect_err("Safari needs a graphical session");
        assert!(
            no_session
                .to_string()
                .contains("log in on the console window")
        );
        assert!(super::parse_safari_inspector_output("nothing").is_err());
    }

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
    fn a_debug_identifier_the_profile_does_not_cover_falls_back_to_the_approved_one() {
        let profile = "TEAM123456.com.example.app";
        assert_eq!(
            device_bundle_identifier("com.example.app", "com.example.app", profile).unwrap(),
            ("com.example.app".to_string(), None)
        );
        assert_eq!(
            device_bundle_identifier("com.example.app.debug", "com.example.app", profile).unwrap(),
            (
                "com.example.app".to_string(),
                Some("com.example.app.debug".to_string())
            )
        );
        // A wildcard profile covers the Debug identifier and it is kept.
        assert_eq!(
            device_bundle_identifier("com.example.app.debug", "com.example.app", "TEAM123456.*")
                .unwrap(),
            ("com.example.app.debug".to_string(), None)
        );
        let error = device_bundle_identifier("com.other.app.debug", "com.other.app", profile)
            .expect_err("neither covered");
        assert!(error.to_string().contains("com.other.app.debug"));
        assert!(error.to_string().contains("does not cover"));

        let plain = device_signing_xcconfig("App", "TEAM123456", "ABCD", "uuid", None);
        assert!(!plain.contains("PRODUCT_BUNDLE_IDENTIFIER"));
        let renamed =
            device_signing_xcconfig("App", "TEAM123456", "ABCD", "uuid", Some("com.example.app"));
        assert!(renamed.contains("BUILDBRIDGE_BUNDLE_App = com.example.app"));
        // Only the app target: every other target inherits its own identifier.
        assert!(renamed.contains(
            "PRODUCT_BUNDLE_IDENTIFIER = $(BUILDBRIDGE_BUNDLE_$(TARGET_NAME):default=$(inherited))"
        ));
    }

    #[test]
    fn the_listing_script_keeps_python_on_one_line_and_lets_printf_make_the_newlines() {
        let script = device_list_script("john");
        // The Python program is single-quoted for the guest shell, so any newline escape in it
        // would reach Python as two characters and be a syntax error.
        let program_start = script.find("python3 -c '").expect("python") + "python3 -c '".len();
        let program_end =
            script[program_start..].find('\'').expect("closing quote") + program_start;
        let program = &script[program_start..program_end];
        assert!(
            !program.contains('\\'),
            "no escapes inside the Python: {program}"
        );
        assert!(program.contains("json.load(open(sys.argv[1]))"));
        assert!(program.contains("re.fullmatch"));
        // printf turns its own \n escapes into newlines around the marker; that is the one
        // place a backslash-n belongs.
        assert!(script.contains("/usr/bin/printf '\\n%s\\n' __BUILDBRIDGE_DEVICE_DETAILS__"));
        assert!(script.contains("devicectl device info details --device \"$id\""));
    }

    #[test]
    fn a_details_report_fills_in_developer_mode_and_the_tunnel_the_listing_left_unknown() {
        let listing = r#"{"result":{"devices":[{"identifier":"E3F1A2B4-5C6D-4E7F-8A9B-0C1D2E3F4A5B","connectionProperties":{"pairingState":"paired","transportType":"wired","tunnelState":"disconnected"},"deviceProperties":{"name":"Matt’s iPhone","osVersionNumber":"26.5"},"hardwareProperties":{"platform":"iOS","udid":"00008030-000614240A51802E"}}]}}"#;
        let details = r#"{"result":{"identifier":"E3F1A2B4-5C6D-4E7F-8A9B-0C1D2E3F4A5B","connectionProperties":{"pairingState":"paired","transportType":"wired","tunnelState":"connected"},"deviceProperties":{"name":"Matt’s iPhone","developerModeStatus":"enabled"}}}"#;
        let before = parse_devicectl_devices(listing).expect("listing");
        assert_eq!(before[0].developer_mode, DeveloperModeState::Unknown);
        assert!(!before[0].ready);

        let merged =
            parse_devicectl_devices(&format!("{listing}\n{DEVICE_DETAILS_MARKER}\n{details}\n"))
                .expect("merged");
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].developer_mode, DeveloperModeState::Enabled);
        assert_eq!(merged[0].tunnel_state, TunnelState::Connected);
        assert!(merged[0].ready, "{:?}", merged[0].issue);

        let stray = parse_devicectl_devices(&format!(
            "{listing}\n{DEVICE_DETAILS_MARKER}\nnot json\n{DEVICE_DETAILS_MARKER}\n{{\"result\":{{\"identifier\":\"0A1B2C3D-4E5F-4061-8273-8495A6B7C8D9\"}}}}\n"
        ))
        .expect("stray");
        assert_eq!(stray[0].developer_mode, DeveloperModeState::Unknown);
    }

    #[test]
    fn the_pairing_script_is_fixed_text_around_a_quoted_udid() {
        let script = device_pair_script("john", "00008030-000614240A51802E");
        assert!(script.contains(
            "/usr/bin/xcrun devicectl manage pair --device '00008030-000614240A51802E' --timeout 90"
        ));
        assert!(script.contains(
            "export DEVELOPER_DIR='/Users/john/Applications/Xcode.app/Contents/Developer'"
        ));
        assert!(
            !script.contains("$("),
            "nothing here is computed in the guest shell"
        );
    }

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

// ---------------------------------------------------------------------------------------------
// Building, installing and launching on one phone.
// ---------------------------------------------------------------------------------------------

use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{
    APPLE_BUILD_DIAGNOSTIC_LINES, APPLE_BUILD_OUTPUT_TAIL_LINES, AppleArchiveProgress,
    GuestEnvFiles, ProjectLayout, ProjectVersion, ProvisioningProfileSummary,
    SIGNING_KEYCHAIN_NAME, apple_archive_signing_xcconfig, apple_build_log_is_diagnostic,
    apple_version_xcconfig, build_setting_value, guest_env_source, ios_container_args,
    ios_container_path, profile_allows_bundle, rebuild_web_assets_with_env, run_guest_command,
    sanitize_build_log_line, stream_bytes_to_guest, valid_release_value, validate_apple_version,
    validate_layout, validate_signing_target, write_secret_frame,
};

const DEVICE_RUN_JOB: &str = "apple-device-run";
const CONSOLE_TAIL_LINES: usize = 400;
const CONSOLE_BATCH_LINES: usize = 200;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AppleDeviceRunPhase {
    Preparing,
    BuildingWebAssets,
    ResolvingTarget,
    Building,
    Verifying,
    Installing,
    Launching,
    Running,
    Completed,
}

/// Progress carries every console line since the last event rather than the latest one: an
/// app console must not drop lines between ticks the way filtered build output may.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AppleDeviceRunProgress {
    pub phase: AppleDeviceRunPhase,
    #[ts(type = "number")]
    pub elapsed_seconds: u64,
    pub detail: String,
    pub log_lines: Vec<String>,
}

/// How the console session ended. Each of these is a run that happened, not a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleEnd {
    Stopped,
    Exited,
    Disconnected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AppleDeviceRunResult {
    pub device: GuestDevice,
    /// The identifier the app was signed and installed under.
    pub bundle_identifier: String,
    /// The project's own Debug identifier, when the build was signed under the approved one
    /// instead because only that one has a development profile.
    #[serde(default)]
    pub project_bundle_identifier: Option<String>,
    pub app_path: String,
    pub marketing_version: String,
    pub build_number: String,
    pub provisioning_profile_uuid: String,
    #[ts(type = "number")]
    pub installed_at_epoch_seconds: u64,
    pub console_end: ConsoleEnd,
    pub exit_status: Option<i32>,
    pub reattached: bool,
    pub build_tail: Vec<String>,
    pub console_tail: Vec<String>,
}

/// What a device build signs with: the development identity and the profile that lists the
/// phone, in the same keychain the archive uses.
pub struct DeviceSigning<'a> {
    pub keychain_path: &'a str,
    pub identity_sha1: &'a str,
    pub development_team: &'a str,
    pub bundle_identifier: &'a str,
    pub profile: &'a ProvisioningProfileSummary,
}

fn device_progress(
    phase: AppleDeviceRunPhase,
    started_at: Instant,
    detail: &str,
    log_lines: Vec<String>,
) -> AppleDeviceRunProgress {
    AppleDeviceRunProgress {
        phase,
        elapsed_seconds: started_at.elapsed().as_secs(),
        detail: detail.to_string(),
        log_lines,
    }
}

fn device_phase_detail(phase: AppleDeviceRunPhase) -> &'static str {
    match phase {
        AppleDeviceRunPhase::Preparing => "Preparing the Debug recipe for the phone.",
        AppleDeviceRunPhase::BuildingWebAssets => "Rebuilding the web assets with the environment.",
        AppleDeviceRunPhase::ResolvingTarget => "Reading the Debug build settings.",
        AppleDeviceRunPhase::Building => "Compiling the Debug configuration for the phone.",
        AppleDeviceRunPhase::Verifying => "Verifying the built app's signature and profile.",
        AppleDeviceRunPhase::Installing => "Installing the app on the phone.",
        AppleDeviceRunPhase::Launching => "Launching the app.",
        AppleDeviceRunPhase::Running => "The app is running; its console streams here.",
        AppleDeviceRunPhase::Completed => "The console session ended.",
    }
}

/// The Debug build target: its name, the bundle identifier it will carry, and where the
/// product lands. The Debug bundle identifier can differ from the release one, which is why it
/// is checked against the profile rather than assumed.
pub(crate) struct DeviceBuildTarget {
    target: String,
    bundle_identifier: String,
    product_path: String,
}

/// Which identifier the Debug build is signed under. A project often gives its Debug
/// configuration a suffixed identifier so both builds can sit on one phone, but the development
/// profile is made for the approved identifier — the one the project was verified and the
/// archive signs with — and Apple profiles are per App ID. When the profile covers the Debug
/// identifier it is used as it is; when it covers only the approved one, the build is signed
/// under that and the project's own identifier is reported back; otherwise the mismatch is
/// named in full.
pub(crate) fn device_bundle_identifier(
    debug_identifier: &str,
    approved_identifier: &str,
    profile_application_identifier: &str,
) -> Result<(String, Option<String>), ProviderError> {
    if profile_allows_bundle(profile_application_identifier, debug_identifier) {
        return Ok((debug_identifier.to_string(), None));
    }
    if profile_allows_bundle(profile_application_identifier, approved_identifier) {
        return Ok((
            approved_identifier.to_string(),
            Some(debug_identifier.to_string()),
        ));
    }
    Err(ProviderError::GuestBridge(format!(
        "the Debug configuration builds bundle identifier {debug_identifier}, which the development profile for {approved_identifier} does not cover"
    )))
}

/// The archive's target-scoped signing settings, plus — when the Debug build is signed under
/// the approved identifier — that identifier for the app target alone. Every other target keeps
/// its own: a bare override would rename every Pod framework too.
pub(crate) fn device_signing_xcconfig(
    target: &str,
    development_team: &str,
    identity_sha1: &str,
    profile_uuid: &str,
    bundle_override: Option<&str>,
) -> String {
    let mut xcconfig =
        apple_archive_signing_xcconfig(target, development_team, identity_sha1, profile_uuid);
    if let Some(bundle) = bundle_override {
        xcconfig.push_str(&format!(
            "BUILDBRIDGE_BUNDLE_{target} = {bundle}\n\
PRODUCT_BUNDLE_IDENTIFIER = $(BUILDBRIDGE_BUNDLE_$(TARGET_NAME):default=$(inherited))\n"
        ));
    }

    xcconfig
}

/// The bundle identifier the App target's Debug configuration builds, read from the guest's
/// build settings. A project often gives Debug its own, suffixed identifier so a debug build
/// can sit beside the store build on one phone; the device step registers that one at Apple.
pub fn resolve_debug_bundle_identifier(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    layout: &ProjectLayout,
) -> Result<String, ProviderError> {
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    validate_layout(layout)?;
    let scheme: &str = layout
        .ios
        .as_ref()
        .map(|ios| ios.scheme.as_str())
        .ok_or_else(|| {
            ProviderError::GuestBridge("the approved project has no iOS project".to_string())
        })?;
    let guest_home = format!("/Users/{username}");
    let xcodebuild =
        format!("{guest_home}/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild");
    let workspace_root = format!("{guest_home}/BuildBridge/workspaces/active");
    let container_args = ios_container_args(layout, &workspace_root);
    let derived_data = format!("{workspace_root}/.buildbridge/DerivedData");
    let settings = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!(
            "set -o pipefail; {} {} -scheme {} -configuration Debug -destination 'generic/platform=iOS' -derivedDataPath {} -showBuildSettings | /usr/bin/awk '$1 == \"TARGET_NAME\" || $1 == \"PRODUCT_BUNDLE_IDENTIFIER\" || $1 == \"CODESIGNING_FOLDER_PATH\" {{ print }}'",
            shell_single_quote(&xcodebuild),
            container_args,
            shell_single_quote(scheme),
            shell_single_quote(&derived_data),
        ),
    )?;

    Ok(parse_device_build_target(&settings, &derived_data)?.bundle_identifier)
}

pub(crate) fn parse_device_build_target(
    output: &str,
    derived_data: &str,
) -> Result<DeviceBuildTarget, ProviderError> {
    let target = build_setting_value(output, "TARGET_NAME").ok_or_else(|| {
        ProviderError::GuestBridge(
            "Xcode did not return the application target for the selected scheme".to_string(),
        )
    })?;
    let bundle_identifier =
        build_setting_value(output, "PRODUCT_BUNDLE_IDENTIFIER").ok_or_else(|| {
            ProviderError::GuestBridge(
                "Xcode did not return the application bundle identifier for the selected scheme"
                    .to_string(),
            )
        })?;
    let product_path = build_setting_value(output, "CODESIGNING_FOLDER_PATH").ok_or_else(|| {
        ProviderError::GuestBridge(
            "Xcode did not return where the Debug product is built".to_string(),
        )
    })?;
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
    validate_signing_target("TEAM", bundle_identifier)?;
    if !product_path.starts_with(derived_data)
        || !product_path.ends_with(".app")
        || product_path.contains("..")
        || product_path.chars().any(char::is_control)
        || product_path.len() > 1024
    {
        return Err(ProviderError::GuestBridge(
            "Xcode reported a Debug product path outside the build directory".to_string(),
        ));
    }

    Ok(DeviceBuildTarget {
        target: target.to_string(),
        bundle_identifier: bundle_identifier.to_string(),
        product_path: product_path.to_string(),
    })
}

pub(crate) struct DeviceAppInspection {
    bundle_identifier: String,
    marketing_version: String,
    build_number: String,
    profile_uuid: String,
}

/// `__BUILDBRIDGE_DEVICE_APP__\t<bundle>\t<version>\t<build>\t<profile uuid>\t<yes|no>`; the last
/// field says whether `get-task-allow` is set, which a development profile must give.
pub(crate) fn parse_device_app_inspection(
    output: &str,
    expected_profile_uuid: &str,
) -> Result<DeviceAppInspection, ProviderError> {
    let values = output
        .lines()
        .find_map(|line| line.strip_prefix("__BUILDBRIDGE_DEVICE_APP__\t"))
        .ok_or_else(|| {
            ProviderError::GuestBridge("macOS returned incomplete app metadata".to_string())
        })?;
    let fields = values.split('\t').collect::<Vec<_>>();
    let [
        bundle_identifier,
        marketing_version,
        build_number,
        profile_uuid,
        task_allow,
    ] = fields[..]
    else {
        return Err(ProviderError::GuestBridge(
            "macOS returned invalid app metadata".to_string(),
        ));
    };
    validate_signing_target("TEAM", bundle_identifier)?;
    if !valid_release_value(marketing_version) || !valid_release_value(build_number) {
        return Err(ProviderError::GuestBridge(
            "macOS returned invalid release version metadata".to_string(),
        ));
    }
    if !valid_profile_uuid(profile_uuid)
        || !profile_uuid.eq_ignore_ascii_case(expected_profile_uuid)
    {
        return Err(ProviderError::GuestBridge(format!(
            "the built app embeds profile {profile_uuid}, not the development profile {expected_profile_uuid}"
        )));
    }
    if task_allow != "yes" {
        return Err(ProviderError::GuestBridge(
            "the built app is not debuggable (get-task-allow is missing), so it was not signed with a development profile"
                .to_string(),
        ));
    }

    Ok(DeviceAppInspection {
        bundle_identifier: bundle_identifier.to_string(),
        marketing_version: marketing_version.to_string(),
        build_number: build_number.to_string(),
        profile_uuid: profile_uuid.to_ascii_uppercase(),
    })
}

fn device_app_inspection_command(app_path: &str) -> String {
    let app = shell_single_quote(app_path);
    format!(
        "set -eu; app={app}; /usr/bin/codesign --verify --deep --strict --verbose=2 \"$app\"; bundle=$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - \"$app/Info.plist\"); version=$(/usr/bin/plutil -extract CFBundleShortVersionString raw -o - \"$app/Info.plist\"); build=$(/usr/bin/plutil -extract CFBundleVersion raw -o - \"$app/Info.plist\"); profile_uuid=$(/usr/bin/security cms -D -i \"$app/embedded.mobileprovision\" | /usr/bin/plutil -extract UUID raw -o - -); if /usr/bin/codesign -d --entitlements :- \"$app\" 2>/dev/null | /usr/bin/tr -d '\\n\\t ' | /usr/bin/grep -q '<key>get-task-allow</key><true/>'; then task_allow=yes; else task_allow=no; fi; /usr/bin/printf '__BUILDBRIDGE_DEVICE_APP__\\t%s\\t%s\\t%s\\t%s\\t%s\\n' \"$bundle\" \"$version\" \"$build\" \"$profile_uuid\" \"$task_allow\""
    )
}

/// The guest job wrapper: one owner runs `body` detached from the SSH session and everyone
/// tails its log, so a dropped bridge can pick the run back up. Mirrors the smoke build's
/// wrapper; `meta` is a line the owner records and a reattaching client is handed back.
pub(crate) fn guest_job_script(job_name: &str, tools: &str, meta: &str, body: &str) -> String {
    format!(
        r#"set -u
exec 2>&1
job_root="{tools}/jobs"
job_state="$job_root/{job_name}"
job_meta={meta}
/bin/mkdir -p "$job_root"

job_owner=0
while /usr/bin/true; do
    if /bin/mkdir "$job_state" 2>/dev/null; then
        job_owner=1
        break
    fi
    if /bin/test -f "$job_state/status"; then
        break
    fi
    if /bin/test -f "$job_state/pid"; then
        job_pid=$(/bin/cat "$job_state/pid")
        if /bin/kill -0 "$job_pid" 2>/dev/null; then
            break
        fi
        /bin/rm -rf "$job_state"
        continue
    fi
    /bin/sleep 1
done

job_log="$job_state/output.log"
job_status="$job_state/status"
if /bin/test "$job_owner" -eq 1; then
    /usr/bin/printf '%s\n' "$job_meta" > "$job_state/meta"
    trap '' HUP
    (
        finish_job() {{
            worker_status=$?
            /usr/bin/printf '%s\n' "$worker_status" > "$job_status.incoming"
            /bin/mv "$job_status.incoming" "$job_status"
        }}
        trap finish_job EXIT
        set -eu
{body}
    ) > "$job_log" 2>&1 < /dev/null &
    job_pid=$!
    /usr/bin/printf '%s\n' "$job_pid" > "$job_state/pid"
    trap - HUP
else
    /usr/bin/printf '__BUILDBRIDGE_REATTACHED__:yes\n'
    /bin/cat "$job_state/meta" 2>/dev/null || /usr/bin/true
fi

next_line=1
while ! /bin/test -f "$job_status"; do
    if /bin/test -f "$job_log"; then
        line_count=$(/usr/bin/wc -l < "$job_log" | /usr/bin/tr -d ' ')
        if /bin/test "$line_count" -ge "$next_line"; then
            /usr/bin/sed -n "$next_line,$line_count p" "$job_log"
            next_line=$((line_count + 1))
        fi
    fi
    /bin/sleep 1
done
if /bin/test -f "$job_log"; then
    line_count=$(/usr/bin/wc -l < "$job_log" | /usr/bin/tr -d ' ')
    if /bin/test "$line_count" -ge "$next_line"; then
        /usr/bin/sed -n "$next_line,$line_count p" "$job_log"
    fi
fi
job_result=$(/bin/cat "$job_status")
exit "$job_result"
"#
    )
}

pub(crate) fn device_run_job_body(
    developer_dir: &str,
    device_identifier: &str,
    app_path: &str,
    bundle_identifier: &str,
) -> String {
    format!(
        "export DEVELOPER_DIR={developer_dir}\n\
phase() {{ /usr/bin/printf '__BUILDBRIDGE_PHASE__:%s\\n' \"$1\"; }}\n\
phase installing\n\
/usr/bin/xcrun devicectl device install app --device {device} {app}\n\
phase launching\n\
/usr/bin/xcrun devicectl device process launch --console --terminate-existing --device {device} {bundle}\n",
        developer_dir = shell_single_quote(developer_dir),
        device = shell_single_quote(device_identifier),
        app = shell_single_quote(app_path),
        bundle = shell_single_quote(bundle_identifier),
    )
}

/// What a reattaching client is handed about the run it did not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceRunMeta {
    pub device_identifier: String,
    pub bundle_identifier: String,
    pub app_path: String,
    pub profile_uuid: String,
    pub marketing_version: String,
    pub build_number: String,
    pub installed_at_epoch_seconds: u64,
}

const META_PREFIX: &str = "__BUILDBRIDGE_DEVICE_RUN_META__\t";

fn encode_device_run_meta(meta: &DeviceRunMeta) -> String {
    format!(
        "{META_PREFIX}{}\t{}\t{}\t{}\t{}\t{}\t{}",
        meta.device_identifier,
        meta.bundle_identifier,
        meta.app_path,
        meta.profile_uuid,
        meta.marketing_version,
        meta.build_number,
        meta.installed_at_epoch_seconds
    )
}

pub(crate) fn parse_device_run_meta(line: &str) -> Result<DeviceRunMeta, ProviderError> {
    let values = line.strip_prefix(META_PREFIX).ok_or_else(|| {
        ProviderError::GuestBridge("the guest returned an invalid run record".to_string())
    })?;
    let fields = values.split('\t').collect::<Vec<_>>();
    let [device, bundle, app, profile, version, build, installed] = fields[..] else {
        return Err(ProviderError::GuestBridge(
            "the guest returned an invalid run record".to_string(),
        ));
    };
    if !valid_profile_uuid(device)
        || !valid_profile_uuid(profile)
        || !valid_release_value(version)
        || !valid_release_value(build)
        || app.is_empty()
        || app.contains("..")
        || app.chars().any(char::is_control)
    {
        return Err(ProviderError::GuestBridge(
            "the guest returned an invalid run record".to_string(),
        ));
    }
    validate_signing_target("TEAM", bundle)?;
    let installed_at_epoch_seconds = installed.parse().map_err(|_| {
        ProviderError::GuestBridge("the guest returned an invalid run record".to_string())
    })?;

    Ok(DeviceRunMeta {
        device_identifier: device.to_string(),
        bundle_identifier: bundle.to_string(),
        app_path: app.to_string(),
        profile_uuid: profile.to_string(),
        marketing_version: version.to_string(),
        build_number: build.to_string(),
        installed_at_epoch_seconds,
    })
}

/// Lines from `devicectl` worth surfacing on their own: what it prints when the phone is
/// locked, unpaired, or not in Developer Mode.
pub(crate) fn apple_device_log_is_diagnostic(line: &str) -> bool {
    let lowered = line.to_ascii_lowercase();
    [
        "error:",
        "unable to",
        "locked",
        "not paired",
        "developer mode",
        "tunnel",
        "could not connect",
        "failed",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

fn push_bounded(lines: &mut Vec<String>, line: String, limit: usize) {
    lines.push(line);
    if lines.len() > limit {
        lines.remove(0);
    }
}

/// Runs the helper's `--device-build` mode, streaming the compiler's diagnostic lines.
#[allow(clippy::too_many_arguments)]
fn run_device_build_helper<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    helper_path: &str,
    keychain_path: &str,
    xcodebuild_path: &str,
    workspace_path: &str,
    scheme: &str,
    derived_data_path: &str,
    signing_settings_path: &str,
    env_source: &str,
    keychain_password: &str,
    started_at: Instant,
    on_progress: &mut F,
) -> Result<Vec<String>, ProviderError>
where
    F: FnMut(AppleDeviceRunProgress),
{
    let remote_command = format!(
        "{env_source}{} --device-build {} {} {} {} {} {} 2>&1",
        shell_single_quote(helper_path),
        shell_single_quote(keychain_path),
        shell_single_quote(xcodebuild_path),
        shell_single_quote(workspace_path),
        shell_single_quote(scheme),
        shell_single_quote(derived_data_path),
        shell_single_quote(signing_settings_path),
    );
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start the device build: {error}"))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not open protected build input".to_string())
    })?;
    write_secret_frame(&mut stdin, keychain_password)?;
    drop(stdin);
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture device build output".to_string())
    })?;
    let mut output_tail = Vec::new();
    let mut diagnostic_lines = Vec::new();
    let mut pending = Vec::new();
    let mut last_event = Instant::now() - Duration::from_secs(1);

    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| {
            ProviderError::GuestBridge(format!("could not read device build output: {error}"))
        })?;
        if let Some(marker) = line.strip_prefix("__BUILDBRIDGE_DEVICE_BUILD__:") {
            match marker {
                "building" | "complete" => {}
                _ => {
                    return Err(ProviderError::GuestBridge(
                        "macOS returned an unknown device build phase".to_string(),
                    ));
                }
            }
            continue;
        }
        let line = sanitize_build_log_line(&line);
        if line.is_empty() {
            continue;
        }
        let diagnostic = apple_build_log_is_diagnostic(&line);
        if diagnostic {
            push_bounded(
                &mut diagnostic_lines,
                line.clone(),
                APPLE_BUILD_DIAGNOSTIC_LINES,
            );
        }
        push_bounded(
            &mut output_tail,
            line.clone(),
            APPLE_BUILD_OUTPUT_TAIL_LINES,
        );
        pending.push(line);
        if diagnostic
            || last_event.elapsed() >= PROGRESS_INTERVAL
            || pending.len() >= CONSOLE_BATCH_LINES
        {
            on_progress(device_progress(
                AppleDeviceRunPhase::Building,
                started_at,
                device_phase_detail(AppleDeviceRunPhase::Building),
                std::mem::take(&mut pending),
            ));
            last_event = Instant::now();
        }
    }
    if !pending.is_empty() {
        on_progress(device_progress(
            AppleDeviceRunPhase::Building,
            started_at,
            device_phase_detail(AppleDeviceRunPhase::Building),
            pending,
        ));
    }

    let status = child.wait().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish the device build: {error}"))
    })?;
    if !status.success() {
        let context = if diagnostic_lines.is_empty() {
            output_tail
                .iter()
                .rev()
                .take(12)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            diagnostic_lines.join("\n")
        };
        return Err(ProviderError::GuestBridge(if context.is_empty() {
            "the device build failed while compiling".to_string()
        } else {
            format!("the device build failed while compiling:\n{context}")
        }));
    }

    Ok(output_tail)
}

/// Builds the Debug configuration signed for one phone, installs it, launches it, and streams
/// its console until the session ends. Any end after the app was launched is a result — the
/// person stopped it, the app exited, or the bridge dropped — not a failure.
#[allow(clippy::too_many_arguments)]
pub fn run_apple_device_build<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    signing: &DeviceSigning<'_>,
    layout: &ProjectLayout,
    device: &GuestDevice,
    keychain_password: &str,
    env: Option<&GuestEnvFiles>,
    version: Option<&ProjectVersion>,
    mut on_progress: F,
) -> Result<AppleDeviceRunResult, ProviderError>
where
    F: FnMut(AppleDeviceRunProgress),
{
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    validate_signing_target(signing.development_team, signing.bundle_identifier)?;
    validate_layout(layout)?;
    let scheme: &str = layout
        .ios
        .as_ref()
        .map(|ios| ios.scheme.as_str())
        .ok_or_else(|| {
            ProviderError::GuestBridge("the approved project has no iOS project".to_string())
        })?;
    if let Some(version) = version {
        validate_apple_version(version).map_err(ProviderError::GuestBridge)?;
    }
    // Set while the target is resolved; a reattached run does not resolve it again.
    let mut project_bundle_identifier: Option<String> = None;
    if keychain_password.is_empty() || keychain_password.len() > 512 {
        return Err(ProviderError::GuestBridge(
            "the signing keychain credential is missing or invalid".to_string(),
        ));
    }
    if signing.identity_sha1.len() != 40
        || !signing
            .identity_sha1
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(ProviderError::GuestBridge(
            "the provisioned development identity is invalid".to_string(),
        ));
    }
    if !valid_profile_uuid(&signing.profile.uuid) || !valid_profile_uuid(&device.identifier) {
        return Err(ProviderError::GuestBridge(
            "the development profile or the device identifier is invalid".to_string(),
        ));
    }
    let expected_keychain_path =
        format!("/Users/{username}/Library/Keychains/{SIGNING_KEYCHAIN_NAME}");
    if signing.keychain_path != expected_keychain_path {
        return Err(ProviderError::GuestBridge(
            "the provisioned signing keychain path is invalid".to_string(),
        ));
    }

    let started_at = Instant::now();
    let guest_home = format!("/Users/{username}");
    let guest_tools = format!("{guest_home}/.buildbridge/tools");
    let helper_source = format!("{guest_tools}/signing-helper.c");
    let helper_binary = format!("{guest_tools}/signing-helper");
    let developer_dir = format!("{guest_home}/Applications/Xcode.app/Contents/Developer");
    let xcodebuild = format!("{developer_dir}/usr/bin/xcodebuild");
    let workspace_root = format!("{guest_home}/BuildBridge/workspaces/active");
    let workspace = ios_container_path(layout, &workspace_root);
    let container_args = ios_container_args(layout, &workspace_root);
    let env_source = guest_env_source(&workspace_root);
    let derived_data = format!("{workspace_root}/.buildbridge/DerivedData");
    let operation_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let staging =
        format!("{guest_home}/Library/Caches/dev.buildbridge.desktop/device-run-{operation_id}");
    let signing_settings = format!("{staging}/Signing.xcconfig");
    let job_dir = format!("{guest_tools}/jobs/{DEVICE_RUN_JOB}");

    // A run that outlived a dropped bridge is picked back up rather than rebuilt.
    let job_state = run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!(
            "if /bin/test -f {pid} && /bin/kill -0 \"$(/bin/cat {pid})\" 2>/dev/null; then /usr/bin/printf running; else /usr/bin/printf idle; fi",
            pid = shell_single_quote(&format!("{job_dir}/pid"))
        ),
    )?;
    let reattached = job_state == "running";

    let mut build_tail = Vec::new();
    let mut meta = None;
    if !reattached {
        on_progress(device_progress(
            AppleDeviceRunPhase::Preparing,
            started_at,
            device_phase_detail(AppleDeviceRunPhase::Preparing),
            Vec::new(),
        ));
        run_guest_command(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &format!(
                "set -eu; /bin/mkdir -p {} {} {}; /bin/chmod 700 {}; /bin/rm -rf {} {}; /bin/mkdir -p {}; /bin/chmod 700 {}",
                shell_single_quote(&guest_tools),
                shell_single_quote(&format!(
                    "{guest_home}/Library/Caches/dev.buildbridge.desktop"
                )),
                shell_single_quote(&derived_data),
                shell_single_quote(&guest_tools),
                shell_single_quote(&staging),
                shell_single_quote(&job_dir),
                shell_single_quote(&staging),
                shell_single_quote(&staging),
            ),
        )?;

        let built = (|| {
            if let Some(env) = env {
                let mut adapt = |progress: AppleArchiveProgress| {
                    on_progress(AppleDeviceRunProgress {
                        phase: AppleDeviceRunPhase::BuildingWebAssets,
                        elapsed_seconds: progress.elapsed_seconds,
                        detail: progress.detail,
                        log_lines: progress.log_line.into_iter().collect(),
                    });
                };
                rebuild_web_assets_with_env(
                    env,
                    layout,
                    ssh_port,
                    username,
                    identity_path,
                    known_hosts_path,
                    &guest_home,
                    started_at,
                    &mut adapt,
                )?;
            }

            on_progress(device_progress(
                AppleDeviceRunPhase::ResolvingTarget,
                started_at,
                device_phase_detail(AppleDeviceRunPhase::ResolvingTarget),
                Vec::new(),
            ));
            let settings = run_guest_command(
                ssh_port,
                username,
                identity_path,
                known_hosts_path,
                &format!(
                    "set -o pipefail; {} {} -scheme {} -configuration Debug -destination 'generic/platform=iOS' -derivedDataPath {} -showBuildSettings | /usr/bin/awk '$1 == \"TARGET_NAME\" || $1 == \"PRODUCT_BUNDLE_IDENTIFIER\" || $1 == \"CODESIGNING_FOLDER_PATH\" {{ print }}'",
                    shell_single_quote(&xcodebuild),
                    container_args,
                    shell_single_quote(scheme),
                    shell_single_quote(&derived_data),
                ),
            )?;
            let mut target = parse_device_build_target(&settings, &derived_data)?;
            let (signed_as, project_identifier) = device_bundle_identifier(
                &target.bundle_identifier,
                signing.bundle_identifier,
                &signing.profile.application_identifier,
            )?;
            if let Some(project) = &project_identifier {
                let note = format!(
                    "Signing the Debug build as {signed_as}: the project's Debug identifier {project} has no development profile, and the app target alone is renamed for this build."
                );
                on_progress(device_progress(
                    AppleDeviceRunPhase::ResolvingTarget,
                    started_at,
                    &note,
                    vec![note.clone()],
                ));
            }
            target.bundle_identifier = signed_as;
            project_bundle_identifier = project_identifier;

            let mut xcconfig = device_signing_xcconfig(
                &target.target,
                signing.development_team,
                signing.identity_sha1,
                &signing.profile.uuid,
                project_bundle_identifier
                    .as_deref()
                    .map(|_| target.bundle_identifier.as_str()),
            );
            // The requested version rides in the same settings file as the signing, the way
            // the archive carries it; the built app is checked against it below.
            if let Some(version) = version {
                xcconfig.push_str(&apple_version_xcconfig(version));
                let note = format!("Building as version {}.", version.display());
                on_progress(device_progress(
                    AppleDeviceRunPhase::ResolvingTarget,
                    started_at,
                    &note,
                    vec![note.clone()],
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
                xcconfig.as_bytes(),
                ssh_port,
                username,
                identity_path,
                known_hosts_path,
                &signing_settings,
                "target-scoped signing settings",
            )?;

            on_progress(device_progress(
                AppleDeviceRunPhase::Building,
                started_at,
                device_phase_detail(AppleDeviceRunPhase::Building),
                Vec::new(),
            ));
            let tail = run_device_build_helper(
                ssh_port,
                username,
                identity_path,
                known_hosts_path,
                &helper_binary,
                signing.keychain_path,
                &xcodebuild,
                &workspace,
                scheme,
                &derived_data,
                &signing_settings,
                &env_source,
                keychain_password,
                started_at,
                &mut on_progress,
            )?;

            on_progress(device_progress(
                AppleDeviceRunPhase::Verifying,
                started_at,
                device_phase_detail(AppleDeviceRunPhase::Verifying),
                Vec::new(),
            ));
            let inspection_output = run_guest_command(
                ssh_port,
                username,
                identity_path,
                known_hosts_path,
                &device_app_inspection_command(&target.product_path),
            )?;
            let inspection =
                parse_device_app_inspection(&inspection_output, &signing.profile.uuid)?;
            if inspection.bundle_identifier != target.bundle_identifier {
                return Err(ProviderError::GuestBridge(format!(
                    "the built app carries bundle identifier {}, not {}",
                    inspection.bundle_identifier, target.bundle_identifier
                )));
            }
            if let Some(version) = version
                && (inspection.marketing_version != version.version
                    || inspection.build_number != version.build)
            {
                return Err(ProviderError::GuestBridge(format!(
                    "the built app reports version {} ({}), not the requested {}; the app's Info.plist must take CFBundleShortVersionString from MARKETING_VERSION and CFBundleVersion from CURRENT_PROJECT_VERSION for a version set here to reach it",
                    inspection.marketing_version,
                    inspection.build_number,
                    version.display()
                )));
            }

            Ok((tail, target, inspection))
        })();
        let _ = run_guest_command(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &format!("/bin/rm -rf {}", shell_single_quote(&staging)),
        );
        let (tail, target, inspection) = built?;
        build_tail = tail;
        meta = Some(DeviceRunMeta {
            device_identifier: device.identifier.clone(),
            bundle_identifier: inspection.bundle_identifier,
            app_path: target.product_path,
            profile_uuid: inspection.profile_uuid,
            marketing_version: inspection.marketing_version,
            build_number: inspection.build_number,
            installed_at_epoch_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
    }

    // The job body is fixed for a reattach: the guest already has it running, and the wrapper
    // only tails. An empty body keeps the script well-formed.
    let body = meta
        .as_ref()
        .map(|meta| {
            device_run_job_body(
                &developer_dir,
                &meta.device_identifier,
                &meta.app_path,
                &meta.bundle_identifier,
            )
        })
        .unwrap_or_else(|| "/usr/bin/true\n".to_string());
    let meta_line = meta
        .as_ref()
        .map(encode_device_run_meta)
        .unwrap_or_default();
    let script = guest_job_script(
        DEVICE_RUN_JOB,
        &guest_tools,
        &shell_single_quote(&meta_line),
        &body,
    );

    on_progress(device_progress(
        AppleDeviceRunPhase::Installing,
        started_at,
        device_phase_detail(AppleDeviceRunPhase::Installing),
        Vec::new(),
    ));
    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start the device session: {error}"))
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture the device session output".to_string())
    })?;

    let mut phase = AppleDeviceRunPhase::Installing;
    let mut console_tail = Vec::new();
    let mut diagnostic_lines = Vec::new();
    let mut pending = Vec::new();
    let mut last_event = Instant::now() - Duration::from_secs(1);
    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| {
            ProviderError::GuestBridge(format!("could not read the device session: {error}"))
        })?;
        if let Some(value) = line.strip_prefix("__BUILDBRIDGE_PHASE__:") {
            phase = match value {
                "installing" => AppleDeviceRunPhase::Installing,
                "launching" => AppleDeviceRunPhase::Launching,
                _ => {
                    return Err(ProviderError::GuestBridge(
                        "the guest returned an unknown device session phase".to_string(),
                    ));
                }
            };
            on_progress(device_progress(
                phase,
                started_at,
                device_phase_detail(phase),
                Vec::new(),
            ));
            last_event = Instant::now();
            continue;
        }
        if line == "__BUILDBRIDGE_REATTACHED__:yes" {
            on_progress(device_progress(
                phase,
                started_at,
                "Reattached to the session already running inside macOS.",
                Vec::new(),
            ));
            continue;
        }
        if line.starts_with(META_PREFIX) {
            if meta.is_none() {
                meta = Some(parse_device_run_meta(&line)?);
            }
            continue;
        }
        let line = sanitize_build_log_line(&line);
        if line.is_empty() {
            continue;
        }
        if phase == AppleDeviceRunPhase::Launching {
            phase = AppleDeviceRunPhase::Running;
            on_progress(device_progress(
                phase,
                started_at,
                device_phase_detail(phase),
                Vec::new(),
            ));
        }
        let diagnostic = apple_device_log_is_diagnostic(&line);
        if diagnostic {
            push_bounded(
                &mut diagnostic_lines,
                line.clone(),
                APPLE_BUILD_DIAGNOSTIC_LINES,
            );
        }
        push_bounded(&mut console_tail, line.clone(), CONSOLE_TAIL_LINES);
        pending.push(line);
        if diagnostic
            || last_event.elapsed() >= PROGRESS_INTERVAL
            || pending.len() >= CONSOLE_BATCH_LINES
        {
            on_progress(device_progress(
                phase,
                started_at,
                device_phase_detail(phase),
                std::mem::take(&mut pending),
            ));
            last_event = Instant::now();
        }
    }
    if !pending.is_empty() {
        on_progress(device_progress(
            phase,
            started_at,
            device_phase_detail(phase),
            pending,
        ));
    }

    let status = child.wait().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish the device session: {error}"))
    })?;
    let exit_code = status.code();
    if exit_code != Some(255) && exit_code.is_some() {
        let _ = run_guest_command(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &format!("/bin/rm -rf {}", shell_single_quote(&job_dir)),
        );
    }
    let Some(meta) = meta else {
        return Err(ProviderError::GuestBridge(
            "the guest did not report what it installed".to_string(),
        ));
    };
    if phase != AppleDeviceRunPhase::Running {
        let context = if diagnostic_lines.is_empty() {
            console_tail
                .iter()
                .rev()
                .take(12)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            diagnostic_lines.join("\n")
        };
        return Err(ProviderError::GuestBridge(if context.is_empty() {
            format!(
                "the device session failed while {}",
                device_phase_detail(phase).to_ascii_lowercase()
            )
        } else {
            format!(
                "the device session failed while {}\n{context}",
                device_phase_detail(phase).to_ascii_lowercase()
            )
        }));
    }
    let console_end = match exit_code {
        None => ConsoleEnd::Stopped,
        Some(255) => ConsoleEnd::Disconnected,
        Some(_) => ConsoleEnd::Exited,
    };
    on_progress(device_progress(
        AppleDeviceRunPhase::Completed,
        started_at,
        device_phase_detail(AppleDeviceRunPhase::Completed),
        Vec::new(),
    ));

    Ok(AppleDeviceRunResult {
        device: device.clone(),
        bundle_identifier: meta.bundle_identifier,
        project_bundle_identifier,
        app_path: meta.app_path,
        marketing_version: meta.marketing_version,
        build_number: meta.build_number,
        provisioning_profile_uuid: meta.profile_uuid,
        installed_at_epoch_seconds: meta.installed_at_epoch_seconds,
        console_end,
        exit_status: exit_code.filter(|code| *code != 255),
        reattached,
        build_tail,
        console_tail,
    })
}

#[cfg(test)]
mod run_tests {
    use super::*;

    #[test]
    fn device_build_targets_are_read_and_bounded() {
        let output = "    TARGET_NAME = App\n    PRODUCT_BUNDLE_IDENTIFIER = com.example.app.debug\n    CODESIGNING_FOLDER_PATH = /Users/b/BuildBridge/workspaces/active/.buildbridge/DerivedData/Build/Products/Debug-iphoneos/App.app\n";
        let derived = "/Users/b/BuildBridge/workspaces/active/.buildbridge/DerivedData";
        let target = parse_device_build_target(output, derived).expect("parses");
        assert_eq!(target.target, "App");
        assert_eq!(target.bundle_identifier, "com.example.app.debug");
        assert!(target.product_path.ends_with("/App.app"));

        let outside = output.replace(derived, "/tmp/elsewhere");
        assert!(parse_device_build_target(&outside, derived).is_err());
        let traversal = output.replace("/App.app", "/../App.app");
        assert!(parse_device_build_target(&traversal, derived).is_err());
        assert!(parse_device_build_target("TARGET_NAME = App\n", derived).is_err());
    }

    #[test]
    fn device_app_inspection_is_strictly_validated() {
        let uuid = "22222222-3333-4444-5555-666666666666";
        let good = format!("__BUILDBRIDGE_DEVICE_APP__\tcom.example.app\t3.2.0\t15\t{uuid}\tyes\n");
        let inspection = parse_device_app_inspection(&good, uuid).expect("parses");
        assert_eq!(inspection.bundle_identifier, "com.example.app");
        assert_eq!(inspection.marketing_version, "3.2.0");
        assert_eq!(inspection.profile_uuid, uuid);

        let other_profile = "__BUILDBRIDGE_DEVICE_APP__\tcom.example.app\t3.2.0\t15\t11111111-2222-3333-4444-555555555555\tyes\n";
        assert!(parse_device_app_inspection(other_profile, uuid).is_err());
        let not_debuggable = good.replace("\tyes", "\tno");
        assert!(parse_device_app_inspection(&not_debuggable, uuid).is_err());
        assert!(parse_device_app_inspection("nothing", uuid).is_err());
    }

    #[test]
    fn the_device_run_job_script_reattaches_and_quotes_every_guest_value() {
        let body = device_run_job_body(
            "/Users/b/Applications/Xcode.app/Contents/Developer",
            "E3F1A2B4-5C6D-4E7F-8A9B-0C1D2E3F4A5B",
            "/Users/b/BuildBridge/workspaces/active/.buildbridge/DerivedData/Build/Products/Debug-iphoneos/App.app",
            "com.example.app",
        );
        assert!(body.contains(
            "devicectl device install app --device 'E3F1A2B4-5C6D-4E7F-8A9B-0C1D2E3F4A5B'"
        ));
        assert!(body.contains("process launch --console --terminate-existing --device"));
        assert!(body.contains("'com.example.app'"));

        let meta = DeviceRunMeta {
            device_identifier: "E3F1A2B4-5C6D-4E7F-8A9B-0C1D2E3F4A5B".to_string(),
            bundle_identifier: "com.example.app".to_string(),
            app_path: "/Users/b/App.app".to_string(),
            profile_uuid: "22222222-3333-4444-5555-666666666666".to_string(),
            marketing_version: "3.2.0".to_string(),
            build_number: "15".to_string(),
            installed_at_epoch_seconds: 1_756_900_000,
        };
        let encoded = encode_device_run_meta(&meta);
        assert_eq!(parse_device_run_meta(&encoded).expect("round trip"), meta);
        assert!(parse_device_run_meta("__BUILDBRIDGE_DEVICE_RUN_META__\tbad").is_err());

        let script = guest_job_script(
            "apple-device-run",
            "/Users/b/.buildbridge/tools",
            "'meta'",
            &body,
        );
        assert!(script.contains("job_state=\"$job_root/apple-device-run\""));
        assert!(script.contains("__BUILDBRIDGE_REATTACHED__:yes"));
        assert!(script.contains("/bin/cat \"$job_state/meta\""));
        assert!(script.contains("trap '' HUP"));
    }

    #[test]
    fn device_console_diagnostics_are_the_lines_a_person_must_act_on() {
        assert!(apple_device_log_is_diagnostic(
            "ERROR: The device is locked."
        ));
        assert!(apple_device_log_is_diagnostic(
            "Unable to install: Developer Mode is disabled"
        ));
        assert!(!apple_device_log_is_diagnostic(
            "[App] scene did become active"
        ));
    }
}
