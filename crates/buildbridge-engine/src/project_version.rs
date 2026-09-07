//! The version a build carries. The machine view reads it from the approved project's own
//! files each time, so the desktop shows what the project says now; a build that asks for a
//! different one gets it written into those files on the host, the edit the person would
//! otherwise make by hand, while the machines crate makes the same edit where the build runs.

use std::path::Path;

use super::*;
use ts_rs::TS;

pub(crate) const APPLE_PROJECT_FILE: &str = "ios/App/App.xcodeproj/project.pbxproj";
const ANDROID_APP_SCRIPT: &str = "android/app/build.gradle";

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

fn android_app_script(local_path: &str) -> Option<PathBuf> {
    [ANDROID_APP_SCRIPT, "android/app/build.gradle.kts"]
        .into_iter()
        .map(|relative| Path::new(local_path).join(relative))
        .find(|candidate| candidate.is_file())
}

pub(crate) fn read_apple_project_version(local_path: &str) -> Option<ProjectVersion> {
    let project = fs::read_to_string(Path::new(local_path).join(APPLE_PROJECT_FILE)).ok()?;
    buildbridge_machines::xcode_project_version(&project)
}

pub(crate) fn read_android_project_version(local_path: &str) -> Option<ProjectVersion> {
    let script = fs::read_to_string(android_app_script(local_path)?).ok()?;
    buildbridge_machines::gradle_project_version(&script)
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
    input: Option<ProjectVersionInput>,
) -> Result<Option<ProjectVersion>, String> {
    resolve_project_version(
        read_apple_project_version(local_path),
        input,
        APPLE_PROJECT_FILE,
        buildbridge_machines::validate_apple_version,
    )
}

pub(crate) fn resolve_android_project_version(
    local_path: &str,
    input: Option<ProjectVersionInput>,
) -> Result<Option<ProjectVersion>, String> {
    resolve_project_version(
        read_android_project_version(local_path),
        input,
        ANDROID_APP_SCRIPT,
        buildbridge_machines::validate_android_version,
    )
}

fn write_project_file(
    path: &Path,
    file: &str,
    rewrite: impl FnOnce(&str) -> Result<String, String>,
) -> Result<(), String> {
    let current =
        fs::read_to_string(path).map_err(|error| format!("Could not read {file}: {error}"))?;
    let rewritten = rewrite(&current)?;
    if rewritten != current {
        fs::write(path, rewritten)
            .map_err(|error| format!("Could not write the version into {file}: {error}"))?;
    }
    Ok(())
}

/// Writes the version into the Xcode project on the host; a project that already says it is
/// left untouched.
pub(crate) fn write_apple_project_version(
    local_path: &str,
    version: &ProjectVersion,
) -> Result<(), String> {
    write_project_file(
        &Path::new(local_path).join(APPLE_PROJECT_FILE),
        APPLE_PROJECT_FILE,
        |project| buildbridge_machines::set_xcode_project_version(project, version),
    )
}

/// Writes the version into the app module's Gradle script on the host, whichever dialect the
/// project uses; a script that already says it is left untouched.
pub(crate) fn write_android_project_version(
    local_path: &str,
    version: &ProjectVersion,
) -> Result<(), String> {
    let path = android_app_script(local_path)
        .ok_or_else(|| format!("This project is missing {ANDROID_APP_SCRIPT}."))?;
    write_project_file(&path, ANDROID_APP_SCRIPT, |script| {
        buildbridge_machines::set_gradle_project_version(script, version)
    })
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
        let project = root.join(APPLE_PROJECT_FILE);
        fs::create_dir_all(project.parent().unwrap()).unwrap();
        fs::write(
            &project,
            "\t\t\t\tCURRENT_PROJECT_VERSION = 15;\n\t\t\t\tMARKETING_VERSION = 3.2.0;\n\t\t\t\tPRODUCT_BUNDLE_IDENTIFIER = com.example.app;\n",
        )
        .unwrap();
        let script = root.join("android/app/build.gradle.kts");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(
            &script,
            "        versionCode = 12\n        versionName = \"3.2.0\"\n",
        )
        .unwrap();
        let local_path = root.to_str().unwrap();

        assert_eq!(
            read_apple_project_version(local_path),
            Some(version("3.2.0", "15"))
        );
        assert_eq!(
            read_android_project_version(local_path),
            Some(version("3.2.0", "12"))
        );
        write_apple_project_version(local_path, &version("3.2.1", "16")).unwrap();
        write_android_project_version(local_path, &version("3.2.1", "13")).unwrap();
        assert_eq!(
            fs::read_to_string(&project).unwrap(),
            "\t\t\t\tCURRENT_PROJECT_VERSION = 16;\n\t\t\t\tMARKETING_VERSION = 3.2.1;\n\t\t\t\tPRODUCT_BUNDLE_IDENTIFIER = com.example.app;\n"
        );
        assert_eq!(
            fs::read_to_string(&script).unwrap(),
            "        versionCode = 13\n        versionName = \"3.2.1\"\n"
        );
        assert_eq!(read_apple_project_version("/nonexistent/project"), None);
        assert!(write_android_project_version("/nonexistent/project", &version("1", "1")).is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}
