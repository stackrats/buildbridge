<script setup lang="ts">
import { Hammer, ScrollText } from '@lucide/vue';
import { computed } from 'vue';

import { percent } from '../../../lib/format';
import type { JourneyStep } from '../../../model/steps';
import { projectPhaseLabel } from '../../../model/phases';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const ui = useUi();

const workspace = computed(() => session.view!.appleWorkspace);
const busy = computed(() => session.operation !== null);
const building = computed(() => step.status === 'running');
const progress = computed(() => session.project);
const lastLine = computed(() => session.buildLog.at(-1)?.text ?? null);
const failure = computed(() =>
    session.lastFailure?.operation === 'test-build' ? session.lastFailure : null,
);
</script>

<template>
    <StepPanel :step="step">
        <template #action>
            <Button
                size="sm"
                :disabled="busy || step.status === 'pending'"
                @click="machines.testBuild(session.id)"
            >
                <Spinner
                    v-if="session.operation === 'test-build'"
                    tone="text-white dark:text-zinc-950"
                />
                <Hammer v-else class="h-3.5 w-3.5" />
                {{
                    workspace?.lastBuildSucceeded
                        ? 'Run the test build again'
                        : 'Run the test build'
                }}
            </Button>
            <Button
                v-if="session.buildLog.length"
                variant="ghost"
                size="sm"
                @click="ui.openLog(session.id, 'build')"
            >
                <ScrollText class="h-3.5 w-3.5" />
                Show log
            </Button>
        </template>

        <template v-if="building || failure || workspace?.lastNativeLockUpdated" #status>
            <ProgressRow
                stoppable
                :stopping="session.cancelling"
                @stop="machines.cancelOperation(session.id)"
                v-if="building"
                :label="progress ? projectPhaseLabel[progress.phase] : 'Preparing'"
                :detail="progress?.detail"
                :elapsed-seconds="progress?.elapsedSeconds ?? null"
                :value="progress ? percent(progress.completedBytes, progress.totalBytes) : null"
                :completed-bytes="progress?.totalBytes ? progress.completedBytes : null"
                :total-bytes="progress?.totalBytes ?? null"
                :last-line="lastLine"
            />
            <FailureBlock
                v-if="failure && !building"
                title="The last test build failed"
                cause="The diagnostic lines are kept in the log. Fix the project on the host, synchronize again, and run the build again."
                :diagnostic="failure.message"
            >
                <template #actions>
                    <Button variant="outline" size="sm" @click="ui.openLog(session.id, 'build')">
                        Show log
                    </Button>
                </template>
            </FailureBlock>
            <Callout
                v-if="workspace?.lastNativeLockUpdated"
                tone="warn"
                title="The guest refreshed Podfile.lock"
            >
                Generated Capacitor plugin metadata had drifted from the committed lock. The change
                stayed inside the guest; commit an updated Podfile.lock on the host and synchronize
                again before a signed build, which fails closed on drift.
            </Callout>
        </template>

        <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
            Compiles the App scheme for a generic iOS Simulator with signing disabled, proving the
            toolchain before any certificate is involved. The first run on a machine bootstraps
            pinned Node, pnpm, and CocoaPods, installs locked dependencies, and downloads Apple's
            iOS Simulator platform when Xcode lacks it: several gigabytes that persist for later
            builds. If this desktop restarts mid-build, running it again reattaches to the job
            instead of starting a second one.
        </p>
    </StepPanel>
</template>
