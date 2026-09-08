//! What kind of app a folder holds, found from its files on the host before anything is copied
//! anywhere. Every mobile project ends in the same two things — an Xcode workspace or project
//! with a scheme, and a Gradle project with an application module — and differs only in what
//! stands in front of them: Capacitor and Cordova build web assets and copy them in, React
//! Native and Expo bundle JavaScript inside the native build, Flutter generates the native
//! configuration from `pubspec.yaml`, and a native app has nothing in front at all. The
//! layout found here names those two things and the framework in front, and the recipes read
//! it instead of assuming `ios/App/App.xcworkspace` and `android/app`.
//!
//! Detection reads files and never runs the project's own tools: a folder is approved on the
//! host, which may have no Xcode, no Gradle and no Node, and the approval must not execute
//! anything the folder contains.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The framework in front of the native projects, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectKind {
    /// Web assets built by the package's `build` script, copied into the native projects by
    /// `cap sync`.
    Capacitor,
    /// Web assets copied into `platforms/` by `cordova prepare`.
    Cordova,
    /// JavaScript bundled by the native builds themselves; CocoaPods on iOS.
    ReactNative,
    /// React Native whose native projects `expo prebuild` writes when they are not committed.
    Expo,
    /// Dart compiled by the Flutter tool, which also generates the native projects' settings.
    Flutter,
    /// A Laravel application bundled with a statically compiled PHP inside a Swift or Kotlin
    /// shell, which `php artisan native:install` writes into `nativephp/`.
    NativePhp,
    /// An Xcode or Gradle project with nothing in front of it.
    Native,
}

impl ProjectKind {
    pub fn label(self) -> &'static str {
        crate::frameworks::framework(self).label
    }

    /// Whether a build produces web assets that an environment changes, so that choosing an
    /// environment for a build means rebuilding them.
    pub fn has_web_assets(self) -> bool {
        crate::frameworks::framework(self).web_assets
    }

    /// Whether the project's dependencies are installed with a JavaScript package manager.
    pub fn uses_javascript(self) -> bool {
        crate::frameworks::framework(self).javascript
    }

    /// Whether the framework writes the native projects where the build runs, so the folder on
    /// this host need not carry them.
    pub fn generates_native_projects(self) -> bool {
        crate::frameworks::framework(self).predict.is_some()
    }
}

/// The JavaScript package manager a project locks its dependencies with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum PackageManager {
    Pnpm,
    Npm,
    Yarn,
    Bun,
}

impl PackageManager {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pnpm => "pnpm",
            Self::Npm => "npm",
            Self::Yarn => "Yarn",
            Self::Bun => "Bun",
        }
    }
}

/// Whether Xcode is handed a workspace or a bare project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum XcodeContainerKind {
    Workspace,
    Project,
}

/// An application target of the Xcode project, by the name a scheme is generated for and the
/// identifier that scheme refers to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct XcodeTarget {
    pub name: String,
    pub identifier: String,
    /// The product file a scheme refers to: `App.app`.
    pub product: String,
}

/// The iOS side of a project: what Xcode opens, which scheme it builds, and where CocoaPods
/// runs first. Every path is relative to the project folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct IosProject {
    /// `ios/App/App.xcworkspace`, or `MyApp.xcodeproj` when there is no workspace. A
    /// workspace that CocoaPods writes may not exist on the host yet.
    pub container: String,
    pub container_kind: XcodeContainerKind,
    /// The `.xcodeproj` the scheme and the version live in.
    pub project: String,
    /// The scheme the builds use: chosen, or the application target's.
    pub scheme: String,
    /// The shared schemes the project carries, for choosing among.
    pub schemes: Vec<String>,
    /// The application targets, so a scheme can be generated for one that has none.
    pub app_targets: Vec<XcodeTarget>,
    /// The directory holding the Podfile, when the project uses CocoaPods.
    pub podfile_dir: Option<String>,
    /// Whether that Podfile has a committed lock, which a build must then leave unchanged.
    pub podfile_locked: bool,
}

impl IosProject {
    /// The `project.pbxproj` inside the project.
    pub fn project_file(&self) -> String {
        format!("{}/project.pbxproj", self.project)
    }

    /// The directory the container lives in, where `xcodebuild` runs.
    pub fn container_dir(&self) -> String {
        parent_of(&self.container)
    }

    /// Whether the scheme is one of the project's own, or one buildbridge writes for a target.
    pub fn scheme_is_shared(&self) -> bool {
        self.schemes.iter().any(|scheme| scheme == &self.scheme)
    }

    /// The target the chosen scheme was generated for, when it is not a shared scheme.
    pub fn generated_scheme_target(&self) -> Option<&XcodeTarget> {
        if self.scheme_is_shared() {
            return None;
        }
        self.app_targets
            .iter()
            .find(|target| target.name == self.scheme)
    }
}

/// The Android side of a project: the Gradle root and the application module in it. Every
/// path is relative to the project folder except `module_dir`, which is relative to the root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AndroidProject {
    /// The directory holding `settings.gradle` and the wrapper: `android`, or empty for the
    /// project folder itself.
    pub root: String,
    /// The application module's directory under the root: `app`, or empty for the root
    /// project.
    pub module_dir: String,
    /// The module's Gradle path: `:app`.
    pub module_path: String,
    /// Every application module's Gradle path, for choosing among.
    pub modules: Vec<String>,
    /// The module's build script, relative to the project folder.
    pub script: String,
    /// Whether the root carries `gradlew`.
    pub wrapper: bool,
}

impl AndroidProject {
    /// The module's directory relative to the project folder.
    pub fn module_relative(&self) -> String {
        join_relative(&self.root, &self.module_dir)
    }

    /// The Gradle tasks that build the module alone, prefixed by its path.
    pub fn task(&self, task: &str) -> String {
        if self.module_path == ":" {
            format!(":{task}")
        } else {
            format!("{}:{task}", self.module_path)
        }
    }
}

/// What a folder holds, for the recipes to build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLayout {
    pub kind: ProjectKind,
    /// The project's own name: from its package, its pubspec, its Xcode project, or its folder.
    pub name: String,
    pub package_manager: Option<PackageManager>,
    pub ios: Option<IosProject>,
    pub android: Option<AndroidProject>,
}

