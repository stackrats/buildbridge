<script setup lang="ts">
// Create or update one signing kit.
//
// The simplest kit is a name and one of two routes to Apple's signing material: a Team key, from
// which buildbridge creates the certificates and profiles at Apple when a machine first needs
// them, or the files exported from a Mac. Both can be stored; files are used where they exist and
// the key creates the rest. The guest keychain password is invented when left blank, since nothing
// but buildbridge ever asks for it. The form says at the bottom exactly what the kit will be able
// to do if saved now.
import { CircleCheck, Eye, Lock, Plus } from '@lucide/vue';
import { computed, reactive, ref, useId, watch } from 'vue';

import { useBackend } from '../../lib/backend';
import type { ListboxOption } from '../../lib/listbox';
import { mergePathList } from '../../lib/paths';
import {
    appStoreConnectIsPartial,
    developmentIdentityStatus,
    draftKitSummary,
    kitReadiness,
    kitShortfall,
    missingKitRequirements,
    signingKitPlatforms,
} from '../../model/signing';
import { useSigningStore } from '../../stores/signing';
import type { MachinePlatform, ManagedAppleProfile, SigningKitSummary } from '../../types/backend';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import CopyButton from '../ui/CopyButton.vue';
import Field from '../ui/Field.vue';
import Input from '../ui/Input.vue';
import Modal from '../ui/Modal.vue';
import PathField from '../ui/PathField.vue';
import PathListField from '../ui/PathListField.vue';
import Select from '../ui/Select.vue';
import Spinner from '../ui/Spinner.vue';
import SigningCredentialsDialog from './SigningCredentialsDialog.vue';

const open = defineModel<boolean>('open', { default: false });
const { kit = null, defaultPlatform = 'ios' } = defineProps<{
    kit?: SigningKitSummary | null;
    defaultPlatform?: MachinePlatform;
}>();

const signing = useSigningStore();
const reviewing = ref(false);
watch(
    () => [open.value, kit?.id],
    () => (reviewing.value = false),
    { flush: 'sync' },
);
const formId = useId();
const platform = ref('ios');
const platformOptions: ListboxOption[] = [
    {
        value: 'ios',
        label: 'Apple (iOS)',
        platforms: ['ios'],
        description: 'Team key or existing signing files',
    },
    {
        value: 'android',
        label: 'Android',
        platforms: ['android'],
        description: 'Upload keystore for Google Play and signed APKs',
    },
    {
        value: 'both',
        label: 'Apple and Android',
        platforms: ['ios', 'android'],
        description: 'Keep both platforms in one set of credentials',
    },
];
const showApple = computed(() => platform.value !== 'android');

const APPLE_CERTIFICATES_URL = 'https://developer.apple.com/account/resources/certificates/list';
const APPLE_PROFILES_URL = 'https://developer.apple.com/account/resources/profiles/list';
const APP_STORE_CONNECT_KEYS_URL = 'https://appstoreconnect.apple.com/access/integrations/api';

const form = reactive({
    name: '',
    signingCertificatePath: '',
    signingCertificatePassword: '',
    provisioningProfilePaths: '',
    guestKeychainPassword: '',
    appStoreConnectKeyId: '',
    appStoreConnectIssuerId: '',
    appStoreConnectPrivateKeyPath: '',
    developmentCertificatePath: '',
    developmentCertificatePassword: '',
    androidKeystorePath: '',
    androidKeystorePassword: '',
    androidKeyAlias: '',
    androidKeyPassword: '',
});

const editing = computed(() => kit !== null);
// A new kit opens on the Team key route; an edited kit opens whichever routes it holds.
const showTeamKey = ref(true);
const showFiles = ref(false);
const showAndroid = ref(false);
watch(platform, (value) => {
    if (value !== 'ios') {
        showAndroid.value = true;
    }
});
const developmentStatus = computed(() => developmentIdentityStatus(form, kit));

/** A section's open state follows its disclosure, so the seeding above can still set it. */
function disclosureOpen(event: Event): boolean {
    return (event.currentTarget as HTMLDetailsElement).open;
}

// Every profile buildbridge downloads is kept on this host. A vault that loses its paths — a
// cleared keyring, a new machine profile — does not lose those files, so they are offered back
// here rather than making anyone find them or fetch them from Apple again.
const managedProfiles = ref<ManagedAppleProfile[]>([]);
const offeredProfiles = computed(() =>
    managedProfiles.value.filter(
        (profile) =>
            !profileLines.value.includes(profile.path) &&
            !(kit?.provisioningProfileNames ?? []).includes(profile.fileName),
    ),
);

