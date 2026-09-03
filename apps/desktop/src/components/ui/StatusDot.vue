<script setup lang="ts">
// A colour with its word one hover away: colour alone excludes anyone who cannot tell the
// hues apart.
const { size = 'h-2 w-2' } = defineProps<{ color: string; label: string; size?: string }>();
</script>

<template>
    <span class="inline-flex shrink-0 items-center" v-tip="label" :aria-label="label">
        <span class="rounded-full" :class="[color, size]" />
    </span>
</template>
