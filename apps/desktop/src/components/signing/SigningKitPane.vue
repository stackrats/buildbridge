<script setup lang="ts">
import {
    BadgePlus,
    Eye,
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
import {
    androidKitShortfall,
    kitReadiness,
    kitShortfall,
    signingKitPlatforms,
} from '../../model/signing';
import Input from '../ui/Input.vue';
import Field from '../ui/Field.vue';
import { useSigningStore } from '../../stores/signing';
import type { MachinePlatform, SigningKitSummary } from '../../types/backend';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import CopyButton from '../ui/CopyButton.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import Chip from '../ui/Chip.vue';
import EmptyState from '../ui/EmptyState.vue';
import KeyValue, { type KeyValueItem } from '../ui/KeyValue.vue';
import Spinner from '../ui/Spinner.vue';
import PlatformIcon from '../ui/PlatformIcon.vue';
import Tabs from '../ui/Tabs.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import ConfirmDialog from '../dialogs/ConfirmDialog.vue';
import AppleVerificationCard from './AppleVerificationCard.vue';
import AndroidVerificationCard from './AndroidVerificationCard.vue';
import SigningKitDialog from './SigningKitDialog.vue';
import SigningCredentialsDialog from './SigningCredentialsDialog.vue';

const signing = useSigningStore();
const machines = useMachinesStore();
const platform = ref<'all' | MachinePlatform>('all');
function matchesPlatform(kit: SigningKitSummary, filter: 'all' | MachinePlatform): boolean {
    const platforms = signingKitPlatforms(kit);
    return filter === 'all' || platforms.length === 0 || platforms.includes(filter);
}
const platformTabs = computed(() =>
    (['all', 'ios', 'android'] as const).map((value) => ({
        value,
        label: value === 'all' ? 'All' : value === 'ios' ? 'iOS' : 'Android',
        badge: signing.kits.value.filter((kit) => matchesPlatform(kit, value)).length,
    })),
);
const visibleKits = computed(() =>
    signing.kits.value.filter((kit) => matchesPlatform(kit, platform.value)),
);

// `?kitDialog=1` opens the form straight away and `?kitDialog=edit` opens the first stored kit,
// so the browser preview can show either state of it.
const previewDialog =
    import.meta.env.DEV && typeof location !== 'undefined'
        ? new URLSearchParams(location.search).get('kitDialog')
        : null;
const dialogOpen = ref(previewDialog !== null);
const editing = ref<SigningKitSummary | null>(null);
const reviewing = ref<SigningKitSummary | null>(null);

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
        signing.state.creatingDevelopmentCertificateKitId !== null ||
        signing.state.creatingKeystoreKitId !== null,
);

// Creating an Android upload key: the password is the person's to keep, typed or invented here
// and shown so it can be written down; either way it goes into the vault for signing.
const keystoreFor = ref<SigningKitSummary | null>(null);
const keystoreForm = ref({ password: '', confirm: '', alias: 'upload', name: '' });
const keystorePasswordShown = ref(false);

function generateKeystorePassword(): void {
    const alphabet = 'abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789';
    const bytes = new Uint8Array(20);
    crypto.getRandomValues(bytes);
    const password = Array.from(bytes, (byte) => alphabet[byte % alphabet.length]).join('');
    keystoreForm.value.password = password;
    keystoreForm.value.confirm = password;
    keystorePasswordShown.value = true;
}
const keystoreProblem = computed(() => {
    if (keystoreForm.value.password.length < 6) {
        return 'Choose a password of at least six characters.';
    }
    if (
        !keystorePasswordShown.value &&
        keystoreForm.value.password !== keystoreForm.value.confirm
    ) {
        return 'The two passwords differ.';
    }
    if (!/^[A-Za-z0-9._-]{1,64}$/.test(keystoreForm.value.alias)) {
        return 'The alias may only contain letters, digits, dots, underscores and dashes.';
    }
    return null;
});

function openKeystoreDialog(kit: SigningKitSummary): void {
    keystoreForm.value = { password: '', confirm: '', alias: 'upload', name: kit.name };
    keystorePasswordShown.value = false;
    keystoreFor.value = kit;
}

