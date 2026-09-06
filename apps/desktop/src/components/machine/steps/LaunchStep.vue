<script setup lang="ts">
import { Play, Square } from '@lucide/vue';
import { computed } from 'vue';

import { isAndroid } from '../../../model/providers';
import type { JourneyStep } from '../../../model/steps';
import { launchPhaseLabel, templateSavePhaseLabel } from '../../../model/phases';
import { isLive } from '../../../lib/status';
import { activityLabel, useMachinesStore, type MachineSession } from '../../../stores/machines';
import Button from '../../ui/Button.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();

const view = computed(() => session.view!);
const live = computed(() => isLive(view.value.runtime.state));
const busy = computed(() => session.operation !== null || view.value.busyOperation !== null);
const dockur = computed(() => view.value.profile.provider === 'dockur_macos');
const android = computed(() => isAndroid(view.value.profile.provider));
const running = computed(() => step.status === 'running');
const failure = computed(() =>
    session.lastFailure?.operation === 'launch' ? session.lastFailure : null,
);

interface Strip {
    label: string;
    detail: string | null;
    elapsed: number | null;
    /** 0–1 when the operation says how far it is. */
    value: number | null;
    stoppable: boolean;
    stopTitle?: string;
}

// Starting is the step's own operation, with phases and a Stop. A stop, a discard, a delete or
// a template save runs on this step too, and each says so rather than borrowing the start's
// strip, whose Stop would cancel the stop itself.
const strip = computed<Strip>(() => {
    const operation = session.operation ?? view.value.busyOperation;
    if (operation === 'launch' || operation === 'starting') {
        const progress = session.launch;
        return {
            label: progress ? launchPhaseLabel[progress.phase] : 'Starting',
            detail: progress?.detail ?? null,
            elapsed: progress?.elapsedSeconds ?? null,
            value: null,
            stoppable: true,
        };
    }
    if (operation === 'stop' || operation === 'stopping') {
        return {
            label: android.value ? 'Stopping the toolchain' : 'Stopping the machine safely',
            detail: android.value
                ? 'the container and its home on this host are kept'
                : 'the container and its macOS disk are kept',
            elapsed: null,
            value: null,
            stoppable: false,
        };
    }
    if (operation === 'save-template' || operation === 'saving_template') {
        // The save runs with the machine stopped and its dialog gone, so this strip is where
        // it is followed. Only a save this client started can be stopped from here.
        const progress = session.templateSave;
        return {
            label: progress
                ? templateSavePhaseLabel[progress.phase]
                : 'Saving the machine as a template',
            detail: progress?.detail ?? null,
            elapsed: progress?.elapsedSeconds ?? null,
            value: progress?.percent == null ? null : progress.percent / 100,
            stoppable: session.operation === 'save-template',
            stopTitle: 'Stop the save. The partial template is removed; the machine stays stopped.',
        };
    }
    return {
        label: activityLabel(operation) ?? 'Working',
        detail: null,
        elapsed: null,
        value: null,
        stoppable: false,
    };
});
</script>

<template>
    <StepPanel :step="step">
        <template #action>
            <Button
                v-if="!live"
                size="sm"
                :title="
                    view.runtime.state === 'missing'
                        ? android
                            ? 'The first start pulls the JDK image, a few hundred megabytes'
                            : `The first start pulls the ${dockur ? 'dockur/macos' : 'Docker-OSX'} image`
                        : undefined
                "
                :disabled="busy || !view.runtime.prerequisites.ready"
                @click="machines.launch(session.id)"
            >
                <Spinner
                    v-if="session.operation === 'launch'"
                    tone="text-white dark:text-zinc-950"
                />
                <Play v-else class="h-3.5 w-3.5" />
                {{ view.runtime.state === 'missing' ? 'Create and start' : 'Start' }}
            </Button>
            <Button
                v-else
                variant="outline"
                size="sm"
                :title="
                    android
                        ? 'Keeps the container and its home'
                        : 'Keeps the container and its disk'
                "
                :disabled="busy"
                @click="machines.stop(session.id)"
            >
                <Spinner v-if="session.operation === 'stop'" />
                <Square v-else class="h-3.5 w-3.5 text-red-700 dark:text-red-400" />
                Stop safely
            </Button>
        </template>

        <template v-if="running || step.status === 'failed' || failure" #status>
            <ProgressRow
                v-if="running"
                :label="strip.label"
                :detail="strip.detail"
                :elapsed-seconds="strip.elapsed"
                :value="strip.value"
                :stoppable="strip.stoppable"
                :stopping="session.cancelling"
                :stop-title="strip.stopTitle"
                @stop="machines.cancelOperation(session.id)"
            />
            <FailureBlock
                v-else-if="step.status === 'failed'"
                title="The machine cannot run right now"
                :cause="step.summary"
            />
            <FailureBlock
                v-else-if="failure"
                title="The last start failed"
                :diagnostic="failure.message"
            />
        </template>

        <p v-if="android" class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
            The first start pulls the pinned JDK image and creates a container that does nothing but
            wait, with a memory and CPU limit on its builds and its home bound from this host. No
            virtual machine, no KVM, no ports. The Android SDK, Node and the Gradle caches are
            downloaded into that home by the first build and survive stops; only the explicit
            discard action in the machine menu removes them.
        </p>
        <p v-else-if="dockur" class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
            The first start pulls the dockur/macos image and creates the container with fixed,
            non-privileged arguments; the machine then downloads macOS from Apple into its storage
            directory on this host and generates its own identity. Its screen is a web page, opened
            from the Install step. Stopping preserves the container and its disk; only the explicit
            discard action in the machine menu removes them.
        </p>
        <p v-else class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
            The first start pulls the Docker-OSX image, generates a stable machine identity, and
            creates the container with fixed, non-privileged arguments. A compact console window
            opens for the macOS installer. Stopping preserves the container and its disk; only the
            explicit discard action in the machine menu removes them.
        </p>
    </StepPanel>
</template>
