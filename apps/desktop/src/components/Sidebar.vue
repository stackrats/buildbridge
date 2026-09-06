<script setup lang="ts">
import { House, KeyRound, Layers, Variable, Plus, Radio } from '@lucide/vue';
import { computed, nextTick, onBeforeUnmount, ref } from 'vue';

import { machineStateDot, machineStateLabel } from '../lib/status';
import { clamp } from '../lib/utils';
import { journeyHeadline } from '../model/steps';
import { providerPlatform } from '../model/providers';
import { useEnvSetsStore } from '../stores/envs';
import { useMachineOrder } from '../stores/machine-order';
import { busyKeyLabel, useMachinesStore } from '../stores/machines';
import { useSigningStore } from '../stores/signing';
import { useUi } from '../stores/ui';
import { useRunnerStore } from '../stores/runner';
import type { MachineSummary } from '../types/backend';
import Spinner from './ui/Spinner.vue';
import StatusDot from './ui/StatusDot.vue';
import PlatformIcon from './ui/PlatformIcon.vue';

const ui = useUi();
const machines = useMachinesStore();
const machineOrder = useMachineOrder();
const signing = useSigningStore();
const envs = useEnvSetsStore();
const runner = useRunnerStore();

// What each shared resource holds, so the count is answered without opening the page.
const kitCount = computed(() => signing.kits.value.length);
const envCount = computed(() => envs.sets.value.length);

// The selected row carries a primary rule on its leading edge; nothing else in the app does.
const itemBase =
    'relative flex w-full items-center gap-2.5 rounded-md py-1.5 pr-2 pl-2.5 text-left text-xs focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300';
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

const navigation = ref<HTMLElement | null>(null);
const machineList = ref<HTMLElement | null>(null);
const draggedMachineId = ref<string | null>(null);
const machineDrop = ref<{ id: string; after: boolean } | null>(null);
const reorderAnnouncement = ref('');
let suppressMachineClick = false;
let scrollFrame: number | null = null;
let machineDrag: {
    id: string;
    pointerId: number;
    startX: number;
    startY: number;
    x: number;
    y: number;
    element: HTMLElement;
} | null = null;

function moveMachine(id: string, targetId: string, after: boolean): void {
    const current = machines.sidebarMachines.value.map((machine) => machine.id);
    if (id === targetId || !current.includes(id) || !current.includes(targetId)) {
        return;
    }
    const reordered = current.filter((machineId) => machineId !== id);
    const index = reordered.indexOf(targetId) + Number(after);
    reordered.splice(index, 0, id);
    if (reordered.every((machineId, position) => machineId === current[position])) {
        return;
    }
    machineOrder.reorder(reordered);
    const name = machines.sidebarMachines.value.find((machine) => machine.id === id)?.config.name;
    reorderAnnouncement.value = `${name ?? 'Machine'} moved to position ${index + 1} of ${reordered.length}.`;
}

function updateMachineDrop(): void {
    const drag = machineDrag;
    const bounds = navigation.value?.getBoundingClientRect();
    if (!drag || !draggedMachineId.value || !bounds) {
        return;
    }
    machineDrop.value = null;
    if (
        drag.x < bounds.left ||
        drag.x > bounds.right ||
        drag.y < bounds.top ||
        drag.y > bounds.bottom
    ) {
        return;
    }
    const rows = Array.from(
        machineList.value?.querySelectorAll<HTMLElement>('[data-machine-id]') ?? [],
    ).filter((row) => row.dataset.machineId !== drag.id);
    const next = rows.find((row) => {
        const rect = row.getBoundingClientRect();
        return drag.y < rect.top + rect.height / 2;
    });
    const target = next ?? rows.at(-1);
    if (target?.dataset.machineId) {
        machineDrop.value = { id: target.dataset.machineId, after: !next };
    }
}

function scrollDuringMachineDrag(): void {
    scrollFrame = null;
    const drag = machineDrag;
    const nav = navigation.value;
    if (!drag || !draggedMachineId.value || !nav) {
        return;
    }
    const bounds = nav.getBoundingClientRect();
    if (
        drag.x >= bounds.left &&
        drag.x <= bounds.right &&
        drag.y >= bounds.top &&
        drag.y <= bounds.bottom
    ) {
        const speed =
            drag.y < bounds.top + 32
                ? -clamp((bounds.top + 32 - drag.y) / 32, 0, 1) * 8
                : clamp((drag.y - bounds.bottom + 32) / 32, 0, 1) * 8;
        if (speed) {
            nav.scrollTop += speed;
            updateMachineDrop();
        }
    }
    scrollFrame = requestAnimationFrame(scrollDuringMachineDrag);
}

