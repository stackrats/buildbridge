<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { ExternalLink, Pause, Play, Share2, RefreshCw } from '@lucide/vue';
import { useBackend } from '../../lib/backend';
import { describeError } from '../../lib/utils';
import { useRunnerStore } from '../../stores/runner';
import type { SharingGrant, SharingInvitation, SharingOverview } from '../../types/backend';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import Checkbox from '../ui/Checkbox.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import CopyButton from '../ui/CopyButton.vue';
import Field from '../ui/Field.vue';
import Modal from '../ui/Modal.vue';
import Select from '../ui/Select.vue';
import Spinner from '../ui/Spinner.vue';
import ConfirmDialog from '../dialogs/ConfirmDialog.vue';

const runner = useRunnerStore();
const overview = ref<SharingOverview | null>(null);
const error = ref<string | null>(null);
const busy = ref(false);
const loading = ref(true);
const open = ref(false);
const machineId = ref('');
const duration = ref('24');
const allowedEnvs = ref<string[]>([]);
const invitation = ref<SharingInvitation | null>(null);
const approving = ref<SharingGrant | null>(null);
let timer: ReturnType<typeof setTimeout> | null = null;
let disposed = false;

const targets = computed(() => overview.value?.targets ?? []);
const selected = computed(() => targets.value.find((target) => target.id === machineId.value));
const targetOptions = computed(() =>
    targets.value.map((target) => ({
        value: target.id,
        label: `${target.name} · ${target.project ?? 'no project approved'}`,
        disabled: !target.ready || !target.repository,
    })),
);
const envOptions = computed(() =>
    selected.value ? [...(selected.value.env_set ? [] : ['']), ...selected.value.env_sets] : [],
);
const requests = computed(
    () => overview.value?.state.grants.filter((grant) => grant.status === 'pending') ?? [],
);
const grants = computed(
    () => overview.value?.state.grants.filter((grant) => grant.status !== 'pending') ?? [],
);
const server = computed(() => runner.state.status?.serverUrl ?? '');
const invitationText = computed(
    () =>
        `Open ${server.value} in your browser, sign in, and choose Use a shared builder.\nEnter invitation code: ${invitation.value?.code ?? ''}\nThe owner will review your request before you can build.`,
);

async function refresh(silent = false): Promise<void> {
    if (!silent) loading.value = true;
    try {
        overview.value = await useBackend().getSharing();
        if (!silent) error.value = null;
    } catch (cause) {
        if (!silent) error.value = describeError(cause);
    } finally {
        loading.value = false;
    }
}
async function poll(): Promise<void> {
    if (!busy.value) await refresh(true);
    if (!disposed)
        timer = setTimeout(() => {
            void poll();
        }, 5000);
}
onMounted(async () => {
    await refresh();
    if (!disposed)
        timer = setTimeout(() => {
            void poll();
        }, 5000);
});
onBeforeUnmount(() => {
    disposed = true;
    if (timer) clearTimeout(timer);
});

watch(machineId, () => {
    allowedEnvs.value = selected.value ? [selected.value.env_set ?? ''] : [];
});
watch(approving, () => {});

function begin(): void {
    invitation.value = null;
    error.value = null;
    machineId.value = targets.value.find((target) => target.ready && target.repository)?.id ?? '';
    allowedEnvs.value = selected.value ? [selected.value.env_set ?? ''] : [];
    open.value = true;
}
function setEnvironment(name: string, checked: boolean): void {
    allowedEnvs.value = checked
        ? [...new Set([...allowedEnvs.value, name])]
        : allowedEnvs.value.filter((value) => value !== name);
}
async function perform(action: () => Promise<void>): Promise<void> {
    busy.value = true;
    error.value = null;
    try {
        await action();
        await refresh(true);
    } catch (cause) {
        error.value = describeError(cause);
    } finally {
        busy.value = false;
    }
}
async function create(): Promise<void> {
    await perform(async () => {
        invitation.value = await useBackend().createSharingInvitation({
            machineId: machineId.value,
            envSets: allowedEnvs.value,
            accessHours: Number(duration.value),
        });
    });
}
async function approve(): Promise<void> {
    if (!approving.value) return;
    const id = approving.value.id;
    await perform(async () => {
        await useBackend().approveSharingGrant(id);
        approving.value = null;
    });
}
async function openDashboard(): Promise<void> {
    try {
        await useBackend().openUrl(server.value);
    } catch (cause) {
        error.value = describeError(cause);
    }
}
function date(value: string | null): string {
    return value ? new Date(value).toLocaleString() : 'Starts when approved';
}
// The dashboard's grant states in the reader's words; an approved grant past its end is expired
// here before the dashboard says so.
const grantStatusLabels: Record<string, string> = {
    pending: 'Needs approval',
    approved: 'Approved',
    revoked: 'Revoked',
    expired: 'Expired',
    cancelled: 'Cancelled',
};
function grantActive(grant: SharingGrant): boolean {
    return (
        grant.status === 'approved' &&
        grant.expires_at !== null &&
        Date.parse(grant.expires_at) > Date.now()
    );
}
function grantLabel(grant: SharingGrant): string {
    if (grant.status === 'approved' && grant.expires_at && !grantActive(grant)) {
        return 'Expired';
    }
    const status = grant.status.replace(/_/g, ' ');
    return grantStatusLabels[grant.status] ?? status.charAt(0).toUpperCase() + status.slice(1);
}
</script>

