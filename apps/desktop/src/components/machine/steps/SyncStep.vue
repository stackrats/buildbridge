<script setup lang="ts">
import { ArrowRight, Link2, RefreshCw } from '@lucide/vue';
import { computed, onMounted, ref, watch } from 'vue';

import { formatBytes, percent } from '../../../lib/format';
import type { JourneyStep } from '../../../model/steps';
import { projectPhaseLabel } from '../../../model/phases';
import { describeEnvSetSize } from '../../../model/envs';
import { useEnvSetsStore } from '../../../stores/envs';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import Field from '../../ui/Field.vue';
import KeyValue from '../../ui/KeyValue.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import Select from '../../ui/Select.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const envs = useEnvSetsStore();
const ui = useUi();

const workspace = computed(() => session.view!.appleWorkspace);
const busy = computed(() => session.operation !== null);
const syncing = computed(() => step.status === 'running');
const progress = computed(() => session.project);
const failure = computed(() =>
    session.lastFailure?.operation === 'sync' ? session.lastFailure : null,
);

// The env set is chosen here because this is where it takes effect: it is written into the
// guest workspace with every snapshot.
const attachedEnvSet = computed(() => session.view!.envSet);
const selectedEnvSet = ref(attachedEnvSet.value?.id ?? '');
watch(attachedEnvSet, (set) => (selectedEnvSet.value = set?.id ?? ''));
const envChanged = computed(() => selectedEnvSet.value !== (attachedEnvSet.value?.id ?? ''));
const envOptions = computed(() => [
    { value: '', label: 'Project configuration only' },
    ...envs.sets.value.map((set) => ({
        value: set.id,
        label: set.name,
        description: describeEnvSetSize(set),
    })),
]);
const attaching = computed(() => session.operation === 'attach-env');
const detaching = computed(() => selectedEnvSet.value === '' && attachedEnvSet.value !== null);

onMounted(() => void envs.load());

async function attachEnv(): Promise<void> {
    await machines.attachEnvSet(session.id, selectedEnvSet.value || null);
}
</script>

<template>
    <StepPanel :step="step">
        <template #action>
            <Button
                size="sm"
                :disabled="busy || envChanged || step.status === 'pending'"
                @click="machines.sync(session.id)"
            >
                <Spinner v-if="session.operation === 'sync'" tone="text-white dark:text-zinc-950" />
                <RefreshCw v-else class="h-3.5 w-3.5" />
                {{ workspace?.lastSnapshotSha256 ? 'Synchronize again' : 'Synchronize source' }}
            </Button>
            <Button
                v-if="session.buildLog.length"
                variant="ghost"
                size="sm"
                @click="ui.openLog(session.id, 'build')"
            >
                Show log
            </Button>
        </template>

        <template v-if="syncing || failure" #status>
            <ProgressRow
                stoppable
                :stopping="session.cancelling"
                @stop="machines.cancelOperation(session.id)"
                v-if="syncing"
                :label="progress ? projectPhaseLabel[progress.phase] : 'Preparing'"
                :detail="progress?.detail"
                :elapsed-seconds="progress?.elapsedSeconds ?? null"
                :value="progress ? percent(progress.completedBytes, progress.totalBytes) : null"
                :completed-bytes="progress?.completedBytes ?? null"
                :total-bytes="progress?.totalBytes ?? null"
            />
            <FailureBlock
                v-else-if="failure"
                title="The last synchronization failed"
                :diagnostic="failure.message"
            >
                <template #actions>
                    <Button variant="outline" size="sm" @click="ui.openLog(session.id, 'build')">
                        Show log
                    </Button>
                </template>
            </FailureBlock>
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                A bounded, checksummed snapshot of the approved folder is streamed into the guest
                and installed atomically. Git metadata, <span class="font-mono">node_modules</span>,
                build output, every <span class="font-mono">.env</span> file, keys, certificates,
                and profiles are excluded. Synchronize again after every source change; it resets
                the test-build state.
            </p>

            <KeyValue
                v-if="workspace?.lastSnapshotSha256"
                :items="[
                    { label: 'Files', value: workspace.lastSyncFileCount },
                    { label: 'Source size', value: formatBytes(workspace.lastSyncBytes) },
                    { label: 'Snapshot SHA-256', value: workspace.lastSnapshotSha256, mono: true },
                    {
                        label: 'Source',
                        value:
                            workspace.lastSource?.kind === 'git'
                                ? `${workspace.lastSource.gitRef} at ${workspace.lastSource.commit?.slice(0, 12)} (remote build)`
                                : 'Approved folder as it was',
                    },
                ]"
                :columns="3"
            />

            <Callout
                v-if="envChanged"
                tone="warn"
                title="Save the environment choice before synchronizing"
            >
                The selected environment has not been saved as this machine's default yet.
            </Callout>

            <Field
                label="Default environment for the next sync"
                :hint="
                    attachedEnvSet
                        ? `${attachedEnvSet.name} is written into the guest as .env.production.local at every sync and exported to the build shell. Synchronize again after changing it.`
                        : 'Optional. Values the web build needs that are not in the repository; stored once on this host under Environments, attached per machine.'
                "
            >
                <Select
                    v-model="selectedEnvSet"
                    :options="envOptions"
                    placeholder="Project configuration only"
                    :disabled="busy"
                />
                <template #action>
                    <Button
                        variant="outline"
                        title="Applies from the next sync"
                        :disabled="busy || !envChanged"
                        @click="attachEnv"
                    >
                        <Spinner v-if="attaching" />
                        <Link2 v-else class="h-3.5 w-3.5" />
                        {{ detaching ? 'Clear default' : 'Save default' }}
                    </Button>
                </template>
            </Field>
            <p
                v-if="workspace?.lastSnapshotSha256"
                class="text-xs leading-5 text-zinc-500 dark:text-zinc-400"
            >
                This default applies to the next sync. Prepared source and web assets keep their
                existing values until you synchronize and build again, or rebuild a release with a
                selected environment.
            </p>
            <Button variant="ghost" size="sm" @click="ui.navigate({ kind: 'envs' })">
                Manage environments
                <ArrowRight class="h-3 w-3" />
            </Button>
        </div>
    </StepPanel>
</template>
