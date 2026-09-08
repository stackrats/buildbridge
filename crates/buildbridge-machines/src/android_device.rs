//! Install a retained APK on one explicitly selected host ADB device, launch its activity, and
//! stream the app's log until the session is stopped, the app exits, or the device leaves.

use super::*;
use std::sync::mpsc;

const ADB_PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const ADB_INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
const ADB_OUTPUT_LIMIT: u64 = 256 * 1024;
/// `am start -W` returns once the activity is drawn, so the process is there by then; a few
/// more looks cover a device that registers it a moment later.
const PROCESS_LOOKUPS_AFTER_LAUNCH: usize = 4;
const PROCESS_LOOKUP_INTERVAL: Duration = Duration::from_millis(500);
/// While the log streams, the app's process is looked for at this rate; gone means it exited.
const PROCESS_POLL_INTERVAL: Duration = Duration::from_secs(2);
const PROCESS_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const CONSOLE_TAIL_LINES: usize = 400;
const CONSOLE_BATCH_LINES: usize = 200;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// What the device log is narrowed to when the app's process id cannot be read: Capacitor,
/// the web view, and crashes, with everything else silent.
const FALLBACK_LOG_FILTERS: [&str; 5] = [
    "Capacitor:*",
    "Capacitor/Console:*",
    "chromium:*",
    "AndroidRuntime:E",
    "*:S",
];
const MISSING_ADB: &str = "Install Android SDK Platform-Tools on this computer and add adb to PATH, or set ANDROID_HOME to the Android SDK directory, then refresh devices.";
static RUNNING_DEVICES: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidDevice {
    pub serial: String,
    pub state: String,
    pub model: Option<String>,
    /// Where a ready phone sits beside this computer's networks, read when listing; `None`
    /// when it was not asked: a device that is not ready, or an emulator, which reaches this
    /// host through its own gateway whatever its address says.
    pub network: Option<AndroidDeviceNetwork>,
}

/// An app that calls an API served on this computer reaches it only from the same network.
/// The device step says so before a run rather than the app after one, as a Network Error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidDeviceNetwork {
    /// The phone's Wi-Fi or Ethernet IPv4 address with its prefix length, when it has one;
    /// `None` with Wi-Fi off, when mobile data is all the phone has.
    pub address: Option<String>,
    /// Whether that address shares a network with one of this computer's.
    pub on_host_network: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidDevices {
    pub available: bool,
    pub devices: Vec<AndroidDevice>,
    pub issue: Option<String>,
    /// This computer's own networks in CIDR form, the ones a phone joins to reach it, with
    /// loopback, link-local and container, VM and tunnel networks left out.
    pub host_networks: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AndroidDeviceRunPhase {
    Checking,
    Staging,
    Installing,
    Launching,
    Running,
    Completed,
}

/// Progress carries every log line since the last event rather than the latest one, as the
/// iPhone run's console does: an app log must not drop lines between ticks.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidDeviceRunProgress {
    pub phase: AndroidDeviceRunPhase,
    #[ts(type = "number")]
    pub elapsed_seconds: u64,
    pub detail: String,
    pub log_lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidDeviceRunResult {
    pub serial: String,
    /// The device's model as ADB lists it, when it says.
    pub model: Option<String>,
    pub application_id: String,
    pub sha256: String,
    pub installed: bool,
    pub launched: bool,
    /// The app's process on the device, when it could be read; the log was filtered to it.
    #[ts(type = "number | null")]
    pub pid: Option<u32>,
    #[ts(type = "number")]
    pub installed_at_epoch_seconds: u64,
    /// How the log session ended. Each of these is a run that happened, not a failure.
    pub console_end: ConsoleEnd,
    pub console_tail: Vec<String>,
}

fn android_progress(
    phase: AndroidDeviceRunPhase,
    started_at: Instant,
    detail: &str,
    log_lines: Vec<String>,
) -> AndroidDeviceRunProgress {
    AndroidDeviceRunProgress {
        phase,
        elapsed_seconds: started_at.elapsed().as_secs(),
        detail: detail.to_string(),
        log_lines,
    }
}

fn android_phase_detail(phase: AndroidDeviceRunPhase) -> &'static str {
    match phase {
        AndroidDeviceRunPhase::Checking => "Checking the device over ADB.",
        AndroidDeviceRunPhase::Staging => "Verifying and staging the APK.",
        AndroidDeviceRunPhase::Installing => "Installing the app on the device.",
        AndroidDeviceRunPhase::Launching => "Launching the app.",
        AndroidDeviceRunPhase::Running => "The app is running; its log streams here.",
        AndroidDeviceRunPhase::Completed => "The log session ended.",
    }
}

/// Host ADB sees USB phones and already-running emulators without a toolchain container.
/// Each ready phone is also asked where it sits beside this computer's networks.
pub fn list_host_android_devices() -> Result<AndroidDevices, String> {
    let Some(adb) = find_adb() else {
        return Ok(AndroidDevices {
            available: false,
            devices: vec![],
            issue: Some(MISSING_ADB.into()),
            host_networks: vec![],
        });
    };
    list_devices_with_networks(&adb, &host_ipv4_interfaces())
}

/// The engine verifies the managed artifact's size and checksum before entering this call.
/// Every device command targets the selected serial; install preserves existing app data.
/// Once the app is up, its log streams through `on_progress` until the operation is stopped,
/// the app's process is gone, or ADB loses the device; each of those ends the run, not fails it.
pub fn run_host_android_device<F>(
    serial: &str,
    application_id: &str,
    apk: &Path,
    expected_sha256: &str,
    on_progress: F,
) -> Result<AndroidDeviceRunResult, String>
where
    F: FnMut(AndroidDeviceRunProgress),
{
    let adb = find_adb().ok_or_else(|| MISSING_ADB.to_string())?;
    run_device_with(
        &adb,
        serial,
        application_id,
        apk,
        expected_sha256,
        on_progress,
    )
}

fn find_adb() -> Option<PathBuf> {
    let executable = if cfg!(windows) { "adb.exe" } else { "adb" };
    let mut candidates = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(executable))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for key in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(root) = std::env::var_os(key) {
            candidates.push(PathBuf::from(root).join("platform-tools").join(executable));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join("Android/Sdk/platform-tools").join(executable));
        candidates.push(
            home.join("Library/Android/sdk/platform-tools")
                .join(executable),
        );
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local)
                .join("Android/Sdk/platform-tools")
                .join(executable),
        );
    }
    candidates.into_iter().find(|path| {
        if !path.is_absolute() || !path.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
        }
        #[cfg(not(unix))]
        true
    })
}

fn list_devices_with(adb: &Path) -> Result<AndroidDevices, String> {
    let output = adb_output(Command::new(adb).args(["devices", "-l"]), ADB_PROBE_TIMEOUT)?;
    if !output.success {
        return Ok(AndroidDevices {
            available: true,
            devices: vec![],
            issue: Some(format!(
                "ADB could not list devices: {}",
                output.text.trim()
            )),
            host_networks: vec![],
        });
    }
    Ok(AndroidDevices {
        available: true,
        devices: parse_devices(&output.text),
        issue: None,
        host_networks: vec![],
    })
}