<template>
    <Card>
        <template #title>Share this Mac</template>
        <template #description
            >Let a trusted collaborator request signed iOS builds on your Mac. Signing credentials
            stay on this host.</template
        >
        <template #actions>
            <Button variant="ghost" size="sm" :disabled="loading || busy" @click="refresh()"
                ><RefreshCw class="h-3.5 w-3.5" />Refresh</Button
            >
            <Button
                v-if="overview"
                variant="outline"
                size="sm"
                :disabled="busy"
                @click="perform(() => useBackend().setSharingPaused(!overview!.paused))"
            >
                <Play v-if="overview.paused" class="h-3.5 w-3.5" /><Pause
                    v-else
                    class="h-3.5 w-3.5"
                />{{ overview.paused ? 'Resume sharing' : 'Pause sharing' }}
            </Button>
            <Button
                size="sm"
                :disabled="
                    busy || !targetOptions.some((option) => !option.disabled) || overview?.paused
                "
                @click="begin"
                ><Share2 class="h-3.5 w-3.5" />Create invitation</Button
            >
        </template>
        <div class="space-y-3">
            <Callout v-if="error && !open && !approving" tone="danger">{{ error }}</Callout>
            <p
                v-if="loading && !overview"
                class="flex items-center gap-2 text-xs text-zinc-500 dark:text-zinc-400"
            >
                <Spinner />Loading sharing permissions
            </p>
            <Callout v-if="overview?.paused" tone="warn"
                >Shared builds are paused. Active shared builds stop at the next permission check;
                your own builds remain available.</Callout
            >
            <p
                v-if="overview && !targetOptions.some((option) => !option.disabled)"
                class="text-xs leading-5 text-zinc-500 dark:text-zinc-400"
            >
                Sharing currently supports a Mac running buildbridge directly. On that Mac, open
                This Mac and approve a Git project and signing credentials before creating an
                invitation. You can also start your own Android and virtual Mac builds in your
                browser.
            </p>
            <div
                v-for="request in requests"
                :key="request.id"
                class="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-zinc-200 p-3 dark:border-zinc-800"
            >
                <div class="min-w-0">
                    <p class="text-sm font-medium">
                        {{ request.requester_name }} <Badge tone="warn">Needs approval</Badge>
                    </p>
                    <p class="mt-1 text-xs break-words text-zinc-500 dark:text-zinc-400">
                        {{ request.requester_email }} · {{ request.project }}
                    </p>
                </div>
                <div class="flex gap-2">
                    <Button
                        variant="ghost"
                        size="sm"
                        :disabled="busy"
                        @click="perform(() => useBackend().revokeSharingGrant(request.id))"
                        >Decline</Button
                    ><Button size="sm" :disabled="busy" @click="approving = request"
                        >Review access</Button
                    >
                </div>
            </div>
            <div
                v-for="grant in grants"
                :key="grant.id"
                class="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-zinc-200 p-3 dark:border-zinc-800"
            >
                <div class="min-w-0">
                    <p class="text-sm font-medium">
                        {{ grant.requester_name }}
                        <Badge :tone="grantActive(grant) ? 'ok' : 'neutral'">{{
                            grantLabel(grant)
                        }}</Badge>
                    </p>
                    <p class="mt-1 text-xs break-words text-zinc-500 dark:text-zinc-400">
                        {{ grant.project }} · Access ends {{ date(grant.expires_at) }}
                    </p>
                </div>
                <Button
                    v-if="grant.status === 'approved'"
                    variant="outline"
                    size="sm"
                    :disabled="busy"
                    @click="perform(() => useBackend().revokeSharingGrant(grant.id))"
                    >Revoke access</Button
                >
            </div>
            <p
                v-if="overview && !requests.length && !grants.length"
                class="text-xs leading-5 text-zinc-500 dark:text-zinc-400"
            >
                Create a code, send it to your collaborator, then approve their request here. Codes
                expire after 10 minutes and can be used once.
            </p>
            <div
                class="flex flex-wrap items-center justify-between gap-2 border-t border-zinc-200 pt-3 dark:border-zinc-800"
            >
                <p class="text-xs text-zinc-500 dark:text-zinc-400">
                    To use an invitation, open the invitation link in your browser, sign in, and
                    choose <b>Use a shared builder</b>.
                </p>
                <Button variant="outline" size="sm" @click="openDashboard"
                    ><ExternalLink class="h-3.5 w-3.5" />Open in browser</Button
                >
            </div>
        </div>
    </Card>

    <Modal v-model:open="open" title="Share this Mac" :busy="busy">
        <div v-if="invitation" class="space-y-4">
            <p class="text-sm">
                Send this invitation to your collaborator. They open the link in their browser, sign
                in, and enter the code; you approve their identity here before any build can run.
            </p>
            <div class="flex flex-col items-start gap-3 rounded-lg bg-zinc-50 p-4 dark:bg-zinc-950">
                <span class="max-w-full font-mono text-sm leading-6 break-all">{{
                    invitation.code
                }}</span
                ><CopyButton :text="invitationText" label="Copy invitation" />
            </div>
            <p class="text-xs text-zinc-500 dark:text-zinc-400">
                Code expires {{ date(invitation.expires_at) }}. Approved access lasts
                {{ invitation.access_hours }} hours from approval.
            </p>
        </div>
        <form v-else id="share-destination-form" class="space-y-4" @submit.prevent="create">
            <Field label="Build destination" required
                ><Select v-model="machineId" :options="targetOptions"
            /></Field>
            <div
                v-if="selected"
                class="rounded-lg bg-zinc-50 p-3 text-xs leading-5 dark:bg-zinc-950"
            >
                <p class="font-medium">{{ selected.project }}</p>
                <p class="break-words text-zinc-500 dark:text-zinc-400">
                    {{ selected.repository }}
                </p>
                <p class="mt-1">
                    {{
                        selected.platform === 'android'
                            ? 'Signed Android releases'
                            : 'Signed iOS archives and App Store exports'
                    }}<template v-if="selected.toolchain_version">
                        · Xcode {{ selected.toolchain_version }}</template
                    >
                </p>
            </div>
            <fieldset class="space-y-2">
                <legend class="mb-2 text-xs font-medium">Allowed environments</legend>
                <Checkbox
                    v-for="name in envOptions"
                    :key="name"
                    :model-value="allowedEnvs.includes(name)"
                    block
                    @update:model-value="setEnvironment(name, $event)"
                    >{{ name || 'No environment' }}</Checkbox
                >
            </fieldset>
            <Field
                label="Access duration"
                hint="Starts when you approve the person. You can revoke access earlier."
                ><Select
                    v-model="duration"
                    :options="[
                        { value: '1', label: '1 hour' },
                        { value: '24', label: '1 day' },
                        { value: '168', label: '7 days' },
                        { value: '720', label: '30 days' },
                    ]"
            /></Field>
            <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                Only exact commits from this approved repository can be requested. The selected
                project, signing identity and environments define the permission.
            </p>
        </form>
        <Callout v-if="error" class="mt-3" tone="danger">{{ error }}</Callout>
        <template #footer
            ><Button variant="outline" size="sm" :disabled="busy" @click="open = false">{{
                invitation ? 'Done' : 'Cancel'
            }}</Button
            ><Button
                v-if="!invitation"
                type="submit"
                form="share-destination-form"
                size="sm"
                :disabled="busy || !selected?.ready || !allowedEnvs.length"
                ><Spinner v-if="busy" tone="text-white dark:text-zinc-950" />Create
                invitation</Button
            ></template
        >
    </Modal>

    <ConfirmDialog
        :open="approving !== null"
        title="Approve build access"
        confirm-label="Approve access"
        :busy="busy"
        acknowledgement="I trust this person to run this project's build scripts on my computer."
        :destructive="false"
        @update:open="!$event && (approving = null)"
        @confirm="approve"
    >
        <template v-if="approving"
            ><p>
                <b>{{ approving.requester_name }}</b> ({{ approving.requester_email }}) is
                requesting signed builds of <b>{{ approving.project }}</b
                >.
            </p>
            <p>
                Allowed environments:
                {{ approving.env_sets.map((name) => name || 'No environment').join(', ') }}. This
                does not share your signing files or give access to other projects.
            </p></template
        >
        <Callout v-if="error" tone="danger">{{ error }}</Callout>
    </ConfirmDialog>
</template>
