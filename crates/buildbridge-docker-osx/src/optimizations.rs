//! Guest optimizations: the catalogue, checking which apply, and applying one.

use super::*;
use ts_rs::TS;

/// How much a guest optimization changes about the machine's security posture, in the words
/// the osx-optimizer project uses for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationTier {
    /// Faster builds, no meaningful change to who can do what on the machine.
    Recommended,
    /// Trades some protection for convenience; the source marks these "at your own risk".
    AtYourOwnRisk,
    /// Removes authentication inside the guest. Only defensible for a VM nothing else can reach.
    ExtremelyInsecure,
}

/// One tweak from sickcodes/osx-optimizer as BuildBridge can run it: a fixed script for the
/// guest, a check that reports whether it is already in effect, and the caveat the source gives.
/// Admin tweaks run in the guest's own Terminal, where `sudo` reads the password from its TTY;
/// BuildBridge never sees it.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GuestOptimization {
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub tier: OptimizationTier,
    /// The caveat, in the source's words where it gives one.
    pub warning: Option<&'static str>,
    pub needs_admin: bool,
    #[serde(skip)]
    pub apply: &'static str,
    /// Prints `applied` or `not_applied`; anything else reads as unknown.
    #[serde(skip)]
    pub check: &'static str,
}

pub fn guest_optimizations() -> &'static [GuestOptimization] {
    &[
        GuestOptimization {
            id: "disable-spotlight",
            title: "Disable Spotlight indexing",
            summary: "Stops the indexer that otherwise churns through every synchronized project and every Xcode install. The single biggest win for a virtual machine.",
            tier: OptimizationTier::Recommended,
            warning: Some(
                "Spotlight stops finding apps and files; `sudo mdutil -i on -a` turns it back on.",
            ),
            needs_admin: true,
            apply: "/usr/bin/sudo /usr/bin/mdutil -i off -a",
            check: "if /usr/bin/mdutil -s / 2>/dev/null | /usr/bin/grep -qi 'disabled'; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "performance-mode",
            title: "Enable performance mode",
            summary: "Sets Apple's server performance mode in the boot arguments, which dedicates more system resources to long-running processes such as builds.",
            tier: OptimizationTier::Recommended,
            warning: Some("Takes effect after the next restart of the guest."),
            needs_admin: true,
            apply: r#"/usr/bin/sudo /usr/sbin/nvram boot-args="serverperfmode=1 $(/usr/sbin/nvram boot-args 2>/dev/null | /usr/bin/cut -f 2-)""#,
            check: "if /usr/sbin/nvram boot-args 2>/dev/null | /usr/bin/grep -q 'serverperfmode=1'; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "reduce-motion",
            title: "Reduce motion and transparency",
            summary: "Turns off the animations and blur the console window otherwise has to render through QEMU.",
            tier: OptimizationTier::Recommended,
            warning: None,
            needs_admin: false,
            apply: "/usr/bin/defaults write com.apple.Accessibility DifferentiateWithoutColor -int 1 && /usr/bin/defaults write com.apple.Accessibility ReduceMotionEnabled -int 1 && /usr/bin/defaults write com.apple.universalaccess reduceMotion -int 1 && /usr/bin/defaults write com.apple.universalaccess reduceTransparency -int 1",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read com.apple.universalaccess reduceMotion 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "no-state-restore",
            title: "Do not restore apps on login",
            summary: "Stops macOS reopening whatever was running at shutdown, so a restart comes up clean and faster.",
            tier: OptimizationTier::Recommended,
            warning: Some("This may be slower for you depending on what you are doing."),
            needs_admin: false,
            apply: "/usr/bin/defaults write com.apple.loginwindow TALLogoutSavesState -bool false",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read com.apple.loginwindow TALLogoutSavesState 2>/dev/null)\" = 0; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "no-app-nap",
            title: "Keep apps from sleeping",
            summary: "Disables App Nap so background processes such as Xcode are never throttled into a sleeping state.",
            tier: OptimizationTier::Recommended,
            warning: Some("This increases RAM usage."),
            needs_admin: false,
            apply: "/usr/bin/defaults write NSGlobalDomain NSAppSleepDisabled -bool YES",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read NSGlobalDomain NSAppSleepDisabled 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "lighter-login",
            title: "Lighter login screen",
            summary: "Drops the login wallpaper and shows a plain name and password prompt instead of a list of users.",
            tier: OptimizationTier::Recommended,
            warning: None,
            needs_admin: true,
            apply: r#"/usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.loginwindow DesktopPicture "" && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.loginwindow.plist SHOWFULLNAME -bool true && /usr/bin/defaults write com.apple.loginwindow AllowList -string '*'"#,
            check: "if /usr/bin/test \"$(/usr/bin/defaults read /Library/Preferences/com.apple.loginwindow SHOWFULLNAME 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "disable-updates",
            title: "Disable software updates",
            summary: "Stops macOS downloading multi-gigabyte updates in the background, which is what makes a virtual disk grow out of proportion.",
            tier: OptimizationTier::AtYourOwnRisk,
            warning: Some(
                "At your own risk: the guest stops receiving security updates. Update it deliberately instead.",
            ),
            needs_admin: true,
            apply: "/usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.SoftwareUpdate AutomaticDownload -bool false && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.SoftwareUpdate AutomaticCheckEnabled -bool false && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.SoftwareUpdate ConfigDataInstall -int 0 && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.SoftwareUpdate CriticalUpdateInstall -int 0 && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.SoftwareUpdate ScheduleFrequency -int 0 && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.commerce AutoUpdate -bool false && /usr/bin/sudo /usr/bin/defaults write /Library/Preferences/com.apple.commerce AutoUpdateRestartRequired -bool false",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read /Library/Preferences/com.apple.SoftwareUpdate AutomaticDownload 2>/dev/null)\" = 0; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "auto-login",
            title: "Skip the login screen",
            summary: "Logs the console straight into the user account at boot instead of stopping at the login window.",
            tier: OptimizationTier::AtYourOwnRisk,
            warning: Some("At your own risk: anyone who can see the console is logged in."),
            needs_admin: false,
            apply: "/usr/bin/defaults write com.apple.loginwindow autoLoginUser -bool true",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read com.apple.loginwindow autoLoginUser 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "disable-screen-lock",
            title: "Disable screen locking",
            summary: "Keeps the console session unlocked so a build is never waiting behind a lock screen.",
            tier: OptimizationTier::AtYourOwnRisk,
            warning: Some("The console never asks for a password again once it is logged in."),
            needs_admin: false,
            apply: "/usr/bin/defaults write com.apple.loginwindow DisableScreenLock -bool true",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read com.apple.loginwindow DisableScreenLock 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "osascript-over-ssh",
            title: "Allow automation over SSH",
            summary: "Lets scripts run over SSH drive apps with osascript without the accessibility and full-disk-access prompts.",
            tier: OptimizationTier::AtYourOwnRisk,
            warning: Some("Anything that can open an SSH session can then automate the desktop."),
            needs_admin: false,
            apply: "/usr/bin/defaults write com.apple.universalaccessAuthWarning /System/Applications/Utilities/Terminal.app -bool true && /usr/bin/defaults write com.apple.universalaccessAuthWarning /usr/libexec -bool true && /usr/bin/defaults write com.apple.universalaccessAuthWarning /usr/libexec/sshd-keygen-wrapper -bool true && /usr/bin/defaults write com.apple.universalaccessAuthWarning com.apple.Terminal -bool true",
            check: "if /usr/bin/test \"$(/usr/bin/defaults read com.apple.universalaccessAuthWarning /usr/libexec/sshd-keygen-wrapper 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "multi-sessions",
            title: "Enable multiple sessions",
            summary: "Allows more than one user session at a time, so a console login and an SSH-driven build do not fight over the one session.",
            tier: OptimizationTier::AtYourOwnRisk,
            warning: None,
            needs_admin: true,
            apply: r#"/usr/bin/sudo /usr/bin/defaults write .GlobalPreferences MultipleSessionsEnabled -bool TRUE && /usr/bin/defaults write "Apple Global Domain" MultipleSessionsEnabled -bool true"#,
            check: "if /usr/bin/test \"$(/usr/bin/defaults read 'Apple Global Domain' MultipleSessionsEnabled 2>/dev/null)\" = 1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "remote-management",
            title: "Enable remote management",
            summary: "Turns on Apple Remote Desktop screen sharing for every user, so the guest can be watched and driven without the QEMU console.",
            tier: OptimizationTier::AtYourOwnRisk,
            warning: Some(
                "At your own risk: every account on the guest can then be reached over the network.",
            ),
            needs_admin: true,
            apply: "/usr/bin/sudo /System/Library/CoreServices/RemoteManagement/ARDAgent.app/Contents/Resources/kickstart -activate -configure -access -off -restart -agent -privs -all -allowAccessFor -allUsers",
            check: "if /usr/bin/pgrep -x ARDAgent >/dev/null 2>&1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
        GuestOptimization {
            id: "disable-passwords",
            title: "Disable passwords globally",
            summary: "Rewrites every PAM policy so no password is ever required: everyone is root, sudo never asks, and SSH password login accepts an empty password.",
            tier: OptimizationTier::ExtremelyInsecure,
            warning: Some(
                "These macOS optimizations should only be used in CI/CD, behind a VPN, and with no external connectivity. This is not a warning, it is absolutely essential, or anyone can just SSH into the remote mac.",
            ),
            needs_admin: true,
            apply: r#"for PAM_FILE in /etc/pam.d/*; do /usr/bin/sudo /usr/bin/sed -i -e 's/required/optional/g' -e 's/sufficient/optional/g' "$PAM_FILE"; done"#,
            check: "if /usr/bin/grep -q 'required' /etc/pam.d/sudo 2>/dev/null; then /usr/bin/printf not_applied; else /usr/bin/printf applied; fi",
        },
        GuestOptimization {
            id: "everyone-sudoer",
            title: "Make every user a passwordless sudoer",
            summary: "Writes a NOPASSWD sudoers rule for every account under /Users, so scripts can use sudo without a password.",
            tier: OptimizationTier::ExtremelyInsecure,
            warning: Some(
                "These macOS optimizations should only be used in CI/CD, behind a VPN, and with no external connectivity. Any account on the guest becomes root without a password.",
            ),
            needs_admin: true,
            apply: r#"for USER_DIR in /Users/*; do REAL_NAME=$(/usr/bin/basename "$USER_DIR"); if /usr/bin/test "$REAL_NAME" = Shared; then continue; fi; /usr/bin/printf '%s ALL=(ALL) NOPASSWD: ALL\n' "$REAL_NAME" | /usr/bin/sudo /usr/bin/tee "/etc/sudoers.d/$REAL_NAME" >/dev/null; /usr/bin/sudo /bin/chmod 440 "/etc/sudoers.d/$REAL_NAME"; done"#,
            check: "if /usr/bin/sudo -n /usr/bin/true >/dev/null 2>&1; then /usr/bin/printf applied; else /usr/bin/printf not_applied; fi",
        },
    ]
}

