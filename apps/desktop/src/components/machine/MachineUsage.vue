<script setup lang="ts">
// One steady line under a running machine's facts: what it costs right now, the last few
// minutes of CPU as a strip, and memory against the limit it runs under. Every figure sits in
// a cell of fixed width, so a reading that changes every few seconds never moves what is
// beside it; the line itself is there for as long as the machine runs.
import { computed } from 'vue';

import { formatCores, formatMemory, memoryTone } from '../../lib/usage';
import { useUsageStore } from '../../stores/usage';
import Meter from '../ui/Meter.vue';
import Sparkline from '../ui/Sparkline.vue';

const { machineId, running } = defineProps<{ machineId: string; running: boolean }>();
const usage = useUsageStore();

const reading = computed(() => usage.forMachine(machineId));
const history = computed(() => usage.historyFor(machineId));
const share = computed(() => {
    const current = reading.value;
    return current && current.memoryLimitBytes > 0
        ? current.memoryBytes / current.memoryLimitBytes
        : 0;
});
const tone = computed(() => {
    const current = reading.value;
    if (!current) {
        return 'neutral' as const;
    }
    return memoryTone(current.memoryBytes, current.memoryLimitBytes, current.memoryLimited);
});
// A machine that is running but missing from a sample is one Docker did not report, which is
// different from a sample that has not arrived yet.
const state = computed(() =>
    reading.value ? 'measured' : usage.latest.value ? 'unreported' : 'measuring',
);
const tip = computed(() => {
    const current = reading.value;
    const latest = usage.latest.value;
    if (!current) {
        return state.value === 'measuring'
            ? 'Measured every few seconds while this window shows'
            : 'Docker reported nothing for this container in the last reading';
    }
    const cores = latest?.hostCores
        ? `${formatCores(current.cpuCores)} of ${latest.hostCores} cores`
        : `${formatCores(current.cpuCores)} cores`;
    const memory = current.memoryLimited
        ? `${formatMemory(current.memoryBytes)} of its ${formatMemory(current.memoryLimitBytes)} limit; past the limit the container is stopped`
        : `${formatMemory(current.memoryBytes)} of the host's ${formatMemory(current.memoryLimitBytes)}`;
    return `${cores} · ${memory}. Measured every few seconds while this window shows.`;
});
</script>

<template>
    <p
        v-if="running"
        class="mt-1.5 flex h-4 items-center gap-2 font-mono text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
        v-tip="tip"
    >
        <template v-if="reading">
            <span class="w-[5.75rem] shrink-0">{{ formatCores(reading.cpuCores) }} cores</span>
            <Sparkline :values="history" :floor="1" />
            <span class="w-[4.25rem] shrink-0 text-right">{{
                formatMemory(reading.memoryBytes)
            }}</span>
            <Meter :value="share" :tone="tone" width="w-16" />
        </template>
        <span v-else>{{ state === 'measuring' ? 'measuring' : 'not measured' }}</span>
    </p>
</template>
