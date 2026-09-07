<script setup lang="ts">
// The window chrome: identity on the left, and on the right the things that are true of the
// whole application — whether this host can run a machine, whether the control plane is
// connected, what the running machines cost right now — and the way into Settings.
import { Bug, Settings } from '@lucide/vue';
import { computed } from 'vue';

import { useBackend } from '../lib/backend';
import { formatCores, formatMemory, rollupTotal, series } from '../lib/usage';
import { useMachinesStore } from '../stores/machines';
import { controlPlaneChip } from '../model/runner';
import { providerHostIssues, providerLabel } from '../model/providers';
import type { MachineProvider } from '../types/backend';
import { useRunnerStore } from '../stores/runner';
import { useSettingsStore } from '../stores/settings';
import { useUi } from '../stores/ui';
import { useUsageStore } from '../stores/usage';
import BrandLogo from './ui/BrandLogo.vue';
import Button from './ui/Button.vue';
import Chip from './ui/Chip.vue';
import Sparkline from './ui/Sparkline.vue';

const machines = useMachinesStore();
const runner = useRunnerStore();
const ui = useUi();
const usage = useUsageStore();

// Without remote builds there is no control plane to report on, so the chip goes entirely
// rather than sitting there saying "local only" about a feature this host does not have.
const remoteBuilds = useSettingsStore().remoteBuilds;

// The webview's own inspector, for what this desktop is doing: its requests to the control
// plane, its console, its layout. What the app on the phone is doing is a different inspector,
// Safari's, and lives in the device step.
const inspectorAvailable = import.meta.env.DEV;
function openInspector(): void {
    void useBackend()
        .openDeveloperTools()
        .catch(() => {
            // Only development builds carry the inspector; the browser preview has its own.
        });
}

// The running machines' cost, summed: cores in cores rather than a percentage, because a
// percentage of every core reads as "340%" on a busy build, and memory against the host's.
// A reading is not a state, so it is not a chip: it reads as the same quiet mono line a machine
// carries, and a rule separates it from the chips that do carry state. It is there only while a
// machine runs, and it leads the group: the group is pinned to the right edge, so the one item
// whose width changes every few seconds grows into the empty space on its left and moves
// nothing. Its figures sit in fixed-width cells for the same reason.
const usageReading = computed(() => {
    const latest = usage.latest.value;
    const total = rollupTotal(latest);
    if (!latest || total.machines === 0) {
        return null;
    }
    const cores = formatCores(total.cpuCores);
    const memory = formatMemory(total.memoryBytes);
    const of = [
        latest.hostCores ? ` of ${latest.hostCores}` : '',
        latest.hostMemoryBytes ? ` of ${formatMemory(latest.hostMemoryBytes)}` : '',
    ];
    return {
        cores,
        memory,
        tip: `${cores}${of[0]} cores and ${memory}${of[1]} across ${total.machines === 1 ? 'the running machine' : `${total.machines} running machines`}, measured while this window shows`,
    };
});
const cpuHistory = computed(() =>
    series(usage.samples.value, (sample) => rollupTotal(sample).cpuCores),
);

