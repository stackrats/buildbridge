<script setup lang="ts">
import { ArrowRight, Check, FolderOpen, Hammer, Package, RefreshCw, Share2 } from '@lucide/vue';
import { computed, nextTick, onMounted, ref, useId, watch } from 'vue';

import { formatBytes, shortHash } from '../../lib/format';
import { useEnvSetsStore } from '../../stores/envs';
import { useNativeMacStore, validNativeCommit, validNativeMinimum } from '../../stores/native-mac';
import { useSettingsStore } from '../../stores/settings';
import { useUi } from '../../stores/ui';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import Checkbox from '../ui/Checkbox.vue';
import CopyButton from '../ui/CopyButton.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import Field from '../ui/Field.vue';
import Input from '../ui/Input.vue';
import KeyValue from '../ui/KeyValue.vue';
import LogView from '../ui/LogView.vue';
import PathField from '../ui/PathField.vue';
import ProgressRow from '../ui/ProgressRow.vue';
import Select from '../ui/Select.vue';
import Spinner from '../ui/Spinner.vue';
import Tabs from '../ui/Tabs.vue';
import PublishingGuide from '../machine/PublishingGuide.vue';

const native = useNativeMacStore();
const envs = useEnvSetsStore();
const ui = useUi();
// This Mac builds for its owner whether or not anyone else can reach it; sharing it is the
// part that needs a buildbridge server, so only that part follows the setting.
const remoteBuilds = useSettingsStore().remoteBuilds;
const state = native.state;
const id = useId();
const pane = ref<HTMLElement | null>(null);
const editingProject = ref(false);
const editingSigning = ref(false);
const status = computed(() => state.status);
const project = computed(() => status.value?.config.project);
const signing = computed(() => status.value?.config.signing);
const toolchain = computed(() => status.value?.toolchain);
const disabled = computed(() => native.busy.value || !status.value?.supported);
const projectFormOpen = computed(() => !project.value || editingProject.value);
const signingFormOpen = computed(() => !signing.value || editingSigning.value);
const ready = computed(() =>
    state.outcome === 'test' ? status.value?.testReady : status.value?.archiveReady,
);
const environmentMissing = computed(
    () => !!state.envSetName && !envs.state.sets.some((set) => set.name === state.envSetName),
);
const commitError = computed(() =>
    state.commit.trim() && !validNativeCommit(state.commit)
        ? 'Use the full 40- or 64-character Git commit ID, copied from your repository.'
        : null,
);
const buildBlocked = computed(() => {
    if (!status.value?.supported) return 'Native iOS builds require this app to run on a Mac.';
    if (native.busy.value) return 'Wait for the current native operation to finish.';
    if (!project.value) return 'Approve your project in setup first.';
    if (!ready.value)
        return state.outcome === 'archive'
            ? 'Review setup: the toolchain, project, and App Store signing must be ready.'
            : 'Review the project and toolchain requirements in setup.';
    if (environmentMissing.value) return 'Choose an environment that is still stored on this Mac.';
    if (!validNativeCommit(state.commit)) return 'Enter the full Git commit ID to build.';
    return null;
});
const tabs = computed(() => [
    {
        value: 'setup' as const,
        label: 'Setup',
        id: `${id}-setup-tab`,
        panelId: `${id}-setup`,
        badge: status.value?.testReady ? 'Ready' : null,
        badgeTone: 'ok' as const,
    },
    {
        value: 'build' as const,
        label: 'Build',
        id: `${id}-build-tab`,
        panelId: `${id}-build`,
        badge: state.running || status.value?.busy ? 'Running' : null,
    },
]);
const identityOptions = computed(() =>
    (status.value?.identities ?? []).map((identity) => ({
        value: identity.sha1,
        label: identity.name,
        description: `Certificate ${identity.sha1.slice(-12)}`,
    })),
);
const environmentOptions = computed(() => [
    {
        value: '',
        label: 'No environment',
        description: 'Use the settings committed with this source.',
    },
    ...envs.state.sets.map((set) => ({ value: set.name, label: set.name })),
]);
const setupIssues = computed(() =>
    (status.value?.issues ?? []).filter((issue) => !toolchain.value?.issues.includes(issue)),
);
const projectValid = computed(
    () =>
        state.projectPath.trim() &&
        validNativeMinimum(state.minXcode) &&
        validNativeMinimum(state.minIosSdk),
);
const elapsed = computed(() =>
    state.startedAt === null ? null : Math.floor((Date.now() - state.startedAt) / 1000),
);
const result = computed(() => state.result);
const launchAtLogin = computed({
    get: () => status.value?.launchAtLogin ?? false,
    set: (enabled: boolean) => {
        void native.setLaunchAtLogin(enabled);
    },
});

