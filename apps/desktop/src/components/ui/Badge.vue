<script setup lang="ts">
import { cn } from '../../lib/utils';

const { tone = 'neutral' } = defineProps<{
    tone?: 'neutral' | 'ok' | 'warn' | 'danger' | 'info' | 'primary' | 'outline';
}>();

const tones = {
    neutral: 'bg-zinc-100 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-300',
    ok: 'bg-emerald-50 dark:bg-emerald-950/50 text-emerald-700 dark:text-emerald-400',
    warn: 'bg-amber-50 dark:bg-amber-950/50 text-amber-700 dark:text-amber-400',
    danger: 'bg-red-50 dark:bg-red-950/50 text-red-700 dark:text-red-400',
    info: 'bg-zinc-100 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-300',
    primary: 'bg-zinc-100 dark:bg-zinc-800/60 text-zinc-800 dark:text-zinc-200',
    outline: 'border border-zinc-200 dark:border-zinc-800 text-zinc-500 dark:text-zinc-400',
};
</script>

<template>
    <span
        :class="
            cn(
                'inline-flex h-[22px] items-center gap-1 rounded-full px-2 text-[11px] font-medium whitespace-nowrap',
                tones[tone],
            )
        "
    >
        <slot />
    </span>
</template>
