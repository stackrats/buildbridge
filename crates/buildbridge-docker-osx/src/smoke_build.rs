//! The unsigned test build.

use super::*;

pub fn run_apple_smoke_build<F>(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
    target: UnsignedBuildTarget,
    mut on_progress: F,
) -> Result<AppleSmokeBuildResult, ProviderError>
where
    F: FnMut(AppleProjectProgress),
{
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    let started_at = Instant::now();
    let needs_simulator = match target {
        UnsignedBuildTarget::Simulator => "1",
        UnsignedBuildTarget::DeviceSdk => "0",
    };
    let build_destination = unsigned_build_destination_args(target);
    let guest_home = format!("/Users/{username}");
    let workspace = format!("{guest_home}/BuildBridge/workspaces/active");
    let GuestToolchain {
        tools,
        node_root,
        pnpm,
        ruby_root,
        gem_home,
        pod,
        developer_dir,
        path,
    } = guest_toolchain(&guest_home);
    let node_name = format!("node-v{NODE_VERSION}-darwin-x64");
    let node_archive = format!("{tools}/{node_name}.tar.gz");
    let ruby_archive = format!("{tools}/portable-ruby-{PORTABLE_RUBY_VERSION}.tar.gz");

    let script = format!(
        r#"set -u
exec 2>&1
phase() {{ /usr/bin/printf '__BUILDBRIDGE_PHASE__:%s\n' "$1"; }}
job_root="{tools}/jobs"
job_state="$job_root/apple-smoke-build"
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
    trap '' HUP
    (
        finish_job() {{
            worker_status=$?
            /usr/bin/printf '%s\n' "$worker_status" > "$job_status.incoming"
            /bin/mv "$job_status.incoming" "$job_status"
        }}
        trap finish_job EXIT
        set -eu
/bin/test -f "{workspace}/package.json"
/bin/test -d "{workspace}/ios/App/App.xcworkspace"
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
/bin/mkdir -p "{tools}"
if /bin/test ! -x "{node_root}/bin/node"; then
    /bin/rm -rf "{node_root}" "{node_archive}"
    /usr/bin/curl --fail --location --show-error --silent "https://nodejs.org/dist/v{NODE_VERSION}/{node_name}.tar.gz" --output "{node_archive}"
    /usr/bin/shasum -a 256 "{node_archive}" | /usr/bin/grep -q "^{NODE_DARWIN_X64_SHA256}  "
    /usr/bin/tar -xzf "{node_archive}" -C "{tools}"
    /bin/rm -f "{node_archive}"
fi
if /bin/test ! -x "{pnpm}"; then
    "{node_root}/bin/npm" install --prefix "{tools}/pnpm" "pnpm@{PNPM_VERSION}" --no-audit --no-fund
fi
if /bin/test ! -x "{ruby_root}/bin/ruby"; then
    /bin/rm -rf "{ruby_root}" "{ruby_archive}"
    /usr/bin/curl --fail --location --show-error --silent "https://github.com/Homebrew/homebrew-portable-ruby/releases/download/{PORTABLE_RUBY_VERSION}/portable-ruby-{PORTABLE_RUBY_VERSION}.el_capitan.bottle.tar.gz" --output "{ruby_archive}"
    /usr/bin/shasum -a 256 "{ruby_archive}" | /usr/bin/grep -q "^{PORTABLE_RUBY_DARWIN_X64_SHA256}  "
    /usr/bin/tar -xzf "{ruby_archive}" -C "{tools}"
    /bin/rm -f "{ruby_archive}"
    /bin/test -x "{ruby_root}/bin/ruby"
fi
if /bin/test ! -x "{pod}"; then
    /bin/rm -rf "{gem_home}" "{tools}/gems"
    "{ruby_root}/bin/gem" install cocoapods --version "{COCOAPODS_VERSION}" --no-document
    /bin/test -x "{pod}"
fi
platform_installed=0
if /bin/test "{needs_simulator}" -eq 1 && ! /usr/bin/xcrun simctl list runtimes 2>/dev/null | /usr/bin/grep -q '^iOS '; then
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
        if /bin/test -d "$platform_assets"; then
            platform_current_kib=$(/usr/bin/du -sk "$platform_assets" 2>/dev/null | /usr/bin/awk '{{ print $1 }}')
            platform_current=$((platform_current_kib * 1024))
        fi
        platform_stage="locating"
        if /bin/test "$platform_total" -gt 0; then
            platform_stage="downloading"
            if /bin/test "$platform_current" -ge "$platform_total"; then
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
fi

phase installing_dependencies
cd "{workspace}"
"{pnpm}" install --frozen-lockfile --prefer-offline

phase building_web_assets
"{workspace}/node_modules/.bin/vp" build

phase syncing_ios
lock_before=$(/usr/bin/shasum -a 256 "{workspace}/ios/App/Podfile.lock" | /usr/bin/cut -d ' ' -f 1)
"{workspace}/node_modules/.bin/cap" sync ios

phase resolving_pods
cd "{workspace}/ios/App"
"{pod}" install --no-ansi
lock_after=$(/usr/bin/shasum -a 256 "{workspace}/ios/App/Podfile.lock" | /usr/bin/cut -d ' ' -f 1)
if /bin/test "$lock_before" != "$lock_after"; then
    /usr/bin/printf '__BUILDBRIDGE_NATIVE_LOCK_UPDATED__:yes\n'
fi

phase building
build_ios() {{
    /usr/bin/xcodebuild -workspace "{workspace}/ios/App/App.xcworkspace" -scheme App -configuration Debug {build_destination} -derivedDataPath "{workspace}/.buildbridge/DerivedData" CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO COMPILER_INDEX_STORE_ENABLE=NO build
}}
build_status=0
build_ios || build_status=$?
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
phase completed
    ) > "$job_log" 2>&1 < /dev/null &
    job_pid=$!
    /usr/bin/printf '%s\n' "$job_pid" > "$job_state/pid"
    trap - HUP
else
    /usr/bin/printf '__BUILDBRIDGE_REATTACHED__:yes\n'
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
    );

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
    let mut output_tail = Vec::new();
    let mut diagnostic_lines = Vec::new();
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
            let message = "The guest-only Podfile.lock was refreshed to match the synchronized native plugins.".to_string();
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
            let message = "The first compile started before the new Simulator runtime had settled. BuildBridge is verifying CoreSimulator and retrying once.".to_string();
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
        output_tail,
    })
}
