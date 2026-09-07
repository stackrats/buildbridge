import { describe, expect, it } from 'vite-plus/test';

import type { MachineView } from '../types/backend';
import { retainedArtifacts } from './artifacts';

const file = (path: string, bytes: number) => ({ path, bytes, sha256: 'ab'.repeat(32) });

function view(partial: Record<string, unknown>): MachineView {
    return partial as unknown as MachineView;
}

describe('retained artifacts', () => {
    it("lists an Android machine's debug APK before its release files, and only what it holds", () => {
        const listed = retainedArtifacts(
            view({
                profile: { provider: 'android_toolchain' },
                android: {
                    workspace: { lastBuild: { apk: file('/a/app-debug.apk', 10) } },
                    release: { aab: file('/r/app.aab', 20), apk: null },
                },
            }),
        );
        expect(listed.map((artifact) => [artifact.id, artifact.file.path, artifact.step])).toEqual([
            ['debug-apk', '/a/app-debug.apk', 'test-build'],
            ['aab', '/r/app.aab', 'release'],
        ]);
        expect(
            retainedArtifacts(
                view({
                    profile: { provider: 'android_toolchain' },
                    android: { workspace: null, release: null },
                }),
            ),
        ).toEqual([]);
    });

    it("lists a macOS machine's IPA and archive together, or nothing", () => {
        const listed = retainedArtifacts(
            view({
                profile: { provider: 'docker_osx' },
                archive: { ipa: file('/x/App.ipa', 7), archive: file('/x/App.xcarchive.zip', 28) },
            }),
        );
        expect(listed.map((artifact) => [artifact.id, artifact.label, artifact.reveal])).toEqual([
            ['ipa', 'App Store IPA', 'archive'],
            ['archive', 'Xcode archive', 'archive'],
        ]);
        expect(
            retainedArtifacts(view({ profile: { provider: 'docker_osx' }, archive: null })),
        ).toEqual([]);
    });
});
