<script setup lang="ts">
// The signed release: Gradle's release bundle and APK, signed with the attached kit's upload
// key inside the container, verified there, and brought to this host with agreed checksums.
import { FolderOpen, Package, ScrollText } from '@lucide/vue';
import { computed, onMounted, ref, watch } from 'vue';

import { formatBytes, percent } from '../../../lib/format';
import { requestedVersion } from '../../../model/build-flow';
import type { JourneyStep } from '../../../model/steps';
import { androidReleasePhaseLabel } from '../../../model/phases';
import { androidOutputLabel, androidOutputOptions } from '../../../model/android-outputs';
import { useBuildFlowStore } from '../../../stores/build-flow';
import type { AndroidReleaseOutputs } from '../../../types/backend';
import { useEnvSetsStore } from '../../../stores/envs';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import VersionFields from '../VersionFields.vue';
import Button from '../../ui/Button.vue';
import ConfirmDialog from '../../dialogs/ConfirmDialog.vue';
import CopyButton from '../../ui/CopyButton.vue';
import DisclosureSummary from '../../ui/DisclosureSummary.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import Field from '../../ui/Field.vue';
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
const draft = useBuildFlowStore().draft(session.id);
const outputs = computed<AndroidReleaseOutputs>({
    get: () => draft.androidOutputs,
    set: (value) => {
        draft.androidOutputs = value;
    },
});

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
    {
        value: '',
        label: 'Use prepared assets',
        description:
            'Keep web assets from the most recent build, including their environment values.',
    },
    ...envs.sets.value.map((set) => ({
        value: set.id,
        label: set.name,
        description:
            set.id === attachedEnvSet.value?.id
                ? 'Rebuild web assets · machine default'
                : 'Rebuild web assets with this environment',
    })),
]);
const chosenEnvName = computed(() => envs.setById(envSetId.value)?.name ?? null);

const recipe = computed(() => [
    { label: 'Build type', value: 'release' },
    {
        label: 'Application identifier',
        value: workspace.value?.applicationId ?? 'From the Gradle script',
        mono: workspace.value?.applicationId !== null,
    },
    { label: 'Upload key', value: kit.value?.androidKeystoreName ?? null, mono: true },
    { label: 'Key alias', value: kit.value?.androidKeyAlias ?? null, mono: true },
    {
        label: 'Web assets for this build',
        value: chosenEnvName.value
            ? `${chosenEnvName.value} · web assets rebuilt with it before bundling`
            : 'Prepared assets · keeps the values from the most recent build',
    },
]);

