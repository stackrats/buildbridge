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
import { useRunnerStore } from '../stores/runner';
import { useUi } from '../stores/ui';
import BrandMark from './ui/BrandMark.vue';

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
const hostChip = computed(() => {
    if (!host.value) {
        return {
            dot: 'bg-zinc-300 dark:bg-zinc-600',
            text: 'checking host',
            tone: 'text-zinc-500 dark:text-zinc-400',
        };
    }
    return host.value.ready
        ? { dot: 'bg-emerald-500', text: 'host ready', tone: 'text-zinc-500 dark:text-zinc-400' }
        : {
              dot: 'bg-red-500',
              text: `host: ${host.value.issues.length} to fix`,
              tone: 'text-red-700 dark:text-red-400',
          };
});

const runnerChip = computed(() => controlPlaneChip(runner.state.status, runner.state.realtime));

const chipClass =
    'inline-flex h-7 items-center gap-1.5 rounded-full border border-zinc-200 dark:border-zinc-800 px-2.5 text-[11px] transition-colors hover:border-zinc-300 dark:hover:border-zinc-600 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300';
</script>

<template>
    <header
        class="flex h-12 shrink-0 items-center justify-between gap-4 border-b border-zinc-200 bg-white px-3 dark:border-zinc-800 dark:bg-zinc-900"
    >
        <button
            type="button"
            class="flex items-center gap-2 rounded-md px-1 py-1 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300"
            v-tip="'Overview'"
            @click="ui.navigate({ kind: 'home' })"
        >
            <BrandMark class="h-[22px] w-[22px] text-zinc-900 dark:text-zinc-50" />
            <span class="text-[15px] font-semibold tracking-tight text-zinc-900 dark:text-zinc-50"
                >BuildBridge</span
            >
        </button>

        <div class="flex items-center gap-1.5">
            <button
                type="button"
                :class="[chipClass, hostChip.tone]"
                v-tip="
                    host && !host.ready ? host.issues.join(' ') : 'Docker, KVM and display checks'
                "
                @click="ui.navigate({ kind: 'home' })"
            >
                <span class="h-1.5 w-1.5 rounded-full" :class="hostChip.dot" />
                {{ hostChip.text }}
            </button>
            <button
                type="button"
                :class="[chipClass, runnerChip.tone]"
                v-tip="'Control plane'"
                @click="ui.navigate({ kind: 'runner' })"
            >
                <span class="h-1.5 w-1.5 rounded-full" :class="runnerChip.dot" />
                {{ runnerChip.text }}
            </button>

            <button
                v-if="inspectorAvailable"
                type="button"
                :class="chipClass"
                v-tip="
                    'Open the web inspector for this desktop: its console, network requests and layout'
                "
                @click="openInspector"
            >
                <Bug class="h-3.5 w-3.5" />
                Inspector
            </button>
            <div
                class="ml-1 flex items-center gap-0.5 rounded-md bg-zinc-100 p-0.5 dark:bg-zinc-800"
            >
                <button
                    v-for="option in themeOptions"
                    :key="option.value"
                    type="button"
                    class="flex h-6 w-6 items-center justify-center rounded-[5px] transition-colors focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300"
                    :class="
                        theme === option.value
                            ? 'bg-white text-zinc-900 shadow-[0_1px_2px_rgb(0_0_0/0.06)] dark:bg-zinc-700 dark:text-zinc-50'
                            : 'text-zinc-500 hover:text-zinc-600 dark:text-zinc-400 dark:hover:text-zinc-300'
                    "
                    v-tip="option.label"
                    @click="setTheme(option.value)"
                >
                    <component :is="option.icon" class="h-3.5 w-3.5" />
                </button>
            </div>
        </div>
    </header>
</template>
