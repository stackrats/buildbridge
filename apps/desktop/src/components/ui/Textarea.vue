<script setup lang="ts">
import { cn } from '../../lib/utils';

const model = defineModel<string>({ default: '' });
const { rows = 3, mono = false } = defineProps<{
    rows?: number;
    placeholder?: string;
    mono?: boolean;
    disabled?: boolean;
}>();
</script>

<template>
    <textarea
        v-model="model"
        :rows="rows"
        :placeholder="placeholder"
        :disabled="disabled"
        :class="
            cn(
                'flex w-full rounded-md border border-zinc-200 bg-white px-2.5 py-1.5 text-[13px] text-zinc-900 transition-colors placeholder:text-zinc-400 focus-visible:border-zinc-700 focus-visible:outline-none disabled:opacity-50 dark:border-zinc-800 dark:bg-zinc-900 dark:text-zinc-50 dark:placeholder:text-zinc-500 dark:focus-visible:border-zinc-300',
                mono && 'font-mono text-xs',
            )
        "
    />
</template>
