// What an approved folder holds, as the engine detected it: the framework in front of the
// native projects, and the Xcode container and Gradle module the builds use. The names here
// are the ones the pages print; the detection itself is the engine's.
import type { AndroidProject, IosProject, ProjectKind, ProjectLayout } from '../types/backend';

export const projectKindLabel: Record<ProjectKind, string> = {
    capacitor: 'Capacitor',
    cordova: 'Cordova',
    'react-native': 'React Native',
    expo: 'Expo',
    flutter: 'Flutter',
    'native-php': 'NativePHP',
    native: 'Native',
};

/** What stands in front of the native project, in a few words, for a facts line. */
export function describeProjectKind(layout: ProjectLayout): string {
    const kind = projectKindLabel[layout.kind];
    switch (layout.kind) {
        case 'capacitor':
        case 'cordova':
            return `${kind} · web assets built with ${layout.packageManager ?? 'npm'}`;
        case 'react-native':
        case 'expo':
            return `${kind} · JavaScript bundled by the native build, installed with ${layout.packageManager ?? 'npm'}`;
        case 'flutter':
            return `${kind} · Dart compiled by the Flutter tool`;
        case 'native-php':
            return `${kind} · a Laravel application inside a native shell`;
        default:
            return layout.ios && layout.android
                ? 'Native Xcode and Gradle projects'
                : layout.ios
                  ? 'Native Xcode project'
                  : 'Native Gradle project';
    }
}

/** Whether choosing an environment for a build means rebuilding web assets with it. */
export function hasWebAssets(layout: ProjectLayout | null | undefined): boolean {
    return layout?.kind === 'capacitor' || layout?.kind === 'cordova';
}

/** The iOS side's one-line description: what Xcode opens and which scheme it builds. */
export function describeIosProject(ios: IosProject): string {
    const container = ios.containerKind === 'project' ? 'project' : 'workspace';
    return `${ios.container} (${container}) · scheme ${ios.scheme}`;
}

/** The Android side's one-line description: the Gradle root and the module it builds. */
export function describeAndroidProject(android: AndroidProject): string {
    const root = android.root === '' ? 'Gradle at the project root' : android.root;
    return `${root} · module ${android.modulePath}`;
}

/** The layout every project had before detection existed, for previews and tests. */
export function capacitorLayout(): ProjectLayout {
    return {
        kind: 'capacitor',
        name: 'app',
        packageManager: 'pnpm',
        ios: {
            container: 'ios/App/App.xcworkspace',
            containerKind: 'workspace',
            project: 'ios/App/App.xcodeproj',
            scheme: 'App',
            schemes: ['App'],
            appTargets: [],
            podfileDir: 'ios/App',
            podfileLocked: true,
        },
        android: {
            root: 'android',
            moduleDir: 'app',
            modulePath: ':app',
            modules: [':app'],
            script: 'android/app/build.gradle',
            wrapper: true,
        },
    };
}
