<script setup lang="ts">
defineProps<{ title: string; description?: string }>();
</script>

<template>
    <div
        class="rounded-lg border border-dashed border-zinc-300 px-6 py-10 text-center dark:border-zinc-700"
    >
        <div
            v-if="$slots.icon"
            class="mx-auto mb-3 flex h-9 w-9 items-center justify-center rounded-md bg-zinc-100 text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400"
        >
            <slot name="icon" />
        </div>
        <p class="text-[13px] text-zinc-600 dark:text-zinc-300">{{ title }}</p>
        <p
            v-if="description"
            class="mx-auto mt-1 max-w-md text-xs leading-5 text-zinc-500 dark:text-zinc-400"
        >
            {{ description }}
        </p>
        <div v-if="$slots.default" class="mt-4 flex justify-center gap-2">
            <slot />
        </div>
    </div>
</template>
