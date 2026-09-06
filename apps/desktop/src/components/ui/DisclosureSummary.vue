<script setup lang="ts">
import { ChevronRight } from '@lucide/vue';
</script>

<template>
    <summary
        class="flex cursor-pointer list-none items-center gap-2 rounded-sm focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300 [&::-webkit-details-marker]:hidden"
    >
        <ChevronRight
            class="h-3.5 w-3.5 shrink-0 [details[open]>summary>&]:rotate-90"
            aria-hidden="true"
        />
        <slot />
    </summary>
</template>
