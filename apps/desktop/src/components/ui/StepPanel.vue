<script setup lang="ts" generic="Id extends string">
// Essential inputs and choices remain visible. Steps can opt into a separate details slot for
// technical reference that does not need to compete with the current task.
import type { Step } from '../../model/steps';
import DisclosureSummary from './DisclosureSummary.vue';

const { detailsLabel = 'Technical details' } = defineProps<{
    step: Step<Id>;
    detailsLabel?: string;
}>();
</script>

<template>
    <div class="min-w-0 space-y-3.5" :data-step="step.id">
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

        <details
            v-if="$slots.details"
            class="group/details border-t border-zinc-200 pt-3.5 dark:border-zinc-800"
        >
            <DisclosureSummary>
                {{ detailsLabel }}
            </DisclosureSummary>
            <div class="mt-3 space-y-3">
                <slot name="details" />
            </div>
        </details>
    </div>
</template>
