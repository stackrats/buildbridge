//! The browser's own device page handles Android WebView discovery and inspection.
//! Chromium filters internal URLs from command-line arguments. A private, temporary
//! profile opens the fixed inspector URL through the browser's startup preferences.

use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CHROME_INSPECTOR: &str = "chrome://inspect/#devices";
const EDGE_INSPECTOR: &str = "edge://inspect/#devices";
const INSPECTOR_HELP: &str = "Install Chrome, Chromium or Edge, or copy chrome://inspect/#devices into Chrome or Chromium (edge://inspect/#devices in Edge).";

struct Browser {
    executable: PathBuf,
    url: &'static str,
}

struct InspectorProfile(PathBuf);

impl InspectorProfile {
    fn new(url: &'static str) -> Result<Self, String> {
        static NEXT_PROFILE: AtomicU64 = AtomicU64::new(0);
        let temporary_root = std::env::temp_dir();
        if !temporary_root.is_absolute() {
            return Err("The browser's temporary directory must be an absolute path.".to_string());
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("Could not name the inspector profile: {error}"))?
            .as_nanos();
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        for _ in 0..32 {
            let sequence = NEXT_PROFILE.fetch_add(1, Ordering::Relaxed);
            let path = temporary_root.join(format!(
                "buildbridge-inspector-profile-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            match builder.create(&path) {
                Ok(()) => {
                    let profile = Self(path);
                    profile.prepare(url)?;
                    return Ok(profile);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!("Could not create the inspector profile: {error}"));
                }
            }
        }
        Err("Could not create a unique inspector profile. Try again.".to_string())
    }

    fn prepare(&self, url: &'static str) -> Result<(), String> {
        let default = self.0.join("Default");
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&default)
            .map_err(|error| format!("Could not prepare the inspector profile: {error}"))?;
        let preferences = serde_json::to_vec(&serde_json::json!({
            "session": { "restore_on_startup": 4, "startup_urls": [url] }
        }))
        .map_err(|error| format!("Could not encode inspector preferences: {error}"))?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(default.join("Preferences"))
            .and_then(|mut file| file.write_all(&preferences))
            .map_err(|error| format!("Could not write inspector preferences: {error}"))
    }
}

impl Drop for InspectorProfile {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn push_browser(browsers: &mut Vec<Browser>, executable: PathBuf, url: &'static str) {
    if executable.is_absolute() && !browsers.iter().any(|item| item.executable == executable) {
        browsers.push(Browser { executable, url });
    }
}

fn path_browsers(path: Option<&OsStr>) -> Vec<Browser> {
    let mut browsers = Vec::new();
    let names = if cfg!(target_os = "windows") {
        vec![
            ("chrome.exe", CHROME_INSPECTOR),
            ("chromium.exe", CHROME_INSPECTOR),
            ("msedge.exe", EDGE_INSPECTOR),
        ]
    } else {
        vec![
            ("google-chrome", CHROME_INSPECTOR),
            ("google-chrome-stable", CHROME_INSPECTOR),
            ("chromium", CHROME_INSPECTOR),
            ("chromium-browser", CHROME_INSPECTOR),
            ("microsoft-edge", EDGE_INSPECTOR),
            ("microsoft-edge-stable", EDGE_INSPECTOR),
        ]
    };
    let directories: Vec<_> = path
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .filter(|directory| directory.is_absolute())
        .collect();
    for (name, url) in names {
        for directory in &directories {
            push_browser(&mut browsers, directory.join(name), url);
        }
    }
    browsers
}

fn host_browsers() -> Vec<Browser> {
    let mut browsers = Vec::new();
    if cfg!(target_os = "macos") {
        let mut directories = vec![PathBuf::from("/Applications")];
        if let Some(home) = std::env::var_os("HOME") {
            directories.push(PathBuf::from(home).join("Applications"));
        }
        for (binary, url) in [
            (
                "Google Chrome.app/Contents/MacOS/Google Chrome",
                CHROME_INSPECTOR,
            ),
            ("Chromium.app/Contents/MacOS/Chromium", CHROME_INSPECTOR),
            (
                "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
                EDGE_INSPECTOR,
            ),
        ] {
            for directory in &directories {
                push_browser(&mut browsers, directory.join(binary), url);
            }
        }
    } else if cfg!(target_os = "windows") {
        for (binary, url) in [
            ("Google/Chrome/Application/chrome.exe", CHROME_INSPECTOR),
            ("Chromium/Application/chrome.exe", CHROME_INSPECTOR),
            ("Microsoft/Edge/Application/msedge.exe", EDGE_INSPECTOR),
        ] {
            for variable in ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"] {
                if let Some(directory) = std::env::var_os(variable) {
                    push_browser(&mut browsers, PathBuf::from(directory).join(binary), url);
                }
            }
        }
    }
    for browser in path_browsers(std::env::var_os("PATH").as_deref()) {
        push_browser(&mut browsers, browser.executable, browser.url);
    }
    // Desktop launchers sometimes provide a smaller PATH than the person's terminal.
    #[cfg(target_os = "linux")]
    for browser in path_browsers(Some(OsStr::new("/usr/bin:/usr/local/bin:/snap/bin"))) {
        push_browser(&mut browsers, browser.executable, browser.url);
    }
    browsers
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

fn launch(browser: &Browser, startup_wait: Duration) -> Result<(), String> {
    // A separate profile for each click also avoids Chromium replacing configured
    // startup pages with New Tab when the same profile already has a window open.
    let profile = InspectorProfile::new(browser.url)?;
    let mut profile_argument = OsString::from("--user-data-dir=");
    profile_argument.push(&profile.0);
    let mut child = Command::new(&browser.executable)
        .arg(profile_argument)
        .args([
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-mode",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not start the browser: {error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "The browser could not open its inspector ({status})."
                    ))
                };
            }
            Ok(None) if started.elapsed() < startup_wait => {
                std::thread::sleep(Duration::from_millis(25));
            }
            state => {
                // Keep its profile available for the entire browser lifetime, then
                // reap and remove it without making the UI wait for the window to close.
                std::thread::spawn(move || {
                    if child.wait().is_ok() {
                        drop(profile);
                    } else {
                        // If the process state cannot be determined, leave the temporary
                        // profile intact rather than deleting files a browser may still use.
                        std::mem::forget(profile);
                    }
                });
                return state
                    .map(|_| ())
                    .map_err(|error| format!("Could not check the browser launch: {error}"));
            }
        }
    }
}