impl ProjectLayout {
    /// The layout every project had before detection existed: a Capacitor project with pnpm,
    /// its iOS platform at `ios/App`, its Android platform at `android`.
    pub fn capacitor_default() -> Self {
        Self {
            kind: ProjectKind::Capacitor,
            name: "app".to_string(),
            package_manager: Some(PackageManager::Pnpm),
            ios: Some(IosProject {
                container: "ios/App/App.xcworkspace".to_string(),
                container_kind: XcodeContainerKind::Workspace,
                project: "ios/App/App.xcodeproj".to_string(),
                scheme: "App".to_string(),
                schemes: vec!["App".to_string()],
                app_targets: Vec::new(),
                podfile_dir: Some("ios/App".to_string()),
                podfile_locked: true,
            }),
            android: Some(AndroidProject {
                root: "android".to_string(),
                module_dir: "app".to_string(),
                module_path: ":app".to_string(),
                modules: vec![":app".to_string()],
                script: "android/app/build.gradle".to_string(),
                wrapper: true,
            }),
        }
    }

    /// One line naming the layout, for the command line and the log.
    pub fn describe(&self) -> String {
        let mut parts = vec![self.kind.label().to_string()];
        if let Some(manager) = self.package_manager {
            parts.push(manager.label().to_string());
        }
        if let Some(ios) = &self.ios {
            parts.push(format!("{} · scheme {}", ios.container, ios.scheme));
        }
        if let Some(android) = &self.android {
            parts.push(format!(
                "{} · module {}",
                if android.root.is_empty() {
                    "Gradle at the root"
                } else {
                    &android.root
                },
                android.module_path
            ));
        }
        parts.join(" · ")
    }
}

/// What the person chose where the project offers more than one: a scheme, an application
/// module. Names that match nothing are refused rather than guessed around.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProjectChoices<'a> {
    pub scheme: Option<&'a str>,
    pub module: Option<&'a str>,
}

/// Reads the folder and names what it holds. A folder with neither an Xcode nor a Gradle
/// project, and no framework that would write them, is refused with what was looked for.
pub fn detect_project(root: &Path, choices: ProjectChoices<'_>) -> Result<ProjectLayout, String> {
    if !root.is_dir() {
        return Err("The selected project path is not a directory.".to_string());
    }
    let package = read_package(root)?;
    let dependencies = package.as_ref().map(dependency_names).unwrap_or_default();
    let pubspec = fs::read_to_string(root.join("pubspec.yaml")).ok();
    let composer = read_json(root, "composer.json")?;
    let kind = crate::frameworks::detect_kind(&crate::frameworks::ProjectMarkers {
        root,
        package: package.as_ref(),
        dependencies: &dependencies,
        composer: composer.as_ref(),
        pubspec: pubspec.as_deref(),
    });
    let package_manager = package
        .as_ref()
        .filter(|_| kind.uses_javascript())
        .map(|package| detect_package_manager(root, package));

    let mut ios = detect_ios(root, kind, choices.scheme)?;
    let mut android = detect_android(root, kind, choices.module)?;
    // A framework that writes the native projects where the build runs names them here, so a
    // folder that does not carry them yet is still approvable.
    if let Some(predict) = crate::frameworks::framework(kind).predict {
        let markers = crate::frameworks::ProjectMarkers {
            root,
            package: package.as_ref(),
            dependencies: &dependencies,
            composer: composer.as_ref(),
            pubspec: pubspec.as_deref(),
        };
        let (predicted_ios, predicted_android) = predict(&markers, choices)?;
        ios = ios.or(predicted_ios);
        android = android.or(predicted_android);
    }
    if ios.is_none() && android.is_none() {
        return Err(crate::frameworks::framework(kind)
            .missing_native
            .to_string());
    }

    let name = package
        .as_ref()
        .and_then(|package| package.get("name"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| pubspec.as_deref().and_then(yaml_top_level_value_of("name")))
        .or_else(|| {
            composer
                .as_ref()
                .and_then(|composer| composer.get("name"))
                .and_then(serde_json::Value::as_str)
                .map(|name| name.rsplit('/').next().unwrap_or(name).to_string())
        })
        .or_else(|| {
            ios.as_ref()
                .map(|ios| file_stem(&ios.project))
                .filter(|name| !name.is_empty())
        })
        .or_else(|| {
            android
                .as_ref()
                .and_then(|android| gradle_root_project_name(root, &android.root))
        })
        .or_else(|| {
            root.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty() && name.len() <= 120)
        .ok_or_else(|| "The selected project name is invalid.".to_string())?;

    Ok(ProjectLayout {
        kind,
        name,
        package_manager,
        ios,
        android,
    })
}

/// The `expo` object of `app.json`, when the project keeps its Expo config there rather than
/// in code buildbridge does not run.
pub fn expo_app_config(root: &Path) -> Option<serde_json::Value> {
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("app.json")).ok()?).ok()?;
    config
        .get("expo")
        .cloned()
        .filter(serde_json::Value::is_object)
}

