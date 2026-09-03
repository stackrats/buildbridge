<script setup lang="ts">
import { cn } from '../../lib/utils';

const model = defineModel<string>({ default: '' });
const {
    type = 'text',
    mono = false,
    disabled = false,
    invalid = false,
} = defineProps<{
    type?: 'text' | 'password' | 'url';
    placeholder?: string;
    mono?: boolean;
    disabled?: boolean;
    invalid?: boolean;
    autocomplete?: string;
    maxlength?: number;
}>();
</script>

<template>
    <input
        v-model="model"
        :type="type"
        :placeholder="placeholder"
        :disabled="disabled"
        :autocomplete="autocomplete"
        :maxlength="maxlength"
        :aria-invalid="invalid || undefined"
        :class="
            cn(
                'flex h-8 w-full rounded-md border bg-white px-2.5 text-[13px] text-zinc-900 transition-colors placeholder:text-zinc-400 focus-visible:border-zinc-700 focus-visible:outline-none disabled:opacity-50 dark:bg-zinc-900 dark:text-zinc-50 dark:placeholder:text-zinc-500 dark:focus-visible:border-zinc-300',
                invalid ? 'border-red-500' : 'border-zinc-200 dark:border-zinc-800',
                mono && 'font-mono text-xs',
            )
        "
    />
</template>
