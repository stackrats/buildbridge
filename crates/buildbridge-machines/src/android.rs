//! The Android toolchain container: a machine's third provider, and the first without a
//! virtual machine. Android's SDK, Gradle and the JDK run in a Linux amd64 container, so what a machine
//! needs is a reproducible toolchain rather than an emulated operating system: a pinned JDK
//! image kept alive with `sleep`, whose home directory is bound from the machine's directory
//! on this host. Every tool the recipes need — Node and pnpm for the web assets, Google's
//! command-line tools and the build tools for the SDK — is downloaded into that home with its
//! checksum pinned, the same way the macOS guest prepares itself, so a fresh container heals
//! itself where a tool is first used. Everything else the container is asked to do goes
//! through `docker exec` with a fixed script, mirroring what the macOS providers speak over SSH:
//! the same snapshot streamed in, the same detachable job wrapper, the same artifact transfer
//! with sizes and checksums agreed on both sides. The container runs as root, which is why its
//! home is emptied through a throwaway container when the machine is deleted.

use super::*;

#[path = "android_signing.rs"]
mod signing_verification;
pub use signing_verification::{
    AndroidSigningAlgorithm, AndroidSigningCertificate, verify_android_signing_material,
};
use signing_verification::{SIGNING_CHECK_SOURCE, read_signing_keystore, verify_signing_bytes};

/// Pinned by digest: `eclipse-temurin:17-jdk-noble` as resolved on 2026-09-06 (OpenJDK 17.0.20
/// on Ubuntu 24.04). JDK 17 is what every Android Gradle Plugin since 8.0 accepts, and what
/// Capacitor 5 through 8 projects run their Gradle on; JDK 21 would refuse the older Gradle
/// wrappers. The same upgrade rule as the macOS images: a deliberate commit that names what
/// changed upstream.
pub const ANDROID_IMAGE: &str =
    "eclipse-temurin@sha256:61a94244559f2e89e4edb02bae37eeb8762ecf5deaf237251fa630e5120a8798";
/// Google distributes the pinned Linux build tools for x86_64, and the pinned Node/JDK
/// downloads below are x64 too. Select this platform even on Apple Silicon rather than
/// mixing an arm64 base image with x64 executables. Docker Desktop supplies the emulation.
const ANDROID_PLATFORM: &str = "linux/amd64";
const ANDROID_PLATFORM_ARG: &str = "--platform=linux/amd64";
const ANDROID_EMULATION_HELP: &str = "buildbridge's Android tools require Linux amd64 containers. On Apple Silicon, enable x86/amd64 emulation in Docker Desktop; the Apple Virtualization framework with Rosetta can accelerate it. Also allow Docker Desktop to share the machine's home directory.";
/// The image's JDK, fixed by the digest above: the JVM Gradle itself runs on.
const JAVA_HOME: &str = "/opt/java/openjdk";
/// Temurin JDK 21 for x86_64 Linux, as published on 2026-09-06 (`jdk-21.0.12.1+1`), with the
/// checksum Adoptium states. Capacitor 8's Android library compiles for Java 21 on the JVM
/// Gradle runs on, and its plugins ask for a Java 21 toolchain, while Gradle 8.0 through 8.4,
/// which Capacitor 5 and 6 projects pin, refuse to run on anything past 17; so both JDKs are
/// present, the project's wrapper version decides which Gradle runs on, and both are registered
/// as toolchains it may compile with.
const JDK_21_VERSION: &str = "21.0.12.1_1";
const JDK_21_RELEASE: &str = "jdk-21.0.12.1%2B1";
const JDK_21_LINUX_X64_SHA256: &str =
    "ce79869e1307ed8ee1e2baa86a412b1eb5b75d10a01006d788a6f968bcfaee94";
/// The container's home: bound from the machine's `home` directory on this host, so the SDK,
/// the Gradle caches, the synchronized project and the build outputs survive the container.
pub(crate) const HOME_CONTAINER_DIR: &str = "/root";
/// Google's command-line tools, as published on 2026-09-06; the checksum is the one the
/// download page states and the one measured here.
const CMDLINE_TOOLS_VERSION: &str = "15859902";
const CMDLINE_TOOLS_LINUX_SHA256: &str =
    "4e4c464f145a7512b57d088ac6c278c03c9eea610886b35a5e0804e74eedf583";
/// The build tools every release needs on its own — `zipalign`, `apksigner`, `aapt2` — whatever
/// build tools the project's Gradle plugin installs for itself.
const BUILD_TOOLS_VERSION: &str = "35.0.0";
const NODE_LINUX_X64_SHA256: &str =
    "855d581f8a4eb1a8117e3426de25fe02770592febcfb31369aee1ffbfee9e8ec";
/// `sleep` is the container's only process and dies on the first signal; ten seconds is ample.
const STOP_TIMEOUT_SECONDS: u16 = 10;
/// A signed app bundle and APK together; an app past this is not a mobile app.
pub const ANDROID_RELEASE_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const ANDROID_BUILD_OUTPUT_TAIL_LINES: usize = 80;
const ANDROID_BUILD_DIAGNOSTIC_LINES: usize = 24;
const AAB_NAME: &str = "app-release.aab";
const APK_NAME: &str = "app-release.apk";
const DEBUG_APK_NAME: &str = "app-debug.apk";
pub(crate) const ANDROID_HTTP_DEBUG_INIT: &str = include_str!("android_http.gradle");

/// The phases of everything that runs the project's tools in the container: the sync and the
/// debug build share them, as the macOS project phases do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AndroidBuildPhase {
    Snapshotting,
    Transferring,
    Extracting,
    PreparingTools,
    InstallingDependencies,
    BuildingWebAssets,
    SyncingAndroid,
    Building,
    Inspecting,
    Completed,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidBuildProgress {
    pub phase: AndroidBuildPhase,
    #[ts(type = "number")]
    pub completed_bytes: u64,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub elapsed_seconds: u64,
    pub detail: String,
    pub log_line: Option<String>,
}

/// What the container built with: named so a build can be repeated somewhere else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidToolchainSummary {
    pub jdk_version: String,
    pub build_tools_version: String,
}

/// The debug build's outcome: the app as `aapt2` reads it back from the APK, the tools, and
/// the APK itself on this host, which is how the app reaches a phone or an emulator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidBuildResult {
    pub application_id: String,
    pub version_name: String,
    pub version_code: String,
    pub toolchain: AndroidToolchainSummary,
    /// The debug APK, retained on this host; `None` on records from before it was kept.
    #[serde(default)]
    pub apk: Option<AndroidArtifact>,
    // Whether buildbridge enabled HTTP APIs for this debug APK.
    #[serde(default)]
    pub allow_http: bool,
    pub output_tail: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AndroidReleasePhase {
    Preparing,
    /// Only when a build chooses an env set: the web assets are rebuilt with it first.
    BuildingWebAssets,
    Bundling,
    Signing,
    Verifying,
    Transferring,
    Completed,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidReleaseProgress {
    pub phase: AndroidReleasePhase,
    #[ts(type = "number")]
    pub completed_bytes: u64,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub elapsed_seconds: u64,
    pub detail: String,
    pub log_line: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidArtifact {
    pub path: String,
    #[ts(type = "number")]
    pub bytes: u64,
    pub sha256: String,
}

/// Which verified artifacts to retain. Existing callers keep both outputs by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AndroidReleaseOutputs {
    #[default]
    Both,
    Aab,
    Apk,
}

impl AndroidReleaseOutputs {
    pub fn includes_aab(self) -> bool {
        matches!(self, Self::Both | Self::Aab)
    }

    pub fn includes_apk(self) -> bool {
        matches!(self, Self::Both | Self::Apk)
    }
}

/// A signed release with the selected app bundle and/or APK, verified before transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidReleaseResult {
    pub application_id: String,
    pub version_name: String,
    pub version_code: String,
    pub key_alias: String,
    /// The signing certificate's SHA-256, as `apksigner` printed it: what Google Play shows as
    /// the upload key certificate.
    pub certificate_sha256: String,
    pub aab: Option<AndroidArtifact>,
    pub apk: Option<AndroidArtifact>,
    pub output_tail: Vec<String>,
}

impl AndroidReleaseResult {
    pub fn artifacts(&self) -> impl Iterator<Item = &AndroidArtifact> {
        self.aab.iter().chain(self.apk.iter())
    }
}

/// The upload key as a release needs it. The keystore is streamed into the container and the
/// passwords with it as owner-only files; none of them is ever an argument or an environment
/// variable, and all of them are removed once the release is signed.
#[derive(Clone)]
pub struct AndroidSigningMaterial {
    pub keystore_path: PathBuf,
    pub keystore_password: String,
    pub key_alias: String,
    pub key_password: String,
}

impl std::fmt::Debug for AndroidSigningMaterial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidSigningMaterial")
            .field("keystore_path", &self.keystore_path)
            .field("key_alias", &self.key_alias)
            .field("keystore_password", &"<redacted>")
            .field("key_password", &"<redacted>")
            .finish()
    }
}

/// What a freshly created keystore reports about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidKeystoreSummary {
    pub path: String,
    pub key_alias: String,
    pub certificate_sha256: String,
}

/// Where the toolchain lives under the container's home. Every recipe derives its paths from
/// here so the debug build and the signed release agree.
pub(crate) struct AndroidToolchain {
    pub(crate) home: String,
    pub(crate) tools: String,
    pub(crate) node_root: String,
    pub(crate) pnpm: String,
    pub(crate) sdk: String,
    pub(crate) build_tools: String,
    /// The second JDK, for projects whose Gradle asks to compile with Java 21.
    pub(crate) jdk_21: String,
    pub(crate) workspace: String,
    pub(crate) signing: String,
    /// The `PATH` the recipes export: pinned tools first, then the image's JDK and Ubuntu.
    pub(crate) path: String,
}

impl AndroidToolchain {
    /// The Gradle buildbridge supplies to a project that commits no wrapper.
    pub(crate) fn gradle(&self) -> String {
        format!("{}/gradle-{GRADLE_VERSION}/bin/gradle", self.tools)
    }

    /// The same tools as the recipes see them.
    pub(crate) fn recipe_tools(&self) -> crate::recipes::RecipeTools {
        crate::recipes::RecipeTools {
            tools: self.tools.clone(),
            node_root: self.node_root.clone(),
            pnpm: self.pnpm.clone(),
            pod: None,
            macos: false,
        }
    }
}

