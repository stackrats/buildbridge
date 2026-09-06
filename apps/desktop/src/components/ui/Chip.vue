<script setup lang="ts">
// A compact fact or filter. Interactive chips look clickable; informational ones do not.
import { cn } from '../../lib/utils';

const {
    dot,
    active = false,
    interactive = true,
    tip,
} = defineProps<{
    /** Status dot colour utility (e.g. "bg-emerald-500"); omitted means no dot. */
    dot?: string;
    active?: boolean;
    interactive?: boolean;
    tip?: string;
}>();

const base =
    'inline-flex h-7 items-center gap-1.5 rounded-full border px-2.5 text-xs whitespace-nowrap focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300';

const variants = {
    active: 'cursor-pointer border-zinc-700 dark:border-zinc-100 bg-zinc-100 dark:bg-zinc-800/60 text-zinc-800 dark:text-zinc-200',
    idle: 'cursor-pointer border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 text-zinc-600 dark:text-zinc-300 hover:border-zinc-300 dark:hover:border-zinc-600 hover:text-zinc-900 dark:hover:text-zinc-50',
    static: 'cursor-default border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 text-zinc-600 dark:text-zinc-300',
};
</script>

<template>
    <button
        type="button"
        :class="cn(base, !interactive ? variants.static : active ? variants.active : variants.idle)"
        v-tip="tip"
        :disabled="!interactive"
    >
        <span v-if="dot" class="h-1.5 w-1.5 shrink-0 rounded-full" :class="dot" />
        <slot />
    </button>
</template>
