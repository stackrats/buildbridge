<script setup lang="ts">
import {
    FolderOpen,
    Package,
    ScrollText,
    FileCheck,
    Variable,
    ArrowRight,
    KeyRound,
} from '@lucide/vue';
import { computed, onMounted, ref, watch } from 'vue';

import { formatBytes, percent, shortHash } from '../../../lib/format';
import { requestedVersion, type BuildRequest } from '../../../model/build-flow';
import type { JourneyStep } from '../../../model/steps';
import { archivePhaseLabel } from '../../../model/phases';
import { describeSnapshot } from '../../../model/snapshot';
import { useBuildFlowStore } from '../../../stores/build-flow';
import { useEnvSetsStore } from '../../../stores/envs';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import VersionFields from '../VersionFields.vue';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import ConfirmDialog from '../../dialogs/ConfirmDialog.vue';
import CopyButton from '../../ui/CopyButton.vue';
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
const workspace = computed(() => view.value.appleWorkspace);
const signing = computed(() => view.value.signing);
const archive = computed(() => view.value.archive);
const flows = useBuildFlowStore();
const busy = computed(() => session.operation !== null || flows.active(session.id));
const archiving = computed(() => step.status === 'running');
const progress = computed(() => session.archive);
const lastLine = computed(() => session.archiveLog.at(-1)?.text ?? null);
const clearOpen = ref(false);
const snapshotSummary = computed(() => describeSnapshot(view.value));
const blocked = computed(() => workspace.value?.lastNativeLockUpdated ?? false);
// The source and version choices live in the machine's build draft, shared with the other
// build steps.
const draft = flows.draft(session.id);
const source = computed<BuildRequest['source']>({
    get: () => draft.source,
    set: (value) => {
        draft.source = value;
    },
});

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
    {
        value: '',
        label: 'Use prepared assets',
        description:
            'Keep web assets from the most recent build, including their environment values.',
    },
    ...envs.sets.value.map((set) => ({
        value: set.id,
        label: set.name,
        description: 'Rebuild web assets with this environment',
    })),
]);
const chosenEnvName = computed(() => envs.setById(envSetId.value)?.name ?? null);

// The latest source goes through the guide, copy, test build, then archive, pausing for any
// decision on the way; the saved snapshot is archived as it is.
function build(): void {
    if (busy.value) return;
    if (source.value === 'latest') {
        void flows.start(
            session.id,
            {
                source: 'latest',
                outcome: 'release',
                target: 'device_sdk',
                envSetId: envSetId.value || null,
                androidOutputs: draft.androidOutputs,
                androidAllowHttp: draft.androidAllowHttp,
                version: draft.version ?? null,
            },
            chosenEnvName.value,
        );
        return;
    }
    void machines.signedArchive(
        session.id,
        envSetId.value || null,
        requestedVersion(view.value, draft),
    );
}