onMounted(() => {
    void native.initialize();
    if (!envs.state.loaded) void envs.load();
});

watch(
    () => state.tab,
    async () => {
        state.notice = null;
        await nextTick();
        pane.value?.closest('main')?.scrollTo({ top: 0 });
        document.getElementById(`${id}-${state.tab}-tab`)?.focus({ preventScroll: true });
    },
);

watch(
    () => state.running || status.value?.busy,
    async (running) => {
        if (!running || state.tab !== 'build') return;
        await nextTick();
        pane.value?.closest('main')?.scrollTo({ top: 0 });
    },
);

async function approveProject(): Promise<void> {
    if (!projectValid.value || disabled.value) return;
    if (await native.approveProject()) {
        editingProject.value = false;
        editingSigning.value = false;
    }
}

async function approveSigning(): Promise<void> {
    if (!state.identitySha1 || !state.profilePath.trim() || disabled.value) return;
    if (await native.configureSigning()) editingSigning.value = false;
}

function cancelEdit(): void {
    native.resetSetupDrafts();
    editingProject.value = false;
    editingSigning.value = false;
}
</script>

<template>
    <div ref="pane" class="mx-auto max-w-4xl space-y-4 p-5">
        <header class="flex items-start justify-between gap-4">
            <div class="min-w-0">
                <h1 class="text-lg font-bold text-zinc-900 dark:text-zinc-50">This Mac</h1>
                <p class="mt-1 max-w-2xl text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    Build iOS apps with this Mac's Xcode and existing signing identities.
                </p>
            </div>
            <div class="flex items-center gap-1.5">
                <Button
                    variant="ghost"
                    size="sm"
                    :disabled="state.refreshing"
                    @click="native.recheck"
                >
                    <Spinner v-if="state.refreshing" /><RefreshCw v-else class="h-3.5 w-3.5" />
                    Recheck
                </Button>
                <Button
                    v-if="remoteBuilds"
                    variant="outline"
                    size="sm"
                    @click="ui.navigate({ kind: 'runner' })"
                >
                    <Share2 class="h-3.5 w-3.5" /> Share this Mac
                </Button>
            </div>
        </header>

        <Callout v-if="state.error || state.statusError" tone="danger" role="alert">
            <p v-if="state.error">{{ state.error }}</p>
            <p v-if="state.statusError">{{ state.statusError }}</p>
        </Callout>
        <Callout v-if="state.notice" tone="ok" role="status">{{ state.notice }}</Callout>

        <div
            v-if="state.loading"
            class="flex items-center gap-2 py-8 text-xs text-zinc-500 dark:text-zinc-400"
            role="status"
        >
            <Spinner /> Checking this Mac's tools and saved approvals
        </div>
        <Callout v-else-if="status && !status.supported" title="Open buildbridge on your Mac">
            <p>
                Native iOS builds require macOS and Xcode. Open this page on the Mac that will build
                the app.
            </p>
            <p class="mt-1">
                Android builds on this computer use an Android machine with Docker.<template
                    v-if="remoteBuilds"
                >
                    Connect to a shared Mac from Remote builds.</template
                ><template v-else> A macOS machine builds for iOS on this computer.</template>
            </p>
            <div class="mt-3 flex flex-wrap gap-2">
                <Button v-if="remoteBuilds" size="sm" @click="ui.navigate({ kind: 'runner' })"
                    >Open Remote builds <ArrowRight class="h-3.5 w-3.5"
                /></Button>
                <Button
                    :variant="remoteBuilds ? 'outline' : 'default'"
                    size="sm"
                    @click="ui.navigate({ kind: 'home' })"
                    >Open machines</Button
                >
            </div>
        </Callout>

        <template v-else-if="status">
            <Tabs v-model="state.tab" :tabs="tabs" aria-label="Native Mac workflow" />

            <div
                v-if="state.tab === 'setup'"
                :id="`${id}-setup`"
                role="tabpanel"
                :aria-labelledby="`${id}-setup-tab`"
                class="space-y-4"
            >
                <Card>
                    <template #title>This Mac's tools</template>
                    <template #actions
                        ><Badge :tone="toolchain?.issues.length ? 'warn' : 'ok'">{{
                            toolchain?.issues.length ? 'Needs attention' : 'Available'
                        }}</Badge></template
                    >
                    <KeyValue
                        :columns="3"
                        :items="[
                            {
                                label: 'Xcode',
                                value: toolchain?.xcodeVersion?.split('\n')[0],
                                copyable: false,
                            },
                            { label: 'iOS SDK', value: toolchain?.iosSdk, copyable: false },
                            { label: 'Node.js', value: toolchain?.nodeVersion, copyable: false },
                        ]"
                    />
                    <Callout
                        v-if="toolchain?.issues.length"
                        tone="warn"
                        class="mt-3"
                        title="Finish installing or selecting the required tools"
                    >
                        <ul class="list-disc space-y-1 pl-4">
                            <li v-for="issue in toolchain.issues" :key="issue">{{ issue }}</li>
                        </ul>
                    </Callout>
                    <details class="mt-3">
                        <DisclosureSummary> More tool details </DisclosureSummary>
                        <KeyValue
                            class="mt-3"
                            :items="[
                                { label: 'Architecture', value: toolchain?.architecture },
                                { label: 'pnpm', value: toolchain?.pnpmVersion },
                                { label: 'CocoaPods', value: toolchain?.cocoapodsVersion },
                                {
                                    label: 'Selected Xcode directory',
                                    value: toolchain?.developerDirectory,
                                    mono: true,
                                },
                                {
                                    label: 'Available SDKs',
                                    value: toolchain?.availableSdks,
                                    copyable: true,
                                },
                            ]"
                        />
                    </details>
                </Card>

                <Card>
                    <template #title
                        ><span class="flex items-center gap-2"
                            >1. Approve a project
                            <Badge v-if="project" tone="ok">Approved</Badge></span
                        ></template
                    >
                    <template #description
                        >Choose the local iOS project whose repository, app identifier, and team
                        this Mac may build.</template
                    >
                    <template v-if="project && !projectFormOpen" #actions>
                        <Button
                            variant="outline"
                            size="sm"
                            :disabled="disabled"
                            @click="editingProject = true"
                            >Change</Button
                        >
                    </template>
                    <form v-if="projectFormOpen" class="space-y-3" @submit.prevent="approveProject">
                        <Field
                            label="Project folder"
                            required
                            hint="The folder holding the app's Xcode workspace or project, with an origin Git repository. Capacitor, Cordova, React Native, Expo and Flutter projects are recognised from their files."
                        >
                            <PathField
                                v-model="state.projectPath"
                                kind="directory"
                                title="Choose the project to approve on this Mac"
                                placeholder="/Users/you/Projects/your-app"
                                :disabled="disabled"
                            />
                        </Field>
                        <details>
                            <DisclosureSummary>
                                Project compatibility requirements (optional)
                            </DisclosureSummary>
                            <p class="mt-2 text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                                Declare the minimum versions your project needs. Without them, a
                                build must confirm compatibility. A committed .xcode-version is also
                                checked.
                            </p>
                            <div class="mt-3 grid gap-3 sm:grid-cols-2">
                                <Field
                                    label="Minimum Xcode"
                                    :error="
                                        validNativeMinimum(state.minXcode)
                                            ? null
                                            : 'Enter a version such as 16.4 or 26.0.'
                                    "
                                >
                                    <Input
                                        v-model="state.minXcode"
                                        placeholder="e.g. 16.4"
                                        :maxlength="32"
                                        :disabled="disabled"
                                    />
                                </Field>
                                <Field
                                    label="Minimum iOS SDK"
                                    :error="
                                        validNativeMinimum(state.minIosSdk)
                                            ? null
                                            : 'Enter a version such as 18.5 or 26.0.'
                                    "
                                >
                                    <Input
                                        v-model="state.minIosSdk"
                                        placeholder="e.g. 18.5"
                                        :maxlength="32"
                                        :disabled="disabled"
                                    />
                                </Field>
                            </div>
                        </details>
                        <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                            Builds fetch a selected commit into a separate checkout. The project's
                            build scripts run as your Mac account, so approve a repository you
                            trust.
                        </p>
                        <Callout v-if="signing" tone="warn"
                            >Approving the project again clears its signing approval. Select the
                            matching identity and profile again afterward.</Callout
                        >
                        <div class="flex gap-2">
                            <Button type="submit" :disabled="disabled || !projectValid"
                                ><Spinner
                                    v-if="state.saving === 'project'"
                                    tone="text-white dark:text-zinc-950"
                                /><Check v-else class="h-3.5 w-3.5" />
                                {{
                                    project ? 'Approve updated project' : 'Approve project'
                                }}</Button
                            >
                            <Button
                                v-if="project"
                                variant="ghost"
                                :disabled="disabled"
                                @click="cancelEdit"
                                >Cancel</Button
                            >
                        </div>
                    </form>
                    <KeyValue
                        v-else-if="project"
                        :items="[
                            { label: 'Project', value: project.name },
                            {
                                label: 'App identifier',
                                value: project.bundleIdentifier,
                                mono: true,
                            },
                            { label: 'Repository', value: project.repository, mono: true },
                            { label: 'Local folder', value: project.path, mono: true },
                            { label: 'Team', value: project.developmentTeam },
                            { label: 'Scheme', value: project.scheme },
                            {
                                label: 'Minimum Xcode',
                                value: project.minXcodeVersion || 'Not declared',
                                copyable: false,
                            },
                            {
                                label: 'Minimum iOS SDK',
                                value: project.minIosSdkVersion || 'Not declared',
                                copyable: false,
                            },
                        ]"
                    />
                    <Callout
                        v-if="status.compatibilityWarnings.length"
                        tone="warn"
                        class="mt-3"
                        title="Compatibility still needs a build"
                    >
                        <ul class="list-disc space-y-1 pl-4">
                            <li v-for="warning in status.compatibilityWarnings" :key="warning">
                                {{ warning }}
                            </li>
                        </ul>
                    </Callout>
                </Card>

                <Card v-if="project">
                    <template #title
                        ><span class="flex items-center gap-2"
                            >2. App Store signing
                            <Badge :tone="status.archiveReady ? 'ok' : 'neutral'">{{
                                status.archiveReady ? 'Ready' : 'Optional for compile tests'
                            }}</Badge></span
                        ></template
                    >
                    <template #description
                        >Use a distribution identity already in this Mac's keychain and its matching
                        App Store provisioning profile.</template
                    >
                    <template v-if="signing && !signingFormOpen" #actions
                        ><Button
                            variant="outline"
                            size="sm"
                            :disabled="disabled"
                            @click="editingSigning = true"
                            >Change</Button
                        ></template
                    >
                    <form v-if="signingFormOpen" class="space-y-3" @submit.prevent="approveSigning">
                        <Field
                            label="Distribution identity"
                            required
                            hint="The private key stays in this Mac's keychain."
                        >
                            <Select
                                v-model="state.identitySha1"
                                :options="identityOptions"
                                placeholder="Choose an existing distribution identity"
                                :disabled="disabled || !identityOptions.length"
                            />
                        </Field>
                        <Callout v-if="!identityOptions.length" tone="warn"
                            >No distribution identity is available. Add your Apple Distribution
                            certificate and private key in Keychain Access, unlock the keychain,
                            then select Recheck.</Callout
                        >
                        <Field
                            label="App Store provisioning profile"
                            required
                            hint="Must match the approved app, team, and selected distribution certificate."
                        >
                            <PathField
                                v-model="state.profilePath"
                                kind="file"
                                title="Choose the App Store provisioning profile"
                                :filter="{
                                    name: 'Provisioning profile',
                                    extensions: ['mobileprovision'],
                                }"
                                placeholder="Choose a .mobileprovision file"
                                :disabled="disabled"
                            />
                        </Field>
                        <div class="flex flex-wrap gap-2">
                            <Button
                                type="submit"
                                :disabled="
                                    disabled || !state.identitySha1 || !state.profilePath.trim()
                                "
                                ><Spinner
                                    v-if="state.saving === 'signing'"
                                    tone="text-white dark:text-zinc-950"
                                /><Check v-else class="h-3.5 w-3.5" /> Approve signing</Button
                            >
                            <Button
                                v-if="signing"
                                variant="ghost"
                                :disabled="disabled"
                                @click="cancelEdit"
                                >Cancel</Button
                            >
                        </div>
                    </form>
                    <template v-else-if="signing">
                        <KeyValue
                            :items="[
                                {
                                    label: 'Distribution identity',
                                    value: signing.identityName,
                                    copyable: false,
                                },
                                {
                                    label: 'Profile expires',
                                    value: signing.profile.expiresAt,
                                    copyable: false,
                                },
                                {
                                    label: 'Profile app identifier',
                                    value: signing.profile.applicationIdentifier,
                                    mono: true,
                                },
                                { label: 'Team', value: signing.profile.teamIdentifier },
                            ]"
                        />
                        <details class="mt-3">
                            <DisclosureSummary> Signing details </DisclosureSummary>
                            <KeyValue
                                class="mt-3"
                                :items="[
                                    {
                                        label: 'Identity fingerprint',
                                        value: signing.identitySha1,
                                        mono: true,
                                    },
                                    {
                                        label: 'Profile UUID',
                                        value: signing.profile.uuid,
                                        mono: true,
                                    },
                                    {
                                        label: 'Saved profile',
                                        value: signing.profilePath,
                                        mono: true,
                                    },
                                    {
                                        label: 'Profile SHA-256',
                                        value: signing.profile.sha256,
                                        mono: true,
                                    },
                                ]"
                            />
                        </details>
                    </template>
                </Card>

                <Callout v-if="setupIssues.length" tone="warn" title="Before you build">
                    <ul class="list-disc space-y-1 pl-4">
                        <li v-for="issue in setupIssues" :key="issue">{{ issue }}</li>
                    </ul>
                </Callout>
                <div class="flex flex-wrap items-center justify-between gap-3">
                    <Checkbox v-model="launchAtLogin" :disabled="disabled"
                        >Open buildbridge at login</Checkbox
                    >
                    <Button
                        :disabled="!status.testReady || native.busy.value"
                        @click="state.tab = 'build'"
                        >Continue to build <ArrowRight class="h-3.5 w-3.5"
                    /></Button>
                </div>
                <p class="text-xs text-zinc-500 dark:text-zinc-400">
                    Opening at login keeps buildbridge available after you sign in.<template
                        v-if="remoteBuilds"
                    >
                        Shared builds also need an active connection and the Mac to stay
                        awake.</template
                    >
                </p>
            </div>

            <div
                v-else
                :id="`${id}-build`"
                role="tabpanel"
                :aria-labelledby="`${id}-build-tab`"
                class="space-y-4"
            >
                <ProgressRow
                    v-if="state.running || status.busy || state.buildState === 'failed'"
                    :label="
                        state.stopping
                            ? 'Stopping the native build'
                            : state.buildState === 'failed' && !native.busy.value
                              ? 'Native build failed'
                              : state.progress?.label || 'Native Mac operation in progress'
                    "
                    :state="
                        state.buildState === 'failed' && !native.busy.value ? 'failed' : 'running'
                    "
                    :last-line="state.progress?.logLine"
                    :elapsed-seconds="elapsed"
                    :stoppable="!state.saving"
                    :stopping="state.stopping"
                    @stop="native.cancel"
                />
                <Card>
                    <template #title>{{ project?.name || 'Build an approved project' }}</template>
                    <template #description>{{
                        project?.repository ||
                        'Approve your local project in Setup to choose a source commit.'
                    }}</template>
                    <form
                        class="space-y-3"
                        @submit.prevent="buildBlocked ? undefined : native.runBuild()"
                    >
                        <fieldset :disabled="disabled" class="grid gap-2 sm:grid-cols-2">
                            <legend
                                class="mb-2 text-[13px] font-medium text-zinc-700 dark:text-zinc-200"
                            >
                                Outcome
                            </legend>
                            <button
                                v-for="option in [
                                    {
                                        value: 'test' as const,
                                        label: 'Compile test',
                                        description:
                                            'Confirm this iOS commit builds. Unsigned; no app preview.',
                                        icon: Hammer,
                                    },
                                    {
                                        value: 'archive' as const,
                                        label: 'Signed IPA',
                                        description:
                                            'Create the IPA and Xcode archive for App Store distribution.',
                                        icon: Package,
                                    },
                                ]"
                                :key="option.value"
                                type="button"
                                :aria-pressed="state.outcome === option.value"
                                class="flex items-start gap-3 rounded-lg border p-3 text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 disabled:cursor-not-allowed disabled:opacity-50 dark:focus-visible:outline-zinc-300"
                                :class="
                                    state.outcome === option.value
                                        ? 'border-zinc-900 bg-zinc-50 dark:border-zinc-100 dark:bg-zinc-800'
                                        : 'border-zinc-200 hover:bg-zinc-50 dark:border-zinc-800 dark:hover:bg-zinc-800'
                                "
                                @click="state.outcome = option.value"
                            >
                                <component
                                    :is="option.icon"
                                    class="mt-0.5 h-3.5 w-3.5 shrink-0"
                                    aria-hidden="true"
                                />
                                <span class="min-w-0 flex-1">
                                    <span
                                        class="flex items-center justify-between gap-2 text-xs font-semibold text-zinc-900 dark:text-zinc-50"
                                    >
                                        {{ option.label }}
                                        <Check
                                            v-if="state.outcome === option.value"
                                            class="h-3.5 w-3.5 shrink-0"
                                            aria-hidden="true"
                                        />
                                    </span>
                                    <span
                                        class="mt-1 block text-xs leading-5 text-zinc-500 dark:text-zinc-400"
                                    >
                                        {{ option.description }}
                                    </span>
                                </span>
                            </button>
                        </fieldset>
                        <Field
                            label="Git commit"
                            required
                            :error="commitError"
                            hint="Fetch this commit from the approved repository. Uncommitted edits stay on this Mac."
                        >
                            <Input
                                v-model="state.commit"
                                mono
                                placeholder="Full Git commit ID"
                                :maxlength="64"
                                :disabled="disabled"
                                autocomplete="off"
                                spellcheck="false"
                            />
                        </Field>
                        <Field
                            label="Environment"
                            :error="
                                environmentMissing
                                    ? 'This environment is no longer stored on this Mac. Choose another.'
                                    : null
                            "
                        >
                            <Select
                                v-model="state.envSetName"
                                :options="environmentOptions"
                                :disabled="disabled || envs.state.loading"
                            />
                            <template #action
                                ><Button variant="ghost" @click="ui.navigate({ kind: 'envs' })"
                                    >Manage environments</Button
                                ></template
                            >
                        </Field>
                        <Callout v-if="envs.state.error" tone="warn">{{
                            envs.state.error
                        }}</Callout>
                        <div class="flex flex-wrap items-center gap-3">
                            <Button
                                type="submit"
                                :disabled="!!buildBlocked"
                                :title="buildBlocked || undefined"
                                ><Spinner
                                    v-if="state.running"
                                    tone="text-white dark:text-zinc-950"
                                /><Hammer v-else class="h-3.5 w-3.5" />{{
                                    state.outcome === 'archive'
                                        ? 'Build signed IPA'
                                        : 'Compile commit'
                                }}</Button
                            >
                            <Button
                                v-if="!ready && !native.busy.value"
                                variant="outline"
                                @click="state.tab = 'setup'"
                                >Review setup</Button
                            >
                            <p
                                v-if="buildBlocked && !native.busy.value"
                                class="text-xs text-zinc-500 dark:text-zinc-400"
                            >
                                {{ buildBlocked }}
                            </p>
                        </div>
                    </form>
                    <details
                        v-if="status.compatibilityWarnings.length"
                        class="mt-3 text-xs leading-5 text-amber-700 dark:text-amber-400"
                    >
                        <DisclosureSummary>
                            Compatibility needs a build to confirm
                        </DisclosureSummary>
                        <ul class="mt-2 list-disc space-y-1 pl-4">
                            <li v-for="warning in status.compatibilityWarnings" :key="warning">
                                {{ warning }}
                            </li>
                        </ul>
                    </details>
                </Card>
                <Callout v-if="state.buildState === 'stopped'" tone="neutral"
                    >Build stopped. Choose a commit when you are ready to try again.</Callout
                >
                <details
                    v-if="state.logs.length || state.running || status.busy"
                    :open="state.running || status.busy || state.buildState === 'failed'"
                >
                    <DisclosureSummary>
                        Build log · {{ state.logs.length }} lines
                    </DisclosureSummary>
                    <LogView class="mt-2" :lines="state.logs" height="h-56" />
                </details>

                <Card v-if="result">
                    <template #title
                        ><span class="flex items-center gap-2"
                            ><Check class="h-3.5 w-3.5 text-emerald-700 dark:text-emerald-400" />
                            Last completed
                            {{ result.outcome === 'archive' ? 'archive' : 'compile test' }}</span
                        ></template
                    >
                    <template #description>{{
                        result.outcome === 'archive'
                            ? 'The IPA is ready for your App Store distribution workflow.'
                            : 'This commit compiled successfully. A compile test does not launch the app on a simulator or phone.'
                    }}</template>
                    <KeyValue
                        :items="[
                            { label: 'Commit', value: result.commit, mono: true },
                            { label: 'Repository', value: result.repository, mono: true },
                            { label: 'App identifier', value: result.bundleIdentifier, mono: true },
                            {
                                label: 'Xcode',
                                value: result.xcodeVersion.split('\n')[0],
                                copyable: false,
                            },
                            {
                                label: 'Environment',
                                value: result.envSet || 'No environment',
                                copyable: false,
                            },
                        ]"
                    />
                    <div
                        v-if="result.artifacts.length"
                        class="mt-4 divide-y divide-zinc-200 rounded-md border border-zinc-200 px-3 dark:divide-zinc-800 dark:border-zinc-800"
                    >
                        <div
                            v-for="artifact in result.artifacts"
                            :key="artifact.path"
                            class="flex flex-wrap items-center justify-between gap-2 py-3"
                        >
                            <div class="min-w-0">
                                <p class="text-xs font-medium">
                                    {{ artifact.path.split('/').pop() }}
                                </p>
                                <p
                                    class="mt-0.5 flex items-center gap-1 text-[11px] text-zinc-500 dark:text-zinc-400"
                                >
                                    {{ formatBytes(artifact.bytes) }} · SHA-256
                                    {{ shortHash(artifact.sha256) }}
                                    <CopyButton
                                        :text="artifact.sha256"
                                        what="Copy artifact SHA-256"
                                        size="iconXs"
                                    />
                                </p>
                            </div>
                            <Button
                                variant="outline"
                                size="sm"
                                @click="native.reveal(artifact.path)"
                                ><FolderOpen class="h-3.5 w-3.5" /> Show in folder</Button
                            >
                        </div>
                    </div>
                    <details
                        v-if="result.outcome === 'archive'"
                        class="mt-4 border-t border-zinc-200 pt-4 dark:border-zinc-800"
                    >
                        <DisclosureSummary>Publish the release</DisclosureSummary>
                        <PublishingGuide
                            class="mt-4"
                            platform="ios"
                            :ipa="
                                result.artifacts.find((artifact) => artifact.path.endsWith('.ipa'))
                            "
                            :app-identifier="result.bundleIdentifier"
                            :environment="result.envSet"
                            @reveal="native.reveal"
                            @build="state.outcome = 'archive'"
                        />
                    </details>
                </Card>
            </div>
        </template>
    </div>
</template>