const host = computed(() => machines.host.value);
const checkedProviders = computed<MachineProvider[]>(() => {
    const route = ui.route.value;
    if (route.kind === 'native_mac') return [];
    const selected =
        route.kind === 'machine'
            ? machines.machines.value.find((machine) => machine.id === route.id)
            : null;
    if (selected) {
        return [selected.config.provider];
    }
    const providers = [
        ...new Set(machines.machines.value.map((machine) => machine.config.provider)),
    ];
    return providers.length
        ? providers
        : runner.state.status?.platform === 'macos'
          ? []
          : ['android_toolchain'];
});
const hostIssues = computed(() => [
    ...new Set(
        checkedProviders.value.flatMap((provider) =>
            providerHostIssues(host.value, provider, runner.state.status?.platform),
        ),
    ),
]);
const hostDetail = computed(() =>
    hostIssues.value.length
        ? hostIssues.value.join(' ')
        : machines.machines.value.length
          ? `Host checks passed for ${checkedProviders.value.map((provider) => providerLabel[provider]).join(', ')}`
          : 'Docker is available. Other requirements are checked for the platform you choose.',
);
// Shaped like the control-plane chip: the dot carries the state, and only a state that needs
// attention colours the label as well.
const hostChip = computed(() => {
    if (!host.value) {
        return {
            dot: 'bg-zinc-300 dark:bg-zinc-600',
            text: 'checking host',
            tone: 'text-zinc-500 dark:text-zinc-400',
            attention: false,
        };
    }
    return hostIssues.value.length === 0
        ? {
              dot: 'bg-emerald-500',
              text: machines.machines.value.length ? 'host ready' : 'Docker ready',
              tone: 'text-zinc-500 dark:text-zinc-400',
              attention: false,
          }
        : {
              dot: 'bg-red-500',
              text: `host: ${hostIssues.value.length} to fix`,
              tone: 'text-red-700 dark:text-red-400',
              attention: true,
          };
});

const runnerChip = computed(() => controlPlaneChip(runner.state.status, runner.state.realtime));
</script>

<template>
    <header
        class="flex h-12 shrink-0 items-center justify-between gap-4 border-b border-zinc-200 bg-white px-3 dark:border-zinc-800 dark:bg-zinc-900"
    >
        <button
            type="button"
            class="flex items-center rounded-md px-1 py-1 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300"
            v-tip="'Overview'"
            aria-label="Overview"
            @click="ui.navigate({ kind: 'home' })"
        >
            <BrandLogo class="text-zinc-900 dark:text-zinc-50" />
        </button>

        <div class="flex items-center gap-1.5">
            <template v-if="usageReading">
                <button
                    type="button"
                    class="flex h-7 items-center gap-2 rounded-md px-1 font-mono text-[11px] text-zinc-500 tabular-nums hover:text-zinc-900 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:text-zinc-400 dark:hover:text-zinc-50 dark:focus-visible:outline-zinc-300"
                    v-tip="usageReading.tip"
                    @click="ui.navigate({ kind: 'home' })"
                >
                    <Sparkline :values="cpuHistory" :floor="1" />
                    <span class="w-[4.75rem] shrink-0">{{ usageReading.cores }} cores</span>
                    <span class="w-[3.75rem] shrink-0 text-right">{{ usageReading.memory }}</span>
                </button>
                <span
                    aria-hidden="true"
                    class="mx-0.5 h-4 w-px shrink-0 bg-zinc-200 dark:bg-zinc-800"
                />
            </template>
            <Chip
                :dot="hostChip.dot"
                :tip="host ? hostDetail : 'Checking the host'"
                @click="ui.navigate({ kind: 'home' })"
            >
                <span :class="hostChip.attention ? hostChip.tone : undefined">
                    {{ hostChip.text }}
                </span>
            </Chip>
            <Chip
                v-if="remoteBuilds"
                :dot="runnerChip.dot"
                tip="Remote builds"
                @click="ui.navigate({ kind: 'runner' })"
            >
                <span :class="runnerChip.attention ? runnerChip.tone : undefined">
                    {{ runnerChip.text }}
                </span>
            </Chip>

            <Chip
                v-if="inspectorAvailable"
                tip="Open the web inspector for this desktop: its console, network requests and layout"
                @click="openInspector"
            >
                <Bug class="h-3.5 w-3.5" />
                Inspector
            </Chip>
            <Button
                variant="ghost"
                size="iconSm"
                class="ml-1"
                title="Settings: appearance, browser, storage"
                aria-label="Settings"
                @click="ui.state.settingsOpen = true"
            >
                <Settings class="h-3.5 w-3.5" />
            </Button>
        </div>
    </header>
</template>
