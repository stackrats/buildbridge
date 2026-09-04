<script setup lang="ts">
import {
    Apple,
    BadgePlus,
    KeyRound,
    Pencil,
    Plus,
    Smartphone,
    TriangleAlert,
    Trash2,
} from '@lucide/vue';
import { computed, ref, watch } from 'vue';

import { formatDate } from '../../lib/format';
import { useMachinesStore } from '../../stores/machines';
import { kitReadiness, kitShortfall } from '../../model/signing';
import { useSigningStore } from '../../stores/signing';
import type { SigningKitSummary } from '../../types/backend';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import CopyButton from '../ui/CopyButton.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import Chip from '../ui/Chip.vue';
import EmptyState from '../ui/EmptyState.vue';
import KeyValue from '../ui/KeyValue.vue';
import Spinner from '../ui/Spinner.vue';
import ConfirmDialog from '../dialogs/ConfirmDialog.vue';
import AppleVerificationCard from './AppleVerificationCard.vue';
import SigningKitDialog from './SigningKitDialog.vue';

const signing = useSigningStore();
const machines = useMachinesStore();

// `?kitDialog=1` opens the form straight away and `?kitDialog=edit` opens the first stored kit,
// so the browser preview can show either state of it.
const previewDialog =
    import.meta.env.DEV && typeof location !== 'undefined'
        ? new URLSearchParams(location.search).get('kitDialog')
        : null;
const dialogOpen = ref(previewDialog !== null);
const editing = ref<SigningKitSummary | null>(null);

if (previewDialog === 'edit') {
    watch(
        signing.kits,
        (kits) => {
            editing.value = editing.value ?? kits[0] ?? null;
        },
        { immediate: true },
    );
}
const removing = ref<SigningKitSummary | null>(null);
// The kit whose Apple certificate creation is awaiting confirmation, and which identity.
const certifying = ref<{
    kit: SigningKitSummary;
    kind: 'distribution' | 'development';
} | null>(null);

async function createCertificate(): Promise<void> {
    const request = certifying.value;
    certifying.value = null;
    if (request?.kind === 'development') {
        await signing.createDevelopmentCertificate(request.kit.id);
    } else if (request) {
        await signing.createCertificate(request.kit.id);
    }
}

const creatingAny = computed(
    () =>
        signing.state.creatingCertificateKitId !== null ||
        signing.state.creatingDevelopmentCertificateKitId !== null,
);

function createKit(): void {
    editing.value = null;
    dialogOpen.value = true;
}

function editKit(kit: SigningKitSummary): void {
    editing.value = kit;
    dialogOpen.value = true;
}

async function remove(): Promise<void> {
    const kit = removing.value;
    if (!kit) {
        return;
    }
    const removed = await signing.remove(kit.id);
    removing.value = null;
    if (removed) {
        await machines.loadList();
    }
}

// A missing file is a warning only when nothing will create it: with a Team key it is simply
// not there yet.
function detailsFor(kit: SigningKitSummary) {
    const readiness = kitReadiness(kit);
    return [
        {
            label: 'Guest keychain password',
            value: kit.guestKeychainConfigured ? 'Stored in the OS vault' : 'Not stored',
            tone: kit.guestKeychainConfigured ? ('default' as const) : ('warn' as const),
        },
        {
            label: 'Team key',
            value: kit.appStoreConnectKeyId ?? 'None',
            mono: kit.appStoreConnectKeyId !== null,
        },
        {
            label: 'Distribution identity',
            value:
                kit.signingCertificateName ??
                (readiness.teamKey ? 'Created at Apple when a machine provisions' : 'Not stored'),
            tone:
                kit.signingCertificateConfigured || readiness.teamKey
                    ? ('default' as const)
                    : ('warn' as const),
        },
        {
            label: 'Export password',
            value: kit.signingCertificatePasswordStored
                ? 'Stored in the OS vault'
                : kit.signingCertificateConfigured
                  ? 'Not stored'
                  : readiness.teamKey
                    ? 'Set when the identity is created'
                    : 'Not stored',
            tone:
                kit.signingCertificatePasswordStored ||
                (readiness.teamKey && !kit.signingCertificateConfigured)
                    ? ('default' as const)
                    : ('warn' as const),
        },
        {
            label: 'Development identity',
            value:
                kit.developmentCertificateName ??
                (readiness.teamKey
                    ? 'Created at Apple when a phone is prepared'
                    : 'Not stored · optional, for iPhone builds'),
        },
        {
            label: 'Added',
            value: formatDate(new Date(kit.createdAtEpochSeconds * 1000).toISOString()),
        },
    ];
}

