import { beforeEach, describe, expect, it, vi } from 'vite-plus/test';

import { createMockBackend } from '../lib/backend-mock';
import type { MachineView } from '../types/backend';

const backend = vi.hoisted(() => ({
    getMachine: vi.fn(),
    listMachines: vi.fn(),
    uploadAppleArchive: vi.fn<(machineId: string, expectedSha256: string) => Promise<void>>(),
    clearArchive: vi.fn(),
    cancelMachineOperation: vi.fn(),
}));
vi.mock('../lib/backend', () => ({ useBackend: () => backend }));

function deferred() {
    let resolve!: () => void;
    let reject!: (reason: Error) => void;
    const promise = new Promise<void>((accept, refuse) => {
        resolve = accept;
        reject = refuse;
    });
    return { promise, resolve, reject };
}

async function fixture() {
    const preview = createMockBackend();
    const view = await preview.getMachine('default');
    backend.getMachine.mockResolvedValue(view);
    backend.listMachines.mockResolvedValue(await preview.listMachines());
    const { useMachinesStore } = await import('./machines');
    const store = useMachinesStore();
    await store.refreshMachine('default');
    return { store, session: store.session('default'), view };
}

describe('Transporter upload state', () => {
    beforeEach(() => {
        vi.resetModules();
        vi.resetAllMocks();
    });

    it('uploads the reviewed IPA hash and keeps pending and success state across navigation', async () => {
        const { store, session, view } = await fixture();
        const request = deferred();
        backend.uploadAppleArchive.mockReturnValue(request.promise);
        const uploading = store.uploadAppleArchive('default');
        expect(backend.uploadAppleArchive).toHaveBeenCalledWith(
            'default',
            view.archive!.ipa.sha256,
        );
        expect(session.operation).toBe('upload-archive');
        expect(store.session('default').archiveUpload?.status).toBe('uploading');
        expect(session.archiveUpload?.status).not.toBe('uploaded');
        expect(await store.uploadAppleArchive('default')).toBe(false);
        expect(backend.uploadAppleArchive).toHaveBeenCalledTimes(1);

        request.resolve();
        expect(await uploading).toBe(true);
        expect(store.session('default').archiveUpload?.status).toBe('uploaded');
        expect(session.operation).toBeNull();
        expect(store.journey('default').find((step) => step.id === 'publish')?.status).toBe(
            'active',
        );
    });

    it.each<[string, (view: MachineView) => void]>([
        [
            'missing IPA',
            (view) => {
                view.archive = null;
            },
        ],
        [
            'missing Team key',
            (view) => {
                view.signingKit!.appStoreConnectConfigured = false;
            },
        ],
        [
            'stopped guest',
            (view) => {
                view.runtime.state = 'exited';
            },
        ],
        [
            'untrusted guest',
            (view) => {
                view.guest.ssh.trust = 'mismatch';
            },
        ],
        [
            'busy backend',
            (view) => {
                view.busyOperation = 'archiving';
            },
        ],
    ])('never invokes upload with %s', async (_name, change) => {
        const { store, session } = await fixture();
        change(session.view!);
        expect(await store.uploadAppleArchive('default')).toBe(false);
        expect(backend.uploadAppleArchive).not.toHaveBeenCalled();
        expect(session.archiveUpload).toBeNull();
        expect(session.error).toBeTruthy();
    });

    it('retains an actionable failure and only reports success after a successful retry', async () => {
        const { store, session } = await fixture();
        backend.uploadAppleArchive.mockRejectedValueOnce(
            new Error('Transporter could not connect to Apple.'),
        );
        expect(await store.uploadAppleArchive('default')).toBe(false);
        expect(session.archiveUpload?.status).toBe('failed');
        expect(session.archiveUpload?.error).toContain('could not connect');
        expect(session.lastFailure?.operation).toBe('upload-archive');

        backend.uploadAppleArchive.mockResolvedValueOnce();
        expect(await store.uploadAppleArchive('default')).toBe(true);
        expect(session.archiveUpload?.status).toBe('uploaded');
        expect(session.archiveUpload?.error).toBeNull();
    });

    it('does not mark cancellation as successful delivery', async () => {
        const { store, session } = await fixture();
        const request = deferred();
        backend.uploadAppleArchive.mockReturnValue(request.promise);
        backend.cancelMachineOperation.mockImplementation(() => {
            request.reject(new Error('Stopped.'));
            return Promise.resolve();
        });
        const uploading = store.uploadAppleArchive('default');
        await store.cancelOperation('default');
        expect(await uploading).toBe(false);
        expect(session.archiveUpload?.status).toBe('failed');
        expect(session.archiveUpload?.error).toContain('Check App Store Connect before retrying');
    });

    it('clears success when a different retained IPA is loaded', async () => {
        const { store, session } = await fixture();
        backend.uploadAppleArchive.mockResolvedValueOnce();
        await store.uploadAppleArchive('default');
        const changed = await createMockBackend().getMachine('default');
        changed.archive!.ipa.sha256 = 'ff'.repeat(32);
        backend.getMachine.mockResolvedValue(changed);
        await store.refreshMachine('default');
        expect(session.archiveUpload).toBeNull();
    });

    it('does not apply a late success to a replaced IPA', async () => {
        const { store, session } = await fixture();
        const request = deferred();
        backend.uploadAppleArchive.mockReturnValue(request.promise);
        const uploading = store.uploadAppleArchive('default');
        const changed = await createMockBackend().getMachine('default');
        changed.archive!.ipa.sha256 = 'ff'.repeat(32);
        backend.getMachine.mockResolvedValue(changed);
        await store.refreshMachine('default', { silent: true });
        request.resolve();
        await uploading;
        expect(session.archiveUpload).toBeNull();
    });
});
