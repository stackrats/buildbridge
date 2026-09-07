<script setup lang="ts">
import { Check } from '@lucide/vue';

const model = defineModel<boolean>({ default: false });
const { block = false, disabled = false } = defineProps<{
    disabled?: boolean;
    label?: string;
    /** Fill the line so the whole width toggles. */
    block?: boolean;
}>();
</script>

<template>
    <!-- Positioned so the hidden input stays beside the label: left to an unpositioned ancestor it
         would sit at the pane's unscrolled position, and focusing it would scroll the window. -->
    <label
        class="relative cursor-pointer items-start gap-2 align-top text-xs leading-5 text-zinc-600 select-none dark:text-zinc-300"
        :class="[
            block ? 'flex w-full' : 'inline-flex',
            disabled && 'cursor-not-allowed opacity-50',
        ]"
    >
        <input v-model="model" type="checkbox" class="peer sr-only" :disabled="disabled" />
        <!-- The tick is always rendered and only fades, so the box's baseline never moves. -->
        <span
            class="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-[5px] border transition-colors peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-zinc-700 dark:peer-focus-visible:outline-zinc-300"
            :class="
                model
                    ? 'border-zinc-700 bg-zinc-900 dark:border-zinc-100 dark:bg-zinc-100'
                    : 'border-zinc-300 bg-white dark:border-zinc-700 dark:bg-zinc-900'
            "
        >
            <Check
                class="h-3 w-3 text-white transition-opacity dark:text-zinc-950"
                :class="model ? 'opacity-100' : 'opacity-0'"
            />
        </span>
        <slot>{{ label }}</slot>
    </label>
</template>
