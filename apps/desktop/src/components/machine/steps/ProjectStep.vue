<script setup lang="ts">
// The project as one step: the folder is approved once, read-only, and the snapshot copied
// from it is what every build compiles. Building the latest source refreshes the snapshot on
// its own; synchronizing here refreshes it without building.
import { FolderCheck, RefreshCw } from '@lucide/vue';
import { computed, ref } from 'vue';

import { formatBytes, percent, relativeTime } from '../../../lib/format';
import { projectPhaseLabel } from '../../../model/phases';
import { describeProjectKind } from '../../../model/project-layout';
import type { JourneyStep } from '../../../model/steps';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import Button from '../../ui/Button.vue';
import ConfirmDialog from '../../dialogs/ConfirmDialog.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import Field from '../../ui/Field.vue';
import KeyValue from '../../ui/KeyValue.vue';
import PathField from '../../ui/PathField.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import Select from '../../ui/Select.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const ui = useUi();

const view = computed(() => session.view!);
const workspace = computed(() => view.value.appleWorkspace);
const busy = computed(() => session.operation !== null);
const path = ref(workspace.value?.localPath ?? '');
const removeOpen = ref(false);
const syncing = computed(() => session.operation === 'sync');
const progress = computed(() => session.project);
const failure = computed(() =>
    session.lastFailure?.operation === 'approve' || session.lastFailure?.operation === 'sync'
        ? session.lastFailure
        : null,
);

// What the folder must hold, and what is read from it: the detector names the framework in
// front of the Xcode project from the files it finds, and never runs the project's own tools.
const requirements = [
    'An Xcode workspace or project with an application target, where the app keeps it',
    'Capacitor, Cordova, React Native, Expo or Flutter in front of it, or nothing at all',
    'Podfile.lock committed beside the Podfile, when the project uses CocoaPods',
    'The lockfile of the package manager the project installs with, when it has one',
];

// The scheme is chosen among the project's shared schemes and application targets; choosing
// one approves the same folder again with it.
const schemeOptions = computed(() => {
    const ios = workspace.value?.layout.ios;
    if (!ios) {
        return [];
    }
    const names = [...ios.schemes];
    for (const target of ios.appTargets) {
        if (!names.includes(target.name)) {
            names.push(target.name);
        }
    }
    return names.map((name) => ({
        value: name,
        label: name,
        description: ios.schemes.includes(name)
            ? 'A shared scheme of the project'
            : 'An application target; buildbridge writes its scheme where the build runs',
    }));
});
const scheme = computed({
    get: () => workspace.value?.scheme ?? '',
    set: (value: string) => {
        if (workspace.value && value && value !== workspace.value.scheme) {
            void machines.approveWorkspace(session.id, workspace.value.localPath, value);
        }
    },
});

async function remove(): Promise<void> {
    removeOpen.value = false;
    await machines.clearWorkspace(session.id);
}
</script>