/// The name `expo prebuild` gives the native projects: the app's display name with
/// everything but letters and digits removed, and no leading digits.
fn expo_project_name(root: &Path, package: Option<&serde_json::Value>) -> String {
    let display = expo_app_config(root)
        .and_then(|expo| {
            expo.get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            package
                .and_then(|package| package.get("name"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    let sanitized: String = display
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .to_string();
    if sanitized.is_empty() {
        "App".to_string()
    } else {
        sanitized
    }
}

/// What `expo prebuild` will write, named from the app's own name before it exists.
pub(crate) fn expo_predicted_projects(
    markers: &crate::frameworks::ProjectMarkers<'_>,
    choices: ProjectChoices<'_>,
) -> Result<(Option<IosProject>, Option<AndroidProject>), String> {
    let name = expo_project_name(markers.root, markers.package);
    Ok((
        Some(expo_predicted_ios(&name, choices.scheme)?),
        Some(expo_predicted_android(choices.module)?),
    ))
}

fn expo_predicted_ios(name: &str, chosen_scheme: Option<&str>) -> Result<IosProject, String> {
    if let Some(chosen) = chosen_scheme
        .map(str::trim)
        .filter(|scheme| !scheme.is_empty())
        && chosen != name
    {
        return Err(format!(
            "Expo writes the iOS project as {name}, so its scheme is {name}; there is no scheme {chosen} to choose."
        ));
    }
    Ok(IosProject {
        container: format!("ios/{name}.xcworkspace"),
        container_kind: XcodeContainerKind::Workspace,
        project: format!("ios/{name}.xcodeproj"),
        scheme: name.to_string(),
        schemes: vec![name.to_string()],
        app_targets: Vec::new(),
        podfile_dir: Some("ios".to_string()),
        podfile_locked: false,
    })
}

fn expo_predicted_android(chosen_module: Option<&str>) -> Result<AndroidProject, String> {
    if let Some(chosen) = chosen_module
        .map(str::trim)
        .filter(|module| !module.is_empty())
        && !matches!(chosen, ":app" | "app")
    {
        return Err(format!(
            "Expo writes the Android project with one application module, :app; there is no module {chosen} to choose."
        ));
    }
    Ok(AndroidProject {
        root: "android".to_string(),
        module_dir: "app".to_string(),
        module_path: ":app".to_string(),
        modules: vec![":app".to_string()],
        script: "android/app/build.gradle".to_string(),
        wrapper: true,
    })
}

fn read_package(root: &Path) -> Result<Option<serde_json::Value>, String> {
    read_json(root, "package.json")
}

/// A manifest in the project folder, parsed; absent is not an error, invalid is.
fn read_json(root: &Path, name: &str) -> Result<Option<serde_json::Value>, String> {
    match fs::read(root.join(name)) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
            .map(Some)
            .map_err(|error| format!("{name} is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Could not read {name}: {error}")),
    }
}

fn dependency_names(package: &serde_json::Value) -> BTreeSet<String> {
    ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|key| package.get(key).and_then(serde_json::Value::as_object))
        .flat_map(|dependencies| dependencies.keys().cloned())
        .collect()
}

fn detect_package_manager(root: &Path, package: &serde_json::Value) -> PackageManager {
    if let Some(declared) = package
        .get("packageManager")
        .and_then(serde_json::Value::as_str)
    {
        let name = declared.split('@').next().unwrap_or_default().trim();
        match name {
            "pnpm" => return PackageManager::Pnpm,
            "npm" => return PackageManager::Npm,
            "yarn" => return PackageManager::Yarn,
            "bun" => return PackageManager::Bun,
            _ => {}
        }
    }
    if root.join("pnpm-lock.yaml").is_file() {
        PackageManager::Pnpm
    } else if root.join("yarn.lock").is_file() {
        PackageManager::Yarn
    } else if root.join("bun.lock").is_file() || root.join("bun.lockb").is_file() {
        PackageManager::Bun
    } else {
        PackageManager::Npm
    }
}

pub(crate) fn pubspec_uses_flutter(pubspec: &str) -> bool {
    let mut in_dependencies = false;
    for line in pubspec.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with(|c: char| !c.is_whitespace()) {
            in_dependencies = trimmed.starts_with("dependencies:");
            if trimmed.starts_with("flutter:") {
                return true;
            }
            continue;
        }
        if in_dependencies && trimmed.trim_start().starts_with("flutter:") {
            return true;
        }
    }
    false
}

/// `key: value` at the top level of a small YAML file, unquoted.
fn yaml_top_level_value_of(key: &'static str) -> impl Fn(&str) -> Option<String> {
    move |text: &str| {
        text.lines().find_map(|line| {
            let value = line.strip_prefix(key)?.strip_prefix(':')?.trim();
            let value = value.trim_matches(|c| c == '"' || c == '\'').trim();
            (!value.is_empty()).then(|| value.to_string())
        })
    }
}

fn cordova_config(root: &Path) -> Option<String> {
    let config = fs::read_to_string(root.join("config.xml")).ok()?;
    config.contains("<widget").then_some(config)
}

/// The application identifier a Cordova project declares on its `<widget>`, which its Gradle
/// script computes rather than writes, so the literal scan cannot find it.
pub fn cordova_widget_id(root: &Path) -> Option<String> {
    let config = cordova_config(root)?;
    let widget = config.split_once("<widget")?.1;
    let widget = widget.split_once('>')?.0;
    let rest = widget.split_once("id=")?.1.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let value = rest[1..].split(quote).next()?;
    let valid = !value.is_empty()
        && value.len() <= 255
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_'));
    valid.then(|| value.to_string())
}

fn detect_ios(
    root: &Path,
    kind: ProjectKind,
    chosen_scheme: Option<&str>,
) -> Result<Option<IosProject>, String> {
    let candidates = search_paths(
        root,
        crate::frameworks::framework(kind).ios_dirs,
        &["iosApp"],
    );
    let Some((directory, found)) = candidates
        .into_iter()
        .filter(|directory| directory.is_dir())
        .find_map(|directory| find_xcode_container(&directory).map(|found| (directory, found)))
    else {
        return Ok(None);
    };
    let relative = |path: &Path| -> String {
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let project_path = directory.join(&found.project);
    let project_file = fs::read_to_string(project_path.join("project.pbxproj"))
        .map_err(|error| format!("Could not read the Xcode project settings: {error}"))?;
    let app_targets = xcode_app_targets(&project_file);
    let mut schemes = shared_schemes(&project_path.join("xcshareddata/xcschemes"));
    if found.container_kind == XcodeContainerKind::Workspace {
        for scheme in shared_schemes(
            &directory
                .join(&found.container)
                .join("xcshareddata/xcschemes"),
        ) {
            if !schemes.contains(&scheme) {
                schemes.push(scheme);
            }
        }
    }
    schemes.sort();
    let scheme = match chosen_scheme
        .map(str::trim)
        .filter(|scheme| !scheme.is_empty())
    {
        Some(chosen) => {
            if !valid_scheme_name(chosen) {
                return Err("The scheme name may hold letters, digits, periods, hyphens, underscores and spaces.".to_string());
            }
            if !schemes.iter().any(|scheme| scheme == chosen)
                && !app_targets.iter().any(|target| target.name == chosen)
            {
                let known = schemes
                    .iter()
                    .cloned()
                    .chain(app_targets.iter().map(|target| target.name.clone()))
                    .collect::<BTreeSet<_>>();
                return Err(format!(
                    "This project has no shared scheme or application target named {chosen}. It offers: {}.",
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.into_iter().collect::<Vec<_>>().join(", ")
                    }
                ));
            }
            chosen.to_string()
        }
        None => schemes
            .iter()
            .find(|scheme| app_targets.iter().any(|target| &target.name == *scheme))
            .cloned()
            .or_else(|| {
                app_targets
                    .first()
                    .map(|target| target.name.clone())
                    .filter(|name| valid_scheme_name(name))
            })
            .or_else(|| schemes.first().cloned())
            .ok_or_else(|| {
                format!(
                    "{} has no application target and no shared scheme to build.",
                    relative(&project_path)
                )
            })?,
    };
    let podfile_dir = directory
        .join("Podfile")
        .is_file()
        .then(|| relative(&directory));
    let podfile_locked = directory.join("Podfile.lock").is_file();

    Ok(Some(IosProject {
        container: relative(&directory.join(&found.container)),
        container_kind: found.container_kind,
        project: relative(&project_path),
        scheme,
        schemes,
        app_targets,
        podfile_dir,
        podfile_locked,
    }))
}

struct FoundContainer {
    /// Relative to the directory searched.
    container: String,
    container_kind: XcodeContainerKind,
    /// The `.xcodeproj`, relative to the directory searched.
    project: String,
}

/// The workspace or project in one directory, preferring a workspace, and the workspace
/// CocoaPods will write when a Podfile sits beside a bare project.
fn find_xcode_container(directory: &Path) -> Option<FoundContainer> {
    let mut workspaces = Vec::new();
    let mut projects = Vec::new();
    for entry in fs::read_dir(directory).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !entry.path().is_dir() {
            continue;
        }
        if name.ends_with(".xcworkspace") {
            workspaces.push(name);
        } else if name.ends_with(".xcodeproj") && name != "Pods.xcodeproj" {
            projects.push(name);
        }
    }
    workspaces.sort();
    projects.sort();
    if projects.is_empty() && workspaces.is_empty() {
        return None;
    }
    if !workspaces.is_empty() {
        // The workspace whose name matches a project is the app's; Pods and stray ones lose.
        let workspace = workspaces
            .iter()
            .find(|workspace| {
                projects
                    .iter()
                    .any(|project| file_stem(project) == file_stem(workspace))
            })
            .or_else(|| workspaces.first())
            .cloned()?;
        let referenced = workspace_projects(&directory.join(&workspace));
        let project = referenced
            .into_iter()
            .find(|candidate| projects.contains(candidate))
            .or_else(|| {
                projects
                    .iter()
                    .find(|project| file_stem(project) == file_stem(&workspace))
                    .cloned()
            })
            .or_else(|| projects.first().cloned())?;
        return Some(FoundContainer {
            container: workspace,
            container_kind: XcodeContainerKind::Workspace,
            project,
        });
    }
    let podfile = fs::read_to_string(directory.join("Podfile")).ok();
    let project = podfile
        .as_deref()
        .and_then(|podfile| podfile_setting(podfile, "project"))
        .map(|name| {
            if name.ends_with(".xcodeproj") {
                name
            } else {
                format!("{name}.xcodeproj")
            }
        })
        .filter(|name| projects.contains(name))
        .or_else(|| projects.first().cloned())?;
    match podfile {
        Some(podfile) => {
            let workspace = podfile_setting(&podfile, "workspace")
                .map(|name| {
                    if name.ends_with(".xcworkspace") {
                        name
                    } else {
                        format!("{name}.xcworkspace")
                    }
                })
                .unwrap_or_else(|| format!("{}.xcworkspace", file_stem(&project)));
            Some(FoundContainer {
                container: workspace,
                container_kind: XcodeContainerKind::Workspace,
                project,
            })
        }
        None => Some(FoundContainer {
            container: project.clone(),
            container_kind: XcodeContainerKind::Project,
            project,
        }),
    }
}

