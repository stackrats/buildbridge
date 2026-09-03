<script setup lang="ts">
import { CircleHelp } from '@lucide/vue';

defineProps<{ text: string }>();
</script>

<template>
    <span
        class="inline-flex cursor-help items-center align-middle text-zinc-500 hover:text-zinc-600 dark:text-zinc-400 dark:hover:text-zinc-300"
        v-tip="text"
    >
        <CircleHelp class="h-3.5 w-3.5" />
    </span>
</template>
