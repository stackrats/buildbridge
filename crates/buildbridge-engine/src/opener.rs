//! Opening things outside buildbridge: a page in the person's browser, a folder in their file
//! manager. Each is one fixed argument vector handed to a detached process, so the page or
//! folder outlives the operation that opened it and nothing typed anywhere is interpolated
//! into a shell. Which browser is a host setting; which file manager is the desktop's.
//!
//! Three desktops are covered. On Linux `xdg-open` dispatches URLs and paths on scheme and
//! type; on macOS `open` does the same and also launches an application bundle by name; on
//! Windows `explorer` dispatches both. Every difference between them lives here.

use std::path::Path;

use super::*;

#[cfg(target_os = "macos")]
const OPENER: &str = "open";
#[cfg(target_os = "windows")]
const OPENER: &str = "explorer";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const OPENER: &str = "xdg-open";

/// Directories searched beyond `PATH`. A macOS application launched from Finder inherits the
/// bare system path, so Homebrew's browsers are invisible without this; a Linux desktop
/// launcher sometimes hands over a shorter path than the person's terminal.
#[cfg(target_os = "macos")]
const EXTRA_BIN_DIRS: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"];
#[cfg(target_os = "linux")]
const EXTRA_BIN_DIRS: [&str; 3] = ["/usr/bin", "/usr/local/bin", "/snap/bin"];
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const EXTRA_BIN_DIRS: [&str; 0] = [];

/// An address a browser can be handed without surprises: HTTPS anywhere, or HTTP on this
/// host's own loopback, with no credentials embedded. Returns the address as parsed.
pub(crate) fn browsable_url(url: &str) -> Result<String, String> {
    let parsed =
        reqwest::Url::parse(url).map_err(|error| format!("The address is invalid: {error}"))?;
    let loopback = parsed.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || !(parsed.scheme() == "https" || (parsed.scheme() == "http" && loopback))
    {
        return Err(
            "Use an HTTPS address, or HTTP on localhost, without embedded credentials.".to_string(),
        );
    }
    Ok(parsed.to_string())
}

fn is_app_bundle(name: &str) -> bool {
    name.ends_with(".app")
}

/// Whether an application bundle is installed where `open -a` will find it.
#[cfg(target_os = "macos")]
fn app_bundle_installed(name: &str) -> bool {
    let user_apps = std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Applications"));
    [
        "/Applications",
        "/Applications/Utilities",
        "/System/Applications",
    ]
    .iter()
    .map(PathBuf::from)
    .chain(user_apps)
    .any(|directory| directory.join(name).exists())
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
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
    {
        true
    }
}

/// The executable `binary` names inside `dir`; on Windows the `.exe` and `.cmd` forms first.
fn candidate_in(dir: &Path, binary: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    for extension in ["exe", "cmd", "bat"] {
        let path = dir.join(format!("{binary}.{extension}"));
        if path.is_file() {
            return Some(path);
        }
    }
    let path = dir.join(binary);
    is_executable(&path).then_some(path)
}

/// A command name resolved through `PATH` and the extra directories, or a path as given.
pub(crate) fn find_in_path(binary: &str) -> Option<PathBuf> {
    if Path::new(binary).is_absolute() || binary.contains('/') || binary.contains('\\') {
        let path = PathBuf::from(binary);
        return is_executable(&path).then_some(path);
    }
    std::env::var_os("PATH")
        .and_then(|paths| std::env::split_paths(&paths).find_map(|dir| candidate_in(&dir, binary)))
        .or_else(|| {
            EXTRA_BIN_DIRS
                .iter()
                .find_map(|dir| candidate_in(Path::new(dir), binary))
        })
}

/// What a configured browser resolves to on this computer: the executable's path, or the
/// bundle name `open -a` launches. The error names the setting so the person knows where to
/// look, and says what a blank field does.
pub(crate) fn resolve_browser(browser: &str) -> Result<String, String> {
    let browser = browser.trim();
    if browser.is_empty() {
        return Err("Name a browser, or leave the field blank for the default one.".to_string());
    }
    if is_app_bundle(browser) {
        #[cfg(target_os = "macos")]
        {
            return if app_bundle_installed(browser) {
                Ok(browser.to_string())
            } else {
                Err(format!(
                    "{browser} was not found in Applications. Check the name, or leave the browser blank for the default one."
                ))
            };
        }
        #[cfg(not(target_os = "macos"))]
        {
            return Err(format!(
                "{browser} is a macOS application bundle; on this computer name the browser's command or path instead."
            ));
        }
    }
    find_in_path(browser)
        .map(|path| path.to_string_lossy().into_owned())
        .ok_or_else(|| {
            format!(
                "No browser named {browser} was found on this computer. Give its command name or full path, or leave the browser blank for the default one."
            )
        })
}

/// The argument vector that opens `url` in the resolved browser, or in the desktop's default
/// when none is set. Every browser binary takes a URL as its lone argument.
pub(crate) fn browser_argv(browser: Option<&str>, url: &str) -> Vec<String> {
    match browser {
        Some(bundle) if is_app_bundle(bundle) => vec![
            OPENER.to_string(),
            "-a".to_string(),
            bundle.to_string(),
            url.to_string(),
        ],
        Some(browser) => vec![browser.to_string(), url.to_string()],
        None => vec![OPENER.to_string(), url.to_string()],
    }
}

/// Starts a program that outlives this process, with nothing attached to it.
fn spawn_detached(argv: &[String]) -> std::io::Result<()> {
    let (program, arguments) = argv
        .split_first()
        .ok_or_else(|| std::io::Error::other("nothing to run"))?;
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn().map(|_| ())
}

