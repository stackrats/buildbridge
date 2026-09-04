<script setup lang="ts">
// The log as a drawer under the machine page, so a running step and its output are on screen
// together. It opens by itself when an operation starts, folds to one line, and remembers
// its height. The bar always says what is running and for how long.
import { ChevronDown, ChevronUp } from '@lucide/vue';
import { computed, onBeforeUnmount, watch } from 'vue';

import { formatElapsed } from '../../lib/format';
import { LOG_HEIGHT_MAX, LOG_HEIGHT_MIN } from '../../lib/prefs';
import { clamp } from '../../lib/utils';
import { activityLabel, useMachinesStore, type MachineSession } from '../../stores/machines';
import { useUi, type LogSource } from '../../stores/ui';
import type { LogLine } from '../ui/LogView.vue';
import Chip from '../ui/Chip.vue';
import LogView from '../ui/LogView.vue';

const { session, runningStep, now } = defineProps<{
    session: MachineSession;
    /** The step whose operation is in flight, so the drawer can open on the right source. */
    runningStep: string | null;
    now: number;
}>();
const machines = useMachinesStore();
const ui = useUi();

const open = computed(() => ui.isLogOpen(session.id));
const source = computed<LogSource>({
    get: () => ui.logSource(session.id),
    set: (value) => ui.setLogSource(session.id, value),
});

const consoleLines = computed<LogLine[]>(() =>
    (session.view?.logs ?? []).map((text) => ({ text })),
);

const sources = computed<{ value: LogSource; label: string; count: number }[]>(() => [
    { value: 'activity', label: 'Activity', count: session.activity.length },
    { value: 'build', label: 'Test build', count: session.buildLog.length },
    { value: 'archive', label: 'Signed archive', count: session.archiveLog.length },
    { value: 'device', label: 'Device console', count: session.deviceLog.length },
    { value: 'console', label: 'Machine console', count: consoleLines.value.length },
]);

const lines = computed<LogLine[]>(() => {
    switch (source.value) {
        case 'console':
            return consoleLines.value;
        case 'build':
            return session.buildLog;
        case 'archive':
            return session.archiveLog;
        case 'device':
            return session.deviceLog;
        default:
            return session.activity;
    }
});

const emptyText = computed(() => {
    switch (source.value) {
        case 'console':
            return 'The last 80 lines of Docker-OSX output appear here once the container exists.';
        case 'build':
            return 'Output from the unsigned test build appears here while it runs.';
        case 'archive':
            return 'Output from the signed archive appears here while it runs.';
        case 'device':
            return 'The app’s console appears here while it runs on the iPhone.';
        default:
            return 'Operations started from this desktop and their results are recorded here.';
    }
});

/** Which source a step writes to, so the drawer opens on the lines that matter. */
function sourceFor(step: string | null): LogSource {
    if (step === 'sync' || step === 'test-build') {
        return 'build';
    }
    if (step === 'archive') {
        return 'archive';
    }
    if (step === 'run-device') {
        return 'device';
    }
    return 'activity';
}

// A new operation never opens the drawer on its own: the step's progress strip already shows
// the phase and the last line, and the collapsed bar keeps a live summary. It only points the
// drawer at the right source, so opening it lands on the lines that matter.
watch(
    () => runningStep,
    (step, previous) => {
        if (step && !previous) {
            ui.selectLog(session.id, sourceFor(step));
        }
    },
);

const running = computed(() => runningStep !== null);
// The tab the running step writes to carries a live dot, so the five tabs say which one moves.
const liveSource = computed(() => (runningStep !== null ? sourceFor(runningStep) : null));
const activity = computed(() => session.operation ?? session.view?.busyOperation ?? null);
const runningLabel = computed(
    () => activityLabel(activity.value) ?? (activity.value ? 'Working' : null),
);
// Only the progress of the operation in flight: every other record is left from an earlier
// one, and would put a finished build's last phase under a stop.
const phaseDetail = computed(() => {
    switch (activity.value) {
        case 'launch':
        case 'starting':
            return session.launch?.detail ?? null;
        case 'xcode-import':
        case 'importing_xcode':
        case 'xcode-activate':
        case 'activating_xcode':
            return session.xcode?.detail ?? null;
        case 'provision':
        case 'provisioning_signing':
            return session.signing?.detail ?? null;
        case 'sync':
        case 'synchronizing':
        case 'test-build':
        case 'test_building':
            return session.project?.detail ?? null;
        case 'archive':
        case 'archiving':
            return session.archive?.detail ?? null;
        case 'usb-migrate':
        case 'migrating_usb':
            return session.usbMigration?.detail ?? null;
        case 'usb-rebuild':
        case 'rebuilding_container':
            return session.rebuild?.detail ?? null;
        case 'usb-attach':
        case 'attaching_usb':
        case 'settling_phone':
            return session.usbAttach?.detail ?? null;
        case 'device-signing':
        case 'preparing_device_signing':
            return session.deviceSigning?.detail ?? session.signing?.detail ?? null;
        case 'run-device':
        case 'running_on_device':
            return session.device?.detail ?? null;
        default:
            return null;
    }
});
const elapsed = computed(() =>
    session.operationStartedAt ? Math.floor((now - session.operationStartedAt) / 1000) : null,
);
const lastLine = computed(() => lines.value.at(-1)?.text ?? null);

