<script setup lang="ts">
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

const view = computed(() => session.view!);
const workspace = computed(() => view.value.appleWorkspace);
const busy = computed(() => session.operation !== null);
const path = ref(workspace.value?.localPath ?? '');
const removeOpen = ref(false);
const failure = computed(() =>
    session.lastFailure?.operation === 'approve' ? session.lastFailure : null,
);

const requirements = [
    'package.json and pnpm-lock.yaml',
    'capacitor.config.ts',
    'ios/App/Podfile and Podfile.lock',
    'ios/App/App.xcodeproj and App.xcworkspace',
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
                cause="The folder must hold an Ionic or Capacitor iOS project with CocoaPods; the files it needs are listed below."
                :diagnostic="failure.message"
            />
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Approval is read-only: BuildBridge validates the project shape and detects the Xcode
                team and release bundle identifier, but copies nothing until you synchronize. Only
                this one folder is ever read.
            </p>

            <KeyValue
                v-if="workspace"
                :items="[
                    { label: 'Project', value: workspace.name },
                    { label: 'Folder', value: workspace.localPath, mono: true },
                    {
                        label: 'Development team',
                        value: workspace.developmentTeam,
                        tone: workspace.developmentTeam ? 'default' : 'warn',
                    },
                    {
                        label: 'Bundle identifier',
                        value: workspace.bundleIdentifier,
                        mono: true,
                        tone: workspace.bundleIdentifier ? 'default' : 'warn',
                    },
                    { label: 'Workspace', value: workspace.iosWorkspace, mono: true },
                    { label: 'Scheme', value: workspace.scheme },
                ]"
            />

            <Field
                :label="workspace ? 'Approve a different folder' : 'Project folder on this host'"
                hint="Currently an Ionic or Capacitor iOS project with CocoaPods. Drop the folder here or paste its absolute path."
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
                host project and the guest workspace are not modified.
            </p>
        </ConfirmDialog>
    </StepPanel>
</template>
