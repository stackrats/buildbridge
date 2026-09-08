//! One row per framework: how a folder is recognised as one, where it keeps its native
//! projects, and what its build needs. Everything that differs between Capacitor and Flutter
//! and the rest used to be spread across the detector's search paths, two error messages and
//! the recipes' match arms; it is gathered here so that adding a framework is a row in this
//! table plus, when it has a step of its own, one function in `recipes.rs`.
//!
//! The order of the table is the order a folder is tested in, so a more specific framework
//! comes before a more general one: Expo is React Native with an app config, and both keep a
//! `package.json` that Capacitor and Cordova also keep. `Native` matches anything and is last.

use std::collections::BTreeSet;
use std::path::Path;

use crate::project::{AndroidProject, IosProject, ProjectChoices, ProjectKind};

/// Names the native projects a framework will write where the build runs, for a folder that
/// does not carry them yet. Returning `None` for a platform leaves it undetected.
pub(crate) type PredictNative = fn(
    &ProjectMarkers<'_>,
    ProjectChoices<'_>,
) -> Result<(Option<IosProject>, Option<AndroidProject>), String>;

/// What a folder shows about itself, read once and offered to every framework's test.
pub(crate) struct ProjectMarkers<'a> {
    pub(crate) root: &'a Path,
    /// The parsed `package.json`, when there is one.
    pub(crate) package: Option<&'a serde_json::Value>,
    /// Its `dependencies` and `devDependencies` names.
    pub(crate) dependencies: &'a BTreeSet<String>,
    /// The parsed `composer.json`, when there is one.
    pub(crate) composer: Option<&'a serde_json::Value>,
    /// The `pubspec.yaml`, when there is one.
    pub(crate) pubspec: Option<&'a str>,
}

impl ProjectMarkers<'_> {
    fn has_file(&self, relative: &str) -> bool {
        self.root.join(relative).is_file()
    }

    fn any_file(&self, relatives: &[&str]) -> bool {
        relatives.iter().any(|relative| self.has_file(relative))
    }

    /// Whether `composer.json` requires a package, in either require section.
    fn requires_composer_package(&self, name: &str) -> bool {
        let Some(composer) = self.composer else {
            return false;
        };
        ["require", "require-dev"]
            .iter()
            .filter_map(|section| composer.get(section))
            .filter_map(serde_json::Value::as_object)
            .any(|require| require.contains_key(name))
    }
}

pub(crate) struct Framework {
    pub(crate) kind: ProjectKind,
    pub(crate) label: &'static str,
    /// Whether a folder is this framework's. Tested in table order.
    pub(crate) detect: fn(&ProjectMarkers<'_>) -> bool,
    /// Where the Xcode workspace or project is looked for, relative to the project folder, in
    /// order. Empty means the search falls back to the project folder and one level down.
    pub(crate) ios_dirs: &'static [&'static str],
    /// Where the Gradle root is looked for, in the same way.
    pub(crate) android_dirs: &'static [&'static str],
    /// Whether the project installs dependencies with a JavaScript package manager.
    pub(crate) javascript: bool,
    /// Whether a build produces web assets, so that choosing an environment for a build means
    /// building them again.
    pub(crate) web_assets: bool,
    /// How the native projects are named before the framework writes them, for a framework
    /// that generates rather than commits them. `None` means they must already be there.
    pub(crate) predict: Option<PredictNative>,
    /// What to tell someone whose folder has neither native project.
    pub(crate) missing_native: &'static str,
}