pub(crate) fn android_toolchain(home: &str) -> AndroidToolchain {
    let tools = format!("{home}/.buildbridge/tools");
    let node_root = format!("{tools}/node-v{NODE_VERSION}-linux-x64");
    let sdk = format!("{home}/android-sdk");
    let build_tools = format!("{sdk}/build-tools/{BUILD_TOOLS_VERSION}");
    let path = format!(
        "{node_root}/bin:{tools}/pnpm/node_modules/.bin:{sdk}/platform-tools:{sdk}/cmdline-tools/latest/bin:{build_tools}:{JAVA_HOME}/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    );

    AndroidToolchain {
        home: home.to_string(),
        pnpm: format!("{tools}/pnpm/node_modules/.bin/pnpm"),
        jdk_21: format!("{tools}/jdk-{JDK_21_VERSION}"),
        workspace: format!("{home}/BuildBridge/workspaces/active"),
        signing: format!("{home}/.buildbridge/signing"),
        tools,
        node_root,
        sdk,
        build_tools,
        path,
    }
}

/// Installs whatever of the toolchain is missing — Node, pnpm, Google's command-line tools,
/// the SDK licences, the platform tools and the pinned build tools — idempotently, with every
/// download checked against its pinned SHA-256. Platforms and further build tools the project
/// asks for are installed by its own Gradle plugin into the same SDK, which the accepted
/// licences allow. The image ships no `unzip`; the JDK's own `jar` opens the zip.
pub(crate) fn android_tools_preparation(
    toolchain: &AndroidToolchain,
    javascript: bool,
    gradle: bool,
) -> String {
    let AndroidToolchain {
        home,
        tools,
        node_root,
        pnpm,
        sdk,
        build_tools,
        jdk_21,
        workspace,
        ..
    } = toolchain;
    let node_name = format!("node-v{NODE_VERSION}-linux-x64");
    let node_archive = format!("{tools}/{node_name}.tar.gz");
    let gradle_home = format!("{tools}/gradle-{GRADLE_VERSION}");
    let gradle_archive = format!("{tools}/gradle-{GRADLE_VERSION}-bin.zip");
    // Only for a project that commits no wrapper: a wrapper is the project's own choice of
    // Gradle, and every project that has one uses it.
    let gradle = if gradle {
        format!(
            r#"if /bin/test ! -x "{gradle_home}/bin/gradle"; then
    /bin/rm -rf "{gradle_home}" "{gradle_archive}" "{tools}/gradle.incoming"
    /usr/bin/curl --fail --location --show-error --silent "https://services.gradle.org/distributions/gradle-{GRADLE_VERSION}-bin.zip" --output "{gradle_archive}"
    /usr/bin/printf '%s  %s\n' "{GRADLE_BIN_SHA256}" "{gradle_archive}" | /usr/bin/sha256sum --check --status
    /bin/mkdir -p "{tools}/gradle.incoming"
    (cd "{tools}/gradle.incoming" && "{JAVA_HOME}/bin/jar" xf "{gradle_archive}")
    /bin/mv "{tools}/gradle.incoming/gradle-{GRADLE_VERSION}" "{gradle_home}"
    /bin/chmod 755 "{gradle_home}/bin/gradle"
    /bin/rm -rf "{gradle_archive}" "{tools}/gradle.incoming"
fi
"#
        )
    } else {
        String::new()
    };
    // Node and pnpm are downloaded only for a project that installs JavaScript dependencies; a
    // native Gradle project or a Flutter app needs neither, and a download it never uses is one
    // more thing that can fail on a slow network.
    let node = if javascript {
        format!(
            r#"if /bin/test ! -x "{node_root}/bin/node"; then
    /bin/rm -rf "{node_root}" "{node_archive}"
    /usr/bin/curl --fail --location --show-error --silent "https://nodejs.org/dist/v{NODE_VERSION}/{node_name}.tar.gz" --output "{node_archive}"
    /usr/bin/printf '%s  %s\n' "{NODE_LINUX_X64_SHA256}" "{node_archive}" | /usr/bin/sha256sum --check --status
    /usr/bin/tar -xzf "{node_archive}" -C "{tools}"
    /bin/rm -f "{node_archive}"
fi
if /bin/test ! -x "{pnpm}"; then
    "{node_root}/bin/npm" install --prefix "{tools}/pnpm" "pnpm@{PNPM_VERSION}" --no-audit --no-fund
fi
"#
        )
    } else {
        String::new()
    };
    let cmdline_archive = format!("{tools}/commandlinetools-linux-{CMDLINE_TOOLS_VERSION}.zip");
    let jdk_archive = format!("{tools}/OpenJDK21U-jdk_x64_linux_hotspot_{JDK_21_VERSION}.tar.gz");
    format!(
        r#"if /bin/test "$(/usr/bin/uname -m)" != x86_64; then
    /usr/bin/printf '%s\n' 'This Android container must use linux/amd64. Stop it and discard its container, then start it again to use the correct platform.' >&2
    exit 1
fi
/bin/mkdir -p "{tools}" "{sdk}" "{home}/.gradle"
{gradle}
if /bin/test ! -x "{jdk_21}/bin/javac"; then
    /bin/rm -rf "{jdk_21}" "{jdk_archive}" "{tools}/jdk-21.incoming"
    /usr/bin/curl --fail --location --show-error --silent "https://github.com/adoptium/temurin21-binaries/releases/download/{JDK_21_RELEASE}/OpenJDK21U-jdk_x64_linux_hotspot_{JDK_21_VERSION}.tar.gz" --output "{jdk_archive}"
    /usr/bin/printf '%s  %s\n' "{JDK_21_LINUX_X64_SHA256}" "{jdk_archive}" | /usr/bin/sha256sum --check --status
    /bin/mkdir -p "{tools}/jdk-21.incoming"
    /usr/bin/tar -xzf "{jdk_archive}" -C "{tools}/jdk-21.incoming" --strip-components=1
    /bin/mv "{tools}/jdk-21.incoming" "{jdk_21}"
    /bin/rm -f "{jdk_archive}"
fi
# Gradle runs on the image's JDK 17 and may compile with either; the project decides which.
/usr/bin/printf 'org.gradle.java.installations.auto-download=false\norg.gradle.java.installations.paths=%s,%s\n' "{JAVA_HOME}" "{jdk_21}" > "{home}/.gradle/gradle.properties"
{node}if /bin/test ! -x "{sdk}/cmdline-tools/latest/bin/sdkmanager"; then
    /bin/rm -rf "{sdk}/cmdline-tools" "{cmdline_archive}" "{tools}/cmdline-tools"
    /usr/bin/curl --fail --location --show-error --silent "https://dl.google.com/android/repository/commandlinetools-linux-{CMDLINE_TOOLS_VERSION}_latest.zip" --output "{cmdline_archive}"
    /usr/bin/printf '%s  %s\n' "{CMDLINE_TOOLS_LINUX_SHA256}" "{cmdline_archive}" | /usr/bin/sha256sum --check --status
    /bin/mkdir -p "{tools}/cmdline-tools" "{sdk}/cmdline-tools"
    (cd "{tools}/cmdline-tools" && "{JAVA_HOME}/bin/jar" xf "{cmdline_archive}")
    /bin/mv "{tools}/cmdline-tools/cmdline-tools" "{sdk}/cmdline-tools/latest"
    /bin/chmod 755 "{sdk}/cmdline-tools/latest/bin"/*
    /bin/rm -rf "{cmdline_archive}" "{tools}/cmdline-tools"
fi
if /bin/test ! -f "{sdk}/licenses/android-sdk-license"; then
    /usr/bin/yes | "{sdk}/cmdline-tools/latest/bin/sdkmanager" --sdk_root="{sdk}" --licenses > /dev/null
fi
if /bin/test ! -f "{build_tools}/apksigner" || /bin/test ! -f "{sdk}/platform-tools/adb"; then
    "{sdk}/cmdline-tools/latest/bin/sdkmanager" --sdk_root="{sdk}" "platform-tools" "build-tools;{BUILD_TOOLS_VERSION}"
fi
/bin/test -f "{build_tools}/apksigner"
/bin/test -f "{build_tools}/zipalign"
/bin/test -f "{build_tools}/aapt2"
# Gradle's caches and the env file live under the workspace's .buildbridge; a tool that
# honours ignore files must not crawl them.
/bin/mkdir -p "{workspace}/.buildbridge"
/usr/bin/printf '*\n' > "{workspace}/.buildbridge/.gitignore""#
    )
}

/// The environment every recipe exports before it touches the project. The Gradle daemon is
/// off: a build is one JVM that exits with it, so a stopped container holds no memory and a
/// cancelled build leaves nothing behind. The JDK starts as the image's and is settled just
/// before Gradle runs, by [`android_jdk_selection`].
pub(crate) fn android_recipe_environment(toolchain: &AndroidToolchain) -> String {
    let AndroidToolchain {
        home, sdk, path, ..
    } = toolchain;
    format!(
        r#"export HOME="{home}"
export PATH="{path}"
export JAVA_HOME="{JAVA_HOME}"
export PATH="$JAVA_HOME/bin:$PATH"
export ANDROID_HOME="{sdk}"
export ANDROID_SDK_ROOT="{sdk}"
export ANDROID_USER_HOME="{home}/.android"
export GRADLE_USER_HOME="{home}/.gradle"
export GRADLE_OPTS="-Dorg.gradle.daemon=false"
export LANG="C.UTF-8"
export CI=1
export CYPRESS_INSTALL_BINARY=0"#
    )
}

/// Which JDK Gradle runs on: the project's committed wrapper's decision, since Gradle 8.5 and
/// newer run on Java 21, which Capacitor 8's own library compiles with, while the Gradle that
/// Capacitor 5 and 6 pin refuses anything past 17. The wrapper is read where the layout says
/// the Gradle root is, and this runs immediately before Gradle rather than at the top of the
/// recipe: an Expo project has no wrapper at all until `expo prebuild` writes one, so reading
/// it any earlier would always find nothing and settle for the image's JDK.
pub(crate) fn android_jdk_selection(
    toolchain: &AndroidToolchain,
    layout: &ProjectLayout,
) -> String {
    let AndroidToolchain { jdk_21, .. } = toolchain;
    let gradle_root = android_gradle_dir(layout, &toolchain.workspace);
    format!(
        r#"gradle_wrapper="{gradle_root}/gradle/wrapper/gradle-wrapper.properties"
if /bin/test -f "$gradle_wrapper"; then
    gradle_major=$(/usr/bin/sed -n 's/^distributionUrl=.*gradle-\([0-9]*\)\.\([0-9]*\).*/\1/p' "$gradle_wrapper" | /usr/bin/head -n 1)
    gradle_minor=$(/usr/bin/sed -n 's/^distributionUrl=.*gradle-\([0-9]*\)\.\([0-9]*\).*/\2/p' "$gradle_wrapper" | /usr/bin/head -n 1)
    if /bin/test "${{gradle_major:-0}}" -gt 8 || {{ /bin/test "${{gradle_major:-0}}" -eq 8 && /bin/test "${{gradle_minor:-0}}" -ge 5; }}; then
        export JAVA_HOME="{jdk_21}"
        export PATH="$JAVA_HOME/bin:$PATH"
    fi
fi
"#
    )
}

/// The container's fixed argv. Nothing is published and no device is passed: the container
/// holds a toolchain, not a machine. Memory and cores are limits on the build rather than a
/// virtual machine's hardware. `--init` gives `sleep` a parent that forwards the stop signal.
pub(crate) fn create_args(
    container_name: &str,
    config: &MachineConfig,
    home_dir: &Path,
) -> Vec<String> {
    vec![
        "create".to_string(),
        ANDROID_PLATFORM_ARG.to_string(),
        format!("--name={container_name}"),
        "--label=dev.buildbridge.managed=true".to_string(),
        "--label=dev.buildbridge.provider=android_toolchain".to_string(),
        "--restart=no".to_string(),
        format!("--stop-timeout={STOP_TIMEOUT_SECONDS}"),
        "--init".to_string(),
        format!("--memory={}g", config.memory_gib),
        format!("--cpus={}", config.cpu_cores),
        format!("--volume={}:{HOME_CONTAINER_DIR}:rw", home_dir.display()),
        format!("--workdir={HOME_CONTAINER_DIR}"),
        format!("--env=HOME={HOME_CONTAINER_DIR}"),
        "--entrypoint=/bin/sleep".to_string(),
        ANDROID_IMAGE.to_string(),
        "infinity".to_string(),
    ]
}

fn image_inspect_args(image: &str) -> Vec<String> {
    vec![
        "image".to_string(),
        "inspect".to_string(),
        "--format={{.Os}}/{{.Architecture}}".to_string(),
        image.to_string(),
    ]
}

fn image_pull_args() -> Vec<String> {
    vec![
        "pull".to_string(),
        ANDROID_PLATFORM_ARG.to_string(),
        ANDROID_IMAGE.to_string(),
    ]
}

fn android_execution_message(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if [
        "exec format error",
        "cannot execute binary",
        "rosetta",
        "no matching manifest",
        "platform does not match",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
    {
        format!("{message}\n{ANDROID_EMULATION_HELP}")
    } else {
        message.to_string()
    }
}

fn run_android_docker(operation: &'static str, args: &[String]) -> Result<Output, ProviderError> {
    run_docker(operation, args).map_err(|error| match error {
        ProviderError::DockerCommand { operation, message } => ProviderError::DockerCommand {
            operation,
            message: android_execution_message(&message),
        },
        other => other,
    })
}

fn ensure_android_image() -> Result<(), ProviderError> {
    let inspected = docker_command()
        .args(image_inspect_args(ANDROID_IMAGE))
        .tracked_output()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?;
    if !inspected.status.success() || clean_output(&inspected.stdout) != ANDROID_PLATFORM {
        run_android_docker("Android image pull", &image_pull_args())?;
    }
    Ok(())
}

fn validate_container_platform(platform: &str) -> Result<(), ProviderError> {
    if platform.trim() == ANDROID_PLATFORM {
        return Ok(());
    }
    Err(ProviderError::AndroidToolchain(format!(
        "This container uses {}; the Android tools need {ANDROID_PLATFORM}. Stop the machine, discard its container, and start it again to recreate it with the correct platform. buildbridge will keep its project records and signing credentials.",
        platform.trim(),
    )))
}

/// Inspect the container's actual image ID, not its possibly multi-platform repository digest.
/// This also catches an arm64 container made by an earlier buildbridge before any build runs.
fn verify_container_platform(container_name: &str) -> Result<(), ProviderError> {
    let image = run_android_docker(
        "Android container inspection",
        &[
            "inspect".to_string(),
            "--format={{.Image}}".to_string(),
            container_name.to_string(),
        ],
    )?;
    let image_id = clean_output(&image.stdout);
    if !image_id.strip_prefix("sha256:").is_some_and(valid_sha256) {
        return Err(ProviderError::AndroidToolchain(
            "Docker did not report the Android container's image identity.".to_string(),
        ));
    }
    let inspected = run_android_docker("Android image inspection", &image_inspect_args(&image_id))?;
    validate_container_platform(&clean_output(&inspected.stdout))
}

pub(crate) fn ensure_home_dir(home_dir: &Path) -> Result<(), ProviderError> {
    validate_bind_path(home_dir, "home directory")?;
    fs::create_dir_all(home_dir).map_err(|error| ProviderError::Identity(error.to_string()))?;
    disk::restrict_directory(home_dir)
}

/// Creates the container when it is missing, then starts it. The first start pulls the JDK
/// image; the toolchain itself is prepared by the first build, where its progress can be shown.
pub(crate) fn launch<F>(
    container_name: &str,
    config: &MachineConfig,
    home_dir: &Path,
    mut on_progress: F,
) -> Result<RuntimeStatus, ProviderError>
where
    F: FnMut(LaunchProgress),
{
    let started = Instant::now();
    let mut report = |phase: LaunchPhase, detail: &str| {
        on_progress(LaunchProgress {
            phase,
            elapsed_seconds: started.elapsed().as_secs(),
            detail: detail.to_string(),
        });
    };
    report(LaunchPhase::Preparing, "Checking the host and Docker");
    let prerequisites = probe_host_for(MachineProvider::AndroidToolchain);
    if !prerequisites.ready {
        return Err(ProviderError::AndroidToolchain(
            prerequisites.issues.join(" "),
        ));
    }

    let (state, _, _) = inspect_container(container_name)?;
    if state == ContainerState::Missing {
        report(
            LaunchPhase::PullingImage,
            "Pulling the Linux amd64 JDK image; the first pull downloads a few hundred megabytes",
        );
        ensure_android_image()?;
        report(
            LaunchPhase::PreparingDisk,
            "Preparing the toolchain's home directory on this host",
        );
        ensure_home_dir(home_dir)?;
        report(
            LaunchPhase::CreatingContainer,
            "Creating the toolchain container",
        );
        run_android_docker(
            "container creation",
            &create_args(container_name, config, home_dir),
        )?;
    } else {
        ensure_manual_restart_policy(container_name)?;
    }

    verify_container_platform(container_name)?;

    let (state, _, _) = inspect_container(container_name)?;
    if state != ContainerState::Running {
        report(
            LaunchPhase::Starting,
            "Starting the Linux amd64 toolchain container",
        );
        run_android_docker("start", &["start".to_string(), container_name.to_string()])?;
    }

    let runtime = status_for(container_name, MachineProvider::AndroidToolchain)?;
    if runtime.state != ContainerState::Running {
        return Err(ProviderError::AndroidToolchain(format!(
            "The toolchain container stopped before it became ready. {ANDROID_EMULATION_HELP}"
        )));
    }
    report(LaunchPhase::Completed, "The Android toolchain is running");
    Ok(runtime)
}

fn cleanup_args(home_dir: &Path) -> Vec<String> {
    vec![
        "run".to_string(),
        "--rm".to_string(),
        ANDROID_PLATFORM_ARG.to_string(),
        format!("--volume={}:/buildbridge-storage:rw", home_dir.display()),
        "--entrypoint=/usr/bin/find".to_string(),
        ANDROID_IMAGE.to_string(),
        "/buildbridge-storage".to_string(),
        "-mindepth".to_string(),
        "1".to_string(),
        "-delete".to_string(),
    ]
}

/// Removes the machine's home directory, emptying it through the image first when its
/// root-owned files refuse this user.
pub fn remove_android_home(home_dir: &Path) -> Result<(), ProviderError> {
    match fs::remove_dir_all(home_dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            validate_bind_path(home_dir, "home directory")?;
            ensure_android_image()?;
            run_android_docker("Android storage cleanup", &cleanup_args(home_dir))?;
            fs::remove_dir_all(home_dir).map_err(|error| ProviderError::Identity(error.to_string()))
        }
        Err(error) => Err(ProviderError::Identity(error.to_string())),
    }
}

/// `docker exec` with a fixed script; values that are the user's travel as positional
/// arguments after it, never inside it.
fn container_exec_command(container_name: &str, script: &str, arguments: &[&str]) -> Command {
    let mut command = docker_command();
    command.args([
        "exec",
        "--interactive",
        container_name,
        "/bin/sh",
        "-c",
        script,
        "sh",
    ]);
    command.args(arguments);
    command
}

pub(crate) fn run_container_command(
    container_name: &str,
    script: &str,
) -> Result<String, ProviderError> {
    let output = container_exec_command(container_name, script, &[])
        .stdin(Stdio::null())
        .tracked_output()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?;

    if output.status.success() {
        Ok(clean_output(&output.stdout))
    } else {
        let message = clean_output(&output.stderr);
        Err(ProviderError::AndroidToolchain(android_execution_message(
            &if message.is_empty() {
                clean_output(&output.stdout)
            } else {
                message
            },
        )))
    }
}

/// Writes the requested version into the synced project where it declares one: the app
/// module's Gradle script, whichever dialect it uses, or a Flutter app's pubspec. The file
/// comes back whole rather than through the bounded command output, is rewritten here with
/// the same function the host edit uses, and goes back the way env files do.
fn set_container_project_version(
    container_name: &str,
    workspace: &str,
    layout: &ProjectLayout,
    version: &ProjectVersion,
) -> Result<(), ProviderError> {
    let (relative, label): (String, &str) = if layout.kind == ProjectKind::Flutter {
        ("pubspec.yaml".to_string(), "pubspec")
    } else {
        (android_script_relative(layout), "app module Gradle script")
    };
    let path = join_relative(workspace, &relative);
    let script = format!(
        "set -eu; /bin/test -f {0}; /bin/cat {0}",
        shell_single_quote(&path)
    );
    let output = container_exec_command(container_name, &script, &[])
        .tracked_output()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?;
    if !output.status.success() {
        return Err(ProviderError::AndroidToolchain(format!(
            "the synced project has no {relative} to write the version into; synchronize the project again"
        )));
    }
    let contents = String::from_utf8(output.stdout).map_err(|_| {
        ProviderError::AndroidToolchain(format!("the synced {label} is not UTF-8 text"))
    })?;
    let rewritten = if layout.kind == ProjectKind::Flutter {
        set_pubspec_project_version(&contents, version)
    } else {
        set_gradle_project_version(&contents, version)
    }
    .map_err(ProviderError::AndroidToolchain)?;
    stream_bytes_to_container(container_name, rewritten.as_bytes(), &path, label)
}

/// Writes bytes into the container as an owner-only file, through the exec's stdin.
pub(crate) fn stream_bytes_to_container(
    container_name: &str,
    contents: &[u8],
    container_path: &str,
    label: &str,
) -> Result<(), ProviderError> {
    let script = format!(
        "set -eu; umask 077; /bin/mkdir -p \"$(/usr/bin/dirname {0})\"; /bin/cat > {0}",
        shell_single_quote(container_path)
    );
    let mut child = container_exec_command(container_name, &script, &[])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::AndroidToolchain(format!("could not start {label} transfer: {error}"))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::AndroidToolchain(format!("could not open {label} transfer"))
    })?;
    stdin.write_all(contents).map_err(|error| {
        ProviderError::AndroidToolchain(format!("{label} transfer was interrupted: {error}"))
    })?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::AndroidToolchain(format!("could not finish {label} transfer: {error}"))
    })?;
    if !output.status.success() {
        return Err(ProviderError::AndroidToolchain(format!(
            "the container rejected the {label}: {}",
            clean_output(&output.stderr)
        )));
    }

    Ok(())
}

