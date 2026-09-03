<script setup lang="ts">
// One labelled control, with an optional action on the same row.
//
// The action sits outside the <label> and beside the control, and the hint sits under the whole
// row. That keeps a field and its button aligned by structure rather than by a hand-tuned margin,
// whether or not the field has a hint.
defineProps<{
    label: string;
    hint?: string;
    error?: string | null;
    /** Marks a field the surrounding action cannot proceed without. */
    required?: boolean;
    /** Marks a field whose value is already held, where blank means "keep it". */
    stored?: boolean;
}>();
</script>

<template>
    <div class="block">
        <div class="flex items-end gap-2">
            <label class="block min-w-0 flex-1">
                <span class="flex items-baseline justify-between gap-2">
                    <span class="text-xs font-medium text-zinc-600 dark:text-zinc-300">
                        {{ label }}
                        <span v-if="required" class="ml-0.5 text-[11px] font-normal text-zinc-400">
                            required
                        </span>
                    </span>
                    <span v-if="stored" class="text-[11px] text-emerald-700 dark:text-emerald-400">
                        stored
                    </span>
                    <span
                        v-else-if="$slots.trailing"
                        class="text-[11px] text-zinc-500 dark:text-zinc-400"
                    >
                        <slot name="trailing" />
                    </span>
                </span>
                <span class="mt-1.5 block">
                    <slot />
                </span>
            </label>
            <div v-if="$slots.action" class="shrink-0">
                <slot name="action" />
            </div>
        </div>
        <p v-if="error" class="mt-1 text-[11px] text-red-700 dark:text-red-400">
            {{ error }}
        </p>
        <p v-else-if="hint" class="mt-1 text-[11px] leading-4 text-zinc-500 dark:text-zinc-400">
            {{ hint }}
        </p>
    </div>
</template>
