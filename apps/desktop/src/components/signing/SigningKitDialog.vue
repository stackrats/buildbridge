<script setup lang="ts">
// Create or update one signing kit.
//
// One thing is always needed — the guest keychain password, which nothing can create — and then
// one of two routes to Apple's signing material: a Team key, from which BuildBridge creates the
// certificates and profiles at Apple when a machine first needs them, or the files exported from a
// Mac. Both can be stored; files are used where they exist and the key creates the rest. The form
// says at the bottom exactly what the kit will be able to do if saved now.
import { ChevronRight, CircleCheck, Lock, Plus } from '@lucide/vue';
import { computed, reactive, ref, watch } from 'vue';

import { useBackend } from '../../lib/backend';
import { mergePathList } from '../../lib/paths';
import {
    appStoreConnectIsPartial,
    developmentIdentityStatus,
    draftKitSummary,
    kitReadiness,
    kitShortfall,
    missingKitRequirements,
} from '../../model/signing';
import { useSigningStore } from '../../stores/signing';
import type { ManagedAppleProfile, SigningKitSummary } from '../../types/backend';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import CopyButton from '../ui/CopyButton.vue';
import Field from '../ui/Field.vue';
import Input from '../ui/Input.vue';
import Modal from '../ui/Modal.vue';
import PathField from '../ui/PathField.vue';
import PathListField from '../ui/PathListField.vue';
import Spinner from '../ui/Spinner.vue';

const open = defineModel<boolean>('open', { default: false });
const { kit = null } = defineProps<{ kit?: SigningKitSummary | null }>();

const signing = useSigningStore();

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
});

const editing = computed(() => kit !== null);
// A new kit opens on the Team key route; an edited kit opens whichever routes it holds.
const showTeamKey = ref(true);
const showFiles = ref(false);
const developmentStatus = computed(() => developmentIdentityStatus(form, kit));

// Every profile BuildBridge downloads is kept on this host. A vault that loses its paths — a
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
        const holdsFiles =
            (kit?.signingCertificateConfigured ?? false) ||
            (kit?.developmentCertificateConfigured ?? false) ||
            (kit?.provisioningProfileNames.length ?? 0) > 0;
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
    });
    if (saved) {
        open.value = false;
    }
}
</script>