/// Streams a host file into the container, reporting bytes as they go.
fn stream_file_to_container<F>(
    container_name: &str,
    path: &Path,
    total_bytes: u64,
    container_path: &str,
    mut on_bytes: F,
) -> Result<(), ProviderError>
where
    F: FnMut(u64),
{
    let mut file = File::open(path).map_err(|error| {
        ProviderError::AndroidToolchain(format!("could not open the source snapshot: {error}"))
    })?;
    let script = format!(
        "set -eu; umask 077; /bin/mkdir -p \"$(/usr/bin/dirname {0})\"; /bin/cat > {0}",
        shell_single_quote(container_path)
    );
    let mut child = container_exec_command(container_name, &script, &[])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::AndroidToolchain(format!(
                "could not start source synchronization: {error}"
            ))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::AndroidToolchain("could not open source synchronization".to_string())
    })?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut completed_bytes = 0;
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            ProviderError::AndroidToolchain(format!("could not read the source snapshot: {error}"))
        })?;
        if read == 0 {
            break;
        }
        stdin.write_all(&buffer[..read]).map_err(|error| {
            ProviderError::AndroidToolchain(format!(
                "source synchronization was interrupted: {error}"
            ))
        })?;
        completed_bytes += read as u64;
        if last_progress.elapsed() >= Duration::from_millis(250) || completed_bytes == total_bytes {
            on_bytes(completed_bytes);
            last_progress = Instant::now();
        }
    }
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::AndroidToolchain(format!("could not finish source synchronization: {error}"))
    })?;
    if !output.status.success() {
        return Err(ProviderError::AndroidToolchain(format!(
            "the container rejected the source snapshot: {}",
            clean_output(&output.stderr)
        )));
    }

    Ok(())
}

/// Streams a file out of the container into a new owner-only host file, refusing more bytes
/// than the container declared for it.
fn stream_container_file<F>(
    container_name: &str,
    container_path: &str,
    local_path: &Path,
    expected_bytes: u64,
    mut on_bytes: F,
) -> Result<(), ProviderError>
where
    F: FnMut(u64),
{
    let script = format!("set -eu; /bin/cat {}", shell_single_quote(container_path));
    let mut child = container_exec_command(container_name, &script, &[])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::AndroidToolchain(format!("could not start artifact transfer: {error}"))
        })?;
    let mut stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::AndroidToolchain("could not read the artifact transfer".to_string())
    })?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(local_path)
        .map_err(|error| {
            ProviderError::AndroidToolchain(format!("could not create the local artifact: {error}"))
        })?;
    set_artifact_permissions(local_path)?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut copied = 0_u64;
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    loop {
        let count = stdout.read(&mut buffer).map_err(|error| {
            ProviderError::AndroidToolchain(format!("could not read the artifact: {error}"))
        })?;
        if count == 0 {
            break;
        }
        copied = copied.checked_add(count as u64).ok_or_else(|| {
            ProviderError::AndroidToolchain("the artifact size overflowed".to_string())
        })?;
        if copied > expected_bytes {
            return Err(ProviderError::AndroidToolchain(
                "the container sent more artifact data than declared".to_string(),
            ));
        }
        file.write_all(&buffer[..count]).map_err(|error| {
            ProviderError::AndroidToolchain(format!("could not retain the artifact: {error}"))
        })?;
        if last_progress.elapsed() >= Duration::from_millis(200) || copied == expected_bytes {
            on_bytes(copied);
            last_progress = Instant::now();
        }
    }
    file.sync_all().map_err(|error| {
        ProviderError::AndroidToolchain(format!("could not flush the artifact: {error}"))
    })?;
    drop(file);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::AndroidToolchain(format!("could not finish artifact transfer: {error}"))
    })?;
    if !output.status.success() {
        return Err(ProviderError::AndroidToolchain(format!(
            "the artifact transfer failed: {}",
            clean_output(&output.stderr)
        )));
    }
    if copied != expected_bytes {
        return Err(ProviderError::AndroidToolchain(format!(
            "the artifact transfer was incomplete ({copied} of {expected_bytes} bytes)"
        )));
    }

    Ok(())
}

/// What runs Gradle: the project's own committed wrapper, or the pinned distribution the
/// toolchain installs for a project that has none.
fn android_gradle_program(toolchain: &AndroidToolchain, layout: &ProjectLayout) -> String {
    if android_has_wrapper(layout) {
        "./gradlew".to_string()
    } else {
        shell_single_quote(&toolchain.gradle())
    }
}

/// The project as the layout describes it, on the host, before a snapshot is taken: the
/// package for a JavaScript project, and the Gradle wrapper and the application module's
/// script for every kind but Expo, whose Android project is written where the build runs.
pub(crate) fn validate_android_project(
    workspace_path: &Path,
    layout: &ProjectLayout,
) -> Result<PathBuf, ProviderError> {
    validate_layout(layout).map_err(|error| ProviderError::AndroidToolchain(error.to_string()))?;
    let workspace_path = fs::canonicalize(workspace_path).map_err(|error| {
        ProviderError::AndroidToolchain(format!("the approved project is unavailable: {error}"))
    })?;
    let android = layout.android.as_ref().ok_or_else(|| {
        ProviderError::AndroidToolchain("the approved project has no Android project".to_string())
    })?;
    if !workspace_path.is_dir() {
        return Err(ProviderError::AndroidToolchain(
            "the approved project is not a directory".to_string(),
        ));
    }
    if layout.kind.uses_javascript() && !workspace_path.join("package.json").is_file() {
        return Err(ProviderError::AndroidToolchain(
            "the approved project must contain package.json".to_string(),
        ));
    }
    if !layout.kind.generates_native_projects() {
        let wrapper = join_relative(&android.root, "gradlew");
        if android.wrapper && !workspace_path.join(&wrapper).is_file() {
            return Err(ProviderError::AndroidToolchain(format!(
                "the approved project must contain the committed Gradle wrapper {wrapper}"
            )));
        }
        if !workspace_path.join(&android.script).is_file() {
            return Err(ProviderError::AndroidToolchain(format!(
                "the approved project must contain the application module's script {}",
                android.script
            )));
        }
    }

    Ok(workspace_path)
}

pub(crate) fn android_progress(
    phase: AndroidBuildPhase,
    completed_bytes: u64,
    total_bytes: u64,
    started_at: Instant,
    detail: &str,
    log_line: Option<String>,
) -> AndroidBuildProgress {
    AndroidBuildProgress {
        phase,
        completed_bytes,
        total_bytes,
        elapsed_seconds: started_at.elapsed().as_secs(),
        detail: detail.to_string(),
        log_line,
    }
}

pub(crate) fn release_progress(
    phase: AndroidReleasePhase,
    completed_bytes: u64,
    total_bytes: u64,
    started_at: Instant,
    detail: &str,
    log_line: Option<String>,
) -> AndroidReleaseProgress {
    AndroidReleaseProgress {
        phase,
        completed_bytes,
        total_bytes,
        elapsed_seconds: started_at.elapsed().as_secs(),
        detail: detail.to_string(),
        log_line,
    }
}

pub(crate) fn android_build_phase(value: &str) -> Option<AndroidBuildPhase> {
    match value {
        "preparing_tools" => Some(AndroidBuildPhase::PreparingTools),
        "installing_dependencies" => Some(AndroidBuildPhase::InstallingDependencies),
        "building_web_assets" => Some(AndroidBuildPhase::BuildingWebAssets),
        "syncing_android" => Some(AndroidBuildPhase::SyncingAndroid),
        "building" => Some(AndroidBuildPhase::Building),
        "inspecting" => Some(AndroidBuildPhase::Inspecting),
        "completed" => Some(AndroidBuildPhase::Completed),
        _ => None,
    }
}

pub(crate) fn android_build_phase_detail(phase: AndroidBuildPhase) -> &'static str {
    match phase {
        AndroidBuildPhase::Snapshotting => "Creating the source snapshot",
        AndroidBuildPhase::Transferring => "Synchronizing source",
        AndroidBuildPhase::Extracting => "Preparing the container workspace",
        AndroidBuildPhase::PreparingTools => "Preparing the build tools and the Android SDK",
        AndroidBuildPhase::InstallingDependencies => "Installing the project's dependencies",
        AndroidBuildPhase::BuildingWebAssets => "Building web assets",
        AndroidBuildPhase::SyncingAndroid => "Preparing the Android project",
        AndroidBuildPhase::Building => "Compiling the debug APK with Gradle",
        AndroidBuildPhase::Inspecting => "Reading the built app back",
        AndroidBuildPhase::Completed => "Debug build complete",
    }
}

pub(crate) fn android_release_phase(value: &str) -> Option<AndroidReleasePhase> {
    match value {
        "preparing" => Some(AndroidReleasePhase::Preparing),
        "building_web_assets" => Some(AndroidReleasePhase::BuildingWebAssets),
        "bundling" => Some(AndroidReleasePhase::Bundling),
        "signing" => Some(AndroidReleasePhase::Signing),
        "verifying" => Some(AndroidReleasePhase::Verifying),
        "completed" => Some(AndroidReleasePhase::Completed),
        _ => None,
    }
}

pub(crate) fn android_release_phase_detail(phase: AndroidReleasePhase) -> &'static str {
    match phase {
        AndroidReleasePhase::Preparing => "Checking the toolchain and the upload key",
        AndroidReleasePhase::BuildingWebAssets => {
            "Applying the chosen environment and rebuilding the web assets"
        }
        AndroidReleasePhase::Bundling => "Building the release bundle and APK with Gradle",
        AndroidReleasePhase::Signing => "Signing with the upload key",
        AndroidReleasePhase::Verifying => "Verifying the signatures",
        AndroidReleasePhase::Transferring => "Transferring artifacts",
        AndroidReleasePhase::Completed => "Signed release complete",
    }
}