/// The `.xcodeproj` entries a workspace's `contents.xcworkspacedata` refers to, relative to the
/// workspace's directory, in the order they appear.
fn workspace_projects(workspace: &Path) -> Vec<String> {
    let Ok(contents) = fs::read_to_string(workspace.join("contents.xcworkspacedata")) else {
        return Vec::new();
    };
    let mut projects = Vec::new();
    let mut rest = contents.as_str();
    while let Some(start) = rest.find("location") {
        rest = &rest[start + "location".len()..];
        let Some(quote_start) = rest.find('"') else {
            break;
        };
        let value = &rest[quote_start + 1..];
        let Some(quote_end) = value.find('"') else {
            break;
        };
        let location = &value[..quote_end];
        rest = &value[quote_end + 1..];
        let path = location
            .split_once(':')
            .map(|(_, path)| path)
            .unwrap_or(location);
        if path.ends_with(".xcodeproj") && !path.contains('/') {
            projects.push(path.to_string());
        }
    }
    projects
}

/// `workspace 'Name'` or `project 'Name'` in a Podfile, unquoted.
fn podfile_setting(podfile: &str, key: &str) -> Option<String> {
    podfile.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix(key)?;
        let rest = rest.trim_start();
        let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let value = rest[1..].split(quote).next()?;
        (!value.is_empty() && !value.contains('/')).then(|| value.to_string())
    })
}

fn shared_schemes(directory: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut schemes: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let scheme = name.strip_suffix(".xcscheme")?;
            valid_scheme_name(scheme).then(|| scheme.to_string())
        })
        .collect();
    schemes.sort();
    schemes
}