function toggle(): void {
    if (open.value) {
        ui.closeLog(session.id);
    } else {
        ui.openLog(session.id);
    }
}

function clear(): void {
    if (source.value === 'build') {
        machines.clearBuildLog(session.id);
    } else if (source.value === 'archive') {
        machines.clearArchiveLog(session.id);
    } else if (source.value === 'device') {
        machines.clearDeviceLog(session.id);
    } else if (source.value === 'activity') {
        machines.clearActivity(session.id);
    }
}

// Drag the bar to resize; the height persists between launches.
let dragging = false;
function startDrag(event: MouseEvent): void {
    if (!open.value) {
        return;
    }
    dragging = true;
    const startY = event.clientY;
    const startHeight = ui.state.logHeight;
    const move = (moveEvent: MouseEvent) => {
        if (dragging) {
            ui.state.logHeight = clamp(
                startHeight + (startY - moveEvent.clientY),
                LOG_HEIGHT_MIN,
                LOG_HEIGHT_MAX,
            );
        }
    };
    const stop = () => {
        dragging = false;
        ui.setLogHeight(ui.state.logHeight);
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
    <div
        class="relative shrink-0 border-t border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900"
    >
        <div
            v-if="open"
            class="absolute -top-1 right-0 left-0 z-10 h-2 cursor-row-resize"
            @mousedown.prevent="startDrag"
        />
        <div class="flex h-9 items-center gap-2 px-3">
            <button
                type="button"
                class="flex min-w-0 flex-1 items-center gap-2 rounded-md py-1 pr-2 pl-1 text-left text-xs focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300"
                :aria-expanded="open"
                @click="toggle"
            >
                <span
                    class="h-1.5 w-1.5 shrink-0 rounded-full"
                    :class="
                        running
                            ? 'animate-pulse bg-zinc-900 motion-reduce:animate-none dark:bg-zinc-100'
                            : 'bg-zinc-300 dark:bg-zinc-600'
                    "
                />
                <span class="shrink-0 font-medium text-zinc-900 dark:text-zinc-50">
                    {{ running && runningLabel ? runningLabel : 'Log' }}
                </span>
                <span
                    v-if="running && phaseDetail"
                    class="min-w-0 truncate text-zinc-500 dark:text-zinc-400"
                    >{{ phaseDetail }}</span
                >
                <span
                    v-else-if="!open && lastLine"
                    class="min-w-0 truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400"
                    >{{ lastLine }}</span
                >
                <span
                    v-if="running && elapsed !== null"
                    class="shrink-0 font-mono text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
                    >{{ formatElapsed(elapsed) }}</span
                >
            </button>
            <div v-if="open" class="flex shrink-0 items-center gap-1">
                <Chip
                    v-for="option in sources"
                    :key="option.value"
                    :active="source === option.value"
                    :dot="option.value === liveSource ? 'bg-emerald-500 animate-pulse' : undefined"
                    @click="source = option.value"
                >
                    {{ option.label }}
                    <span class="text-[10px] text-zinc-500 tabular-nums dark:text-zinc-400">{{
                        option.count
                    }}</span>
                </Chip>
            </div>
            <button
                type="button"
                class="flex h-6 w-6 shrink-0 items-center justify-center rounded-[5px] text-zinc-500 hover:bg-zinc-100 hover:text-zinc-900 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-50 dark:focus-visible:outline-zinc-300"
                @click="toggle"
            >
                <ChevronDown v-if="open" class="h-3.5 w-3.5" />
                <ChevronUp v-else class="h-3.5 w-3.5" />
            </button>
        </div>
        <Transition
            enter-active-class="transition-opacity duration-150 motion-reduce:transition-none"
            enter-from-class="opacity-0"
            leave-active-class="transition-opacity duration-100 motion-reduce:transition-none"
            leave-to-class="opacity-0"
        >
            <div v-if="open" class="px-3 pb-3">
                <LogView
                    :lines="lines"
                    height=""
                    :style="{ height: `${ui.state.logHeight}px` }"
                    :empty-text="emptyText"
                    :clearable="source !== 'console'"
                    @clear="clear"
                />
                <p class="mt-1.5 text-[11px] leading-4 text-zinc-500 dark:text-zinc-400">
                    {{
                        source === 'device'
                            ? 'The device console keeps the last 600 lines the app printed. Secret values never appear here.'
                            : 'Build logs keep the last 600 lines the guest printed; the complete Xcode output stays in the guest. Secret values never appear here.'
                    }}
                </p>
            </div>
        </Transition>
    </div>
</template>
