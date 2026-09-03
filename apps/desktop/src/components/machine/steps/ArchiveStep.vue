<script setup lang="ts">
import { FolderOpen, Package, ScrollText } from '@lucide/vue';
import { computed, onMounted, ref, watch } from 'vue';

import { formatBytes, percent, shortHash } from '../../../lib/format';
import type { JourneyStep } from '../../../model/steps';
import { archivePhaseLabel } from '../../../model/phases';
import { useEnvSetsStore } from '../../../stores/envs';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import ConfirmDialog from '../../dialogs/ConfirmDialog.vue';
import CopyButton from '../../ui/CopyButton.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import KeyValue from '../../ui/KeyValue.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import Spinner from '../../ui/Spinner.vue';
import Select from '../../ui/Select.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const ui = useUi();

const view = computed(() => session.view!);
const workspace = computed(() => view.value.appleWorkspace);
const signing = computed(() => view.value.signing);
const archive = computed(() => view.value.archive);
const busy = computed(() => session.operation !== null);
const archiving = computed(() => step.status === 'running');
const progress = computed(() => session.archive);
const lastLine = computed(() => session.archiveLog.at(-1)?.text ?? null);
const clearOpen = ref(false);
const blocked = computed(() => workspace.value?.lastNativeLockUpdated ?? false);

// The env is chosen per archive: the web assets are rebuilt inside the guest with it before
// archiving, so staging and production archives come from one synced snapshot. Defaults to
// the set attached to the machine, which is what the test build used.
const envs = useEnvSetsStore();
const attachedEnvSet = computed(() => view.value.envSet);
const envSetId = ref(attachedEnvSet.value?.id ?? '');
watch(attachedEnvSet, (set, previous) => {
    if (envSetId.value === (previous?.id ?? '')) {
        envSetId.value = set?.id ?? '';
    }
});
onMounted(() => void envs.load());
const envOptions = computed(() => [
    { value: '', label: 'No env set' },
    ...envs.sets.value.map((set) => ({
        value: set.id,
        label:
            set.id === attachedEnvSet.value?.id ? `env ${set.name} · attached` : `env ${set.name}`,
    })),
]);
const chosenEnvName = computed(() => envs.setById(envSetId.value)?.name ?? null);

const recipe = computed(() => [
    { label: 'Scheme', value: workspace.value?.scheme ?? 'App' },
    { label: 'Configuration', value: 'Release' },
    { label: 'Export', value: 'App Store Connect · manual signing · app target only' },
    { label: 'Bundle identifier', value: workspace.value?.bundleIdentifier, mono: true },
    {
        label: 'Team',
        value: signing.value?.developmentTeam ?? workspace.value?.developmentTeam,
        mono: true,
    },
    { label: 'Profile', value: signing.value?.profiles[0]?.uuid ?? null, mono: true },
    {
        label: 'Environment',
        value: chosenEnvName.value
            ? `${chosenEnvName.value} · web assets rebuilt with it before archiving`
            : 'None · web assets as the test build left them',
    },
]);

const artifacts = computed(() =>
    archive.value
        ? [
              { label: 'IPA', file: archive.value.ipa },
              { label: 'Xcode archive (zip)', file: archive.value.archive },
          ]
        : [],
);

async function clear(): Promise<void> {
    clearOpen.value = false;
    await machines.clearArchive(session.id);
}
</script>