/// The listing the desktop and the command line show. The run's own check, above, stays
/// this probe free: it wants the device's state, not its address.
fn list_devices_with_networks(
    adb: &Path,
    host: &[Ipv4Interface],
) -> Result<AndroidDevices, String> {
    let mut listing = list_devices_with(adb)?;
    let mut networks = host.iter().map(Ipv4Interface::cidr).collect::<Vec<_>>();
    networks.sort();
    networks.dedup();
    listing.host_networks = networks;
    for device in &mut listing.devices {
        if device.state != "device" || device.serial.starts_with("emulator-") {
            continue;
        }
        // A phone that cannot say is left unmarked rather than warned about.
        device.network = adb_output(
            Command::new(adb).args(["-s", &device.serial, "shell", "ip", "-4", "-o", "addr"]),
            PROCESS_PROBE_TIMEOUT,
        )
        .ok()
        .filter(|output| output.success)
        .map(|output| device_network(&parse_ip_addresses(&output.text), host));
    }
    Ok(listing)
}

fn parse_devices(output: &str) -> Vec<AndroidDevice> {
    let mut devices = Vec::new();
    let mut listing = false;
    for line in output.lines().map(str::trim) {
        if line == "List of devices attached" {
            listing = true;
            continue;
        }
        if !listing || line.is_empty() || line.starts_with('*') {
            continue;
        }
        let mut words = line.split_whitespace();
        let (Some(serial), Some(state)) = (words.next(), words.next()) else {
            continue;
        };
        // ADB may be unable to read a USB serial until host permissions are configured.
        let permission_placeholder = state == "no"
            && words.clone().next() == Some("permissions")
            && serial.len() <= 256
            && serial.bytes().all(|byte| byte == b'?');
        if (!valid_serial(serial) && !permission_placeholder)
            || devices
                .iter()
                .any(|device: &AndroidDevice| device.serial == serial)
        {
            continue;
        }
        let state = if state == "no" && words.clone().next() == Some("permissions") {
            "no_permissions"
        } else {
            state
        };
        if !matches!(
            state,
            "device"
                | "offline"
                | "unauthorized"
                | "no_permissions"
                | "recovery"
                | "sideload"
                | "bootloader"
                | "rescue"
                | "authorizing"
                | "connecting"
        ) {
            continue;
        }
        let model = words
            .find_map(|word| word.strip_prefix("model:"))
            .map(|model| model.replace('_', " "));
        devices.push(AndroidDevice {
            serial: serial.into(),
            state: state.into(),
            model,
            network: None,
        });
    }
    devices
}

/// One IPv4 address with its prefix length, as `ip -4 -o addr` and `ifconfig` list them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ipv4Interface {
    name: String,
    address: Ipv4Addr,
    prefix: u8,
}

impl Ipv4Interface {
    fn mask(&self) -> u32 {
        if self.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - u32::from(self.prefix))
        }
    }

    fn contains(&self, other: Ipv4Addr) -> bool {
        u32::from(other) & self.mask() == u32::from(self.address) & self.mask()
    }

    /// The network alone, `192.168.110.0/24`, as the desktop names it to join.
    fn cidr(&self) -> String {
        format!(
            "{}/{}",
            Ipv4Addr::from(u32::from(self.address) & self.mask()),
            self.prefix
        )
    }

    fn with_prefix(&self) -> String {
        format!("{}/{}", self.address, self.prefix)
    }
}

/// `ip -4 -o addr` on Linux and Android: one interface per line, `N: name    inet A/P ...`.
fn parse_ip_addresses(output: &str) -> Vec<Ipv4Interface> {
    output
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let _index = words.next()?;
            let name = words.next()?;
            if words.next()? != "inet" {
                return None;
            }
            let (address, prefix) = words.next()?.split_once('/')?;
            Some(Ipv4Interface {
                name: name.to_string(),
                address: address.parse().ok()?,
                prefix: prefix.parse().ok().filter(|prefix| *prefix <= 32)?,
            })
        })
        .collect()
}

/// `ifconfig` on macOS: an unindented `name: flags=...` line opens each interface, and its
/// `inet A netmask 0xHHHHHHHH` lines follow indented.
fn parse_ifconfig_addresses(output: &str) -> Vec<Ipv4Interface> {
    let mut interfaces = Vec::new();
    let mut name: Option<String> = None;
    for line in output.lines() {
        if !line.starts_with(char::is_whitespace) {
            name = line
                .split_once(':')
                .map(|(name, _)| name.trim().to_string());
            continue;
        }
        let mut words = line.split_whitespace();
        if words.next() != Some("inet") {
            continue;
        }
        let (Some(current), Some(address), Some("netmask"), Some(mask)) =
            (name.as_ref(), words.next(), words.next(), words.next())
        else {
            continue;
        };
        let (Ok(address), Some(mask)) = (
            address.parse::<Ipv4Addr>(),
            mask.strip_prefix("0x")
                .and_then(|hex| u32::from_str_radix(hex, 16).ok()),
        ) else {
            continue;
        };
        // Only a contiguous mask is a prefix.
        if (!mask).count_ones() != (!mask).trailing_ones() {
            continue;
        }
        interfaces.push(Ipv4Interface {
            name: current.clone(),
            address,
            prefix: mask.count_ones() as u8,
        });
    }
    interfaces
}

/// Interface names a phone never shares a network with: loopback and this computer's own
/// container, VM and tunnel networks, which its Wi-Fi never reaches.
const HOST_VIRTUAL_INTERFACES: [&str; 24] = [
    "lo",
    "docker",
    "br-",
    "veth",
    "virbr",
    "vboxnet",
    "vmnet",
    "tun",
    "tap",
    "wg",
    "utun",
    "bridge",
    "lxc",
    "lxd",
    "zt",
    "tailscale",
    "cni",
    "flannel",
    "podman",
    "awdl",
    "llw",
    "anpi",
    "gif",
    "stf",
];

fn host_lan_interface(interface: &Ipv4Interface) -> bool {
    !interface.address.is_loopback()
        && !interface.address.is_link_local()
        && interface.prefix <= 30
        && !HOST_VIRTUAL_INTERFACES
            .iter()
            .any(|prefix| interface.name.starts_with(prefix))
}

/// This computer's networks a phone could be on. Elsewhere, and where neither tool answers,
/// the list is empty and no phone is warned about.
fn host_ipv4_interfaces() -> Vec<Ipv4Interface> {
    let interfaces = if cfg!(target_os = "macos") {
        host_command_output(&["/sbin/ifconfig", "ifconfig"], &["-a"])
            .map(|output| parse_ifconfig_addresses(&output))
    } else {
        host_command_output(
            &["ip", "/usr/sbin/ip", "/sbin/ip", "/usr/bin/ip", "/bin/ip"],
            &["-4", "-o", "addr"],
        )
        .map(|output| parse_ip_addresses(&output))
    };
    interfaces
        .unwrap_or_default()
        .into_iter()
        .filter(host_lan_interface)
        .collect()
}

/// A desktop opened from a launcher may not carry a login shell's PATH; the tool's usual
/// locations follow the PATH lookup.
fn host_command_output(candidates: &[&str], arguments: &[&str]) -> Option<String> {
    candidates.iter().find_map(|program| {
        let output = Command::new(program)
            .args(arguments)
            .stdin(Stdio::null())
            .tracked_output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    })
}

/// The phone's Wi-Fi and Ethernet interfaces alone count: mobile data and VPN tunnels never
/// share a network with this computer, whatever their addresses look like.
fn phone_lan_interface(interface: &Ipv4Interface) -> bool {
    interface.prefix <= 30
        && ["wlan", "eth", "swlan", "wifi"]
            .iter()
            .any(|prefix| interface.name.starts_with(prefix))
}

