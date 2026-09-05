<script setup lang="ts">
// The live operation strip: what is happening, for how long, how far along, and the last
// diagnostic line, so a build that is working reads differently from one that has stalled.
import { Square } from '@lucide/vue';
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';

import { formatBytes, formatElapsed } from '../../lib/format';
import Button from './Button.vue';
import Meter from './Meter.vue';
import Spinner from './Spinner.vue';

const {
    label,
    detail = null,
    elapsedSeconds = null,
    value = null,
    completedBytes = null,
    totalBytes = null,
    lastLine = null,
    state = 'running',
    stoppable = false,
    stopping = false,
    stopTitle = 'Stop this operation. Nothing already retained is affected.',
} = defineProps<{
    label: string;
    detail?: string | null;
    elapsedSeconds?: number | null;
    /** 0–1 when known; null shows an indeterminate sweep. */
    value?: number | null;
    completedBytes?: number | null;
    totalBytes?: number | null;
    lastLine?: string | null;
    state?: 'running' | 'failed' | 'done';
    /** Shows a Stop button that emits `stop`; the operation ends as stopped, not failed. */
    stoppable?: boolean;
    stopping?: boolean;
    /** What Stop does here, when it is not the default "nothing retained is affected". */
    stopTitle?: string;
}>();

const emit = defineEmits<{ stop: [] }>();

// The elapsed figure arrives with each progress event, and a phase that prints nothing sends
// none for minutes: the row keeps counting from the last figure it was given, on its own clock,
// so a build that is working never reads as one that has stalled.
const anchor = ref(Date.now());
const now = ref(Date.now());
let ticker: ReturnType<typeof setInterval> | null = null;
watch(
    () => elapsedSeconds,
    () => {
        anchor.value = Date.now();
    },
);
onMounted(() => {
    ticker = setInterval(() => {
        now.value = Date.now();
    }, 1000);
});
onBeforeUnmount(() => {
    if (ticker !== null) {
        clearInterval(ticker);
    }
});
const shownElapsed = computed(() => {
    if (elapsedSeconds === null) {
        return null;
    }
    if (state !== 'running') {
        return elapsedSeconds;
    }
    return elapsedSeconds + Math.max(0, Math.floor((now.value - anchor.value) / 1000));
});
</script>

<template>
    <div
        class="rounded-md border-l-2 bg-zinc-50 py-2.5 pr-3 pl-3 dark:bg-zinc-800/50"
        :class="state === 'failed' ? 'border-red-500' : 'border-zinc-700 dark:border-zinc-100'"
    >
        <div class="flex items-center gap-2">
            <Spinner v-if="state === 'running'" />
            <p class="min-w-0 flex-1 truncate text-xs font-medium text-zinc-900 dark:text-zinc-50">
                {{ label }}
                <span v-if="detail" class="font-normal text-zinc-500 dark:text-zinc-400">
                    · {{ detail }}</span
                >
            </p>
            <span
                v-if="shownElapsed !== null"
                class="shrink-0 text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
            >
                {{ formatElapsed(shownElapsed) }}
            </span>
            <Button
                v-if="stoppable && state === 'running'"
                variant="outline"
                size="sm"
                class="shrink-0"
                :disabled="stopping"
                :title="stopTitle"
                @click="emit('stop')"
            >
                <Spinner v-if="stopping" />
                <Square v-else class="h-3 w-3" />
                {{ stopping ? 'Stopping' : 'Stop' }}
            </Button>
        </div>
        <div v-if="state === 'running'" class="mt-2 flex items-center gap-3">
            <Meter :value="value" />
            <span
                v-if="totalBytes && completedBytes !== null"
                class="shrink-0 text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
            >
                {{ formatBytes(completedBytes) }} / {{ formatBytes(totalBytes) }}
            </span>
        </div>
        <p
            v-if="lastLine"
            class="mt-2 truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400"
            v-tip="lastLine"
        >
            {{ lastLine }}
        </p>
    </div>
</template>
