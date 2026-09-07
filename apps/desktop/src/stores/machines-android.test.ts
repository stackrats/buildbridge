import { beforeEach, describe, expect, it, vi } from 'vite-plus/test';

import { createMockBackend } from '../lib/backend-mock';
import type {
    AndroidDeviceRunResult,
    AndroidDevices,
    GooglePlayUploadResult,
    RunAndroidBuildResult,
    RunAndroidDeviceResult,
} from '../types/backend';

const backend = vi.hoisted(() => ({
    getMachine: vi.fn(),
    listMachines: vi.fn(),
    listAndroidDevices: vi.fn(),
    runAndroidDevice: vi.fn(),
    runAndroidDebugBuild: vi.fn(),
    googlePlayConnection: vi.fn(),
    configureGooglePlay: vi.fn(),
    disconnectGooglePlay: vi.fn(),
    uploadGooglePlay: vi.fn(),
}));
vi.mock('../lib/backend', () => ({ useBackend: () => backend }));

function deferred<T>() {
    let resolve!: (result: T) => void;
    let reject!: (cause: Error) => void;
    const promise = new Promise<T>((accept, refuse) => {
        resolve = accept;
        reject = refuse;
    });
    return { promise, resolve, reject };
}
async function fixture() {
    const preview = createMockBackend();
    const view = await preview.getMachine('pixel-builder');
    view.runtime.state = 'exited';
    backend.getMachine.mockResolvedValue(view);
    backend.listMachines.mockResolvedValue(await preview.listMachines());
    backend.listAndroidDevices.mockResolvedValue({
        available: true,
        issue: null,
        devices: [{ serial: 'phone', state: 'device', model: 'Pixel', network: null }],
        hostNetworks: [],
    });
    backend.googlePlayConnection.mockResolvedValue({
        configured: true,
        clientEmail: 'account@example.iam.gserviceaccount.com',
        projectId: 'example',
    });
    const { useMachinesStore } = await import('./machines');
    const store = useMachinesStore();
    await store.refreshMachine('pixel-builder');
    await store.refreshAndroidDevices('pixel-builder');
    await store.loadGooglePlayConnection('pixel-builder');
    const session = store.session('pixel-builder');
    const input = {
        kind: 'debug' as const,
        serial: 'phone',
        expectedSha256: view.android!.workspace!.lastBuild!.apk!.sha256,
    };
    const deviceResult: AndroidDeviceRunResult = {
        serial: 'phone',
        model: 'Pixel',
        sha256: input.expectedSha256,
        applicationId: view.android!.workspace!.lastBuild!.applicationId,
        installed: true,
        launched: true,
        pid: 4242,
        installedAtEpochSeconds: 1_756_900_000,
        consoleEnd: 'stopped',
        consoleTail: ['I/Capacitor( 4242): Starting BridgeActivity'],
    };
    // The run ends with the machine's fresh view, as every operation that changes it does.
    const deviceRun: RunAndroidDeviceResult = { view, run: deviceResult };
    const uploadResult: GooglePlayUploadResult = {
        packageName: view.android!.release!.applicationId,
        versionCode: view.android!.release!.versionCode,
        track: 'internal',
        status: 'draft',
        sha256: view.android!.release!.aab!.sha256,
    };
    return { store, session, view, input, deviceResult, deviceRun, uploadResult };
}

beforeEach(() => {
    vi.resetModules();
    vi.resetAllMocks();
});