<template>
    <StepPanel :step="step">
        <template v-if="workspace" #action>
            <Button
                size="sm"
                :disabled="busy || step.status === 'pending'"
                title="Copies a fresh snapshot of the approved folder into the guest without building it"
                @click="machines.sync(session.id)"
            >
                <Spinner v-if="syncing" tone="text-white dark:text-zinc-950" />
                <RefreshCw v-else class="h-3.5 w-3.5" />
                {{ workspace.lastSnapshotSha256 ? 'Synchronize again' : 'Synchronize source' }}
            </Button>
            <Button
                v-if="session.buildLog.length"
                variant="ghost"
                size="sm"
                @click="ui.openLog(session.id, 'build')"
            >
                Show log
            </Button>
            <Button variant="ghost" size="sm" :disabled="busy" @click="removeOpen = true">
                <Spinner v-if="session.operation === 'clear-workspace'" />
                Remove approval
            </Button>
        </template>

        <template v-if="syncing || failure" #status>
            <ProgressRow
                v-if="syncing"
                stoppable
                :stopping="session.cancelling"
                :label="progress ? projectPhaseLabel[progress.phase] : 'Preparing'"
                :detail="progress?.detail"
                :elapsed-seconds="progress?.elapsedSeconds ?? null"
                :value="progress ? percent(progress.completedBytes, progress.totalBytes) : null"
                :completed-bytes="progress?.completedBytes ?? null"
                :total-bytes="progress?.totalBytes ?? null"
                @stop="machines.cancelOperation(session.id)"
            />
            <FailureBlock
                v-else-if="failure"
                :title="
                    failure.operation === 'sync'
                        ? 'The last synchronization failed'
                        : 'That folder was not approved'
                "
                :cause="
                    failure.operation === 'sync'
                        ? null
                        : 'The folder must hold an iOS app: an Xcode workspace or project, with or without a framework in front of it. What buildbridge looks for is listed below.'
                "
                :diagnostic="failure.message"
            >
                <template v-if="failure.operation === 'sync'" #actions>
                    <Button variant="outline" size="sm" @click="ui.openLog(session.id, 'build')">
                        Show log
                    </Button>
                </template>
            </FailureBlock>
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Approval is read-only: buildbridge detects what kind of project the folder holds,
                the Xcode workspace or project and scheme to build, and the team and release bundle
                identifier; only this one folder is ever read. Every build of the latest source
                copies a bounded, checksummed snapshot of it into the guest over the pinned bridge,
                leaving out Git metadata,
                <span class="font-mono">node_modules</span>, build output, every
                <span class="font-mono">.env</span> file, keys, certificates, and profiles.
                Synchronizing here refreshes that snapshot without building.
            </p>

            <KeyValue
                v-if="workspace"
                :items="[
                    { label: 'Project', value: workspace.name },
                    { label: 'Folder', value: workspace.localPath, mono: true },
                    {
                        label: 'Team',
                        value: workspace.developmentTeam,
                        tone: workspace.developmentTeam ? 'default' : 'warn',
                    },
                    {
                        label: 'Bundle identifier',
                        value: workspace.bundleIdentifier,
                        mono: true,
                        tone: workspace.bundleIdentifier ? 'default' : 'warn',
                    },
                    {
                        label: 'Project kind',
                        value: describeProjectKind(workspace.layout),
                        copyable: false,
                    },
                    { label: 'Xcode opens', value: workspace.layout.ios?.container, mono: true },
                    { label: 'Scheme', value: workspace.scheme },
                ]"
            />
            <KeyValue
                v-if="workspace?.lastSnapshotSha256"
                :items="[
                    { label: 'Snapshot SHA-256', value: workspace.lastSnapshotSha256, mono: true },
                    {
                        label: 'Copied',
                        value: workspace.lastSyncedAtEpochSeconds
                            ? relativeTime(
                                  new Date(workspace.lastSyncedAtEpochSeconds * 1000).toISOString(),
                              )
                            : 'before the time was recorded',
                        copyable: false,
                    },
                    { label: 'Files', value: workspace.lastSyncFileCount },
                    { label: 'Source size', value: formatBytes(workspace.lastSyncBytes) },
                    {
                        label: 'Source',
                        value:
                            workspace.lastSource?.kind === 'git'
                                ? `${workspace.lastSource.gitRef} at ${workspace.lastSource.commit?.slice(0, 12)} (remote build)`
                                : 'Approved folder as it was',
                    },
                    {
                        label: 'Environment',
                        value: view.envSet?.name ?? 'Project configuration only',
                        copyable: false,
                    },
                ]"
                :columns="3"
            />

            <Field
                :label="workspace ? 'Approve a different folder' : 'Project folder on this host'"
                hint="Any iOS app: Capacitor, Cordova, React Native, Expo, Flutter, or a plain Xcode project. Drop the folder here or paste its absolute path."
            >
                <PathField
                    v-model="path"
                    kind="directory"
                    title="Choose the project folder"
                    placeholder="/path/to/your-app"
                    :disabled="busy || step.status === 'pending'"
                />
                <template #action>
                    <Button
                        :disabled="busy || step.status === 'pending' || !path.trim()"
                        @click="machines.approveWorkspace(session.id, path.trim())"
                    >
                        <Spinner
                            v-if="session.operation === 'approve'"
                            tone="text-white dark:text-zinc-950"
                        />
                        <FolderCheck v-else class="h-3.5 w-3.5" />
                        {{ workspace ? 'Re-approve' : 'Approve project' }}
                    </Button>
                </template>
            </Field>
            <Field
                v-if="workspace && schemeOptions.length > 1"
                label="Scheme to build"
                hint="The project offers more than one. Choosing approves the same folder again with it."
            >
                <Select
                    v-model="scheme"
                    :options="schemeOptions"
                    :disabled="busy || step.status === 'pending'"
                />
            </Field>
            <ul class="grid gap-1 text-xs text-zinc-500 sm:grid-cols-2 dark:text-zinc-400">
                <li v-for="item in requirements" :key="item">· {{ item }}</li>
            </ul>
        </div>

        <ConfirmDialog
            v-model:open="removeOpen"
            title="Remove the project approval"
            confirm-label="Remove approval"
            :destructive="false"
            @confirm="remove"
        >
            <p>
                buildbridge forgets this folder and the sync and build state recorded for it. The
                host project and the guest workspace are not modified.
            </p>
        </ConfirmDialog>
    </StepPanel>
</template>
