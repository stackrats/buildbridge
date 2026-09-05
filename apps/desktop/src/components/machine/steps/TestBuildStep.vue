<script setup lang="ts">
import { FileCheck, Hammer, ScrollText } from '@lucide/vue';
import { computed, ref, watch } from 'vue';

import { percent } from '../../../lib/format';
import { projectPhaseLabel } from '../../../model/phases';
import type { JourneyStep } from '../../../model/steps';
import { activityLabel, useMachinesStore, type MachineSession } from '../../../stores/machines';
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
const running = computed(() => step.status === 'running');
const lastLine = computed(() => session.buildLog.at(-1)?.text ?? null);

// The build has phases, bytes, a last line and a Stop. Adopting the lockfile runs on this step
// too and is a plain strip, not the last build's phase with a Stop that cancels nothing.
const strip = computed(() => {
    const operation = session.operation ?? session.view!.busyOperation;
    if (operation === 'test-build' || operation === 'test_building') {
        const progress = session.project;
        return {
            label: progress ? projectPhaseLabel[progress.phase] : 'Preparing',
            detail: progress?.detail ?? null,
            elapsed: progress?.elapsedSeconds ?? null,
            value: progress ? percent(progress.completedBytes, progress.totalBytes) : null,
            completedBytes: progress?.totalBytes ? progress.completedBytes : null,
            totalBytes: progress?.totalBytes ?? null,
            lastLine: lastLine.value,
            stoppable: true,
        };
    }
    return {
        label: activityLabel(operation) ?? 'Working',
        detail: null,
        elapsed: null,
        value: null,
        completedBytes: null,
        totalBytes: null,
        lastLine: null,
        stoppable: false,
    };
});
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
    {
        value: 'device_sdk',
        label: 'iOS device SDK · downloads the iOS platform only if Xcode asks',
    },
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
                        : 'Compiles the App scheme with signing disabled; installs the iOS platform first only if Xcode refuses to build without it'
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
            v-if="
                running ||
                failure ||
                workspace?.lastNativeLockUpdated ||
                session.lockAdoption ||
                downloadsSimulator
            "
            #status
        >
            <ProgressRow
                v-if="running"
                :label="strip.label"
                :detail="strip.detail"
                :elapsed-seconds="strip.elapsed"
                :value="strip.value"
                :completed-bytes="strip.completedBytes"
                :total-bytes="strip.totalBytes"
                :last-line="strip.lastLine"
                :stoppable="strip.stoppable"
                :stopping="session.cancelling"
                @stop="machines.cancelOperation(session.id)"
            />
            <FailureBlock
                v-if="failure && !running"
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
                v-if="downloadsSimulator && !running"
                tone="neutral"
                title="This target downloads Apple's iOS Simulator platform"
            >
                About 8 GB from Apple into the guest disk, once per machine, with byte progress
                shown here. Signed archives and phone builds use the SDK inside Xcode, but newer
                Xcodes refuse even those until the platform is installed; the device SDK target then
                installs it the same way, once, and only when Xcode insists.
            </Callout>
            <Callout
                v-if="workspace?.lastNativeLockUpdated"
                tone="warn"
                title="The guest refreshed Podfile.lock"
            >
                <p>
                    CocoaPods resolved different pod versions in the guest than the project's
                    committed Podfile.lock pins, usually because the JavaScript packages moved on.
                    Signed builds fail closed on that drift. Adopt the guest's lock into the project
                    here, then commit it; the build that just passed used exactly that lock, so
                    nothing needs rebuilding.
                </p>
                <div class="mt-2">
                    <Button
                        size="sm"
                        variant="outline"
                        title="Copies the guest's Podfile.lock over the project's, keeping the previous copy"
                        :disabled="busy"
                        @click="machines.adoptGuestLock(session.id)"
                    >
                        <Spinner v-if="session.operation === 'adopt-lock'" />
                        <FileCheck v-else class="h-3.5 w-3.5" />
                        Adopt the guest's Podfile.lock
                    </Button>
                </div>
            </Callout>
            <Callout
                v-else-if="session.lockAdoption"
                tone="ok"
                title="The guest's Podfile.lock is now the project's"
            >
                <div v-if="session.lockAdoption" class="mt-2 space-y-1">
                    <p>
                        Adopted into
                        <span class="font-mono">{{ session.lockAdoption.hostPath }}</span
                        >{{
                            session.lockAdoption.changes.identical
                                ? ', which already matched.'
                                : `: ${session.lockAdoption.changes.pods.length} pod${session.lockAdoption.changes.pods.length === 1 ? '' : 's'} repinned, ${session.lockAdoption.changes.linesAdded} lines in, ${session.lockAdoption.changes.linesRemoved} out.`
                        }}
                        Commit it in the project; the previous copy is kept at
                        <span class="font-mono">{{ session.lockAdoption.backupPath }}</span
                        >.
                    </p>
                    <ul
                        v-if="session.lockAdoption.changes.pods.length"
                        class="font-mono text-[11px] leading-4"
                    >
                        <li v-for="pod in session.lockAdoption.changes.pods" :key="pod.name">
                            {{ pod.name }} · {{ pod.before ?? 'absent' }} →
                            {{ pod.after ?? 'removed' }}
                        </li>
                    </ul>
                </div>
            </Callout>
        </template>

        <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
            Compiles the App scheme with signing disabled, proving the toolchain before any
            certificate is involved. The device SDK ships inside Xcode and is what the signed
            archive and the phone build use; newer Xcodes still ask for Apple's iOS platform before
            building for it, and it is installed once only if they do. The Simulator is the only
            target that can run on screen inside the guest, and it always needs that platform first.
            The first run on a machine bootstraps pinned Node, pnpm, Ruby, and CocoaPods and
            installs locked dependencies. If this desktop restarts mid-build, running it again
            reattaches to the job instead of starting a second one.
        </p>
    </StepPanel>
</template>
