<script setup lang="ts">
import { LoaderCircle } from '@lucide/vue';

const { size = 'h-3.5 w-3.5', tone = 'text-zinc-700 dark:text-zinc-300' } = defineProps<{
    size?: string;
    tone?: string;
}>();
</script>

<template>
    <LoaderCircle class="shrink-0 animate-spin" :class="[size, tone]" aria-hidden="true" />
</template>
