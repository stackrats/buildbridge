//! The shell in front of Xcode and Gradle, written from the project's layout. Each build
//! script used to carry one fixed Capacitor recipe — pnpm, `vp build`, `cap sync`, CocoaPods,
//! `ios/App/App.xcworkspace`, `android/app` — inline. The pieces here render the same shell for
//! whatever the layout names: the package manager the project locks with, the web build and the
//! framework's own preparation when there is one, CocoaPods where a Podfile is, the container
//! Xcode opens and the module Gradle builds, and the Flutter tool when the project is Dart.
//!
//! Paths from the project go through single quotes, since a folder in someone's project may be
//! called anything; paths buildbridge chose are plain. Every snippet keeps the phase markers the
//! existing scripts print, so the desktop's progress reads the same for every kind of project.

use super::*;
use crate::android::ANDROID_HTTP_DEBUG_INIT;

/// Where the tools the recipes call live on the machine that builds: under the guest's home on
/// a macOS machine, under the container's on an Android one.
pub(crate) struct RecipeTools {
    pub(crate) tools: String,
    pub(crate) node_root: String,
    pub(crate) pnpm: String,
    /// CocoaPods, on a macOS machine.
    pub(crate) pod: Option<String>,
    pub(crate) macos: bool,
}

impl RecipeTools {
    fn node(&self) -> String {
        format!("{}/bin/node", self.node_root)
    }

    fn npm(&self) -> String {
        format!("{}/bin/npm", self.node_root)
    }

    fn corepack(&self) -> String {
        format!("{}/bin/corepack", self.node_root)
    }

    fn yarn(&self) -> String {
        format!("{}/yarn/node_modules/.bin/yarn", self.tools)
    }

    fn bun(&self) -> String {
        format!("{}/bun/node_modules/.bin/bun", self.tools)
    }

    fn cordova(&self) -> String {
        format!("{}/cordova/node_modules/.bin/cordova", self.tools)
    }

    pub(crate) fn flutter_root(&self) -> String {
        format!("{}/flutter", self.tools)
    }

    pub(crate) fn flutter(&self) -> String {
        format!("{}/bin/flutter", self.flutter_root())
    }

