import { describe, expect, it } from 'vite-plus/test';

import type { MachineView } from '../types/backend';
import { describeSnapshot } from './snapshot';

const now = Date.parse('2026-09-07T12:00:00Z');

function android(workspace: Record<string, unknown> | null, envSet: { name: string } | null) {
    return {
        profile: { provider: 'android_toolchain' },
        android: { workspace },
        envSet,
    } as unknown as MachineView;
}

describe('the saved snapshot in one line', () => {
    it('names the copy, its age, its size and the environment it was copied with', () => {
        const view = android(
            {
                lastSnapshotSha256:
                    '6c71e768dabeca9d740236c5e0073e72ba619d7efd23cd4bd38c9f017c23a52e',
                lastSyncedAtEpochSeconds: now / 1000 - 2 * 3600,
                lastSyncFileCount: 1396,
                lastSyncBytes: 28_278_463,
                lastSource: { kind: 'folder', gitRef: null, commit: null },
            },
            { name: 'Dev' },
        );
        expect(describeSnapshot(view, now)).toBe(
            'snapshot 6c71e768dabe · copied 2 hours ago · 1396 files · 28.3 MB · with Dev',
        );
    });

    it('leaves out what a record from before the time was kept cannot say', () => {
        const view = android(
            {
                lastSnapshotSha256: 'abcdef1234567890',
                lastSyncedAtEpochSeconds: null,
                lastSyncFileCount: 12,
                lastSyncBytes: 2048,
                lastSource: { kind: 'git', gitRef: 'main', commit: '0123456789abcdef' },
            },
            null,
        );
        expect(describeSnapshot(view, now)).toBe(
            'snapshot abcdef123456 · main at 0123456789ab · 12 files · 2.05 KB · project configuration only',
        );
    });

    it('is nothing until a snapshot exists', () => {
        expect(describeSnapshot(android({ lastSnapshotSha256: null }, null), now)).toBeNull();
        expect(describeSnapshot(android(null, null), now)).toBeNull();
    });
});
