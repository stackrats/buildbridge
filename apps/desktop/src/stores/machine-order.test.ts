import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import { computed } from 'vue';

import type { MachineSummary } from '../types/backend';
import { createMachineOrderStore } from './machine-order';

const key = 'buildbridge.machine-order';
let storage: Map<string, string>;

function machine(
    id: string,
    createdAtEpochSeconds = 1,
    state: MachineSummary['state'] = 'exited',
): MachineSummary {
    return {
        id,
        config: {
            name: id,
            macosRelease: 'sequoia',
            memoryGib: 8,
            cpuCores: 4,
            sshPort: 2222,
            provider: 'docker_osx',
        },
        platform: 'ios',
        createdAtEpochSeconds,
        state,
        containerId: null,
        busyOperation: null,
        guestConfigured: false,
        trustPinned: false,
        workspaceName: null,
        signingKitName: null,
        signingProvisioned: false,
        signingIdentity: null,
        archiveRetained: false,
        envSetName: null,
        usbReady: false,
        deviceRunRetained: false,
        templateName: null,
    };
}

function ids(machines: readonly MachineSummary[]): string[] {
    return machines.map((item) => item.id);
}

beforeEach(() => {
    storage = new Map();
    vi.stubGlobal('localStorage', {
        getItem: (name: string) => storage.get(name) ?? null,
        setItem: (name: string, value: string) => storage.set(name, value),
    });
});

afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

describe('machine ordering preferences', () => {
    it('persists sidebar order without changing the backend list, appending new machines', () => {
        const machines = Object.freeze([machine('a'), machine('b'), machine('c')]);
        const store = createMachineOrderStore();
        expect(ids(store.sortSidebar(machines))).toEqual(['a', 'b', 'c']);
        store.reorder(['c', 'deleted', 'a', 'b']);
        const restored = createMachineOrderStore();
        expect(ids(restored.sortSidebar([...machines, machine('d')]))).toEqual([
            'c',
            'a',
            'b',
            'd',
        ]);
        expect(ids(restored.sortSidebar([machine('a'), machine('d')]))).toEqual(['a', 'd']);
        expect(ids(machines)).toEqual(['a', 'b', 'c']);
    });

    it('puts running machines first and orders each group by creation or most recent use', () => {
        const machines = [
            machine('old-running', 1, 'running'),
            machine('new-stopped', 40),
            machine('new-running', 20, 'running'),
            machine('old-stopped', 10),
        ];
        const store = createMachineOrderStore();
        expect(ids(store.sortDashboard(machines))).toEqual([
            'new-running',
            'old-running',
            'new-stopped',
            'old-stopped',
        ]);
        vi.spyOn(Date, 'now').mockReturnValue(50_000);
        store.markUsed('old-running');
        store.markUsed('old-stopped');
        const restored = createMachineOrderStore();
        expect(ids(restored.sortDashboard(machines))).toEqual([
            'old-running',
            'new-running',
            'old-stopped',
            'new-stopped',
        ]);
        expect(ids(restored.sortSidebar(machines))).toEqual(ids(machines));
    });

    it('uses creation time when it is newer than the recorded use', () => {
        const store = createMachineOrderStore();
        vi.spyOn(Date, 'now').mockReturnValue(1000);
        store.markUsed('new');
        expect(ids(store.sortDashboard([machine('old', 10), machine('new', 20)]))).toEqual([
            'new',
            'old',
        ]);
    });

    it('uses sidebar order for equal recency and updates computed views when preferences change', () => {
        const machines = [machine('a'), machine('b'), machine('c')];
        const store = createMachineOrderStore();
        const sidebar = computed(() => ids(store.sortSidebar(machines)));
        const dashboard = computed(() => ids(store.sortDashboard(machines)));
        expect(dashboard.value).toEqual(['a', 'b', 'c']);
        expect(sidebar.value).toEqual(['a', 'b', 'c']);
        store.reorder(['c', 'b', 'a']);
        expect(sidebar.value).toEqual(['c', 'b', 'a']);
        expect(dashboard.value).toEqual(['c', 'b', 'a']);
        vi.spyOn(Date, 'now').mockReturnValue(2000);
        store.markUsed('a');
        expect(dashboard.value).toEqual(['a', 'c', 'b']);
        expect(sidebar.value).toEqual(['c', 'b', 'a']);
    });

    it.each(['invalid JSON', 'null', '42', '"order"', '[]', '{"ids": {}, "lastUsed": []}'])(
        'falls back to backend order for malformed preferences: %s',
        (raw) => {
            storage.set(key, raw);
            const store = createMachineOrderStore();
            const machines = [machine('a'), machine('b')];
            expect(ids(store.sortSidebar(machines))).toEqual(['a', 'b']);
            expect(ids(store.sortDashboard(machines))).toEqual(['a', 'b']);
        },
    );

    it('retains valid entries while ignoring duplicate IDs and invalid recency values', () => {
        storage.set(
            key,
            '{"ids":["c",7,"","c","a"],"lastUsed":{"a":"9000","b":2000,"c":-1,"d":1e400}}',
        );
        const store = createMachineOrderStore();
        const machines = [machine('a'), machine('b'), machine('c'), machine('d')];
        expect(ids(store.sortSidebar(machines))).toEqual(['c', 'a', 'b', 'd']);
        expect(ids(store.sortDashboard(machines))).toEqual(['b', 'c', 'a', 'd']);
    });

    it.each(['missing', 'denied', 'full', 'getter'])(
        'keeps ordering functional when localStorage is %s',
        (failure) => {
            const fail = () => {
                throw new Error('Storage unavailable');
            };
            if (failure === 'missing') vi.stubGlobal('localStorage', undefined);
            if (failure === 'denied') {
                vi.stubGlobal('localStorage', { getItem: fail, setItem: fail });
            }
            if (failure === 'full') {
                vi.stubGlobal('localStorage', { getItem: () => null, setItem: fail });
            }
            if (failure === 'getter') {
                Object.defineProperty(globalThis, 'localStorage', {
                    configurable: true,
                    get: fail,
                });
            }
            const store = createMachineOrderStore();
            store.reorder(['b', 'a']);
            vi.spyOn(Date, 'now').mockReturnValue(2000);
            store.markUsed('a');
            const machines = [machine('a'), machine('b')];
            expect(ids(store.sortSidebar(machines))).toEqual(['b', 'a']);
            expect(ids(store.sortDashboard(machines))).toEqual(['a', 'b']);
        },
    );
});