pub fn guest_optimization(id: &str) -> Option<&'static GuestOptimization> {
    guest_optimizations().iter().find(|item| item.id == id)
}

/// Reports which optimizations are in effect, in one round trip: every check prints its id
/// and state on its own line, and a check that cannot run reads as unknown rather than as
/// "not applied", so nothing is offered as undone when the truth is unknowable.
pub fn check_guest_optimizations(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<Vec<(&'static str, Option<bool>)>, ProviderError> {
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    let script = guest_optimizations()
        .iter()
        .map(|item| {
            format!(
                "/usr/bin/printf '%s=' {}; ({}) 2>/dev/null || /usr/bin/printf unknown; /usr/bin/printf '\\n'",
                shell_single_quote(item.id),
                item.check
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let output = run_guest_command(ssh_port, username, identity_path, known_hosts_path, &script)?;

    Ok(guest_optimizations()
        .iter()
        .map(|item| {
            let state = output
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{}=", item.id)))
                .map(str::trim);
            let applied = match state {
                Some("applied") => Some(true),
                Some("not_applied") => Some(false),
                _ => None,
            };
            (item.id, applied)
        })
        .collect())
}

/// Applies one optimization. A user-level tweak runs straight over the bridge; an admin tweak
/// opens the guest's Terminal with a fixed command file so `sudo` reads the password from its
/// own TTY. Either way the script is the catalogue's, verbatim.
pub fn apply_guest_optimization<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    optimization_id: &str,
    mut on_waiting: F,
) -> Result<(), ProviderError>
where
    F: FnMut(u64),
{
    let item = guest_optimization(optimization_id).ok_or_else(|| {
        ProviderError::GuestBridge("that optimization is not in the catalogue".to_string())
    })?;
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;

    if !item.needs_admin {
        run_guest_command(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            item.apply,
        )?;
        return Ok(());
    }

    let task_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let guest_cache = format!("/Users/{username}/Library/Caches/dev.buildbridge.desktop");
    let status_path = format!("{guest_cache}/optimize-{task_id}.status");
    let script_path = format!("{guest_cache}/optimize-{task_id}.command");
    let terminal_script = format!(
        r#"#!/bin/zsh
/usr/bin/clear
/usr/bin/printf "BuildBridge: {title}\n\n"
/usr/bin/printf "Enter the local macOS login password when sudo asks.\n"
/usr/bin/printf "The password remains inside this macOS Terminal.\n\n"
trap "/usr/bin/printf \"failed:interrupted\\n\" > {status_path}" EXIT
if ( {apply} ); then
    trap - EXIT
    /usr/bin/printf "success\n" > {status_path}
    /usr/bin/printf "\nDone. You can close this window.\n"
else
    result=$?
    trap - EXIT
    /usr/bin/printf "failed:%s\n" "$result" > {status_path}
    /usr/bin/printf "\nThat did not complete. Return to BuildBridge.\n"
fi
read -k 1 "?Press any key to close this window."
"#,
        title = item.title,
        apply = item.apply,
    );
    let run = run_guest_terminal_script(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &GuestTerminalScript {
            guest_cache: &guest_cache,
            status_path: &status_path,
            script_path: &script_path,
            script: &terminal_script,
            timeout_seconds: 900,
            spawn_failure: "could not open the guest Terminal",
        },
        &mut |elapsed| on_waiting(elapsed),
    )?;
    if !run.status_success {
        return Err(ProviderError::GuestBridge(clean_output(&run.stderr)));
    }
    match clean_output(&run.stdout).as_str() {
        "success" => Ok(()),
        "launch_failed" => Err(ProviderError::GuestBridge(
            "macOS could not open a Terminal window; log in on the console and try again"
                .to_string(),
        )),
        "timeout" => Err(ProviderError::GuestBridge(
            "no password was entered in the guest Terminal within 15 minutes".to_string(),
        )),
        other => Err(ProviderError::GuestBridge(format!(
            "the guest reported: {}",
            if other.is_empty() { "no result" } else { other }
        ))),
    }
}

/// Stops any BuildBridge job still running inside the guest after its host-side operation was
/// cancelled. The test build deliberately survives a dropped SSH session so a desktop restart
/// can reattach; a Stop button must reach past that.
pub fn stop_guest_jobs(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<(), ProviderError> {
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    let jobs = format!("/Users/{username}/.buildbridge/tools/jobs");
    let script = format!(
        "for pid_file in {}/*/pid; do if /bin/test -f \"$pid_file\"; then pid=$(/bin/cat \"$pid_file\"); pgid=$(/bin/ps -o pgid= -p \"$pid\" 2>/dev/null | /usr/bin/tr -d ' '); if /bin/test -n \"$pgid\"; then /bin/kill -TERM -- \"-$pgid\" 2>/dev/null || /bin/true; fi; /bin/kill -TERM \"$pid\" 2>/dev/null || /bin/true; fi; done; /bin/rm -rf {}/*; /usr/bin/true",
        shell_single_quote(&jobs),
        shell_single_quote(&jobs)
    );
    run_guest_command(ssh_port, username, identity_path, known_hosts_path, &script).map(|_| ())
}