function useManagedProfile(profile: ManagedAppleProfile): void {
    form.provisioningProfilePaths = mergePathList(form.provisioningProfilePaths, [profile.path]);
}

const profileLines = computed(() =>
    form.provisioningProfilePaths
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean),
);

const draft = computed(() => ({
    certificatePath: form.signingCertificatePath,
    certificatePassword: form.signingCertificatePassword,
    profilePaths: profileLines.value,
    keychainPassword: form.guestKeychainPassword,
    developmentCertificatePath: form.developmentCertificatePath,
    developmentCertificatePassword: form.developmentCertificatePassword,
    teamKeyPath: form.appStoreConnectPrivateKeyPath,
    teamKeyId: form.appStoreConnectKeyId,
    teamIssuerId: form.appStoreConnectIssuerId,
    androidKeystorePath: form.androidKeystorePath,
    androidKeystorePassword: form.androidKeystorePassword,
    androidKeyAlias: form.androidKeyAlias,
    androidKeyPassword: form.androidKeyPassword,
}));
// The kit as it would be stored if saved now, and what that kit could do.
const wouldStore = computed(() => draftKitSummary(draft.value, kit));
const readiness = computed(() => kitReadiness(wouldStore.value));
const shortfall = computed(() => kitShortfall(wouldStore.value));
const missingFiles = computed(() => missingKitRequirements(draft.value, kit));
const filesStarted = computed(() => missingFiles.value.length < 3);

const partialAppStoreConnect = computed(() =>
    appStoreConnectIsPartial({
        keyPath: form.appStoreConnectPrivateKeyPath,
        keyId: form.appStoreConnectKeyId,
        issuerId: form.appStoreConnectIssuerId,
    }),
);

watch(
    open,
    async (value) => {
        if (!value) {
            return;
        }
        try {
            managedProfiles.value = await useBackend().listManagedAppleProfiles();
        } catch {
            // Nothing to offer is a normal state; the field still takes a typed or browsed path.
            managedProfiles.value = [];
        }
    },
    { immediate: true },
);

// Re-seeds whenever the dialog opens, or when it is pointed at a different kit while open.
watch([open, () => kit], ([value]) => {
    if (value) {
        form.name = kit?.name ?? '';
        form.signingCertificatePath = '';
        form.signingCertificatePassword = '';
        form.provisioningProfilePaths = '';
        form.guestKeychainPassword = '';
        form.appStoreConnectKeyId = '';
        form.appStoreConnectIssuerId = '';
        form.appStoreConnectPrivateKeyPath = '';
        form.developmentCertificatePath = '';
        form.developmentCertificatePassword = '';
        form.androidKeystorePath = '';
        form.androidKeystorePassword = '';
        form.androidKeyAlias = '';
        form.androidKeyPassword = '';
        const holdsFiles =
            (kit?.signingCertificateConfigured ?? false) ||
            (kit?.developmentCertificateConfigured ?? false) ||
            (kit?.provisioningProfileNames.length ?? 0) > 0;
        const storedPlatforms = kit ? signingKitPlatforms(kit) : [];
        platform.value = kit
            ? storedPlatforms.includes('android')
                ? storedPlatforms.includes('ios')
                    ? 'both'
                    : 'android'
                : 'ios'
            : defaultPlatform;
        showAndroid.value = platform.value !== 'ios';
        showTeamKey.value = kit ? kit.appStoreConnectConfigured || !holdsFiles : true;
        showFiles.value = holdsFiles;
        signing.clearMessages();
    }
});

async function save(): Promise<void> {
    const saved = await signing.save({
        kitId: kit?.id ?? null,
        name: form.name,
        appStoreConnectKeyId: form.appStoreConnectKeyId.trim(),
        appStoreConnectIssuerId: form.appStoreConnectIssuerId.trim(),
        appStoreConnectPrivateKeyPath: form.appStoreConnectPrivateKeyPath.trim(),
        signingCertificatePath: form.signingCertificatePath.trim(),
        signingCertificatePassword: form.signingCertificatePassword,
        provisioningProfilePaths: profileLines.value,
        guestKeychainPassword: form.guestKeychainPassword,
        developmentCertificatePath: form.developmentCertificatePath.trim(),
        developmentCertificatePassword: form.developmentCertificatePassword,
        androidKeystorePath: form.androidKeystorePath.trim(),
        androidKeystorePassword: form.androidKeystorePassword,
        androidKeyAlias: form.androidKeyAlias.trim(),
        androidKeyPassword: form.androidKeyPassword,
    });
    if (saved) {
        open.value = false;
    }
}
</script>