pub(crate) const FRAMEWORKS: &[Framework] = &[
    Framework {
        kind: ProjectKind::Flutter,
        label: "Flutter",
        detect: |markers| {
            markers
                .pubspec
                .is_some_and(crate::project::pubspec_uses_flutter)
        },
        ios_dirs: &["ios"],
        android_dirs: &["android"],
        javascript: false,
        web_assets: false,
        predict: None,
        missing_native: "This Flutter project has no ios or android folder. Run `flutter create --platforms=ios,android .` in it and approve it again.",
    },
    Framework {
        kind: ProjectKind::Capacitor,
        label: "Capacitor",
        detect: |markers| {
            markers.package.is_some()
                && (markers.dependencies.contains("@capacitor/core")
                    || markers.any_file(&[
                        "capacitor.config.ts",
                        "capacitor.config.js",
                        "capacitor.config.json",
                    ]))
        },
        ios_dirs: &["ios/App", "ios"],
        android_dirs: &["android"],
        javascript: true,
        web_assets: true,
        predict: None,
        missing_native: "This Capacitor project has no ios or android platform yet. Run `npx cap add ios` or `npx cap add android` in it and approve it again.",
    },
    Framework {
        kind: ProjectKind::Expo,
        label: "Expo",
        detect: |markers| {
            markers.package.is_some()
                && markers.dependencies.contains("expo")
                && markers.any_file(&[
                    "app.json",
                    "app.config.js",
                    "app.config.ts",
                    "app.config.mjs",
                ])
        },
        ios_dirs: &["ios"],
        android_dirs: &["android"],
        javascript: true,
        web_assets: false,
        predict: Some(crate::project::expo_predicted_projects),
        missing_native: "",
    },
    Framework {
        kind: ProjectKind::ReactNative,
        label: "React Native",
        detect: |markers| {
            markers.package.is_some() && markers.dependencies.contains("react-native")
        },
        ios_dirs: &["ios"],
        android_dirs: &["android"],
        javascript: true,
        web_assets: false,
        predict: None,
        missing_native: "This React Native project has no ios or android folder. Generate them with the React Native command line and approve it again.",
    },
    Framework {
        kind: ProjectKind::Cordova,
        label: "Cordova",
        detect: |markers| {
            markers.package.is_some()
                && (markers.dependencies.contains("cordova")
                    || markers.dependencies.contains("cordova-android")
                    || markers.dependencies.contains("cordova-ios")
                    || crate::project::cordova_widget_id(markers.root).is_some())
        },
        ios_dirs: &["platforms/ios"],
        android_dirs: &["platforms/android"],
        javascript: true,
        web_assets: true,
        predict: None,
        missing_native: "This Cordova project has no platform added yet. Run `cordova platform add ios` or `cordova platform add android` in it and approve it again.",
    },
    Framework {
        // NativePHP bundles a statically compiled PHP with the Laravel application inside a
        // Swift or Kotlin shell. `php artisan native:install` writes that shell into
        // `nativephp/`, which the documentation asks people to keep out of Git, so the folder
        // is approved once the install has been run rather than predicted like Expo's: the
        // shell's own name is the package's to choose, and buildbridge does not guess it.
        kind: ProjectKind::NativePhp,
        label: "NativePHP",
        detect: |markers| {
            markers.requires_composer_package("nativephp/mobile")
                || (markers.composer.is_some() && markers.has_file("config/nativephp.php"))
        },
        ios_dirs: &["nativephp/ios"],
        android_dirs: &["nativephp/android"],
        javascript: false,
        web_assets: false,
        predict: None,
        missing_native: "This NativePHP project has no nativephp/ios or nativephp/android project yet. Run `php artisan native:install` in it and approve it again.",
    },
    Framework {
        kind: ProjectKind::Native,
        label: "Native",
        detect: |_| true,
        // Searched at the project folder, the conventional platform folders, and one level
        // down, since a native checkout keeps its projects wherever it likes.
        ios_dirs: &[],
        android_dirs: &[],
        javascript: false,
        web_assets: false,
        predict: None,
        missing_native: "This folder holds no Xcode workspace or project and no Gradle project. Choose the folder that contains the app's ios and android projects, or the .xcodeproj or settings.gradle itself.",
    },
];

pub(crate) fn framework(kind: ProjectKind) -> &'static Framework {
    FRAMEWORKS
        .iter()
        .find(|framework| framework.kind == kind)
        .expect("every kind has a row")
}

/// The first framework whose test the folder passes; `Native` matches anything, so there is
/// always one.
pub(crate) fn detect_kind(markers: &ProjectMarkers<'_>) -> ProjectKind {
    FRAMEWORKS
        .iter()
        .find(|framework| (framework.detect)(markers))
        .expect("the last row matches anything")
        .kind
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_exactly_one_row_and_native_is_last() {
        let mut kinds: Vec<ProjectKind> = FRAMEWORKS.iter().map(|f| f.kind).collect();
        let count = kinds.len();
        kinds.dedup();
        assert_eq!(kinds.len(), count, "a kind is described twice");
        assert_eq!(
            FRAMEWORKS.last().map(|f| f.kind),
            Some(ProjectKind::Native),
            "the catch-all row must be last"
        );
        for framework in FRAMEWORKS {
            assert!(!framework.label.is_empty());
            if framework.predict.is_none() {
                assert!(
                    !framework.missing_native.is_empty(),
                    "{} must say what to do about a missing native project",
                    framework.label
                );
            }
        }
    }

    #[test]
    fn a_laravel_app_is_nativephp_only_when_it_says_so() {
        let dependencies = BTreeSet::new();
        let composer = serde_json::json!({ "require": { "laravel/framework": "^11.0" } });
        let plain = ProjectMarkers {
            root: Path::new("/nonexistent"),
            package: None,
            dependencies: &dependencies,
            composer: Some(&composer),
            pubspec: None,
        };
        assert_eq!(detect_kind(&plain), ProjectKind::Native);

        let mobile = serde_json::json!({ "require": { "nativephp/mobile": "^1.0" } });
        let native_php = ProjectMarkers {
            composer: Some(&mobile),
            ..plain
        };
        assert_eq!(detect_kind(&native_php), ProjectKind::NativePhp);
        assert_eq!(
            framework(ProjectKind::NativePhp).ios_dirs,
            &["nativephp/ios"]
        );
        assert!(!framework(ProjectKind::NativePhp).javascript);
        assert!(framework(ProjectKind::NativePhp).predict.is_none());
        assert!(
            framework(ProjectKind::Expo).predict.is_some(),
            "Expo names its projects before prebuild writes them"
        );
    }
}