fn open_in_browsers(browsers: &[Browser], startup_wait: Duration) -> Result<String, String> {
    let mut last_error = None;
    for browser in browsers {
        if !is_executable(&browser.executable) {
            continue;
        }
        match launch(browser, startup_wait) {
            Ok(()) => return Ok(browser.url.to_string()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(match last_error {
        Some(error) => format!("{error} {INSPECTOR_HELP}"),
        None => format!("No supported browser was found. {INSPECTOR_HELP}"),
    })
}

/// Opens only the fixed Android devices page in a supported host browser. The browser
/// selects the device and WebView; this does not change ADB state or enable a CDP server.
pub fn open_android_inspector() -> Result<String, String> {
    open_in_browsers(&host_browsers(), Duration::from_millis(350))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "buildbridge-inspector-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn browser(&self, name: &str, script: &str, url: &'static str) -> Browser {
            let executable = self.0.join(name);
            fs::write(&executable, script).unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            Browser { executable, url }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_absolute_path_entries_are_used_and_browser_urls_match() {
        let candidates = path_browsers(Some(OsStr::new(":relative:/opt/browsers:/usr/bin")));
        assert!(
            candidates
                .iter()
                .all(|browser| browser.executable.is_absolute())
        );
        assert_eq!(
            candidates[0].executable,
            Path::new("/opt/browsers/google-chrome")
        );
        assert_eq!(candidates[0].url, CHROME_INSPECTOR);
        let edge = candidates
            .iter()
            .find(|browser| browser.executable.ends_with("microsoft-edge"))
            .unwrap();
        assert_eq!(edge.url, EDGE_INSPECTOR);
    }

    #[test]
    fn opens_the_fixed_url_through_private_preferences_without_url_arguments() {
        let fixture = Fixture::new();
        let browser = fixture.browser(
            "browser with spaces",
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\nprofile=\"${1#--user-data-dir=}\"\ncat \"$profile/Default/Preferences\" > \"$0.preferences\"\n",
            CHROME_INSPECTOR,
        );
        let args = fixture.0.join("browser with spaces.args");
        let preferences = fixture.0.join("browser with spaces.preferences");
        assert_eq!(
            open_in_browsers(&[browser], Duration::from_secs(1)).unwrap(),
            CHROME_INSPECTOR
        );
        let argv = fs::read_to_string(args).unwrap();
        let arguments: Vec<_> = argv.lines().collect();
        let profile = Path::new(arguments[0].strip_prefix("--user-data-dir=").unwrap());
        assert!(profile.is_absolute());
        assert!(
            !profile.exists(),
            "an exited browser no longer needs its profile"
        );
        assert_eq!(
            &arguments[1..],
            [
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-mode",
            ]
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(preferences).unwrap())
                .unwrap(),
            serde_json::json!({
                "session": { "restore_on_startup": 4, "startup_urls": [CHROME_INSPECTOR] }
            })
        );
    }

    #[test]
    fn falls_back_after_failed_startup_and_returns_edges_url() {
        let fixture = Fixture::new();
        let failing = fixture.browser(
            "chrome",
            "#!/bin/sh\nprintf '%s' \"${1#--user-data-dir=}\" > \"$0.profile\"\nexit 1\n",
            CHROME_INSPECTOR,
        );
        let working = fixture.browser(
            "edge",
            "#!/bin/sh\nprintf '%s' \"${1#--user-data-dir=}\" > \"$0.profile\"\nexit 0\n",
            EDGE_INSPECTOR,
        );
        assert_eq!(
            open_in_browsers(&[failing, working], Duration::from_secs(1)).unwrap(),
            EDGE_INSPECTOR
        );
        let failed_profile = fs::read_to_string(fixture.0.join("chrome.profile")).unwrap();
        let working_profile = fs::read_to_string(fixture.0.join("edge.profile")).unwrap();
        assert_ne!(failed_profile, working_profile);
        assert!(!Path::new(&failed_profile).exists());
        assert!(!Path::new(&working_profile).exists());
    }

    #[test]
    fn profiles_are_unique_private_and_removed_when_released() {
        let first = InspectorProfile::new(CHROME_INSPECTOR).unwrap();
        let second = InspectorProfile::new(EDGE_INSPECTOR).unwrap();
        assert_ne!(first.0, second.0);
        for profile in [&first, &second] {
            assert_eq!(
                fs::metadata(&profile.0).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(profile.0.join("Default/Preferences"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let first_path = first.0.clone();
        drop(first);
        assert!(!first_path.exists());
        assert!(second.0.join("Default/Preferences").is_file());
    }

    #[test]
    fn a_running_browser_keeps_its_profile_until_exit() {
        let fixture = Fixture::new();
        let browser = fixture.browser(
            "running",
            "#!/bin/sh\nprintf '%s' \"${1#--user-data-dir=}\" > \"$0.profile\"\nattempts=0\nwhile [ ! -e \"$0.release\" ] && [ \"$attempts\" -lt 250 ]; do sleep 0.02; attempts=$((attempts + 1)); done\n",
            CHROME_INSPECTOR,
        );
        assert_eq!(
            open_in_browsers(&[browser], Duration::ZERO).unwrap(),
            CHROME_INSPECTOR
        );
        let profile_record = fixture.0.join("running.profile");
        let started = Instant::now();
        let profile = loop {
            if let Ok(path) = fs::read_to_string(&profile_record)
                && !path.is_empty()
            {
                break path;
            }
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "the fixture must start"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let profile = Path::new(&profile);
        let still_exists = profile.join("Default/Preferences").is_file();
        // Release the fixture before asserting, so a failed assertion cannot leave it running.
        fs::write(fixture.0.join("running.release"), []).unwrap();
        assert!(
            still_exists,
            "a live browser must retain its startup preferences"
        );
        let finished = Instant::now();
        while profile.exists() && finished.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !profile.exists(),
            "the reaper must remove the exited browser's profile"
        );
    }

    #[test]
    fn missing_or_failed_browser_explains_manual_recovery() {
        assert!(
            open_in_browsers(&[], Duration::ZERO)
                .unwrap_err()
                .contains("No supported browser")
        );
        let fixture = Fixture::new();
        let failing = fixture.browser("chrome", "#!/bin/sh\nexit 1\n", CHROME_INSPECTOR);
        let error = open_in_browsers(&[failing], Duration::from_secs(1)).unwrap_err();
        assert!(error.contains("could not open its inspector"));
        assert!(error.contains(CHROME_INSPECTOR));
        assert!(error.contains(EDGE_INSPECTOR));
    }
}
