//! The unsigned test build.

use super::*;

#[allow(clippy::too_many_arguments)]
pub fn run_apple_smoke_build<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    layout: &ProjectLayout,
    target: UnsignedBuildTarget,
    version: Option<&ProjectVersion>,
    mut on_progress: F,
) -> Result<AppleSmokeBuildResult, ProviderError>
where
    F: FnMut(AppleProjectProgress),
{
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    validate_layout(layout)?;
    let scheme = layout
        .ios
        .as_ref()
        .map(|ios| ios.scheme.clone())
        .ok_or_else(|| {
            ProviderError::GuestBridge("the approved project has no iOS project".to_string())
        })?;
    if let Some(version) = version {
        validate_apple_version(version).map_err(ProviderError::GuestBridge)?;
    }
    let started_at = Instant::now();
    let needs_simulator = match target {
        UnsignedBuildTarget::Simulator => "1",
        UnsignedBuildTarget::DeviceSdk => "0",
    };
    let build_destination = unsigned_build_destination_args(target);
    // The requested version rides on xcodebuild's command line as build settings, the way the
    // archive carries it in its settings file; the built app is checked against it below.
    let version_settings = version
        .map(|version| ios_version_settings(layout, version))
        .unwrap_or_default();
    let products_dir = match target {
        UnsignedBuildTarget::DeviceSdk => "Debug-iphoneos",
        UnsignedBuildTarget::Simulator => "Debug-iphonesimulator",
    };
    let guest_home = format!("/Users/{username}");
    let workspace = format!("{guest_home}/BuildBridge/workspaces/active");
    let toolchain = guest_toolchain(&guest_home);
    let prepare_tools = guest_tools_preparation(
        &toolchain,
        layout.kind.uses_javascript(),
        layout
            .ios
            .as_ref()
            .is_some_and(|ios| ios.podfile_dir.is_some()),
    );
    let prepare_project =
        ios_prepare_script(layout, &workspace, &toolchain.recipe_tools(), version);
    let container_args = ios_container_args(layout, &workspace);
    let scheme_argument = shell_single_quote(&scheme);
    let GuestToolchain {
        tools,
        gem_home,
        developer_dir,
        path,
        ..
    } = &toolchain;

    let body = format!(
        r#"phase() {{ /usr/bin/printf '__BUILDBRIDGE_PHASE__:%s\n' "$1"; }}
/bin/test -d "{workspace}"
export PATH="{path}"
export GEM_HOME="{gem_home}"
export GEM_PATH="{gem_home}"
export DEVELOPER_DIR="{developer_dir}"
export LANG="en_US.UTF-8"
export CYPRESS_INSTALL_BINARY=0
if /bin/test -f "{workspace}/.buildbridge/env.sh"; then
    . "{workspace}/.buildbridge/env.sh"
fi

phase preparing_tools
{prepare_tools}
platform_installed=0
# Apple's iOS platform: the Simulator runtime, which since Xcode 15 is a separate download and
# which newer Xcodes require before they accept even the generic device destination.
install_ios_platform() {{
    phase preparing_platform
    platform_assets="/System/Library/AssetsV2/com_apple_MobileAsset_iOSSimulatorRuntime"
    platform_catalog="$platform_assets/com_apple_MobileAsset_iOSSimulatorRuntime.xml"
    platform_log="{tools}/ios-platform-install.log"
    platform_total=0
    /bin/rm -f "$platform_log"
    /usr/bin/xcodebuild -downloadPlatform iOS -architectureVariant universal > "$platform_log" 2>&1 &
    platform_pid=$!
    while /bin/kill -0 "$platform_pid" 2>/dev/null; do
        platform_total=0
        if /bin/test -f "$platform_catalog"; then
            platform_total=$(/usr/bin/plutil -p "$platform_catalog" 2>/dev/null | /usr/bin/awk '/"_DownloadSize"/ {{ if ($3 > max) max = $3 }} END {{ printf "%.0f", max }}')
        fi
        platform_current=0
        # xcodebuild reports its own count, "(10.56 GB of 10.6 GB)", after carriage returns;
        # that is the truth wherever macOS puts the bytes. The asset directory is the fallback.
        platform_report=$(/usr/bin/tr '\r' '\n' < "$platform_log" 2>/dev/null | /usr/bin/grep -o '([0-9.]* [kMG]B of [0-9.]* [kMG]B)' | /usr/bin/tail -1)
        if /bin/test -n "$platform_report"; then
            platform_current=$(/usr/bin/printf '%s\n' "$platform_report" | /usr/bin/awk '{{ gsub(/[()]/, ""); printf "%.0f", $1 * (($2 ~ /^k/) ? 1000 : ($2 ~ /^M/) ? 1000000 : 1000000000) }}')
            platform_total=$(/usr/bin/printf '%s\n' "$platform_report" | /usr/bin/awk '{{ gsub(/[()]/, ""); printf "%.0f", $4 * (($5 ~ /^k/) ? 1000 : ($5 ~ /^M/) ? 1000000 : 1000000000) }}')
        elif /bin/test -d "$platform_assets"; then
            platform_current_kib=$(/usr/bin/du -sk "$platform_assets" 2>/dev/null | /usr/bin/awk '{{ print $1 }}')
            platform_current=$((platform_current_kib * 1024))
        fi
        platform_stage="locating"
        if /bin/test "$platform_total" -gt 0; then
            platform_stage="downloading"
            if /bin/test "$platform_current" -ge "$platform_total" || /usr/bin/grep -q 'Installing' "$platform_log" 2>/dev/null; then
                platform_current="$platform_total"
                platform_stage="installing"
            fi
        fi
        /usr/bin/printf '__BUILDBRIDGE_PLATFORM_PROGRESS__:%s:%s:%s\n' "$platform_current" "$platform_total" "$platform_stage"
        /bin/sleep 2
    done
    platform_status=0
    wait "$platform_pid" || platform_status=$?
    /bin/cat "$platform_log"
    /bin/rm -f "$platform_log"
    /bin/test "$platform_status" -eq 0
    if /bin/test "$platform_total" -gt 0; then
        /usr/bin/printf '__BUILDBRIDGE_PLATFORM_PROGRESS__:%s:%s:installing\n' "$platform_total" "$platform_total"
    fi
    /usr/bin/xcrun simctl list runtimes | /usr/bin/grep -q '^iOS '
    platform_installed=1
}}
if /bin/test "{needs_simulator}" -eq 1 && ! /usr/bin/xcrun simctl list runtimes 2>/dev/null | /usr/bin/grep -q '^iOS '; then
    install_ios_platform
fi

{prepare_project}
phase building
cd "{workspace}"
build_ios() {{
    /usr/bin/xcodebuild {container_args} -scheme {scheme_argument} -configuration Debug {build_destination} -derivedDataPath "{workspace}/.buildbridge/DerivedData" CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO COMPILER_INDEX_STORE_ENABLE=NO{version_settings} build
}}
build_log="{tools}/apple-build.log"
build_status_file="{tools}/apple-build.status"
# The output streams on while the exit status lands in a file, whichever shell runs this.
build_and_log() {{
    /bin/rm -f "$build_status_file"
    ( build_ios 2>&1; /bin/echo "$?" > "$build_status_file" ) | /usr/bin/tee "$build_log"
    build_status=$(/bin/cat "$build_status_file" 2>/dev/null || /bin/echo 1)
}}
build_status=0
build_and_log
if /bin/test "$build_status" -ne 0 && /bin/test "$platform_installed" -eq 0 && /usr/bin/grep -q 'is not installed' "$build_log"; then
    /usr/bin/printf '__BUILDBRIDGE_BUILD_RETRY__:platform_missing\n'
    install_ios_platform
    build_status=0
    build_and_log
fi
/bin/rm -f "$build_log" "$build_status_file"
if /bin/test "$build_status" -ne 0 && /bin/test "$platform_installed" -eq 1; then
    /usr/bin/printf '__BUILDBRIDGE_BUILD_RETRY__:platform\n'
    readiness_attempt=0
    while /bin/test "$readiness_attempt" -lt 6; do
        if /usr/bin/xcrun simctl list runtimes 2>/dev/null | /usr/bin/grep -q '^iOS ' && /usr/bin/xcrun simctl list devices available >/dev/null 2>&1; then
            break
        fi
        readiness_attempt=$((readiness_attempt + 1))
        /bin/sleep 2
    done
    /bin/sleep 5
    build_status=0
    build_ios || build_status=$?
fi
/bin/test "$build_status" -eq 0
app_bundle=$(/usr/bin/find "{workspace}/.buildbridge/DerivedData/Build/Products/{products_dir}" -maxdepth 1 -type d -name '*.app' 2>/dev/null | /usr/bin/head -n 1)
if /bin/test -n "$app_bundle" && /bin/test -f "$app_bundle/Info.plist"; then
    app_version=$(/usr/bin/plutil -extract CFBundleShortVersionString raw -o - "$app_bundle/Info.plist" 2>/dev/null || /bin/echo '')
    app_build=$(/usr/bin/plutil -extract CFBundleVersion raw -o - "$app_bundle/Info.plist" 2>/dev/null || /bin/echo '')
    /usr/bin/printf '__BUILDBRIDGE_APP_VERSION__\t%s\t%s\n' "$app_version" "$app_build"
fi
phase completed"#
    );
    let script = crate::device_run::guest_job_script("apple-smoke-build", tools, "''", &body);

    let mut child = guest_ssh_command(ssh_port, username, identity_path, known_hosts_path)
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::GuestBridge(format!("could not start the Apple test build: {error}"))
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::GuestBridge("could not capture Apple build output".to_string())
    })?;
    let mut phase = AppleProjectPhase::PreparingTools;
    let mut native_lockfile_updated = false;
    let mut built_version: Option<ProjectVersion> = None;
    let mut output_tail = Vec::new();
    let mut diagnostic_lines = Vec::new();
    if let Some(version) = version {
        output_tail.push(format!("Building as version {}.", version.display()));
    }
    on_progress(apple_progress(
        phase,
        0,
        0,
        started_at,
        phase_detail(phase),
        None,
    ));

    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| {
            ProviderError::GuestBridge(format!("could not read Apple build output: {error}"))
        })?;
        if let Some(value) = line.strip_prefix("__BUILDBRIDGE_PHASE__:") {
            phase = apple_project_phase(value).ok_or_else(|| {
                ProviderError::GuestBridge("the guest returned an unknown build phase".to_string())
            })?;
            on_progress(apple_progress(
                phase,
                0,
                0,
                started_at,
                phase_detail(phase),
                None,
            ));
            continue;
        }
        if line == "__BUILDBRIDGE_NATIVE_LOCK_UPDATED__:yes" {
            native_lockfile_updated = true;
            let message =
                "The guest's Podfile.lock was refreshed to match the resolved native dependencies."
                    .to_string();
            output_tail.push(message.clone());
            on_progress(apple_progress(
                phase,
                0,
                0,
                started_at,
                phase_detail(phase),
                Some(message),
            ));
            continue;
        }
        if let Some(rest) = line.strip_prefix("__BUILDBRIDGE_APP_VERSION__\t") {
            let mut fields = rest.split('\t');
            if let (Some(app_version), Some(app_build)) = (fields.next(), fields.next())
                && valid_release_value(app_version)
                && valid_release_value(app_build)
            {
                built_version = Some(ProjectVersion {
                    version: app_version.to_string(),
                    build: app_build.to_string(),
                });
            }
            continue;
        }
        if line == "__BUILDBRIDGE_REATTACHED__:yes" {
            let message = "Reattached to the build already running inside macOS.".to_string();
            output_tail.push(message.clone());
            on_progress(apple_progress(
                phase,
                0,
                0,
                started_at,
                "Reconnecting to the active guest build",
                Some(message),
            ));
            continue;
        }
        if let Some(retry_detail) = apple_build_retry_detail(&line) {
            let message = if line.ends_with(":platform_missing") {
                "Xcode refused the device destination because its iOS platform is not installed. buildbridge is downloading the platform once, then building again.".to_string()
            } else {
                "The first compile started before the new Simulator runtime had settled. buildbridge is verifying CoreSimulator and retrying once.".to_string()
            };
            output_tail.push(message.clone());
            on_progress(apple_progress(
                AppleProjectPhase::Building,
                0,
                0,
                started_at,
                retry_detail,
                Some(message),
            ));
            continue;
        }
        if let Some((completed_bytes, total_bytes, detail)) = apple_platform_progress(&line) {
            phase = AppleProjectPhase::PreparingPlatform;
            on_progress(apple_progress(
                phase,
                completed_bytes,
                total_bytes,
                started_at,
                detail,
                None,
            ));
            continue;
        }

        let line = sanitize_build_log_line(&line);
        if line.is_empty() {
            continue;
        }
        let is_diagnostic = apple_build_log_is_diagnostic(&line);
        if is_diagnostic {
            diagnostic_lines.push(line.clone());
            if diagnostic_lines.len() > APPLE_BUILD_DIAGNOSTIC_LINES {
                diagnostic_lines.remove(0);
            }
        }
        output_tail.push(line.clone());
        if output_tail.len() > APPLE_BUILD_OUTPUT_TAIL_LINES {
            output_tail.remove(0);
        }
        on_progress(apple_progress(
            phase,
            0,
            0,
            started_at,
            phase_detail(phase),
            Some(line),
        ));
    }

    let status = child.wait().map_err(|error| {
        ProviderError::GuestBridge(format!("could not finish the Apple test build: {error}"))
    })?;
    if status.code() != Some(255) {
        let cleanup = format!("/bin/rm -rf '{tools}/jobs/apple-smoke-build'");
        let _ = run_guest_command(
            ssh_port,
            username,
            identity_path,
            known_hosts_path,
            &cleanup,
        );
    }
    if !status.success() {
        let context = build_failure_context(&diagnostic_lines, &output_tail);
        return Err(ProviderError::GuestBridge(if context.is_empty() {
            format!("the Apple test build failed during {}", phase_detail(phase))
        } else {
            format!(
                "the Apple test build failed during {}:\n{context}",
                phase_detail(phase)
            )
        }));
    }

    if let Some(version) = version
        && built_version.as_ref() != Some(version)
    {
        return Err(ProviderError::GuestBridge(format!(
            "the test build reports version {}, not the requested {}; the app's Info.plist must take CFBundleShortVersionString from MARKETING_VERSION and CFBundleVersion from CURRENT_PROJECT_VERSION for a version set here to reach it",
            built_version
                .as_ref()
                .map(ProjectVersion::display)
                .unwrap_or_else(|| "unknown".to_string()),
            version.display()
        )));
    }

    let xcode_version = parse_xcode_version(&run_guest_command(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        "/usr/bin/xcodebuild -version",
    )?);
    on_progress(apple_progress(
        AppleProjectPhase::Completed,
        0,
        0,
        started_at,
        unsigned_build_completed_detail(target),
        None,
    ));

    Ok(AppleSmokeBuildResult {
        target,
        xcode_version,
        native_lockfile_updated,
        version: built_version,
        output_tail,
    })
}
