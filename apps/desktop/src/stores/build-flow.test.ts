import { beforeEach, describe, expect, it, vi } from 'vite-plus/test';

import { createMockBackend } from '../lib/backend-mock';
import type { AndroidDeviceRun } from '../model/android-device';
import type { AndroidReleaseOutputs } from '../types/backend';

const machines = vi.hoisted(() => ({
    session: vi.fn(),
    debugBuild: vi.fn(),
    signedRelease: vi.fn(),
    sync: vi.fn(),
    runAndroidDevice: vi.fn(),
    cancelOperation: vi.fn(),
}));

vi.mock('./machines', () => ({ useMachinesStore: () => machines }));

describe('Android release file selection', () => {
    beforeEach(() => {
        vi.resetModules();
        vi.clearAllMocks();
    });

    it.each<AndroidReleaseOutputs>(['both', 'aab', 'apk'])(
        'keeps %s selected through the debug build and release operation',
        async (outputs) => {
            const view = await createMockBackend().getMachine('pixel-builder');
            machines.session.mockReturnValue({ view, operation: null, error: null });
            machines.debugBuild.mockResolvedValue({ view });
            machines.signedRelease.mockResolvedValue({ view });
            const { useBuildFlowStore } = await import('./build-flow');
            const store = useBuildFlowStore();
            expect(store.draft('pixel-builder').androidOutputs).toBe('both');

            await store.start(
                'pixel-builder',
                {
                    source: 'snapshot',
                    outcome: 'release',
                    target: 'device_sdk',
                    envSetId: null,
                    androidOutputs: outputs,
                    androidAllowHttp: true,
                },
                null,
            );

            expect(machines.debugBuild).toHaveBeenCalledWith('pixel-builder', false);
            // The release carries the version the project declares, so the artifact matches
            // what the panel showed even when the snapshot is older.
            expect(machines.signedRelease).toHaveBeenCalledWith('pixel-builder', null, outputs, {
                version: '3.2.0',
                build: '12',
            });
            expect(store.builds['pixel-builder']?.status).toBe('complete');
            expect(store.builds['pixel-builder']?.request.androidAllowHttp).toBe(false);
        },
    );
});

