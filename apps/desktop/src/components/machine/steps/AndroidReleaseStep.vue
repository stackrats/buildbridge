<script setup lang="ts">
// The signed release: Gradle's release bundle and APK, signed with the attached kit's upload
// key inside the container, verified there, and brought to this host with agreed checksums.
import { FolderOpen, Package, ScrollText } from '@lucide/vue';
import { computed, onMounted, ref, watch } from 'vue';

import { formatBytes, percent, shortHash } from '../../../lib/format';
import type { JourneyStep } from '../../../model/steps';
import { androidReleasePhaseLabel } from '../../../model/phases';
import { useEnvSetsStore } from '../../../stores/envs';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import Button from '../../ui/Button.vue';
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
const android = computed(() => view.value.android);
const workspace = computed(() => android.value?.workspace ?? null);
const kit = computed(() => view.value.signingKit);
const release = computed(() => android.value?.release ?? null);
const releaseError = computed(() => android.value?.releaseError ?? null);
const busy = computed(() => session.operation !== null);
const releasing = computed(() => step.status === 'running');
const progress = computed(() => session.androidRelease);
const lastLine = computed(() => session.archiveLog.at(-1)?.text ?? null);
const clearOpen = ref(false);

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
    { value: '', label: 'No environment' },
    ...envs.sets.value.map((set) => ({
        value: set.id,
        label: set.id === attachedEnvSet.value?.id ? `${set.name} · attached` : `${set.name}`,
    })),
]);
const chosenEnvName = computed(() => envs.setById(envSetId.value)?.name ?? null);

const recipe = computed(() => [
    { label: 'Build type', value: 'release' },
    { label: 'Outputs', value: 'App bundle for Google Play · APK for direct install' },
    {
        label: 'Application identifier',
        value: workspace.value?.applicationId ?? 'From the Gradle script',
        mono: workspace.value?.applicationId !== null,
    },
    { label: 'Upload key', value: kit.value?.androidKeystoreName ?? null, mono: true },
    { label: 'Key alias', value: kit.value?.androidKeyAlias ?? null, mono: true },
    {
        label: 'Environment',
        value: chosenEnvName.value
            ? `${chosenEnvName.value} · web assets rebuilt with it before bundling`
            : 'None · web assets as the debug build left them',
    },
]);

const artifacts = computed(() =>
    release.value
        ? [
              { label: 'App bundle', file: release.value.aab },
              { label: 'APK', file: release.value.apk },
          ]
        : [],
);

async function clear(): Promise<void> {
    clearOpen.value = false;
    await machines.clearRelease(session.id);
}
</script>

<template>
    <StepPanel :step="step">
        <template #action>
            <span
                v-if="envs.sets.value.length"
                class="w-56 max-w-full"
                title="The environment the web assets are rebuilt with for this release; the one attached at the sync step is the default"
            >
                <Select
                    v-model="envSetId"
                    :options="envOptions"
                    size="sm"
                    placeholder="No environment"
                    :disabled="busy"
                />
            </span>
            <Button
                size="sm"
                :disabled="busy || step.status === 'pending'"
                @click="machines.signedRelease(session.id, envSetId || null)"
            >
                <Spinner
                    v-if="session.operation === 'release'"
                    tone="text-white dark:text-zinc-950"
                />
                <Package v-else class="h-3.5 w-3.5" />
                {{ release ? 'Build the signed release again' : 'Build the signed bundle and APK' }}
            </Button>
            <Button
                v-if="release"
                variant="outline"
                size="sm"
                @click="machines.revealRelease(session.id)"
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
                v-if="release || releaseError"
                variant="ghost"
                size="sm"
                title="Deletes the bundle and APK on this host"
                :disabled="busy"
                @click="clearOpen = true"
            >
                <Spinner v-if="session.operation === 'clear-release'" />
                Clear artifacts
            </Button>
        </template>

        <template v-if="releasing || (releaseError && !releasing)" #status>
            <ProgressRow
                stoppable
                :stopping="session.cancelling"
                @stop="machines.cancelOperation(session.id)"
                v-if="releasing"
                :label="progress ? androidReleasePhaseLabel[progress.phase] : 'Preparing'"
                :detail="progress?.detail"
                :elapsed-seconds="progress?.elapsedSeconds ?? null"
                :value="progress ? percent(progress.completedBytes, progress.totalBytes) : null"
                :last-line="lastLine"
            />
            <FailureBlock
                v-if="releaseError && !releasing"
                title="The last signed release failed"
                cause="The diagnostic is kept until the artifacts are cleared. Fix the cause, then build the signed release again."
                :diagnostic="releaseError"
            >
                <template v-if="session.archiveLog.length" #actions>
                    <Button variant="outline" size="sm" @click="ui.openLog(session.id, 'archive')">
                        Show log
                    </Button>
                </template>
            </FailureBlock>
        </template>

        <template v-if="release" #result>
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
                {{ release.versionName }} ({{ release.versionCode }}) ·
                {{ release.applicationId }} · key {{ release.keyAlias }} · certificate
                {{ shortHash(release.certificateSha256, 12) }} · env
                {{ android?.releaseEnvSet ?? 'none' }}
            </p>
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Runs Gradle's <span class="font-mono">bundleRelease</span> and
                <span class="font-mono">assembleRelease</span>, signs the bundle with
                <span class="font-mono">jarsigner</span> and the aligned APK with
                <span class="font-mono">apksigner</span> using the upload key in the attached
                credentials, streamed into the container for the run, verifies both signatures, and
                copies both artifacts to this host with matching SHA-256 checksums. Nothing is
                uploaded to Google Play; the bundle is what its console takes.
            </p>
            <KeyValue :items="recipe" :columns="3" />
        </div>

        <ConfirmDialog
            v-model:open="clearOpen"
            title="Clear retained artifacts"
            confirm-label="Clear artifacts"
            @confirm="clear"
        >
            <p>
                The retained app bundle, the APK, and the last failure diagnostic are deleted from
                this host. The container and the signing credentials are not affected.
            </p>
        </ConfirmDialog>
    </StepPanel>
</template>