/// Gradle and the tools name their failures in a handful of shapes.
pub(crate) fn android_build_log_is_diagnostic(line: &str) -> bool {
    let normalized = line.to_ascii_lowercase();

    normalized.starts_with("error:")
        || normalized.starts_with("e: ")
        || normalized.contains(": error:")
        || normalized.contains("execution failed for task")
        || normalized.starts_with("* what went wrong")
        || normalized.starts_with("> ")
            && (normalized.contains("failed") || normalized.contains("error"))
        || normalized.starts_with("failure:")
        || normalized.contains("build failed")
        || normalized.contains("err_pnpm")
        || normalized.contains("exception:")
}

/// Synchronizes the approved project into the container: the same bounded, secret-filtered
/// snapshot the macOS guest gets, streamed through the exec and extracted atomically.
pub fn sync_android_workspace<F>(
    container_name: &str,
    workspace_path: &Path,
    layout: &ProjectLayout,
    env: Option<&GuestEnvFiles>,
    mut on_progress: F,
) -> Result<WorkspaceSyncResult, ProviderError>
where
    F: FnMut(AndroidBuildProgress),
{
    let workspace_path = validate_android_project(workspace_path, layout)?;
    // A detached worker can outlive the desktop's operation lock. Do not replace the
    // files it is building, or replay its completed result against this new snapshot.
    prepare_android_jobs(
        container_name,
        &android_toolchain(HOME_CONTAINER_DIR),
        false,
    )?;
    let started_at = Instant::now();
    on_progress(android_progress(
        AndroidBuildPhase::Snapshotting,
        0,
        0,
        started_at,
        "Inspecting the approved project and excluding local dependencies and secrets.",
        None,
    ));
    let mut exclusions = SnapshotExclusions::for_layout(layout);
    let (source_file_count, source_bytes) =
        inspect_snapshot_tree(&workspace_path, &mut exclusions)?;
    let temporary_archive = TemporaryArchive::new();
    create_workspace_archive(&workspace_path, &temporary_archive.0, &exclusions)?;
    let archive_bytes = fs::metadata(&temporary_archive.0)
        .map_err(|error| {
            ProviderError::AndroidToolchain(format!(
                "could not inspect the source snapshot: {error}"
            ))
        })?
        .len();
    let snapshot_sha256 = snapshot_sha256(&temporary_archive.0)?;

    let root = format!("{HOME_CONTAINER_DIR}/BuildBridge/workspaces");
    let workspace = format!("{root}/active");
    let staging = format!("{root}/active.incoming");
    let archive = format!("{root}/active.tar.gz");
    let env_file = format!("{root}/active.env");
    let env_shell = format!("{root}/active.env.sh");
    let prepare = format!(
        "set -eu; /bin/mkdir -p '{root}'; /bin/rm -rf '{staging}'; /bin/rm -f '{archive}' '{env_file}' '{env_shell}'"
    );
    run_container_command(container_name, &prepare)?;
    stream_file_to_container(
        container_name,
        &temporary_archive.0,
        archive_bytes,
        &archive,
        |completed_bytes| {
            on_progress(android_progress(
                AndroidBuildPhase::Transferring,
                completed_bytes,
                archive_bytes,
                started_at,
                "Copying the secret-filtered snapshot into the container.",
                None,
            ));
        },
    )?;
    if let Some(env) = env {
        stream_bytes_to_container(container_name, env.dotenv.as_bytes(), &env_file, "env file")?;
        stream_bytes_to_container(
            container_name,
            env.shell.as_bytes(),
            &env_shell,
            "env script",
        )?;
    }

    on_progress(android_progress(
        AndroidBuildPhase::Extracting,
        archive_bytes,
        archive_bytes,
        started_at,
        "Extracting the bounded snapshot into the container workspace.",
        None,
    ));
    let checks = crate::workspace::extraction_checks(layout, &staging);
    let extract = format!(
        "set -eu; /bin/mkdir -p '{staging}'; /usr/bin/tar -xzf '{archive}' -C '{staging}'; {checks}/bin/rm -rf '{workspace}.previous'; if /bin/test -d '{workspace}'; then /bin/mv '{workspace}' '{workspace}.previous'; fi; /bin/mv '{staging}' '{workspace}'; if /bin/test -f '{env_file}'; then /bin/mv '{env_file}' '{workspace}/.env.production.local'; /bin/chmod 600 '{workspace}/.env.production.local'; fi; if /bin/test -f '{env_shell}'; then /bin/mkdir -p '{workspace}/.buildbridge'; /bin/mv '{env_shell}' '{workspace}/.buildbridge/env.sh'; /bin/chmod 600 '{workspace}/.buildbridge/env.sh'; fi; /bin/rm -f '{archive}'; /bin/rm -rf '{workspace}.previous'"
    );
    run_container_command(container_name, &extract)?;

    Ok(WorkspaceSyncResult {
        guest_path: workspace,
        snapshot_sha256,
        source_file_count,
        source_bytes,
        archive_bytes,
    })
}

/// A tab-separated marker line's fields after its name.
fn marker_fields<'a>(line: &'a str, marker: &str) -> Option<Vec<&'a str>> {
    let rest = line.strip_prefix(marker)?;
    let rest = rest.strip_prefix('\t')?;
    Some(rest.split('\t').collect())
}

pub(crate) fn valid_application_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_'))
}

pub(crate) fn valid_version_value(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && !value.chars().any(char::is_control)
}

/// Runs the recipe body as a detachable job and streams its output back, routing the marker
/// lines to `on_marker` and everything else to the tail and the log.
fn run_container_job<M>(
    container_name: &str,
    job_name: &str,
    toolchain: &AndroidToolchain,
    body: &str,
    mut on_line: M,
) -> Result<(Vec<String>, Vec<String>, bool), ProviderError>
where
    M: FnMut(&str, &mut Vec<String>),
{
    let script = crate::device_run::guest_job_script(job_name, &toolchain.tools, "''", body);
    let mut child = container_exec_command(container_name, &script, &[])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .tracked_spawn()
        .map_err(|error| {
            ProviderError::AndroidToolchain(format!("could not start the Android build: {error}"))
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ProviderError::AndroidToolchain("could not capture the Android build output".to_string())
    })?;
    let mut output_tail = Vec::new();
    let mut diagnostic_lines = Vec::new();
    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| {
            ProviderError::AndroidToolchain(format!(
                "could not read the Android build output: {error}"
            ))
        })?;
        if line.starts_with("__BUILDBRIDGE_") {
            on_line(&line, &mut output_tail);
            continue;
        }
        let line = sanitize_build_log_line(&line);
        if line.is_empty() {
            continue;
        }
        if android_build_log_is_diagnostic(&line) {
            diagnostic_lines.push(line.clone());
            if diagnostic_lines.len() > ANDROID_BUILD_DIAGNOSTIC_LINES {
                diagnostic_lines.remove(0);
            }
        }
        output_tail.push(line.clone());
        if output_tail.len() > ANDROID_BUILD_OUTPUT_TAIL_LINES {
            output_tail.remove(0);
        }
        on_line(&line, &mut output_tail);
    }
    let status = child.wait().map_err(|error| {
        ProviderError::AndroidToolchain(format!("could not finish the Android build: {error}"))
    })?;
    // A killed exec client leaves the job running for a reattach; a finished one is cleaned up.
    if status.code().is_some() {
        let cleanup = format!("/bin/rm -rf '{}/jobs/{job_name}'", toolchain.tools);
        let _ = run_container_command(container_name, &cleanup);
    }

    Ok((output_tail, diagnostic_lines, status.success()))
}

/// The debug build: prepares the toolchain, installs the project's dependencies, runs the
/// framework's own preparation when it has one, compiles the application module's debug APK
/// with Gradle, reads the app back from it, and brings the APK to `output_directory` with its
/// size and checksum agreed.
pub fn run_android_debug_build<F>(
    container_name: &str,
    layout: &ProjectLayout,
    output_directory: &Path,
    allow_http: bool,
    version: Option<&ProjectVersion>,
    mut on_progress: F,
) -> Result<AndroidBuildResult, ProviderError>
where
    F: FnMut(AndroidBuildProgress),
{
    validate_layout(layout).map_err(|error| ProviderError::AndroidToolchain(error.to_string()))?;
    if let Some(version) = version {
        validate_android_version(version).map_err(ProviderError::AndroidToolchain)?;
    }
    validate_archive_output_directory(output_directory)?;
    // The retained APK is named once the build has said which version it is; the partial it
    // arrives as is fixed, and the destination must start empty.
    let apk_part = output_directory.join(format!("{DEBUG_APK_NAME}.part"));
    if directory_has_entries(output_directory).map_err(|error| {
        ProviderError::AndroidToolchain(format!("the artifact directory is unavailable: {error}"))
    })? {
        return Err(ProviderError::AndroidToolchain(
            "the artifact directory is not empty".to_string(),
        ));
    }
    let started_at = Instant::now();
    let toolchain = android_toolchain(HOME_CONTAINER_DIR);
    // Reconnect only to an unfinished debug run on the unchanged workspace. Completed
    // jobs belong to earlier requests, and their APK may have been removed by a sync.
    prepare_android_jobs(container_name, &toolchain, true)?;
    // The requested version goes into the synced script before the recipe runs, the way the
    // release does it; the built APK is checked against it below.
    if let Some(version) = version {
        set_container_project_version(container_name, &toolchain.workspace, layout, version)?;
        on_progress(android_progress(
            AndroidBuildPhase::PreparingTools,
            0,
            0,
            started_at,
            android_build_phase_detail(AndroidBuildPhase::PreparingTools),
            Some(format!("Building as version {}.", version.display())),
        ));
    }
    let prepare_tools = android_tools_preparation(
        &toolchain,
        layout.kind.uses_javascript(),
        !android_has_wrapper(layout),
    );
    let environment = android_recipe_environment(&toolchain);
    let jdk = android_jdk_selection(&toolchain, layout);
    let AndroidToolchain {
        build_tools,
        workspace,
        ..
    } = &toolchain;
    let prepare_project =
        android_prepare_script(layout, workspace, &toolchain.recipe_tools(), version, false);
    let gradle_dir = shell_single_quote(&android_gradle_dir(layout, workspace));
    let build_dir = android_build_dir(layout, workspace);
    let build_dir_argument = shell_single_quote(&build_dir);
    let gradle_program = android_gradle_program(&toolchain, layout);
    let debug_gradle = android_gradle_command(
        layout,
        &gradle_program,
        &android_task(layout, "assembleDebug"),
        allow_http,
    );
    let body = format!(
        r#"phase() {{ /usr/bin/printf '__BUILDBRIDGE_PHASE__:%s\n' "$1"; }}
/usr/bin/printf '__BUILDBRIDGE_ALLOW_HTTP__\t%s\n' '{allow_http}'
/bin/test -d "{workspace}"
{environment}
if /bin/test -f "{workspace}/.buildbridge/env.sh"; then
    . "{workspace}/.buildbridge/env.sh"
fi

phase preparing_tools
{prepare_tools}

{prepare_project}
phase building
{jdk}cd {gradle_dir}
{debug_gradle}

phase inspecting
apk=$(/usr/bin/find {build_dir_argument}/outputs/apk -type f -name '*.apk' -path '*debug*' 2>/dev/null | /usr/bin/sort | /usr/bin/head -n 1)
if /bin/test -z "$apk"; then
    /usr/bin/printf '%s\n' 'Gradle finished without a debug APK under the module'"'"'s build/outputs/apk.' >&2
    exit 1
fi
/usr/bin/printf '__BUILDBRIDGE_APK_PATH__\t%s\n' "$apk"
badging=$("{build_tools}/aapt2" dump badging "$apk" | /usr/bin/head -n 1)
package=$(/usr/bin/printf '%s\n' "$badging" | /usr/bin/sed -n "s/^package: name='\([^']*\)'.*/\1/p")
version_code=$(/usr/bin/printf '%s\n' "$badging" | /usr/bin/sed -n "s/.* versionCode='\([^']*\)'.*/\1/p")
version_name=$(/usr/bin/printf '%s\n' "$badging" | /usr/bin/sed -n "s/.* versionName='\([^']*\)'.*/\1/p")
jdk=$("$JAVA_HOME/bin/java" -version 2>&1 | /usr/bin/head -n 1)
apk_bytes=$(/usr/bin/stat -c %s "$apk")
apk_sha256=$(/usr/bin/sha256sum "$apk" | /usr/bin/cut -d ' ' -f 1)
/usr/bin/printf '__BUILDBRIDGE_APP__\t%s\t%s\t%s\n' "$package" "$version_name" "$version_code"
/usr/bin/printf '__BUILDBRIDGE_TOOLCHAIN__\t%s\t%s\n' "$jdk" "{BUILD_TOOLS_VERSION}"
/usr/bin/printf '__BUILDBRIDGE_APK__\t%s\t%s\n' "$apk_bytes" "$apk_sha256""#
    );
    // The recipe ends in the inspecting phase on purpose: the transfer that follows is this
    // host's, and completion is reported once the APK is here.
    let mut container_apk: Option<String> = None;

    let mut phase = AndroidBuildPhase::PreparingTools;
    let mut app: Option<(String, String, String)> = None;
    let mut apk: Option<(u64, String)> = None;
    let mut jdk_version: Option<String> = None;
    let mut built_allow_http = None;
    on_progress(android_progress(
        phase,
        0,
        0,
        started_at,
        android_build_phase_detail(phase),
        None,
    ));
    let (output_tail, diagnostic_lines, succeeded) = run_container_job(
        container_name,
        "android-debug-build",
        &toolchain,
        &body,
        |line, tail| {
            if let Some(fields) = marker_fields(line, "__BUILDBRIDGE_ALLOW_HTTP__") {
                built_allow_http = match fields.as_slice() {
                    ["true"] => Some(true),
                    ["false"] => Some(false),
                    _ => None,
                };
                return;
            }
            if let Some(value) = line.strip_prefix("__BUILDBRIDGE_PHASE__:") {
                if let Some(next) = android_build_phase(value) {
                    phase = next;
                    on_progress(android_progress(
                        phase,
                        0,
                        0,
                        started_at,
                        android_build_phase_detail(phase),
                        None,
                    ));
                }
                return;
            }
            if line == "__BUILDBRIDGE_REATTACHED__:yes" {
                let message =
                    "Reattached to the build already running in the container.".to_string();
                tail.push(message.clone());
                on_progress(android_progress(
                    phase,
                    0,
                    0,
                    started_at,
                    "Reconnecting to the active build",
                    Some(message),
                ));
                return;
            }
            if let Some(fields) = marker_fields(line, "__BUILDBRIDGE_APP__")
                && fields.len() == 3
            {
                app = Some((
                    fields[0].to_string(),
                    fields[1].to_string(),
                    fields[2].to_string(),
                ));
                return;
            }
            if let Some(fields) = marker_fields(line, "__BUILDBRIDGE_TOOLCHAIN__")
                && fields.len() == 2
            {
                jdk_version = Some(fields[0].to_string());
                return;
            }
            if let Some(fields) = marker_fields(line, "__BUILDBRIDGE_APK_PATH__")
                && fields.len() == 1
                && fields[0].starts_with(&format!("{build_dir}/outputs/apk/"))
                && !fields[0].contains("..")
            {
                container_apk = Some(fields[0].to_string());
                return;
            }
            if let Some(fields) = marker_fields(line, "__BUILDBRIDGE_APK__")
                && fields.len() == 2
            {
                apk = fields[0]
                    .parse()
                    .ok()
                    .map(|bytes| (bytes, fields[1].to_string()));
                return;
            }
            if !line.starts_with("__BUILDBRIDGE_") {
                on_progress(android_progress(
                    phase,
                    0,
                    0,
                    started_at,
                    android_build_phase_detail(phase),
                    Some(line.to_string()),
                ));
            }
        },
    )?;
    if !succeeded {
        let context = build_failure_context(&diagnostic_lines, &output_tail);
        return Err(ProviderError::AndroidToolchain(if context.is_empty() {
            format!(
                "the Android debug build failed during {}",
                android_build_phase_detail(phase).to_ascii_lowercase()
            )
        } else {
            format!(
                "the Android debug build failed during {}:\n{context}",
                android_build_phase_detail(phase).to_ascii_lowercase()
            )
        }));
    }
    validate_android_debug_http_mode(allow_http, built_allow_http)?;
    let (application_id, version_name, version_code) = app.ok_or_else(|| {
        ProviderError::AndroidToolchain("the container did not report the built app".to_string())
    })?;
    if !valid_application_id(&application_id)
        || !valid_version_value(&version_name)
        || !valid_version_value(&version_code)
    {
        return Err(ProviderError::AndroidToolchain(
            "the container reported an app identity in an unexpected shape".to_string(),
        ));
    }
    if let Some(version) = version
        && (version_name != version.version || version_code != version.build)
    {
        return Err(ProviderError::AndroidToolchain(format!(
            "the debug build reports version {version_name} ({version_code}), not the requested {}; a flavour or a script in the project overrides the version declared in defaultConfig",
            version.display()
        )));
    }
    let apk_path = output_directory.join(format!(
        "app-debug-{}.apk",
        version_file_tag(&version_name, &version_code)
    ));
    let (apk_bytes, apk_sha256) = apk.ok_or_else(|| {
        ProviderError::AndroidToolchain("the container did not report the debug APK".to_string())
    })?;
    if !valid_sha256(&apk_sha256) || apk_bytes == 0 || apk_bytes > ANDROID_RELEASE_MAX_BYTES {
        return Err(ProviderError::AndroidToolchain(
            "the container reported the debug APK in an unexpected shape".to_string(),
        ));
    }
    let container_apk = container_apk.ok_or_else(|| {
        ProviderError::AndroidToolchain(
            "the container did not report where the debug APK was written".to_string(),
        )
    })?;
    let transferred = (|| {
        on_progress(android_progress(
            AndroidBuildPhase::Transferring,
            0,
            apk_bytes,
            started_at,
            "Copying the debug APK to this host.",
            None,
        ));
        stream_container_file(
            container_name,
            &container_apk,
            &apk_part,
            apk_bytes,
            |copied| {
                on_progress(android_progress(
                    AndroidBuildPhase::Transferring,
                    copied,
                    apk_bytes,
                    started_at,
                    "Copying the debug APK to this host.",
                    None,
                ));
            },
        )?;
        verify_local_artifact(&apk_part, apk_bytes, &apk_sha256)?;
        fs::rename(&apk_part, &apk_path).map_err(|error| {
            ProviderError::AndroidToolchain(format!("could not place the debug APK: {error}"))
        })?;
        set_artifact_permissions(&apk_path)
    })();
    if let Err(error) = transferred {
        let _ = fs::remove_file(&apk_part);
        let _ = fs::remove_file(&apk_path);
        return Err(error);
    }
    on_progress(android_progress(
        AndroidBuildPhase::Completed,
        apk_bytes,
        apk_bytes,
        started_at,
        android_build_phase_detail(AndroidBuildPhase::Completed),
        None,
    ));

    Ok(AndroidBuildResult {
        application_id,
        version_name,
        version_code,
        toolchain: AndroidToolchainSummary {
            jdk_version: jdk_version.unwrap_or_else(|| "unknown".to_string()),
            build_tools_version: BUILD_TOOLS_VERSION.to_string(),
        },
        apk: Some(AndroidArtifact {
            path: apk_path.to_string_lossy().to_string(),
            bytes: apk_bytes,
            sha256: apk_sha256.to_ascii_lowercase(),
        }),
        allow_http,
        output_tail,
    })
}

