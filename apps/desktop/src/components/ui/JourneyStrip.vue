<script setup lang="ts">
// The journey as a strip of segments, one per step, grouped by phase. It is drawn from the
// same step model as the rail, so the overview never disagrees with the machine page.
import { computed } from 'vue';

import { groupByPhase, type Step, type StepStatus } from '../../model/steps';

const { steps } = defineProps<{ steps: Step<string>[] }>();

const groups = computed(() => groupByPhase(steps));

const segmentClass: Record<StepStatus, string> = {
    done: 'bg-emerald-500',
    active: 'bg-zinc-900 dark:bg-zinc-100',
    running: 'bg-zinc-900 dark:bg-zinc-100 animate-pulse motion-reduce:animate-none',
    failed: 'bg-red-500',
    pending: 'bg-zinc-200 dark:bg-zinc-700',
};

const statusWord: Record<StepStatus, string> = {
    done: 'done',
    active: 'next',
    running: 'running',
    failed: 'needs attention',
    pending: 'not yet',
};

const caption = computed(() =>
    groups.value
        .map((group) => {
            const word = group.phase === 'setup' ? 'setup' : 'build';
            return `${word} ${group.done}/${group.steps.length}`;
        })
        .join(' · '),
);
</script>

<template>
    <div>
        <div class="flex items-center gap-2" role="img" :aria-label="caption">
            <div v-for="group in groups" :key="group.phase" class="flex gap-0.5">
                <span
                    v-for="step in group.steps"
                    :key="step.id"
                    class="h-1.5 w-3 rounded-[2px] transition-colors duration-500 motion-reduce:transition-none"
                    :class="segmentClass[step.status]"
                    v-tip="`${step.title} · ${statusWord[step.status]}`"
                />
            </div>
        </div>
        <p class="mt-1.5 text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400">
            {{ caption }}
        </p>
    </div>
</template>