/// Opens a page in the person's browser rather than in a window of this app: a provider's
/// repository or a store console is someone else's page, read best where the person's
/// bookmarks and sign-ins already are.
pub async fn open_url(app: &Engine, url: String) -> Result<(), String> {
    let url = browsable_url(&url)?;
    let browser = match load_settings(app)?.browser {
        Some(configured) => Some(resolve_browser(&configured).map_err(|error| {
            format!("The browser chosen in Settings could not be used. {error}")
        })?),
        None => None,
    };
    let argv = browser_argv(browser.as_deref(), &url);
    tokio::task::spawn_blocking(move || spawn_detached(&argv))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("Could not open the browser: {error}"))
}

/// Shows a folder in the desktop's file manager.
pub(crate) async fn reveal_directory(directory: &Path, what: &str) -> Result<(), String> {
    let directory = directory.to_path_buf();
    tokio::task::spawn_blocking(move || reveal(&directory))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("Could not reveal {what}: {error}"))
}

fn reveal(directory: &Path) -> std::io::Result<()> {
    // `xdg-open` hands a folder to whatever claims `inode/directory`, and editors such as VS
    // Code claim it too, so ask the file manager itself over the freedesktop FileManager1
    // interface. The first D-Bus client installed decides: if the bus has no file manager to
    // answer, the folder goes to `xdg-open` after all.
    #[cfg(target_os = "linux")]
    {
        for argv in file_manager_argvs(&file_uri(directory)) {
            let (program, arguments) = argv.split_first().expect("fixed argv");
            let status = Command::new(program)
                .args(arguments)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            match status {
                Ok(status) if status.success() => return Ok(()),
                Ok(_) => break,
                Err(_) => continue,
            }
        }
    }
    spawn_detached(&[OPENER.to_string(), directory.to_string_lossy().into_owned()])
}

/// FileManager1's `ShowFolders` through `dbus-send`, then `busctl` for a system without the
/// reference tools. Both wait for the reply, and both give up after ten seconds.
#[cfg(target_os = "linux")]
fn file_manager_argvs(uri: &str) -> [Vec<String>; 2] {
    let argv = |parts: &[&str]| parts.iter().map(|part| part.to_string()).collect();
    [
        argv(&[
            "dbus-send",
            "--session",
            "--print-reply",
            "--reply-timeout=10000",
            "--dest=org.freedesktop.FileManager1",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowFolders",
            &format!("array:string:{uri}"),
            "string:",
        ]),
        argv(&[
            "busctl",
            "--user",
            "--timeout=10",
            "call",
            "org.freedesktop.FileManager1",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1",
            "ShowFolders",
            "ass",
            "1",
            uri,
            "",
        ]),
    ]
}

/// A `file://` URI with every byte outside the unreserved set and `/` percent-encoded, which
/// also keeps commas out of `dbus-send`'s comma-separated array syntax.
#[cfg(target_os = "linux")]
fn file_uri(path: &Path) -> String {
    use std::fmt::Write as _;
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~') {
            uri.push(byte as char);
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_loopback_http_without_credentials_is_browsable() {
        for accepted in [
            "https://developer.apple.com/download/all/",
            "http://127.0.0.1:8006/",
            "http://localhost:1421/",
            "http://[::1]:8000/",
        ] {
            assert!(browsable_url(accepted).is_ok(), "{accepted}");
        }
        for refused in [
            "http://example.com/",
            "https://user:secret@example.com/",
            "https://user@example.com/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://",
            "not an address",
        ] {
            assert!(browsable_url(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn the_browser_takes_the_address_as_its_lone_argument() {
        assert_eq!(
            browser_argv(Some("/usr/bin/firefox"), "https://x.test/"),
            vec!["/usr/bin/firefox", "https://x.test/"]
        );
        assert_eq!(
            browser_argv(Some("Firefox.app"), "https://x.test/"),
            vec![OPENER, "-a", "Firefox.app", "https://x.test/"]
        );
        assert_eq!(
            browser_argv(None, "https://x.test/"),
            vec![OPENER, "https://x.test/"]
        );
    }

    #[test]
    fn a_browser_that_is_not_installed_is_refused_by_name() {
        let error = resolve_browser("no-such-browser-here").unwrap_err();
        assert!(error.contains("no-such-browser-here"), "{error}");
        assert!(error.contains("blank"), "{error}");
        assert!(resolve_browser("   ").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_folder_goes_to_the_file_manager_as_one_encoded_uri() {
        let uri = file_uri(Path::new("/home/a b/c,d'e/ü~x.y-z_"));
        assert_eq!(uri, "file:///home/a%20b/c%2Cd%27e/%C3%BC~x.y-z_");
        let [dbus_send, busctl] = file_manager_argvs(&uri);
        assert_eq!(dbus_send[0], "dbus-send");
        assert_eq!(dbus_send[7], format!("array:string:{uri}"));
        assert!(!dbus_send[7].contains(','));
        assert_eq!(busctl[0], "busctl");
        assert_eq!(busctl[8..], ["ass", "1", uri.as_str(), ""]);
    }

    #[cfg(unix)]
    #[test]
    fn a_path_resolves_only_when_it_is_an_executable_file() {
        assert!(find_in_path("/etc/hostname").is_none());
        assert!(find_in_path("/bin/sh").is_some() || find_in_path("/usr/bin/sh").is_some());
        assert!(resolve_browser("sh").is_ok());
    }
}