fn validate_android_debug_http_mode(
    requested: bool,
    built: Option<bool>,
) -> Result<(), ProviderError> {
    if built == Some(requested) {
        return Ok(());
    }
    Err(ProviderError::AndroidToolchain(
        "The reattached Android build used different or unknown HTTP API settings. Retry the debug build to apply the selected option."
            .to_string(),
    ))
}

pub(crate) fn valid_key_alias(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

/// A keystore password as `keytool` accepts one and a file can carry it: six characters or
/// more, one line.
pub(crate) fn valid_keystore_password(value: &str) -> bool {
    value.chars().count() >= 6 && value.len() <= 512 && !value.chars().any(char::is_control)
}

/// The name that goes into the certificate's subject: letters, digits and a few separators,
/// none of the characters a distinguished name gives meaning to.
pub(crate) fn valid_certificate_name(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 64
        && trimmed.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, ' ' | '.' | '_' | '-')
        })
}

// The host lock does not survive a desktop restart, while container jobs deliberately do.
// Check every worker before any cleanup: synchronization and releases require an idle
// workspace, and only a debug invocation may reconnect to an unfinished debug worker.
// Completed jobs must never replay old success markers for a replaced workspace or key.
const ANDROID_JOB_PREPARATION: &str = r#"set -eu
for job_name in android-debug-build android-release; do
    job_path="$1/$job_name"
    if /bin/test -e "$job_path" && ! /bin/test -f "$job_path/status"; then
        if /bin/test "$2" = 1 && /bin/test "$job_name" = android-debug-build; then
            continue
        fi
        /usr/bin/printf '%s\n' 'A previous Android build is still running or unfinished in this toolchain. Stop and start the Android toolchain, then retry. Its workspace and job files have been preserved.' >&2
        exit 1
    fi
done
for job_name in android-debug-build android-release; do
    job_path="$1/$job_name"
    if /bin/test -f "$job_path/status"; then
        /bin/rm -rf "$job_path"
    fi
done"#;

fn prepare_android_jobs(
    container_name: &str,
    toolchain: &AndroidToolchain,
    reattach_debug: bool,
) -> Result<(), ProviderError> {
    let jobs_root = format!("{}/jobs", toolchain.tools);
    let output = container_exec_command(
        container_name,
        ANDROID_JOB_PREPARATION,
        &[&jobs_root, if reattach_debug { "1" } else { "0" }],
    )
    .stdin(Stdio::null())
    .tracked_output()
    .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ProviderError::AndroidToolchain(clean_output(
            &output.stderr,
        )))
    }
}

/// Validates inspection metadata and bounds only the artifacts selected for transfer.
fn selected_android_release_bytes(
    outputs: AndroidReleaseOutputs,
    apk: &(u64, String),
    aab: Option<&(u64, String)>,
) -> Result<u64, ProviderError> {
    // Even an AAB-only release uses the internal APK for package and signing inspection.
    if apk.0 == 0 || !valid_sha256(&apk.1) {
        return Err(ProviderError::AndroidToolchain(
            "the container reported invalid APK metadata".to_string(),
        ));
    }
    let aab_bytes = if outputs.includes_aab() {
        let aab = aab.ok_or_else(|| {
            ProviderError::AndroidToolchain(
                "the container did not report the selected app bundle".to_string(),
            )
        })?;
        if aab.0 == 0 || !valid_sha256(&aab.1) {
            return Err(ProviderError::AndroidToolchain(
                "the container reported invalid app bundle metadata".to_string(),
            ));
        }
        aab.0
    } else {
        0
    };
    let apk_bytes = if outputs.includes_apk() { apk.0 } else { 0 };
    let total = apk_bytes.checked_add(aab_bytes).ok_or_else(|| {
        ProviderError::AndroidToolchain("the artifact sizes overflowed".to_string())
    })?;
    if total > ANDROID_RELEASE_MAX_BYTES {
        return Err(ProviderError::AndroidToolchain(
            "the selected artifacts exceed the 4 GiB transfer limit".to_string(),
        ));
    }
    Ok(total)
}