/// The application targets in a `project.pbxproj`: each `PBXNativeTarget` whose product type
/// is an application, with the identifier a scheme names it by.
pub fn xcode_app_targets(project: &str) -> Vec<XcodeTarget> {
    let Some(start) = project.find("/* Begin PBXNativeTarget section */") else {
        return Vec::new();
    };
    let section = &project[start..];
    let section = section
        .find("/* End PBXNativeTarget section */")
        .map(|end| &section[..end])
        .unwrap_or(section);
    let mut targets = Vec::new();
    struct Partial {
        identifier: String,
        name: Option<String>,
        product: Option<String>,
        is_application: bool,
    }
    let mut current: Option<Partial> = None;
    for line in section.lines() {
        let trimmed = line.trim();
        let Some(partial) = current.as_mut() else {
            if let Some((identifier, _)) = trimmed.split_once(' ')
                && trimmed.ends_with("= {")
                && identifier.len() == 24
                && identifier.chars().all(|c| c.is_ascii_hexdigit())
            {
                current = Some(Partial {
                    identifier: identifier.to_string(),
                    name: None,
                    product: None,
                    is_application: false,
                });
            }
            continue;
        };
        if trimmed == "};" {
            if partial.is_application
                && let Some(name) = partial.name.take()
            {
                targets.push(XcodeTarget {
                    product: partial
                        .product
                        .take()
                        .unwrap_or_else(|| format!("{name}.app")),
                    name,
                    identifier: partial.identifier.clone(),
                });
            }
            current = None;
            continue;
        }
        if let Some(value) = trimmed
            .strip_prefix("name = ")
            .and_then(|v| v.strip_suffix(';'))
        {
            let value = value.trim().trim_matches('"').to_string();
            if valid_scheme_name(&value) {
                partial.name = Some(value);
            }
        } else if let Some(value) = trimmed
            .strip_prefix("productType = ")
            .and_then(|v| v.strip_suffix(';'))
        {
            partial.is_application =
                value.trim().trim_matches('"') == "com.apple.product-type.application";
        } else if let Some(rest) = trimmed.strip_prefix("productReference = ")
            && let Some((_, comment)) = rest.split_once("/* ")
            && let Some((product, _)) = comment.split_once(" */")
            && product.ends_with(".app")
            && product.len() <= 160
        {
            partial.product = Some(product.to_string());
        }
    }
    targets
}

pub(crate) fn valid_scheme_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with(' ')
        && !value.ends_with(' ')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
}

fn detect_android(
    root: &Path,
    kind: ProjectKind,
    chosen_module: Option<&str>,
) -> Result<Option<AndroidProject>, String> {
    let candidates = search_paths(root, crate::frameworks::framework(kind).android_dirs, &[]);
    let Some(gradle_root) = candidates.into_iter().find(|directory| {
        directory.join("settings.gradle").is_file()
            || directory.join("settings.gradle.kts").is_file()
    }) else {
        return Ok(None);
    };
    let settings = ["settings.gradle", "settings.gradle.kts"]
        .iter()
        .map(|name| gradle_root.join(name))
        .find(|path| path.is_file())
        .and_then(|path| fs::read_to_string(path).ok())
        .unwrap_or_default();
    let mut included = gradle_included_modules(&settings);
    if included.is_empty() {
        included.push(":".to_string());
    }
    let with_scripts: Vec<(String, PathBuf)> = included
        .iter()
        .filter_map(|module_path| {
            let directory = gradle_root.join(module_dir_of(module_path));
            ["build.gradle", "build.gradle.kts"]
                .iter()
                .map(|name| directory.join(name))
                .find(|path| path.is_file())
                .map(|script| (module_path.clone(), script))
        })
        .collect();
    let mut application_modules: Vec<String> = with_scripts
        .iter()
        .filter(|(_, script)| {
            fs::read_to_string(script).is_ok_and(|script| gradle_script_is_application(&script))
        })
        .map(|(module_path, _)| module_path.clone())
        .collect();
    if application_modules.is_empty() {
        // A build script that reaches its plugins another way: the module called app is
        // the app by Android Studio's convention.
        application_modules.extend(
            with_scripts
                .iter()
                .filter(|(module_path, _)| module_path == ":app")
                .map(|(module_path, _)| module_path.clone()),
        );
    }
    let relative = |path: &Path| -> String {
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let module_path = match chosen_module
        .map(str::trim)
        .filter(|module| !module.is_empty())
    {
        Some(chosen) => {
            let normalized = if chosen.starts_with(':') {
                chosen.to_string()
            } else {
                format!(":{}", chosen.replace('/', ":"))
            };
            if !application_modules.contains(&normalized) {
                return Err(format!(
                    "This project has no application module {chosen}. It offers: {}.",
                    if application_modules.is_empty() {
                        "none".to_string()
                    } else {
                        application_modules.join(", ")
                    }
                ));
            }
            normalized
        }
        None => application_modules
            .iter()
            .find(|module| *module == ":app")
            .or_else(|| application_modules.first())
            .cloned()
            .ok_or_else(|| {
                format!(
                    "{} has no module that applies the Android application plugin.",
                    relative(&gradle_root.join(
                        if gradle_root.join("settings.gradle.kts").is_file() {
                            "settings.gradle.kts"
                        } else {
                            "settings.gradle"
                        }
                    ))
                )
            })?,
    };
    let script = with_scripts
        .iter()
        .find(|(path, _)| path == &module_path)
        .map(|(_, script)| relative(script))
        .expect("an application module has a script");

    Ok(Some(AndroidProject {
        root: relative(&gradle_root),
        module_dir: module_dir_of(&module_path),
        module_path,
        modules: application_modules,
        script,
        wrapper: gradle_root.join("gradlew").is_file(),
    }))
}

/// The Gradle paths `include` lines name, in either dialect: `include ':app'`,
/// `include(":app", ":lib")`, one or several per line.
pub fn gradle_included_modules(settings: &str) -> Vec<String> {
    let mut modules = Vec::new();
    for line in settings.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("include") else {
            continue;
        };
        if rest.starts_with("Build") {
            continue;
        }
        let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '(');
        let mut rest = rest;
        while let Some(quote_start) = rest.find(['\'', '"']) {
            let quote = rest.as_bytes()[quote_start] as char;
            let value = &rest[quote_start + 1..];
            let Some(quote_end) = value.find(quote) else {
                break;
            };
            let module = &value[..quote_end];
            let module = if module.starts_with(':') {
                module.to_string()
            } else {
                format!(":{module}")
            };
            if module.len() > 1
                && module
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_' | '.'))
                && !modules.contains(&module)
            {
                modules.push(module);
            }
            rest = &value[quote_end + 1..];
        }
    }
    modules
}

fn module_dir_of(module_path: &str) -> String {
    module_path.trim_start_matches(':').replace(':', "/")
}

