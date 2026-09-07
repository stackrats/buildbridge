<script setup lang="ts">
// The clickable line of a <details>: the chevron, then the label. It owns the label's type, so
// every disclosure in the app reads the same; `quiet` is the smaller, muted form for an aside.
import { ChevronRight } from '@lucide/vue';

import { cn } from '../../lib/utils';

const { quiet = false } = defineProps<{ quiet?: boolean }>();
</script>

<template>
    <summary
        :class="
            cn(
                'flex cursor-pointer list-none items-center gap-2 rounded-sm focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300 [&::-webkit-details-marker]:hidden',
                quiet
                    ? 'text-[11px] text-zinc-500 hover:text-zinc-700 dark:text-zinc-400 dark:hover:text-zinc-200'
                    : 'text-xs font-medium text-zinc-700 dark:text-zinc-200',
            )
        "
    >
        <ChevronRight
            class="h-3.5 w-3.5 shrink-0 [details[open]>summary>&]:rotate-90"
            aria-hidden="true"
        />
        <slot />
    </summary>
</template>