const recipe = computed(() => [
    { label: 'Scheme', value: workspace.value?.scheme ?? 'App' },
    { label: 'Configuration', value: 'Release', copyable: false },
    { label: 'Export', value: 'App Store Connect · manual signing · app target only' },
    { label: 'Bundle identifier', value: workspace.value?.bundleIdentifier, mono: true },
    {
        label: 'Team',
        value: signing.value?.developmentTeam ?? workspace.value?.developmentTeam,
        mono: true,
    },
    { label: 'Profile', value: signing.value?.profiles[0]?.uuid ?? null, mono: true },
    {
        label: 'Web assets for this build',
        value: chosenEnvName.value
            ? `${chosenEnvName.value} · web assets rebuilt with it before archiving`
            : 'Prepared assets · keeps the values from the most recent build',
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
            <Button
                size="sm"
                :disabled="
                    busy ||
                    blocked ||
                    step.status === 'pending' ||
                    (source === 'snapshot' && !workspace?.lastSnapshotSha256)
                "
                :title="
                    source === 'latest'
                        ? 'Copies the latest local source, runs the test build, then archives and exports'
                        : 'Archives and exports the saved snapshot as it is'
                "
                @click="build"
            >
                <Spinner
                    v-if="session.operation === 'archive' || flows.active(session.id)"
                    tone="text-white dark:text-zinc-950"
                />
                <Package v-else class="h-3.5 w-3.5" />
                {{
                    source === 'latest'
                        ? 'Build and sign the latest source'
                        : archive
                          ? 'Build the signed archive again'
                          : 'Build the signed archive and IPA'
                }}
            </Button>
            <Button
                v-if="archive"
                variant="outline"
                size="sm"
                @click="machines.revealArchive(session.id)"
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

        <template
            v-if="archiving || (view.archiveError && !archiving) || blocked || session.lockAdoption"
            #status
        >
            <ProgressRow
                v-if="archiving"
                stoppable
                :stopping="session.cancelling"
                @stop="machines.cancelOperation(session.id)"
                :label="progress ? archivePhaseLabel[progress.phase] : 'Preparing'"
                :detail="progress?.detail"
                :elapsed-seconds="progress?.elapsedSeconds ?? null"
                :value="progress ? percent(progress.completedBytes, progress.totalBytes) : null"
                :last-line="lastLine"
            />
            <FailureBlock
                v-if="view.archiveError && !archiving"
                title="The last signed archive failed"
                cause="The diagnostic is kept until the artifacts are cleared. Fix the cause, then build the signed archive again."
                :diagnostic="view.archiveError"
            >
                <template v-if="session.archiveLog.length" #actions>
                    <Button variant="outline" size="sm" @click="ui.openLog(session.id, 'archive')">
                        Show log
                    </Button>
                </template>
            </FailureBlock>
            <Callout v-if="blocked" tone="warn" title="Blocked until the lockfile is adopted">
                <p>
                    The guest refreshed Podfile.lock during the test build; signed builds fail
                    closed on drift. Adopt the guest's lock into the project and the block lifts:
                    the test build already compiled with exactly that lock, so nothing needs
                    rebuilding. Then commit it in the project.
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
                        class="w-[4.5rem] shrink-0 text-right font-mono text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
                    >
                        {{ formatBytes(artifact.file.bytes) }}
                    </span>
                    <CopyButton
                        :text="artifact.file.path"
                        :what="`Copy the ${artifact.label} path`"
                        size="iconXs"
                    />
                </li>
            </ul>
            <p class="mt-2 font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                {{ archive.marketingVersion }} ({{ archive.buildNumber }}) ·
                {{ archive.bundleIdentifier }} · profile
                {{ shortHash(archive.provisioningProfileUuid, 8) }} ·
                <span class="inline-flex items-center gap-1" v-tip="'Environment'">
                    <Variable class="h-3 w-3 shrink-0" aria-hidden="true" />
                    <span class="sr-only">environment</span
                    >{{ view.archiveEnvSet ?? 'prepared assets (environment not recorded)' }}
                </span>
                · sha256 {{ shortHash(archive.ipa.sha256, 12) }}
            </p>
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Creates a signed IPA and Xcode archive and saves both on this host. Nothing is
                uploaded to Apple. The latest local source is copied and test-built first; the saved
                snapshot is built and signed as it is.
            </p>
            <div class="grid gap-4 sm:grid-cols-2">
                <Field
                    label="Source"
                    :hint="
                        source === 'latest'
                            ? 'Copies the current files from the approved folder, runs the test build, then archives and exports.'
                            : `The saved snapshot is built and signed as it is: ${snapshotSummary}. New local edits are not included.`
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
                                disabled: !workspace?.lastSnapshotSha256,
                            },
                        ]"
                    />
                </Field>
                <Field
                    v-if="envs.sets.value.length"
                    label="Environment"
                    :hint="
                        chosenEnvName
                            ? 'The web assets are rebuilt with this environment before archiving.'
                            : 'Keeps the web assets from the most recent build, including their environment values.'
                    "
                >
                    <Select
                        v-model="envSetId"
                        :options="envOptions"
                        placeholder="Use prepared assets"
                        :disabled="busy"
                    />
                </Field>
            </div>
            <KeyValue :items="recipe" :columns="3" />
            <p class="flex flex-wrap items-center gap-1.5 text-xs">
                <KeyRound
                    class="h-3.5 w-3.5 shrink-0 text-zinc-500 dark:text-zinc-400"
                    aria-hidden="true"
                />
                <span class="sr-only">Signing credentials</span>
                <span v-tip="'Signing credentials'">{{
                    view.signingKit?.name ?? 'Choose before signing'
                }}</span>
                <Button variant="ghost" size="sm" @click="ui.selectStep(session.id, 'signing-kit')"
                    >{{
                        view.signingKit
                            ? 'Review signing credentials'
                            : 'Choose signing credentials'
                    }}<ArrowRight class="h-3.5 w-3.5"
                /></Button>
            </p>
            <VersionFields :session="session" :disabled="busy" store-check />
        </div>

        <template #details>
            Runs a fixed <span class="font-mono">xcodebuild archive</span> and
            <span class="font-mono">-exportArchive</span> recipe, verifies the signature, packages
            the archive, and checks SHA-256 checksums after copying both artifacts to this host. Use
            prepared assets keeps any environment values already compiled into the web app; choosing
            an environment rebuilds those assets before archiving.
        </template>

        <ConfirmDialog
            v-model:open="clearOpen"
            title="Clear retained artifacts"
            confirm-label="Clear artifacts"
            @confirm="clear"
        >
            <p>
                The retained IPA, the packaged Xcode archive, and the last failure diagnostic are
                deleted from this host. The guest and the signing credentials are not affected.
            </p>
        </ConfirmDialog>
    </StepPanel>
</template>
