<script setup lang="ts">
import { Hammer, ScrollText } from '@lucide/vue';
import { computed, ref, watch } from 'vue';

import { percent } from '../../../lib/format';
import { projectPhaseLabel } from '../../../model/phases';
import type { JourneyStep } from '../../../model/steps';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import type { UnsignedBuildTarget } from '../../../types/backend';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import Select from '../../ui/Select.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const ui = useUi();

const workspace = computed(() => session.view!.appleWorkspace);
const simulatorRuntime = computed(() => session.view!.guest.diagnostics.iosSimulatorRuntime);
const busy = computed(() => session.operation !== null);
const building = computed(() => step.status === 'running');
const progress = computed(() => session.project);
const lastLine = computed(() => session.buildLog.at(-1)?.text ?? null);
const failure = computed(() =>
    session.lastFailure?.operation === 'test-build' ? session.lastFailure : null,
);

// The target follows whatever last built here, so "again" repeats the same build. The device
// SDK is the default: it ships inside Xcode, needs nothing downloaded, and is what the signed
// archive and the phone build compile against.
const target = ref<UnsignedBuildTarget>(workspace.value?.lastBuildTarget ?? 'device_sdk');
watch(
    () => workspace.value?.lastBuildTarget,
    (value) => {
        if (value) {
            target.value = value;
        }
    },
);
const downloadsSimulator = computed(
    () => target.value === 'simulator' && simulatorRuntime.value === null,
);
const targetOptions = computed(() => [
    { value: 'device_sdk', label: 'iOS device SDK · no download' },
    {
        value: 'simulator',
        label: simulatorRuntime.value
            ? `iOS Simulator · runtime ${simulatorRuntime.value} installed`
            : 'iOS Simulator · about 8 GB download',
    },
]);
</script>

<template>
    <StepPanel :step="step">
        <template #action>
            <span class="w-72 max-w-full">
                <Select
                    v-model="target"
                    :options="targetOptions"
                    size="sm"
                    :disabled="busy || step.status === 'pending'"
                />
            </span>
            <Button
                size="sm"
                :disabled="busy || step.status === 'pending'"
                :title="
                    downloadsSimulator
                        ? 'Downloads Apple\'s iOS Simulator platform into the guest first, then builds'
                        : 'Compiles the App scheme with signing disabled'
                "
                @click="machines.testBuild(session.id, target)"
            >
                <Spinner
                    v-if="session.operation === 'test-build'"
                    tone="text-white dark:text-zinc-950"
                />
                <Hammer v-else class="h-3.5 w-3.5" />
                {{
                    downloadsSimulator
                        ? 'Download the Simulator and run the test build'
                        : workspace?.lastBuildSucceeded
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

        <template
            v-if="building || failure || workspace?.lastNativeLockUpdated || downloadsSimulator"
            #status
        >
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
                v-if="downloadsSimulator && !building"
                tone="neutral"
                title="This target downloads Apple's iOS Simulator platform"
            >
                About 8 GB from Apple into the guest disk, once per machine, with byte progress
                shown here. Nothing else in BuildBridge needs it: signed archives and phone builds
                use the SDK inside Xcode. Choose the device SDK if you only want those.
            </Callout>
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
            Compiles the App scheme with signing disabled, proving the toolchain before any
            certificate is involved. The device SDK ships inside Xcode and is what the signed
            archive and the phone build use, so it downloads nothing; the Simulator is the only
            target that can run on screen inside the guest, and it needs Apple's iOS Simulator
            platform first. The first run on a machine bootstraps pinned Node, pnpm, Ruby, and
            CocoaPods and installs locked dependencies. If this desktop restarts mid-build, running
            it again reattaches to the job instead of starting a second one.
        </p>
    </StepPanel>
</template>