    /// A line that fails unless the file has the pinned digest, with the tool each platform has.
    fn sha256_check(&self, sha256: &str, file: &str) -> String {
        if self.macos {
            format!(r#"/usr/bin/shasum -a 256 "{file}" | /usr/bin/grep -q "^{sha256}  ""#)
        } else {
            format!(
                r#"/usr/bin/printf '%s  %s\n' "{sha256}" "{file}" | /usr/bin/sha256sum --check --status"#
            )
        }
    }

    fn file_sha256(&self, file: &str) -> String {
        if self.macos {
            format!("/usr/bin/shasum -a 256 {file} | /usr/bin/cut -d ' ' -f 1")
        } else {
            format!("/usr/bin/sha256sum {file} | /usr/bin/cut -d ' ' -f 1")
        }
    }
}

/// A relative path from the project, safe to join under the workspace: no absolute root, no
/// step upward, nothing a shell or a line-oriented protocol would misread.
fn valid_relative_path(path: &str, may_be_empty: bool) -> bool {
    if path.is_empty() {
        return may_be_empty;
    }
    path.len() <= 1_024
        && !path.starts_with('/')
        && !path.contains(['\0', '\n', '\r'])
        && !path
            .split('/')
            .any(|component| component.is_empty() || component == "..")
}

fn valid_module_path(path: &str) -> bool {
    path == ":"
        || (path.starts_with(':')
            && path.len() <= 256
            && path[1..].split(':').all(|segment| {
                !segment.is_empty()
                    && segment
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            }))
}

/// Refuses a layout whose names could not have come from detection, before any of them is
/// written into a script.
pub(crate) fn validate_layout(layout: &ProjectLayout) -> Result<(), ProviderError> {
    let refuse = |what: &str| {
        Err(ProviderError::GuestBridge(format!(
            "the approved project's {what} is not a usable path; approve the project again"
        )))
    };
    if let Some(ios) = &layout.ios {
        if !valid_relative_path(&ios.container, false) {
            return refuse("Xcode workspace");
        }
        if !valid_relative_path(&ios.project, false) || !ios.project.ends_with(".xcodeproj") {
            return refuse("Xcode project");
        }
        if let Some(podfile_dir) = &ios.podfile_dir
            && !valid_relative_path(podfile_dir, true)
        {
            return refuse("Podfile directory");
        }
        if !crate::project::valid_scheme_name(&ios.scheme) {
            return Err(ProviderError::GuestBridge(
                "the stored Xcode scheme is missing or invalid".to_string(),
            ));
        }
        for target in &ios.app_targets {
            if target.identifier.len() != 24
                || !target.identifier.chars().all(|c| c.is_ascii_hexdigit())
                || !crate::project::valid_scheme_name(&target.name)
                || target.product.contains(['\'', '\n', '\0'])
            {
                return refuse("Xcode target");
            }
        }
    }
    if let Some(android) = &layout.android {
        if !valid_relative_path(&android.root, true)
            || !valid_relative_path(&android.module_dir, true)
            || !valid_relative_path(&android.script, false)
        {
            return refuse("Gradle module");
        }
        if !valid_module_path(&android.module_path) {
            return refuse("Gradle module path");
        }
    }
    Ok(())
}

/// The absolute path of a project-relative one in the workspace, quoted for the shell.
fn under(workspace: &str, relative: &str) -> String {
    shell_single_quote(&join_relative(workspace, relative))
}

/// The one function that runs a package script, defined for the manager the project uses, so
/// the web build and any other script read the same whichever it is. Yarn 2 and later go
/// through corepack, which reads the version the package declares.
fn js_runner(manager: PackageManager, tools: &RecipeTools) -> String {
    let (pnpm, npm, yarn, bun, corepack) = (
        &tools.pnpm,
        tools.npm(),
        tools.yarn(),
        tools.bun(),
        tools.corepack(),
    );
    match manager {
        PackageManager::Pnpm => format!("js_run() {{ \"{pnpm}\" run \"$@\"; }}"),
        PackageManager::Npm => format!("js_run() {{ \"{npm}\" run \"$@\"; }}"),
        PackageManager::Yarn => format!(
            r#"export COREPACK_HOME="{tools}/corepack"
export COREPACK_ENABLE_DOWNLOAD_PROMPT=0
if /bin/test -f .yarnrc.yml; then js_run() {{ "{corepack}" yarn run "$@"; }}; else js_run() {{ "{yarn}" run "$@"; }}; fi"#,
            tools = tools.tools
        ),
        PackageManager::Bun => format!("js_run() {{ \"{bun}\" run \"$@\"; }}"),
    }
}

/// Installs the project's dependencies: the JavaScript ones with the manager it locks with,
/// installing that manager first when it is not pnpm; the Dart ones with Flutter, installing
/// Flutter first. Nothing for a native project. Ends in the workspace directory.
pub(crate) fn dependencies_script(
    layout: &ProjectLayout,
    workspace: &str,
    tools: &RecipeTools,
) -> String {
    let root = shell_single_quote(workspace);
    if layout.kind == ProjectKind::Flutter {
        let flutter = tools.flutter();
        return format!(
            "phase installing_dependencies\n{}\ncd {root}\n\"{flutter}\" pub get\n",
            flutter_tools_preparation(tools)
        );
    }
    let Some(manager) = layout
        .package_manager
        .filter(|_| layout.kind.uses_javascript())
    else {
        return String::new();
    };
    let (pnpm, npm, yarn, bun, corepack) = (
        &tools.pnpm,
        tools.npm(),
        tools.yarn(),
        tools.bun(),
        tools.corepack(),
    );
    let tools_dir = &tools.tools;
    let install = match manager {
        PackageManager::Pnpm => format!(
            r#"if /bin/test -f pnpm-lock.yaml; then "{pnpm}" install --frozen-lockfile --prefer-offline; else "{pnpm}" install --prefer-offline; fi"#
        ),
        PackageManager::Npm => format!(
            r#"if /bin/test -f package-lock.json || /bin/test -f npm-shrinkwrap.json; then "{npm}" ci --no-audit --no-fund; else "{npm}" install --no-audit --no-fund; fi"#
        ),
        PackageManager::Yarn => format!(
            r#"if /bin/test -f .yarnrc.yml; then
    "{corepack}" yarn install --immutable
else
    if /bin/test ! -x "{yarn}"; then "{npm}" install --prefix "{tools_dir}/yarn" "yarn@{YARN_CLASSIC_VERSION}" --no-audit --no-fund; fi
    if /bin/test -f yarn.lock; then "{yarn}" install --frozen-lockfile --non-interactive; else "{yarn}" install --non-interactive; fi
fi"#
        ),
        PackageManager::Bun => format!(
            r#"if /bin/test ! -x "{bun}"; then "{npm}" install --prefix "{tools_dir}/bun" "bun@{BUN_VERSION}" --no-audit --no-fund; fi
if /bin/test -f bun.lock || /bin/test -f bun.lockb; then "{bun}" install --frozen-lockfile; else "{bun}" install; fi"#
        ),
    };
    format!(
        "phase installing_dependencies\ncd {root}\n{}\n{install}\n",
        js_runner(manager, tools)
    )
}

/// Builds the web assets with the package's `build` script, for the kinds that have any; a
/// package without the script keeps whatever assets it committed.
fn web_assets_script(layout: &ProjectLayout, workspace: &str, tools: &RecipeTools) -> String {
    let Some(manager) = layout
        .package_manager
        .filter(|_| layout.kind.has_web_assets())
    else {
        return String::new();
    };
    let root = shell_single_quote(workspace);
    let node = tools.node();
    format!(
        r#"phase building_web_assets
cd {root}
{}
if "{node}" -e 'const scripts = require(process.argv[1]).scripts || {{}}; process.exit(scripts.build ? 0 : 1)' {root}/package.json; then
    js_run build
fi
"#,
        js_runner(manager, tools)
    )
}

/// Cordova's command line, as shell that leaves it in `cordova_cli`: the project's own when it
/// keeps one in its dependencies, and otherwise the pinned one installed into the tools
/// directory. Cordova is conventionally installed globally rather than locked by the project,
/// so requiring it in the project would fail every app the Cordova command line itself creates.
fn cordova_command(workspace: &str, tools: &RecipeTools) -> String {
    let root = shell_single_quote(workspace);
    let (npm, cordova, tools_dir) = (tools.npm(), tools.cordova(), &tools.tools);
    format!(
        r#"if /bin/test -x {root}/node_modules/.bin/cordova; then
    cordova_cli={root}/node_modules/.bin/cordova
else
    if /bin/test ! -x "{cordova}"; then
        "{npm}" install --prefix "{tools_dir}/cordova" "cordova@{CORDOVA_VERSION}" --no-audit --no-fund
    fi
    cordova_cli="{cordova}"
fi
"#
    )
}

/// The framework's own step between the dependencies and Xcode: `cap sync`, `cordova
/// prepare`, `expo prebuild` when the native project is not committed, Flutter's project
/// configuration. Nothing for React Native, which bundles inside Xcode, and for a native app.
fn ios_framework_script(
    layout: &ProjectLayout,
    workspace: &str,
    tools: &RecipeTools,
    version: Option<&ProjectVersion>,
) -> String {
    let root = shell_single_quote(workspace);
    match layout.kind {
        ProjectKind::Capacitor => format!(
            r#"phase syncing_ios
cd {root}
if /bin/test ! -x {root}/node_modules/.bin/cap; then
    /usr/bin/printf '%s\n' 'The project has no Capacitor command line: add @capacitor/cli to its devDependencies.' >&2
    exit 1
fi
{root}/node_modules/.bin/cap sync ios
"#
        ),
        ProjectKind::Cordova => format!(
            "phase syncing_ios\ncd {root}\n{}\"$cordova_cli\" prepare ios\n",
            cordova_command(workspace, tools)
        ),
        ProjectKind::Expo => {
            let container = layout
                .ios
                .as_ref()
                .map(|ios| under(workspace, &ios.project))
                .unwrap_or_else(|| under(workspace, "ios"));
            format!(
                r#"phase syncing_ios
cd {root}
if /bin/test ! -d {container}; then
    if /bin/test ! -x {root}/node_modules/.bin/expo; then
        /usr/bin/printf '%s\n' 'The project has no Expo command line: add expo to its dependencies.' >&2
        exit 1
    fi
    CI=1 {root}/node_modules/.bin/expo prebuild --platform ios --no-install
fi
"#
            )
        }
        ProjectKind::Flutter => {
            let flutter = tools.flutter();
            let version_args = version
                .map(|version| {
                    format!(
                        " --build-name {} --build-number {}",
                        shell_single_quote(&version.version),
                        shell_single_quote(&version.build)
                    )
                })
                .unwrap_or_default();
            format!(
                r#"phase syncing_ios
cd {root}
"{flutter}" build ios --config-only --no-codesign{version_args}
"#
            )
        }
        // React Native bundles inside Xcode; NativePHP's shell was written by
        // `php artisan native:install` before the folder was approved; a native app has
        // nothing in front of Xcode at all.
        ProjectKind::ReactNative | ProjectKind::NativePhp | ProjectKind::Native => String::new(),
    }
}

/// CocoaPods where the Podfile is. A lock the project committed is compared before and after,
/// and a change is reported with the marker the callers already read.
fn pods_script(layout: &ProjectLayout, workspace: &str, tools: &RecipeTools) -> String {
    let (Some(ios), Some(pod)) = (&layout.ios, &tools.pod) else {
        return String::new();
    };
    let Some(podfile_dir) = &ios.podfile_dir else {
        return String::new();
    };
    let directory = under(workspace, podfile_dir);
    let lock = under(workspace, &join_relative(podfile_dir, "Podfile.lock"));
    let lock_after = if ios.podfile_locked {
        format!(
            r#"lock_after=$({})
if /bin/test "$lock_before" != "$lock_after"; then
    /usr/bin/printf '__BUILDBRIDGE_NATIVE_LOCK_UPDATED__:yes\n'
fi
"#,
            tools.file_sha256(&lock)
        )
    } else {
        String::new()
    };
    format!(
        r#"phase resolving_pods
cd {directory}
"{pod}" install --no-ansi
{lock_after}"#
    )
}

/// Records the committed Podfile.lock's digest before anything can move it.
fn lock_before_script(layout: &ProjectLayout, workspace: &str, tools: &RecipeTools) -> String {
    let Some(ios) = &layout.ios else {
        return String::new();
    };
    let Some(podfile_dir) = ios.podfile_dir.as_ref().filter(|_| ios.podfile_locked) else {
        return String::new();
    };
    let lock = under(workspace, &join_relative(podfile_dir, "Podfile.lock"));
    format!("lock_before=$({})\n", tools.file_sha256(&lock))
}

/// Everything between a fresh snapshot and `xcodebuild`: dependencies, web assets, the
/// framework's step, CocoaPods, and the scheme when the project has none. Phases are printed
/// as the fixed recipe printed them.
pub(crate) fn ios_prepare_script(
    layout: &ProjectLayout,
    workspace: &str,
    tools: &RecipeTools,
    version: Option<&ProjectVersion>,
) -> String {
    format!(
        "{}{}{}{}{}{}",
        lock_before_script(layout, workspace, tools),
        dependencies_script(layout, workspace, tools),
        web_assets_script(layout, workspace, tools),
        ios_framework_script(layout, workspace, tools, version),
        pods_script(layout, workspace, tools),
        ios_scheme_script(layout, workspace),
    )
}

/// What a chosen environment changes without a new snapshot: the web assets and their copy
/// into the native project, for the kinds that have them. A committed lock that moves ends
/// the script with status 3, as the fixed recipe did. Other kinds read the environment at
/// build time, so there is nothing to rebuild.
pub(crate) fn ios_environment_rebuild_script(
    layout: &ProjectLayout,
    workspace: &str,
    tools: &RecipeTools,
) -> String {
    if !layout.kind.has_web_assets() {
        return String::new();
    }
    let lock_check = match &layout.ios {
        Some(ios) if ios.podfile_locked => {
            let podfile_dir = ios.podfile_dir.clone().unwrap_or_default();
            let lock = under(workspace, &join_relative(&podfile_dir, "Podfile.lock"));
            format!(
                r#"lock_after=$({})
if /bin/test "$lock_before" != "$lock_after"; then
    /usr/bin/printf '__BUILDBRIDGE_NATIVE_LOCK_UPDATED__:yes\n'
    exit 3
fi
"#,
                tools.file_sha256(&lock)
            )
        }
        _ => String::new(),
    };
    format!(
        "{}{}{}{lock_check}",
        lock_before_script(layout, workspace, tools),
        web_assets_script(layout, workspace, tools),
        ios_framework_script(layout, workspace, tools, None),
    )
}

/// Writes a shared scheme for the application target when the project carries none, so
/// `xcodebuild -scheme` has one to open; a project that has the scheme is left alone.
pub(crate) fn ios_scheme_script(layout: &ProjectLayout, workspace: &str) -> String {
    let Some(ios) = &layout.ios else {
        return String::new();
    };
    let Some(target) = ios.generated_scheme_target() else {
        return String::new();
    };
    let schemes_dir = under(
        workspace,
        &format!("{}/xcshareddata/xcschemes", ios.project),
    );
    let scheme_file = under(
        workspace,
        &format!(
            "{}/xcshareddata/xcschemes/{}.xcscheme",
            ios.project, ios.scheme
        ),
    );
    let project_name = ios
        .project
        .rsplit('/')
        .next()
        .unwrap_or(&ios.project)
        .to_string();
    let scheme = generated_scheme_xml(target, &project_name);
    format!(
        r#"if /bin/test ! -f {scheme_file}; then
    /bin/mkdir -p {schemes_dir}
    /usr/bin/printf '%s\n' {} > {scheme_file}
fi
"#,
        shell_single_quote(&scheme)
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The scheme Xcode would create for the target: build for every action, run Debug, archive
/// Release.
pub(crate) fn generated_scheme_xml(target: &XcodeTarget, project_name: &str) -> String {
    let identifier = xml_escape(&target.identifier);
    let name = xml_escape(&target.name);
    let product = xml_escape(&target.product);
    let container = xml_escape(project_name);
    let reference = format!(
        r#"<BuildableReference BuildableIdentifier = "primary" BlueprintIdentifier = "{identifier}" BuildableName = "{product}" BlueprintName = "{name}" ReferencedContainer = "container:{container}"></BuildableReference>"#
    );
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Scheme LastUpgradeVersion = "1500" version = "1.7">
   <BuildAction parallelizeBuildables = "YES" buildImplicitDependencies = "YES">
      <BuildActionEntries>
         <BuildActionEntry buildForTesting = "YES" buildForRunning = "YES" buildForProfiling = "YES" buildForArchiving = "YES" buildForAnalyzing = "YES">
            {reference}
         </BuildActionEntry>
      </BuildActionEntries>
   </BuildAction>
   <TestAction buildConfiguration = "Debug" selectedDebuggerIdentifier = "Xcode.DebuggerFoundation.Debugger.LLDB" selectedLauncherIdentifier = "Xcode.DebuggerFoundation.Launcher.LLDB" shouldUseLaunchSchemeArgsEnv = "YES">
      <Testables>
      </Testables>
   </TestAction>
   <LaunchAction buildConfiguration = "Debug" selectedDebuggerIdentifier = "Xcode.DebuggerFoundation.Debugger.LLDB" selectedLauncherIdentifier = "Xcode.DebuggerFoundation.Launcher.LLDB" launchStyle = "0" useCustomWorkingDirectory = "NO" ignoresPersistentStateOnLaunch = "NO" debugDocumentVersioning = "YES" debugServiceExtension = "internal" allowLocationSimulation = "YES">
      <BuildableProductRunnable runnableDebuggingMode = "0">
         {reference}
      </BuildableProductRunnable>
   </LaunchAction>
   <ProfileAction buildConfiguration = "Release" shouldUseLaunchSchemeArgsEnv = "YES" savedToolIdentifier = "" useCustomWorkingDirectory = "NO" debugDocumentVersioning = "YES">
      <BuildableProductRunnable runnableDebuggingMode = "0">
         {reference}
      </BuildableProductRunnable>
   </ProfileAction>
   <AnalyzeAction buildConfiguration = "Debug">
   </AnalyzeAction>
   <ArchiveAction buildConfiguration = "Release" revealArchiveInOrganizer = "YES">
   </ArchiveAction>
</Scheme>"#
    )
}

/// The absolute path of what Xcode opens, in the workspace.
pub(crate) fn ios_container_path(layout: &ProjectLayout, workspace: &str) -> String {
    layout
        .ios
        .as_ref()
        .map(|ios| join_relative(workspace, &ios.container))
        .unwrap_or_else(|| format!("{workspace}/ios/App/App.xcworkspace"))
}

/// `-workspace 'path'` or `-project 'path'`, for a command line.
pub(crate) fn ios_container_args(layout: &ProjectLayout, workspace: &str) -> String {
    let flag = match layout.ios.as_ref().map(|ios| ios.container_kind) {
        Some(XcodeContainerKind::Project) => "-project",
        _ => "-workspace",
    };
    format!(
        "{flag} {}",
        shell_single_quote(&ios_container_path(layout, workspace))
    )
}

/// The build settings a requested version travels as on `xcodebuild`'s command line. A
/// Flutter app takes its version from the generated configuration, which the framework step
/// already wrote from the same request; the settings are still passed so a target that reads
/// them directly agrees.
pub(crate) fn ios_version_settings(layout: &ProjectLayout, version: &ProjectVersion) -> String {
    let mut settings = format!(
        " MARKETING_VERSION={} CURRENT_PROJECT_VERSION={}",
        shell_single_quote(&version.version),
        shell_single_quote(&version.build)
    );
    if layout.kind == ProjectKind::Flutter {
        settings.push_str(&format!(
            " FLUTTER_BUILD_NAME={} FLUTTER_BUILD_NUMBER={}",
            shell_single_quote(&version.version),
            shell_single_quote(&version.build)
        ));
    }
    settings
}

/// The Android module's Gradle root, absolute and unquoted.
pub(crate) fn android_gradle_dir(layout: &ProjectLayout, workspace: &str) -> String {
    layout
        .android
        .as_ref()
        .map(|android| join_relative(workspace, &android.root))
        .unwrap_or_else(|| format!("{workspace}/android"))
}

/// Where the module's outputs land: Gradle's `build` beside the module, or the project's own
/// `build/<module>` for a Flutter app, which moves the Android build there.
pub(crate) fn android_build_dir(layout: &ProjectLayout, workspace: &str) -> String {
    match &layout.android {
        Some(android) if layout.kind == ProjectKind::Flutter => {
            join_relative(workspace, &join_relative("build", &android.module_dir))
        }
        Some(android) => join_relative(
            workspace,
            &join_relative(&android.module_relative(), "build"),
        ),
        None => format!("{workspace}/android/app/build"),
    }
}

/// Whether the project commits its own Gradle wrapper. Cordova's generated Android project
/// does not, and neither do some hand-made ones; those are built with the Gradle buildbridge
/// supplies instead.
pub fn android_has_wrapper(layout: &ProjectLayout) -> bool {
    layout
        .android
        .as_ref()
        .is_none_or(|android| android.wrapper)
}

/// A Gradle task addressed to the application module alone.
pub(crate) fn android_task(layout: &ProjectLayout, task: &str) -> String {
    layout
        .android
        .as_ref()
        .map(|android| android.task(task))
        .unwrap_or_else(|| format!(":app:{task}"))
}

/// The module's build script in the workspace, relative to the workspace.
pub(crate) fn android_script_relative(layout: &ProjectLayout) -> String {
    layout
        .android
        .as_ref()
        .map(|android| android.script.clone())
        .unwrap_or_else(|| "android/app/build.gradle".to_string())
}

/// The framework's step before Gradle: `cap sync`, `cordova prepare`, `expo prebuild` when the
/// native project is not committed, Flutter's `local.properties`. React Native bundles inside
/// Gradle, with the init script that makes its debug build bundle too.
fn android_framework_script(
    layout: &ProjectLayout,
    workspace: &str,
    tools: &RecipeTools,
    version: Option<&ProjectVersion>,
    release: bool,
) -> String {
    let root = shell_single_quote(workspace);
    let gradle_dir = shell_single_quote(&android_gradle_dir(layout, workspace));
    match layout.kind {
        ProjectKind::Capacitor => format!(
            r#"phase syncing_android
cd {root}
if /bin/test ! -x {root}/node_modules/.bin/cap; then
    /usr/bin/printf '%s\n' 'The project has no Capacitor command line: add @capacitor/cli to its devDependencies.' >&2
    exit 1
fi
{root}/node_modules/.bin/cap sync android
"#
        ),
        ProjectKind::Cordova => format!(
            "phase syncing_android\ncd {root}\n{}\"$cordova_cli\" prepare android\n",
            cordova_command(workspace, tools)
        ),
        ProjectKind::Expo => format!(
            r#"phase syncing_android
cd {root}
if /bin/test ! -f {gradle_dir}/gradlew; then
    if /bin/test ! -x {root}/node_modules/.bin/expo; then
        /usr/bin/printf '%s\n' 'The project has no Expo command line: add expo to its dependencies.' >&2
        exit 1
    fi
    CI=1 {root}/node_modules/.bin/expo prebuild --platform android --no-install
fi
/bin/chmod 755 {gradle_dir}/gradlew
"#
        ),
        ProjectKind::Flutter => {
            let flutter_root = tools.flutter_root();
            let mode = if release { "release" } else { "debug" };
            let version_lines = version
                .map(|version| {
                    format!(
                        "/usr/bin/printf 'flutter.versionName=%s\\nflutter.versionCode=%s\\n' {} {} >> {gradle_dir}/local.properties\n",
                        shell_single_quote(&version.version),
                        shell_single_quote(&version.build)
                    )
                })
                .unwrap_or_default();
            format!(
                r#"phase syncing_android
cd {root}
/usr/bin/printf 'sdk.dir=%s\nflutter.sdk=%s\nflutter.buildMode={mode}\n' "$ANDROID_HOME" "{flutter_root}" > {gradle_dir}/local.properties
{version_lines}"#
            )
        }
        // The same three that need nothing before Xcode need nothing before Gradle.
        ProjectKind::ReactNative | ProjectKind::NativePhp | ProjectKind::Native => String::new(),
    }
}

/// Everything between a fresh snapshot and Gradle. Ends in the workspace directory.
pub(crate) fn android_prepare_script(
    layout: &ProjectLayout,
    workspace: &str,
    tools: &RecipeTools,
    version: Option<&ProjectVersion>,
    release: bool,
) -> String {
    format!(
        "{}{}{}",
        dependencies_script(layout, workspace, tools),
        web_assets_script(layout, workspace, tools),
        android_framework_script(layout, workspace, tools, version, release),
    )
}

/// What a chosen environment changes without a new snapshot, for the kinds with web assets.
pub(crate) fn android_environment_rebuild_script(
    layout: &ProjectLayout,
    workspace: &str,
    tools: &RecipeTools,
) -> String {
    if !layout.kind.has_web_assets() {
        return String::new();
    }
    format!(
        "{}{}",
        web_assets_script(layout, workspace, tools),
        android_framework_script(layout, workspace, tools, None, true),
    )
}

/// The init scripts a Gradle build runs under: the HTTP override when the debug build allows
/// it, and React Native's debug bundling. Each is written to a private temporary directory
/// for this one invocation and removed after it, so no later build inherits either.
pub(crate) fn android_gradle_command(
    layout: &ProjectLayout,
    gradle: &str,
    tasks: &str,
    allow_http: bool,
) -> String {
    let mut scripts: Vec<(&str, &str)> = Vec::new();
    if allow_http {
        scripts.push(("http.gradle", ANDROID_HTTP_DEBUG_INIT));
    }
    if matches!(layout.kind, ProjectKind::ReactNative | ProjectKind::Expo) {
        scripts.push(("react-native.gradle", ANDROID_REACT_NATIVE_INIT));
    }
    if scripts.is_empty() {
        return format!("{gradle} --no-daemon --console=plain {tasks}");
    }
    // mktemp keeps the scripts outside any source set, in a private directory so the template
    // ends in the X's that both GNU and BSD mktemp replace. Only this invocation points
    // Gradle at them; even SIGKILL cannot make later builds inherit them.
    let mut written = String::new();
    let mut flags = String::new();
    for (name, contents) in scripts {
        written.push_str(&format!(
            "/bin/cat > \"$init_dir/{name}\" <<'BUILDBRIDGE_GRADLE_INIT'\n{contents}\nBUILDBRIDGE_GRADLE_INIT\n"
        ));
        flags.push_str(&format!(" --init-script \"$init_dir/{name}\""));
    }
    format!(
        r#"init_dir=$(/usr/bin/mktemp -d /tmp/buildbridge-gradle-init.XXXXXX)
{written}if {gradle} --no-daemon --console=plain --no-configuration-cache{flags} {tasks}; then
    /bin/rm -rf "$init_dir"
else
    init_status=$?
    /bin/rm -rf "$init_dir"
    exit "$init_status"
fi"#
    )
}

/// Installs Flutter into the tools directory when a Flutter project is built for the first
/// time: the pinned stable release, checked against its digest, plus what its tool itself
/// needs on the Android image, which ships without git or xz.
pub(crate) fn flutter_tools_preparation(tools: &RecipeTools) -> String {
    let flutter_root = tools.flutter_root();
    let flutter = tools.flutter();
    let tools_dir = &tools.tools;
    let base = "https://storage.googleapis.com/flutter_infra_release/releases";
    if tools.macos {
        let archive = format!("{tools_dir}/flutter_macos_{FLUTTER_VERSION}-stable.zip");
        let check = tools.sha256_check(FLUTTER_MACOS_X64_SHA256, &archive);
        format!(
            r#"if /bin/test ! -x "{flutter}"; then
    /bin/rm -rf "{flutter_root}" "{archive}"
    /usr/bin/curl --fail --location --show-error --silent "{base}/stable/macos/flutter_macos_{FLUTTER_VERSION}-stable.zip" --output "{archive}"
    {check}
    /usr/bin/ditto -x -k "{archive}" "{tools_dir}"
    /bin/rm -f "{archive}"
    /bin/test -x "{flutter}"
    "{flutter}" config --no-analytics --no-cli-animations > /dev/null
fi
export FLUTTER_ROOT="{flutter_root}"
export PATH="{flutter_root}/bin:$PATH""#
        )
    } else {
        let archive = format!("{tools_dir}/flutter_linux_{FLUTTER_VERSION}-stable.tar.xz");
        let check = tools.sha256_check(FLUTTER_LINUX_X64_SHA256, &archive);
        format!(
            r#"if ! command -v git > /dev/null 2>&1 || ! command -v xz > /dev/null 2>&1; then
    export DEBIAN_FRONTEND=noninteractive
    /usr/bin/apt-get update -qq
    /usr/bin/apt-get install -qq -y --no-install-recommends git xz-utils > /dev/null
fi
if /bin/test ! -x "{flutter}"; then
    /bin/rm -rf "{flutter_root}" "{archive}"
    /usr/bin/curl --fail --location --show-error --silent "{base}/stable/linux/flutter_linux_{FLUTTER_VERSION}-stable.tar.xz" --output "{archive}"
    {check}
    /usr/bin/tar -xJf "{archive}" -C "{tools_dir}"
    /bin/rm -f "{archive}"
    /bin/test -x "{flutter}"
    /usr/bin/git config --global --add safe.directory "{flutter_root}"
    "{flutter}" config --no-analytics --no-cli-animations > /dev/null
fi
export FLUTTER_ROOT="{flutter_root}"
export PATH="{flutter_root}/bin:$PATH""#
        )
    }
}

const ANDROID_REACT_NATIVE_INIT: &str = include_str!("android_react_native.gradle");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{AndroidProject, IosProject, PackageManager, XcodeContainerKind};

    fn recipe_tools(macos: bool) -> RecipeTools {
        RecipeTools {
            tools: "/home/b/.buildbridge/tools".to_string(),
            node_root: "/home/b/.buildbridge/tools/node".to_string(),
            pnpm: "/home/b/.buildbridge/tools/pnpm/node_modules/.bin/pnpm".to_string(),
            pod: macos.then(|| "/home/b/.buildbridge/tools/gems/bin/pod".to_string()),
            macos,
        }
    }

    fn native_ios(scheme_shared: bool) -> ProjectLayout {
        ProjectLayout {
            kind: ProjectKind::Native,
            name: "Weather".to_string(),
            package_manager: None,
            ios: Some(IosProject {
                container: "Weather.xcodeproj".to_string(),
                container_kind: XcodeContainerKind::Project,
                project: "Weather.xcodeproj".to_string(),
                scheme: "Weather".to_string(),
                schemes: if scheme_shared {
                    vec!["Weather".to_string()]
                } else {
                    Vec::new()
                },
                app_targets: vec![XcodeTarget {
                    name: "Weather".to_string(),
                    identifier: "504EC2FC1FED79650016851F".to_string(),
                    product: "Weather.app".to_string(),
                }],
                podfile_dir: None,
                podfile_locked: false,
            }),
            android: None,
        }
    }

    #[test]
    fn the_capacitor_default_renders_the_recipe_the_fixed_scripts_had() {
        let layout = ProjectLayout::capacitor_default();
        let tools = recipe_tools(true);
        let script = ios_prepare_script(
            &layout,
            "/Users/b/BuildBridge/workspaces/active",
            &tools,
            None,
        );
        assert!(script.contains("phase installing_dependencies"), "{script}");
        assert!(
            script.contains("install --frozen-lockfile --prefer-offline"),
            "{script}"
        );
        assert!(script.contains("phase building_web_assets"), "{script}");
        assert!(script.contains("js_run build"), "{script}");
        assert!(script.contains("phase syncing_ios"), "{script}");
        assert!(
            script.contains("node_modules/.bin/cap sync ios"),
            "{script}"
        );
        assert!(script.contains("phase resolving_pods"), "{script}");
        assert!(
            script.contains("cd '/Users/b/BuildBridge/workspaces/active/ios/App'"),
            "{script}"
        );
        assert!(
            script.contains("__BUILDBRIDGE_NATIVE_LOCK_UPDATED__:yes"),
            "{script}"
        );
        assert!(script.starts_with("lock_before=$(/usr/bin/shasum -a 256 '/Users/b/BuildBridge/workspaces/active/ios/App/Podfile.lock'"), "{script}");
        assert!(
            !script.contains("xcscheme"),
            "the App scheme is the project's own"
        );
        assert_eq!(
            ios_container_args(&layout, "/Users/b/BuildBridge/workspaces/active"),
            "-workspace '/Users/b/BuildBridge/workspaces/active/ios/App/App.xcworkspace'"
        );
        let rebuild = ios_environment_rebuild_script(&layout, "/w", &tools);
        assert!(rebuild.contains("exit 3"), "{rebuild}");
        assert!(rebuild.contains("cap sync ios"), "{rebuild}");
        assert!(!rebuild.contains("installing_dependencies"), "{rebuild}");
        assert_eq!(android_gradle_dir(&layout, "/root/w"), "/root/w/android");
        assert_eq!(
            android_build_dir(&layout, "/root/w"),
            "/root/w/android/app/build"
        );
        assert_eq!(android_task(&layout, "assembleDebug"), ":app:assembleDebug");
        assert!(android_has_wrapper(&layout));
        assert_eq!(
            android_gradle_command(&layout, "./gradlew", ":app:assembleDebug", false),
            "./gradlew --no-daemon --console=plain :app:assembleDebug"
        );
        let with_http = android_gradle_command(&layout, "./gradlew", ":app:assembleDebug", true);
        assert!(
            with_http.contains("--init-script \"$init_dir/http.gradle\""),
            "{with_http}"
        );
        assert!(!with_http.contains("react-native.gradle"), "{with_http}");
    }

    #[test]
    fn a_native_project_gets_no_javascript_and_a_generated_scheme() {
        let layout = native_ios(false);
        let tools = recipe_tools(true);
        let script = ios_prepare_script(&layout, "/Users/b/w", &tools, None);
        assert!(!script.contains("installing_dependencies"), "{script}");
        assert!(!script.contains("resolving_pods"), "{script}");
        assert!(
            script.contains("Weather.xcodeproj/xcshareddata/xcschemes/Weather.xcscheme"),
            "{script}"
        );
        assert!(
            script.contains("BlueprintIdentifier = \"504EC2FC1FED79650016851F\""),
            "{script}"
        );
        assert!(
            script.contains("ReferencedContainer = \"container:Weather.xcodeproj\""),
            "{script}"
        );
        assert_eq!(
            ios_container_args(&layout, "/Users/b/w"),
            "-project '/Users/b/w/Weather.xcodeproj'"
        );
        assert!(ios_environment_rebuild_script(&layout, "/Users/b/w", &tools).is_empty());
        assert!(ios_scheme_script(&native_ios(true), "/Users/b/w").is_empty());
        validate_layout(&layout).unwrap();
    }

    #[test]
    fn the_other_package_managers_install_themselves_first() {
        let mut layout = ProjectLayout::capacitor_default();
        let tools = recipe_tools(false);
        layout.package_manager = Some(PackageManager::Yarn);
        let script = dependencies_script(&layout, "/root/w", &tools);
        assert!(script.contains("yarn@1.22.22"), "{script}");
        assert!(
            script.contains("corepack\" yarn install --immutable"),
            "{script}"
        );
        layout.package_manager = Some(PackageManager::Bun);
        let script = dependencies_script(&layout, "/root/w", &tools);
        assert!(script.contains("bun@1.4.2"), "{script}");
        assert!(
            script.contains("bun\" install --frozen-lockfile"),
            "{script}"
        );
        layout.package_manager = Some(PackageManager::Npm);
        let script = dependencies_script(&layout, "/root/w", &tools);
        assert!(script.contains("npm\" ci --no-audit --no-fund"), "{script}");
        assert!(
            script.contains("js_run() { \"/home/b/.buildbridge/tools/node/bin/npm\" run \"$@\"; }"),
            "{script}"
        );
    }

    #[test]
    fn react_native_and_flutter_prepare_their_own_way() {
        let mut layout = ProjectLayout::capacitor_default();
        layout.kind = ProjectKind::ReactNative;
        layout.package_manager = Some(PackageManager::Npm);
        let tools = recipe_tools(false);
        let script = android_prepare_script(&layout, "/root/w", &tools, None, false);
        assert!(script.contains("installing_dependencies"), "{script}");
        assert!(!script.contains("building_web_assets"), "{script}");
        assert!(!script.contains("syncing_android"), "{script}");
        let command = android_gradle_command(&layout, "./gradlew", ":app:assembleDebug", false);
        assert!(command.contains("react-native.gradle"), "{command}");
        assert!(command.contains("debuggableVariants"), "{command}");
        assert!(android_environment_rebuild_script(&layout, "/root/w", &tools).is_empty());

        layout.kind = ProjectKind::Flutter;
        layout.package_manager = None;
        let version = ProjectVersion {
            version: "1.2.3".to_string(),
            build: "4".to_string(),
        };
        let script = android_prepare_script(&layout, "/root/w", &tools, Some(&version), true);
        assert!(
            script.contains("apt-get install -qq -y --no-install-recommends git xz-utils"),
            "{script}"
        );
        assert!(
            script.contains("flutter_linux_3.47.2-stable.tar.xz"),
            "{script}"
        );
        assert!(script.contains(FLUTTER_LINUX_X64_SHA256), "{script}");
        assert!(script.contains("\" pub get"), "{script}");
        assert!(script.contains("flutter.buildMode=release"), "{script}");
        assert!(
            script.contains("flutter.versionName=%s\\nflutter.versionCode=%s\\n' '1.2.3' '4'"),
            "{script}"
        );
        assert_eq!(android_build_dir(&layout, "/root/w"), "/root/w/build/app");
        let ios = ios_prepare_script(&layout, "/Users/b/w", &recipe_tools(true), Some(&version));
        assert!(ios.contains("flutter_macos_3.47.2-stable.zip"), "{ios}");
        assert!(
            ios.contains(
                "build ios --config-only --no-codesign --build-name '1.2.3' --build-number '4'"
            ),
            "{ios}"
        );
        assert!(ios.contains("resolving_pods"), "{ios}");
        assert!(ios_version_settings(&layout, &version).contains("FLUTTER_BUILD_NAME='1.2.3'"));
        assert!(
            !ios_version_settings(&ProjectLayout::capacitor_default(), &version)
                .contains("FLUTTER")
        );
    }

    #[test]
    fn a_cordova_project_uses_its_own_command_line_or_the_pinned_one() {
        let mut layout = ProjectLayout::capacitor_default();
        layout.kind = ProjectKind::Cordova;
        layout.package_manager = Some(PackageManager::Npm);
        let script = android_prepare_script(&layout, "/root/w", &recipe_tools(false), None, false);
        assert!(
            script.contains("cordova_cli='/root/w'/node_modules/.bin/cordova"),
            "{script}"
        );
        assert!(
            script.contains(&format!("cordova@{CORDOVA_VERSION}")),
            "{script}"
        );
        assert!(
            script.contains("\"$cordova_cli\" prepare android"),
            "{script}"
        );
        let ios = ios_prepare_script(&layout, "/Users/b/w", &recipe_tools(true), None);
        assert!(ios.contains("\"$cordova_cli\" prepare ios"), "{ios}");
        // Cordova's generated project commits no wrapper, so Gradle comes from the toolchain.
        layout.android.as_mut().unwrap().wrapper = false;
        assert!(!android_has_wrapper(&layout));
        let supplied = android_gradle_command(
            &layout,
            "'/tools/gradle-x/bin/gradle'",
            ":app:assembleDebug",
            false,
        );
        assert_eq!(
            supplied,
            "'/tools/gradle-x/bin/gradle' --no-daemon --console=plain :app:assembleDebug"
        );
    }

    #[test]
    fn expo_writes_the_native_projects_when_they_are_missing() {
        let mut layout = ProjectLayout::capacitor_default();
        layout.kind = ProjectKind::Expo;
        let ios = ios_prepare_script(&layout, "/Users/b/w", &recipe_tools(true), None);
        assert!(
            ios.contains("expo prebuild --platform ios --no-install"),
            "{ios}"
        );
        let android = android_prepare_script(&layout, "/root/w", &recipe_tools(false), None, false);
        assert!(
            android.contains("expo prebuild --platform android --no-install"),
            "{android}"
        );
        assert!(
            android.contains("/bin/chmod 755 '/root/w/android'/gradlew"),
            "{android}"
        );
    }

    #[test]
    fn project_paths_are_quoted_and_checked() {
        let mut layout = ProjectLayout::capacitor_default();
        layout.ios.as_mut().unwrap().container = "ios/My App's/App.xcworkspace".to_string();
        assert_eq!(
            ios_container_args(&layout, "/w"),
            "-workspace '/w/ios/My App'\"'\"'s/App.xcworkspace'"
        );
        validate_layout(&layout).unwrap();
        layout.ios.as_mut().unwrap().project = "../escape.xcodeproj".to_string();
        assert!(validate_layout(&layout).is_err());
        let mut layout = ProjectLayout::capacitor_default();
        layout.android.as_mut().unwrap().module_path = ":app; rm -rf /".to_string();
        assert!(validate_layout(&layout).is_err());
        let mut layout = ProjectLayout::capacitor_default();
        layout.android.as_mut().unwrap().root = "/etc".to_string();
        assert!(validate_layout(&layout).is_err());
        let mut root_module = ProjectLayout::capacitor_default();
        let android = root_module.android.as_mut().unwrap();
        android.root = String::new();
        android.module_dir = String::new();
        android.module_path = ":".to_string();
        android.script = "build.gradle".to_string();
        validate_layout(&root_module).unwrap();
        assert_eq!(android_gradle_dir(&root_module, "/w"), "/w");
        assert_eq!(android_build_dir(&root_module, "/w"), "/w/build");
        assert_eq!(
            android_task(&root_module, "assembleDebug"),
            ":assembleDebug"
        );
    }

    #[test]
    fn the_generated_scheme_escapes_what_it_quotes() {
        let target = XcodeTarget {
            name: "My App".to_string(),
            identifier: "504EC2FC1FED79650016851F".to_string(),
            product: "My <App>.app".to_string(),
        };
        let xml = generated_scheme_xml(&target, "My App.xcodeproj");
        assert!(
            xml.contains("BuildableName = \"My &lt;App&gt;.app\""),
            "{xml}"
        );
        assert!(xml.contains("BlueprintName = \"My App\""), "{xml}");
        assert!(
            xml.contains("<ArchiveAction buildConfiguration = \"Release\""),
            "{xml}"
        );
    }

    #[test]
    fn a_gradle_project_without_a_layout_falls_back_to_the_capacitor_paths() {
        let layout = ProjectLayout {
            kind: ProjectKind::Native,
            name: "x".to_string(),
            package_manager: None,
            ios: None,
            android: Some(AndroidProject {
                root: "android".to_string(),
                module_dir: "apps/phone".to_string(),
                module_path: ":apps:phone".to_string(),
                modules: vec![":apps:phone".to_string()],
                script: "android/apps/phone/build.gradle.kts".to_string(),
                wrapper: true,
            }),
        };
        assert_eq!(
            android_build_dir(&layout, "/w"),
            "/w/android/apps/phone/build"
        );
        assert_eq!(
            android_task(&layout, "bundleRelease"),
            ":apps:phone:bundleRelease"
        );
        assert_eq!(
            android_script_relative(&layout),
            "android/apps/phone/build.gradle.kts"
        );
        assert!(dependencies_script(&layout, "/w", &recipe_tools(false)).is_empty());
    }
}
