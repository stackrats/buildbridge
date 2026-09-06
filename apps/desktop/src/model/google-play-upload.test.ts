import { describe, expect, it } from 'vite-plus/test';

import { createMockBackend } from '../lib/backend-mock';
import type { MachineView } from '../types/backend';
import { googlePlayUploadBlocker } from './google-play-upload';

async function androidView(): Promise<MachineView> {
    return createMockBackend().getMachine('pixel-builder');
}

describe('Google Play upload blocker', () => {
    it('asks for an Android machine before anything else', async () => {
        expect(googlePlayUploadBlocker(null, true, false)).toBe('Choose an Android machine first.');
        const mac = await createMockBackend().getMachine('default');
        expect(googlePlayUploadBlocker(mac, true, false)).toBe('Choose an Android machine first.');
    });

    it('waits for the running operation whether this client or the backend reports it', async () => {
        const view = await androidView();
        expect(googlePlayUploadBlocker(view, true, true)).toBe(
            'Wait for the current operation to finish.',
        );
        expect(googlePlayUploadBlocker({ ...view, busyOperation: 'releasing' }, true, false)).toBe(
            'Wait for the current operation to finish.',
        );
    });

    it('needs a retained AAB, then credentials, before the upload is allowed', async () => {
        const view = await androidView();
        expect(view.android?.release?.aab).toBeTruthy();
        const noRelease: MachineView = { ...view, android: { ...view.android!, release: null } };
        expect(googlePlayUploadBlocker(noRelease, true, false)).toBe(
            'Build a signed AAB before uploading to Google Play.',
        );
        const apkOnly: MachineView = {
            ...view,
            android: { ...view.android!, release: { ...view.android!.release!, aab: null } },
        };
        expect(googlePlayUploadBlocker(apkOnly, true, false)).toBe(
            'Build a signed AAB before uploading to Google Play.',
        );
        expect(googlePlayUploadBlocker(view, false, false)).toBe(
            'Import a Google Play service account JSON key with access to this app.',
        );
        expect(googlePlayUploadBlocker(view, true, false)).toBeNull();
    });
});