/// Whether a module's build script applies the Android application plugin, by its id or by a
/// version-catalog alias that names it.
pub fn gradle_script_is_application(script: &str) -> bool {
    script.lines().any(|line| {
        let line = line.trim();
        if line.starts_with("//") {
            return false;
        }
        line.contains("com.android.application")
            || (line.contains("alias(")
                && line.contains("plugins")
                && line.to_ascii_lowercase().contains("android")
                && line.to_ascii_lowercase().contains("application"))
    })
}

fn gradle_root_project_name(root: &Path, gradle_root: &str) -> Option<String> {
    let directory = root.join(gradle_root);
    let settings = ["settings.gradle", "settings.gradle.kts"]
        .iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file())
        .and_then(|path| fs::read_to_string(path).ok())?;
    settings.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix("rootProject.name")?.trim_start();
        let rest = rest.strip_prefix('=')?.trim_start();
        let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let value = rest[1..].split(quote).next()?;
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Where a platform's project is looked for: the framework's own folders when it names any,
/// and otherwise the project folder, the conventional names, and one level down, since a
/// native checkout keeps its projects wherever it likes.
fn search_paths(root: &Path, named: &[&str], extra: &[&str]) -> Vec<PathBuf> {
    if !named.is_empty() {
        return named.iter().map(|name| root.join(name)).collect();
    }
    let mut candidates = vec![root.to_path_buf(), root.join("ios"), root.join("android")];
    candidates.extend(extra.iter().map(|name| root.join(name)));
    candidates.extend(child_directories(root));
    candidates
}

fn child_directories(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut directories: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            entry.path().is_dir()
                && !name.starts_with('.')
                && !matches!(
                    name.as_str(),
                    "node_modules" | "Pods" | "build" | "dist" | "DerivedData" | "vendor"
                )
                && !name.ends_with(".xcodeproj")
                && !name.ends_with(".xcworkspace")
        })
        .map(|entry| entry.path())
        .collect();
    directories.sort();
    directories
}

