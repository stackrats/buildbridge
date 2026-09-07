<script setup lang="ts">
// Apple is ink, so it follows the mode; Android is one green in both modes, the one closest to
// the platform's own, so the same machine is the same colour wherever the window is.
import type { MachinePlatform } from '../../types/backend';

defineProps<{ platform: MachinePlatform }>();
</script>

<template>
    <svg
        viewBox="0 0 24 24"
        fill="currentColor"
        role="img"
        :aria-label="platform === 'android' ? 'Android' : 'Apple'"
        focusable="false"
        class="shrink-0"
        :class="platform === 'android' ? 'text-emerald-400' : 'text-zinc-900 dark:text-white'"
    >
        <g v-if="platform === 'android'">
            <path
                d="M6.5 8 4 3.5M17.5 8 20 3.5"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
            />
            <path
                fill-rule="evenodd"
                d="M2 20v-3a10 10 0 0 1 20 0v3H2ZM7 13a1.25 1.25 0 1 0 0 2.5A1.25 1.25 0 0 0 7 13Zm10 0a1.25 1.25 0 1 0 0 2.5A1.25 1.25 0 0 0 17 13Z"
            />
        </g>
        <path
            v-else
            d="M14.1 5.7c1.1-.1 2.2-.8 2.9-1.7.7-.9 1.2-2 1.1-3.1-1.1.1-2.3.8-3 1.6-.7.8-1.3 2-1 3.2Zm4.4 7.2c0-2.2 1.8-3.3 1.9-3.4-1.1-1.6-2.9-1.8-3.5-1.8-1.5-.2-2.9.9-3.7.9-.8 0-2-1-3.3-.9-1.7 0-3.3 1-4.1 2.5-1.7 3-.4 7.5 1.3 9.9.8 1.2 1.8 2.5 3.1 2.4 1.2 0 1.7-.8 3.2-.8 1.5 0 1.9.8 3.2.8 1.3 0 2.1-1.2 2.9-2.3.9-1.3 1.3-2.5 1.3-2.6-.1 0-2.3-.9-2.3-3.7Z"
        />
    </svg>
</template>
