import { beforeEach, describe, expect, it, vi } from 'vite-plus/test';

import { createMockBackend } from '../lib/backend-mock';
import type { MachineListView, MachineView, StopAllMachinesResult } from '../types/backend';

const backend = vi.hoisted(() => ({
    getMachine: vi.fn(),
    listMachines: vi.fn(),
    stopAllMachines: vi.fn(),
    launchMachine: vi.fn(),
    cancelMachineOperation: vi.fn(),
}));
vi.mock('../lib/backend', () => ({ useBackend: () => backend }));

function deferred<T>() {
    let resolve!: (result: T) => void;
    const promise = new Promise<T>((accept) => {
        resolve = accept;
    });
    return { promise, resolve };
}

async function fixture(empty = false) {
    const preview = createMockBackend();
    const list = await preview.listMachines();
    list.machines = empty
        ? []
        : list.machines.filter((machine) => ['default', 'pixel-builder'].includes(machine.id));
    const views = new Map<string, MachineView>();
    for (const machine of list.machines) {
        const view = await preview.getMachine(machine.id);
        view.runtime.state = 'exited';
        views.set(machine.id, view);
    }
    backend.listMachines.mockResolvedValue(list);
    backend.getMachine.mockImplementation(async (id: string) => views.get(id));
    const { useMachinesStore } = await import('./machines');
    const store = useMachinesStore();
    await store.loadList();
    if (!empty) await store.refreshMachine('default');
    backend.getMachine.mockClear();
    backend.listMachines.mockClear();
    return { store, list, views };
}

const stopped: StopAllMachinesResult = {
    results: [
        { machineId: 'default', name: 'Local macOS builder', outcome: 'stopped', error: null },
        {
            machineId: 'pixel-builder',
            name: 'Android builder',
            outcome: 'already_stopped',
            error: null,
        },
    ],
};

beforeEach(() => {
    vi.resetModules();
    vi.resetAllMocks();
});

describe('Stop all machines', () => {
    it('retains per-machine failures while refreshing every machine and the host', async () => {
        const { store, list, views } = await fixture();
        const running = views.get('pixel-builder')!;
        running.runtime.state = 'running';
        const target = store.session('pixel-builder');
        target.operation = 'test-build';
        const result: StopAllMachinesResult = {
            results: [
                stopped.results[0]!,
                {
                    machineId: 'pixel-builder',
                    name: 'Android builder',
                    outcome: 'failed',
                    error: 'Build is still stopping.',
                },
            ],
        };
        const refreshed: MachineListView = {
            ...list,
            host: { ...list.host, dockerVersion: 'refreshed host' },
        };
        backend.listMachines.mockResolvedValue(refreshed);
        backend.stopAllMachines.mockResolvedValue(result);

        expect(await store.stopAll()).toEqual(result);
        expect(store.stopAllResult.value).toEqual(result);
        expect(store.stopAllError.value).toBeNull();
        expect(store.stoppingAll.value).toBe(false);
        expect(backend.listMachines).toHaveBeenCalledOnce();
        expect(backend.getMachine).toHaveBeenCalledWith('default');
        expect(backend.getMachine).toHaveBeenCalledWith('pixel-builder');
        expect(store.host.value?.dockerVersion).toBe('refreshed host');
        expect(store.session('default').view?.runtime.state).toBe('exited');
        expect(store.session('default').notice).toContain('retained builds are kept');
        expect(target.view?.runtime.state).toBe('running');
        expect(target.error).toBe('Build is still stopping.');
        expect(target.lastFailure?.operation).toBe('stop');
        expect(target.operation).toBe('test-build');
        expect(target.cancelling).toBe(false);
        expect(backend.cancelMachineOperation).not.toHaveBeenCalled();
    });

    it('handles an empty registry without inventing machine sessions', async () => {
        const { store } = await fixture(true);
        backend.stopAllMachines.mockResolvedValue({ results: [] });

        expect(await store.stopAll()).toEqual({ results: [] });
        expect(store.stopAllResult.value?.results).toEqual([]);
        expect(store.stoppingAll.value).toBe(false);
        expect(backend.listMachines).toHaveBeenCalledOnce();
        expect(backend.getMachine).not.toHaveBeenCalled();
        expect(Object.keys(store.state.sessions)).toEqual([]);
    });

    it.each(['stopped', 'already_stopped'] as const)(
        'preserves unrelated errors and their diagnostic after %s',
        async (outcome) => {
            const { store } = await fixture();
            const target = store.session('default');
            const failure = {
                operation: 'launch' as const,
                message: 'Not enough available memory to start this machine.',
                at: 123,
            };
            target.error = failure.message;
            target.lastFailure = failure;
            const other = store.session('pixel-builder');
            other.error = 'The last status check failed.';
            const result: StopAllMachinesResult = {
                results: [{ ...stopped.results[0]!, outcome }, stopped.results[1]!],
            };
            backend.stopAllMachines.mockResolvedValue(result);

            expect(await store.stopAll()).toEqual(result);
            expect(target.error).toBe(failure.message);
            expect(target.lastFailure).toEqual(failure);
            expect(target.notice).toContain('retained builds are kept');
            expect(other.error).toBe('The last status check failed.');
            expect(store.stopAllResult.value).toEqual(result);
        },
    );

    it('clears a previous stop failure when that machine is now stopped', async () => {
        const { store } = await fixture();
        const target = store.session('default');
        target.error = 'The stop timed out.';
        target.lastFailure = { operation: 'stop', message: target.error, at: 123 };
        backend.stopAllMachines.mockResolvedValue(stopped);

        await store.stopAll();

        expect(target.error).toBeNull();
        expect(target.lastFailure).toBeNull();
        expect(target.notice).toContain('Stopped.');
    });

    it('shares in-flight state and blocks duplicate stops and new machine operations', async () => {
        const { store } = await fixture();
        const pending = deferred<StopAllMachinesResult>();
        backend.stopAllMachines.mockReturnValue(pending.promise);
        const operation = store.stopAll();

        expect(store.stoppingAll.value).toBe(true);
        expect(store.stopAllResult.value).toBeNull();
        expect(await store.stopAll()).toBeNull();
        expect(await store.launch('default')).toBeNull();
        expect(backend.stopAllMachines).toHaveBeenCalledOnce();
        expect(backend.launchMachine).not.toHaveBeenCalled();
        expect(store.stoppingAll.value).toBe(true);

        pending.resolve(stopped);
        expect(await operation).toEqual(stopped);
        expect(store.stoppingAll.value).toBe(false);
        expect(store.session('default').error).toBeNull();
        expect(store.session('pixel-builder').notice).toContain('Already stopped');
    });

    it('reports a command failure, refreshes state, and allows another attempt', async () => {
        const { store } = await fixture();
        backend.stopAllMachines.mockRejectedValueOnce(new Error('Docker is unavailable.'));

        expect(await store.stopAll()).toBeNull();
        expect(store.stopAllError.value).toBe('Docker is unavailable.');
        expect(store.stopAllResult.value).toBeNull();
        expect(store.stoppingAll.value).toBe(false);
        expect(backend.listMachines).toHaveBeenCalledOnce();
        expect(backend.getMachine).toHaveBeenCalledTimes(2);

        backend.stopAllMachines.mockResolvedValue(stopped);
        expect(await store.stopAll()).toEqual(stopped);
        expect(store.stopAllError.value).toBeNull();
        expect(backend.stopAllMachines).toHaveBeenCalledTimes(2);
    });
});