async function createKeystore(): Promise<void> {
    const kit = keystoreFor.value;
    if (!kit || keystoreProblem.value) {
        return;
    }
    keystoreFor.value = null;
    await signing.createKeystore(kit.id, {
        password: keystoreForm.value.password,
        keyAlias: keystoreForm.value.alias,
        certificateName: keystoreForm.value.name,
    });
    keystoreForm.value = { password: '', confirm: '', alias: 'upload', name: '' };
}

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
function hasAppleMaterial(kit: SigningKitSummary): boolean {
    return signingKitPlatforms(kit).includes('ios');
}

function detailsFor(kit: SigningKitSummary) {
    const readiness = kitReadiness(kit);
    const platforms = signingKitPlatforms(kit);
    const items: KeyValueItem[] = [];
    if (platform.value !== 'android' && platforms.includes('ios')) {
        items.push(
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
                    (readiness.teamKey
                        ? 'Created at Apple when a machine provisions'
                        : 'Not stored'),
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
        );
        if (kit.developmentCertificateConfigured || kit.developmentCertificatePasswordStored) {
            items.push({
                label: 'Development export password',
                value: kit.developmentCertificatePasswordStored
                    ? 'Stored in the OS vault'
                    : 'Not stored',
                tone: kit.developmentCertificatePasswordStored ? 'default' : 'warn',
            });
        }
    }
    if (platform.value !== 'ios' && platforms.includes('android')) {
        items.push(
            {
                label: 'Android upload keystore',
                value:
                    kit.androidKeystoreName ??
                    (kit.androidKeystoreConfigured ? 'Stored on this host' : 'Not stored'),
                mono: kit.androidKeystoreName !== null,
                tone: kit.androidKeystoreConfigured ? 'default' : 'warn',
            },
            {
                label: 'Key alias',
                value: kit.androidKeyAlias ?? 'Not stored',
                mono: kit.androidKeyAlias !== null,
                tone: kit.androidKeyAlias ? 'default' : 'warn',
            },
            {
                label: 'Keystore password',
                value: kit.androidKeystorePasswordStored ? 'Stored in the OS vault' : 'Not stored',
                tone: kit.androidKeystorePasswordStored ? 'default' : 'warn',
            },
            {
                label: 'Key password',
                value: kit.androidKeyPasswordStored
                    ? 'Stored in the OS vault'
                    : 'Same as the keystore password',
            },
        );
    }
    items.push({
        label: 'Added',
        value: formatDate(new Date(kit.createdAtEpochSeconds * 1000).toISOString()),
    });
    return items;
}

