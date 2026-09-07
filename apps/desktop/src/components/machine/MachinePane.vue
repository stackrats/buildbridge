<script setup lang="ts">
// One page per machine: the whole journey as a timeline, with each build's choices on the step
// that builds, so a preview or a release is a step opened in place rather than another page.
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
import { isAndroid } from '../../model/providers';
import { useBuildFlowStore } from '../../stores/build-flow';
import { useMachinesStore } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import StepTimeline from '../ui/StepTimeline.vue';
import ArtifactsSection from './ArtifactsSection.vue';
import GuidedBuildStatus from './GuidedBuildStatus.vue';
import LogDrawer from './LogDrawer.vue';
import MachineHeader from './MachineHeader.vue';
import MachineLoading from './MachineLoading.vue';
import OptimizationsSection from './OptimizationsSection.vue';
import StepDetail from './StepDetail.vue';

const { machineId } = defineProps<{ machineId: string }>();
const ui = useUi();
const machines = useMachinesStore();
const flows = useBuildFlowStore();

const session = computed(() => machines.session(machineId));
const view = computed(() => session.value.view);
const summary = computed(() => machines.machines.value.find((machine) => machine.id === machineId));

const now = ref(Date.now());
const runningStep = computed(() => machines.runningStep(machineId));
const runningOperation = computed(() => machines.runningOperation(machineId));
const runningLive = computed(() => machines.runningLive(machineId));
const steps = computed<JourneyStep[]>(() =>
    view.value
        ? deriveJourney(view.value, {
              runningStep: runningStep.value,
              runningOperation: runningOperation.value,
              runningLive: runningLive.value,
              now: now.value,
          })
        : [],
);

const focus = computed(() => focusStep(steps.value));
const headline = computed(() => journeyHeadline(steps.value));
// Which phases and sections a person folded or opened on this machine, kept for the session.
const sections = computed<Record<string, boolean>>({
    get: () => ui.state.machineSections[machineId] ?? {},
    set: (value) => {
        ui.state.machineSections[machineId] = value;
    },
});
function openStep(id: JourneyStepId): void {
    ui.selectStep(machineId, id);
}
function openResult(): void {
    const build = flows.builds[machineId];
    openStep(
        build?.request.outcome === 'release'
            ? isAndroid(view.value!.profile.provider)
                ? 'release'
                : 'archive'
            : 'test-build',
    );
}

// What each phase achieved, for its header once the phase folds away. Setup is the one you
// stop thinking about, so its line is the toolchain every later build reuses.
const summaries = computed<Partial<Record<StepPhase, string>>>(() => {
    const current = view.value;
    if (!current) {
        return {};
    }
    if (isAndroid(current.profile.provider)) {
        const lastBuild = current.android?.workspace?.lastBuild ?? null;
        const release = current.android?.release ?? null;
        return {
            setup: current.runtime.state === 'running' ? 'toolchain running' : '',
            build: [
                lastBuild
                    ? lastBuild.toolchain.jdkVersion
                          .replace(/^openjdk version /, 'JDK ')
                          .replace(/"/g, '')
                          .split(' ')
                          .slice(0, 2)
                          .join(' ')
                    : null,
                release
                    ? `${release.versionName} (${release.versionCode}) · ${[release.aab ? `AAB ${formatBytes(release.aab.bytes)}` : null, release.apk ? `APK ${formatBytes(release.apk.bytes)}` : null].filter(Boolean).join(' · ')}`
                    : null,
            ]
                .filter(Boolean)
                .join(' · '),
        };
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
    if (
        !current ||
        isAndroid(current.profile.provider) ||
        current.runtime.state !== 'running' ||
        current.guest.ssh.reachable
    ) {
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
        <div class="relative min-h-0 flex-1 overflow-y-auto">
            <div
                class="mx-auto w-full max-w-4xl p-5"
                :class="{ 'flex min-h-full flex-col': !view }"
            >
                <MachineLoading
                    v-if="!view && (session.loading || !session.error)"
                    :machine="summary"
                />
                <Callout v-else-if="!view && session.error" tone="danger">{{
                    session.error
                }}</Callout>

                <template v-else-if="view">
                    <MachineHeader :session="session" :now="now" />

                    <!-- Only a failure earns a banner: what an operation achieved is on its row
                         and its panel, so a notice that repeated it would say nothing new. -->
                    <Callout
                        v-if="session.error && flows.builds[machineId]?.status !== 'failed'"
                        tone="danger"
                        class="mt-4"
                    >
                        <p>{{ session.error }}</p>
                        <Button class="mt-2" size="sm" variant="ghost" @click="session.error = null"
                            >Dismiss</Button
                        >
                    </Callout>

                    <div class="mt-5 space-y-5">
                        <GuidedBuildStatus
                            :session="session"
                            :now="now"
                            @review="openStep"
                            @results="openResult"
                        />
                        <StepTimeline
                            v-model:selected="selected"
                            v-model:expanded="sections"
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

                    <ArtifactsSection :session="session" />
                    <OptimizationsSection
                        v-if="!isAndroid(view.profile.provider)"
                        :session="session"
                    />
                </template>
            </div>
        </div>
        <LogDrawer v-if="view" :session="session" :running-step="runningStep" :now="now" />
    </div>
</template>
