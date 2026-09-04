//! Reading xcodebuild output: phases, diagnostics, retry hints, and redaction.

use super::*;

pub(crate) fn apple_project_phase(value: &str) -> Option<AppleProjectPhase> {
    match value {
        "preparing_tools" => Some(AppleProjectPhase::PreparingTools),
        "preparing_platform" => Some(AppleProjectPhase::PreparingPlatform),
        "installing_dependencies" => Some(AppleProjectPhase::InstallingDependencies),
        "building_web_assets" => Some(AppleProjectPhase::BuildingWebAssets),
        "syncing_ios" => Some(AppleProjectPhase::SyncingIos),
        "resolving_pods" => Some(AppleProjectPhase::ResolvingPods),
        "building" => Some(AppleProjectPhase::Building),
        "completed" => Some(AppleProjectPhase::Completed),
        _ => None,
    }
}

pub(crate) fn apple_platform_progress(value: &str) -> Option<(u64, u64, &'static str)> {
    let value = value.strip_prefix("__BUILDBRIDGE_PLATFORM_PROGRESS__:")?;
    let mut values = value.splitn(3, ':');
    let completed_bytes: u64 = values.next()?.parse().ok()?;
    let total_bytes: u64 = values.next()?.parse().ok()?;
    let detail = match values.next()? {
        "locating" => "Apple is locating the matching iOS Simulator platform",
        "downloading" => "Downloading Apple's iOS Simulator platform",
        "installing" => "Simulator downloaded; macOS is installing and registering it",
        _ => return None,
    };

    Some((completed_bytes.min(total_bytes), total_bytes, detail))
}

pub(crate) fn apple_build_log_is_diagnostic(line: &str) -> bool {
    let normalized = line.to_ascii_lowercase();

    normalized.starts_with("error:")
        || normalized.contains(": error:")
        || normalized.starts_with("make: ***")
        || normalized.contains("extconf failed")
        || normalized.contains("mkmf.rb can't find header files")
        || normalized.contains("com.apple.actool.errors")
        || normalized.contains("failed with a nonzero exit code")
        || normalized.starts_with("** build failed **")
        || normalized.starts_with("the following build commands failed:")
        || normalized.trim_start().starts_with("compileassetcatalog")
}

/// What a failed guest build reports: every diagnostic line kept during the run, then the last
/// lines the guest printed so the failing command is named even when its output matches no
/// known diagnostic shape. Lines already in the tail are not repeated.
pub(crate) fn build_failure_context(diagnostic_lines: &[String], output_tail: &[String]) -> String {
    let tail = &output_tail[output_tail
        .len()
        .saturating_sub(APPLE_BUILD_FAILURE_TAIL_LINES)..];

    diagnostic_lines
        .iter()
        .filter(|line| !tail.contains(line))
        .chain(tail.iter())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Where the pinned toolchain lives under the guest home directory. Every guest recipe derives
/// its paths from here so the test build and the signed archive agree.
pub(crate) struct GuestToolchain {
    pub(crate) tools: String,
    pub(crate) node_root: String,
    pub(crate) pnpm: String,
    pub(crate) ruby_root: String,
    pub(crate) gem_home: String,
    pub(crate) pod: String,
    pub(crate) developer_dir: String,
    /// The `PATH` the recipes export: pinned tools first, then only Apple's system directories.
    pub(crate) path: String,
}

pub(crate) fn guest_toolchain(guest_home: &str) -> GuestToolchain {
    let tools = format!("{guest_home}/.buildbridge/tools");
    let node_root = format!("{tools}/node-v{NODE_VERSION}-darwin-x64");
    let ruby_root = format!("{tools}/portable-ruby/{PORTABLE_RUBY_VERSION}");
    let gem_home = format!("{tools}/gems-ruby-{PORTABLE_RUBY_VERSION}");
    let path = format!(
        "{node_root}/bin:{tools}/pnpm/node_modules/.bin:{ruby_root}/bin:{gem_home}/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    );

    GuestToolchain {
        pnpm: format!("{tools}/pnpm/node_modules/.bin/pnpm"),
        pod: format!("{gem_home}/bin/pod"),
        developer_dir: format!("{guest_home}/Applications/Xcode.app/Contents/Developer"),
        tools,
        node_root,
        ruby_root,
        gem_home,
        path,
    }
}

pub(crate) fn apple_build_retry_detail(value: &str) -> Option<&'static str> {
    match value {
        "__BUILDBRIDGE_BUILD_RETRY__:platform" => {
            Some("Verifying the new Simulator runtime before one automatic retry")
        }
        _ => None,
    }
}

pub(crate) fn phase_detail(phase: AppleProjectPhase) -> &'static str {
    match phase {
        AppleProjectPhase::Snapshotting => "Creating the source snapshot",
        AppleProjectPhase::Transferring => "Synchronizing source",
        AppleProjectPhase::Extracting => "Preparing the guest workspace",
        AppleProjectPhase::PreparingTools => "Preparing Node, pnpm, Ruby, and CocoaPods",
        AppleProjectPhase::PreparingPlatform => {
            "Downloading and installing Apple's iOS Simulator platform"
        }
        AppleProjectPhase::InstallingDependencies => "Installing locked project dependencies",
        AppleProjectPhase::BuildingWebAssets => "Building web assets",
        AppleProjectPhase::SyncingIos => "Synchronizing the Capacitor iOS project",
        AppleProjectPhase::ResolvingPods => "Resolving locked CocoaPods",
        AppleProjectPhase::Building => "Compiling the unsigned iOS app",
        AppleProjectPhase::Completed => "Unsigned test build complete",
    }
}

pub(crate) fn sanitize_build_log_line(line: &str) -> String {
    line.chars()
        .filter(|character| !character.is_control() || matches!(character, '\t'))
        .take(1_000)
        .collect::<String>()
        .trim()
        .to_string()
}
