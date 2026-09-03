<script setup lang="ts">
import { House, KeyRound, Variable, Plus, Radio } from '@lucide/vue';
import { computed, onBeforeUnmount } from 'vue';

import { machineStateDot, machineStateLabel } from '../lib/status';
import { clamp } from '../lib/utils';
import { journeyHeadline } from '../model/steps';
import { useEnvSetsStore } from '../stores/envs';
import { busyKeyLabel, useMachinesStore } from '../stores/machines';
import { useSigningStore } from '../stores/signing';
import { useUi } from '../stores/ui';
import type { MachineSummary } from '../types/backend';
import Spinner from './ui/Spinner.vue';
import StatusDot from './ui/StatusDot.vue';

const ui = useUi();
const machines = useMachinesStore();
const signing = useSigningStore();
const envs = useEnvSetsStore();

// What each shared resource holds, so the count is answered without opening the page.
const kitCount = computed(() => signing.kits.value.length);
const envCount = computed(() => envs.sets.value.length);

// The selected row carries a primary rule on its leading edge; nothing else in the app does.
const itemBase =
    'relative flex w-full items-center gap-2.5 rounded-md py-1.5 pr-2 pl-2.5 text-left text-xs transition-colors focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300';
const itemIdle =
    'text-zinc-600 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-zinc-900 dark:hover:text-zinc-50';
const itemActive =
    'bg-zinc-100 dark:bg-zinc-800/60 font-medium text-zinc-800 dark:text-zinc-200 before:absolute before:top-1.5 before:bottom-1.5 before:left-0 before:w-[2px] before:rounded-full before:bg-zinc-700 dark:before:bg-zinc-300';

const isMachine = (id: string) => ui.state.route.kind === 'machine' && ui.state.route.id === id;

// The second line says where the machine is in its journey: what is running, what needs
// attention, or what comes next.
function machineSecondary(machine: MachineSummary): string {
    if (machine.busyOperation) {
        return busyKeyLabel[machine.busyOperation] ?? machine.busyOperation;
    }
    return journeyHeadline(machines.journey(machine.id)) || machineStateLabel[machine.state];
}

const machineCount = computed(() => machines.machines.value.length);

// Drag-to-resize, persisted between launches.
let dragging = false;
function startDrag(event: MouseEvent): void {
    dragging = true;
    const startX = event.clientX;
    const startWidth = ui.state.sidebarWidth;
    const move = (moveEvent: MouseEvent) => {
        if (dragging) {
            ui.state.sidebarWidth = clamp(startWidth + moveEvent.clientX - startX, 180, 420);
        }
    };
    const stop = () => {
        dragging = false;
        ui.setSidebarWidth(ui.state.sidebarWidth);
        window.removeEventListener('mousemove', move);
        window.removeEventListener('mouseup', stop);
    };
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', stop);
}

onBeforeUnmount(() => {
    dragging = false;
});
</script>

<template>
    <aside
        class="relative flex shrink-0 flex-col border-r border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900"
        :style="{ width: `${ui.state.sidebarWidth}px` }"
    >
        <nav class="min-w-0 flex-1 overflow-x-hidden overflow-y-auto p-2">
            <button
                type="button"
                :class="[itemBase, ui.state.route.kind === 'home' ? itemActive : itemIdle]"
                @click="ui.navigate({ kind: 'home' })"
            >
                <House class="h-4 w-4 shrink-0" />
                Overview
            </button>

            <div class="mt-4 flex h-6 items-center justify-between pr-1 pl-2.5">
                <span class="text-[11px] font-semibold text-zinc-500 dark:text-zinc-400">
                    macOS machines
                    <span v-if="machineCount" class="tabular-nums">· {{ machineCount }}</span>
                </span>
                <button
                    type="button"
                    class="flex h-5 w-5 items-center justify-center rounded-[5px] text-zinc-500 transition-colors hover:bg-zinc-100 hover:text-zinc-900 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-50 dark:focus-visible:outline-zinc-300"
                    v-tip="'New machine'"
                    @click="ui.state.newMachineOpen = true"
                >
                    <Plus class="h-3.5 w-3.5" />
                </button>
            </div>

            <ul class="mt-0.5 flex flex-col gap-0.5">
                <li v-for="machine in machines.machines.value" :key="machine.id">
                    <button
                        type="button"
                        :class="[itemBase, isMachine(machine.id) ? itemActive : itemIdle]"
                        @click="ui.openMachine(machine.id)"
                    >
                        <span class="min-w-0 flex-1">
                            <span class="block truncate">{{ machine.config.name }}</span>
                            <span
                                class="block truncate text-[11px] font-normal"
                                :class="
                                    isMachine(machine.id)
                                        ? 'text-zinc-800/75 dark:text-zinc-200/75'
                                        : 'text-zinc-500 dark:text-zinc-400'
                                "
                            >
                                {{ machineSecondary(machine) }}
                            </span>
                        </span>
                        <Spinner v-if="machine.busyOperation" size="h-3 w-3" />
                        <StatusDot
                            v-else
                            :color="machineStateDot[machine.state]"
                            :label="machineStateLabel[machine.state]"
                        />
                    </button>
                </li>
                <li v-if="!machineCount && !machines.state.listLoading">
                    <p class="px-2.5 py-1.5 text-[11px] leading-4 text-zinc-500 dark:text-zinc-400">
                        No machines yet. A machine is installed once and builds any number of
                        projects.
                    </p>
                </li>
            </ul>
        </nav>

        <div class="flex flex-col gap-0.5 border-t border-zinc-200 p-2 dark:border-zinc-800">
            <button
                type="button"
                :class="[itemBase, ui.state.route.kind === 'signing' ? itemActive : itemIdle]"
                @click="ui.navigate({ kind: 'signing' })"
            >
                <KeyRound class="h-4 w-4 shrink-0" />
                <span class="flex-1">Signing kits</span>
                <span
                    v-if="kitCount"
                    class="text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
                    >{{ kitCount }}</span
                >
            </button>
            <button
                type="button"
                :class="[itemBase, ui.state.route.kind === 'envs' ? itemActive : itemIdle]"
                @click="ui.navigate({ kind: 'envs' })"
            >
                <Variable class="h-4 w-4 shrink-0" />
                <span class="flex-1">Env sets</span>
                <span
                    v-if="envCount"
                    class="text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
                    >{{ envCount }}</span
                >
            </button>
            <button
                type="button"
                :class="[itemBase, ui.state.route.kind === 'runner' ? itemActive : itemIdle]"
                @click="ui.navigate({ kind: 'runner' })"
            >
                <Radio class="h-4 w-4 shrink-0" />
                <span class="flex-1">Control plane</span>
            </button>
        </div>

        <div
            class="absolute top-0 -right-0.5 z-10 h-full w-1.5 cursor-col-resize hover:bg-zinc-100/25"
            @mousedown.prevent="startDrag"
        />
    </aside>
</template>
