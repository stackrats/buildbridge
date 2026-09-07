<script setup lang="ts">
import {
    CircleCheck,
    CircleX,
    Info,
    Plus,
    Radio,
    RefreshCw,
    TriangleAlert,
    Unplug,
    ExternalLink,
} from '@lucide/vue';
import { computed, ref } from 'vue';

import { formatTime } from '../../lib/format';
import { useBackend } from '../../lib/backend';
import { describeError } from '../../lib/utils';
import { useRunnerStore } from '../../stores/runner';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import EmptyState from '../ui/EmptyState.vue';
import Field from '../ui/Field.vue';
import Input from '../ui/Input.vue';
import KeyValue from '../ui/KeyValue.vue';
import Spinner from '../ui/Spinner.vue';
import ConfirmDialog from '../dialogs/ConfirmDialog.vue';
import SharingPane from './SharingPane.vue';

const runner = useRunnerStore();

const serverUrl = ref('');
const runnerName = ref('');
const pairingCode = ref('');
const unpairOpen = ref(false);
const browseServer = ref('');
const browseError = ref<string | null>(null);

const status = computed(() => runner.state.status);
const paired = computed(() => status.value?.paired === true);

const defaultName = computed(() =>
    status.value ? `${status.value.platform}-${status.value.architecture}` : '',
);

const realtimeBadge = computed(() => {
    switch (runner.state.realtime) {
        case 'connected':
            return { tone: 'ok' as const, label: 'Connected' };
        case 'connecting':
            return { tone: 'warn' as const, label: 'Connecting' };
        case 'unavailable':
            return { tone: 'danger' as const, label: 'Dashboard unreachable' };
        default:
            return { tone: 'neutral' as const, label: 'Not connected' };
    }
});

const activityIcon = {
    info: Info,
    success: CircleCheck,
    warning: TriangleAlert,
    danger: CircleX,
};

const activityTone = {
    info: 'text-zinc-600 dark:text-zinc-300',
    success: 'text-emerald-700 dark:text-emerald-400',
    warning: 'text-amber-700 dark:text-amber-400',
    danger: 'text-red-700 dark:text-red-400',
};

async function pair(): Promise<void> {
    try {
        await runner.pair({
            serverUrl: serverUrl.value.trim(),
            code: pairingCode.value.trim(),
            runnerName: (runnerName.value.trim() || defaultName.value).slice(0, 80),
        });
        pairingCode.value = '';
    } catch {
        // The store keeps the error for display.
    }
}

// The dialog stays up with its button spinning until the token is gone from the vault.
async function unpair(): Promise<void> {
    await runner.unpair();
    unpairOpen.value = false;
}

async function openDashboard(): Promise<void> {
    browseError.value = null;
    try {
        const address = browseServer.value.trim() || status.value?.serverUrl || '';
        const parsed = new URL(address);
        if (!['https:', 'http:'].includes(parsed.protocol) || parsed.username || parsed.password)
            throw new Error('Enter your buildbridge server address.');
        await useBackend().openUrl(parsed.toString());
    } catch (error) {
        browseError.value = describeError(error);
    }
}
</script>

