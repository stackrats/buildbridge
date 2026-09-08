//! The version a build carries. The machine view reads it from the approved project's own
//! files each time, so the desktop shows what the project says now; a build that asks for a
//! different one gets it written into those files on the host, the edit the person would
//! otherwise make by hand, while the machines crate makes the same edit where the build runs.

use std::path::Path;

use super::*;
use ts_rs::TS;

/// What a build may ask for: either half alone, the other kept as the project declares it.
#[derive(Debug, Clone, Default, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ProjectVersionInput {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub build: Option<String>,
}

/// Where a project declares its version, for one platform: the file, and how it is read and
/// written. Xcode's project and the app module's Gradle script for most kinds; the pubspec for
/// a Flutter app, which writes both native projects' versions from it; the Expo config when
/// prebuild will write the native project from it and the project is not on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VersionFile {
    Xcode(String),
    Gradle(String),
    Pubspec,
    Expo { android: bool },
}

impl VersionFile {
    fn relative(&self) -> &str {
        match self {
            Self::Xcode(path) | Self::Gradle(path) => path,
            Self::Pubspec => "pubspec.yaml",
            Self::Expo { .. } => "app.json",
        }
    }

    fn read(&self, contents: &str) -> Option<ProjectVersion> {
        match self {
            Self::Xcode(_) => buildbridge_machines::xcode_project_version(contents),
            Self::Gradle(_) => buildbridge_machines::gradle_project_version(contents),
            Self::Pubspec => buildbridge_machines::pubspec_project_version(contents),
            Self::Expo { android } => serde_json::from_str(contents)
                .ok()
                .and_then(|config| buildbridge_machines::expo_project_version(&config, *android)),
        }
    }

    fn write(&self, contents: &str, version: &ProjectVersion) -> Result<String, String> {
        match self {
            Self::Xcode(_) => buildbridge_machines::set_xcode_project_version(contents, version),
            Self::Gradle(_) => buildbridge_machines::set_gradle_project_version(contents, version),
            Self::Pubspec => buildbridge_machines::set_pubspec_project_version(contents, version),
            Self::Expo { android } => {
                buildbridge_machines::set_expo_project_version(contents, version, *android)
            }
        }
    }
}

pub(crate) fn apple_version_file(local_path: &str, layout: &ProjectLayout) -> VersionFile {
    if layout.kind == ProjectKind::Flutter {
        return VersionFile::Pubspec;
    }
    let project_file = layout
        .ios
        .as_ref()
        .map(|ios| ios.project_file())
        .unwrap_or_else(|| "ios/App/App.xcodeproj/project.pbxproj".to_string());
    if layout.kind == ProjectKind::Expo && !Path::new(local_path).join(&project_file).is_file() {
        return VersionFile::Expo { android: false };
    }
    VersionFile::Xcode(project_file)
}

pub(crate) fn android_version_file(local_path: &str, layout: &ProjectLayout) -> VersionFile {
    if layout.kind == ProjectKind::Flutter {
        return VersionFile::Pubspec;
    }
    let script = layout
        .android
        .as_ref()
        .map(|android| android.script.clone())
        .unwrap_or_else(|| "android/app/build.gradle".to_string());
    if layout.kind == ProjectKind::Expo && !Path::new(local_path).join(&script).is_file() {
        return VersionFile::Expo { android: true };
    }
    VersionFile::Gradle(script)
}

fn read_project_version(local_path: &str, file: &VersionFile) -> Option<ProjectVersion> {
    let contents = fs::read_to_string(Path::new(local_path).join(file.relative())).ok()?;
    file.read(&contents)
}

pub(crate) fn read_apple_project_version(
    local_path: &str,
    layout: &ProjectLayout,
) -> Option<ProjectVersion> {
    read_project_version(local_path, &apple_version_file(local_path, layout))
}

pub(crate) fn read_android_project_version(
    local_path: &str,
    layout: &ProjectLayout,
) -> Option<ProjectVersion> {
    read_project_version(local_path, &android_version_file(local_path, layout))
}