const artifacts = computed(() => [
    ...(release.value?.aab
        ? [{ label: 'App bundle (AAB)', destination: 'Google Play', file: release.value.aab }]
        : []),
    ...(release.value?.apk
        ? [{ label: 'APK', destination: 'Direct installation', file: release.value.apk }]
        : []),
]);
// This command is copied into the user's terminal; quote the host path as one shell argument.
const installCommand = computed(() =>
    release.value?.apk
        ? `adb install -r '${release.value.apk.path.replaceAll("'", "'\\''")}'`
        : null,
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
                v-tip="
                    'The environment the web assets are rebuilt with for this release; the one attached at the sync step is the default'
                "
            >
                <Select
                    v-model="envSetId"
                    :options="envOptions"
                    size="sm"
                    placeholder="Use prepared assets"
                    :disabled="busy"
                />
            </span>
            <Button
                size="sm"
                :disabled="busy || step.status === 'pending'"
                @click="
                    machines.signedRelease(
                        session.id,
                        envSetId || null,
                        outputs,
                        requestedVersion(view, draft),
                    )
                "
            >
                <Spinner
                    v-if="session.operation === 'release'"
                    tone="text-white dark:text-zinc-950"
                />
                <Package v-else class="h-3.5 w-3.5" />
                Build signed {{ androidOutputLabel[outputs] }}
            </Button>
            <Button
                v-if="release"
                variant="outline"
                size="sm"
                @click="machines.revealRelease(session.id)"
            >
                <FolderOpen class="h-3.5 w-3.5" />
                Show in folder
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
                title="Deletes the retained release files on this host"
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
                        <span
                            class="block text-[11px] font-normal text-zinc-500 dark:text-zinc-400"
                        >
                            {{ artifact.destination }}
                        </span>
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
                {{ release.applicationId }} · key {{ release.keyAlias }} · environment
                {{ android?.releaseEnvSet ?? 'prepared assets (environment not recorded)' }}
            </p>
            <div class="mt-3 flex items-start gap-2">
                <div class="min-w-0 flex-1 text-[11px] text-zinc-500 dark:text-zinc-400">
                    <span class="block font-medium">Signing certificate · SHA-256</span>
                    <code class="mt-1 block break-all">{{ release.certificateSha256 }}</code>
                </div>
                <CopyButton :text="release.certificateSha256" label="Copy fingerprint" />
            </div>
            <details v-if="installCommand" class="mt-3 text-xs text-zinc-600 dark:text-zinc-300">
                <DisclosureSummary class="font-medium">Install this APK with ADB</DisclosureSummary>
                <div class="mt-2 space-y-2">
                    <div
                        class="flex items-center gap-2 rounded-md bg-zinc-50 p-2.5 dark:bg-zinc-950"
                    >
                        <code class="min-w-0 flex-1 text-[11px] break-all">{{
                            installCommand
                        }}</code>
                        <CopyButton :text="installCommand" label="Copy command" />
                    </div>
                    <p class="text-[11px] leading-5 text-zinc-500 dark:text-zinc-400">
                        Run this in a terminal on this host with Android SDK Platform-Tools
                        installed and a phone or emulator connected. Preview has the ADB setup
                        instructions.
                    </p>
                </div>
            </details>
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Saves the selected signed release files on this host. Nothing is uploaded.
                Synchronize and run the debug build first to include project changes.
            </p>
            <Field label="Release files" hint="AAB for Google Play; APK for direct installation.">
                <Select v-model="outputs" :disabled="busy" :options="androidOutputOptions" />
            </Field>
            <KeyValue :items="recipe" :columns="3" />
            <VersionFields :session="session" :disabled="busy" />
            <details class="text-xs text-zinc-600 dark:text-zinc-300">
                <DisclosureSummary class="font-medium">
                    Google Play and direct installation
                </DisclosureSummary>
                <div class="mt-2 space-y-2 leading-5">
                    <p>
                        Upload an AAB through Play Console, or distribute an APK directly. The
                        selected files use this build's signing key; a Google Play account is not
                        needed to create or install the APK.
                    </p>
                    <p>
                        With Play App Signing, Google Play may use a different app signing key for
                        the APKs it delivers. Compare this build's certificate with the upload key
                        certificate in Play Console when checking an AAB for upload.
                    </p>
                    <p>
                        For direct APK updates, keep the same application identifier and signing
                        key. If Google Play uses a different app signing key, a locally signed APK
                        cannot update the Play-installed app. Keep a secure backup of the key used
                        for direct distribution.
                    </p>
                </div>
            </details>
        </div>

        <template #details>
            Runs Gradle's <span class="font-mono">assembleRelease</span>, plus
            <span class="font-mono">bundleRelease</span> when an AAB is selected. AAB-only builds
            also use an internal APK to check the application identifier, version and signing
            certificate. Only selected release files are retained on this host. The signing key is
            streamed into the container for this run. BuildBridge signs with
            <span class="font-mono">jarsigner</span> and <span class="font-mono">apksigner</span>,
            verifies signatures, and checks SHA-256 checksums after copying the artifacts. Use
            prepared assets keeps the environment values already compiled into the web app.
        </template>

        <ConfirmDialog
            v-model:open="clearOpen"
            title="Clear retained artifacts"
            confirm-label="Clear artifacts"
            @confirm="clear"
        >
            <p>
                The retained release files and the last failure diagnostic are deleted from this
                host. The container and the signing credentials are not affected.
            </p>
        </ConfirmDialog>
    </StepPanel>
</template>
