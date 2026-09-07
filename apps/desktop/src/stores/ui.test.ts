import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';

let storage: Map<string, string>;

beforeEach(() => {
    vi.resetModules();
    storage = new Map();
    vi.stubGlobal('localStorage', {
        getItem: (name: string) => storage.get(name) ?? null,
        setItem: (name: string, value: string) => storage.set(name, value),
    });
});

afterEach(() => {
    vi.unstubAllGlobals();
});

async function load(selection?: string) {
    vi.resetModules();
    if (selection !== undefined) storage.set('buildbridge.selection', selection);
    const { useUi } = await import('./ui');
    return useUi();
}

describe('window view state', () => {
    it('restores the saved selection and falls back to home for anything unknown', async () => {
        expect((await load()).route.value).toEqual({ kind: 'home' });
        expect((await load('machine:pixel-builder')).route.value).toEqual({
            kind: 'machine',
            id: 'pixel-builder',
        });
        expect((await load('envs')).route.value).toEqual({ kind: 'envs' });
        expect((await load('settings')).route.value).toEqual({ kind: 'home' });
    });

    it('persists navigation and records the machine as recently used', async () => {
        const ui = await load();
        ui.navigate({ kind: 'machine', id: 'a' });
        expect(storage.get('buildbridge.selection')).toBe('machine:a');
        const order = JSON.parse(storage.get('buildbridge.machine-order') ?? '{}') as {
            lastUsed?: Record<string, number>;
        };
        expect(order.lastUsed?.a).toBeGreaterThan(0);
        ui.navigate({ kind: 'signing' });
        expect(ui.route.value).toEqual({ kind: 'signing' });
        expect(storage.get('buildbridge.selection')).toBe('signing');
    });

    it('opens a machine on a chosen step, or on its focus step, and tells a closed step from none', async () => {
        const ui = await load();
        ui.openMachine('a', 'archive');
        expect(ui.route.value).toEqual({ kind: 'machine', id: 'a' });
        expect(ui.selectedStep('a')).toBe('archive');
        expect(ui.state.machineSections.a).toBe('steps');
        ui.openMachine('a');
        expect(ui.selectedStep('a')).toBeNull();
        ui.selectStep('a', null);
        expect(ui.selectedStep('a')).toBe('');
        expect(ui.selectedStep('b')).toBeNull();
    });

    it('keeps the tab a person chose on each machine while they move between machines and pages', async () => {
        const ui = await load();
        ui.openMachine('a');
        expect(ui.state.machineSections.a).toBeUndefined();
        ui.state.machineSections.a = 'preview';
        ui.openMachine('b');
        ui.navigate({ kind: 'signing' });
        ui.openMachine('a');
        expect(ui.state.machineSections.a).toBe('preview');
        expect(ui.state.machineSections.b).toBeUndefined();
        ui.openMachine('a', 'signing-kit');
        expect(ui.state.machineSections.a).toBe('steps');
    });

    it('keeps the log drawer closed on the activity source until it is opened', async () => {
        const ui = await load();
        expect(ui.isLogOpen('a')).toBe(false);
        expect(ui.logSource('a')).toBe('activity');
        ui.openLog('a', 'build');
        expect(ui.isLogOpen('a')).toBe(true);
        expect(ui.logSource('a')).toBe('build');
        ui.closeLog('a');
        ui.selectLog('a', 'console');
        expect(ui.isLogOpen('a')).toBe(false);
        expect(ui.logSource('a')).toBe('console');
        ui.openLog('a');
        expect(ui.logSource('a')).toBe('console');
        expect(ui.isLogOpen('b')).toBe(false);
    });
});
