<script setup lang="ts">
// A bounded, tail-following log pane. Lines are interpolated, never injected.
import { Eraser, TextWrap } from '@lucide/vue';
import { computed, nextTick, onMounted, ref, watch } from 'vue';

import { loadLogWrap, saveLogWrap } from '../../lib/prefs';
import Button from './Button.vue';
import CopyButton from './CopyButton.vue';

export interface LogLine {
    text: string;
    tone?: 'default' | 'stderr' | 'system' | 'success';
}

const {
    lines,
    height = 'h-64',
    emptyText = 'Waiting for output',
    clearable = false,
} = defineProps<{
    lines: LogLine[];
    height?: string;
    emptyText?: string;
    clearable?: boolean;
}>();

const emit = defineEmits<{ clear: [] }>();

const scroller = ref<HTMLElement | null>(null);
const pinned = ref(true);
const wrap = ref(loadLogWrap());

const text = computed(() => lines.map((line) => line.text).join('\n'));

const toneClass = {
    default: 'text-zinc-600 dark:text-zinc-300',
    stderr: 'text-amber-700 dark:text-amber-400',
    system: 'text-zinc-600 dark:text-zinc-300',
    success: 'text-emerald-700 dark:text-emerald-400',
};

function onScroll(): void {
    const element = scroller.value;
    if (!element) {
        return;
    }
    pinned.value = element.scrollTop + element.clientHeight >= element.scrollHeight - 8;
}

async function scrollToEnd(): Promise<void> {
    await nextTick();
    const element = scroller.value;
    if (element) {
        element.scrollTop = element.scrollHeight;
    }
}

// Follows the tail only while the reader is at the tail: opening a log, switching to another
// one, and new lines all land at the end, and a reader who has scrolled up to read something is
// left exactly where they are until they return to the bottom.
watch(
    () => lines.length,
    () => {
        if (pinned.value) {
            void scrollToEnd();
        }
    },
);
watch(
    () => lines,
    () => {
        pinned.value = true;
        void scrollToEnd();
    },
);
onMounted(() => {
    void scrollToEnd();
});

function toggleWrap(): void {
    wrap.value = !wrap.value;
    saveLogWrap(wrap.value);
}
</script>

<template>
    <div
        class="flex flex-col overflow-hidden rounded-lg border border-zinc-200 dark:border-zinc-800"
    >
        <div
            class="flex shrink-0 items-center justify-between gap-2 border-b border-zinc-200 bg-white px-2 py-1.5 dark:border-zinc-800 dark:bg-zinc-900"
        >
            <span class="text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400">
                {{ lines.length }} {{ lines.length === 1 ? 'line' : 'lines' }}
            </span>
            <div class="flex items-center gap-0.5">
                <Button
                    variant="ghost"
                    size="iconSm"
                    :title="wrap ? 'Scroll long lines' : 'Wrap long lines'"
                    :aria-label="wrap ? 'Scroll long lines' : 'Wrap long lines'"
                    @click="toggleWrap"
                >
                    <TextWrap
                        class="h-3.5 w-3.5"
                        :class="wrap ? 'text-zinc-700 dark:text-zinc-300' : ''"
                    />
                </Button>
                <CopyButton
                    v-if="lines.length"
                    :text="text"
                    what="Copy the log lines"
                    size="iconSm"
                />
                <Button
                    v-if="clearable && lines.length"
                    variant="ghost"
                    size="iconSm"
                    title="Clears these lines from the drawer; the guest keeps its own log"
                    aria-label="Clear the log"
                    @click="emit('clear')"
                >
                    <Eraser class="h-3.5 w-3.5" />
                </Button>
            </div>
        </div>
        <div
            ref="scroller"
            class="min-h-0 grow overflow-y-auto bg-zinc-50 px-3 py-2 font-mono text-[11px] leading-5 dark:bg-zinc-950"
            :class="[height, wrap ? '' : 'overflow-x-auto']"
            @scroll="onScroll"
        >
            <p v-if="!lines.length" class="text-zinc-500 dark:text-zinc-400">{{ emptyText }}</p>
            <div
                v-for="(line, index) in lines"
                :key="index"
                :class="[
                    wrap ? 'break-all whitespace-pre-wrap' : 'whitespace-pre',
                    toneClass[line.tone ?? 'default'],
                ]"
            >
                {{ line.text }}
            </div>
        </div>
    </div>
</template>