/// Builds and verifies a signed Android release, retaining only the selected files. A
/// requested version is written into the synced app module's Gradle script first, the same
/// edit the engine makes in the project on the host, and the signed APK is checked against it.
#[allow(clippy::too_many_arguments)]
pub fn run_signed_android_release<F>(
    container_name: &str,
    layout: &ProjectLayout,
    signing: &AndroidSigningMaterial,
    env: Option<&GuestEnvFiles>,
    outputs: AndroidReleaseOutputs,
    version: Option<&ProjectVersion>,
    output_directory: &Path,
    mut on_progress: F,
) -> Result<AndroidReleaseResult, ProviderError>
where
    F: FnMut(AndroidReleaseProgress),
{
    validate_layout(layout).map_err(|error| ProviderError::AndroidToolchain(error.to_string()))?;
    let keystore = read_signing_keystore(signing)?;
    if let Some(version) = version {
        validate_android_version(version).map_err(ProviderError::AndroidToolchain)?;
    }
    validate_archive_output_directory(output_directory)?;
    // The retained files are named once the signed APK has said which version it is; the
    // partials they arrive as are fixed, and the destination must start empty.
    let aab_part = output_directory.join(format!("{AAB_NAME}.part"));
    let apk_part = output_directory.join(format!("{APK_NAME}.part"));
    if directory_has_entries(output_directory).map_err(|error| {
        ProviderError::AndroidToolchain(format!("the artifact directory is unavailable: {error}"))
    })? {
        return Err(ProviderError::AndroidToolchain(
            "the artifact directory is not empty".to_string(),
        ));
    }
    let mut retained_paths: Vec<PathBuf> = Vec::new();

    let started_at = Instant::now();
    let toolchain = android_toolchain(HOME_CONTAINER_DIR);
    let prepare_tools = android_tools_preparation(
        &toolchain,
        layout.kind.uses_javascript(),
        !android_has_wrapper(layout),
    );
    let environment = android_recipe_environment(&toolchain);
    let jdk = android_jdk_selection(&toolchain, layout);
    let AndroidToolchain {
        build_tools,
        workspace,
        signing: signing_dir,
        home,
        ..
    } = &toolchain;
    on_progress(release_progress(
        AndroidReleasePhase::Preparing,
        0,
        0,
        started_at,
        android_release_phase_detail(AndroidReleasePhase::Preparing),
        None,
    ));

    prepare_android_jobs(container_name, &toolchain, false)?;
    // Verify the exact bytes we will stage, before any project script sees the key.
    let checked_certificate = verify_signing_bytes(signing, &keystore)?;
    let jarsigner_algorithm = checked_certificate.algorithm.jarsigner_algorithm();
    let expected_certificate = &checked_certificate.certificate_sha256;

    // The key and its passwords land as owner-only files and are removed by the recipe and
    // again afterwards, whichever way the recipe ends.
    let cleanup_signing = || {
        let _ = run_container_command(
            container_name,
            &format!("/bin/rm -rf {}", shell_single_quote(signing_dir)),
        );
    };
    let staged = (|| {
        run_container_command(
            container_name,
            &format!(
                "set -eu; /bin/rm -rf {0}; /bin/mkdir -p {0}; /bin/chmod 700 {0}",
                shell_single_quote(signing_dir)
            ),
        )?;
        stream_bytes_to_container(
            container_name,
            &keystore,
            &format!("{signing_dir}/release.keystore"),
            "keystore",
        )?;
        stream_bytes_to_container(
            container_name,
            signing.keystore_password.as_bytes(),
            &format!("{signing_dir}/store.pass"),
            "keystore password",
        )?;
        stream_bytes_to_container(
            container_name,
            signing.key_password.as_bytes(),
            &format!("{signing_dir}/key.pass"),
            "key password",
        )?;
        if let Some(env) = env {
            stream_bytes_to_container(
                container_name,
                env.dotenv.as_bytes(),
                &format!("{workspace}/.env.production.local"),
                "env file",
            )?;
            stream_bytes_to_container(
                container_name,
                env.shell.as_bytes(),
                &format!("{workspace}/.buildbridge/env.sh"),
                "env script",
            )?;
        }
        if let Some(version) = version {
            set_container_project_version(container_name, workspace, layout, version)?;
            on_progress(release_progress(
                AndroidReleasePhase::Preparing,
                0,
                0,
                started_at,
                android_release_phase_detail(AndroidReleasePhase::Preparing),
                Some(format!("Building as version {}.", version.display())),
            ));
        }
        Ok::<_, ProviderError>(())
    })();
    if let Err(error) = staged {
        cleanup_signing();
        return Err(error);
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let staging = format!("{home}/BuildBridge/artifacts/release-{nonce}");
    // A release with a chosen env rebuilds what reads it; a Flutter or native project reads
    // it from the build's environment and needs nothing rebuilt.
    let rebuild_web_assets = if env.is_some() {
        format!(
            "cd \"{workspace}\"\n. \"{workspace}/.buildbridge/env.sh\"\n{}",
            android_environment_rebuild_script(layout, workspace, &toolchain.recipe_tools())
        )
    } else {
        String::new()
    };
    // The framework's own preparation runs before the release the way it runs before the
    // debug build, since the snapshot alone is not yet a buildable native project for every
    // kind; dependencies are already installed by the debug build this release follows.
    let prepare_project =
        android_prepare_script(layout, workspace, &toolchain.recipe_tools(), version, true);
    let gradle_dir = shell_single_quote(&android_gradle_dir(layout, workspace));
    let build_dir = shell_single_quote(&android_build_dir(layout, workspace));
    let key_alias = shell_single_quote(&signing.key_alias);
    let build_aab = if outputs.includes_aab() { "1" } else { "0" };
    // A release APK remains an internal inspection artifact for AAB-only builds: aapt2
    // reads its effective application ID/version, and apksigner proves its signing key.
    let gradle_tasks = if outputs.includes_aab() {
        format!(
            "{} {}",
            android_task(layout, "bundleRelease"),
            android_task(layout, "assembleRelease")
        )
    } else {
        android_task(layout, "assembleRelease")
    };
    let gradle_command = android_gradle_command(
        layout,
        &android_gradle_program(&toolchain, layout),
        &gradle_tasks,
        false,
    );
    let body = format!(
        r#"phase() {{ /usr/bin/printf '__BUILDBRIDGE_PHASE__:%s\n' "$1"; }}
phase preparing
/bin/test -d "{workspace}"
{environment}
if /bin/test -f "{workspace}/.buildbridge/env.sh"; then
    . "{workspace}/.buildbridge/env.sh"
fi
{prepare_tools}
keystore="{signing_dir}/release.keystore"
store_pass="{signing_dir}/store.pass"
key_pass="{signing_dir}/key.pass"
/bin/test -f "$keystore"
/bin/test -f "$store_pass"
/bin/test -f "$key_pass"
"{JAVA_HOME}/bin/keytool" -list -keystore "$keystore" -storepass:file "$store_pass" -alias {key_alias} > /dev/null
{prepare_project}{rebuild_web_assets}
phase bundling
{jdk}cd {gradle_dir}
{gradle_command}

phase signing
staging="{staging}"
/bin/rm -rf "$staging"
/bin/mkdir -p "$staging"
aab_in=$(/usr/bin/find {build_dir}/outputs/bundle -type f -name '*.aab' -path '*release*' 2>/dev/null | /usr/bin/sort | /usr/bin/head -n 1)
apk_in=$(/usr/bin/find {build_dir}/outputs/apk -type f -name '*-release-unsigned.apk' 2>/dev/null | /usr/bin/sort | /usr/bin/head -n 1)
if /bin/test -z "$apk_in"; then
    apk_in=$(/usr/bin/find {build_dir}/outputs/apk -type f -name '*.apk' -path '*release*' 2>/dev/null | /usr/bin/sort | /usr/bin/head -n 1)
fi
if /bin/test -z "$apk_in"; then
    /usr/bin/printf '%s\n' 'Gradle finished without a release APK under the module'"'"'s build/outputs/apk.' >&2
    exit 1
fi
"{build_tools}/zipalign" -p -f 4 "$apk_in" "$staging/aligned.apk"
"{build_tools}/apksigner" sign --ks "$keystore" --ks-pass "file:$store_pass" --ks-key-alias {key_alias} --key-pass "file:$key_pass" --out "$staging/{APK_NAME}" "$staging/aligned.apk"
/bin/rm -f "$staging/aligned.apk"
if /bin/test {build_aab} = 1; then
    /bin/test -f "$aab_in"
    "{JAVA_HOME}/bin/jarsigner" -keystore "$keystore" -storepass:file "$store_pass" -keypass:file "$key_pass" -sigalg {jarsigner_algorithm} -digestalg SHA-256 -signedjar "$staging/{AAB_NAME}" "$aab_in" {key_alias} > /dev/null
fi
/bin/rm -rf "{signing_dir}"

phase verifying
"{build_tools}/apksigner" verify --print-certs "$staging/{APK_NAME}" > "$staging/verify.txt"
certificate=$(/usr/bin/sed -n 's/^Signer #1 certificate SHA-256 digest: //p' "$staging/verify.txt" | /usr/bin/head -n 1)
/bin/rm -f "$staging/verify.txt"
/bin/test -n "$certificate"
if /bin/test "$certificate" != "{expected_certificate}"; then
    /usr/bin/printf '%s\n' 'The APK signing certificate differs from the verified upload key.' >&2
    exit 1
fi
if /bin/test {build_aab} = 1; then
cat > "$staging/AndroidSigningCheck.java" <<'BUILDBRIDGE_JAVA_SOURCE'
{SIGNING_CHECK_SOURCE}
BUILDBRIDGE_JAVA_SOURCE
"{JAVA_HOME}/bin/java" -Xmx192m "$staging/AndroidSigningCheck.java" verify-jar "$staging/{AAB_NAME}" "{expected_certificate}"
/bin/rm -f "$staging/AndroidSigningCheck.java"
fi
badging=$("{build_tools}/aapt2" dump badging "$staging/{APK_NAME}" | /usr/bin/head -n 1)
package=$(/usr/bin/printf '%s\n' "$badging" | /usr/bin/sed -n "s/^package: name='\([^']*\)'.*/\1/p")
version_code=$(/usr/bin/printf '%s\n' "$badging" | /usr/bin/sed -n "s/.* versionCode='\([^']*\)'.*/\1/p")
version_name=$(/usr/bin/printf '%s\n' "$badging" | /usr/bin/sed -n "s/.* versionName='\([^']*\)'.*/\1/p")
apk_bytes=$(/usr/bin/stat -c %s "$staging/{APK_NAME}")
apk_sha256=$(/usr/bin/sha256sum "$staging/{APK_NAME}" | /usr/bin/cut -d ' ' -f 1)
/usr/bin/printf '__BUILDBRIDGE_APP__\t%s\t%s\t%s\t%s\n' "$package" "$version_name" "$version_code" "$certificate"
/usr/bin/printf '__BUILDBRIDGE_APK__\t%s\t%s\n' "$apk_bytes" "$apk_sha256"
if /bin/test {build_aab} = 1; then
    aab_bytes=$(/usr/bin/stat -c %s "$staging/{AAB_NAME}")
    aab_sha256=$(/usr/bin/sha256sum "$staging/{AAB_NAME}" | /usr/bin/cut -d ' ' -f 1)
    /usr/bin/printf '__BUILDBRIDGE_AAB__\t%s\t%s\n' "$aab_bytes" "$aab_sha256"
fi"#
    );
    // The recipe ends in the verifying phase on purpose: the transfer that follows is this
    // host's, and completion is reported once the artifacts are here.

    let mut phase = AndroidReleasePhase::Preparing;
    let mut app: Option<(String, String, String, String)> = None;
    let mut apk: Option<(u64, String)> = None;
    let mut aab: Option<(u64, String)> = None;
    let job = run_container_job(
        container_name,
        "android-release",
        &toolchain,
        &body,
        |line, tail| {
            if let Some(value) = line.strip_prefix("__BUILDBRIDGE_PHASE__:") {
                if let Some(next) = android_release_phase(value) {
                    phase = next;
                    on_progress(release_progress(
                        phase,
                        0,
                        0,
                        started_at,
                        android_release_phase_detail(phase),
                        None,
                    ));
                }
                return;
            }
            if line == "__BUILDBRIDGE_REATTACHED__:yes" {
                let message =
                    "Reattached to the release already running in the container.".to_string();
                tail.push(message.clone());
                on_progress(release_progress(
                    phase,
                    0,
                    0,
                    started_at,
                    "Reconnecting to the active release",
                    Some(message),
                ));
                return;
            }
            if let Some(fields) = marker_fields(line, "__BUILDBRIDGE_APP__")
                && fields.len() == 4
            {
                app = Some((
                    fields[0].to_string(),
                    fields[1].to_string(),
                    fields[2].to_string(),
                    fields[3].to_string(),
                ));
                return;
            }
            if let Some(fields) = marker_fields(line, "__BUILDBRIDGE_APK__")
                && fields.len() == 2
            {
                apk = fields[0]
                    .parse()
                    .ok()
                    .map(|bytes| (bytes, fields[1].to_string()));
                return;
            }
            if let Some(fields) = marker_fields(line, "__BUILDBRIDGE_AAB__")
                && fields.len() == 2
            {
                aab = fields[0]
                    .parse()
                    .ok()
                    .map(|bytes| (bytes, fields[1].to_string()));
                return;
            }
            if !line.starts_with("__BUILDBRIDGE_") {
                on_progress(release_progress(
                    phase,
                    0,
                    0,
                    started_at,
                    android_release_phase_detail(phase),
                    Some(line.to_string()),
                ));
            }
        },
    );
    cleanup_signing();
    let cleanup_staging = || {
        let _ = run_container_command(
            container_name,
            &format!("/bin/rm -rf {}", shell_single_quote(&staging)),
        );
    };
    let (output_tail, diagnostic_lines, succeeded) = match job {
        Ok(job) => job,
        Err(error) => {
            cleanup_staging();
            return Err(error);
        }
    };
    if !succeeded {
        cleanup_staging();
        let context = build_failure_context(&diagnostic_lines, &output_tail);
        return Err(ProviderError::AndroidToolchain(if context.is_empty() {
            format!(
                "the signed Android release failed during {}",
                android_release_phase_detail(phase).to_ascii_lowercase()
            )
        } else {
            format!(
                "the signed Android release failed during {}:\n{context}",
                android_release_phase_detail(phase).to_ascii_lowercase()
            )
        }));
    }

    let transferred = (|| {
        let (application_id, version_name, version_code, certificate_sha256) =
            app.ok_or_else(|| {
                ProviderError::AndroidToolchain(
                    "the container did not report the signed app".to_string(),
                )
            })?;
        let apk = apk.ok_or_else(|| {
            ProviderError::AndroidToolchain("the container did not report the APK".to_string())
        })?;
        let total_bytes = selected_android_release_bytes(outputs, &apk, aab.as_ref())?;
        let (apk_bytes, apk_sha256) = apk;
        let (aab_bytes, aab_sha256) = if outputs.includes_aab() {
            aab.ok_or_else(|| {
                ProviderError::AndroidToolchain(
                    "the container did not report the app bundle".to_string(),
                )
            })?
        } else {
            (0, String::new())
        };
        if !valid_application_id(&application_id)
            || !valid_version_value(&version_name)
            || !valid_version_value(&version_code)
            || !certificate_sha256
                .chars()
                .all(|character| character.is_ascii_hexdigit())
            || certificate_sha256.len() != 64
        {
            return Err(ProviderError::AndroidToolchain(
                "the container reported the release in an unexpected shape".to_string(),
            ));
        }
        if let Some(version) = version
            && (version_name != version.version || version_code != version.build)
        {
            return Err(ProviderError::AndroidToolchain(format!(
                "the signed release reports version {version_name} ({version_code}), not the requested {}; a flavour or a script in the project overrides the version declared in defaultConfig",
                version.display()
            )));
        }
        if !certificate_sha256.eq_ignore_ascii_case(expected_certificate) {
            return Err(ProviderError::AndroidToolchain(
                "the signed APK does not use the upload certificate verified before this release"
                    .to_string(),
            ));
        }
        let tag = version_file_tag(&version_name, &version_code);
        let aab_path = output_directory.join(format!("app-release-{tag}.aab"));
        let apk_path = output_directory.join(format!("app-release-{tag}.apk"));
        retained_paths.extend([aab_path.clone(), apk_path.clone()]);
        if outputs.includes_aab() {
            on_progress(release_progress(
                AndroidReleasePhase::Transferring,
                0,
                total_bytes,
                started_at,
                "Copying the signed app bundle to this host.",
                None,
            ));
            stream_container_file(
                container_name,
                &format!("{staging}/{AAB_NAME}"),
                &aab_part,
                aab_bytes,
                |copied| {
                    on_progress(release_progress(
                        AndroidReleasePhase::Transferring,
                        copied,
                        total_bytes,
                        started_at,
                        "Copying the signed app bundle to this host.",
                        None,
                    ));
                },
            )?;
            verify_local_artifact(&aab_part, aab_bytes, &aab_sha256)?;
            fs::rename(&aab_part, &aab_path).map_err(|error| {
                ProviderError::AndroidToolchain(format!("could not place the app bundle: {error}"))
            })?;
            set_artifact_permissions(&aab_path)?;
        }
        if outputs.includes_apk() {
            on_progress(release_progress(
                AndroidReleasePhase::Transferring,
                aab_bytes,
                total_bytes,
                started_at,
                "Copying the signed APK to this host.",
                None,
            ));
            stream_container_file(
                container_name,
                &format!("{staging}/{APK_NAME}"),
                &apk_part,
                apk_bytes,
                |copied| {
                    on_progress(release_progress(
                        AndroidReleasePhase::Transferring,
                        aab_bytes + copied,
                        total_bytes,
                        started_at,
                        "Copying the signed APK to this host.",
                        None,
                    ));
                },
            )?;
            verify_local_artifact(&apk_part, apk_bytes, &apk_sha256)?;
            fs::rename(&apk_part, &apk_path).map_err(|error| {
                ProviderError::AndroidToolchain(format!("could not place the APK: {error}"))
            })?;
            set_artifact_permissions(&apk_path)?;
        }

        Ok(AndroidReleaseResult {
            application_id,
            version_name,
            version_code,
            key_alias: signing.key_alias.clone(),
            certificate_sha256: certificate_sha256.to_ascii_lowercase(),
            aab: outputs.includes_aab().then(|| AndroidArtifact {
                path: aab_path.to_string_lossy().to_string(),
                bytes: aab_bytes,
                sha256: aab_sha256.to_ascii_lowercase(),
            }),
            apk: outputs.includes_apk().then(|| AndroidArtifact {
                path: apk_path.to_string_lossy().to_string(),
                bytes: apk_bytes,
                sha256: apk_sha256.to_ascii_lowercase(),
            }),
            output_tail,
        })
    })();
    cleanup_staging();
    match transferred {
        Ok(result) => {
            let total_bytes = result.artifacts().map(|artifact| artifact.bytes).sum();
            on_progress(release_progress(
                AndroidReleasePhase::Completed,
                total_bytes,
                total_bytes,
                started_at,
                android_release_phase_detail(AndroidReleasePhase::Completed),
                None,
            ));
            Ok(result)
        }
        Err(error) => {
            for path in retained_paths.iter().chain([&aab_part, &apk_part]) {
                let _ = fs::remove_file(path);
            }
            Err(error)
        }
    }
}

/// The script a throwaway container runs to create an upload key: the password arrives on
/// stdin and never as an argument, the keystore leaves on stdout, and the certificate's
/// SHA-256 on stderr behind a marker. The alias and the name are positional arguments.
const KEYSTORE_CREATION_SCRIPT: &str = r#"set -eu
umask 077
/bin/mkdir -p /tmp/buildbridge-keystore
/bin/cat > /tmp/buildbridge-keystore/pass
/opt/java/openjdk/bin/keytool -genkeypair -keystore /tmp/buildbridge-keystore/upload.keystore -storetype PKCS12 -alias "$1" -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=$2" -storepass:file /tmp/buildbridge-keystore/pass -keypass:file /tmp/buildbridge-keystore/pass > /dev/null 2>&1
certificate=$(/opt/java/openjdk/bin/keytool -list -v -keystore /tmp/buildbridge-keystore/upload.keystore -storepass:file /tmp/buildbridge-keystore/pass -alias "$1" | /usr/bin/sed -n 's/^[[:space:]]*SHA256: //p' | /usr/bin/head -n 1)
/usr/bin/printf '__BUILDBRIDGE_CERTIFICATE__\t%s\n' "$certificate" >&2
/bin/cat /tmp/buildbridge-keystore/upload.keystore
"#;

fn keystore_creation_args(key_alias: &str, certificate_name: &str) -> Vec<String> {
    [
        "run",
        "--rm",
        ANDROID_PLATFORM_ARG,
        "--interactive",
        "--entrypoint=/bin/sh",
        ANDROID_IMAGE,
        "-c",
        KEYSTORE_CREATION_SCRIPT,
        "sh",
        key_alias,
        certificate_name.trim(),
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Creates an upload key in a throwaway container of the toolchain image and writes the
/// keystore owner-only to `output_path`. Nothing is created on this host but that file.
pub fn create_android_keystore(
    output_path: &Path,
    password: &str,
    key_alias: &str,
    certificate_name: &str,
) -> Result<AndroidKeystoreSummary, ProviderError> {
    if !valid_key_alias(key_alias) {
        return Err(ProviderError::AndroidToolchain(
            "the key alias may only contain letters, digits, dots, underscores and dashes"
                .to_string(),
        ));
    }
    if !valid_keystore_password(password) {
        return Err(ProviderError::AndroidToolchain(
            "the keystore password must be one line of six to 512 characters".to_string(),
        ));
    }
    if !valid_certificate_name(certificate_name) {
        return Err(ProviderError::AndroidToolchain(
            "the certificate name may only contain letters, digits, spaces, dots, underscores and dashes"
                .to_string(),
        ));
    }
    if output_path.exists() {
        return Err(ProviderError::AndroidToolchain(format!(
            "{} already exists",
            output_path.display()
        )));
    }
    ensure_android_image()?;
    let mut child = docker_command()
        .args(keystore_creation_args(key_alias, certificate_name))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ProviderError::AndroidToolchain("could not hand the password to keytool".to_string())
    })?;
    stdin.write_all(password.as_bytes()).map_err(|error| {
        ProviderError::AndroidToolchain(format!("could not hand the password to keytool: {error}"))
    })?;
    drop(stdin);
    let output = child.wait_with_output().map_err(|error| {
        ProviderError::AndroidToolchain(format!("keytool did not finish: {error}"))
    })?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let message = stderr
            .lines()
            .filter(|line| !line.starts_with("__BUILDBRIDGE_"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(ProviderError::AndroidToolchain(format!(
            "keytool could not create the keystore: {}",
            android_execution_message(&message.trim().chars().take(2_000).collect::<String>())
        )));
    }
    let certificate_sha256 = stderr
        .lines()
        .find_map(|line| marker_fields(line, "__BUILDBRIDGE_CERTIFICATE__"))
        .and_then(|fields| {
            fields
                .first()
                .map(|value| value.replace(':', "").to_ascii_lowercase())
        })
        .filter(|value| valid_sha256(value))
        .ok_or_else(|| {
            ProviderError::AndroidToolchain(
                "keytool did not report the certificate it created".to_string(),
            )
        })?;
    if output.stdout.len() < 64 || output.stdout.len() > 1024 * 1024 {
        return Err(ProviderError::AndroidToolchain(
            "keytool returned a keystore of an unexpected size".to_string(),
        ));
    }
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ProviderError::AndroidToolchain(format!(
                "could not create the keystore directory: {error}"
            ))
        })?;
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output_path)
        .map_err(|error| {
            ProviderError::AndroidToolchain(format!("could not write the keystore: {error}"))
        })?;
    set_artifact_permissions(output_path)?;
    file.write_all(&output.stdout).map_err(|error| {
        ProviderError::AndroidToolchain(format!("could not write the keystore: {error}"))
    })?;
    file.sync_all().map_err(|error| {
        ProviderError::AndroidToolchain(format!("could not flush the keystore: {error}"))
    })?;

    Ok(AndroidKeystoreSummary {
        path: output_path.to_string_lossy().to_string(),
        key_alias: key_alias.to_string(),
        certificate_sha256,
    })
}