fn device_network(interfaces: &[Ipv4Interface], host: &[Ipv4Interface]) -> AndroidDeviceNetwork {
    let lan = interfaces
        .iter()
        .filter(|interface| phone_lan_interface(interface))
        .collect::<Vec<_>>();
    AndroidDeviceNetwork {
        address: lan.first().map(|interface| interface.with_prefix()),
        on_host_network: lan.iter().any(|phone| {
            host.iter()
                .any(|host| host.contains(phone.address) || phone.contains(host.address))
        }),
    }
}

fn valid_serial(serial: &str) -> bool {
    !serial.is_empty()
        && serial.len() <= 256
        && !serial.starts_with('-')
        && serial
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:[]".contains(&byte))
}

fn valid_java_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.split('.').all(|segment| {
            segment
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}

fn launcher_component(output: &str, application_id: &str) -> Option<String> {
    output.lines().map(str::trim).find_map(|line| {
        let (package, activity) = line.split_once('/')?;
        (package == application_id
            && valid_java_name(activity.strip_prefix('.').unwrap_or(activity)))
        .then(|| line.to_string())
    })
}

fn run_device_with<F>(
    adb: &Path,
    serial: &str,
    application_id: &str,
    apk: &Path,
    expected_sha256: &str,
    mut on_progress: F,
) -> Result<AndroidDeviceRunResult, String>
where
    F: FnMut(AndroidDeviceRunProgress),
{
    if !valid_serial(serial) {
        return Err("Select a valid Android device serial from the device list.".into());
    }
    // ADB joins shell arguments on the device. Only allow shell-inert Java identifiers.
    if !application_id.contains('.') || !valid_java_name(application_id) {
        return Err("The retained APK's application identifier is invalid.".into());
    }
    if !valid_sha256(expected_sha256) {
        return Err("The retained APK's checksum is invalid. Build the APK again.".into());
    }
    if !apk.is_absolute()
        || !apk.is_file()
        || apk.extension().and_then(|part| part.to_str()) != Some("apk")
    {
        return Err("The retained APK is unavailable. Build it again before installing.".into());
    }
    let started_at = Instant::now();
    let mut report = |phase: AndroidDeviceRunPhase| {
        on_progress(android_progress(
            phase,
            started_at,
            android_phase_detail(phase),
            Vec::new(),
        ));
    };
    let _device = AndroidDeviceGuard::claim(serial)?;
    report(AndroidDeviceRunPhase::Checking);
    let listing = list_devices_with(adb)?;
    if let Some(issue) = listing.issue {
        return Err(issue);
    }
    let device = listing.devices.iter().find(|device| device.serial == serial)
        .ok_or_else(|| "The selected Android device is no longer connected. Refresh devices and select it again.".to_string())?;
    let model = device.model.clone();
    match device.state.as_str() {
        "device" => {}
        "unauthorized" | "authorizing" => return Err("Unlock the selected Android device and accept its USB debugging authorization prompt, then refresh devices.".into()),
        "offline" | "connecting" => return Err("The selected Android device is offline. Reconnect it or finish starting the emulator, then refresh devices.".into()),
        "no_permissions" => return Err("ADB cannot access the selected Android device. Configure the host's Android USB permissions and reconnect it.".into()),
        _ => return Err("The selected Android device is not in its normal running state. Start Android and refresh devices.".into()),
    }
    let booted = adb_output(
        Command::new(adb).args(["-s", serial, "shell", "getprop", "sys.boot_completed"]),
        ADB_PROBE_TIMEOUT,
    )?;
    if !booted.success || booted.text.trim() != "1" {
        return Err("The selected Android device has not finished booting. Unlock it and try again once Android is ready.".into());
    }
    // The source may be edited outside buildbridge while ADB probes the phone. Install a
    // private copy whose bytes we verify now, rather than reopening that mutable source.
    report(AndroidDeviceRunPhase::Staging);
    let snapshot = AndroidApkSnapshot::create(apk, expected_sha256)?;
    report(AndroidDeviceRunPhase::Installing);
    let installed = adb_output(
        Command::new(adb)
            .args(["-s", serial, "install", "-r"])
            .arg(snapshot.apk()),
        ADB_INSTALL_TIMEOUT,
    )?;
    if !installed.success || !installed.text.lines().any(|line| line.trim() == "Success") {
        return Err(install_error(&installed.text, application_id, serial));
    }
    report(AndroidDeviceRunPhase::Launching);
    let resolved = adb_output(
        Command::new(adb).args([
            "-s",
            serial,
            "shell",
            "cmd",
            "package",
            "resolve-activity",
            "--brief",
            "--user",
            "current",
            "-a",
            "android.intent.action.MAIN",
            "-c",
            "android.intent.category.LAUNCHER",
            "-p",
            application_id,
        ]),
        ADB_PROBE_TIMEOUT,
    )
    .map_err(|error| {
        format!("The APK was installed, but its launcher could not be read: {error}")
    })?;
    let component = launcher_component(&resolved.text, application_id)
        .filter(|_| resolved.success)
        .ok_or_else(|| "The APK was installed, but Android returned no safe launcher activity for this application. Open the app on the device.".to_string())?;
    let launched = adb_output(
        Command::new(adb).args([
            "-s",
            serial,
            "shell",
            "am",
            "start",
            "-W",
            "--user",
            "current",
            "-a",
            "android.intent.action.MAIN",
            "-c",
            "android.intent.category.LAUNCHER",
            "-n",
            &component,
        ]),
        ADB_PROBE_TIMEOUT,
    )
    .map_err(|error| {
        format!("The APK was installed, but launch could not be confirmed: {error}")
    })?;
    if !launched.success
        || !launched
            .text
            .lines()
            .any(|line| line.trim() == "Status: ok")
        || launched
            .text
            .lines()
            .any(|line| line.trim().starts_with("Error"))
    {
        return Err(format!(
            "The APK was installed, but Android did not confirm launch. Unlock the device and open the app, or retry. {}",
            launched.text.trim()
        ));
    }
    let installed_at_epoch_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let pid = process_id(adb, serial, application_id, PROCESS_LOOKUPS_AFTER_LAUNCH)?;
    let (console_end, console_tail) = stream_app_log(
        adb,
        serial,
        application_id,
        pid,
        started_at,
        &mut on_progress,
    )?;
    on_progress(android_progress(
        AndroidDeviceRunPhase::Completed,
        started_at,
        android_phase_detail(AndroidDeviceRunPhase::Completed),
        Vec::new(),
    ));
    Ok(AndroidDeviceRunResult {
        serial: serial.into(),
        model,
        application_id: application_id.into(),
        sha256: expected_sha256.to_ascii_lowercase(),
        installed: true,
        launched: true,
        pid,
        installed_at_epoch_seconds,
        console_end,
        console_tail,
    })
}

/// What `pidof` says about the app's process: present with its id, gone, or unreadable when
/// ADB itself answered with an error rather than the device.
enum AppProcess {
    Present(u32),
    Gone,
    Unknown,
}

fn app_process(output: &AdbOutput) -> AppProcess {
    let text = output.text.trim();
    if let Some(pid) = text
        .split_whitespace()
        .next()
        .and_then(|word| word.parse::<u32>().ok())
        .filter(|pid| *pid > 0)
    {
        return AppProcess::Present(pid);
    }
    // toybox's pidof exits 1 with nothing printed when no process matches; an ADB failure
    // exits 1 too, but says so.
    if text.is_empty() || (!output.success && !text.contains("error")) {
        AppProcess::Gone
    } else {
        AppProcess::Unknown
    }
}

/// The app's process on the device, tried `attempts` times. `None` means the device did not
/// name one — a process still registering, or a build without `pidof` — and the log is then
/// narrowed by tag instead. Only a Stop turns into an error.
fn process_id(
    adb: &Path,
    serial: &str,
    application_id: &str,
    attempts: usize,
) -> Result<Option<u32>, String> {
    for attempt in 0..attempts {
        if attempt > 0 {
            thread::sleep(PROCESS_LOOKUP_INTERVAL);
        }
        match adb_output(
            Command::new(adb).args(["-s", serial, "shell", "pidof", application_id]),
            PROCESS_PROBE_TIMEOUT,
        ) {
            Ok(output) => {
                if let AppProcess::Present(pid) = app_process(&output) {
                    return Ok(Some(pid));
                }
            }
            Err(error) => {
                if current_scope().is_some_and(|scope| scope.is_cancelled()) {
                    return Err(error);
                }
            }
        }
    }
    Ok(None)
}

/// Streams the app's log to `on_progress` until the operation is stopped, the app's process
/// is gone, or the log client loses the device. A stop kills the log client alone: the app
/// stays installed and running on the device.
fn stream_app_log<F>(
    adb: &Path,
    serial: &str,
    application_id: &str,
    pid: Option<u32>,
    started_at: Instant,
    on_progress: &mut F,
) -> Result<(ConsoleEnd, Vec<String>), String>
where
    F: FnMut(AndroidDeviceRunProgress),
{
    let mut command = Command::new(adb);
    command.args(["-s", serial, "logcat", "-v", "time"]);
    match pid {
        Some(pid) => {
            command.arg(format!("--pid={pid}"));
        }
        None => {
            command.arg("-s").args(FALLBACK_LOG_FILTERS);
        }
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .tracked_spawn()
        .map_err(|error| format!("The app launched, but its log could not be opened: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "The app launched, but its log output could not be captured.".to_string())?;
    let (sender, receiver) = mpsc::channel::<String>();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                break;
            };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let detail = match pid {
        Some(pid) => format!("The app is running; its log streams here (process {pid})."),
        None => "The app is running; the device log streams here, narrowed to Capacitor, the web view and crashes: the process id could not be read.".to_string(),
    };
    on_progress(android_progress(
        AndroidDeviceRunPhase::Running,
        started_at,
        &detail,
        Vec::new(),
    ));

    let mut console_tail = Vec::new();
    let mut pending = Vec::new();
    let mut last_event = Instant::now();
    let mut last_probe = Instant::now();
    let cancelled = || current_scope().is_some_and(|scope| scope.is_cancelled());
    let take_line = |line: String, console_tail: &mut Vec<String>, pending: &mut Vec<String>| {
        let line = sanitize_build_log_line(&line);
        if !line.is_empty() {
            push_bounded(console_tail, line.clone(), CONSOLE_TAIL_LINES);
            pending.push(line);
        }
    };
    let console_end = loop {
        if cancelled() {
            break ConsoleEnd::Stopped;
        }
        match receiver.recv_timeout(PROGRESS_INTERVAL) {
            Ok(line) => take_line(line, &mut console_tail, &mut pending),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break if cancelled() {
                    ConsoleEnd::Stopped
                } else {
                    ConsoleEnd::Disconnected
                };
            }
        }
        if !pending.is_empty()
            && (last_event.elapsed() >= PROGRESS_INTERVAL || pending.len() >= CONSOLE_BATCH_LINES)
        {
            on_progress(android_progress(
                AndroidDeviceRunPhase::Running,
                started_at,
                &detail,
                std::mem::take(&mut pending),
            ));
            last_event = Instant::now();
        }
        if let Some(pid) = pid
            && last_probe.elapsed() >= PROCESS_POLL_INTERVAL
        {
            last_probe = Instant::now();
            let probe = adb_output(
                Command::new(adb).args(["-s", serial, "shell", "pidof", application_id]),
                PROCESS_PROBE_TIMEOUT,
            );
            match probe.as_ref().map(app_process) {
                Ok(AppProcess::Present(current)) if current == pid => {}
                Ok(AppProcess::Present(_) | AppProcess::Gone) => break ConsoleEnd::Exited,
                // A probe that ADB could not answer, or that a Stop interrupted: the next turn
                // of the loop reads the stop; the log itself says when the device is gone.
                Ok(AppProcess::Unknown) | Err(_) => {}
            }
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    for line in receiver.try_iter() {
        take_line(line, &mut console_tail, &mut pending);
    }
    if !pending.is_empty() {
        on_progress(android_progress(
            AndroidDeviceRunPhase::Running,
            started_at,
            &detail,
            pending,
        ));
    }
    Ok((console_end, console_tail))
}

fn push_bounded(lines: &mut Vec<String>, line: String, limit: usize) {
    lines.push(line);
    if lines.len() > limit {
        lines.remove(0);
    }
}

struct AndroidApkSnapshot(PathBuf);

impl AndroidApkSnapshot {
    fn create(source: &Path, expected_sha256: &str) -> Result<Self, String> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "buildbridge-android-apk-{}-{nonce}",
            std::process::id()
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&directory)
            .map_err(|error| format!("Could not prepare the APK for installation: {error}"))?;
        let snapshot = Self(directory);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut destination = options
            .open(snapshot.apk())
            .map_err(|error| format!("Could not stage the APK: {error}"))?;
        let mut source = File::open(source)
            .map_err(|error| format!("The retained APK is unavailable: {error}"))?;
        let metadata = source.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > ANDROID_RELEASE_MAX_BYTES
        {
            return Err("The retained APK is not a supported nonempty regular file.".into());
        }
        let mut buffer = [0_u8; 64 * 1024];
        let mut total = 0_u64;
        loop {
            if current_scope().is_some_and(|scope| scope.is_cancelled()) {
                return Err("Android device operation stopped.".into());
            }
            let read = source
                .read(&mut buffer)
                .map_err(|error| format!("Could not read the retained APK: {error}"))?;
            if read == 0 {
                break;
            }
            total += read as u64;
            if total > ANDROID_RELEASE_MAX_BYTES {
                return Err(
                    "The retained APK grew beyond the supported size while preparing installation."
                        .into(),
                );
            }
            destination
                .write_all(&buffer[..read])
                .map_err(|error| format!("Could not stage the APK: {error}"))?;
        }
        drop(destination);
        if total != metadata.len()
            || !native_sha256(&snapshot.apk())?.eq_ignore_ascii_case(expected_sha256)
        {
            return Err(
                "The retained APK's checksum or size changed. Build a new APK before installing."
                    .into(),
            );
        }
        Ok(snapshot)
    }

    fn apk(&self) -> PathBuf {
        self.0.join("App.apk")
    }
}

impl Drop for AndroidApkSnapshot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct AndroidDeviceGuard(String);

impl AndroidDeviceGuard {
    fn claim(serial: &str) -> Result<Self, String> {
        let mut devices = RUNNING_DEVICES
            .lock()
            .map_err(|_| "The Android device operation registry is unavailable.".to_string())?;
        if devices.iter().any(|device| device == serial) {
            return Err("Another buildbridge operation is installing on this Android device. Wait for it to finish.".into());
        }
        devices.push(serial.into());
        Ok(Self(serial.into()))
    }
}

impl Drop for AndroidDeviceGuard {
    fn drop(&mut self) {
        if let Ok(mut devices) = RUNNING_DEVICES.lock() {
            devices.retain(|device| device != &self.0);
        }
    }
}

fn install_error(output: &str, application_id: &str, serial: &str) -> String {
    let hint = if output.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE")
        || output.contains("signatures do not match")
    {
        "The installed app uses a different signing key. The existing app and its data are unchanged.\n\nTo preserve its data, rebuild with the original signing key. To keep both apps, use a different application ID and rebuild. For a disposable test installation, manually remove the existing app from the phone, then retry this retained APK with Install and open. Removing the app deletes its local data. Rebuilding with the current signing key alone will not fix this mismatch."
    } else if output.contains("INSTALL_FAILED_VERSION_DOWNGRADE") {
        "This APK has an older version code than the installed app. Build a newer version; buildbridge kept the existing app and its data."
    } else if output.contains("INSTALL_FAILED_USER_RESTRICTED") {
        "Unlock the device and allow USB app installation in its developer settings, then retry."
    } else if output.contains("INSTALL_FAILED_INSUFFICIENT_STORAGE") {
        "Free storage on the selected Android device and retry."
    } else {
        "Unlock the selected Android device and check its USB installation prompt."
    };
    // ADB's prefix includes the private staging path, which is removed after the attempt.
    // Retain the package-manager diagnostic without directing users to that temporary file.
    let diagnostic = output
        .split_once("Failure [")
        .map(|(_, detail)| format!("Failure [{}", detail.trim()))
        .unwrap_or_else(|| output.trim().to_string());
    format!(
        "The APK is ready, but Android could not install {application_id} on {serial}. {hint}\n\nADB: {diagnostic}"
    )
}

struct AdbOutput {
    success: bool,
    text: String,
}

/// Files avoid pipe deadlocks and inherited pipes keeping a cancelled ADB client alive.
/// The bounded capture is owner-readable and removed on every outcome.
struct AdbCapture(PathBuf);

impl Drop for AdbCapture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn adb_output(command: &mut Command, timeout: Duration) -> Result<AdbOutput, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let capture_path = std::env::temp_dir().join(format!(
        "buildbridge-adb-{}-{nonce}.log",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&capture_path)
        .map_err(|error| format!("Could not capture ADB output: {error}"))?;
    let capture = AdbCapture(capture_path);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(file.try_clone().map_err(|error| error.to_string())?)
        .stderr(file)
        .tracked_spawn()
        .map_err(|error| format!("Could not start host ADB: {error}. {MISSING_ADB}"))?;
    let started = Instant::now();
    let outcome = loop {
        if current_scope().is_some_and(|scope| scope.is_cancelled()) {
            break Err("Android device operation stopped.".to_string());
        }
        if started.elapsed() >= timeout {
            break Err("ADB timed out. Check the device before retrying; an installation may already have completed.".to_string());
        }
        if fs::metadata(&capture.0).is_ok_and(|metadata| metadata.len() > ADB_OUTPUT_LIMIT) {
            break Err("ADB returned too much output to confirm the device operation.".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => break Err(format!("Could not monitor ADB: {error}")),
        }
    };
    if outcome.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let status = outcome?;
    let mut bytes = Vec::new();
    File::open(&capture.0)
        .map_err(|error| error.to_string())?
        .take(ADB_OUTPUT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > ADB_OUTPUT_LIMIT {
        return Err("ADB returned too much output to confirm the device operation.".into());
    }
    Ok(AdbOutput {
        success: status.success(),
        text: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    static TEST_DEVICE: Mutex<()> = Mutex::new(());

    struct Fixture {
        _serial_guard: std::sync::MutexGuard<'static, ()>,
        root: PathBuf,
        adb: PathBuf,
        apk: PathBuf,
    }

    impl Fixture {
        fn new(scenario: &str) -> Self {
            let serial_guard = TEST_DEVICE.lock().unwrap();
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "buildbridge-adb-test-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&root).unwrap();
            let adb = root.join("adb");
            crate::test_scripts::write_runnable(
                &adb,
                r#"#!/bin/sh
printf '%s\n' "$@" >> "$0.args"
printf '%s\n' '__END__' >> "$0.args"
scenario=$(cat "$0.scenario")
if [ "$scenario" = timeout ]; then exec /bin/sleep 30; fi
if [ "$1" = devices ]; then
    printf '%s\n' 'List of devices attached' 'emulator-5554 device product:sdk model:Preview_Emulator'
    case "$scenario" in
        unauthorized) printf '%s\n' 'phone-123 unauthorized';;
        offline) printf '%s\n' 'phone-123 offline';;
        missing) :;;
        *) printf '%s\n' 'phone-123 device product:phone model:Pixel_9';;
    esac
elif [ "$3" = install ]; then
    cp "$5" "$0.installed" || exit 2
    case "$scenario" in
        signature) printf '%s\n' 'Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE]'; exit 1;;
        signature_details)
            printf '%s\n' 'Performing Streamed Install'
            printf 'adb: failed to install %s: Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE: Existing package nz.co.thinksolar.app.debug signatures do not match newer version; ignoring!]\n' "$5"
            exit 1;;
        downgrade) printf '%s\n' 'Failure [INSTALL_FAILED_VERSION_DOWNGRADE]'; exit 1;;
        zero_install_failure) printf '%s\n' 'Failure [INSTALL_FAILED_USER_RESTRICTED]';;
        *) printf '%s\n' Success;;
    esac
elif [ "$4" = getprop ]; then
    if [ "$scenario" = changed_source ]; then printf '%s' abd > "$0.source-apk"; fi
    if [ "$scenario" = booting ]; then printf '%s\n' 0; else printf '%s\n' 1; fi
elif [ "$4" = cmd ]; then
    case "$scenario" in
        injected_component) printf '%s\n' 'com.example.app/.MainActivity;touch /tmp/injected';;
        foreign_component) printf '%s\n' 'com.other.app/.MainActivity';;
        *) printf '%s\n' 'priority=0 preferredOrder=0 match=0x108000 specificIndex=-1 isDefault=true' 'com.example.app/.MainActivity';;
    esac
