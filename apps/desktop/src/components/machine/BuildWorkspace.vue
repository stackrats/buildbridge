<script setup lang="ts">
import { ArrowRight, FolderOpen, Hammer, Package } from '@lucide/vue';
import { computed, watch } from 'vue';

import { formatBytes, shortHash } from '../../lib/format';
import { buildPrerequisite, projectWorkspace, type BuildRequest } from '../../model/build-flow';
import { androidOutputOptions } from '../../model/android-outputs';
import { isAndroid } from '../../model/providers';
import {
    unsignedBuildTargetOptions,
    type JourneyStep,
    type JourneyStepId,
} from '../../model/steps';
import { useBuildFlowStore } from '../../stores/build-flow';
import { useEnvSetsStore } from '../../stores/envs';
import { useMachinesStore, type MachineSession } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import CopyButton from '../ui/CopyButton.vue';
import Field from '../ui/Field.vue';
import Select from '../ui/Select.vue';
import AndroidHttpOption from './AndroidHttpOption.vue';
import StepDetail from './StepDetail.vue';
import VersionFields from './VersionFields.vue';

const { session, steps } = defineProps<{ session: MachineSession; steps: JourneyStep[] }>();
const emit = defineEmits<{ step: [id: JourneyStepId]; preview: [] }>();
const machines = useMachinesStore();
const flows = useBuildFlowStore();
const envs = useEnvSetsStore();
const ui = useUi();
const view = computed(() => session.view!);
const android = computed(() => isAndroid(view.value.profile.provider));
const project = computed(() => projectWorkspace(view.value));
const blocked = computed(() => buildPrerequisite(view.value));
const busy = computed(
    () =>
        session.operation !== null || view.value.busyOperation !== null || flows.active(session.id),
);
const approve = computed(() => steps.find((step) => step.id === 'approve'));
const draft = computed(() => flows.draft(session.id));
const outcome = computed<BuildRequest['outcome']>({
    get: () => draft.value.outcome,
    set: (value) => {
        draft.value.outcome = value;
    },
});
const source = computed<BuildRequest['source']>({
    get: () => draft.value.source,
    set: (value) => {
        draft.value.source = value;
    },
});
const target = computed<BuildRequest['target']>({
    get: () => draft.value.target,
    set: (value) => {
        draft.value.target = value;
    },
});
const androidOutputs = computed<BuildRequest['androidOutputs']>({
    get: () => draft.value.androidOutputs,
    set: (value) => {
        draft.value.androidOutputs = value;
    },
});
const envSetId = computed({
    get: () => draft.value.envSetId ?? '',
    set: (value: string) => {
        draft.value.envSetId = value || null;
    },
});
watch(
    () => view.value.envSet?.id,
    (next, previous) => {
        if (source.value === 'latest' && envSetId.value === (previous ?? ''))
            envSetId.value = next ?? '';
    },
);
watch(source, (value) => {
    envSetId.value = value === 'latest' ? (view.value.envSet?.id ?? '') : '';
});
const environmentOptions = computed(() => [
    { value: '', label: source.value === 'latest' ? 'No environment' : 'Use prepared assets' },
    ...envs.sets.value.map((set) => ({ value: set.id, label: set.name })),
]);
const environmentAllowed = computed(() => source.value === 'latest' || outcome.value === 'release');
const selectedEnvironment = computed(() =>
    environmentAllowed.value ? envSetId.value || null : null,
);
const actionLabel = computed(() =>
    source.value === 'latest' ? 'Build latest source' : 'Rebuild saved snapshot',
);
const snapshot = computed(() => project.value?.lastSnapshotSha256 ?? null);
const archive = computed(() => view.value.archive);
const release = computed(() => view.value.android?.release);
const debugApk = computed(() => view.value.android?.workspace?.lastBuild?.apk ?? null);
const releaseSummary = computed(() => {
    const version = android.value
        ? release.value
            ? `${release.value.versionName} (${release.value.versionCode})`
            : null
        : archive.value
          ? `${archive.value.marketingVersion} (${archive.value.buildNumber})`
          : null;
    if (!version) return null;
    const environment = android.value
        ? view.value.android?.releaseEnvSet
        : view.value.archiveEnvSet;
    return `Retained release ${version} · ${environment ? `environment ${environment}` : 'prepared assets; environment not recorded'}`;
});
const artifacts = computed(() =>
    android.value
        ? [
              ...(debugApk.value
                  ? [{ label: 'Debug APK', file: debugApk.value, debug: true }]
                  : []),
              ...(release.value?.aab
                  ? [{ label: 'Release bundle (AAB)', file: release.value.aab, debug: false }]
                  : []),
              ...(release.value?.apk
                  ? [{ label: 'Release APK', file: release.value.apk, debug: false }]
                  : []),
          ]
        : archive.value
          ? [
                { label: 'App Store IPA', file: archive.value.ipa, debug: false },
                { label: 'Xcode archive', file: archive.value.archive, debug: false },
            ]
          : [],
);