<template>
    <div class="mx-auto max-w-4xl space-y-4 p-5">
        <header class="flex items-start justify-between gap-4">
            <div>
                <h1 class="text-lg font-bold text-zinc-900 dark:text-zinc-50">Remote builds</h1>
                <p class="mt-1 max-w-2xl text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    Build on another computer, or let trusted people build on this one. Local builds
                    work without a connection.
                </p>
            </div>
        </header>

        <Card v-if="!paired">
            <template #title>Use a shared builder</template>
            <template #description
                >Open the address included with your invitation, sign in and enter the code to
                request access.</template
            >
            <form class="space-y-3" @submit.prevent="openDashboard">
                <Field
                    label="buildbridge server address"
                    required
                    hint="Use the address included with the sharing invitation."
                    ><Input
                        v-model="browseServer"
                        type="url"
                        placeholder="https://buildbridge.example"
                /></Field>
                <Callout v-if="browseError" tone="danger">{{ browseError }}</Callout>
                <div class="flex flex-wrap items-center justify-between gap-3">
                    <p class="text-xs text-zinc-500 dark:text-zinc-400">
                        This computer does not need to accept builds.
                    </p>
                    <Button type="submit" size="sm" :disabled="!browseServer.trim()"
                        ><ExternalLink class="h-3.5 w-3.5" />Open in browser</Button
                    >
                </div>
            </form>
        </Card>

        <SharingPane v-if="paired" />

        <Callout
            v-if="status?.credentialsMissing"
            tone="danger"
            title="This runner's token is missing from the vault"
        >
            A runner is configured here
            <template v-if="status.runnerName"
                >as <b>{{ status.runnerName }}</b></template
            >
            , but its token is gone from this host's operating-system keyring — the same thing that
            happens when the keyring is reset. Open the dashboard in your browser to generate a new
            pairing code and pair again; machines, projects and artifacts on this host are
            unaffected.
        </Callout>

        <Callout v-if="runner.state.error" tone="danger">
            {{ runner.state.error }}
        </Callout>

        <div class="grid gap-4 lg:grid-cols-[minmax(0,1fr)_320px]">
            <Card v-if="paired && status">
                <template #title>
                    <span class="flex items-center gap-2">
                        <Radio class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" />
                        {{ status.runnerName }}
                        <Badge :tone="realtimeBadge.tone">{{ realtimeBadge.label }}</Badge>
                    </span>
                </template>
                <template #description>Remote builds run on this computer.</template>
                <template #actions>
                    <Button
                        variant="outline"
                        size="sm"
                        :disabled="runner.state.checking"
                        @click="runner.checkForWork()"
                    >
                        <Spinner v-if="runner.state.checking" />
                        <RefreshCw v-else class="h-3.5 w-3.5" />
                        Check for work
                    </Button>
                    <Button variant="ghost" size="sm" @click="unpairOpen = true">
                        <Unplug class="h-3.5 w-3.5 text-red-700 dark:text-red-400" />
                        Unpair
                    </Button>
                </template>
                <KeyValue
                    :items="[
                        { label: 'Runner ID', value: status.runnerId, mono: true },
                        { label: 'buildbridge server', value: status.serverUrl, mono: true },
                        { label: 'Version', value: status.version, mono: true },
                        { label: 'Host', value: `${status.platform} / ${status.architecture}` },
                        { label: 'Token storage', value: 'Operating-system credential vault' },
                    ]"
                />
                <details class="mt-3">
                    <DisclosureSummary> How remote builds work </DisclosureSummary>
                    <p class="mt-2 text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                        Queue builds for projects you have approved on this computer. Native Mac
                        builds require a full commit SHA. Virtual Mac and Android builders can use
                        the approved local folder or a Git revision.
                    </p>
                </details>
            </Card>

            <Card v-else>
                <template #title>Connect this computer to accept builds</template>
                <template #description>
                    Open the dashboard in your browser and choose Pair runner to get a pairing code.
                    Enter it here to connect this computer to your account. You approve access for
                    other people separately.
                </template>
                <form class="space-y-3" @submit.prevent="pair">
                    <Field
                        label="buildbridge server address"
                        required
                        hint="Use the same address where you generated the pairing code."
                    >
                        <Input
                            v-model="serverUrl"
                            type="url"
                            placeholder="https://buildbridge.example"
                        />
                    </Field>
                    <Field
                        label="Name for this desktop"
                        :hint="`Defaults to ${defaultName || 'platform-architecture'}`"
                    >
                        <Input v-model="runnerName" :placeholder="defaultName" :maxlength="80" />
                    </Field>
                    <Field label="Pairing code" required>
                        <Input
                            v-model="pairingCode"
                            mono
                            placeholder="Single-use code"
                            autocomplete="one-time-code"
                            :maxlength="20"
                        />
                    </Field>
                    <div class="flex justify-end">
                        <Button
                            type="submit"
                            size="sm"
                            :disabled="
                                runner.state.pairing || !pairingCode.trim() || !serverUrl.trim()
                            "
                        >
                            <Spinner
                                v-if="runner.state.pairing"
                                tone="text-white dark:text-zinc-950"
                            />
                            <Plus v-else class="h-3.5 w-3.5" />
                            Connect this computer
                        </Button>
                    </div>
                </form>
            </Card>

            <Card>
                <template #title>Recent activity</template>
                <template #description
                    >Claims, completions, and connection events for this session.</template
                >
                <ul v-if="runner.state.activity.length" class="space-y-1.5">
                    <li
                        v-for="entry in runner.state.activity"
                        :key="entry.id"
                        class="flex items-start gap-2 text-xs text-zinc-600 dark:text-zinc-300"
                    >
                        <component
                            :is="activityIcon[entry.tone]"
                            class="mt-0.5 h-3.5 w-3.5 shrink-0"
                            :class="activityTone[entry.tone]"
                        />
                        <span class="min-w-0 flex-1">
                            {{ entry.message }}
                            <span
                                v-if="entry.buildId"
                                class="font-mono text-[11px] text-zinc-500 dark:text-zinc-400"
                            >
                                · {{ entry.buildId.slice(0, 8) }}
                            </span>
                        </span>
                        <span
                            class="shrink-0 text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
                        >
                            {{ formatTime(entry.at) }}
                        </span>
                    </li>
                </ul>
                <EmptyState
                    v-else
                    title="No activity yet"
                    description="Queued builds and their results appear here as they are claimed."
                />
            </Card>
        </div>

        <ConfirmDialog
            v-model:open="unpairOpen"
            title="Unpair this runner"
            confirm-label="Unpair locally"
            :busy="runner.state.unpairing"
            @confirm="unpair"
        >
            <p>
                The runner token is removed from the operating-system vault and this desktop stops
                receiving builds. Machines and their disks are not affected.
            </p>
            <p>Also remove this builder in the dashboard to revoke its access.</p>
        </ConfirmDialog>
    </div>
</template>