<template>
    <Modal
        v-model:open="open"
        :title="editing ? 'Edit signing credentials' : 'New signing credentials'"
        :busy="signing.state.saving"
        wide
    >
        <form :id="formId" class="space-y-4" @submit.prevent="save">
            <!-- The transparent border and p-3 mirror the sections below, so every box in the
                 form shares one left and right edge. -->
            <div class="space-y-3 border border-transparent px-3">
                <Field
                    label="Name"
                    required
                    hint="How you will recognise it when attaching a machine, for example the team or the app."
                >
                    <Input
                        v-model="form.name"
                        placeholder="A name for this team or app"
                        :maxlength="60"
                    />
                </Field>
                <Field
                    label="Platform"
                    hint="Choose the fields to show. Switching keeps anything already entered or stored."
                >
                    <Select v-model="platform" :options="platformOptions" />
                </Field>
            </div>

            <Callout v-if="editing" tone="neutral">
                <p>
                    Leave a field blank to keep what is already stored. Review saved values or
                    export files before making changes.
                </p>
                <Button variant="outline" size="sm" class="mt-2" @click="reviewing = true">
                    <Eye class="h-3.5 w-3.5" />
                    Review saved credentials
                </Button>
            </Callout>

            <p v-if="showApple" class="px-3 text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                A Team key is the simplest route. buildbridge creates signing files as needed and
                generates the guest keychain password for you. Existing files work too.
            </p>

            <details
                v-if="showApple"
                :open="showTeamKey"
                class="rounded-lg border border-zinc-200 p-3 dark:border-zinc-800"
                @toggle="showTeamKey = disclosureOpen($event)"
            >
                <DisclosureSummary>
                    <span class="min-w-0 flex-1">
                        <span
                            class="block text-[13px] font-semibold text-zinc-900 dark:text-zinc-50"
                        >
                            Team key
                            <span class="font-normal text-zinc-500 dark:text-zinc-400"
                                >· recommended</span
                            >
                        </span>
                        <span class="block text-[11px] text-zinc-500 dark:text-zinc-400">
                            An App Store Connect API key. buildbridge creates the distribution
                            certificate and App Store profile when a machine first provisions, and
                            the development identity and device profile when a phone is prepared. No
                            Mac needed.
                        </span>
                    </span>
                    <span
                        v-if="kit?.appStoreConnectKeyId"
                        class="shrink-0 font-mono text-[11px] text-emerald-700 dark:text-emerald-400"
                    >
                        {{ kit.appStoreConnectKeyId }}
                    </span>
                </DisclosureSummary>

                <div class="mt-3 space-y-3">
                    <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                        Created in App Store Connect under Users and Access, Integrations, App Store
                        Connect API, with the <b>Admin</b> role. Apple lets the private key be
                        downloaded once. All three parts are needed together.
                    </p>
                    <Field
                        label="Private key (.p8)"
                        :stored="kit?.appStoreConnectConfigured"
                        hint="Choose the key downloaded from Apple, then enter its Key ID and Issuer ID below. The key contents are stored in the vault."
                    >
                        <PathField
                            v-model="form.appStoreConnectPrivateKeyPath"
                            kind="file"
                            title="Choose the App Store Connect key"
                            :filter="{ name: 'App Store Connect key', extensions: ['p8'] }"
                            placeholder="/path/to/AuthKey_KEYID.p8"
                        />
                    </Field>
                    <div class="grid gap-3 sm:grid-cols-2">
                        <Field label="Key ID">
                            <Input
                                v-model="form.appStoreConnectKeyId"
                                mono
                                :placeholder="kit?.appStoreConnectKeyId ?? 'Ten characters'"
                            />
                        </Field>
                        <Field label="Issuer ID">
                            <Input
                                v-model="form.appStoreConnectIssuerId"
                                mono
                                placeholder="The issuer UUID"
                            />
                        </Field>
                    </div>
                    <p
                        v-if="partialAppStoreConnect"
                        class="text-[11px] text-amber-700 dark:text-amber-400"
                    >
                        Apple needs the key file, its ID and the issuer ID together. Fill all three,
                        or clear them to save without a key.
                    </p>
                    <CopyButton
                        :text="APP_STORE_CONNECT_KEYS_URL"
                        what="Copy the App Store Connect link"
                        size="iconSm"
                    />
                </div>
            </details>

            <details
                v-if="showApple"
                :open="showFiles"
                class="rounded-lg border border-zinc-200 p-3 dark:border-zinc-800"
                @toggle="showFiles = disclosureOpen($event)"
            >
                <DisclosureSummary>
                    <span class="min-w-0 flex-1">
                        <span
                            class="block text-[13px] font-semibold text-zinc-900 dark:text-zinc-50"
                        >
                            Files from a Mac
                            <span class="font-normal text-zinc-500 dark:text-zinc-400"
                                >· instead of, or alongside, a Team key</span
                            >
                        </span>
                        <span class="block text-[11px] text-zinc-500 dark:text-zinc-400">
                            The identity and profiles exported from Xcode and Keychain Access. Only
                            needed without a Team key, or to sign with certificates you already
                            hold.
                        </span>
                    </span>
                    <span
                        v-if="missingFiles.length === 0"
                        class="inline-flex shrink-0 items-center gap-1 text-[11px] text-emerald-700 dark:text-emerald-400"
                    >
                        <CircleCheck class="h-3.5 w-3.5" />
                        Complete
                    </span>
                    <span
                        v-else-if="filesStarted"
                        class="shrink-0 text-[11px] text-amber-700 tabular-nums dark:text-amber-400"
                    >
                        {{ 3 - missingFiles.length }}/3
                    </span>
                </DisclosureSummary>

                <div class="mt-3 space-y-3">
                    <details>
                        <DisclosureSummary quiet> Where do these come from? </DisclosureSummary>
                        <ol
                            class="mt-2 space-y-1.5 text-xs leading-5 text-zinc-600 dark:text-zinc-300"
                        >
                            <li class="flex gap-2">
                                <span
                                    class="shrink-0 font-semibold text-zinc-400 dark:text-zinc-500"
                                    >1</span
                                >
                                <span>
                                    In Xcode, open Settings, then Accounts, then Manage
                                    Certificates, create an <b>Apple Distribution</b> identity if
                                    you need one, then export it from Keychain Access as a
                                    password-protected <span class="font-mono">.p12</span>. The
                                    private key stays on the Mac that made it; a downloaded
                                    <span class="font-mono">.cer</span> cannot sign on its own.
                                </span>
                            </li>
                            <li class="flex gap-2">
                                <span
                                    class="shrink-0 font-semibold text-zinc-400 dark:text-zinc-500"
                                    >2</span
                                >
                                <span>
                                    In Certificates, Identifiers &amp; Profiles, create a
                                    Distribution <b>App Store Connect</b> profile for the exact
                                    bundle identifier and that certificate, then download its
                                    <span class="font-mono">.mobileprovision</span>.
                                </span>
                            </li>
                        </ol>
                        <div class="mt-2 flex flex-wrap gap-2">
                            <CopyButton
                                :text="APPLE_CERTIFICATES_URL"
                                what="Copy the certificates link"
                                size="iconSm"
                            />
                            <CopyButton
                                :text="APPLE_PROFILES_URL"
                                what="Copy the profiles link"
                                size="iconSm"
                            />
                        </div>
                    </details>

                    <Field
                        label="Distribution identity (.p12 or .pfx)"
                        :stored="kit?.signingCertificateConfigured"
                    >
                        <PathField
                            v-model="form.signingCertificatePath"
                            kind="file"
                            title="Choose the signing identity"
                            :filter="{ name: 'Signing identity', extensions: ['p12', 'pfx'] }"
                            :placeholder="
                                kit?.signingCertificateName ?? '/path/to/distribution.p12'
                            "
                        />
                    </Field>
                    <Field
                        label="Export password"
                        :stored="kit?.signingCertificatePasswordStored"
                        hint="The password set when the .p12 was exported."
                    >
                        <Input
                            v-model="form.signingCertificatePassword"
                            type="password"
                            autocomplete="off"
                        />
                    </Field>
                    <Field
                        label="Provisioning profiles (.mobileprovision)"
                        :stored="(kit?.provisioningProfileNames.length ?? 0) > 0"
                        :hint="
                            editing
                                ? 'Leave empty to keep the stored profiles. Anything listed replaces them.'
                                : 'An App Store profile for the project\'s exact bundle identifier.'
                        "
                    >
                        <PathListField
                            v-model="form.provisioningProfilePaths"
                            title="Choose provisioning profiles"
                            :filter="{
                                name: 'Provisioning profile',
                                extensions: ['mobileprovision'],
                            }"
                            :placeholder="
                                kit?.provisioningProfileNames.join('\n') ||
                                '/path/to/AppStore.mobileprovision'
                            "
                        />
                    </Field>
                    <div
                        v-if="offeredProfiles.length"
                        class="rounded-md bg-zinc-50 p-2 dark:bg-zinc-800/50"
                    >
                        <p class="text-[11px] text-zinc-600 dark:text-zinc-300">
                            Already on this host. buildbridge keeps a copy of every profile it
                            downloads, so one can be added back without going to Apple.
                        </p>
                        <div class="mt-1.5 flex flex-wrap gap-1.5">
                            <Button
                                v-for="profile in offeredProfiles"
                                :key="profile.path"
                                variant="outline"
                                size="sm"
                                class="max-w-full"
                                @click="useManagedProfile(profile)"
                            >
                                <Plus class="h-3.5 w-3.5 shrink-0" />
                                <span class="truncate font-mono text-[11px]">{{
                                    profile.fileName
                                }}</span>
                            </Button>
                        </div>
                    </div>

                    <div class="border-t border-zinc-200 pt-3 dark:border-zinc-800">
                        <p class="text-xs font-semibold text-zinc-900 dark:text-zinc-50">
                            Development identity
                            <span class="font-normal text-zinc-500 dark:text-zinc-400"
                                >· optional, for iPhone builds</span
                            >
                        </p>
                        <p class="mt-0.5 text-[11px] leading-4 text-zinc-500 dark:text-zinc-400">
                            Signs a Debug build for a registered iPhone. Store one only if you
                            already hold its .p12; a Team key creates one when a phone is prepared.
                        </p>
                    </div>
                    <Field
                        label="Development identity (.p12)"
                        :stored="kit?.developmentCertificateConfigured"
                        :hint="kit?.developmentCertificateName ?? undefined"
                    >
                        <PathField
                            v-model="form.developmentCertificatePath"
                            kind="file"
                            title="Choose the development identity"
                            :filter="{ name: 'PKCS#12 identity', extensions: ['p12', 'pfx'] }"
                        />
                    </Field>
                    <Field
                        label="Development identity export password"
                        :stored="kit?.developmentCertificatePasswordStored"
                    >
                        <Input
                            v-model="form.developmentCertificatePassword"
                            type="password"
                            autocomplete="off"
                        />
                    </Field>
                    <p
                        v-if="developmentStatus === 'partial'"
                        class="text-[11px] leading-4 text-amber-700 dark:text-amber-400"
                    >
                        A development identity needs both the .p12 and its export password.
                    </p>
                </div>
            </details>

            <details
                v-if="showApple"
                class="rounded-lg border border-zinc-200 p-3 dark:border-zinc-800"
            >
                <DisclosureSummary> Advanced: guest keychain password </DisclosureSummary>
                <Field
                    class="mt-3"
                    label="Guest keychain password"
                    :stored="kit?.guestKeychainConfigured"
                    hint="Optional. Leave blank to keep the stored password or generate one for new credentials. This protects buildbridge's signing keychain in macOS; it is not your Apple password."
                >
                    <Input
                        v-model="form.guestKeychainPassword"
                        type="password"
                        autocomplete="new-password"
                    />
                </Field>
            </details>

            <details
                v-if="platform !== 'ios'"
                :open="showAndroid"
                class="rounded-lg border border-zinc-200 p-3 dark:border-zinc-800"
                @toggle="showAndroid = disclosureOpen($event)"
            >
                <DisclosureSummary>
                    <span class="min-w-0 flex-1">
                        <span
                            class="block text-[13px] font-semibold text-zinc-900 dark:text-zinc-50"
                        >
                            Android upload key
                            <span class="font-normal text-zinc-500 dark:text-zinc-400"
                                >· for Android machines</span
                            >
                        </span>
                        <span class="block text-[11px] text-zinc-500 dark:text-zinc-400">
                            The keystore a signed app bundle and APK are signed with. Point these
                            credentials at one you already use for Google Play, or save them first
                            and create one from their card.
                        </span>
                    </span>
                    <span
                        v-if="kit?.androidKeystoreName"
                        class="shrink-0 font-mono text-[11px] text-emerald-700 dark:text-emerald-400"
                    >
                        {{ kit.androidKeystoreName }}
                    </span>
                </DisclosureSummary>

                <div class="mt-3 space-y-3">
                    <Field
                        label="Keystore (.jks, .keystore or .p12)"
                        :stored="kit?.androidKeystoreConfigured"
                        hint="Stored as a path; the file stays where it is. Keep the keystore and its password backed up."
                    >
                        <PathField
                            v-model="form.androidKeystorePath"
                            kind="file"
                            title="Choose the upload keystore"
                            :filter="{
                                name: 'Keystore',
                                extensions: ['jks', 'keystore', 'p12', 'pfx'],
                            }"
                            :placeholder="kit?.androidKeystoreName ?? '/path/to/upload.keystore'"
                        />
                    </Field>
                    <div class="grid gap-3 sm:grid-cols-2">
                        <Field
                            label="Key alias"
                            :stored="
                                kit?.androidKeyAlias !== null && kit?.androidKeyAlias !== undefined
                            "
                            hint="The name of the key inside the keystore."
                        >
                            <Input
                                v-model="form.androidKeyAlias"
                                mono
                                :placeholder="kit?.androidKeyAlias ?? 'upload'"
                            />
                        </Field>
                        <Field
                            label="Keystore password"
                            :stored="kit?.androidKeystorePasswordStored"
                        >
                            <Input
                                v-model="form.androidKeystorePassword"
                                type="password"
                                autocomplete="off"
                            />
                        </Field>
                    </div>
                    <Field
                        label="Key password"
                        :stored="kit?.androidKeyPasswordStored"
                        hint="Only when it differs from the keystore password; a PKCS12 keystore uses one password for both."
                    >
                        <Input
                            v-model="form.androidKeyPassword"
                            type="password"
                            autocomplete="off"
                        />
                    </Field>
                </div>
            </details>

            <!-- What the credentials will be able to do if saved now, in the reader's terms. -->
            <Callout :tone="readiness.provisionable || readiness.android ? 'ok' : 'warn'">
                <p v-if="readiness.android" class="font-semibold">
                    Android credentials complete. Save, then check the signing key.
                </p>
                <template v-if="readiness.provisionable">
                    <p class="font-semibold">These credentials can provision a machine.</p>
                    <ul class="mt-1 space-y-0.5">
                        <li>
                            App Store archives:
                            {{
                                readiness.archive === 'team_key'
                                    ? 'the distribution certificate and App Store profile are created at Apple during provisioning.'
                                    : readiness.archive === 'files'
                                      ? 'signed with the stored identity and profile.'
                                      : 'not possible with these credentials; add a Team key, or the distribution identity with its export password and profile.'
                            }}
                        </li>
                        <li>
                            Debug builds on an iPhone:
                            {{
                                readiness.phone === 'team_key'
                                    ? 'the development identity and device profile are created at Apple when the phone is prepared.'
                                    : readiness.phone === 'files'
                                      ? 'signed with the stored development identity.'
                                      : 'not possible with these credentials; add a Team key or a development identity.'
                            }}
                        </li>
                    </ul>
                </template>
                <p v-else-if="showApple" :class="readiness.android ? 'mt-1' : ''">
                    {{ readiness.android ? 'For iOS it' : 'Not usable yet: it' }} still needs
                    {{ shortfall.join(' and ') }}. Credentials can be saved now and finished later.
                </p>
                <p v-if="platform === 'android' && !readiness.android">
                    Add an existing keystore above, or save these credentials and choose
                    <b>Create Android upload key</b> on their card.
                </p>
            </Callout>
            <Callout v-if="signing.state.error" tone="danger">{{ signing.state.error }}</Callout>
        </form>

        <template #footer>
            <Button
                variant="outline"
                size="sm"
                :disabled="signing.state.saving"
                @click="open = false"
            >
                Cancel
            </Button>
            <Button
                type="submit"
                :form="formId"
                size="sm"
                :disabled="signing.state.saving || form.name.trim() === ''"
            >
                <Spinner v-if="signing.state.saving" tone="text-white dark:text-zinc-950" />
                <Lock v-else class="h-3.5 w-3.5" />
                {{ editing ? 'Save changes' : 'Store in the OS vault' }}
            </Button>
        </template>
    </Modal>
    <SigningCredentialsDialog v-if="open && kit && reviewing" v-model:open="reviewing" :kit="kit" />
</template>