const anyMachines = computed(() => machines.machines.value.length > 0);

// Machines that signed before but now resolve to no kit: the shape a cleared keyring leaves.
const orphaned = computed(() =>
    machines.machines.value.filter(
        (machine) => machine.signingProvisioned && machine.signingKitName === null,
    ),
);
</script>

<template>
    <div class="mx-auto max-w-4xl space-y-4 p-5">
        <header class="flex items-start justify-between gap-4">
            <div>
                <h1 class="text-lg font-bold text-zinc-900 dark:text-zinc-50">Signing kits</h1>
                <p class="mt-1 max-w-2xl text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    A kit is one set of Apple signing credentials, held in this host's
                    operating-system vault. The simplest kit is a Team key and a keychain password:
                    BuildBridge creates the certificates and profiles from the key as machines need
                    them. Files exported from a Mac work too. Keep one kit per developer team,
                    attach it to each machine, and provisioning imports it into that machine's own
                    keychain.
                </p>
            </div>
            <Button size="sm" @click="createKit">
                <Plus class="h-3.5 w-3.5" />
                New kit
            </Button>
        </header>

        <Callout
            v-if="orphaned.length"
            tone="danger"
            title="Signing was provisioned from a kit that is no longer stored"
        >
            <p>
                {{ orphaned.map((machine) => machine.config.name).join(', ') }}
                {{ orphaned.length === 1 ? 'has' : 'have' }} a provisioned guest keychain, but no
                kit here matches. An operating-system keyring reset does exactly this. Store the kit
                again below, attach it at the machine's Attach a signing kit step, then provision
                once more.
            </p>
        </Callout>

        <template v-if="signing.state.messageKitId === null">
            <Callout v-if="signing.state.error" tone="danger">{{ signing.state.error }}</Callout>
            <Callout v-else-if="signing.state.notice" tone="ok">{{ signing.state.notice }}</Callout>
        </template>

        <EmptyState
            v-if="!signing.kits.value.length && !signing.state.loading"
            title="No signing kits stored"
            description="Store a Team key (an App Store Connect API key) and a keychain password once, and BuildBridge creates the certificates and profiles it needs at Apple. Or bring the .p12 and profiles exported from a Mac. Every machine can then be attached to the kit."
        >
            <template #icon><KeyRound class="h-4 w-4" /></template>
            <Button size="sm" @click="createKit">
                <Plus class="h-3.5 w-3.5" />
                Store the first kit
            </Button>
        </EmptyState>

        <Card v-for="kit in signing.kits.value" :key="kit.id">
            <template #title>
                <span class="flex flex-wrap items-center gap-2">
                    {{ kit.name }}
                    <Badge v-if="kitReadiness(kit).provisionable" tone="ok"
                        >ready to provision</Badge
                    >
                    <Badge v-else tone="warn">incomplete</Badge>
                    <Badge
                        v-if="kitReadiness(kit).provisionable && kitReadiness(kit).archive === null"
                        tone="warn"
                        >phone only</Badge
                    >
                </span>
            </template>
            <template #actions>
                <!-- Provisioning and the device step create these themselves; the buttons
                     are for creating one ahead of time, so they go once the identity exists. -->
                <Button
                    v-if="kit.appStoreConnectConfigured && !kit.signingCertificateConfigured"
                    variant="outline"
                    size="sm"
                    title="Not required: provisioning creates it when a machine first needs it"
                    :disabled="creatingAny"
                    @click="certifying = { kit, kind: 'distribution' }"
                >
                    <Spinner v-if="signing.state.creatingCertificateKitId === kit.id" />
                    <BadgePlus v-else class="h-3.5 w-3.5" />
                    Create distribution certificate now
                </Button>
                <Button
                    v-if="kit.appStoreConnectConfigured && !kit.developmentCertificateConfigured"
                    variant="outline"
                    size="sm"
                    title="Not required: preparing a phone creates it when needed"
                    :disabled="creatingAny"
                    @click="certifying = { kit, kind: 'development' }"
                >
                    <Spinner v-if="signing.state.creatingDevelopmentCertificateKitId === kit.id" />
                    <Smartphone v-else class="h-3.5 w-3.5" />
                    Create development certificate now
                </Button>
                <Button variant="outline" size="sm" @click="editKit(kit)">
                    <Pencil class="h-3.5 w-3.5" />
                    Edit
                </Button>
                <Button variant="ghost" size="sm" @click="removing = kit">
                    <Trash2 class="h-3.5 w-3.5 text-red-700 dark:text-red-400" />
                    Remove
                </Button>
            </template>

            <!-- A message about this kit sits beside the button that produced it. -->
            <template v-if="signing.state.messageKitId === kit.id">
                <Callout v-if="signing.state.error" tone="danger" class="mb-3">
                    {{ signing.state.error }}
                </Callout>
                <Callout v-else-if="signing.state.notice" tone="ok" class="mb-3">
                    {{ signing.state.notice }}
                </Callout>
            </template>

            <KeyValue :items="detailsFor(kit)" :columns="3" />
            <p
                v-if="!kitReadiness(kit).provisionable"
                class="mt-2 text-[11px] leading-4 text-amber-700 dark:text-amber-400"
            >
                Not usable yet: still needs {{ kitShortfall(kit).join(' and ') }}. Edit the kit to
                add it.
            </p>
            <p
                v-else-if="kitReadiness(kit).archive === null"
                class="mt-2 text-[11px] leading-4 text-amber-700 dark:text-amber-400"
            >
                This kit can run Debug builds on a phone, but cannot sign an App Store archive: it
                holds no distribution identity and no Team key to create one. Edit the kit to add
                either.
            </p>

            <div class="mt-3 border-t border-zinc-200 pt-3 dark:border-zinc-800">
                <p class="text-[11px] text-zinc-500 dark:text-zinc-400">
                    Provisioning profiles
                    <span class="tabular-nums">({{ kit.provisioningProfileNames.length }})</span>
                </p>
                <ul v-if="kit.provisioningProfileNames.length" class="mt-1 space-y-0.5">
                    <li
                        v-for="name in kit.provisioningProfileNames"
                        :key="name"
                        class="group flex items-center gap-1"
                    >
                        <span
                            class="min-w-0 font-mono text-[11px] break-all text-zinc-600 dark:text-zinc-300"
                            >{{ name }}</span
                        >
                        <CopyButton
                            :text="name"
                            what="Copy the profile file name"
                            size="iconXs"
                            class="opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100 motion-reduce:transition-none"
                        />
                    </li>
                </ul>
                <p
                    v-else-if="kit.appStoreConnectConfigured"
                    class="mt-1 text-[11px] text-zinc-500 dark:text-zinc-400"
                >
                    None yet. The App Store profile is created at Apple when a machine provisions,
                    and a device profile when a phone is prepared; both are kept here.
                </p>
                <p v-else class="mt-1 text-[11px] text-amber-700 dark:text-amber-400">
                    None stored. A signed archive needs an App Store profile for the project's exact
                    bundle identifier; a Team key would create one.
                </p>
            </div>

            <div class="mt-3 border-t border-zinc-200 pt-3 dark:border-zinc-800">
                <p class="text-[11px] text-zinc-500 dark:text-zinc-400">Attached machines</p>
                <div v-if="kit.attachedMachines.length" class="mt-1.5 flex flex-wrap gap-1.5">
                    <Chip
                        v-for="name in kit.attachedMachines"
                        :key="name"
                        :interactive="false"
                        dot="bg-emerald-500"
                    >
                        {{ name }}
                    </Chip>
                </div>
                <p v-else class="mt-1 text-[11px] text-zinc-500 dark:text-zinc-400">
                    Not attached to any machine yet. Open a machine and attach this kit at its
                    Attach a signing kit step.
                </p>
            </div>
        </Card>

        <Callout v-if="signing.kits.value.length && !anyMachines" tone="neutral">
            Create a macOS machine to attach a kit to.
        </Callout>

        <AppleVerificationCard v-if="signing.kits.value.length && anyMachines" />

        <Card tone="well">
            <template #title>
                <span class="flex items-center gap-2">
                    <Apple class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" />
                    Signing in through Xcode instead
                </span>
            </template>
            <template #description> Optional, and not needed by anything above. </template>
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                You can open Xcode inside a machine, sign in under Settings, then Accounts, and let
                it manage certificates. Apple often rejects account sign-in inside a virtual
                machine, so treat it as best effort: cancel rather than retrying a generic
                verification failure. BuildBridge never collects an Apple Account password or a
                two-factor code, so it cannot verify this route or use it for a signed build.
            </p>
        </Card>

        <SigningKitDialog v-model:open="dialogOpen" :kit="editing" />

        <ConfirmDialog
            :open="certifying !== null"
            :title="
                certifying?.kind === 'development'
                    ? 'Create a Development certificate at Apple'
                    : 'Create a Distribution certificate at Apple'
            "
            confirm-label="Create certificate"
            :destructive="false"
            @update:open="(value) => (certifying = value ? certifying : null)"
            @confirm="createCertificate"
        >
            <p>
                BuildBridge generates a private key on this host, asks Apple to sign it with the
                Team key in <b>{{ certifying?.kit.name }}</b
                >, and stores the result in the kit as a password-protected .p12. No Mac is involved
                and nothing at Apple is revoked.
            </p>
            <p v-if="certifying?.kind === 'development'">
                A development identity signs only Debug builds installed on iPhones registered with
                the team; the distribution identity in the kit is untouched. The key needs the
                <b>Admin</b> role at Apple, and Apple allows only a few active development
                certificates per team; if it refuses, its reason is shown as is.
            </p>
            <p v-else>
                The key needs the <b>Admin</b> role at Apple, and Apple allows only a few active
                distribution certificates per team; if it refuses, its reason is shown as is. The
                private key then exists only in this kit, so keep this host's vault backed up.
            </p>
        </ConfirmDialog>

        <ConfirmDialog
            :open="removing !== null"
            title="Remove this signing kit"
            confirm-label="Remove kit"
            :busy="signing.state.deleting"
            @update:open="(value) => (removing = value ? removing : null)"
            @confirm="remove"
        >
            <p>
                <b>{{ removing?.name }}</b> is deleted from the operating-system vault: the
                certificate path and password, the profile paths, the guest keychain password, and
                any Team API key.
            </p>
            <p>
                <span class="inline-flex items-center gap-1 font-medium">
                    <TriangleAlert class="h-3.5 w-3.5" />
                    {{ removing?.attachedMachines.length ?? 0 }} machine{{
                        (removing?.attachedMachines.length ?? 0) === 1 ? '' : 's'
                    }}
                </span>
                using this kit will be left unattached and cannot provision signing until another
                kit is attached.
            </p>
            <p>
                Nothing at Apple is revoked, and signing already provisioned inside a machine stays
                there until you remove it from that machine.
            </p>
        </ConfirmDialog>
    </div>
</template>
