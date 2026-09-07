<script setup lang="ts">
// The Android counterpart of the project step: the same one folder, read-only, checked for the
// Android platform instead of the iOS one, and the same snapshot streamed into the toolchain
// container through docker exec instead of SSH.
import { FolderCheck, RefreshCw } from '@lucide/vue';
import { computed, ref } from 'vue';

import { formatBytes, percent, relativeTime } from '../../../lib/format';
import { androidBuildPhaseLabel } from '../../../model/phases';
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
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const ui = useUi();

const view = computed(() => session.view!);
const workspace = computed(() => view.value.android?.workspace ?? null);
const busy = computed(() => session.operation !== null);
const path = ref(workspace.value?.localPath ?? '');
const removeOpen = ref(false);
const syncing = computed(() => session.operation === 'sync');
const progress = computed(() => session.androidBuild);
const failure = computed(() =>
    session.lastFailure?.operation === 'approve' || session.lastFailure?.operation === 'sync'
        ? session.lastFailure
        : null,
);

const requirements = [
    'package.json and pnpm-lock.yaml',
    'capacitor.config.ts',
    'android/gradlew, committed with its wrapper',
    'android/settings.gradle and android/app/build.gradle',
];

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
                title="Copies a fresh snapshot of the approved folder into the container without building it"
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
                :label="progress ? androidBuildPhaseLabel[progress.phase] : 'Preparing'"
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
                        : 'The folder must hold a Capacitor project with its Android platform added; the files it needs are listed below.'
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
                Approval is read-only: buildbridge validates the project shape and reads the
                application identifier from the app module's Gradle script; only this one folder is
                ever read. Every build of the latest source copies a bounded, checksummed snapshot
                of it into the container, leaving out Git metadata,
                <span class="font-mono">node_modules</span>, Gradle and build output, every
                <span class="font-mono">.env</span> file, keys, and certificates. Synchronizing here
                refreshes that snapshot without building.
            </p>

            <KeyValue
                v-if="workspace"
                :items="[
                    { label: 'Project', value: workspace.name },
                    { label: 'Folder', value: workspace.localPath, mono: true },
                    {
                        label: 'App identifier',
                        value: workspace.applicationId ?? 'From the Gradle script',
                        mono: workspace.applicationId !== null,
                        tone: workspace.applicationId ? 'default' : 'warn',
                    },
                    { label: 'Module', value: 'android/app', mono: true },
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
                hint="A Capacitor project with its Android platform added. Drop the folder here or paste its absolute path."
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
                host project and the container workspace are not modified.
            </p>
        </ConfirmDialog>
    </StepPanel>
</template>
