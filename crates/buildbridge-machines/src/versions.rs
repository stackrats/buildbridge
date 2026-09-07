//! The version a project declares, read from and written into the project's own files: the
//! Xcode project's `MARKETING_VERSION` and `CURRENT_PROJECT_VERSION`, and the Android app
//! module's `versionName` and `versionCode`. Both platforms have the same two-part shape, so
//! one value travels from the desktop or the command line into either build. Nothing here
//! bumps a number on its own: buildbridge builds the version the project says, or the one the
//! person asked for, and writes that into the project so it is there to commit.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A version as a project declares it and as a build reports it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ProjectVersion {
    /// The version people see: `MARKETING_VERSION` on iOS, `versionName` on Android.
    pub version: String,
    /// The number the stores order builds by: `CURRENT_PROJECT_VERSION` or `versionCode`.
    pub build: String,
}

impl ProjectVersion {
    /// `3.2.0 (15)`, the way both stores print it.
    pub fn display(&self) -> String {
        format!("{} ({})", self.version, self.build)
    }
}

/// The largest version code Google Play accepts.
const ANDROID_VERSION_CODE_MAX: u64 = 2_100_000_000;

/// A version string either platform accepts and the recipes can pass along verbatim: the same
/// shape the built app is later checked against.
pub fn valid_project_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
}

/// Apple's `CFBundleVersion`: one to three integers separated by periods.
pub fn valid_apple_build_number(value: &str) -> bool {
    let components: Vec<&str> = value.split('.').collect();
    !value.is_empty()
        && value.len() <= 32
        && (1..=3).contains(&components.len())
        && components
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// Android's `versionCode`: a positive integer within Google Play's bound.
pub fn valid_android_version_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 10
        && value.chars().all(|c| c.is_ascii_digit())
        && value
            .parse::<u64>()
            .is_ok_and(|code| (1..=ANDROID_VERSION_CODE_MAX).contains(&code))
}

pub fn validate_apple_version(version: &ProjectVersion) -> Result<(), String> {
    if !valid_project_version(&version.version) {
        return Err(
            "The version must be 1 to 64 letters, digits, periods or hyphens, such as 3.2.1."
                .to_string(),
        );
    }
    if !valid_apple_build_number(&version.build) {
        return Err(
            "The build number must be a whole number, or up to three joined by periods, such as 16."
                .to_string(),
        );
    }
    Ok(())
}

pub fn validate_android_version(version: &ProjectVersion) -> Result<(), String> {
    if !valid_project_version(&version.version) {
        return Err(
            "The version name must be 1 to 64 letters, digits, periods or hyphens, such as 3.2.1."
                .to_string(),
        );
    }
    if !valid_android_version_code(&version.build) {
        return Err(
            "The version code must be a whole number from 1 to 2100000000, such as 13.".to_string(),
        );
    }
    Ok(())
}

/// Rewrites a file line by line, keeping each line's own ending.
fn rewrite_lines(text: &str, mut rewrite: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    for segment in text.split_inclusive('\n') {
        let (line, ending) = match segment.strip_suffix('\n') {
            Some(line) => match line.strip_suffix('\r') {
                Some(line) => (line, "\r\n"),
                None => (line, "\n"),
            },
            None => (segment, ""),
        };
        match rewrite(line) {
            Some(replacement) => out.push_str(&replacement),
            None => out.push_str(line),
        }
        out.push_str(ending);
    }
    out
}

/// The value on a `KEY = value;` line of a pbxproj, unquoted; `None` for any other line.
fn xcode_setting_line<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.trim().strip_prefix(key)?;
    let rest = match rest.chars().next() {
        Some(c) if c.is_whitespace() || c == '=' => rest.trim_start(),
        _ => return None,
    };
    let value = rest.strip_prefix('=')?.trim().strip_suffix(';')?.trim();
    Some(value.trim_matches('"'))
}

/// Every distinct value a setting takes across the project's configurations.
fn xcode_setting_values(project: &str, key: &str) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    for value in project
        .lines()
        .filter_map(|line| xcode_setting_line(line, key))
    {
        if !values.iter().any(|stored| stored == value) {
            values.push(value.to_string());
        }
    }
    values
}