function incompleteMessage(kit: SigningKitSummary): string | null {
    const platforms = signingKitPlatforms(kit);
    if (!platforms.length) {
        return platform.value === 'android'
            ? 'Add an Android upload key to use these credentials for a release.'
            : platform.value === 'ios'
              ? 'Add a Team key or iOS signing files to use these credentials.'
              : 'Add iOS signing material or an Android upload key to use these credentials.';
    }
    const readiness = kitReadiness(kit);
    const missing: string[] = [];
    if (platform.value !== 'android' && platforms.includes('ios') && !readiness.provisionable) {
        missing.push(`iOS needs ${kitShortfall(kit).join(' and ')}`);
    }
    if (platform.value !== 'ios' && platforms.includes('android') && !readiness.android) {
        missing.push(`Android needs ${androidKitShortfall(kit).join(' and ')}`);
    }
    return missing.length ? `${missing.join('. ')}. Edit the credentials to finish setup.` : null;
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
                <h1 class="text-lg font-bold text-zinc-900 dark:text-zinc-50">Signing</h1>
                <p class="mt-1 max-w-2xl text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    Store credentials once per team or app and reuse them across machines. Use a
                    Team key or existing files for iOS, and an upload key for Android. Passwords
                    stay in this host's operating-system vault.
                </p>
            </div>
            <Button size="sm" @click="createKit">
                <Plus class="h-3.5 w-3.5" />
                New credentials
            </Button>
        </header>

        <Tabs v-model="platform" :tabs="platformTabs" aria-label="Signing platform" />
        <p v-if="platform !== 'all'" class="text-xs text-zinc-500 dark:text-zinc-400">
            Shared credentials appear under both platforms. Empty credentials stay visible until you
            add signing material.
        </p>

        <Callout
            v-if="platform !== 'android' && orphaned.length"
            tone="danger"
            title="The credentials these machines were provisioned from are no longer stored"
        >
            <p>
                {{ orphaned.map((machine) => machine.config.name).join(', ') }}
                {{ orphaned.length === 1 ? 'has' : 'have' }} a provisioned guest keychain, but no
                credentials here match. An operating-system keyring reset does exactly this. Store
                the credentials again below, attach them at the machine's Attach signing credentials
                step, then provision once more.
            </p>
        </Callout>

        <template v-if="signing.state.messageKitId === null">
            <Callout v-if="signing.state.error" tone="danger">{{ signing.state.error }}</Callout>
            <Callout v-else-if="signing.state.notice" tone="ok">{{ signing.state.notice }}</Callout>
        </template>

        <EmptyState
            v-if="!signing.kits.value.length && !signing.state.loading"
            title="No signing credentials stored"
            description="Choose Apple or Android when adding credentials. For iOS, a Team key is enough and BuildBridge generates the guest keychain password. For Android, import an upload key or create one after saving."
        >
            <template #icon><KeyRound class="h-4 w-4" /></template>
            <Button size="sm" @click="createKit">
                <Plus class="h-3.5 w-3.5" />
                Store credentials
            </Button>
        </EmptyState>

        <EmptyState
            v-if="signing.kits.value.length && !visibleKits.length && !signing.state.loading"
            :title="
                platform === 'android' ? 'No Android credentials yet' : 'No iOS credentials yet'
            "
            description="Add credentials for this platform, or edit an existing set from All to use it for both."
        >
            <template #icon>
                <PlatformIcon v-if="platform !== 'all'" :platform="platform" class="h-4 w-4" />
            </template>
            <Button size="sm" @click="createKit">
                <Plus class="h-3.5 w-3.5" />
                Store credentials
            </Button>
        </EmptyState>

        <Card v-for="kit in visibleKits" :key="kit.id">
            <template #title>
                <span class="flex flex-wrap items-center gap-2">
                    <PlatformIcon
                        v-for="kitPlatform in signingKitPlatforms(kit)"
                        :key="kitPlatform"
                        :platform="kitPlatform"
                        class="h-4 w-4"
                    />
                    {{ kit.name }}
                    <Badge v-if="!signingKitPlatforms(kit).length"
                        >No platform credentials yet</Badge
                    >
                    <Badge
                        v-if="
                            platform !== 'android' &&
                            kitReadiness(kit).provisionable &&
                            kitReadiness(kit).archive
                        "
                        tone="ok"
                        >iOS release ready</Badge
                    >
                    <Badge v-if="incompleteMessage(kit)" tone="warn">Incomplete</Badge>
                    <Badge v-if="platform !== 'ios' && kitReadiness(kit).android"
                        >Android configured</Badge
                    >
                    <Badge
                        v-if="
                            platform !== 'android' &&
                            kitReadiness(kit).provisionable &&
                            kitReadiness(kit).archive === null
                        "
                        tone="warn"
                        >Phone only</Badge
                    >
                </span>
            </template>
            <template #actions>
                <Button
                    v-if="
                        platform !== 'ios' &&
                        !kit.androidKeystoreConfigured &&
                        !hasAppleMaterial(kit)
                    "
                    variant="outline"
                    size="sm"
                    title="A 2048-bit RSA upload key in a PKCS12 keystore, created in a container of the toolchain image and kept owner-only on this host"
                    :disabled="creatingAny"
                    @click="openKeystoreDialog(kit)"
                >
                    <Spinner v-if="signing.state.creatingKeystoreKitId === kit.id" />
                    <Smartphone v-else class="h-3.5 w-3.5" />
                    Create Android upload key
                </Button>
                <Button variant="outline" size="sm" @click="reviewing = kit">
                    <Eye class="h-3.5 w-3.5" />
                    Review credentials
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

            <!-- A message about these credentials sits beside the button that produced it. -->
            <template v-if="signing.state.messageKitId === kit.id">
                <Callout v-if="signing.state.error" tone="danger" class="mb-3">
                    {{ signing.state.error }}
                </Callout>
                <Callout v-else-if="signing.state.notice" tone="ok" class="mb-3">
                    {{ signing.state.notice }}
                </Callout>
            </template>

            <AndroidVerificationCard
                v-if="platform !== 'ios' && kitReadiness(kit).android"
                :kit="kit"
                class="mb-3"
            />

            <details class="group rounded-md border border-zinc-200 p-3 dark:border-zinc-800">
                <DisclosureSummary
                    class="cursor-pointer text-xs font-medium text-zinc-700 dark:text-zinc-200"
                >
                    Credential details
                </DisclosureSummary>
                <KeyValue class="mt-3" :items="detailsFor(kit)" :columns="3" />
            </details>
            <p
                v-if="incompleteMessage(kit)"
                class="mt-2 text-[11px] leading-4 text-amber-700 dark:text-amber-400"
            >
                {{ incompleteMessage(kit) }}
            </p>

            <p
                v-else-if="
                    platform !== 'android' &&
                    kitReadiness(kit).provisionable &&
                    kitReadiness(kit).archive === null
                "
                class="mt-2 text-[11px] leading-4 text-amber-700 dark:text-amber-400"
            >
                These credentials can run Debug builds on a phone, but cannot sign an App Store
                archive: they hold no distribution identity and no Team key to create one. Edit them
                to add either.
            </p>

            <details
                v-if="platform !== 'android' && hasAppleMaterial(kit)"
                class="mt-3 border-t border-zinc-200 pt-3 dark:border-zinc-800"
            >
                <DisclosureSummary
                    class="cursor-pointer text-xs font-medium text-zinc-700 dark:text-zinc-200"
                >
                    Profiles and optional signing actions
                </DisclosureSummary>
                <div class="my-3 flex flex-wrap gap-2">
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
                        v-if="
                            kit.appStoreConnectConfigured && !kit.developmentCertificateConfigured
                        "
                        variant="outline"
                        size="sm"
                        title="Not required: preparing a phone creates it when needed"
                        :disabled="creatingAny"
                        @click="certifying = { kit, kind: 'development' }"
                    >
                        <Spinner
                            v-if="signing.state.creatingDevelopmentCertificateKitId === kit.id"
                        />
                        <Smartphone v-else class="h-3.5 w-3.5" />
                        Create development certificate now
                    </Button>
                    <Button
                        v-if="platform !== 'ios' && !kit.androidKeystoreConfigured"
                        variant="outline"
                        size="sm"
                        :disabled="creatingAny"
                        @click="openKeystoreDialog(kit)"
                    >
                        <Spinner v-if="signing.state.creatingKeystoreKitId === kit.id" />
                        <Smartphone v-else class="h-3.5 w-3.5" />
                        Add Android upload key
                    </Button>
                </div>
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
                            class="opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
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
            </details>

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
                    Not attached to any machine yet. Open a machine and attach these credentials at
                    its Attach signing credentials step.
                </p>
            </div>
        </Card>

        <Callout v-if="signing.kits.value.length && !anyMachines" tone="neutral">
            Create a machine to attach credentials to.
        </Callout>

        <ConfirmDialog
            :open="keystoreFor !== null"
            title="Create an Android upload key"
            confirm-label="Create upload key"
            :destructive="false"
            :confirm-disabled="keystoreProblem !== null"
            @update:open="(value) => (keystoreFor = value ? keystoreFor : null)"
            @confirm="createKeystore"
        >
            <p>
                A 2048-bit RSA key in a PKCS12 keystore, valid for 27 years, created by
                <span class="font-mono">keytool</span> in a container of the toolchain image and
                stored owner-only on this host in <b>{{ keystoreFor?.name }}</b
                >. Google Play takes it as the upload key of a new app.
            </p>
            <p>
                The password goes into the vault for BuildBridge to sign with, but neither Google
                Play nor BuildBridge can recover an upload key whose password is lost, so keep a
                copy and back the keystore up. Let BuildBridge invent one and copy it, or type your
                own.
            </p>
            <div class="grid gap-3 sm:grid-cols-2">
                <Field label="Keystore password" required>
                    <Input
                        v-model="keystoreForm.password"
                        :type="keystorePasswordShown ? 'text' : 'password'"
                        :mono="keystorePasswordShown"
                        autocomplete="new-password"
                    />
                    <template #action>
                        <Button
                            variant="outline"
                            title="Fills both fields with a random password and shows it"
                            @click="generateKeystorePassword"
                        >
                            Generate password
                        </Button>
                    </template>
                </Field>
                <Field
                    :label="keystorePasswordShown ? 'Copy it somewhere safe' : 'Confirm password'"
                    :required="!keystorePasswordShown"
                >
                    <Input
                        v-if="!keystorePasswordShown"
                        v-model="keystoreForm.confirm"
                        type="password"
                        autocomplete="new-password"
                    />
                    <div v-else class="flex h-full items-center">
                        <CopyButton :text="keystoreForm.password" label="Copy password" />
                    </div>
                </Field>
                <Field label="Key alias" hint="The key's name inside the keystore.">
                    <Input v-model="keystoreForm.alias" mono />
                </Field>
                <Field label="Certificate name" hint="Goes into the certificate as its subject.">
                    <Input v-model="keystoreForm.name" :maxlength="64" />
                </Field>
            </div>
            <p
                v-if="keystoreProblem && (keystoreForm.password || keystoreForm.confirm)"
                class="text-[11px] text-amber-700 dark:text-amber-400"
            >
                {{ keystoreProblem }}
            </p>
        </ConfirmDialog>

        <AppleVerificationCard
            v-if="platform !== 'android' && signing.kits.value.length && anyMachines"
        />

        <details
            v-if="platform !== 'android'"
            class="rounded-lg border border-zinc-200 bg-zinc-50 p-4 dark:border-zinc-800 dark:bg-zinc-950"
        >
            <DisclosureSummary
                class="cursor-pointer text-xs font-medium text-zinc-700 dark:text-zinc-200"
            >
                Optional: signing in through Xcode
            </DisclosureSummary>
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                You can open Xcode inside a machine, sign in under Settings, then Accounts, and let
                it manage certificates. Apple often rejects account sign-in inside a virtual
                machine, so treat it as best effort: cancel rather than retrying a generic
                verification failure. BuildBridge never collects an Apple Account password or a
                two-factor code, so it cannot verify this route or use it for a signed build.
            </p>
        </details>

        <SigningKitDialog
            v-model:open="dialogOpen"
            :kit="editing"
            :default-platform="platform === 'android' ? 'android' : 'ios'"
        />

        <SigningCredentialsDialog
            v-if="reviewing"
            :open="true"
            :kit="reviewing"
            @update:open="
                (value) => {
                    if (!value) reviewing = null;
                }
            "
        />

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
                >, and stores the result in the credentials as a password-protected .p12. No Mac is
                involved and nothing at Apple is revoked.
            </p>
            <p v-if="certifying?.kind === 'development'">
                A development identity signs only Debug builds installed on iPhones registered with
                the team; the distribution identity in the credentials is untouched. The key needs
                the
                <b>Admin</b> role at Apple, and Apple allows only a few active development
                certificates per team; if it refuses, its reason is shown as is.
            </p>
            <p v-else>
                The key needs the <b>Admin</b> role at Apple, and Apple allows only a few active
                distribution certificates per team; if it refuses, its reason is shown as is. The
                private key then exists only in these credentials, so keep this host's vault backed
                up.
            </p>
        </ConfirmDialog>

        <ConfirmDialog
            :open="removing !== null"
            title="Remove these signing credentials"
            confirm-label="Remove credentials"
            :busy="signing.state.deleting"
            @update:open="(value) => (removing = value ? removing : null)"
            @confirm="remove"
        >
            <p>
                <b>{{ removing?.name }}</b> is deleted from the operating-system vault: the
                certificate path and password, the profile paths, the guest keychain password, any
                Team API key, and the Android upload key's path, alias and passwords. An upload key
                BuildBridge created for them is deleted from this host as well.
            </p>
            <p>
                <span class="inline-flex items-center gap-1 font-medium">
                    <TriangleAlert class="h-3.5 w-3.5" />
                    {{ removing?.attachedMachines.length ?? 0 }} machine{{
                        (removing?.attachedMachines.length ?? 0) === 1 ? '' : 's'
                    }}
                </span>
                using these credentials will be left unattached and cannot provision signing until
                others are attached.
            </p>
            <p>
                Nothing at Apple is revoked, and signing already provisioned inside a machine stays
                there until you remove it from that machine.
            </p>
        </ConfirmDialog>
    </div>
</template>