describe('Android host device automation', () => {
    it.each([false, true])(
        'forwards the debug HTTP override %s to the backend',
        async (allowHttp) => {
            const { store, session, view } = await fixture();
            const build = { ...view.android!.workspace!.lastBuild!, allowHttp };
            backend.runAndroidDebugBuild.mockResolvedValue({ view, build });
            if (allowHttp) await store.debugBuild(session.id, true);
            else await store.debugBuild(session.id);
            expect(backend.runAndroidDebugBuild).toHaveBeenCalledWith(session.id, allowHttp, null);
        },
    );

    it('clears an earlier success log when a new Android build fails', async () => {
        const { store, session } = await fixture();
        session.buildLog = [{ text: 'BUILD SUCCESSFUL from an earlier build' }];
        backend.runAndroidDebugBuild.mockRejectedValue(new Error('The APK could not be copied.'));

        expect(await store.debugBuild(session.id)).toBeNull();

        expect(session.buildLog).toEqual([]);
        expect(session.lastFailure?.message).toBe('The APK could not be copied.');
    });

    it('preserves the current Android build log when a duplicate start is rejected', async () => {
        const { store, session, view } = await fixture();
        const request = deferred<RunAndroidBuildResult>();
        backend.runAndroidDebugBuild.mockReturnValue(request.promise);
        session.buildLog = [{ text: 'Previous build output' }];

        const building = store.debugBuild(session.id);
        expect(session.buildLog).toEqual([]);
        session.buildLog.push({ text: 'Current build output' });

        expect(await store.debugBuild(session.id)).toBeNull();
        expect(session.buildLog).toEqual([{ text: 'Current build output' }]);
        expect(backend.runAndroidDebugBuild).toHaveBeenCalledTimes(1);

        request.resolve({
            view,
            build: { ...view.android!.workspace!.lastBuild!, outputTail: ['BUILD SUCCESSFUL'] },
        });
        await building;
        expect(session.buildLog).toEqual([
            { text: 'Current build output' },
            { text: 'BUILD SUCCESSFUL' },
        ]);
    });

    it('installs the reviewed APK with the container stopped and survives navigation', async () => {
        const { store, session, input, deviceRun } = await fixture();
        const request = deferred<RunAndroidDeviceResult>();
        backend.runAndroidDevice.mockReturnValue(request.promise);
        const installing = store.runAndroidDevice(session.id, input);
        expect(backend.runAndroidDevice).toHaveBeenCalledWith(session.id, input);
        expect(store.session(session.id).androidDeviceRun?.status).toBe('installing');
        expect(await store.runAndroidDevice(session.id, input)).toBe(false);
        expect(backend.runAndroidDevice).toHaveBeenCalledTimes(1);
        request.resolve(deviceRun);
        expect(await installing).toBe(true);
        expect(store.session(session.id).androidDeviceRun?.status).toBe('complete');
        // A run is an activity, not an achievement: the step is available again, not done.
        expect(store.journey(session.id).find((step) => step.id === 'run-device')?.status).toBe(
            'active',
        );
        // The log the run streamed is kept; a session that saw no events takes the tail.
        expect(store.session(session.id).deviceLog).toEqual([
            { text: 'I/Capacitor( 4242): Starting BridgeActivity' },
        ]);
    });
    it.each(['missing ADB', 'unauthorized device', 'changed APK', 'busy backend'])(
        'does not install with %s',
        async (problem) => {
            const { store, session, input } = await fixture();
            if (problem === 'missing ADB') session.androidDevices!.available = false;
            if (problem === 'unauthorized device')
                session.androidDevices!.devices[0]!.state = 'unauthorized';
            if (problem === 'changed APK')
                session.view!.android!.workspace!.lastBuild!.apk!.sha256 = 'changed';
            if (problem === 'busy backend') session.view!.busyOperation = 'releasing';
            expect(await store.runAndroidDevice(session.id, input)).toBe(false);
            expect(backend.runAndroidDevice).not.toHaveBeenCalled();
            expect(session.androidDeviceRun).toBeNull();
        },
    );
    it('requires launch confirmation before marking a run successful', async () => {
        const { store, session, input, deviceRun } = await fixture();
        backend.runAndroidDevice.mockResolvedValue({
            ...deviceRun,
            run: { ...deviceRun.run, launched: false },
        });
        expect(await store.runAndroidDevice(session.id, input)).toBe(false);
        expect(session.androidDeviceRun?.status).toBe('failed');
        expect(store.journey(session.id).find((step) => step.id === 'run-device')?.status).toBe(
            'failed',
        );
    });
    it('invalidates completion when the retained APK changes', async () => {
        const { store, session, input, deviceRun } = await fixture();
        backend.runAndroidDevice.mockResolvedValue(deviceRun);
        await store.runAndroidDevice(session.id, input);
        const changed = await createMockBackend().getMachine(session.id);
        changed.android!.workspace!.lastBuild!.apk!.sha256 = 'replacement';
        backend.getMachine.mockResolvedValue(changed);
        await store.refreshMachine(session.id);
        expect(session.androidDeviceRun).toBeNull();
        expect(store.journey(session.id).find((step) => step.id === 'run-device')?.status).toBe(
            'active',
        );
    });
    it('clears a stale device selection when it becomes unauthorized', async () => {
        const { store, session } = await fixture();
        expect(session.androidDeviceSerial).toBe('phone');
        backend.listAndroidDevices.mockResolvedValue({
            available: true,
            issue: null,
            devices: [{ serial: 'phone', state: 'unauthorized', model: null, network: null }],
            hostNetworks: [],
        });
        await store.refreshAndroidDevices(session.id);
        expect(session.androidDeviceSerial).toBe('');
    });
    it('probes quietly, and a refresh asked during the probe shares its answer', async () => {
        const { store, session } = await fixture();
        const listing = deferred<AndroidDevices>();
        backend.listAndroidDevices.mockClear();
        backend.listAndroidDevices.mockReturnValue(listing.promise);
        const probe = store.refreshAndroidDevices(session.id, { quiet: true });
        expect(session.androidDevicesLoading).toBe(false);
        const refresh = store.refreshAndroidDevices(session.id);
        expect(session.androidDevicesLoading).toBe(true);
        expect(backend.listAndroidDevices).toHaveBeenCalledTimes(1);
        listing.resolve({
            available: true,
            issue: null,
            devices: [{ serial: 'second', state: 'device', model: 'Pixel', network: null }],
            hostNetworks: [],
        });
        await Promise.all([probe, refresh]);
        expect(session.androidDevicesLoading).toBe(false);
        expect(session.androidDevices?.devices.map((device) => device.serial)).toEqual(['second']);
        expect(session.androidDeviceSerial).toBe('second');
    });
    it('keeps a failed listing as the diagnostic until a refresh succeeds', async () => {
        const { store, session } = await fixture();
        backend.listAndroidDevices.mockRejectedValueOnce(new Error('adb hung'));
        await store.refreshAndroidDevices(session.id, { quiet: true });
        expect(session.androidDevices).toBeNull();
        expect(session.androidDevicesError).toContain('adb hung');
        await store.refreshAndroidDevices(session.id);
        expect(session.androidDevicesError).toBeNull();
        expect(session.androidDevices?.devices).toHaveLength(1);
    });
});