/// The version an Xcode project declares, when every configuration agrees on one marketing
/// version and one build number in a shape a store accepts. A project that computes either,
/// or gives its configurations different values, is left unknown rather than guessed.
pub fn xcode_project_version(project: &str) -> Option<ProjectVersion> {
    let mut versions = xcode_setting_values(project, "MARKETING_VERSION");
    let mut builds = xcode_setting_values(project, "CURRENT_PROJECT_VERSION");
    if versions.len() != 1 || builds.len() != 1 {
        return None;
    }
    let version = ProjectVersion {
        version: versions.remove(0),
        build: builds.remove(0),
    };
    validate_apple_version(&version).ok().map(|()| version)
}

/// The same line with a new value, keeping the indentation the file uses.
fn replace_xcode_value(line: &str, key: &str, value: &str) -> String {
    let indent = &line[..line.len() - line.trim_start().len()];
    format!("{indent}{key} = {value};")
}

/// The project with the version written into every configuration that declares one, the way
/// Xcode's own version editor does; fails when the project declares neither key.
pub fn set_xcode_project_version(
    project: &str,
    version: &ProjectVersion,
) -> Result<String, String> {
    validate_apple_version(version)?;
    let (mut versions, mut builds) = (0, 0);
    let rewritten = rewrite_lines(project, |line| {
        if xcode_setting_line(line, "MARKETING_VERSION").is_some() {
            versions += 1;
            Some(replace_xcode_value(
                line,
                "MARKETING_VERSION",
                &version.version,
            ))
        } else if xcode_setting_line(line, "CURRENT_PROJECT_VERSION").is_some() {
            builds += 1;
            Some(replace_xcode_value(
                line,
                "CURRENT_PROJECT_VERSION",
                &version.build,
            ))
        } else {
            None
        }
    });
    if versions == 0 || builds == 0 {
        return Err(
            "The Xcode project does not declare MARKETING_VERSION and CURRENT_PROJECT_VERSION in its build settings, so buildbridge cannot set its version. Set the version and build in Xcode once; after that they can be set here."
                .to_string(),
        );
    }
    Ok(rewritten)
}

/// Where the value of a Gradle setting sits on one line: `versionCode 12`, `versionCode = 12`,
/// `versionName "3.2.0"`, `versionName = "3.2.0"`, in the Groovy or the Kotlin dialect. The
/// range excludes the quotes. Comment lines and settings that merely start with the key,
/// such as `versionNameSuffix`, are not matches.
fn gradle_value_span(line: &str, key: &str) -> Option<std::ops::Range<usize>> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*") {
        return None;
    }
    let mut offset = line.len() - trimmed.len();
    let mut rest = trimmed.strip_prefix(key)?;
    match rest.chars().next() {
        Some(c) if c.is_whitespace() || c == '=' => {}
        _ => return None,
    }
    offset += key.len();
    let skip_whitespace = |rest: &mut &str, offset: &mut usize| {
        let spaces = rest.len() - rest.trim_start().len();
        *offset += spaces;
        *rest = &rest[spaces..];
    };
    skip_whitespace(&mut rest, &mut offset);
    if let Some(after) = rest.strip_prefix('=') {
        offset += 1;
        rest = after;
        skip_whitespace(&mut rest, &mut offset);
    }
    match rest.chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let inner = &rest[1..];
            let end = inner.find(quote)?;
            let start = offset + 1;
            (end > 0).then_some(start..start + end)
        }
        Some(_) => {
            let end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, ';' | '/' | ')'))
                .unwrap_or(rest.len());
            (end > 0).then_some(offset..offset + end)
        }
        None => None,
    }
}

fn gradle_setting_line<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    gradle_value_span(line, key).map(|span| &line[span])
}

fn gradle_setting_values(script: &str, key: &str) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    for value in script
        .lines()
        .filter_map(|line| gradle_setting_line(line, key))
    {
        if !values.iter().any(|stored| stored == value) {
            values.push(value.to_string());
        }
    }
    values
}

/// The version an app module's Gradle script declares, when it declares one literal version
/// name and one literal version code. Computed values and flavours that disagree are left
/// unknown.
pub fn gradle_project_version(script: &str) -> Option<ProjectVersion> {
    let mut versions = gradle_setting_values(script, "versionName");
    let mut builds = gradle_setting_values(script, "versionCode");
    if versions.len() != 1 || builds.len() != 1 {
        return None;
    }
    let version = ProjectVersion {
        version: versions.remove(0),
        build: builds.remove(0),
    };
    validate_android_version(&version).ok().map(|()| version)
}

