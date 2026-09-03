<script setup lang="ts" generic="Id extends string">
// The body of an open step, inside its timeline row: what you can do, what is happening, what
// it produced, and the reference for it. Everything is visible at once — the timeline row
// above already says which step this is and where it stands, so nothing is repeated and
// nothing is folded away.
import type { Step } from '../../model/steps';

defineProps<{ step: Step<Id> }>();
</script>

<template>
    <div class="space-y-3.5" :data-step="step.id">
        <div v-if="$slots.action" class="flex flex-wrap items-center gap-2">
            <slot name="action" />
        </div>

        <div v-if="$slots.status" class="space-y-3">
            <slot name="status" />
        </div>

        <div v-if="$slots.result" class="rounded-md bg-zinc-50 p-3 dark:bg-zinc-950">
            <slot name="result" />
        </div>

        <div v-if="$slots.default" class="border-t border-zinc-200 pt-3.5 dark:border-zinc-800">
            <slot />
        </div>
    </div>
</template>
