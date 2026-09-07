<script setup lang="ts">
// Every failure has the same anatomy: what failed, why in one sentence, the diagnostic as it
// came, and the ways out. Used wherever a step or a pane has something a person must decide.
defineProps<{
    title: string;
    cause?: string | null;
    /** The raw message from the backend or the tool, kept verbatim and never interpreted. */
    diagnostic?: string | null;
}>();
</script>

<template>
    <div
        class="rounded-md border border-l-2 border-zinc-200 border-l-red-500 bg-white p-3 dark:border-zinc-800 dark:bg-zinc-900"
        role="alert"
    >
        <p class="text-xs font-semibold text-zinc-900 dark:text-zinc-50">{{ title }}</p>
        <p v-if="cause" class="mt-0.5 text-xs leading-5 text-zinc-600 dark:text-zinc-300">
            {{ cause }}
        </p>
        <div v-if="$slots.default" class="mt-1 text-xs leading-5 text-zinc-600 dark:text-zinc-300">
            <slot />
        </div>
        <pre
            v-if="diagnostic"
            class="mt-2 max-h-40 overflow-auto rounded bg-zinc-50 px-2.5 py-2 font-mono text-[11px] leading-4 whitespace-pre-wrap text-zinc-600 dark:bg-zinc-950 dark:text-zinc-300"
            >{{ diagnostic }}</pre>
        <div v-if="$slots.actions" class="mt-2.5 flex flex-wrap items-center gap-2">
            <slot name="actions" />
        </div>
    </div>
</template>