<template>
    <StepPanel :step="step">
        <template #action>
            <span v-if="envs.sets.value.length" class="w-56 max-w-full">
                <Select
                    v-model="envSetId"
                    :options="envOptions"
                    size="sm"
                    placeholder="No env set"
                    :disabled="busy"
                />
            </span>
            <Button
                size="sm"
                :disabled="busy || blocked || step.status === 'pending'"
                @click="machines.signedArchive(session.id, envSetId || null)"
            >
                <Spinner
                    v-if="session.operation === 'archive'"
                    tone="text-white dark:text-zinc-950"
                />
                <Package v-else class="h-3.5 w-3.5" />
                {{
                    archive ? 'Build the signed archive again' : 'Build the signed archive and IPA'
                }}
            </Button>
            <Button
                v-if="archive"
                variant="outline"
                size="sm"
                @click="machines.revealArchive(session.id)"
            >
                <FolderOpen class="h-3.5 w-3.5" />
                Reveal folder
            </Button>
            <Button
                v-if="session.archiveLog.length"
                variant="ghost"
                size="sm"
                @click="ui.openLog(session.id, 'archive')"
            >
                <ScrollText class="h-3.5 w-3.5" />
                Show log
            </Button>
            <Button
                v-if="archive || view.archiveError"
                variant="ghost"
                size="sm"
                title="Deletes the IPA and archive on this host"
                :disabled="busy"
                @click="clearOpen = true"
            >
                <Spinner v-if="session.operation === 'clear-archive'" />
                Clear artifacts
            </Button>
        </template>

        <template v-if="archiving || (view.archiveError && !archiving) || blocked" #status>
            <ProgressRow
                stoppable
                :stopping="session.cancelling"
                @stop="machines.cancelOperation(session.id)"
                v-if="archiving"
                :label="progress ? archivePhaseLabel[progress.phase] : 'Preparing'"
                :detail="progress?.detail"
                :elapsed-seconds="progress?.elapsedSeconds ?? null"
                :value="progress ? percent(progress.completedBytes, progress.totalBytes) : null"
                :last-line="lastLine"
            />
            <FailureBlock
                v-if="view.archiveError && !archiving"
                title="The last signed build failed"
                cause="The diagnostic is kept until the artifacts are cleared. Fix the cause, then build the signed archive again."
                :diagnostic="view.archiveError"
            >
                <template v-if="session.archiveLog.length" #actions>
                    <Button variant="outline" size="sm" @click="ui.openLog(session.id, 'archive')">
                        Show log
                    </Button>
                </template>
            </FailureBlock>
            <Callout v-if="blocked" tone="warn" title="Blocked until the lockfile is reviewed">
                The guest refreshed Podfile.lock during the test build. Commit the updated lock on
                the host, synchronize again, and rerun the test build; signed builds fail closed on
                drift.
            </Callout>
        </template>

        <template v-if="archive" #result>
            <ul class="divide-y divide-zinc-200 dark:divide-zinc-800">
                <li
                    v-for="artifact in artifacts"
                    :key="artifact.label"
                    class="flex items-center gap-3 py-2 first:pt-0 last:pb-0"
                >
                    <Package class="h-3.5 w-3.5 shrink-0 text-zinc-500 dark:text-zinc-400" />
                    <span
                        class="w-32 shrink-0 text-xs font-medium text-zinc-700 dark:text-zinc-200"
                    >
                        {{ artifact.label }}
                    </span>
                    <span
                        class="min-w-0 flex-1 truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400"
                        v-tip="`${artifact.file.path}\nsha256 ${artifact.file.sha256}`"
                    >
                        {{ artifact.file.path }}
                    </span>
                    <span
                        class="shrink-0 font-mono text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
                    >
                        {{ formatBytes(artifact.file.bytes) }}
                    </span>
                    <CopyButton :text="artifact.file.path" label="Copy path" />
                </li>
            </ul>
            <p class="mt-2 font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                {{ archive.marketingVersion }} ({{ archive.buildNumber }}) ·
                {{ archive.bundleIdentifier }} · profile
                {{ shortHash(archive.provisioningProfileUuid, 8) }} · env
                {{ view.archiveEnvSet ?? 'none' }} · sha256
                {{ shortHash(archive.ipa.sha256, 12) }}
            </p>
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Runs a fixed <span class="font-mono">xcodebuild archive</span> and
                <span class="font-mono">-exportArchive</span> recipe, verifies the signature,
                packages the archive, and copies both artifacts to this host with matching SHA-256
                checksums. Nothing is uploaded to Apple.
            </p>
            <KeyValue :items="recipe" :columns="3" />
        </div>

        <ConfirmDialog
            v-model:open="clearOpen"
            title="Clear retained artifacts"
            confirm-label="Delete artifacts"
            @confirm="clear"
        >
            <p>
                The retained IPA, the packaged Xcode archive, and the last failure diagnostic are
                deleted from this host. The guest and the signing kit are not affected.
            </p>
        </ConfirmDialog>
    </StepPanel>
</template>
