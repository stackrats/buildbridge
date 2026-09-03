<script setup lang="ts">
import { Play, Square } from '@lucide/vue';
import { computed } from 'vue';

import type { JourneyStep } from '../../../model/steps';
import { launchPhaseLabel } from '../../../model/phases';
import { isLive } from '../../../lib/status';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
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
const launching = computed(() => step.status === 'running');
const failure = computed(() =>
    session.lastFailure?.operation === 'launch' ? session.lastFailure : null,
);
</script>

<template>
    <StepPanel :step="step">
        <template #action>
            <Button
                v-if="!live"
                size="sm"
                :title="
                    view.runtime.state === 'missing'
                        ? 'The first start pulls the Docker-OSX image'
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
                title="Keeps the container and its disk"
                :disabled="busy"
                @click="machines.stop(session.id)"
            >
                <Spinner v-if="session.operation === 'stop'" />
                <Square v-else class="h-3.5 w-3.5 text-red-700 dark:text-red-400" />
                Stop safely
            </Button>
        </template>

        <template v-if="launching || step.status === 'failed' || failure" #status>
            <ProgressRow
                stoppable
                :stopping="session.cancelling"
                @stop="machines.cancelOperation(session.id)"
                v-if="launching"
                :label="session.launch ? launchPhaseLabel[session.launch.phase] : 'Starting'"
                :detail="session.launch?.detail"
                :elapsed-seconds="session.launch?.elapsedSeconds ?? null"
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

        <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
            The first start pulls the Docker-OSX image, generates a stable machine identity, and
            creates the container with fixed, non-privileged arguments. A compact console window
            opens for the macOS installer. Stopping preserves the container and its disk; only the
            explicit discard action in the machine menu removes them.
        </p>
    </StepPanel>
</template>