/// The script with the version written into every literal declaration, keeping each line's
/// spacing, quoting and trailing comment; fails when the script declares neither.
pub fn set_gradle_project_version(
    script: &str,
    version: &ProjectVersion,
) -> Result<String, String> {
    validate_android_version(version)?;
    let (mut versions, mut builds) = (0, 0);
    let rewritten = rewrite_lines(script, |line| {
        if let Some(span) = gradle_value_span(line, "versionName") {
            versions += 1;
            Some(format!(
                "{}{}{}",
                &line[..span.start],
                version.version,
                &line[span.end..]
            ))
        } else if let Some(span) = gradle_value_span(line, "versionCode") {
            builds += 1;
            Some(format!(
                "{}{}{}",
                &line[..span.start],
                version.build,
                &line[span.end..]
            ))
        } else {
            None
        }
    });
    if versions == 0 || builds == 0 {
        return Err(
            "The app module's Gradle script does not declare versionName and versionCode as literals, so buildbridge cannot set its version. Declare both in defaultConfig once; after that they can be set here."
                .to_string(),
        );
    }
    Ok(rewritten)
}

/// `3.2.0-16`: the version and build as a file-name tag, so a retained artifact says which
/// build it is without being opened. Anything a file name should not carry becomes `_`.
pub fn version_file_tag(version: &str, build: &str) -> String {
    let clean = |value: &str| {
        value
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    format!("{}-{}", clean(version), clean(build))
}

/// The build settings that give an archive its version, appended to the signing settings the
/// archive already runs with. They apply to every target in the build, the way Xcode's own
/// version editor sets both fields on every configuration.
pub(crate) fn apple_version_xcconfig(version: &ProjectVersion) -> String {
    format!(
        "MARKETING_VERSION = {}\nCURRENT_PROJECT_VERSION = {}\n",
        version.version, version.build
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PBXPROJ: &str = "\t\t\t\tCODE_SIGN_STYLE = Automatic;\n\t\t\t\tCURRENT_PROJECT_VERSION = 15;\n\t\t\t\tINFOPLIST_FILE = App/Info.plist;\n\t\t\t\tMARKETING_VERSION = 3.2.0;\n\t\t\t};\n\t\t\tname = Debug;\n\t\t};\n\t\t\t\tCURRENT_PROJECT_VERSION = 15;\n\t\t\t\tMARKETING_VERSION = 3.2.0;\n\t\t\t\tPRODUCT_BUNDLE_IDENTIFIER = com.example.app;\n";

    const GROOVY: &str = "android {\n    namespace \"com.example.app\"\n    defaultConfig {\n        applicationId \"com.example.app\"\n        versionCode 12\n        versionName \"3.2.0\"\n        // versionName \"9.9.9\"\n    }\n    buildTypes {\n        debug {\n            versionNameSuffix \"-debug\"\n        }\n    }\n}\n";

    const KOTLIN: &str = "android {\r\n    defaultConfig {\r\n        versionCode = 12 // bumped by hand\r\n        versionName = \"3.2.0\"\r\n    }\r\n}\r\n";

    fn version(version: &str, build: &str) -> ProjectVersion {
        ProjectVersion {
            version: version.to_string(),
            build: build.to_string(),
        }
    }

    #[test]
    fn xcode_version_is_read_when_every_configuration_agrees() {
        assert_eq!(xcode_project_version(PBXPROJ), Some(version("3.2.0", "15")));
        let quoted = PBXPROJ.replace(
            "MARKETING_VERSION = 3.2.0;",
            "MARKETING_VERSION = \"3.2.0\";",
        );
        assert_eq!(xcode_project_version(&quoted), Some(version("3.2.0", "15")));
    }

    #[test]
    fn xcode_version_is_unknown_when_configurations_differ_or_compute_it() {
        let differing = PBXPROJ.replacen(
            "MARKETING_VERSION = 3.2.0;",
            "MARKETING_VERSION = 3.3.0;",
            1,
        );
        assert_eq!(xcode_project_version(&differing), None);
        let computed = PBXPROJ.replace(
            "CURRENT_PROJECT_VERSION = 15;",
            "CURRENT_PROJECT_VERSION = $(BUILD);",
        );
        assert_eq!(xcode_project_version(&computed), None);
        assert_eq!(xcode_project_version("no settings here"), None);
    }

    #[test]
    fn xcode_version_is_written_into_every_configuration_with_its_indentation() {
        let rewritten = set_xcode_project_version(PBXPROJ, &version("3.2.1", "16")).unwrap();
        assert_eq!(
            rewritten
                .matches("\t\t\t\tMARKETING_VERSION = 3.2.1;\n")
                .count(),
            2
        );
        assert_eq!(
            rewritten
                .matches("\t\t\t\tCURRENT_PROJECT_VERSION = 16;\n")
                .count(),
            2
        );
        assert!(!rewritten.contains("3.2.0"));
        assert!(rewritten.contains("\t\t\t\tPRODUCT_BUNDLE_IDENTIFIER = com.example.app;\n"));
        assert_eq!(
            xcode_project_version(&rewritten),
            Some(version("3.2.1", "16"))
        );
        assert!(set_xcode_project_version("no settings", &version("1.0", "1")).is_err());
        assert!(set_xcode_project_version(PBXPROJ, &version("1.0", "sixteen")).is_err());
    }

    #[test]
    fn gradle_version_is_read_in_either_dialect_and_ignores_comments_and_suffixes() {
        assert_eq!(gradle_project_version(GROOVY), Some(version("3.2.0", "12")));
        assert_eq!(gradle_project_version(KOTLIN), Some(version("3.2.0", "12")));
        let computed = GROOVY.replace("versionCode 12", "versionCode buildNumber()");
        assert_eq!(gradle_project_version(&computed), None);
        let flavours = GROOVY.replace(
            "versionCode 12\n",
            "versionCode 12\n        versionCode 13\n",
        );
        assert_eq!(gradle_project_version(&flavours), None);
    }

    #[test]
    fn gradle_version_is_written_keeping_spacing_quotes_comments_and_line_endings() {
        let groovy = set_gradle_project_version(GROOVY, &version("3.2.1", "13")).unwrap();
        assert!(groovy.contains("        versionCode 13\n        versionName \"3.2.1\"\n"));
        assert!(groovy.contains("        // versionName \"9.9.9\"\n"));
        assert!(groovy.contains("versionNameSuffix \"-debug\""));
        assert_eq!(
            gradle_project_version(&groovy),
            Some(version("3.2.1", "13"))
        );

        let kotlin = set_gradle_project_version(KOTLIN, &version("4.0.0", "20")).unwrap();
        assert!(kotlin.contains("        versionCode = 20 // bumped by hand\r\n"));
        assert!(kotlin.contains("        versionName = \"4.0.0\"\r\n"));
        assert_eq!(
            gradle_project_version(&kotlin),
            Some(version("4.0.0", "20"))
        );

        assert!(set_gradle_project_version("android {}", &version("1.0", "1")).is_err());
        assert!(set_gradle_project_version(GROOVY, &version("1.0", "0")).is_err());
    }

    #[test]
    fn validators_bound_both_platforms() {
        assert!(validate_apple_version(&version("3.2.0", "15")).is_ok());
        assert!(validate_apple_version(&version("3.2.0", "1.0.3")).is_ok());
        assert!(validate_apple_version(&version("3.2.0", "1.0.3.4")).is_err());
        assert!(validate_apple_version(&version("3.2 beta", "15")).is_err());
        assert!(validate_apple_version(&version("$(V)", "15")).is_err());
        assert!(validate_android_version(&version("3.2.0", "12")).is_ok());
        assert!(validate_android_version(&version("3.2.0", "1.2")).is_err());
        assert!(validate_android_version(&version("3.2.0", "2100000001")).is_err());
        assert!(validate_android_version(&version("", "12")).is_err());
    }

    #[test]
    fn the_file_tag_carries_both_numbers_and_nothing_a_file_name_cannot() {
        assert_eq!(version_file_tag("3.2.0", "16"), "3.2.0-16");
        assert_eq!(version_file_tag("3.2 beta/1", "16"), "3.2_beta_1-16");
    }

    #[test]
    fn version_settings_join_the_signing_settings() {
        assert_eq!(
            apple_version_xcconfig(&version("3.2.1", "16")),
            "MARKETING_VERSION = 3.2.1\nCURRENT_PROJECT_VERSION = 16\n"
        );
        assert_eq!(version("3.2.1", "16").display(), "3.2.1 (16)");
    }
}
