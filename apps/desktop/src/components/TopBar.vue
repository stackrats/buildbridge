<script setup lang="ts">
// The window chrome: identity on the left, the two things that are true of the whole
// application on the right — whether this host can run a machine, and whether the control
// plane is connected.
import { Bug, Monitor, Moon, Sun } from '@lucide/vue';
import { computed, ref } from 'vue';

import { useBackend } from '../lib/backend';
import { applyTheme, loadTheme, saveTheme, type Theme } from '../lib/prefs';
import { useMachinesStore } from '../stores/machines';
import { controlPlaneChip } from '../model/runner';
import { providerHostIssues, providerLabel } from '../model/providers';
import type { MachineProvider } from '../types/backend';
import { useRunnerStore } from '../stores/runner';
import { useUi } from '../stores/ui';
import BrandLogo from './ui/BrandLogo.vue';
import Chip from './ui/Chip.vue';

const machines = useMachinesStore();
const runner = useRunnerStore();
const ui = useUi();

const theme = ref<Theme>(loadTheme());
const themeOptions: { value: Theme; label: string; icon: typeof Sun }[] = [
    { value: 'system', label: 'System theme', icon: Monitor },
    { value: 'light', label: 'Light theme', icon: Sun },
    { value: 'dark', label: 'Dark theme', icon: Moon },
];

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

function setTheme(next: Theme): void {
    theme.value = next;
    saveTheme(next);
    applyTheme(next);
}

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
            <div
                class="ml-1 flex items-center gap-0.5 rounded-md bg-zinc-100 p-0.5 dark:bg-zinc-800"
            >
                <button
                    v-for="option in themeOptions"
                    :key="option.value"
                    type="button"
                    class="flex h-6 w-6 items-center justify-center rounded-[5px] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300"
                    :class="
                        theme === option.value
                            ? 'bg-white text-zinc-900 shadow-[0_1px_2px_rgb(0_0_0/0.06)] dark:bg-zinc-700 dark:text-zinc-50'
                            : 'text-zinc-500 hover:text-zinc-600 dark:text-zinc-400 dark:hover:text-zinc-300'
                    "
                    v-tip="option.label"
                    :aria-label="option.label"
                    :aria-pressed="theme === option.value"
                    @click="setTheme(option.value)"
                >
                    <component :is="option.icon" class="h-3.5 w-3.5" />
                </button>
            </div>
        </div>
    </header>
</template>