describe('Android build and run', () => {
    beforeEach(() => {
        vi.resetModules();
        vi.resetAllMocks();
    });

    async function fixture() {
        const view = await createMockBackend().getMachine('pixel-builder');
        const session = {
            view,
            operation: null,
            error: null as string | null,
            androidDevices: {
                available: true,
                devices: [{ serial: 'phone', state: 'device', model: null }],
                issue: null,
            },
            androidDeviceApk: 'release',
            androidDeviceRun: null as AndroidDeviceRun | null,
        };
        machines.session.mockReturnValue(session);
        machines.sync.mockResolvedValue({ view });
        machines.debugBuild.mockImplementation(async (_id, allowHttp = false) => {
            const build = structuredClone(view.android!.workspace!.lastBuild!);
            build.apk!.sha256 = 'fresh-apk';
            build.allowHttp = allowHttp;
            view.android!.workspace!.lastBuild = build;
            return { view, build };
        });
        machines.runAndroidDevice.mockResolvedValue(true);
        const { useBuildFlowStore } = await import('./build-flow');
        return { store: useBuildFlowStore(), session };
    }

    it('syncs and builds before installing the exact new debug APK', async () => {
        const { store, session } = await fixture();
        await store.startAndroidPreview('pixel-builder', 'phone');
        expect(machines.sync).toHaveBeenCalledWith('pixel-builder');
        expect(machines.debugBuild).toHaveBeenCalledWith('pixel-builder', false);
        expect(machines.runAndroidDevice).toHaveBeenCalledWith('pixel-builder', {
            kind: 'debug',
            serial: 'phone',
            expectedSha256: 'fresh-apk',
        });
        expect(store.builds['pixel-builder']?.request.source).toBe('latest');
        expect(store.builds['pixel-builder']?.status).toBe('complete');
        expect(session.androidDeviceApk).toBe('debug');
    });

    it('defaults the shared HTTP option off and forwards the choice into a fresh preview APK', async () => {
        const { store, session } = await fixture();
        const draft = store.draft('pixel-builder');
        expect(draft.androidAllowHttp).toBe(false);
        draft.androidAllowHttp = true;
        await store.startAndroidPreview('pixel-builder', 'phone');
        expect(machines.debugBuild).toHaveBeenCalledWith('pixel-builder', true);
        expect(store.builds['pixel-builder']?.request.androidAllowHttp).toBe(true);
        expect(session.view.android!.workspace!.lastBuild!.allowHttp).toBe(true);
        expect(machines.runAndroidDevice).toHaveBeenCalledWith('pixel-builder', {
            kind: 'debug',
            serial: 'phone',
            expectedSha256: 'fresh-apk',
        });
        draft.androidAllowHttp = false;
        expect(store.builds['pixel-builder']?.request.androidAllowHttp).toBe(true);
    });

    it('forwards the HTTP option for a standalone guided test build', async () => {
        const { store } = await fixture();
        await store.start(
            'pixel-builder',
            {
                ...store.draft('pixel-builder'),
                source: 'snapshot',
                outcome: 'test',
                androidAllowHttp: true,
            },
            null,
        );
        expect(machines.debugBuild).toHaveBeenCalledWith('pixel-builder', true);
        expect(store.builds['pixel-builder']?.status).toBe('complete');
        expect(machines.runAndroidDevice).not.toHaveBeenCalled();
    });

    it('clears the HTTP opt-in when the approved project changes', async () => {
        const { store, session } = await fixture();
        const draft = store.draft('pixel-builder');
        draft.androidAllowHttp = true;
        expect(store.draft('pixel-builder').androidAllowHttp).toBe(true);
        session.view.android!.workspace!.localPath = '/home/you/different-project';
        expect(store.draft('pixel-builder').androidAllowHttp).toBe(false);
        expect(draft.androidAllowHttp).toBe(false);
        await store.startAndroidPreview('pixel-builder', 'phone');
        expect(machines.debugBuild).toHaveBeenCalledWith('pixel-builder', false);
    });

    it('clears the HTTP opt-in when a paused preview continues with another project', async () => {
        const { store, session } = await fixture();
        store.draft('pixel-builder').androidAllowHttp = true;
        session.view.runtime.state = 'exited';
        await store.startAndroidPreview('pixel-builder', 'phone');
        expect(store.builds['pixel-builder']?.status).toBe('paused');
        session.view.android!.workspace!.localPath = '/home/you/different-project';
        session.view.runtime.state = 'running';
        await store.resume('pixel-builder');
        expect(machines.debugBuild).toHaveBeenCalledWith('pixel-builder', false);
        expect(store.builds['pixel-builder']?.request.androidAllowHttp).toBe(false);
    });

    it('does not install a retained APK when the fresh build fails', async () => {
        const { store, session } = await fixture();
        machines.debugBuild.mockImplementation(async () => {
            session.error = 'Gradle failed';
            return null;
        });
        await store.startAndroidPreview('pixel-builder', 'phone');
        expect(store.builds['pixel-builder']?.status).toBe('failed');
        session.error = null;
        expect(store.builds['pixel-builder']?.failure).toEqual({
            message: 'Gradle failed',
            step: 'test-build',
            artifactSha256: null,
            deviceSerial: null,
        });
        expect(machines.runAndroidDevice).not.toHaveBeenCalled();
    });

    it('reselects the fresh debug APK after sync temporarily leaves only a retained release', async () => {
        const { store, session } = await fixture();
        const build = structuredClone(session.view.android!.workspace!.lastBuild!);
        build.apk!.sha256 = 'fresh-after-sync';
        machines.sync.mockImplementation(async () => {
            session.view.android!.workspace!.lastBuild = null;
            // The APK picker falls back to the release while no debug record exists.
            session.androidDeviceApk = 'release';
            return { view: session.view };
        });
        machines.debugBuild.mockImplementation(async () => {
            session.view.android!.workspace!.lastBuild = build;
            return { view: session.view, build };
        });
        await store.startAndroidPreview('pixel-builder', 'phone');
        expect(session.androidDeviceApk).toBe('debug');
        expect(machines.runAndroidDevice).toHaveBeenCalledWith('pixel-builder', {
            kind: 'debug',
            serial: 'phone',
            expectedSha256: 'fresh-after-sync',
        });
        expect(store.builds['pixel-builder']?.androidPreviewSha256).toBe('fresh-after-sync');
    });

    it('does not install when build setup pauses', async () => {
        const { store, session } = await fixture();
        session.view.runtime.state = 'exited';
        await store.startAndroidPreview('pixel-builder', 'phone');
        expect(store.builds['pixel-builder']?.status).toBe('paused');
        expect(machines.runAndroidDevice).not.toHaveBeenCalled();
    });

    it('does not install a retained APK after the user stops the fresh build', async () => {
        const { store } = await fixture();
        let finishSync!: () => void;
        machines.sync.mockReturnValue(
            new Promise<void>((resolve) => {
                finishSync = resolve;
            }),
        );
        const running = store.startAndroidPreview('pixel-builder', 'phone');
        await store.stop('pixel-builder');
        finishSync();
        await running;
        expect(store.builds['pixel-builder']?.status).toBe('stopped');
        expect(machines.debugBuild).not.toHaveBeenCalled();
        expect(machines.runAndroidDevice).not.toHaveBeenCalled();
    });

    it('does not install an old APK when the successful build reports no new APK', async () => {
        const { store, session } = await fixture();
        const build = structuredClone(session.view.android!.workspace!.lastBuild!);
        build.apk = null;
        machines.debugBuild.mockResolvedValue({ view: session.view, build });
        await store.startAndroidPreview('pixel-builder', 'phone');
        expect(store.builds['pixel-builder']?.status).toBe('failed');
        expect(machines.runAndroidDevice).not.toHaveBeenCalled();
    });

    it('does not mark build and run complete when installation fails', async () => {
        const { store } = await fixture();
        machines.runAndroidDevice.mockResolvedValue(false);
        await store.startAndroidPreview('pixel-builder', 'phone');
        expect(store.builds['pixel-builder']?.status).toBe('failed');
    });

    it('retains the matching device install failure when refresh clears the shared error', async () => {
        const { store, session } = await fixture();
        const message =
            'INSTALL_FAILED_UPDATE_INCOMPATIBLE: nz.co.thinksolar.app.debug has an incompatible signing certificate.';
        machines.runAndroidDevice.mockImplementation(async (_id, input) => {
            session.androidDeviceRun = {
                kind: input.kind,
                serial: input.serial,
                sha256: input.expectedSha256,
                status: 'failed',
                result: null,
                error: message,
            };
            // A refresh can clear the shared banner error while the per-artifact run survives.
            session.error = null;
            return false;
        });
        await store.startAndroidPreview('pixel-builder', 'phone');
        const { useBuildFlowStore } = await import('./build-flow');
        const reopened = useBuildFlowStore();
        expect(reopened.builds['pixel-builder']?.failure).toEqual({
            message,
            step: 'run-device',
            artifactSha256: 'fresh-apk',
            deviceSerial: 'phone',
        });
        session.error = 'An unrelated later operation failed.';
        session.androidDeviceRun = null;
        session.view.android!.workspace!.lastBuild!.apk!.sha256 = 'replacement-apk';
        expect(reopened.builds['pixel-builder']?.failure?.message).toBe(message);
        expect(reopened.builds['pixel-builder']?.failure?.artifactSha256).toBe('fresh-apk');
    });

    it('does not attribute an older device failure to a new preview blocked before installation', async () => {
        const { store, session } = await fixture();
        session.androidDeviceRun = {
            kind: 'debug',
            serial: 'phone',
            sha256: 'fresh-apk',
            status: 'failed',
            result: null,
            error: 'An older installation failed.',
        };
        machines.runAndroidDevice.mockImplementation(async () => {
            session.error = 'Refresh devices and select an authorized phone or emulator first.';
            return false;
        });
        await store.startAndroidPreview('pixel-builder', 'phone');
        expect(store.builds['pixel-builder']?.failure?.message).toBe(session.error);
        expect(store.builds['pixel-builder']?.failure?.message).not.toContain('older');
    });

    it('keeps a failure only on its original flow when a subsequent preview succeeds', async () => {
        const { store, session } = await fixture();
        machines.runAndroidDevice.mockImplementationOnce(async () => {
            session.error = 'The device rejected the APK.';
            return false;
        });
        await store.startAndroidPreview('pixel-builder', 'phone');
        const previous = store.builds['pixel-builder'];
        expect(previous?.failure?.message).toBe('The device rejected the APK.');
        session.error = null;
        await store.startAndroidPreview('pixel-builder', 'phone');
        expect(store.builds['pixel-builder']?.status).toBe('complete');
        expect(store.builds['pixel-builder']?.failure).toBeNull();
        expect(previous?.failure?.message).toBe('The device rejected the APK.');
    });
});