elif [ "$4" = am ]; then
    if [ "$scenario" = launch_error ]; then
        printf '%s\n' 'Error: Activity class does not exist.'
    else
        printf '%s\n' 'Starting: Intent { act=android.intent.action.MAIN }' 'Status: ok' 'Complete'
    fi
elif [ "$4" = ip ]; then
    printf '%s\n' '1: lo    inet 127.0.0.1/8 scope host lo\       valid_lft forever preferred_lft forever'
    if [ "$scenario" = mobile_data ]; then
        printf '%s\n' '12: ccmni2    inet 10.183.239.119/8 scope global ccmni2\       valid_lft forever preferred_lft forever' '47: vgate0    inet 172.30.205.219/32 scope global vgate0\       valid_lft forever preferred_lft forever'
    else
        printf '%s\n' '40: wlan0    inet 192.168.110.252/24 brd 192.168.110.255 scope global dynamic wlan0\       valid_lft 1037sec preferred_lft 1037sec'
    fi
elif [ "$4" = pidof ]; then
    lookups=$(( $(cat "$0.pidof" 2>/dev/null || echo 0) + 1 ))
    printf '%s' "$lookups" > "$0.pidof"
    case "$scenario" in
        no_pid) exit 1;;
        app_exits) if [ "$lookups" -gt 1 ]; then exit 1; fi; printf '%s\n' 4242;;
        *) printf '%s\n' 4242;;
    esac