function build(): void {
    if (busy.value || blocked.value) return;
    void flows.start(
        session.id,
        {
            source: source.value,
            outcome: outcome.value,
            target: target.value,
            envSetId: selectedEnvironment.value,
            androidOutputs: androidOutputs.value,
            androidAllowHttp: draft.value.androidAllowHttp,
            version: outcome.value === 'release' ? (draft.value.version ?? null) : null,
        },
        selectedEnvironment.value
            ? (envs.setById(selectedEnvironment.value)?.name ?? 'Selected environment')
            : null,
    );
}

function reveal(debug: boolean): void {
    if (debug) void machines.revealDebugApk(session.id);
    else if (android.value) void machines.revealRelease(session.id);
    else void machines.revealArchive(session.id);
}
</script>

<template>
    <div class="space-y-5">
        <header class="flex flex-wrap items-start justify-between gap-3">
            <div class="min-w-0 flex-1">
                <h2 class="text-base font-semibold">
                    {{ project ? 'Build your app' : 'Choose your project' }}
                </h2>
                <p class="mt-1 text-sm leading-6 text-zinc-500 dark:text-zinc-400">
                    {{
                        project
                            ? 'Choose your output and source. BuildBridge guides the build and pauses for any decisions.'
                            : 'Your machine is reusable. Approve a local Capacitor project to build it here.'
                    }}
                </p>
            </div>
            <Button
                v-if="project"
                :disabled="busy || blocked !== null || (source === 'snapshot' && !snapshot)"
                @click="build"
                ><Hammer class="h-4 w-4" />{{ actionLabel }}</Button
            >
        </header>

        <Callout v-if="blocked && blocked.step !== 'approve'" tone="warn" :title="blocked.message">
            <Button class="mt-2" size="sm" variant="outline" @click="emit('step', blocked.step)">{{
                blocked.action
            }}</Button>
        </Callout>
        <div
            v-if="!project && approve"
            class="rounded-lg border border-zinc-200 bg-white p-4 dark:border-zinc-800 dark:bg-zinc-900"
        >
            <StepDetail :session="session" :step="approve" />
        </div>

        <section
            v-if="project"
            class="rounded-lg border border-zinc-200 bg-white p-5 dark:border-zinc-800 dark:bg-zinc-900"
        >
            <div
                class="mb-5 flex flex-wrap items-start justify-between gap-3 border-b border-zinc-200 pb-4 dark:border-zinc-800"
            >
                <div class="min-w-0">
                    <h3 class="text-sm font-semibold">{{ project.name }}</h3>
                    <p class="mt-1 font-mono text-xs break-all text-zinc-500 dark:text-zinc-400">
                        {{ project.localPath }}
                    </p>
                </div>
                <Button
                    size="sm"
                    variant="outline"
                    :disabled="busy"
                    @click="emit('step', 'approve')"
                    >Change project</Button
                >
            </div>
            <div class="grid gap-4 sm:grid-cols-2">
                <Field
                    label="Output"
                    :hint="
                        outcome === 'test'
                            ? android
                                ? 'A debug APK you can install on a phone or emulator. No upload key required.'
                                : 'Check that the app compiles. Signing is only needed for a release or an iPhone run.'
                            : android
                              ? 'Choose an app bundle for Google Play, an APK for direct installation, or both.'
                              : 'A signed App Store IPA and Xcode archive. Nothing is uploaded to Apple.'
                    "
                >
                    <Select
                        v-model="outcome"
                        :disabled="busy"
                        :options="[
                            { value: 'test', label: 'Build for testing' },
                            { value: 'release', label: 'Create a release' },
                        ]"
                    />
                </Field>
                <Field
                    v-if="android && outcome === 'release'"
                    label="Release files"
                    hint="AAB for Google Play; APK for direct installation."
                >
                    <Select
                        v-model="androidOutputs"
                        :disabled="busy"
                        :options="androidOutputOptions"
                    />
                </Field>
                <Field
                    label="Source"
                    :hint="
                        source === 'latest'
                            ? 'Copies the current files from the approved folder, including local edits. Replaces the previous snapshot.'
                            : 'Reuses the last copied source. New local edits are not included.'
                    "
                >
                    <Select
                        v-model="source"
                        :disabled="busy"
                        :options="[
                            { value: 'latest', label: 'Latest local source' },
                            { value: 'snapshot', label: 'Saved snapshot', disabled: !snapshot },
                        ]"
                    />
                </Field>
                <Field
                    v-if="environmentAllowed"
                    label="Environment"
                    :hint="
                        source === 'latest'
                            ? 'Applied before building. This also becomes the machine’s default for its next source copy.'
                            : selectedEnvironment
                              ? 'Rebuilds web assets with the current saved values in this environment.'
                              : 'Retains existing web assets. Their environment may differ from the machine default.'
                    "
                >
                    <Select v-model="envSetId" :options="environmentOptions" :disabled="busy" />
                </Field>
                <div v-else class="text-sm leading-6 text-zinc-500 dark:text-zinc-400">
                    The saved snapshot keeps its prepared environment. Choose Latest local source to
                    change it for a test build.
                </div>
                <Field
                    v-if="!android && outcome === 'test'"
                    label="Compile target"
                    :hint="
                        target === 'simulator'
                            ? 'Compiles for Simulator; this does not open or run a simulator. A missing runtime requires a large download.'
                            : 'Verifies compilation without installing on a phone. Installs the iOS platform if Xcode requires it.'
                    "
                >
                    <Select
                        v-model="target"
                        :disabled="busy"
                        :options="
                            unsignedBuildTargetOptions(view.guest.diagnostics.iosSimulatorRuntime)
                        "
                    />
                </Field>
            </div>
            <AndroidHttpOption
                v-if="android && outcome === 'test'"
                class="mt-4"
                :session="session"
                :disabled="busy"
            />
            <VersionFields
                v-if="outcome === 'release'"
                class="mt-4"
                :session="session"
                :disabled="busy"
            />
            <div
                v-if="outcome === 'release'"
                class="mt-4 flex flex-wrap items-center justify-between gap-2 rounded-md bg-zinc-50 p-3 dark:bg-zinc-950"
            >
                <p class="text-sm">
                    <span class="text-zinc-500 dark:text-zinc-400">Signing credentials:</span>
                    {{ view.signingKit?.name ?? 'Choose before exporting' }}
                </p>
                <Button
                    variant="ghost"
                    size="sm"
                    :disabled="busy"
                    @click="emit('step', 'signing-kit')"
                    >{{
                        view.signingKit
                            ? 'Review signing credentials'
                            : 'Choose signing credentials'
                    }}<ArrowRight class="h-3.5 w-3.5"
                /></Button>
            </div>
            <p
                v-if="outcome === 'release'"
                class="mt-3 text-xs leading-5 text-zinc-500 dark:text-zinc-400"
            >
                The test build runs first. BuildBridge pauses if signing setup or a lockfile
                decision is needed.
            </p>
            <div class="mt-5 flex flex-wrap items-center gap-2">
                <Button variant="outline" @click="emit('preview')"
                    >Preview options<ArrowRight class="h-4 w-4"
                /></Button>
                <Button variant="ghost" size="sm" @click="ui.navigate({ kind: 'envs' })"
                    >Manage environments</Button
                >
            </div>
        </section>

        <section v-if="project" class="space-y-3">
            <div class="flex flex-wrap items-center justify-between gap-2">
                <h3 class="text-sm font-semibold">
                    {{ artifacts.length ? 'Available artifacts' : 'Build results' }}
                </h3>
                <div class="flex flex-wrap items-center gap-2">
                    <Button
                        v-if="release || archive"
                        size="sm"
                        variant="outline"
                        @click="emit('step', 'publish')"
                    >
                        Publish release<ArrowRight class="h-3.5 w-3.5" />
                    </Button>
                    <Button
                        size="sm"
                        variant="ghost"
                        @click="emit('step', android ? 'release' : 'archive')"
                        >Release details</Button
                    >
                </div>
            </div>
            <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                {{
                    snapshot
                        ? `Current saved source: ${shortHash(snapshot)}. Retained releases may come from an earlier snapshot. New local edits are included only after copying source again.`
                        : 'No source has been copied yet. Your first build creates a snapshot.'
                }}
            </p>
            <p v-if="releaseSummary" class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                {{ releaseSummary }}
            </p>
            <div
                v-if="artifacts.length"
                class="divide-y divide-zinc-200 rounded-lg border border-zinc-200 bg-white px-4 dark:divide-zinc-800 dark:border-zinc-800 dark:bg-zinc-900"
            >
                <div
                    v-for="artifact in artifacts"
                    :key="artifact.label"
                    class="flex flex-wrap items-center gap-3 py-3"
                >
                    <Package class="h-4 w-4 shrink-0 text-zinc-500 dark:text-zinc-400" />
                    <div class="min-w-0 flex-1">
                        <p class="text-sm font-medium">{{ artifact.label }}</p>
                        <p class="text-xs text-zinc-500 dark:text-zinc-400">
                            {{ formatBytes(artifact.file.bytes) }} · verified
                            <template v-if="artifact.debug">
                                ·
                                {{
                                    view.android?.workspace?.lastBuild?.allowHttp
                                        ? 'HTTP APIs allowed for testing'
                                        : 'Project HTTP settings'
                                }}
                            </template>
                        </p>
                    </div>
                    <Button size="sm" variant="outline" @click="reveal(artifact.debug)"
                        ><FolderOpen class="h-3.5 w-3.5" />Show in folder</Button
                    >
                    <CopyButton :text="artifact.file.path" label="Copy path" />
                </div>
            </div>
            <Callout v-else-if="project.lastBuildSucceeded" tone="ok" title="Test build passed"
                >{{
                    android
                        ? 'Open the test-build details for the build result.'
                        : 'The saved project compiled successfully. You can prepare an iPhone preview or create a release next.'
                }}<Button
                    class="mt-2"
                    size="sm"
                    variant="outline"
                    @click="emit('step', 'test-build')"
                    >Test-build details</Button
                ></Callout
            >
            <p v-else class="text-sm text-zinc-500 dark:text-zinc-400">
                Your build result will appear here.
            </p>
        </section>
    </div>
</template>
