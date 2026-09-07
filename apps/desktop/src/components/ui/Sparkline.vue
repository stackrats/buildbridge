<script setup lang="ts">
// A tiny history strip, built from divs: a row of bars needs no viewBox arithmetic to stay
// crisp at any zoom. One reading is noise; a momentary spike and a steady climb are the same
// number and different problems, and only the shape tells them apart.
import { computed } from 'vue';

const {
    values,
    slots = 24,
    max,
    floor = 0,
} = defineProps<{
    /** Oldest first. Fewer than `slots` draws as leading gaps rather than invented readings. */
    values: number[];
    slots?: number;
    /** A fixed ceiling, where the denominator means something, such as the host's cores. */
    max?: number;
    /**
     * The smallest ceiling to scale against when `max` is not given. Pure self-scaling turns
     * an idle machine's noise into a mountain range; a floor keeps small numbers looking small
     * while leaving a real climb room to show.
     */
    floor?: number;
}>();

const bars = computed(() => {
    const recent = values.slice(-slots);
    const ceiling = max ?? Math.max(...recent, floor, 0);
    const pad = Math.max(0, slots - recent.length);
    return [
        ...Array.from({ length: pad }, () => null),
        ...recent.map((value) => (ceiling > 0 ? Math.min(1, Math.max(0, value / ceiling)) : 0)),
    ];
});
</script>

<template>
    <div class="flex h-3.5 items-end gap-px" aria-hidden="true">
        <div
            v-for="(bar, index) in bars"
            :key="index"
            class="w-0.5 rounded-t-[1px]"
            :class="bar === null ? 'bg-transparent' : 'bg-zinc-400 dark:bg-zinc-500'"
            :style="{ height: bar === null ? '100%' : `${Math.max(8, bar * 100)}%` }"
        />
    </div>
</template>
