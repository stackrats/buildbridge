import { describe, expect, it } from 'vite-plus/test';

import { createMockBackend } from '../lib/backend-mock';
import type { MachineView } from '../types/backend';
import { appleUploadBlocker } from './apple-upload';

describe('Transporter upload readiness', () => {
    it('accepts a retained IPA and Team key without reprovisioning signing or selecting Xcode', async () => {
        const view = await createMockBackend().getMachine('default');
        view.signing = null;
        view.signingHealth = 'unconfigured';
        view.guest.diagnostics.xcodeSelected = false;
        expect(appleUploadBlocker(view)).toBeNull();
    });

    it.each<[string, (view: MachineView) => void, string | null]>([
        [
            'missing IPA',
            (view) => {
                view.archive = null;
            },
            null,
        ],
        [
            'missing credentials',
            (view) => {
                view.signingKit = null;
            },
            'credentials',
        ],
        [
            'certificate without Team key',
            (view) => {
                view.signingKit!.appStoreConnectConfigured = false;
            },
            'credentials',
        ],
        [
            'inaccessible vault',
            (view) => {
                view.vaultIssue = 'Unlock the credential vault.';
            },
            'credentials',
        ],
        [
            'stopped macOS',
            (view) => {
                view.runtime.state = 'exited';
            },
            'start',
        ],
        [
            'unreachable guest',
            (view) => {
                view.guest.ssh.reachable = false;
            },
            'guest',
        ],
        [
            'untrusted guest',
            (view) => {
                view.guest.ssh.trust = 'untrusted';
            },
            'guest',
        ],
        [
            'changed guest identity',
            (view) => {
                view.guest.ssh.trust = 'mismatch';
            },
            'guest',
        ],
        [
            'unauthorized guest',
            (view) => {
                view.guest.diagnostics.authenticated = false;
            },
            'guest',
        ],
        [
            'another process building',
            (view) => {
                view.busyOperation = 'archiving';
            },
            null,
        ],
    ])('blocks %s with an actionable reason', async (_name, change, action) => {
        const view = await createMockBackend().getMachine('default');
        change(view);
        const blocker = appleUploadBlocker(view);
        expect(blocker?.message).toBeTruthy();
        expect(blocker?.action).toBe(action);
    });

    it('blocks a local operation and Android machines', async () => {
        const backend = createMockBackend();
        expect(appleUploadBlocker(await backend.getMachine('default'), true)).not.toBeNull();
        expect(appleUploadBlocker(await backend.getMachine('pixel-builder'))).not.toBeNull();
    });
});
