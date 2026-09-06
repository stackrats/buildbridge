<script setup lang="ts">
// The Android counterpart of the project approval: the same one folder, read-only, checked for
// the Android platform instead of the iOS one.
import { FolderCheck } from '@lucide/vue';
import { computed, ref } from 'vue';

import type { JourneyStep } from '../../../model/steps';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import Button from '../../ui/Button.vue';
import ConfirmDialog from '../../dialogs/ConfirmDialog.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import Field from '../../ui/Field.vue';
import KeyValue from '../../ui/KeyValue.vue';
import PathField from '../../ui/PathField.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();

const workspace = computed(() => session.view!.android?.workspace ?? null);
const busy = computed(() => session.operation !== null);
const path = ref(workspace.value?.localPath ?? '');
const removeOpen = ref(false);
const failure = computed(() =>
    session.lastFailure?.operation === 'approve' ? session.lastFailure : null,
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
            <Button variant="ghost" size="sm" :disabled="busy" @click="removeOpen = true">
                <Spinner v-if="session.operation === 'clear-workspace'" />
                Remove approval
            </Button>
        </template>

        <template v-if="failure" #status>
            <FailureBlock
                title="That folder was not approved"
                cause="The folder must hold a Capacitor project with its Android platform added; the files it needs are listed below."
                :diagnostic="failure.message"
            />
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Approval is read-only: BuildBridge validates the project shape and reads the
                application identifier from the app module's Gradle script, but copies nothing until
                you synchronize. Only this one folder is ever read.
            </p>

            <KeyValue
                v-if="workspace"
                :items="[
                    { label: 'Project', value: workspace.name },
                    { label: 'Folder', value: workspace.localPath, mono: true },
                    {
                        label: 'Application identifier',
                        value: workspace.applicationId ?? 'Computed by the Gradle script',
                        mono: workspace.applicationId !== null,
                        tone: workspace.applicationId ? 'default' : 'warn',
                    },
                    { label: 'Module', value: 'android/app', mono: true },
                ]"
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
                BuildBridge forgets this folder and the sync and build state recorded for it. The
                host project and the container workspace are not modified.
            </p>
        </ConfirmDialog>
    </StepPanel>
</template>
