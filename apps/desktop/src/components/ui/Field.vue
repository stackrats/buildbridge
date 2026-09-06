<script setup lang="ts">
// One labelled control, with an optional action on the same row. The control receives its label
// and help text through context, including when it is nested inside a path picker.
import { computed, provide, useId } from 'vue';

import { fieldKey } from '../../lib/field';

const {
    error,
    hint,
    required = false,
} = defineProps<{
    label: string;
    hint?: string;
    error?: string | null;
    /** Marks a field the surrounding action cannot proceed without. */
    required?: boolean;
    /** Marks a field whose value is already held, where blank means "keep it". */
    stored?: boolean;
}>();

const id = useId();
const controlId = `${id}-control`;
const labelId = `${id}-label`;
const descriptionId = `${id}-description`;

provide(fieldKey, {
    controlId,
    labelId,
    descriptionId: computed(() => (error || hint ? descriptionId : undefined)),
    required: computed(() => required),
    invalid: computed(() => Boolean(error)),
});
</script>

<template>
    <div class="block">
        <div class="flex flex-wrap items-end gap-2">
            <div class="block min-w-0 flex-[1_1_10rem]">
                <span class="flex items-baseline justify-between gap-2">
                    <label
                        :id="labelId"
                        :for="controlId"
                        class="text-[13px] font-medium text-zinc-700 dark:text-zinc-200"
                    >
                        {{ label }}
                        <span
                            v-if="required"
                            class="ml-0.5 text-xs font-normal text-zinc-500"
                            aria-hidden="true"
                        >
                            required
                        </span>
                    </label>
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
            </div>
            <div v-if="$slots.action" class="shrink-0">
                <slot name="action" />
            </div>
        </div>
        <p
            v-if="error"
            :id="descriptionId"
            role="alert"
            class="mt-1 text-xs leading-5 text-red-700 dark:text-red-400"
        >
            {{ error }}
        </p>
        <p
            v-else-if="hint"
            :id="descriptionId"
            class="mt-1 text-xs leading-5 text-zinc-500 dark:text-zinc-400"
        >
            {{ hint }}
        </p>
    </div>
</template>