fn file_stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn parent_of(relative: &str) -> String {
    Path::new(relative)
        .parent()
        .map(|parent| parent.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

/// Joins two relative paths, either of which may be empty.
pub fn join_relative(base: &str, tail: &str) -> String {
    match (base.is_empty(), tail.is_empty()) {
        (true, _) => tail.to_string(),
        (_, true) => base.to_string(),
        _ => format!("{base}/{tail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "buildbridge-project-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }

        fn dir(&self, relative: &str) {
            fs::create_dir_all(self.0.join(relative)).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const PBXPROJ: &str = r#"// !$*UTF8*$!
{
	objects = {
/* Begin PBXNativeTarget section */
		504EC2FC1FED79650016851F /* App */ = {
			isa = PBXNativeTarget;
			buildPhases = (
				504EC2F91FED79650016851F /* Sources */,
			);
			name = App;
			productName = App;
			productType = "com.apple.product-type.application";
		};
		504EC2FC1FED79650016851A /* AppTests */ = {
			isa = PBXNativeTarget;
			name = AppTests;
			productType = "com.apple.product-type.bundle.unit-test";
		};
/* End PBXNativeTarget section */
	};
}
"#;

    fn xcode_project(fixture: &Fixture, directory: &str, name: &str, shared_scheme: bool) {
        fixture.write(
            &join_relative(directory, &format!("{name}.xcodeproj/project.pbxproj")),
            &PBXPROJ.replace("App", name),
        );
        if shared_scheme {
            fixture.write(
                &join_relative(
                    directory,
                    &format!("{name}.xcodeproj/xcshareddata/xcschemes/{name}.xcscheme"),
                ),
                "<Scheme/>",
            );
        }
    }

    fn gradle_project(fixture: &Fixture, root: &str, modules: &[(&str, bool)]) {
        let includes = modules
            .iter()
            .map(|(module, _)| format!("include(\":{module}\")"))
            .collect::<Vec<_>>()
            .join("\n");
        fixture.write(
            &join_relative(root, "settings.gradle.kts"),
            &format!("rootProject.name = \"Sample\"\n{includes}\n"),
        );
        fixture.write(&join_relative(root, "gradlew"), "#!/bin/sh\n");
        for (module, application) in modules {
            let plugin = if *application {
                "plugins { id(\"com.android.application\") }\nandroid { defaultConfig { applicationId = \"com.example.app\" } }\n"
            } else {
                "plugins { id(\"com.android.library\") }\n"
            };
            fixture.write(
                &join_relative(
                    root,
                    &format!("{}/build.gradle.kts", module.replace(':', "/")),
                ),
                plugin,
            );
        }
    }

    #[test]
    fn a_capacitor_project_keeps_its_familiar_layout() {
        let fixture = Fixture::new();
        fixture.write(
            "package.json",
            r#"{"name":"my-app","dependencies":{"@capacitor/core":"8.0.0"}}"#,
        );
        fixture.write("pnpm-lock.yaml", "");
        fixture.write("capacitor.config.ts", "export default {}");
        fixture.write("ios/App/Podfile", "platform :ios, '14.0'\n");
        fixture.write("ios/App/Podfile.lock", "PODS:\n");
        fixture.dir("ios/App/App.xcworkspace");
        fixture.write(
            "ios/App/App.xcworkspace/contents.xcworkspacedata",
            r#"<Workspace><FileRef location = "group:App.xcodeproj"></FileRef><FileRef location = "group:Pods/Pods.xcodeproj"></FileRef></Workspace>"#,
        );
        xcode_project(&fixture, "ios/App", "App", false);
        gradle_project(
            &fixture,
            "android",
            &[("app", true), ("capacitor-cordova-android-plugins", false)],
        );

        let layout = detect_project(&fixture.0, ProjectChoices::default()).unwrap();
        assert_eq!(layout.kind, ProjectKind::Capacitor);
        assert_eq!(layout.name, "my-app");
        assert_eq!(layout.package_manager, Some(PackageManager::Pnpm));
        let ios = layout.ios.unwrap();
        assert_eq!(ios.container, "ios/App/App.xcworkspace");
        assert_eq!(ios.container_kind, XcodeContainerKind::Workspace);
        assert_eq!(ios.project, "ios/App/App.xcodeproj");
        assert_eq!(ios.scheme, "App");
        assert!(ios.schemes.is_empty(), "the scheme comes from the target");
        let target = ios.generated_scheme_target().unwrap();
        assert_eq!(target.identifier, "504EC2FC1FED79650016851F");
        assert_eq!(target.product, "App.app");
        assert_eq!(ios.podfile_dir.as_deref(), Some("ios/App"));
        assert!(ios.podfile_locked);
        assert_eq!(ios.project_file(), "ios/App/App.xcodeproj/project.pbxproj");
        assert_eq!(ios.container_dir(), "ios/App");
        let android = layout.android.unwrap();
        assert_eq!(android.root, "android");
        assert_eq!(android.module_dir, "app");
        assert_eq!(android.module_path, ":app");
        assert_eq!(android.modules, vec![":app"]);
        assert_eq!(android.script, "android/app/build.gradle.kts");
        assert!(android.wrapper);
        assert_eq!(android.task("assembleDebug"), ":app:assembleDebug");
        assert_eq!(android.module_relative(), "android/app");
    }

    #[test]
    fn react_native_and_expo_are_told_apart_and_find_their_platforms() {
        let fixture = Fixture::new();
        fixture.write(
            "package.json",
            r#"{"name":"rnapp","dependencies":{"react-native":"0.76.0"}}"#,
        );
        fixture.write("yarn.lock", "");
        fixture.write(
            "ios/Podfile",
            "require_relative '../node_modules/react-native/scripts/react_native_pods'\n",
        );
        xcode_project(&fixture, "ios", "RnApp", true);
        gradle_project(&fixture, "android", &[("app", true)]);
        let layout = detect_project(&fixture.0, ProjectChoices::default()).unwrap();
        assert_eq!(layout.kind, ProjectKind::ReactNative);
        assert_eq!(layout.package_manager, Some(PackageManager::Yarn));
        let ios = layout.ios.unwrap();
        assert_eq!(
            ios.container, "ios/RnApp.xcworkspace",
            "CocoaPods writes the workspace"
        );
        assert_eq!(ios.container_kind, XcodeContainerKind::Workspace);
        assert_eq!(ios.scheme, "RnApp");
        assert!(ios.scheme_is_shared());
        assert!(!ios.podfile_locked);

        let managed = Fixture::new();
        managed.write(
            "package.json",
            r#"{"name":"expoapp","dependencies":{"expo":"52.0.0","react-native":"0.76.0"}}"#,
        );
        managed.write(
            "app.json",
            r#"{"expo":{"name":"Expo App","slug":"expoapp"}}"#,
        );
        let layout = detect_project(&managed.0, ProjectChoices::default()).unwrap();
        assert_eq!(layout.kind, ProjectKind::Expo);
        assert_eq!(layout.package_manager, Some(PackageManager::Npm));
        let ios = layout.ios.unwrap();
        assert_eq!(
            ios.container, "ios/ExpoApp.xcworkspace",
            "prebuild writes it from the name"
        );
        assert_eq!(ios.scheme, "ExpoApp");
        assert!(ios.scheme_is_shared());
        assert_eq!(ios.podfile_dir.as_deref(), Some("ios"));
        assert_eq!(layout.android.unwrap().script, "android/app/build.gradle");
        assert_eq!(
            expo_app_config(&managed.0).unwrap()["slug"],
            serde_json::json!("expoapp")
        );
        let error = detect_project(
            &managed.0,
            ProjectChoices {
                scheme: Some("Other"),
                module: None,
            },
        )
        .unwrap_err();
        assert!(error.contains("ExpoApp"), "{error}");
    }

    #[test]
    fn flutter_is_read_from_the_pubspec() {
        let fixture = Fixture::new();
        fixture.write(
            "pubspec.yaml",
            "name: flutter_app\nversion: 1.2.3+4\n\ndependencies:\n  flutter:\n    sdk: flutter\n",
        );
        fixture.write("ios/Podfile", "platform :ios, '13.0'\n");
        xcode_project(&fixture, "ios", "Runner", true);
        fixture.dir("ios/Runner.xcworkspace");
        fixture.write(
            "ios/Runner.xcworkspace/contents.xcworkspacedata",
            r#"<Workspace><FileRef location = "group:Runner.xcodeproj"></FileRef></Workspace>"#,
        );
        gradle_project(&fixture, "android", &[("app", true)]);
        let layout = detect_project(&fixture.0, ProjectChoices::default()).unwrap();
        assert_eq!(layout.kind, ProjectKind::Flutter);
        assert_eq!(layout.name, "flutter_app");
        assert_eq!(layout.package_manager, None);
        assert_eq!(layout.ios.unwrap().container, "ios/Runner.xcworkspace");
        assert_eq!(layout.android.unwrap().root, "android");
        assert!(pubspec_uses_flutter(
            "flutter:\n  uses-material-design: true\n"
        ));
        assert!(!pubspec_uses_flutter(
            "name: tool\ndependencies:\n  http: ^1.0.0\n"
        ));
    }

    #[test]
    fn a_native_xcode_project_without_a_workspace_or_a_shared_scheme_builds_its_app_target() {
        let fixture = Fixture::new();
        xcode_project(&fixture, "", "Weather", false);
        let layout = detect_project(&fixture.0, ProjectChoices::default()).unwrap();
        assert_eq!(layout.kind, ProjectKind::Native);
        assert_eq!(layout.name, "Weather");
        assert_eq!(layout.package_manager, None);
        assert!(layout.android.is_none());
        let ios = layout.ios.unwrap();
        assert_eq!(ios.container, "Weather.xcodeproj");
        assert_eq!(ios.container_kind, XcodeContainerKind::Project);
        assert_eq!(ios.container_dir(), "");
        assert_eq!(ios.scheme, "Weather");
        assert_eq!(ios.generated_scheme_target().unwrap().name, "Weather");
        assert_eq!(ios.podfile_dir, None);

        let error = detect_project(
            &fixture.0,
            ProjectChoices {
                scheme: Some("Nope"),
                module: None,
            },
        )
        .unwrap_err();
        assert!(error.contains("Weather"), "{error}");
    }

    #[test]
    fn a_native_gradle_project_offers_its_application_modules_and_takes_a_choice() {
        let fixture = Fixture::new();
        gradle_project(
            &fixture,
            "",
            &[("core", false), ("apps:phone", true), ("apps:tv", true)],
        );
        fixture.write(
            "apps/tv/build.gradle.kts",
            "plugins {\n    alias(libs.plugins.android.application)\n}\n",
        );
        let layout = detect_project(&fixture.0, ProjectChoices::default()).unwrap();
        assert_eq!(layout.kind, ProjectKind::Native);
        assert_eq!(layout.name, "Sample", "from rootProject.name");
        let android = layout.android.clone().unwrap();
        assert_eq!(android.root, "");
        assert_eq!(android.modules, vec![":apps:phone", ":apps:tv"]);
        assert_eq!(android.module_path, ":apps:phone");
        assert_eq!(android.module_dir, "apps/phone");
        assert_eq!(android.module_relative(), "apps/phone");
        assert_eq!(android.task("bundleRelease"), ":apps:phone:bundleRelease");

        let chosen = detect_project(
            &fixture.0,
            ProjectChoices {
                scheme: None,
                module: Some("apps/tv"),
            },
        )
        .unwrap();
        assert_eq!(chosen.android.unwrap().module_path, ":apps:tv");
        let error = detect_project(
            &fixture.0,
            ProjectChoices {
                scheme: None,
                module: Some(":core"),
            },
        )
        .unwrap_err();
        assert!(error.contains(":apps:phone, :apps:tv"), "{error}");
        assert_eq!(
            layout.describe(),
            "Native · Gradle at the root · module :apps:phone"
        );
    }

    #[test]
    fn a_kotlin_multiplatform_checkout_has_both_sides() {
        let fixture = Fixture::new();
        gradle_project(&fixture, "", &[("composeApp", true), ("shared", false)]);
        fixture.write("iosApp/Podfile", "target 'iosApp' do\nend\n");
        xcode_project(&fixture, "iosApp", "iosApp", true);
        let layout = detect_project(&fixture.0, ProjectChoices::default()).unwrap();
        assert_eq!(layout.ios.unwrap().container, "iosApp/iosApp.xcworkspace");
        assert_eq!(layout.android.unwrap().module_path, ":composeApp");
    }

    #[test]
    fn a_folder_with_nothing_to_build_is_refused_with_what_was_looked_for() {
        let fixture = Fixture::new();
        fixture.write("README.md", "hello");
        let error = detect_project(&fixture.0, ProjectChoices::default()).unwrap_err();
        assert!(
            error.contains("no Xcode workspace or project and no Gradle project"),
            "{error}"
        );
        fixture.write(
            "package.json",
            r#"{"name":"web","dependencies":{"@capacitor/core":"8.0.0"}}"#,
        );
        let error = detect_project(&fixture.0, ProjectChoices::default()).unwrap_err();
        assert!(
            error.contains("Capacitor project has no ios or android platform"),
            "{error}"
        );
    }

    #[test]
    fn gradle_settings_are_read_in_both_dialects() {
        assert_eq!(
            gradle_included_modules(
                "include ':app', ':lib'\ninclude(\":feature:home\")\nincludeBuild(\"plugins\")\ninclude 'plain'\n"
            ),
            vec![":app", ":lib", ":feature:home", ":plain"]
        );
        assert!(gradle_script_is_application(
            "apply plugin: 'com.android.application'\n"
        ));
        assert!(gradle_script_is_application(
            "plugins {\n  alias(libs.plugins.androidApplication)\n}\n"
        ));
        assert!(!gradle_script_is_application(
            "plugins { id(\"com.android.library\") }\n"
        ));
        assert!(!gradle_script_is_application(
            "// plugins { id(\"com.android.application\") }\n"
        ));
    }

    #[test]
    fn the_package_manager_follows_the_declaration_then_the_lockfile() {
        let fixture = Fixture::new();
        let package =
            |manager: Option<&str>| serde_json::json!({ "name": "x", "packageManager": manager });
        assert_eq!(
            detect_package_manager(&fixture.0, &package(None)),
            PackageManager::Npm
        );
        fixture.write("bun.lockb", "");
        assert_eq!(
            detect_package_manager(&fixture.0, &package(None)),
            PackageManager::Bun
        );
        fixture.write("yarn.lock", "");
        assert_eq!(
            detect_package_manager(&fixture.0, &package(None)),
            PackageManager::Yarn
        );
        fixture.write("pnpm-lock.yaml", "");
        assert_eq!(
            detect_package_manager(&fixture.0, &package(None)),
            PackageManager::Pnpm
        );
        assert_eq!(
            detect_package_manager(&fixture.0, &package(Some("npm@10.8.0"))),
            PackageManager::Npm
        );
        assert_eq!(
            detect_package_manager(&fixture.0, &package(Some("yarn@4.5.0+sha512.abc"))),
            PackageManager::Yarn
        );
    }

    #[test]
    fn a_cordova_widget_names_the_application_identifier_its_gradle_computes() {
        let fixture = Fixture::new();
        fixture.write(
            "config.xml",
            "<?xml version='1.0' encoding='utf-8'?>\n<widget id=\"com.example.cordovaapp\" version=\"1.0.0\" xmlns=\"http://www.w3.org/ns/widgets\">\n    <name>CordovaApp</name>\n</widget>\n",
        );
        assert_eq!(
            cordova_widget_id(&fixture.0).as_deref(),
            Some("com.example.cordovaapp")
        );
        fixture.write("config.xml", "<widget version=\"1.0.0\"></widget>");
        assert_eq!(cordova_widget_id(&fixture.0), None);
        assert_eq!(cordova_widget_id(Path::new("/nonexistent")), None);
    }

    #[test]
    fn the_default_layout_matches_what_older_records_assumed() {
        let layout = ProjectLayout::capacitor_default();
        assert_eq!(
            layout.describe(),
            "Capacitor · pnpm · ios/App/App.xcworkspace · scheme App · android · module :app"
        );
        assert!(layout.ios.unwrap().scheme_is_shared());
        assert_eq!(join_relative("", "app"), "app");
        assert_eq!(join_relative("android", ""), "android");
        assert_eq!(join_relative("android", "app"), "android/app");
    }
}
