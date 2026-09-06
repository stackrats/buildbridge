//! Install a retained APK on one explicitly selected host ADB device and launch its activity.

use super::*;

const ADB_PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const ADB_INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
const ADB_OUTPUT_LIMIT: u64 = 256 * 1024;
const MISSING_ADB: &str = "Install Android SDK Platform-Tools on this computer and add adb to PATH, or set ANDROID_HOME to the Android SDK directory, then refresh devices.";
static RUNNING_DEVICES: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidDevice {
    pub serial: String,
    pub state: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidDevices {
    pub available: bool,
    pub devices: Vec<AndroidDevice>,
    pub issue: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidDeviceRunResult {
    pub serial: String,
    pub application_id: String,
    pub sha256: String,
    pub installed: bool,
    pub launched: bool,
}

/// Host ADB sees USB phones and already-running emulators without a toolchain container.
pub fn list_host_android_devices() -> Result<AndroidDevices, String> {
    let Some(adb) = find_adb() else {
        return Ok(AndroidDevices {
            available: false,
            devices: vec![],
            issue: Some(MISSING_ADB.into()),
        });
    };
    list_devices_with(&adb)
}

/// The engine verifies the managed artifact's size and checksum before entering this call.
/// Every device command targets the selected serial; install preserves existing app data.
pub fn run_host_android_device(
    serial: &str,
    application_id: &str,
    apk: &Path,
    expected_sha256: &str,
) -> Result<AndroidDeviceRunResult, String> {
    let adb = find_adb().ok_or_else(|| MISSING_ADB.to_string())?;
    run_device_with(&adb, serial, application_id, apk, expected_sha256)
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
        });
    }
    Ok(AndroidDevices {
        available: true,
        devices: parse_devices(&output.text),
        issue: None,
    })
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
        });
    }
    devices
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

fn run_device_with(
    adb: &Path,
    serial: &str,
    application_id: &str,
    apk: &Path,
    expected_sha256: &str,
) -> Result<AndroidDeviceRunResult, String> {
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
    let _device = AndroidDeviceGuard::claim(serial)?;
    let listing = list_devices_with(adb)?;
    if let Some(issue) = listing.issue {
        return Err(issue);
    }
    let device = listing.devices.iter().find(|device| device.serial == serial)
        .ok_or_else(|| "The selected Android device is no longer connected. Refresh devices and select it again.".to_string())?;
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
    // The source may be edited outside BuildBridge while ADB probes the phone. Install a
    // private copy whose bytes we verify now, rather than reopening that mutable source.
    let snapshot = AndroidApkSnapshot::create(apk, expected_sha256)?;
    let installed = adb_output(
        Command::new(adb)
            .args(["-s", serial, "install", "-r"])
            .arg(snapshot.apk()),
        ADB_INSTALL_TIMEOUT,
    )?;
    if !installed.success || !installed.text.lines().any(|line| line.trim() == "Success") {
        return Err(install_error(&installed.text, application_id, serial));
    }
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
    Ok(AndroidDeviceRunResult {
        serial: serial.into(),
        application_id: application_id.into(),
        sha256: expected_sha256.to_ascii_lowercase(),
        installed: true,
        launched: true,
    })
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
            return Err("Another BuildBridge operation is installing on this Android device. Wait for it to finish.".into());
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
        "This APK has an older version code than the installed app. Build a newer version; BuildBridge kept the existing app and its data."
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
    use std::os::unix::fs::PermissionsExt;

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
            fs::write(&adb, r#"#!/bin/sh
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
else
    printf '%s\n' 'unexpected command' >&2
    exit 2
fi
"#).unwrap();
            fs::set_permissions(&adb, fs::Permissions::from_mode(0o700)).unwrap();
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
            run_device_with(&self.adb, "phone-123", "com.example.app", &self.apk, SHA256)
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

    #[test]
    fn install_and_launch_use_the_exact_selected_device_and_verified_private_apk() {
        let fixture = Fixture::new("success");
        let result = fixture.run().unwrap();
        assert!(result.installed && result.launched);
        assert_eq!(result.serial, "phone-123");
        assert_eq!(result.sha256, SHA256);
        let calls = fixture.calls();
        assert_eq!(calls.len(), 5);
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
                    SHA256
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
                    SHA256
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