/// Stops any buildbridge job still running in the container after its host-side operation was
/// cancelled: the builds deliberately survive their exec so a desktop restart can reattach,
/// and a Stop must reach past that.
pub fn stop_android_jobs(container_name: &str) -> Result<(), ProviderError> {
    run_container_command(container_name, &stop_android_jobs_script()).map(|_| ())
}

fn stop_android_jobs_script() -> String {
    let jobs = format!("{HOME_CONTAINER_DIR}/.buildbridge/tools/jobs");
    let signing = android_toolchain(HOME_CONTAINER_DIR).signing;
    format!(
        "for pid_file in {0}/*/pid; do if /bin/test -f \"$pid_file\"; then pid=$(/bin/cat \"$pid_file\"); pgid=$(/bin/ps -o pgid= -p \"$pid\" 2>/dev/null | /usr/bin/tr -d ' '); if /bin/test -n \"$pgid\"; then /bin/kill -TERM -- \"-$pgid\" 2>/dev/null || /bin/true; fi; /bin/kill -TERM \"$pid\" 2>/dev/null || /bin/true; fi; done; /bin/rm -rf {0}/* {1}; /usr/bin/true",
        shell_single_quote(&jobs),
        shell_single_quote(&signing)
    )
}

#[cfg(test)]
#[path = "android_http_tests.rs"]
mod http_tests;

