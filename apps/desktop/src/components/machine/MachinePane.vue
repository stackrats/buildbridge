<script setup lang="ts">
// One machine as one journey: the header, the full-width timeline with the open step showing
// everything it has, and the log drawer under it. There is one layout at every window size.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';

import {
    deriveJourney,
    focusStep,
    journeyHeadline,
    type JourneyStep,
    type JourneyStepId,
    type StepPhase,
} from '../../model/steps';
import { formatBytes } from '../../lib/format';
import { useMachinesStore } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import Callout from '../ui/Callout.vue';
import Spinner from '../ui/Spinner.vue';
import StepTimeline from '../ui/StepTimeline.vue';
import LogDrawer from './LogDrawer.vue';
import MachineHeader from './MachineHeader.vue';
import OptimizationsSection from './OptimizationsSection.vue';
import StepDetail from './StepDetail.vue';

const { machineId } = defineProps<{ machineId: string }>();
const ui = useUi();
const machines = useMachinesStore();

const session = computed(() => machines.session(machineId));
const view = computed(() => session.value.view);

const now = ref(Date.now());
const runningStep = computed(() => machines.runningStep(machineId));
const steps = computed<JourneyStep[]>(() =>
    view.value ? deriveJourney(view.value, { runningStep: runningStep.value, now: now.value }) : [],
);

const focus = computed(() => focusStep(steps.value));
const headline = computed(() => journeyHeadline(steps.value));

// What each phase achieved, for its header once the phase folds away. Setup is the one you
// stop thinking about, so its line is the toolchain every later build reuses.
const summaries = computed<Partial<Record<StepPhase, string>>>(() => {
    const current = view.value;
    if (!current) {
        return {};
    }
    const { diagnostics, ssh } = current.guest;
    return {
        setup: [
            diagnostics.macosVersion ? `macOS ${diagnostics.macosVersion}` : null,
            diagnostics.xcodeVersion ? `Xcode ${diagnostics.xcodeVersion}` : null,
            ssh.trust === 'trusted' ? 'SSH pinned' : null,
        ]
            .filter(Boolean)
            .join(' · '),
        build: [
            current.archive
                ? `${current.archive.marketingVersion} (${current.archive.buildNumber}) · IPA ${formatBytes(current.archive.ipa.bytes)}`
                : null,
            current.deviceRun ? `ran on ${current.deviceRun.device.name}` : null,
        ]
            .filter(Boolean)
            .join(' · '),
    };
});
const runningSeconds = computed(() =>
    session.value.operationStartedAt
        ? Math.floor((now.value - session.value.operationStartedAt) / 1000)
        : null,
);

// The open step follows the focus until a person opens another one; it follows again once the
// step they opened becomes the focus, which is what happens as steps complete.
const selected = computed<JourneyStepId | null>({
    get: () => {
        const chosen = ui.selectedStep(machineId);
        if (chosen === '') {
            return null;
        }
        if (chosen && steps.value.some((step) => step.id === chosen)) {
            return chosen as JourneyStepId;
        }
        // With nothing to do, land on the last thing achieved or the last required step, so a
        // finished machine opens on its artifacts rather than on an optional experiment.
        return (
            focus.value?.id ??
            [...steps.value].reverse().find((step) => step.status === 'done' || !step.optional)
                ?.id ??
            null
        );
    },
    set: (value) => ui.selectStep(machineId, value),
});
watch(focus, (next, previous) => {
    if (next && ui.selectedStep(machineId) === previous?.id) {
        ui.selectStep(machineId, next.id);
    }
});

// Guest readiness polling: while the machine runs but SSH is not yet reachable, the user is
// installing macOS in the console. Probe every 10 seconds so the timeline advances by itself.
const PROBE_INTERVAL_MS = 10_000;
let probeTimer: ReturnType<typeof setTimeout> | null = null;
let clockTimer: ReturnType<typeof setInterval> | null = null;

function scheduleProbe(): void {
    if (probeTimer) {
        clearTimeout(probeTimer);
        probeTimer = null;
    }
    const current = view.value;
    if (!current || current.runtime.state !== 'running' || current.guest.ssh.reachable) {
        return;
    }
    probeTimer = setTimeout(async () => {
        probeTimer = null;
        if (session.value.operation === null) {
            await machines.refreshMachine(machineId, { silent: true });
        }
        scheduleProbe();
    }, PROBE_INTERVAL_MS);
}

watch(
    () => [view.value?.runtime.state, view.value?.guest.ssh.reachable],
    () => scheduleProbe(),
);

onMounted(async () => {
    clockTimer = setInterval(() => (now.value = Date.now()), 1000);
    await machines.refreshMachine(machineId);
    scheduleProbe();
});

onBeforeUnmount(() => {
    if (probeTimer) {
        clearTimeout(probeTimer);
    }
    if (clockTimer) {
        clearInterval(clockTimer);
    }
});
</script>

<template>
    <div class="flex h-full flex-col">
        <div class="min-h-0 flex-1 overflow-y-auto">
            <div class="mx-auto max-w-4xl p-5">
                <div
                    v-if="!view && session.loading"
                    class="flex items-center gap-2 text-xs text-zinc-500 dark:text-zinc-400"
                >
                    <Spinner />
                    Probing the machine…
                </div>
                <Callout v-else-if="!view && session.error" tone="danger">{{
                    session.error
                }}</Callout>

                <template v-else-if="view">
                    <MachineHeader :session="session" :now="now" />

                    <div v-if="session.error || session.notice" class="mt-4">
                        <Callout v-if="session.error" tone="danger">{{ session.error }}</Callout>
                        <Callout v-else-if="session.notice" tone="ok">{{ session.notice }}</Callout>
                    </div>

                    <div class="mt-5">
                        <StepTimeline
                            v-model:selected="selected"
                            :steps="steps"
                            :headline="headline"
                            :summaries="summaries"
                            :running-seconds="runningSeconds"
                        >
                            <template #detail="{ step }">
                                <StepDetail :key="step.id" :session="session" :step="step" />
                            </template>
                        </StepTimeline>
                    </div>

                    <OptimizationsSection :session="session" />
                </template>
            </div>
        </div>
        <LogDrawer v-if="view" :session="session" :running-step="runningStep" :now="now" />
    </div>
</template>