<template>
    <Modal v-model:open="open" :title="editing ? 'Edit signing kit' : 'New signing kit'" wide>
        <form class="space-y-4" @submit.prevent="save">
            <!-- The transparent border and p-3 mirror the sections below, so every box in the
                 form shares one left and right edge. -->
            <div class="space-y-3 border border-transparent px-3">
                <Field
                    label="Kit name"
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
                    label="Guest keychain password"
                    required
                    :stored="kit?.guestKeychainConfigured"
                    hint="Any new password you invent. BuildBridge creates a keychain inside each machine for this kit and locks it with this. Not an Apple password, and nothing else uses it."
                >
                    <Input
                        v-model="form.guestKeychainPassword"
                        type="password"
                        autocomplete="new-password"
                    />
                </Field>
            </div>

            <Callout v-if="editing" tone="neutral">
                Leave a field blank to keep what is already stored. Secret values are never shown
                again, so an empty box does not mean an empty vault.
            </Callout>

            <p class="px-3 text-[11px] leading-4 text-zinc-500 dark:text-zinc-400">
                Then one of the two routes below. A Team key alone is enough; files exported from a
                Mac work too, and a kit can hold both — files are used where they exist and the key
                creates the rest.
            </p>

            <section class="rounded-lg border border-zinc-200 p-3 dark:border-zinc-800">
                <button
                    type="button"
                    class="flex w-full items-center gap-2 text-left"
                    @click="showTeamKey = !showTeamKey"
                >
                    <ChevronRight
                        class="h-3.5 w-3.5 shrink-0 text-zinc-400 transition-transform"
                        :class="showTeamKey ? 'rotate-90' : ''"
                    />
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
                            An App Store Connect API key. BuildBridge creates the distribution
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
                </button>

                <div v-if="showTeamKey" class="mt-3 space-y-3">
                    <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                        Created in App Store Connect under Users and Access, Integrations, App Store
                        Connect API, with the <b>Admin</b> role. Apple lets the private key be
                        downloaded once. All three parts are needed together.
                    </p>
                    <Field
                        label="Private key (.p8)"
                        :stored="kit?.appStoreConnectConfigured"
                        hint="A conventional AuthKey_<KEY_ID>.p8 name fills the key ID in. Only the contents are stored, in the vault."
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
                        label="Copy App Store Connect link"
                    />
                </div>
            </section>

            <section class="rounded-lg border border-zinc-200 p-3 dark:border-zinc-800">
                <button
                    type="button"
                    class="flex w-full items-center gap-2 text-left"
                    @click="showFiles = !showFiles"
                >
                    <ChevronRight
                        class="h-3.5 w-3.5 shrink-0 text-zinc-400 transition-transform"
                        :class="showFiles ? 'rotate-90' : ''"
                    />
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
                        complete
                    </span>
                    <span
                        v-else-if="filesStarted"
                        class="shrink-0 text-[11px] text-amber-700 tabular-nums dark:text-amber-400"
                    >
                        {{ 3 - missingFiles.length }}/3
                    </span>
                </button>

                <div v-if="showFiles" class="mt-3 space-y-3">
                    <details class="group">
                        <summary
                            class="flex cursor-pointer list-none items-center gap-1 text-[11px] text-zinc-500 hover:text-zinc-700 dark:text-zinc-400 dark:hover:text-zinc-200"
                        >
                            <ChevronRight
                                class="h-3 w-3 transition-transform group-open:rotate-90"
                            />
                            Where do these come from?
                        </summary>
                        <ol
                            class="mt-2 space-y-1.5 text-xs leading-5 text-zinc-600 dark:text-zinc-300"
                        >
                            <li class="flex gap-2">
                                <span class="shrink-0 font-semibold text-zinc-400">1</span>
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
                                <span class="shrink-0 font-semibold text-zinc-400">2</span>
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
                                label="Copy certificates link"
                            />
                            <CopyButton :text="APPLE_PROFILES_URL" label="Copy profiles link" />
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
                            Already on this host. BuildBridge keeps a copy of every profile it
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
            </section>

            <!-- What the kit will be able to do if saved now, in the reader's terms. -->
            <div
                class="rounded-lg p-3 text-[11px] leading-4"
                :class="
                    readiness.provisionable
                        ? 'bg-emerald-50 text-emerald-800 dark:bg-emerald-950/40 dark:text-emerald-300'
                        : 'bg-amber-50 text-amber-800 dark:bg-amber-950/40 dark:text-amber-300'
                "
            >
                <template v-if="readiness.provisionable">
                    <p class="flex items-center gap-1 font-medium">
                        <CircleCheck class="h-3.5 w-3.5" />
                        This kit can provision a machine.
                    </p>
                    <ul class="mt-1 space-y-0.5">
                        <li>
                            App Store archives:
                            {{
                                readiness.archive === 'team_key'
                                    ? 'the distribution certificate and App Store profile are created at Apple during provisioning.'
                                    : readiness.archive === 'files'
                                      ? 'signed with the stored identity and profile.'
                                      : 'not possible with this kit; add a Team key, or the distribution identity with its export password and profile.'
                            }}
                        </li>
                        <li>
                            Debug builds on an iPhone:
                            {{
                                readiness.phone === 'team_key'
                                    ? 'the development identity and device profile are created at Apple when the phone is prepared.'
                                    : readiness.phone === 'files'
                                      ? 'signed with the stored development identity.'
                                      : 'not possible with this kit; add a Team key or a development identity.'
                            }}
                        </li>
                    </ul>
                </template>
                <p v-else>
                    Not usable yet: still needs {{ shortfall.join(' and ') }}. A kit can be saved
                    now and finished later.
                </p>
            </div>

            <Callout v-if="signing.state.error" tone="danger">{{ signing.state.error }}</Callout>

            <div class="flex items-center justify-end gap-2">
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
                    size="sm"
                    :disabled="signing.state.saving || form.name.trim() === ''"
                >
                    <Spinner v-if="signing.state.saving" tone="text-white dark:text-zinc-950" />
                    <Lock v-else class="h-3.5 w-3.5" />
                    {{ editing ? 'Save changes' : 'Store in the OS vault' }}
                </Button>
            </div>
        </form>
    </Modal>
</template>
