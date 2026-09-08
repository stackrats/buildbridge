<script setup lang="ts">
// The test build and its choices in one place: which source, which environment, which SDK,
// which version. The latest source is copied into the guest first, through the guided flow;
// the saved snapshot is compiled as it is.
import { FileCheck, Hammer, ScrollText } from '@lucide/vue';
import { computed, onMounted, ref, watch } from 'vue';

import { percent } from '../../../lib/format';
import type { BuildRequest } from '../../../model/build-flow';
import { describeEnvSetSize } from '../../../model/envs';
import { projectPhaseLabel } from '../../../model/phases';
import { describeSnapshot } from '../../../model/snapshot';
import { unsignedBuildTargetOptions, type JourneyStep } from '../../../model/steps';
import { useBuildFlowStore } from '../../../stores/build-flow';
import { useEnvSetsStore } from '../../../stores/envs';
import { activityLabel, useMachinesStore, type MachineSession } from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import type { UnsignedBuildTarget } from '../../../types/backend';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import Field from '../../ui/Field.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import Select from '../../ui/Select.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';
import VersionFields from '../VersionFields.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const ui = useUi();

const view = computed(() => session.view!);
const workspace = computed(() => view.value.appleWorkspace);
// The choices live in the machine's build draft, shared with the other build steps.
const flows = useBuildFlowStore();
const draft = flows.draft(session.id);
const envs = useEnvSetsStore();
onMounted(() => void envs.load());
const simulatorRuntime = computed(() => session.view!.guest.diagnostics.iosSimulatorRuntime);
const busy = computed(() => session.operation !== null || flows.active(session.id));
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
// SDK is the default and is what signed archives and phone builds compile against. Newer
// Xcodes can still require a one-time iOS platform download for that target.
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
const targetOptions = computed(() => unsignedBuildTargetOptions(simulatorRuntime.value));

// Latest source copies the folder again before compiling; the saved snapshot compiles what
// was copied last time. The environment travels with the copy, so it is chosen only then.
const source = computed<BuildRequest['source']>({
    get: () => draft.source,
    set: (value) => {
        draft.source = value;
    },
});
const snapshot = computed(() => workspace.value?.lastSnapshotSha256 ?? null);
const snapshotSummary = computed(() => describeSnapshot(view.value));
const envSetId = ref(view.value.envSet?.id ?? '');
const envOptions = computed(() => [
    { value: '', label: 'No environment', description: 'The project configuration alone' },
    ...envs.sets.value.map((set) => ({
        value: set.id,
        label: set.name,
        description: describeEnvSetSize(set),
    })),
]);
function build(): void {
    if (busy.value) return;
    void flows.start(
        session.id,
        {
            source: source.value,
            outcome: 'test',
            target: target.value,
            envSetId: source.value === 'latest' ? envSetId.value || null : null,
            androidOutputs: draft.androidOutputs,
            androidAllowHttp: draft.androidAllowHttp,
            version: draft.version ?? null,
        },
        source.value === 'latest' ? (envs.setById(envSetId.value)?.name ?? null) : null,
    );
}
</script>

<template>
    <StepPanel :step="step">
        <template #action>
            <Button
                size="sm"
                :disabled="
                    busy || step.status === 'pending' || (source === 'snapshot' && !snapshot)
                "
                :title="
                    (source === 'latest'
                        ? 'Copies the latest local source into the guest, then compiles the App scheme with signing disabled'
                        : 'Compiles the saved snapshot with signing disabled') +
                    (downloadsSimulator
                        ? '; downloads the iOS Simulator platform from Apple into the guest first'
                        : '; installs the iOS platform first only if Xcode refuses to build without it')
                "
                @click="build"
            >
                <Spinner
                    v-if="session.operation === 'test-build' || flows.active(session.id)"
                    tone="text-white dark:text-zinc-950"
                />
                <Hammer v-else class="h-3.5 w-3.5" />
                {{ source === 'latest' ? 'Build latest source' : 'Rebuild saved snapshot' }}
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

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Checks that your project compiles without signing credentials. This step does not
                launch the app, including when Simulator is selected. The latest local source is
                copied into the guest first; the saved snapshot compiles what was copied last time.
            </p>
            <div class="grid gap-4 sm:grid-cols-2">
                <Field
                    label="Source"
                    :hint="
                        source === 'latest'
                            ? 'Copies the current files from the approved folder, including local edits, and replaces the snapshot.'
                            : `The saved snapshot is compiled as it is: ${snapshotSummary}. New local edits are not included.`
                    "
                >
                    <Select
                        v-model="source"
                        :disabled="busy"
                        :options="[
                            { value: 'latest', label: 'Latest local source' },
                            {
                                value: 'snapshot',
                                label: 'Saved snapshot',
                                description: snapshotSummary ?? undefined,
                                disabled: !snapshot,
                            },
                        ]"
                    />
                </Field>
                <Field
                    v-if="source === 'latest'"
                    label="Environment"
                    hint="Written into the guest with the snapshot and applied to the web build. The next build starts from the same choice."
                >
                    <Select v-model="envSetId" :options="envOptions" :disabled="busy" />
                </Field>
                <p v-else class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    The saved snapshot keeps the environment it was copied with. Choose the latest
                    local source to change it.
                </p>
                <Field
                    label="Compile target"
                    :hint="
                        target === 'simulator'
                            ? 'Compiles for the Simulator; this does not open or run one. A missing runtime is a large download.'
                            : 'Compiles against the device SDK inside Xcode, the target signed archives and phone builds use.'
                    "
                >
                    <Select
                        v-model="target"
                        :options="targetOptions"
                        :disabled="busy || step.status === 'pending'"
                    />
                </Field>
            </div>
            <VersionFields :session="session" :disabled="busy" />
        </div>

        <template #details>
            Compiles the project's scheme with signing disabled. The device SDK is also used by
            signed archives and phone builds. Xcode may require a one-time iOS platform download for
            it; the Simulator target always requires that platform. The first run on a machine
            bootstraps pinned Node, the project's package manager, Ruby and CocoaPods, and Flutter
            for a Flutter project, then installs the dependencies. If this desktop restarts
            mid-build, running it again reattaches to the job instead of starting a second one.
        </template>
    </StepPanel>
</template>
