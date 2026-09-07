import { beforeEach, describe, expect, it, vi } from 'vite-plus/test';

const backend = {
    setUsageSampling: vi.fn(async (_enabled: boolean) => {}),
    onHostUsage: vi.fn(async (_handler: (sample: unknown) => void) => () => {}),
};

vi.mock('../lib/backend', () => ({ useBackend: () => backend }));

async function load() {
    vi.resetModules();
    const { useUsageStore } = await import('./usage');
    return useUsageStore();
}

const GIB = 1024 ** 3;
const sample = (cores: number) => ({
    atUnixMs: 0,
    hostCores: 8,
    hostMemoryBytes: 16 * GIB,
    machines: [
        {
            machineId: 'team-mac',
            cpuCores: cores,
            memoryBytes: 4 * GIB,
            memoryLimitBytes: 16 * GIB,
            memoryLimited: false,
        },
    ],
});

describe('usage store', () => {
    beforeEach(() => {
        backend.setUsageSampling.mockClear();
        backend.onHostUsage.mockClear();
    });

    it('asks the engine to measure only when the window is visible, once per change', async () => {
        const usage = await load();
        await usage.watch(true);
        await usage.watch(true);
        expect(backend.setUsageSampling.mock.calls).toEqual([[true]]);
        await usage.watch(false);
        expect(backend.setUsageSampling.mock.calls).toEqual([[true], [false]]);
    });

    it('keeps a bounded history and answers for one machine', async () => {
        const usage = await load();
        let push: (sample: unknown) => void = () => {};
        backend.onHostUsage.mockImplementationOnce(async (handler) => {
            push = handler;
            return () => {};
        });
        await usage.listen();
        for (let index = 0; index < 70; index += 1) {
            push(sample(index));
        }
        expect(usage.samples.value).toHaveLength(60);
        expect(usage.latest.value?.machines[0]?.cpuCores).toBe(69);
        expect(usage.forMachine('team-mac')?.cpuCores).toBe(69);
        expect(usage.forMachine('other')).toBeNull();
        expect(usage.historyFor('team-mac').slice(-3)).toEqual([67, 68, 69]);
        expect(usage.historyFor('other').every((value) => value === 0)).toBe(true);
        await usage.watch(true);
        await usage.watch(false);
        expect(usage.samples.value).toHaveLength(0);
        expect(usage.latest.value).toBeNull();
    });

    it('records a refusal and lets the next visibility change try again', async () => {
        const usage = await load();
        backend.setUsageSampling.mockRejectedValueOnce(new Error('no engine'));
        await usage.watch(true);
        expect(usage.state.watching).toBe(false);
        expect(usage.state.error).toBe('no engine');
        await usage.watch(true);
        expect(backend.setUsageSampling).toHaveBeenCalledTimes(2);
        expect(usage.state.error).toBeNull();
    });
});