/// The version a build will carry: the requested halves over the project's own, checked for
/// the platform. `None` when nothing was requested, so the build runs the project as synced.
fn resolve_project_version(
    current: Option<ProjectVersion>,
    input: Option<ProjectVersionInput>,
    file: &str,
    validate: fn(&ProjectVersion) -> Result<(), String>,
) -> Result<Option<ProjectVersion>, String> {
    let Some(input) = input else {
        return Ok(None);
    };
    let requested = |value: Option<String>| {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let (version, build) = (requested(input.version), requested(input.build));
    if version.is_none() && build.is_none() {
        return Ok(None);
    }
    let resolved = match (version, build, current) {
        (Some(version), Some(build), _) => ProjectVersion { version, build },
        (version, build, Some(current)) => ProjectVersion {
            version: version.unwrap_or(current.version),
            build: build.unwrap_or(current.build),
        },
        _ => {
            return Err(format!(
                "buildbridge could not read one version and build number from {file}, so give both to set them."
            ));
        }
    };
    validate(&resolved)?;
    Ok(Some(resolved))
}

pub(crate) fn resolve_apple_project_version(
    local_path: &str,
    layout: &ProjectLayout,
    input: Option<ProjectVersionInput>,
) -> Result<Option<ProjectVersion>, String> {
    let file = apple_version_file(local_path, layout);
    resolve_project_version(
        read_project_version(local_path, &file),
        input,
        file.relative(),
        buildbridge_machines::validate_apple_version,
    )
}

pub(crate) fn resolve_android_project_version(
    local_path: &str,
    layout: &ProjectLayout,
    input: Option<ProjectVersionInput>,
) -> Result<Option<ProjectVersion>, String> {
    let file = android_version_file(local_path, layout);
    resolve_project_version(
        read_project_version(local_path, &file),
        input,
        file.relative(),
        buildbridge_machines::validate_android_version,
    )
}

/// Writes the version into the project's own file on the host; a file that already says it is
/// left untouched.
fn write_project_version(
    local_path: &str,
    file: &VersionFile,
    version: &ProjectVersion,
) -> Result<(), String> {
    let path = Path::new(local_path).join(file.relative());
    let relative = file.relative();
    let current =
        fs::read_to_string(&path).map_err(|error| format!("Could not read {relative}: {error}"))?;
    let rewritten = file.write(&current, version)?;
    if rewritten != current {
        fs::write(&path, rewritten)
            .map_err(|error| format!("Could not write the version into {relative}: {error}"))?;
    }
    Ok(())
}

pub(crate) fn write_apple_project_version(
    local_path: &str,
    layout: &ProjectLayout,
    version: &ProjectVersion,
) -> Result<(), String> {
    write_project_version(local_path, &apple_version_file(local_path, layout), version)
}

pub(crate) fn write_android_project_version(
    local_path: &str,
    layout: &ProjectLayout,
    version: &ProjectVersion,
) -> Result<(), String> {
    write_project_version(
        local_path,
        &android_version_file(local_path, layout),
        version,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(version: &str, build: &str) -> ProjectVersion {
        ProjectVersion {
            version: version.to_string(),
            build: build.to_string(),
        }
    }

    fn input(version: Option<&str>, build: Option<&str>) -> Option<ProjectVersionInput> {
        Some(ProjectVersionInput {
            version: version.map(str::to_string),
            build: build.map(str::to_string),
        })
    }

    #[test]
    fn a_request_fills_its_missing_half_from_the_project_and_is_checked_for_the_platform() {
        let current = || Some(version("3.2.0", "15"));
        let apple = buildbridge_machines::validate_apple_version;
        assert_eq!(
            resolve_project_version(current(), None, "f", apple).unwrap(),
            None
        );
        assert_eq!(
            resolve_project_version(current(), input(Some(" "), None), "f", apple).unwrap(),
            None
        );
        assert_eq!(
            resolve_project_version(current(), input(None, Some(" 16 ")), "f", apple).unwrap(),
            Some(version("3.2.0", "16"))
        );
        assert_eq!(
            resolve_project_version(current(), input(Some("3.3.0"), None), "f", apple).unwrap(),
            Some(version("3.3.0", "15"))
        );
        assert_eq!(
            resolve_project_version(None, input(Some("3.3.0"), Some("1")), "f", apple).unwrap(),
            Some(version("3.3.0", "1"))
        );
        let unreadable = resolve_project_version(None, input(None, Some("16")), "f", apple);
        assert!(unreadable.unwrap_err().contains("give both"));
        let android = buildbridge_machines::validate_android_version;
        assert!(
            resolve_project_version(current(), input(None, Some("1.0.3")), "f", android).is_err()
        );
    }

    #[test]
    fn the_project_files_on_the_host_take_the_version_and_keep_their_other_lines() {
        let root = std::env::temp_dir().join(format!(
            "buildbridge-version-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let layout = ProjectLayout::capacitor_default();
        let project = root.join("ios/App/App.xcodeproj/project.pbxproj");
        fs::create_dir_all(project.parent().unwrap()).unwrap();
        fs::write(
            &project,
            "\t\t\t\tCURRENT_PROJECT_VERSION = 15;\n\t\t\t\tMARKETING_VERSION = 3.2.0;\n\t\t\t\tPRODUCT_BUNDLE_IDENTIFIER = com.example.app;\n",
        )
        .unwrap();
        let script = root.join("android/app/build.gradle");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(
            &script,
            "        versionCode = 12\n        versionName = \"3.2.0\"\n",
        )
        .unwrap();
        let local_path = root.to_str().unwrap();

        assert_eq!(
            read_apple_project_version(local_path, &layout),
            Some(version("3.2.0", "15"))
        );
        assert_eq!(
            read_android_project_version(local_path, &layout),
            Some(version("3.2.0", "12"))
        );
        write_apple_project_version(local_path, &layout, &version("3.2.1", "16")).unwrap();
        write_android_project_version(local_path, &layout, &version("3.2.1", "13")).unwrap();
        assert_eq!(
            fs::read_to_string(&project).unwrap(),
            "\t\t\t\tCURRENT_PROJECT_VERSION = 16;\n\t\t\t\tMARKETING_VERSION = 3.2.1;\n\t\t\t\tPRODUCT_BUNDLE_IDENTIFIER = com.example.app;\n"
        );
        assert_eq!(
            fs::read_to_string(&script).unwrap(),
            "        versionCode = 13\n        versionName = \"3.2.1\"\n"
        );
        assert_eq!(
            read_apple_project_version("/nonexistent/project", &layout),
            None
        );
        assert!(
            write_android_project_version("/nonexistent/project", &layout, &version("1", "1"))
                .is_err()
        );

        // A Flutter app keeps both platforms' version in its pubspec; an Expo app without
        // native projects keeps them in its app config.
        let mut flutter = layout.clone();
        flutter.kind = ProjectKind::Flutter;
        fs::write(root.join("pubspec.yaml"), "name: app\nversion: 1.0.0+7\n").unwrap();
        assert_eq!(
            apple_version_file(local_path, &flutter),
            VersionFile::Pubspec
        );
        assert_eq!(
            read_android_project_version(local_path, &flutter),
            Some(version("1.0.0", "7"))
        );
        write_apple_project_version(local_path, &flutter, &version("1.1.0", "8")).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("pubspec.yaml")).unwrap(),
            "name: app\nversion: 1.1.0+8\n"
        );
        let mut expo = layout.clone();
        expo.kind = ProjectKind::Expo;
        expo.ios.as_mut().unwrap().project = "ios/Missing.xcodeproj".to_string();
        expo.android.as_mut().unwrap().script = "android/missing/build.gradle".to_string();
        fs::write(
            root.join("app.json"),
            r#"{"expo":{"name":"App","version":"2.0.0","ios":{"buildNumber":"3"}}}"#,
        )
        .unwrap();
        assert_eq!(
            apple_version_file(local_path, &expo),
            VersionFile::Expo { android: false }
        );
        assert_eq!(
            read_apple_project_version(local_path, &expo),
            Some(version("2.0.0", "3"))
        );
        assert_eq!(
            read_android_project_version(local_path, &expo),
            Some(version("2.0.0", "1"))
        );
        write_android_project_version(local_path, &expo, &version("2.1.0", "9")).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(root.join("app.json")).unwrap()).unwrap();
        assert_eq!(written["expo"]["android"]["versionCode"], 9);
        assert_eq!(written["expo"]["ios"]["buildNumber"], "3");
        fs::remove_dir_all(&root).unwrap();
    }
}