describe('Google Play draft upload', () => {
    it('uploads the exact AAB with the container stopped and retains draft status', async () => {
        const { store, session, uploadResult } = await fixture();
        const request = deferred<GooglePlayUploadResult>();
        backend.uploadGooglePlay.mockReturnValue(request.promise);
        const uploading = store.uploadGooglePlay(session.id);
        expect(backend.uploadGooglePlay).toHaveBeenCalledWith(session.id, uploadResult.sha256);
        expect(store.session(session.id).googlePlayUpload?.status).toBe('uploading');
        expect(await store.uploadGooglePlay(session.id)).toBe(false);
        request.resolve(uploadResult);
        expect(await uploading).toBe(true);
        expect(store.session(session.id).googlePlayUpload?.result?.status).toBe('draft');
        expect(store.journey(session.id).find((step) => step.id === 'publish')?.status).toBe(
            'active',
        );
    });
    it.each(['no credentials', 'no AAB', 'busy backend'])(
        'never invokes upload with %s',
        async (problem) => {
            const { store, session } = await fixture();
            if (problem === 'no credentials') session.googlePlayConnection = null;
            if (problem === 'no AAB') session.view!.android!.release!.aab = null;
            if (problem === 'busy backend') session.view!.busyOperation = 'releasing';
            expect(await store.uploadGooglePlay(session.id)).toBe(false);
            expect(backend.uploadGooglePlay).not.toHaveBeenCalled();
        },
    );
    it('preserves failure details and allows explicit retry', async () => {
        const { store, session, uploadResult } = await fixture();
        backend.uploadGooglePlay.mockRejectedValueOnce(
            new Error('Grant this service account access to the app.'),
        );
        expect(await store.uploadGooglePlay(session.id)).toBe(false);
        expect(session.googlePlayUpload?.status).toBe('failed');
        expect(session.googlePlayUpload?.error).toContain('Grant this service account');
        backend.uploadGooglePlay.mockResolvedValue(uploadResult);
        expect(await store.uploadGooglePlay(session.id)).toBe(true);
        expect(session.googlePlayUpload?.status).toBe('uploaded');
    });
    it('does not attach a late upload success to a replacement AAB', async () => {
        const { store, session, uploadResult } = await fixture();
        const request = deferred<GooglePlayUploadResult>();
        backend.uploadGooglePlay.mockReturnValue(request.promise);
        const uploading = store.uploadGooglePlay(session.id);
        const changed = await createMockBackend().getMachine(session.id);
        changed.android!.release!.aab!.sha256 = 'replacement';
        backend.getMachine.mockResolvedValue(changed);
        await store.refreshMachine(session.id, { silent: true });
        request.resolve(uploadResult);
        await uploading;
        expect(session.googlePlayUpload).toBeNull();
    });
    it('passes only a file path for credentials and clears connection metadata on disconnect', async () => {
        const { store, session } = await fixture();
        backend.configureGooglePlay.mockResolvedValue({
            configured: true,
            clientEmail: 'new@example.iam.gserviceaccount.com',
            projectId: 'example',
        });
        await store.configureGooglePlay(session.id, '/tmp/service-account.json');
        expect(backend.configureGooglePlay).toHaveBeenCalledWith(
            session.id,
            '/tmp/service-account.json',
        );
        expect(session.googlePlayConnection?.clientEmail).toBe(
            'new@example.iam.gserviceaccount.com',
        );
        await store.disconnectGooglePlay(session.id);
        expect(session.googlePlayConnection).toEqual({
            configured: false,
            clientEmail: null,
            projectId: null,
        });
    });
});
