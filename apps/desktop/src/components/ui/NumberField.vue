<script setup lang="ts">
// A bounded whole number. The browser's own spinner is hidden and replaced with two steppers, so
// a number reads like every other control here rather than like a raw form element.
import { ChevronDown, ChevronUp } from '@lucide/vue';
import { computed } from 'vue';

import { clampNumber } from '../../lib/numbers';

const model = defineModel<number>({ required: true });
const {
    min,
    max,
    step = 1,
    disabled = false,
} = defineProps<{
    min: number;
    max: number;
    step?: number;
    disabled?: boolean;
}>();

const atMinimum = computed(() => model.value <= min);
const atMaximum = computed(() => model.value >= max);

function commit(value: number): void {
    model.value = clampNumber(value, min, max, model.value);
}

function onInput(event: Event): void {
    const raw = (event.target as HTMLInputElement).value;
    // Typing is allowed to pass through unclamped; blur settles it, so a partial "1" on the way
    // to "1024" is not rewritten under the cursor.
    const parsed = Number(raw);
    if (raw !== '' && Number.isFinite(parsed)) {
        model.value = parsed;
    }
}

function onBlur(): void {
    commit(model.value);
}

const stepperClass =
    'flex h-3.5 w-5 items-center justify-center rounded-sm text-zinc-500 hover:bg-zinc-100 hover:text-zinc-900 disabled:pointer-events-none disabled:opacity-30 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-50';
</script>

<template>
    <span class="relative block w-full">
        <input
            type="number"
            inputmode="numeric"
            :value="model"
            :min="min"
            :max="max"
            :step="step"
            :disabled="disabled"
            class="flex h-8 w-full [appearance:textfield] rounded-md border border-zinc-200 bg-white pr-7 pl-2.5 text-[13px] text-zinc-900 tabular-nums transition-colors outline-none focus-visible:border-zinc-700 disabled:opacity-50 dark:border-zinc-800 dark:bg-zinc-900 dark:text-zinc-50 dark:focus-visible:border-zinc-300 [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
            @input="onInput"
            @blur="onBlur"
        />
        <span class="absolute top-1/2 right-1 flex -translate-y-1/2 flex-col gap-px">
            <button
                type="button"
                tabindex="-1"
                aria-label="Increase"
                :class="stepperClass"
                :disabled="disabled || atMaximum"
                @click="commit(model + step)"
            >
                <ChevronUp class="h-3 w-3" />
            </button>
            <button
                type="button"
                tabindex="-1"
                aria-label="Decrease"
                :class="stepperClass"
                :disabled="disabled || atMinimum"
                @click="commit(model - step)"
            >
                <ChevronDown class="h-3 w-3" />
            </button>
        </span>
    </span>
</template>
