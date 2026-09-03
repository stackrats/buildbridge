<script setup lang="ts">
// One machine on the overview: its state, the whole journey as a strip, and the next thing
// to do as the button. Drawn from the same step model as the machine page.
import { ArrowRight, Play } from '@lucide/vue';
import { computed } from 'vue';

import { machineStateBadge, machineStateLabel } from '../../lib/status';
import { focusStep, groupByPhase } from '../../model/steps';
import { busyKeyLabel, useMachinesStore } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import type { MachineSummary } from '../../types/backend';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import JourneyStrip from '../ui/JourneyStrip.vue';
import Spinner from '../ui/Spinner.vue';

const { machine } = defineProps<{ machine: MachineSummary }>();
const ui = useUi();
const machines = useMachinesStore();

const host = computed(() => machines.host.value);
const steps = computed(() => machines.journey(machine.id));
const focus = computed(() => focusStep(steps.value));
const complete = computed(() => groupByPhase(steps.value).every((group) => group.complete));
const archive = computed(() => steps.value.find((step) => step.id === 'archive') ?? null);

const canStart = computed(
    () =>
        machine.busyOperation === null &&
        (host.value?.ready ?? false) &&
        (machine.state === 'missing' || machine.state === 'exited' || machine.state === 'created'),
);

/** What the row says is next, and whether the person or BuildBridge does it. */
const next = computed(() => {
    if (machine.busyOperation) {
        return {
            title: busyKeyLabel[machine.busyOperation] ?? machine.busyOperation,
            note: 'in progress',
        };
    }
    if (!focus.value) {
        return complete.value && archive.value
            ? {
                  title: `signed ${archive.value.summary.split(' · ')[0]}`,
                  note: 'ready to build again',
              }
            : { title: 'All done', note: '' };
    }
    const owner = focus.value.kind === 'automatic' ? 'BuildBridge does this' : 'you do this';
    return {
        title: focus.value.title,
        note:
            focus.value.status === 'failed'
                ? 'needs attention'
                : focus.value.status === 'running'
                  ? 'running'
                  : focus.value.expected
                    ? `${owner} · usually ${focus.value.expected}`
                    : owner,
    };
});

// The ink button is spent on the machine that needs the person; the rest are outline.
const primary = computed(() => {
    if (machine.busyOperation) {
        return null;
    }
    if (focus.value?.id === 'launch' && canStart.value) {
        return { label: 'Start and continue', ink: true, action: startAndOpen };
    }
    if (focus.value) {
        const step = focus.value;
        return {
            label: step.status === 'failed' ? 'Resolve' : step.title,
            ink: step.kind !== 'automatic' || step.status === 'failed',
            action: () => ui.openMachine(machine.id, step.id),
        };
    }
    return {
        label: 'Build again',
        ink: false,
        action: () => ui.openMachine(machine.id, 'archive'),
    };
});

async function startAndOpen(): Promise<void> {
    ui.openMachine(machine.id, 'launch');
    await machines.launch(machine.id);
}
</script>

<template>
    <article
        class="@container rounded-lg border border-zinc-200 bg-white p-3.5 dark:border-zinc-800 dark:bg-zinc-900"
    >
        <div
            class="grid items-center gap-x-5 gap-y-3 @2xl:grid-cols-[minmax(0,1.2fr)_auto_minmax(0,1fr)_auto]"
        >
            <div class="min-w-0">
                <div class="flex flex-wrap items-center gap-2">
                    <h3 class="truncate text-sm font-semibold text-zinc-900 dark:text-zinc-50">
                        {{ machine.config.name }}
                    </h3>
                    <Badge v-if="machine.busyOperation" tone="warn">
                        <Spinner size="h-3 w-3" tone="text-amber-700 dark:text-amber-400" />
                        {{ busyKeyLabel[machine.busyOperation] ?? machine.busyOperation }}
                    </Badge>
                    <Badge v-else :tone="machineStateBadge[machine.state]">
                        {{ machineStateLabel[machine.state] }}
                    </Badge>
                </div>
                <p class="mt-1 font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                    {{ machine.config.memoryGib }} GiB · {{ machine.config.cpuCores }} cores
                    <template v-if="machine.workspaceName"> · {{ machine.workspaceName }}</template>
                    <template v-if="machine.signingKitName">
                        · {{ machine.signingKitName }}</template
                    >
                    <template v-if="machine.envSetName"> · env {{ machine.envSetName }}</template>
                </p>
            </div>

            <JourneyStrip :steps="steps" />

            <div class="min-w-0">
                <p
                    class="truncate text-xs font-medium"
                    :class="
                        focus?.status === 'failed'
                            ? 'text-red-700 dark:text-red-400'
                            : 'text-zinc-900 dark:text-zinc-50'
                    "
                >
                    {{ next.title }}
                </p>
                <p v-if="next.note" class="mt-0.5 text-[11px] text-zinc-500 dark:text-zinc-400">
                    {{ next.note }}
                </p>
            </div>

            <div class="flex items-center gap-1.5 @2xl:justify-end">
                <Button
                    v-if="primary"
                    :variant="primary.ink ? 'default' : 'outline'"
                    size="sm"
                    @click="primary.action()"
                >
                    <Play v-if="primary.label === 'Start and continue'" class="h-3.5 w-3.5" />
                    {{ primary.label }}
                </Button>
                <Button variant="ghost" size="sm" @click="ui.openMachine(machine.id)">
                    Open
                    <ArrowRight class="h-3 w-3" />
                </Button>
            </div>
        </div>
    </article>
</template>