function moveMachineDrag(event: PointerEvent): void {
    const drag = machineDrag;
    if (!drag || event.pointerId !== drag.pointerId) {
        return;
    }
    drag.x = event.clientX;
    drag.y = event.clientY;
    if (!draggedMachineId.value) {
        if (Math.hypot(drag.x - drag.startX, drag.y - drag.startY) < 5) {
            return;
        }
        draggedMachineId.value = drag.id;
        suppressMachineClick = true;
        drag.element.setPointerCapture(event.pointerId);
        scrollFrame = requestAnimationFrame(scrollDuringMachineDrag);
    }
    event.preventDefault();
    updateMachineDrop();
}

function stopMachineDrag(): void {
    const drag = machineDrag;
    machineDrag = null;
    draggedMachineId.value = null;
    machineDrop.value = null;
    if (scrollFrame !== null) {
        cancelAnimationFrame(scrollFrame);
        scrollFrame = null;
    }
    if (drag?.element.hasPointerCapture(drag.pointerId)) {
        drag.element.releasePointerCapture(drag.pointerId);
    }
    window.removeEventListener('pointermove', moveMachineDrag);
    window.removeEventListener('pointerup', finishMachineDrag);
    window.removeEventListener('pointercancel', stopMachineDrag);
    window.removeEventListener('blur', stopMachineDrag);
    window.removeEventListener('keydown', cancelMachineDrag);
}

function finishMachineDrag(event: PointerEvent): void {
    if (event.pointerId !== machineDrag?.pointerId) {
        return;
    }
    machineDrag.x = event.clientX;
    machineDrag.y = event.clientY;
    updateMachineDrop();
    if (draggedMachineId.value && machineDrop.value) {
        moveMachine(draggedMachineId.value, machineDrop.value.id, machineDrop.value.after);
    }
    stopMachineDrag();
}

function cancelMachineDrag(event: KeyboardEvent): void {
    if (event.key === 'Escape') {
        event.preventDefault();
        stopMachineDrag();
    }
}

function startMachineDrag(event: PointerEvent, id: string): void {
    if (!event.isPrimary || event.button !== 0 || machineCount.value < 2) {
        return;
    }
    stopMachineDrag();
    suppressMachineClick = false;
    machineDrag = {
        id,
        pointerId: event.pointerId,
        startX: event.clientX,
        startY: event.clientY,
        x: event.clientX,
        y: event.clientY,
        element: event.currentTarget as HTMLElement,
    };
    window.addEventListener('pointermove', moveMachineDrag);
    window.addEventListener('pointerup', finishMachineDrag);
    window.addEventListener('pointercancel', stopMachineDrag);
    window.addEventListener('blur', stopMachineDrag);
    window.addEventListener('keydown', cancelMachineDrag);
}

function handleMachineClick(event: MouseEvent): void {
    if (suppressMachineClick && event.detail !== 0) {
        event.preventDefault();
        event.stopPropagation();
        suppressMachineClick = false;
    }
}

function reorderWithKeyboard(event: KeyboardEvent, id: string): void {
    if (!event.altKey || (event.key !== 'ArrowUp' && event.key !== 'ArrowDown')) {
        return;
    }
    event.preventDefault();
    const current = machines.sidebarMachines.value;
    const index = current.findIndex((machine) => machine.id === id);
    const after = event.key === 'ArrowDown';
    const target = current[index + (after ? 1 : -1)];
    if (target) {
        stopMachineDrag();
        const button = event.currentTarget as HTMLElement;
        moveMachine(id, target.id, after);
        void nextTick(() => {
            button.focus();
            button.scrollIntoView({ block: 'nearest' });
        });
    }
}

// A template being saved shows on the Templates entry as well as on its machine, since the
// save outlives its dialog and the page it started on; the first one names the tooltip.
const templateSaves = computed(() => machines.templateSaves());
const templateSavePercent = computed(() => templateSaves.value[0]?.progress?.percent ?? null);
const templateSaveTip = computed(() => {
    const [save] = templateSaves.value;
    if (!save) {
        return null;
    }
    const what = save.name ? `Saving ${save.name}` : `Saving a template from ${save.machineName}`;
    return templateSavePercent.value === null ? what : `${what} · ${templateSavePercent.value}%`;
});

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
    stopMachineDrag();
});
</script>

