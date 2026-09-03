<script setup lang="ts">
import { CircleAlert, CircleCheck, Info, TriangleAlert } from '@lucide/vue';

import { cn } from '../../lib/utils';

const { tone = 'info' } = defineProps<{
    tone?: 'info' | 'ok' | 'warn' | 'danger' | 'neutral';
    title?: string;
}>();

// A tinted ground plus a coloured left rule: the meaning is legible before the text is read.
const tones = {
    info: 'bg-zinc-100 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-300 border-zinc-400 dark:border-zinc-500',
    ok: 'bg-emerald-50 dark:bg-emerald-950/50 text-emerald-700 dark:text-emerald-400 border-emerald-600 dark:border-emerald-500',
    warn: 'bg-amber-50 dark:bg-amber-950/50 text-amber-700 dark:text-amber-400 border-amber-500',
    danger: 'bg-red-50 dark:bg-red-950/50 text-red-700 dark:text-red-400 border-red-500',
    neutral:
        'bg-zinc-100 dark:bg-zinc-800/60 text-zinc-600 dark:text-zinc-300 border-zinc-300 dark:border-zinc-700',
};

const icons = {
    info: Info,
    ok: CircleCheck,
    warn: TriangleAlert,
    danger: CircleAlert,
    neutral: Info,
};
</script>

<template>
    <div
        :class="
            cn(
                'flex items-start gap-2.5 rounded-md border-l-2 py-2.5 pr-3 pl-2.5 text-xs leading-5',
                tones[tone],
            )
        "
    >
        <component :is="icons[tone]" class="mt-px h-4 w-4 shrink-0" />
        <div class="min-w-0 flex-1">
            <p v-if="title" class="font-semibold">{{ title }}</p>
            <slot />
        </div>
    </div>
</template>
