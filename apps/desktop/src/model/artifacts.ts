// Every file a machine has retained on this host, in the order it was made, so the page can
// list them under one heading with what each one is for.
import type { AndroidArtifact, AppleArchiveArtifact, MachineView } from '../types/backend';
import { isAndroid } from './providers';
import type { JourneyStepId } from './steps';

export interface RetainedArtifact {
    id: 'debug-apk' | 'aab' | 'apk' | 'ipa' | 'archive';
    label: string;
    /** What the file is for, in a few words. */
    destination: string;
    file: AndroidArtifact | AppleArchiveArtifact;
    /** The step whose panel holds the file's details and actions. */
    step: JourneyStepId;
    /** The reveal action that opens the folder holding it. */
    reveal: 'debug' | 'release' | 'archive';
}

export function retainedArtifacts(view: MachineView): RetainedArtifact[] {
    const artifacts: RetainedArtifact[] = [];
    if (isAndroid(view.profile.provider)) {
        const debug = view.android?.workspace?.lastBuild?.apk ?? null;
        const release = view.android?.release ?? null;
        if (debug) {
            artifacts.push({
                id: 'debug-apk',
                label: 'Debug APK',
                destination: 'Phone or emulator, for testing',
                file: debug,
                step: 'test-build',
                reveal: 'debug',
            });
        }
        if (release?.aab) {
            artifacts.push({
                id: 'aab',
                label: 'App bundle (AAB)',
                destination: 'Google Play',
                file: release.aab,
                step: 'release',
                reveal: 'release',
            });
        }
        if (release?.apk) {
            artifacts.push({
                id: 'apk',
                label: 'Release APK',
                destination: 'Direct installation',
                file: release.apk,
                step: 'release',
                reveal: 'release',
            });
        }
        return artifacts;
    }
    if (view.archive) {
        artifacts.push({
            id: 'ipa',
            label: 'App Store IPA',
            destination: 'App Store Connect',
            file: view.archive.ipa,
            step: 'archive',
            reveal: 'archive',
        });
        artifacts.push({
            id: 'archive',
            label: 'Xcode archive',
            destination: 'Symbols and re-export, zipped',
            file: view.archive.archive,
            step: 'archive',
            reveal: 'archive',
        });
    }
    return artifacts;
}
