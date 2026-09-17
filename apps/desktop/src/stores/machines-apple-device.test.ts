import { beforeEach, describe, expect, it, vi } from 'vite-plus/test';

import { createMockBackend } from '../lib/backend-mock';
import type { AppleDeviceRunResult, RunAppleDeviceResult } from '../types/backend';

const backend = vi.hoisted(() => ({
    getMachine: vi.fn(),
    listMachines: vi.fn(),
    runAppleDeviceBuild: vi.fn(),
}));
vi.mock('../lib/backend', () => ({ useBackend: () => backend }));

async function fixture() {
    const preview = createMockBackend();
    const view = await preview.getMachine('default');
    view.runtime.state = 'exited';
    backend.getMachine.mockResolvedValue(view);
    backend.listMachines.mockResolvedValue(await preview.listMachines());
    const { useMachinesStore } = await import('./machines');
    const store = useMachinesStore();
    await store.refreshMachine('default');
    const session = store.session('default');
    const run: AppleDeviceRunResult = {
        liveReloadUrl: 'http://192.168.1.10:5173',
        device: {
            identifier: 'phone',
            udid: 'phone',
            name: 'iPhone',
            osVersion: '18.6',
            model: 'iPhone',
            developerMode: 'enabled',
            pairingState: 'paired',
            tunnelState: 'connected',
            transportType: 'wired',
            ready: true,
            issue: null,
        },
        bundleIdentifier: 'com.example.app',
        projectBundleIdentifier: null,
        appPath: '/Users/builder/App.app',
        marketingVersion: '3.2.0',
        buildNumber: '15',
        provisioningProfileUuid: 'profile',
        installedAtEpochSeconds: 1_756_900_000,
        consoleEnd: 'stopped',
        exitStatus: null,
        reattached: false,
        buildTail: [],
        consoleTail: ['Capacitor: loading app'],
    };
    return { store, session, view, run };
}

beforeEach(() => {
    vi.resetModules();
    vi.resetAllMocks();
});

describe('iPhone build and run live reload', () => {
    it('forwards the trimmed URL, environment and version and retains actual run metadata', async () => {
        const { store, session, view, run } = await fixture();
        backend.runAppleDeviceBuild.mockResolvedValue({ view: { ...view, deviceRun: run }, run });
        const version = { version: '4.0', build: '40' };

        await store.runOnDevice(
            session.id,
            'phone',
            'development',
            version,
            `  ${run.liveReloadUrl}  `,
        );

        expect(backend.runAppleDeviceBuild).toHaveBeenCalledWith(
            session.id,
            'phone',
            'development',
            version,
            run.liveReloadUrl,
        );
        expect(session.view?.deviceRun?.liveReloadUrl).toBe(run.liveReloadUrl);
        expect(session.deviceLog).toEqual([{ text: 'Capacitor: loading app' }]);
    });

    it.each(['http://localhost:5173', '', 'http://user:secret@192.168.1.10:5173'])(
        'rejects %s before changing the operation or previous console',
        async (url) => {
            const { store, session } = await fixture();
            session.deviceLog = [{ text: 'Previous console' }];
            expect(await store.runOnDevice(session.id, 'phone', null, null, url)).toBeNull();
            expect(backend.runAppleDeviceBuild).not.toHaveBeenCalled();
            expect(session.operation).toBeNull();
            expect(session.error).not.toBeNull();
            expect(session.deviceLog).toEqual([{ text: 'Previous console' }]);
        },
    );

    it('does not inherit the previous live reload URL for an ordinary run', async () => {
        const { store, session, view, run } = await fixture();
        session.deviceLiveReloadUrl = run.liveReloadUrl;
        backend.runAppleDeviceBuild.mockResolvedValue({
            view,
            run: { ...run, liveReloadUrl: null },
        });
        await store.runOnDevice(session.id, 'phone');
        expect(backend.runAppleDeviceBuild).toHaveBeenCalledWith(
            session.id,
            'phone',
            null,
            null,
            null,
        );
        expect(session.deviceLiveReloadUrl).toBeNull();
    });

    it('keeps the active URL and console when a second run is attempted', async () => {
        const { store, session, view, run } = await fixture();
        let resolve!: (value: RunAppleDeviceResult) => void;
        backend.runAppleDeviceBuild.mockReturnValue(
            new Promise<RunAppleDeviceResult>((accept) => {
                resolve = accept;
            }),
        );
        const running = store.runOnDevice(session.id, 'phone', null, null, run.liveReloadUrl);
        session.deviceLog.push({ text: 'Current console' });
        await store.runOnDevice(session.id, 'phone', null, null, 'https://dev.example.test');
        expect(backend.runAppleDeviceBuild).toHaveBeenCalledTimes(1);
        expect(session.deviceLiveReloadUrl).toBe(run.liveReloadUrl);
        expect(session.deviceLog).toEqual([{ text: 'Current console' }]);
        resolve({ view, run });
        await running;
    });
});
