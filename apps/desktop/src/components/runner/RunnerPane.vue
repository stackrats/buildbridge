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
} from '@lucide/vue';
import { computed, ref } from 'vue';

import { formatTime } from '../../lib/format';
import { useRunnerStore } from '../../stores/runner';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import EmptyState from '../ui/EmptyState.vue';
import Field from '../ui/Field.vue';
import Input from '../ui/Input.vue';
import KeyValue from '../ui/KeyValue.vue';
import Spinner from '../ui/Spinner.vue';
import ConfirmDialog from '../dialogs/ConfirmDialog.vue';

const runner = useRunnerStore();

const serverUrl = ref('http://127.0.0.1:8001');
const runnerName = ref('');
const pairingCode = ref('');
const unpairOpen = ref(false);

const status = computed(() => runner.state.status);
const paired = computed(() => status.value?.paired === true);

const defaultName = computed(() =>
    status.value ? `${status.value.platform}-${status.value.architecture}` : '',
);

const realtimeBadge = computed(() => {
    switch (runner.state.realtime) {
        case 'connected':
            return { tone: 'ok' as const, label: 'connected' };
        case 'connecting':
            return { tone: 'warn' as const, label: 'connecting…' };
        case 'unavailable':
            return { tone: 'danger' as const, label: 'unavailable' };
        default:
            return { tone: 'neutral' as const, label: 'disconnected' };
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
</script>

<template>
    <div class="mx-auto max-w-4xl space-y-4 p-5">
        <header>
            <h1 class="text-lg font-bold text-zinc-900 dark:text-zinc-50">Control plane</h1>
            <p class="mt-0.5 text-xs text-zinc-500 dark:text-zinc-400">
                Optional. This desktop builds on its own; pairing it to a control plane adds
                starting builds from any browser and a record of every build. The machines here
                still do all the building.
            </p>
        </header>

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
            happens when the keyring is reset. Generate a new pairing code in the control plane and
            pair again; machines, projects and artifacts on this host are unaffected.
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
                <template #description>Paired and reporting its machines</template>
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
                        { label: 'Control plane', value: status.serverUrl, mono: true },
                        { label: 'Version', value: status.version, mono: true },
                        { label: 'Host', value: `${status.platform} / ${status.architecture}` },
                        { label: 'Token storage', value: 'Operating-system credential vault' },
                    ]"
                />
                <p class="mt-3 text-[11px] leading-4 text-zinc-500 dark:text-zinc-400">
                    Builds are claimed when the channel connects or a queue event arrives, and the
                    20-second heartbeat reports back anything still waiting, so a missed event is
                    caught within a heartbeat. The heartbeat also reports the machines here, so the
                    control plane can queue a signed archive on any machine that is ready — of the
                    approved folder as it is, or of a branch, tag, or commit of the same project.
                </p>
            </Card>

            <Card v-else>
                <template #title>Pair this runner</template>
                <template #description>
                    Nothing here is needed to build locally. Pair when you want to queue a signed
                    archive from another device, or keep build history somewhere the desktop is not.
                    Generate a single-use code in the control plane and enter it here; the resulting
                    token is stored only in the operating-system vault.
                </template>
                <form class="space-y-3" @submit.prevent="pair">
                    <Field label="Control plane URL">
                        <Input
                            v-model="serverUrl"
                            type="url"
                            placeholder="https://buildbridge.example"
                        />
                    </Field>
                    <Field
                        label="Runner name"
                        :hint="`Defaults to ${defaultName || 'platform-architecture'}`"
                    >
                        <Input v-model="runnerName" :placeholder="defaultName" :maxlength="80" />
                    </Field>
                    <Field label="Pairing code">
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
                            :disabled="runner.state.pairing || !pairingCode.trim()"
                        >
                            <Spinner
                                v-if="runner.state.pairing"
                                tone="text-white dark:text-zinc-950"
                            />
                            <Plus v-else class="h-3.5 w-3.5" />
                            Pair securely
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
                    title="Nothing yet"
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
            <p>Revoke the runner in the web control plane as well so the token cannot be reused.</p>
        </ConfirmDialog>
    </div>
</template>
