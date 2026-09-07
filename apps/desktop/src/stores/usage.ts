// What the machines cost right now, from the engine's samples. Measuring runs only while the
// window is visible: the app asks the engine to start when the page shows and to stop when it
// hides, so buildbridge in the tray costs nothing measuring itself. A short history is kept
// for the sparkline and dropped when sampling stops, since a gap would draw as a flat line
// that was never measured.

import { computed, reactive } from 'vue';

import { useBackend, type Unlisten } from '../lib/backend';
import { describeError, pushBounded } from '../lib/utils';
import type { MachineUsage, UsageSample } from '../types/backend';

/** Samples kept: sixty at a reading every few seconds is a few minutes of shape. */
export const USAGE_HISTORY = 60;

const state = reactive({
    samples: [] as UsageSample[],
    /** Whether the engine has been asked to keep measuring. */
    watching: false,
    error: null as string | null,
});

let unlisten: Unlisten | null = null;

export function useUsageStore() {
    return {
        state,
        samples: computed(() => state.samples),
        latest: computed(() => state.samples.at(-1) ?? null),
        /** The last reading for one machine, or null once it is no longer running. */
        forMachine(machineId: string): MachineUsage | null {
            const latest = state.samples.at(-1);
            return latest?.machines.find((machine) => machine.machineId === machineId) ?? null;
        },
        /** One machine's cores over the kept samples, oldest first, zero where it was absent. */
        historyFor(machineId: string): number[] {
            return state.samples.map(
                (sample) =>
                    sample.machines.find((machine) => machine.machineId === machineId)?.cpuCores ??
                    0,
            );
        },
        async listen(): Promise<void> {
            if (unlisten) {
                return;
            }
            unlisten = await useBackend().onHostUsage((sample) => {
                pushBounded(state.samples, sample, USAGE_HISTORY);
            });
        },
        /** Tells the engine whether anyone is looking; a hidden window stops the sampler. */
        async watch(visible: boolean): Promise<void> {
            if (state.watching === visible) {
                return;
            }
            state.watching = visible;
            if (!visible) {
                state.samples.splice(0);
            }
            try {
                await useBackend().setUsageSampling(visible);
                state.error = null;
            } catch (error) {
                state.watching = false;
                state.error = describeError(error);
            }
        },
        dispose(): void {
            unlisten?.();
            unlisten = null;
            state.samples.splice(0);
            state.watching = false;
        },
    };
}