#[cfg(test)]
#[path = "android_job_tests.rs"]
mod job_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_release_job_is_rejected_without_modifying_its_state() {
        let root = std::env::temp_dir().join(format!(
            "buildbridge-release-retry-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let job = root.join("android-release");
        let check = || {
            Command::new("/bin/sh")
                .args(["-c", ANDROID_JOB_PREPARATION, "sh"])
                .arg(&root)
                .arg("0")
                .output()
                .unwrap()
        };
        assert!(check().status.success());
        fs::create_dir(&job).unwrap();
        fs::write(job.join("meta"), "previous-request").unwrap();
        let refused = check();
        assert!(!refused.status.success());
        assert!(clean_output(&refused.stderr).contains("Stop and start"));
        assert_eq!(
            fs::read_to_string(job.join("meta")).unwrap(),
            "previous-request"
        );
        fs::write(job.join("status"), "0").unwrap();
        assert!(
            check().status.success(),
            "a completed job is cleared before starting a fresh recipe"
        );
        assert!(!job.exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn stopping_android_jobs_removes_staged_signing_secrets() {
        let script = stop_android_jobs_script();
        let signing = android_toolchain(HOME_CONTAINER_DIR).signing;
        assert!(script.contains(&format!(
            "/bin/rm -rf '{HOME_CONTAINER_DIR}/.buildbridge/tools/jobs'/* '{}'",
            signing
        )));
        assert!(script.find("/bin/kill -TERM").unwrap() < script.find("/bin/rm -rf").unwrap());
        assert!(!script.contains("/BuildBridge/workspace"));
    }

    #[cfg(unix)]
    #[test]
    fn stopped_job_cleanup_deletes_credentials_and_preserves_project_and_tools() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!(
            "buildbridge-stop-signing-{}-{nonce}",
            std::process::id()
        ));
        let toolchain = android_toolchain(home.to_str().unwrap());
        fs::create_dir_all(&toolchain.signing).unwrap();
        fs::create_dir_all(&toolchain.workspace).unwrap();
        fs::create_dir_all(&toolchain.tools).unwrap();
        for name in ["release.keystore", "store.pass", "key.pass"] {
            fs::write(
                Path::new(&toolchain.signing).join(name),
                b"disposable fixture",
            )
            .unwrap();
        }
        let project = Path::new(&toolchain.workspace).join("keep-project");
        let tools = Path::new(&toolchain.tools).join("keep-tools");
        fs::write(&project, b"project").unwrap();
        fs::write(&tools, b"tools").unwrap();
        let script = stop_android_jobs_script().replace(HOME_CONTAINER_DIR, home.to_str().unwrap());
        assert!(
            Command::new("/bin/sh")
                .args(["-c", &script])
                .status()
                .unwrap()
                .success()
        );
        assert!(!Path::new(&toolchain.signing).exists());
        assert!(project.is_file());
        assert!(tools.is_file());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn selected_release_artifacts_count_only_the_requested_files() {
        let apk = (700, "ab".repeat(32));
        let aab = (300, "cd".repeat(32));
        assert_eq!(
            selected_android_release_bytes(AndroidReleaseOutputs::Both, &apk, Some(&aab)).unwrap(),
            1000
        );
        assert_eq!(
            selected_android_release_bytes(AndroidReleaseOutputs::Aab, &apk, Some(&aab)).unwrap(),
            300
        );
        assert_eq!(
            selected_android_release_bytes(AndroidReleaseOutputs::Apk, &apk, None).unwrap(),
            700
        );
        assert!(selected_android_release_bytes(AndroidReleaseOutputs::Aab, &apk, None).is_err());
    }

    #[test]
    fn selected_release_artifacts_reject_invalid_metadata_and_oversized_transfers() {
        let apk = (1, "ab".repeat(32));
        let invalid = (1, "not-a-digest".to_string());
        let empty = (0, "ab".repeat(32));
        let large = (ANDROID_RELEASE_MAX_BYTES, "cd".repeat(32));
        assert!(
            selected_android_release_bytes(AndroidReleaseOutputs::Apk, &invalid, None).is_err()
        );
        assert!(
            selected_android_release_bytes(AndroidReleaseOutputs::Aab, &apk, Some(&empty)).is_err()
        );
        assert!(
            selected_android_release_bytes(AndroidReleaseOutputs::Both, &apk, Some(&large))
                .is_err()
        );
        assert_eq!(
            selected_android_release_bytes(AndroidReleaseOutputs::Aab, &apk, Some(&large)).unwrap(),
            ANDROID_RELEASE_MAX_BYTES
        );
        let overflow = (u64::MAX, "cd".repeat(32));
        assert!(
            selected_android_release_bytes(AndroidReleaseOutputs::Both, &apk, Some(&overflow))
                .is_err()
        );
    }

    fn profile() -> MachineConfig {
        MachineConfig {
            provider: MachineProvider::AndroidToolchain,
            memory_gib: 6,
            cpu_cores: 3,
            ..MachineConfig::default()
        }
    }

    #[test]
    fn the_container_is_a_resource_limited_sleep_with_its_home_bound_from_this_host() {
        let args = create_args(
            "buildbridge-android-x",
            &profile(),
            Path::new("/tmp/buildbridge/x/home"),
        );

        assert_eq!(args[0], "create");
        assert!(args.contains(&ANDROID_PLATFORM_ARG.to_string()));
        assert!(args.contains(&"--init".to_string()));
        assert!(args.contains(&"--memory=6g".to_string()));
        assert!(args.contains(&"--cpus=3".to_string()));
        assert!(args.contains(&"--volume=/tmp/buildbridge/x/home:/root:rw".to_string()));
        assert!(args.contains(&"--entrypoint=/bin/sleep".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("infinity"));
        assert_eq!(args[args.len() - 2], ANDROID_IMAGE);
        assert!(!args.iter().any(|arg| arg.contains("--privileged")));
        assert!(!args.iter().any(|arg| arg.starts_with("--publish")));
        assert!(!args.iter().any(|arg| arg.starts_with("--device")));
    }

    #[test]
    fn every_android_container_and_pull_uses_the_pinned_amd64_platform() {
        for args in [
            create_args("buildbridge-android-x", &profile(), Path::new("/tmp/home")),
            image_pull_args(),
            cleanup_args(Path::new(
                "/Users/Builder/Library/Application Support/buildbridge/home",
            )),
            keystore_creation_args("upload", "Example team"),
        ] {
            let platform = args
                .iter()
                .position(|arg| arg == ANDROID_PLATFORM_ARG)
                .unwrap();
            let image = args.iter().position(|arg| arg == ANDROID_IMAGE).unwrap();
            assert!(
                platform < image,
                "Docker must parse the platform flag, not pass it to the image"
            );
        }
    }

    #[test]
    fn existing_arm_containers_are_rejected_with_a_recreation_route() {
        assert!(validate_container_platform("linux/amd64\n").is_ok());
        let error = validate_container_platform("linux/arm64")
            .unwrap_err()
            .to_string();
        assert!(error.contains("linux/arm64"));
        assert!(error.contains("discard its container"));
        assert!(error.contains("keep its project records and signing credentials"));
        assert!(validate_container_platform("windows/amd64").is_err());
    }

    #[test]
    fn emulation_errors_explain_apple_silicon_without_misclassifying_other_failures() {
        let message = android_execution_message("exec /bin/sleep: exec format error");
        assert!(message.contains("exec /bin/sleep: exec format error"));
        assert!(message.contains("Apple Silicon"));
        assert!(message.contains("Rosetta"));
        assert_eq!(
            android_execution_message("network timeout"),
            "network timeout"
        );
    }

    #[test]
    fn mac_home_paths_and_keystore_values_stay_single_arguments() {
        let home = Path::new("/Users/Builder/Library/Application Support/buildbridge/home");
        let args = cleanup_args(home);
        assert!(args.contains(&format!(
            "--volume={}:/buildbridge-storage:rw",
            home.display()
        )));
        let args = keystore_creation_args("upload", "Example team");
        assert_eq!(&args[args.len() - 2..], ["upload", "Example team"]);
        assert_eq!(args[7], KEYSTORE_CREATION_SCRIPT);
    }

    #[test]
    fn incompatible_architecture_is_rejected_before_downloading_any_tools() {
        let script = android_tools_preparation(&android_toolchain("/root"), true, false);
        let architecture = script.find("uname -m").unwrap();
        let first_download = script.find("/usr/bin/curl").unwrap();
        assert!(architecture < first_download);
        assert!(script.contains("!= x86_64"));
    }

    #[test]
    fn the_toolchain_lives_under_the_home_and_leads_the_path() {
        let toolchain = android_toolchain("/root");
        assert_eq!(toolchain.tools, "/root/.buildbridge/tools");
        assert_eq!(toolchain.sdk, "/root/android-sdk");
        assert_eq!(
            toolchain.build_tools,
            "/root/android-sdk/build-tools/35.0.0"
        );
        assert_eq!(toolchain.workspace, "/root/BuildBridge/workspaces/active");
        assert!(toolchain.path.starts_with(&format!(
            "/root/.buildbridge/tools/node-v{NODE_VERSION}-linux-x64/bin:"
        )));
        assert!(toolchain.path.contains("/opt/java/openjdk/bin"));
    }

    #[test]
    fn every_download_in_the_preparation_is_checked_against_its_pin() {
        let script = android_tools_preparation(&android_toolchain("/root"), true, false);
        assert!(script.contains(NODE_LINUX_X64_SHA256));
        assert!(
            !script.contains(GRADLE_BIN_SHA256),
            "the wrapper is the project's own"
        );
        // A project that commits no wrapper gets the pinned distribution instead.
        let supplied = android_tools_preparation(&android_toolchain("/root"), true, true);
        assert!(supplied.contains(GRADLE_BIN_SHA256));
        assert!(supplied.contains(&format!("gradle-{GRADLE_VERSION}-bin.zip")));
        // A project with no JavaScript downloads no Node and no pnpm.
        let native = android_tools_preparation(&android_toolchain("/root"), false, false);
        assert!(!native.contains(NODE_LINUX_X64_SHA256));
        assert!(!native.contains("pnpm@"));
        assert!(native.contains(CMDLINE_TOOLS_LINUX_SHA256));
        assert!(native.contains(&format!("build-tools;{BUILD_TOOLS_VERSION}")));
        assert!(script.contains(CMDLINE_TOOLS_LINUX_SHA256));
        assert!(script.contains(JDK_21_LINUX_X64_SHA256));
        assert!(script.contains("org.gradle.java.installations.paths=%s,%s"));
        assert!(script.contains(&format!(
            "commandlinetools-linux-{CMDLINE_TOOLS_VERSION}_latest.zip"
        )));
        assert!(script.contains("sha256sum --check --status"));
        assert!(script.contains("--licenses"));
        assert!(script.contains(&format!("build-tools;{BUILD_TOOLS_VERSION}")));
        assert!(
            !script.contains("unzip"),
            "the image ships no unzip; jar opens the zip"
        );
    }

    #[test]
    fn the_recipe_environment_keeps_the_gradle_daemon_off_and_names_the_sdk() {
        let layout = ProjectLayout::capacitor_default();
        let environment = android_recipe_environment(&android_toolchain("/root"));
        assert!(environment.contains("export ANDROID_HOME=\"/root/android-sdk\""));
        assert!(environment.contains("export GRADLE_USER_HOME=\"/root/.gradle\""));
        assert!(environment.contains("org.gradle.daemon=false"));
        assert!(environment.contains("export JAVA_HOME=\"/opt/java/openjdk\""));
        // The wrapper decides, and it is read where the layout says the Gradle root is.
        let jdk = android_jdk_selection(&android_toolchain("/root"), &layout);
        assert!(jdk.contains(
            "gradle_wrapper=\"/root/BuildBridge/workspaces/active/android/gradle/wrapper/gradle-wrapper.properties\""
        ));
        assert!(jdk.contains("-ge 5"));
        assert!(jdk.contains(&format!(
            "export JAVA_HOME=\"/root/.buildbridge/tools/jdk-{JDK_21_VERSION}\""
        )));
        // A project whose Gradle root is the project itself keeps its wrapper there.
        let mut root_gradle = ProjectLayout::capacitor_default();
        let android = root_gradle.android.as_mut().unwrap();
        android.root = String::new();
        assert!(
            android_jdk_selection(&android_toolchain("/root"), &root_gradle).contains(
                "gradle_wrapper=\"/root/BuildBridge/workspaces/active/gradle/wrapper/gradle-wrapper.properties\""
            )
        );
    }

    #[test]
    fn marker_lines_split_on_tabs_after_their_name() {
        assert_eq!(
            marker_fields(
                "__BUILDBRIDGE_APP__\tcom.example\t1.2.0\t7",
                "__BUILDBRIDGE_APP__"
            ),
            Some(vec!["com.example", "1.2.0", "7"])
        );
        assert_eq!(
            marker_fields("__BUILDBRIDGE_APP__", "__BUILDBRIDGE_APP__"),
            None
        );
        assert_eq!(marker_fields("something else", "__BUILDBRIDGE_APP__"), None);
    }

    #[test]
    fn identities_and_versions_are_bounded_to_what_android_allows() {
        assert!(valid_application_id("nz.co.thinksolar.app"));
        assert!(valid_application_id("com.example.app_debug"));
        assert!(!valid_application_id(""));
        assert!(!valid_application_id("com.example;rm"));
        assert!(valid_version_value("3.2.0"));
        assert!(valid_version_value("12"));
        assert!(!valid_version_value("1\n2"));
    }

    #[test]
    fn keystore_inputs_are_bounded_before_they_reach_keytool() {
        assert!(valid_key_alias("upload"));
        assert!(valid_key_alias("my-key.v2"));
        assert!(!valid_key_alias("my key"));
        assert!(!valid_key_alias("$(rm)"));
        assert!(valid_keystore_password("secret-1"));
        assert!(!valid_keystore_password("short"));
        assert!(!valid_keystore_password("two\nlines"));
        assert!(valid_certificate_name("Think Solar"));
        assert!(!valid_certificate_name("Think, Solar"));
        assert!(!valid_certificate_name("CN=x"));
    }

    #[test]
    fn build_diagnostics_recognise_gradle_pnpm_and_kotlin_failures() {
        assert!(android_build_log_is_diagnostic("* What went wrong:"));
        assert!(android_build_log_is_diagnostic(
            "Execution failed for task ':app:compileDebugKotlin'."
        ));
        assert!(android_build_log_is_diagnostic(
            "e: file.kt:3:1 Unresolved reference"
        ));
        assert!(android_build_log_is_diagnostic(
            " ERR_PNPM_OUTDATED_LOCKFILE  Cannot install"
        ));
        assert!(android_build_log_is_diagnostic(
            "FAILURE: Build failed with an exception."
        ));
        assert!(!android_build_log_is_diagnostic(
            "> Task :app:compileDebugKotlin"
        ));
        assert!(!android_build_log_is_diagnostic(
            "BUILD SUCCESSFUL in 1m 2s"
        ));
    }

    #[test]
    fn every_phase_the_recipes_print_is_known() {
        for name in [
            "preparing_tools",
            "installing_dependencies",
            "building_web_assets",
            "syncing_android",
            "building",
            "inspecting",
            "completed",
        ] {
            assert!(android_build_phase(name).is_some(), "{name}");
        }
        for name in [
            "preparing",
            "building_web_assets",
            "bundling",
            "signing",
            "verifying",
            "completed",
        ] {
            assert!(android_release_phase(name).is_some(), "{name}");
        }
        assert!(android_build_phase("resolving_pods").is_none());
    }

    #[test]
    fn the_keystore_script_takes_the_password_on_stdin_and_the_rest_as_arguments() {
        assert!(KEYSTORE_CREATION_SCRIPT.contains("/bin/cat > /tmp/buildbridge-keystore/pass"));
        assert!(KEYSTORE_CREATION_SCRIPT.contains("-alias \"$1\""));
        assert!(KEYSTORE_CREATION_SCRIPT.contains("-dname \"CN=$2\""));
        assert!(KEYSTORE_CREATION_SCRIPT.contains("-storepass:file"));
        assert!(!KEYSTORE_CREATION_SCRIPT.contains("-storepass "));
    }

    #[test]
    fn the_signing_material_never_prints_its_passwords() {
        let material = AndroidSigningMaterial {
            keystore_path: PathBuf::from("/tmp/upload.keystore"),
            keystore_password: "store-secret".to_string(),
            key_alias: "upload".to_string(),
            key_password: "key-secret".to_string(),
        };
        let printed = format!("{material:?}");
        assert!(printed.contains("upload"));
        assert!(!printed.contains("secret"));
    }

    #[test]
    fn an_approved_android_project_needs_its_package_wrapper_and_module_script() {
        let root = std::env::temp_dir().join(format!(
            "buildbridge-android-project-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let write = |relative: &str| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"").unwrap();
        };
        let mut layout = ProjectLayout::capacitor_default();
        fs::create_dir_all(&root).unwrap();
        assert!(validate_android_project(&root, &layout).is_err());
        assert!(validate_android_project(&root.join("missing"), &layout).is_err());
        write("package.json");
        write("android/gradlew");
        write("android/settings.gradle.kts");
        write("android/app/build.gradle");
        assert_eq!(
            validate_android_project(&root, &layout).unwrap(),
            fs::canonicalize(&root).unwrap()
        );
        fs::remove_file(root.join("android/gradlew")).unwrap();
        let error = validate_android_project(&root, &layout)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Gradle wrapper"), "{error}");
        assert!(!error.contains(root.to_str().unwrap()), "{error}");
        fs::create_dir(root.join("android/gradlew")).unwrap();
        assert!(
            validate_android_project(&root, &layout).is_err(),
            "a directory is not the wrapper script"
        );
        fs::remove_dir(root.join("android/gradlew")).unwrap();
        write("android/gradlew");
        // A native project keeps no package.json; an Expo project has nothing native yet.
        fs::remove_file(root.join("package.json")).unwrap();
        assert!(validate_android_project(&root, &layout).is_err());
        layout.kind = ProjectKind::Native;
        layout.package_manager = None;
        assert!(validate_android_project(&root, &layout).is_ok());
        layout.kind = ProjectKind::Expo;
        fs::remove_dir_all(root.join("android")).unwrap();
        write("package.json");
        assert!(validate_android_project(&root, &layout).is_ok());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn release_outputs_default_to_both_and_earlier_release_records_still_read() {
        assert_eq!(
            AndroidReleaseOutputs::default(),
            AndroidReleaseOutputs::Both
        );
        for (outputs, wire, aab, apk) in [
            (AndroidReleaseOutputs::Both, "\"both\"", true, true),
            (AndroidReleaseOutputs::Aab, "\"aab\"", true, false),
            (AndroidReleaseOutputs::Apk, "\"apk\"", false, true),
        ] {
            assert_eq!(serde_json::to_string(&outputs).unwrap(), wire);
            assert_eq!(
                serde_json::from_str::<AndroidReleaseOutputs>(wire).unwrap(),
                outputs
            );
            assert_eq!(outputs.includes_aab(), aab);
            assert_eq!(outputs.includes_apk(), apk);
        }
        assert!(serde_json::from_str::<AndroidReleaseOutputs>("\"Both\"").is_err());
        let both: AndroidReleaseResult = serde_json::from_str(
            r#"{"applicationId":"com.example.app","versionName":"1.0","versionCode":"7","keyAlias":"upload","certificateSha256":"ab","aab":{"path":"/a/app.aab","bytes":10,"sha256":"aa"},"apk":{"path":"/a/app.apk","bytes":5,"sha256":"bb"},"outputTail":[]}"#,
        )
        .expect("a record written before outputs were selectable still reads");
        assert_eq!(
            both.artifacts()
                .map(|artifact| artifact.path.as_str())
                .collect::<Vec<_>>(),
            ["/a/app.aab", "/a/app.apk"]
        );
        let apk_only: AndroidReleaseResult = serde_json::from_str(
            r#"{"applicationId":"com.example.app","versionName":"1.0","versionCode":"7","keyAlias":"upload","certificateSha256":"ab","aab":null,"apk":{"path":"/a/app.apk","bytes":5,"sha256":"bb"},"outputTail":[]}"#,
        )
        .unwrap();
        assert_eq!(
            apk_only
                .artifacts()
                .map(|artifact| artifact.bytes)
                .sum::<u64>(),
            5
        );
        assert_eq!(
            serde_json::to_value(&apk_only).unwrap()["aab"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn container_exec_keeps_sh_as_argument_zero_so_user_values_bind_to_positionals() {
        let script = "printf '%s|%s' \"$1\" \"$2\"";
        let command =
            container_exec_command("buildbridge-android", script, &["/jobs root", "$(evil)"]);
        let args = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "exec",
                "--interactive",
                "buildbridge-android",
                "/bin/sh",
                "-c",
                script,
                "sh",
                "/jobs root",
                "$(evil)"
            ]
        );
        let output = Command::new("/bin/sh").args(&args[4..]).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "/jobs root|$(evil)"
        );
        assert_eq!(
            image_inspect_args("img@sha256:abc"),
            [
                "image",
                "inspect",
                "--format={{.Os}}/{{.Architecture}}",
                "img@sha256:abc"
            ]
        );
    }
}