<template>
    <aside
        class="relative flex shrink-0 flex-col border-r border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900"
        :style="{ width: `${ui.state.sidebarWidth}px` }"
    >
        <nav
            ref="navigation"
            class="min-w-0 flex-1 overflow-x-hidden overflow-y-auto p-2"
            @scroll="updateMachineDrop"
        >
            <button
                type="button"
                :class="[itemBase, ui.state.route.kind === 'home' ? itemActive : itemIdle]"
                @click="ui.navigate({ kind: 'home' })"
            >
                <House class="h-4 w-4 shrink-0" />
                Overview
            </button>

            <button
                v-if="runner.state.status?.platform === 'macos'"
                type="button"
                :class="[itemBase, ui.state.route.kind === 'native_mac' ? itemActive : itemIdle]"
                @click="ui.navigate({ kind: 'native_mac' })"
            >
                <PlatformIcon platform="ios" class="h-4 w-4" />
                This Mac
            </button>

            <div class="mt-4 flex h-6 items-center justify-between pr-1 pl-2.5">
                <span class="text-[11px] font-semibold text-zinc-500 dark:text-zinc-400">
                    Machines
                    <span v-if="machineCount" class="tabular-nums">· {{ machineCount }}</span>
                </span>
                <button
                    type="button"
                    class="flex h-5 w-5 items-center justify-center rounded-[5px] text-zinc-500 hover:bg-zinc-100 hover:text-zinc-900 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-50 dark:focus-visible:outline-zinc-300"
                    v-tip="'New machine'"
                    aria-label="New machine"
                    @click="ui.state.newMachineOpen = true"
                >
                    <Plus class="h-3.5 w-3.5" />
                </button>
            </div>

            <p id="machine-reorder-help" class="sr-only">
                Drag a machine to reorder it, or focus it and press Alt with the Up and Down arrow
                keys.
            </p>
            <p class="sr-only" aria-live="polite" aria-atomic="true">{{ reorderAnnouncement }}</p>
            <ul ref="machineList" class="mt-0.5 flex flex-col gap-0.5">
                <li
                    v-for="machine in machines.sidebarMachines.value"
                    :key="machine.id"
                    :data-machine-id="machine.id"
                    :class="[
                        itemBase,
                        isMachine(machine.id) ? itemActive : itemIdle,
                        'touch-pan-y select-none',
                        { 'opacity-50': draggedMachineId === machine.id },
                    ]"
                    @pointerdown="startMachineDrag($event, machine.id)"
                    @click.capture="handleMachineClick"
                    @click="ui.openMachine(machine.id)"
                    @dragstart.prevent
                >
                    <span
                        v-if="machineDrop?.id === machine.id"
                        aria-hidden="true"
                        class="pointer-events-none absolute right-0 left-0 z-10 h-0.5 rounded-full bg-emerald-500"
                        :class="machineDrop.after ? '-bottom-0.5' : '-top-0.5'"
                    />
                    <button
                        type="button"
                        aria-describedby="machine-reorder-help"
                        aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
                        class="flex min-w-0 flex-1 items-center gap-2.5 rounded-sm text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300"
                        @keydown="reorderWithKeyboard($event, machine.id)"
                    >
                        <PlatformIcon
                            :platform="providerPlatform[machine.config.provider]"
                            class="h-4 w-4"
                        />
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
                        <!-- An app live on the phone is up, not loading: the dot pulses in
                             the status colour instead of spinning. -->
                        <StatusDot
                            v-if="machine.busyOperation && machines.runningLive(machine.id)"
                            color="bg-emerald-500 animate-pulse motion-reduce:animate-none"
                            label="live on the phone"
                        />
                        <Spinner v-else-if="machine.busyOperation" size="h-3 w-3" />
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
                <span class="flex-1">Signing</span>
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
                <span class="flex-1">Environments</span>
                <span
                    v-if="envCount"
                    class="text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
                    >{{ envCount }}</span
                >
            </button>
            <button
                type="button"
                :class="[itemBase, ui.state.route.kind === 'templates' ? itemActive : itemIdle]"
                @click="ui.navigate({ kind: 'templates' })"
            >
                <Layers class="h-4 w-4 shrink-0" />
                <span class="flex-1">Templates</span>
                <span
                    v-if="templateSaves.length"
                    class="flex items-center gap-1 text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
                    v-tip="templateSaveTip"
                >
                    <Spinner size="h-3 w-3" />
                    <span v-if="templateSavePercent !== null">{{ templateSavePercent }}%</span>
                </span>
                <span
                    v-else-if="machines.state.templates.length"
                    class="text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
                    >{{ machines.state.templates.length }}</span
                >
            </button>
            <button
                type="button"
                :class="[itemBase, ui.state.route.kind === 'runner' ? itemActive : itemIdle]"
                @click="ui.navigate({ kind: 'runner' })"
            >
                <Radio class="h-4 w-4 shrink-0" />
                <span class="flex-1">Remote builds</span>
            </button>
        </div>

        <div
            class="absolute top-0 -right-0.5 z-10 h-full w-1.5 cursor-col-resize hover:bg-zinc-100/25"
            @mousedown.prevent="startDrag"
        />
    </aside>
</template>
