import { beforeEach, describe, expect, it, vi } from 'vite-plus/test';

import type { DesktopStatus, RealtimeConfiguration, RunOnceResult } from '../types/backend';

const backend = vi.hoisted(() => ({
    getRunnerStatus: vi.fn<() => Promise<DesktopStatus>>(),
    runOnce: vi.fn<() => Promise<RunOnceResult>>(),
    onRunnerActivity: vi.fn<() => Promise<() => void>>(),
    getRealtimeConfiguration: vi.fn<() => Promise<RealtimeConfiguration>>(),
}));
vi.mock('../lib/backend', () => ({ useBackend: () => backend }));
// Enough of a Pusher for the store to bind its handlers to; the socket itself is not the
// subject of these tests, only whether the store opens one at all.
vi.mock('pusher-js', () => ({
    default: class {
        connection = { bind: vi.fn() };
        subscribe = () => ({ bind: vi.fn() });
        disconnect = vi.fn();
    },
}));

function status(paired: boolean): DesktopStatus {
    return {
        paired,
        credentialsMissing: false,
        serverUrl: paired ? 'https://control.example.test' : null,
        runnerId: paired ? 'runner-1' : null,
        runnerName: paired ? 'linux-builder' : null,
        platform: 'linux',
        architecture: 'x86_64',
        version: '0.1.0',
    };
}

function result(state: RunOnceResult['state'], buildId: string | null = null): RunOnceResult {
    return { state, buildId, message: `build ${buildId ?? 'none'} ${state}` };
}

function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((accept) => {
        resolve = accept;
    });
    return { promise, resolve };
}

async function pairedStore(paired = true) {
    backend.getRunnerStatus.mockResolvedValue(status(paired));
    const { useRunnerStore } = await import('./runner');
    const store = useRunnerStore();
    await store.refresh();
    return store;
}

describe('runner claim loop', () => {
    beforeEach(() => {
        vi.resetModules();
        vi.resetAllMocks();
    });

    it('claims until the control plane reports nothing queued, logging each outcome newest first', async () => {
        backend.runOnce
            .mockResolvedValueOnce(result('completed', '12'))
            .mockResolvedValueOnce(result('failed', '13'))
            .mockResolvedValueOnce(result('idle'));
        const store = await pairedStore();
        await store.checkForWork();
        expect(backend.runOnce).toHaveBeenCalledTimes(3);
        expect(store.state.activity.map((entry) => [entry.tone, entry.buildId])).toEqual([
            ['danger', '13'],
            ['success', '12'],
        ]);
        expect(store.state.checking).toBe(false);
        expect(store.state.error).toBeNull();
    });

    it('stops at a busy machine and folds requests that arrive mid-drain into one more pass', async () => {
        const first = deferred<RunOnceResult>();
        backend.runOnce.mockReturnValueOnce(first.promise).mockResolvedValueOnce(result('idle'));
        const store = await pairedStore();
        const drain = store.checkForWork();
        expect(store.state.checking).toBe(true);
        await store.checkForWork();
        await store.checkForWork();
        expect(backend.runOnce).toHaveBeenCalledTimes(1);
        first.resolve(result('busy'));
        await drain;
        await vi.waitFor(() => expect(store.state.checking).toBe(false));
        expect(backend.runOnce).toHaveBeenCalledTimes(2);
        expect(store.state.activity).toEqual([]);
    });

    it('records a failed claim as an error without leaving the loop stuck', async () => {
        backend.runOnce.mockRejectedValueOnce(new Error('control plane returned 503'));
        const store = await pairedStore();
        await store.checkForWork();
        expect(store.state.error).toBe('control plane returned 503');
        expect(store.state.activity.map((entry) => entry.tone)).toEqual(['danger']);
        expect(store.state.checking).toBe(false);
        backend.runOnce.mockResolvedValueOnce(result('idle'));
        await store.checkForWork();
        expect(backend.runOnce).toHaveBeenCalledTimes(2);
    });

    it('never claims while unpaired', async () => {
        const store = await pairedStore(false);
        await store.checkForWork();
        expect(backend.runOnce).not.toHaveBeenCalled();
        expect(store.state.checking).toBe(false);
    });
});

describe('a host with remote builds off', () => {
    beforeEach(() => {
        vi.resetModules();
        vi.resetAllMocks();
    });

    async function initialize(remoteBuilds: boolean) {
        backend.getRunnerStatus.mockResolvedValue(status(true));
        backend.onRunnerActivity.mockResolvedValue(() => {});
        backend.getRealtimeConfiguration.mockResolvedValue({
            key: 'key',
            host: 'realtime.example.test',
            port: 443,
            scheme: 'https',
            channel: 'private-runner.1',
        });
        const { useRunnerStore } = await import('./runner');
        const store = useRunnerStore();
        await store.initialize(remoteBuilds);
        return store;
    }

    it('reads the local status but reaches no control plane, even while paired', async () => {
        const store = await initialize(false);
        // The platform is why the status is still read: the rest of the window needs it.
        expect(store.state.status?.platform).toBe('linux');
        expect(backend.onRunnerActivity).not.toHaveBeenCalled();
        expect(backend.getRealtimeConfiguration).not.toHaveBeenCalled();
        expect(store.state.realtime).toBe('disconnected');
    });

    it('subscribes and connects once the host turns them on', async () => {
        await initialize(true);
        expect(backend.onRunnerActivity).toHaveBeenCalledTimes(1);
        expect(backend.getRealtimeConfiguration).toHaveBeenCalledTimes(1);
    });
});