elif [ "$3" = logcat ]; then
    printf '%s\n' '09-07 10:00:00.000 I/Capacitor( 4242): Starting BridgeActivity' '09-07 10:00:00.100 D/Capacitor/Console( 4242): [log] app ready'
    if [ "$scenario" = logcat_ends ]; then exit 0; fi
    exec /bin/sleep 30
else
    printf '%s\n' 'unexpected command' >&2
    exit 2
fi
"#,
            );
            fs::write(root.join("adb.scenario"), scenario).unwrap();
            let apk = root.join("App $(touch never) preview.apk");
            fs::write(&apk, b"abc").unwrap();
            std::os::unix::fs::symlink(&apk, root.join("adb.source-apk")).unwrap();
            Self {
                _serial_guard: serial_guard,
                root,
                adb,
                apk,
            }
        }

        fn run(&self) -> Result<AndroidDeviceRunResult, String> {
            self.run_with(|_| {})
        }

        fn run_with<F: FnMut(AndroidDeviceRunProgress)>(
            &self,
            on_progress: F,
        ) -> Result<AndroidDeviceRunResult, String> {
            run_device_with(
                &self.adb,
                "phone-123",
                "com.example.app",
                &self.apk,
                SHA256,
                on_progress,
            )
        }

        /// Runs under a scope that is cancelled after `after`, as a Stop from the desktop is.
        fn run_then_stop(
            &self,
            after: Duration,
        ) -> (
            Result<AndroidDeviceRunResult, String>,
            Vec<AndroidDeviceRunProgress>,
        ) {
            let scope = OperationScope::new();
            let cancellation = Arc::clone(&scope);
            let _entered = enter_operation(scope);
            let cancel = thread::spawn(move || {
                thread::sleep(after);
                cancellation.cancel();
            });
            let mut events = Vec::new();
            let result = self.run_with(|progress| events.push(progress));
            cancel.join().unwrap();
            (result, events)
        }

        fn calls(&self) -> Vec<Vec<String>> {
            fs::read_to_string(self.root.join("adb.args"))
                .unwrap_or_default()
                .split("__END__\n")
                .filter(|call| !call.is_empty())
                .map(|call| call.lines().map(str::to_string).collect())
                .collect()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn lists_devices_and_retains_actionable_readiness_states() {
        let parsed = parse_devices(
            "* daemon started successfully *\nList of devices attached\nphone-123 unauthorized\nemulator-5554 device model:Preview_Emulator transport_id:2\nusb-456 no permissions (missing udev rules)\nphone-123 device\ninvalid;serial device\n",
        );
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].state, "unauthorized");
        assert_eq!(parsed[1].model.as_deref(), Some("Preview Emulator"));
        assert_eq!(parsed[2].state, "no_permissions");
        let unknown = parse_devices(
            "List of devices attached\n???????????? no permissions (missing udev rules)\n",
        );
        assert_eq!(unknown[0].state, "no_permissions");
        assert!(parse_devices("daemon startup noise").is_empty());
    }

    fn interface(name: &str, address: &str, prefix: u8) -> Ipv4Interface {
        Ipv4Interface {
            name: name.into(),
            address: address.parse().unwrap(),
            prefix,
        }
    }

    #[test]
    fn reads_addresses_as_ip_and_ifconfig_list_them() {
        let phone = parse_ip_addresses(
            "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever\n40: wlan0    inet 192.168.110.252/24 brd 192.168.110.255 scope global dynamic wlan0\\       valid_lft 1037sec preferred_lft 1037sec\n47: vgate0    inet 172.30.205.219/32 scope global vgate0\\       valid_lft forever preferred_lft forever\nnoise\n5: bad    inet 300.1.1.1/24\n6: wide    inet 10.0.0.1/33\n",
        );
        assert_eq!(
            phone,
            [
                interface("lo", "127.0.0.1", 8),
                interface("wlan0", "192.168.110.252", 24),
                interface("vgate0", "172.30.205.219", 32),
            ]
        );
        let mac = parse_ifconfig_addresses(
            "lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384\n\tinet 127.0.0.1 netmask 0xff000000\nen0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\tether 3c:22:fb:00:00:00\n\tinet6 fe80::1%en0 prefixlen 64\n\tinet 192.168.1.10 netmask 0xffffff00 broadcast 192.168.1.255\nutun3: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1400\n\tinet 10.8.0.2 --> 10.8.0.1 netmask 0xffffffff\nodd0: flags=0 mtu 0\n\tinet 10.9.0.2 netmask 0xffff00ff\n",
        );
        assert_eq!(
            mac,
            [
                interface("lo0", "127.0.0.1", 8),
                interface("en0", "192.168.1.10", 24),
            ]
        );
        assert_eq!(
            interface("en0", "192.168.1.10", 24).cidr(),
            "192.168.1.0/24"
        );
        assert_eq!(interface("any", "10.1.2.3", 0).cidr(), "0.0.0.0/0");
    }

    #[test]
    fn a_phone_is_on_this_computers_network_by_its_wifi_or_ethernet_address_alone() {
        let host = [interface("wlp0s20f3", "192.168.110.238", 24)];
        let wifi = device_network(
            &[
                interface("lo", "127.0.0.1", 8),
                interface("wlan0", "192.168.110.252", 24),
                interface("vgate0", "172.30.205.219", 32),
            ],
            &host,
        );
        assert_eq!(
            wifi,
            AndroidDeviceNetwork {
                address: Some("192.168.110.252/24".into()),
                on_host_network: true,
            }
        );
        // Mobile data's /8 would swallow a 10.x computer; it is not a network the phone shares.
        let mobile = device_network(
            &[
                interface("ccmni2", "10.183.239.119", 8),
                interface("vgate0", "172.30.205.219", 32),
            ],
            &[interface("eth0", "10.0.1.5", 24)],
        );
        assert_eq!(
            mobile,
            AndroidDeviceNetwork {
                address: None,
                on_host_network: false,
            }
        );
        let elsewhere = device_network(&[interface("wlan0", "10.1.2.3", 24)], &host);
        assert_eq!(elsewhere.address.as_deref(), Some("10.1.2.3/24"));
        assert!(!elsewhere.on_host_network);
        // Either side's prefix may be the wider one.
        assert!(
            device_network(
                &[interface("eth0", "192.168.1.9", 16)],
                &[interface("en0", "192.168.7.7", 24)]
            )
            .on_host_network
        );
        assert!(!device_network(&[interface("wlan0", "192.168.110.252", 24)], &[]).on_host_network);
    }

    #[test]
    fn this_computers_networks_leave_out_loopback_and_its_container_and_tunnel_networks() {
        let kept = [
            interface("wlp0s20f3", "192.168.110.238", 24),
            interface("en0", "10.0.1.5", 24),
            interface("enp3s0", "172.16.4.9", 22),
        ];
        for interface in &kept {
            assert!(host_lan_interface(interface), "{interface:?}");
        }
        let dropped = [
            interface("lo", "127.0.0.1", 8),
            interface("lo0", "127.0.0.1", 8),
            interface("docker0", "172.17.0.1", 16),
            interface("br-9574cee6088a", "172.27.0.1", 16),
            interface("virbr0", "192.168.122.1", 24),
            interface("utun3", "10.8.0.2", 32),
            interface("wg0", "10.200.0.2", 24),
            interface("tailscale0", "100.64.0.3", 32),
            interface("en5", "169.254.10.4", 16),
            interface("ppp0", "10.64.64.64", 32),
        ];
        for interface in &dropped {
            assert!(!host_lan_interface(interface), "{interface:?}");
        }
    }

    #[test]
    fn the_listing_asks_each_ready_phone_where_it_is_and_nothing_else() {
        let host = [
            interface("wlp0s20f3", "192.168.110.238", 24),
            interface("wlp0s20f3", "192.168.110.239", 24),
        ];
        let fixture = Fixture::new("success");
        let listing = list_devices_with_networks(&fixture.adb, &host).unwrap();
        assert_eq!(listing.host_networks, ["192.168.110.0/24"]);
        let phone = listing
            .devices
            .iter()
            .find(|device| device.serial == "phone-123")
            .unwrap();
        assert_eq!(
            phone.network,
            Some(AndroidDeviceNetwork {
                address: Some("192.168.110.252/24".into()),
                on_host_network: true,
            })
        );
        let emulator = listing
            .devices
            .iter()
            .find(|device| device.serial == "emulator-5554")
            .unwrap();
        assert_eq!(
            emulator.network, None,
            "an emulator reaches the host anyway"
        );
        let calls = fixture.calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert_eq!(
            calls[1],
            ["-s", "phone-123", "shell", "ip", "-4", "-o", "addr"]
        );
        drop(fixture);

        let fixture = Fixture::new("mobile_data");
        let listing = list_devices_with_networks(&fixture.adb, &host).unwrap();
        let phone = listing
            .devices
            .iter()
            .find(|device| device.serial == "phone-123")
            .unwrap();
        assert_eq!(
            phone.network,
            Some(AndroidDeviceNetwork {
                address: None,
                on_host_network: false,
            })
        );
        drop(fixture);

        // A device that is not ready is not asked, and the run's own listing never asks.
        let fixture = Fixture::new("unauthorized");
        let listing = list_devices_with_networks(&fixture.adb, &host).unwrap();
        assert!(
            listing
                .devices
                .iter()
                .all(|device| device.network.is_none())
        );
        assert_eq!(fixture.calls().len(), 1);
    }

    #[test]
    fn install_and_launch_use_the_exact_selected_device_and_verified_private_apk() {
        let fixture = Fixture::new("success");
        let (result, events) = fixture.run_then_stop(Duration::from_millis(600));
        let result = result.unwrap();
        assert!(result.installed && result.launched);
        assert_eq!(result.serial, "phone-123");
        assert_eq!(result.model.as_deref(), Some("Pixel 9"));
        assert_eq!(result.sha256, SHA256);
        assert_eq!(result.pid, Some(4242));
        assert_eq!(result.console_end, ConsoleEnd::Stopped);
        assert!(result.installed_at_epoch_seconds > 0);
        let calls = fixture.calls();
        assert_eq!(calls.len(), 7, "{calls:?}");
        assert_eq!(calls[0], ["devices", "-l"]);
        for call in &calls[1..] {
            assert_eq!(&call[..2], ["-s", "phone-123"]);
            assert!(
                !call
                    .iter()
                    .any(|argument| argument == "uninstall" || argument == "clear")
            );
        }
        assert_eq!(&calls[2][..4], ["-s", "phone-123", "install", "-r"]);
        let installed_path = PathBuf::from(&calls[2][4]);
        assert_ne!(installed_path, fixture.apk);
        assert_eq!(installed_path.file_name().unwrap(), "App.apk");
        assert!(
            !installed_path.exists(),
            "the staged APK is removed after installation"
        );
        assert_eq!(
            fs::read(fixture.root.join("adb.installed")).unwrap(),
            b"abc"
        );
        assert_eq!(calls[4].last().unwrap(), "com.example.app/.MainActivity");
        assert_eq!(&calls[5][2..], ["shell", "pidof", "com.example.app"]);
        assert_eq!(
            &calls[6][2..],
            ["logcat", "-v", "time", "--pid=4242"],
            "the log is the app's alone"
        );

        // The phases arrive in order, the app's lines land while it runs, and a stop is the
        // session's normal end rather than a failure.
        let phases = events.iter().map(|event| event.phase).collect::<Vec<_>>();
        let mut expected = phases.clone();
        expected.dedup();
        assert_eq!(
            expected,
            [
                AndroidDeviceRunPhase::Checking,
                AndroidDeviceRunPhase::Staging,
                AndroidDeviceRunPhase::Installing,
                AndroidDeviceRunPhase::Launching,
                AndroidDeviceRunPhase::Running,
                AndroidDeviceRunPhase::Completed,
            ]
        );
        let streamed = events
            .iter()
            .flat_map(|event| event.log_lines.iter().cloned())
            .collect::<Vec<_>>();
        assert_eq!(streamed, result.console_tail);
        assert_eq!(streamed.len(), 2);
        assert!(
            streamed[0].ends_with("Starting BridgeActivity"),
            "{streamed:?}"
        );
        assert!(
            events
                .iter()
                .all(|event| event.log_lines.is_empty()
                    || event.phase == AndroidDeviceRunPhase::Running),
            "lines belong to the running phase"
        );
    }

    #[test]
    fn the_session_ends_when_the_app_exits() {
        let fixture = Fixture::new("app_exits");
        let started = Instant::now();
        let result = fixture.run().unwrap();
        assert_eq!(result.console_end, ConsoleEnd::Exited);
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(result.console_tail.len(), 2);
        let pidof_calls = fixture
            .calls()
            .iter()
            .filter(|call| call.contains(&"pidof".to_string()))
            .count();
        assert_eq!(pidof_calls, 2, "the process is found once, then found gone");
    }

    #[test]
    fn the_session_ends_when_the_log_client_loses_the_device() {
        let fixture = Fixture::new("logcat_ends");
        let started = Instant::now();
        let result = fixture.run().unwrap();
        assert_eq!(result.console_end, ConsoleEnd::Disconnected);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(result.console_tail.len(), 2);
    }

    #[test]
    fn without_a_process_id_the_log_is_narrowed_by_tag_instead() {
        let fixture = Fixture::new("no_pid");
        let (result, events) = fixture.run_then_stop(Duration::from_millis(2_500));
        let result = result.unwrap();
        assert_eq!(result.pid, None);
        assert_eq!(result.console_end, ConsoleEnd::Stopped);
        let calls = fixture.calls();
        let logcat = calls
            .iter()
            .find(|call| call.get(2).map(String::as_str) == Some("logcat"))
            .expect("the log session starts without a pid");
        assert!(
            !logcat.iter().any(|argument| argument.starts_with("--pid")),
            "{logcat:?}"
        );
        assert!(logcat.iter().any(|argument| argument == "Capacitor:*"));
        assert_eq!(logcat.last().unwrap(), "*:S", "everything else is silent");
        let running = events
            .iter()
            .find(|event| event.phase == AndroidDeviceRunPhase::Running)
            .unwrap();
        assert!(running.detail.contains("process id could not be read"));
    }

    #[test]
    fn a_process_answer_is_read_as_present_gone_or_unknown() {
        let present = AdbOutput {
            success: true,
            text: "4242\n".into(),
        };
        assert!(matches!(app_process(&present), AppProcess::Present(4242)));
        let gone = AdbOutput {
            success: false,
            text: String::new(),
        };
        assert!(matches!(app_process(&gone), AppProcess::Gone));
        let offline = AdbOutput {
            success: false,
            text: "error: device offline\n".into(),
        };
        assert!(matches!(app_process(&offline), AppProcess::Unknown));
    }

    #[test]
    fn disconnected_unauthorized_offline_and_booting_devices_never_install() {
        for (scenario, diagnostic) in [
            ("missing", "no longer connected"),
            ("unauthorized", "authorization prompt"),
            ("offline", "offline"),
            ("booting", "not finished booting"),
        ] {
            let fixture = Fixture::new(scenario);
            let error = fixture.run().unwrap_err();
            assert!(error.contains(diagnostic), "{scenario}: {error}");
            assert!(
                !fixture
                    .calls()
                    .iter()
                    .any(|call| call.iter().any(|argument| argument == "install"))
            );
        }
    }

    #[test]
    fn signature_downgrade_and_exit_zero_install_failures_never_launch_or_remove_data() {
        for (scenario, diagnostic) in [
            ("signature", "different signing key"),
            ("downgrade", "older version code"),
            ("zero_install_failure", "developer settings"),
        ] {
            let fixture = Fixture::new(scenario);
            let error = fixture.run().unwrap_err();
            assert!(error.contains(diagnostic), "{scenario}: {error}");
            assert_eq!(fixture.calls().len(), 3);
        }
    }

    #[test]
    fn debug_signature_conflict_names_the_app_and_safe_recovery_without_retrying() {
        let fixture = Fixture::new("signature_details");
        let application_id = "nz.co.thinksolar.app.debug";
        let error = run_device_with(
            &fixture.adb,
            "phone-123",
            application_id,
            &fixture.apk,
            SHA256,
            |_| {},
        )
        .unwrap_err();
        assert!(error.contains("The APK is ready"), "{error}");
        assert!(
            error.contains("nz.co.thinksolar.app.debug on phone-123"),
            "{error}"
        );
        assert!(error.contains("original signing key"), "{error}");
        assert!(
            error.contains("Removing the app deletes its local data"),
            "{error}"
        );
        assert!(
            error.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE"),
            "{error}"
        );
        let calls = fixture.calls();
        assert_eq!(
            calls.len(),
            3,
            "a rejected update must not trigger another device command"
        );
        assert_eq!(&calls[2][..4], ["-s", "phone-123", "install", "-r"]);
        assert!(
            !error.contains(&calls[2][4]),
            "the temporary staged path is not recovery advice"
        );
        assert_eq!(fs::read(&fixture.apk).unwrap(), b"abc");
    }

    #[test]
    fn invalid_shell_tokens_are_rejected_before_spawning_adb() {
        let fixture = Fixture::new("success");
        for application_id in [
            "com.example.app;touch /tmp/pwn",
            "com.example.$(id)",
            "com.example.`id`",
            "-p",
            "com..app",
        ] {
            assert!(
                run_device_with(
                    &fixture.adb,
                    "phone-123",
                    application_id,
                    &fixture.apk,
                    SHA256,
                    |_| {},
                )
                .is_err()
            );
        }
        for serial in ["", "-s", "phone 123", "phone;id", "phone\n123"] {
            assert!(
                run_device_with(
                    &fixture.adb,
                    serial,
                    "com.example.app",
                    &fixture.apk,
                    SHA256,
                    |_| {},
                )
                .is_err()
            );
        }
        assert!(fixture.calls().is_empty());
    }

    #[test]
    fn a_launcher_must_belong_to_the_app_and_be_safe_for_adb_shell() {
        for scenario in ["injected_component", "foreign_component"] {
            let fixture = Fixture::new(scenario);
            let error = fixture.run().unwrap_err();
            assert!(error.contains("installed, but"), "{error}");
            assert_eq!(fixture.calls().len(), 4);
        }
        assert!(launcher_component("com.example.app/.Main$Activity", "com.example.app").is_none());
    }

    #[test]
    fn an_exit_zero_activity_error_is_not_reported_as_a_successful_launch() {
        let fixture = Fixture::new("launch_error");
        let error = fixture.run().unwrap_err();
        assert!(
            error.contains("installed, but Android did not confirm launch"),
            "{error}"
        );
    }

    #[test]
    fn adb_missing_and_timeouts_have_actionable_errors() {
        let fixture = Fixture::new("timeout");
        let started = Instant::now();
        let error = adb_output(
            Command::new(&fixture.adb).arg("devices"),
            Duration::from_millis(50),
        )
        .err()
        .unwrap();
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
        let error = adb_output(
            &mut Command::new(fixture.root.join("missing-adb")),
            ADB_PROBE_TIMEOUT,
        )
        .err()
        .unwrap();
        assert!(
            error.contains("Install Android SDK Platform-Tools"),
            "{error}"
        );
    }

    #[test]
    fn cancelling_the_operation_stops_its_adb_client() {
        let fixture = Fixture::new("timeout");
        let scope = OperationScope::new();
        let cancellation = Arc::clone(&scope);
        let _entered = enter_operation(scope);
        let cancel = thread::spawn(move || {
            thread::sleep(Duration::from_millis(75));
            cancellation.cancel();
        });
        let started = Instant::now();
        let error = adb_output(Command::new(&fixture.adb).arg("devices"), ADB_PROBE_TIMEOUT)
            .err()
            .unwrap();
        cancel.join().unwrap();
        assert!(error.contains("stopped"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn another_machine_cannot_interleave_on_the_same_device() {
        let guard = AndroidDeviceGuard::claim("serialized-test-phone").unwrap();
        assert!(AndroidDeviceGuard::claim("serialized-test-phone").is_err());
        assert!(AndroidDeviceGuard::claim("different-test-phone").is_ok());
        drop(guard);
        assert!(AndroidDeviceGuard::claim("serialized-test-phone").is_ok());
    }

    #[test]
    fn changes_to_the_source_during_device_probes_are_refused_before_installing() {
        let fixture = Fixture::new("changed_source");
        let error = fixture.run().unwrap_err();
        assert!(error.contains("checksum or size changed"), "{error}");
        assert_eq!(fixture.calls().len(), 2);
        assert!(!fixture.root.join("adb.installed").exists());
    }

    #[test]
    fn install_failures_keep_the_package_manager_diagnostic_but_drop_the_staging_path() {
        let output = "Performing Streamed Install\nadb: failed to install /tmp/buildbridge-android-apk-4242/app.apk: Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE: not enough space]\n";
        let message = install_error(output, "com.example.app", "phone-123");
        assert!(
            message.starts_with(
                "The APK is ready, but Android could not install com.example.app on phone-123."
            ),
            "{message}"
        );
        assert!(message.contains("Free storage"), "{message}");
        assert!(
            message
                .ends_with("ADB: Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE: not enough space]"),
            "{message}"
        );
        assert!(!message.contains("buildbridge-android-apk"), "{message}");
        let restricted = install_error(
            "Failure [INSTALL_FAILED_USER_RESTRICTED: Install canceled by user]",
            "com.example.app",
            "phone-123",
        );
        assert!(restricted.contains("developer settings"), "{restricted}");
        let unknown = install_error("  adb: device offline  ", "com.example.app", "phone-123");
        assert!(unknown.contains("USB installation prompt"), "{unknown}");
        assert!(unknown.ends_with("ADB: adb: device offline"), "{unknown}");
    }
}
