<script setup lang="ts">
import { inject, useAttrs } from 'vue';

import { fieldKey } from '../../lib/field';
import { cn } from '../../lib/utils';

const field = inject(fieldKey, null);
const attrs = useAttrs();
const model = defineModel<string>({ default: '' });
const { rows = 3, mono = false } = defineProps<{
    rows?: number;
    placeholder?: string;
    mono?: boolean;
    disabled?: boolean;
    invalid?: boolean;
}>();
</script>

<template>
    <textarea
        v-model="model"
        :id="field?.controlId"
        :aria-labelledby="attrs['aria-label'] ? undefined : field?.labelId"
        :aria-describedby="field?.descriptionId.value"
        :aria-required="field?.required.value || undefined"
        :aria-invalid="invalid || field?.invalid.value || undefined"
        :rows="rows"
        :placeholder="placeholder"
        :disabled="disabled"
        :class="
            cn(
                'flex min-h-20 w-full min-w-0 resize-y rounded-md border bg-white px-2.5 py-1.5 text-[13px] text-zinc-900 transition-colors placeholder:text-zinc-400 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 disabled:opacity-50 dark:bg-zinc-900 dark:text-zinc-50 dark:placeholder:text-zinc-500 dark:focus-visible:outline-zinc-300',
                invalid || field?.invalid.value
                    ? 'border-red-500'
                    : 'border-zinc-200 dark:border-zinc-800',
                mono && 'font-mono text-xs',
            )
        "
    />
</template>
