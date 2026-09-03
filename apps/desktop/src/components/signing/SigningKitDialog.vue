<script setup lang="ts">
// Create or update one signing kit.
//
// The form is deliberately not tabbed: the signing files and the App Store Connect key are not
// alternatives. The files are what signs a build; the key only adds read-only checks and
// replacement-profile creation on top. So the required set is shown plainly and always, and the
// optional key sits behind a disclosure that says what it buys.
import { ChevronRight, CircleCheck, Lock, Plus } from '@lucide/vue';
import { computed, reactive, ref, watch } from 'vue';

import { useBackend } from '../../lib/backend';
import { mergePathList } from '../../lib/paths';
import { appStoreConnectIsPartial, missingKitRequirements } from '../../model/signing';
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
});

const editing = computed(() => kit !== null);
const showAppStoreConnect = ref(false);

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

const missing = computed(() =>
    missingKitRequirements(
        {
            certificatePath: form.signingCertificatePath,
            certificatePassword: form.signingCertificatePassword,
            profilePaths: profileLines.value,
            keychainPassword: form.guestKeychainPassword,
        },
        kit,
    ),
);

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
        showAppStoreConnect.value = kit?.appStoreConnectConfigured ?? false;
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
            <div class="border border-transparent px-3">
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
            </div>

            <Callout v-if="editing" tone="neutral">
                Leave a field blank to keep what is already stored. Secret values are never shown
                again, so an empty box does not mean an empty vault.
            </Callout>

            <section class="space-y-3 rounded-lg border border-zinc-200 p-3 dark:border-zinc-800">
                <div class="flex items-start justify-between gap-3">
                    <div>
                        <h4 class="text-[13px] font-semibold text-zinc-900 dark:text-zinc-50">
                            Signing files
                        </h4>
                        <p class="mt-0.5 text-[11px] text-zinc-500 dark:text-zinc-400">
                            All four are needed before this kit can sign a build.
                        </p>
                    </div>
                    <span
                        v-if="missing.length === 0"
                        class="inline-flex shrink-0 items-center gap-1 text-[11px] text-emerald-700 dark:text-emerald-400"
                    >
                        <CircleCheck class="h-3.5 w-3.5" />
                        complete
                    </span>
                    <span
                        v-else
                        class="shrink-0 text-[11px] text-amber-700 tabular-nums dark:text-amber-400"
                    >
                        {{ 4 - missing.length }}/4
                    </span>
                </div>

                <details class="group">
                    <summary
                        class="flex cursor-pointer list-none items-center gap-1 text-[11px] text-zinc-500 hover:text-zinc-700 dark:text-zinc-400 dark:hover:text-zinc-200"
                    >
                        <ChevronRight class="h-3 w-3 transition-transform group-open:rotate-90" />
                        Where do these come from?
                    </summary>
                    <ol class="mt-2 space-y-1.5 text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                        <li class="flex gap-2">
                            <span class="shrink-0 font-semibold text-zinc-400">1</span>
                            <span>
                                In Xcode, open Settings, then Accounts, then Manage Certificates,
                                create an <b>Apple Distribution</b> identity if you need one, then
                                export it from Keychain Access as a password-protected
                                <span class="font-mono">.p12</span>. The private key stays on the
                                Mac that made it; a downloaded
                                <span class="font-mono">.cer</span> cannot sign on its own.
                            </span>
                        </li>
                        <li class="flex gap-2">
                            <span class="shrink-0 font-semibold text-zinc-400">2</span>
                            <span>
                                In Certificates, Identifiers &amp; Profiles, create a Distribution
                                <b>App Store Connect</b> profile for the exact bundle identifier and
                                that certificate, then download its
                                <span class="font-mono">.mobileprovision</span>.
                            </span>
                        </li>
                        <li class="flex gap-2">
                            <span class="shrink-0 font-semibold text-zinc-400">3</span>
                            <span>
                                Choose any new password for the keychain BuildBridge creates inside
                                a machine. It is not an Apple password and nothing else uses it.
                            </span>
                        </li>
                    </ol>
                    <div class="mt-2 flex flex-wrap gap-2">
                        <CopyButton :text="APPLE_CERTIFICATES_URL" label="Copy certificates link" />
                        <CopyButton :text="APPLE_PROFILES_URL" label="Copy profiles link" />
                    </div>
                </details>

                <Field
                    label="Distribution identity (.p12 or .pfx)"
                    required
                    :stored="kit?.signingCertificateConfigured"
                >
                    <PathField
                        v-model="form.signingCertificatePath"
                        kind="file"
                        title="Choose the signing identity"
                        :filter="{ name: 'Signing identity', extensions: ['p12', 'pfx'] }"
                        :placeholder="kit?.signingCertificateName ?? '/path/to/distribution.p12'"
                    />
                </Field>
                <Field
                    label="Export password"
                    required
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
                    required
                    :stored="(kit?.provisioningProfileNames.length ?? 0) > 0"
                    :hint="
                        editing
                            ? 'Leave empty to keep the stored profiles. Anything listed replaces them.'
                            : 'At least one, matching the project\'s bundle identifier.'
                    "
                >
                    <PathListField
                        v-model="form.provisioningProfilePaths"
                        title="Choose provisioning profiles"
                        :filter="{ name: 'Provisioning profile', extensions: ['mobileprovision'] }"
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

                <Field
                    label="Guest keychain password"
                    required
                    :stored="kit?.guestKeychainConfigured"
                    hint="Any new password you choose. It protects the keychain BuildBridge creates inside a machine."
                >
                    <Input
                        v-model="form.guestKeychainPassword"
                        type="password"
                        autocomplete="new-password"
                    />
                </Field>

                <p
                    v-if="missing.length"
                    class="text-[11px] leading-4 text-amber-700 dark:text-amber-400"
                >
                    Still needed: {{ missing.map((item) => item.label).join(', ') }}. A kit can be
                    saved incomplete, but it cannot provision until all four are present.
                </p>
            </section>

            <section class="rounded-lg border border-zinc-200 p-3 dark:border-zinc-800">
                <button
                    type="button"
                    class="flex w-full items-center gap-2 text-left"
                    @click="showAppStoreConnect = !showAppStoreConnect"
                >
                    <ChevronRight
                        class="h-3.5 w-3.5 shrink-0 text-zinc-400 transition-transform"
                        :class="showAppStoreConnect ? 'rotate-90' : ''"
                    />
                    <span class="min-w-0 flex-1">
                        <span
                            class="block text-[13px] font-semibold text-zinc-900 dark:text-zinc-50"
                        >
                            App Store Connect key
                            <span class="font-normal text-zinc-500 dark:text-zinc-400"
                                >· optional</span
                            >
                        </span>
                        <span class="block text-[11px] text-zinc-500 dark:text-zinc-400">
                            Not needed to sign. It lets BuildBridge check this team against Apple
                            and create a replacement profile when yours expires.
                        </span>
                    </span>
                    <span
                        v-if="kit?.appStoreConnectKeyId"
                        class="shrink-0 font-mono text-[11px] text-emerald-700 dark:text-emerald-400"
                    >
                        {{ kit.appStoreConnectKeyId }}
                    </span>
                </button>

                <div v-if="showAppStoreConnect" class="mt-3 space-y-3">
                    <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                        Created in App Store Connect under Users and Access, Integrations, App Store
                        Connect API. Apple lets the private key be downloaded once. All three parts
                        are needed together.
                    </p>
                    <Field
                        label="Private key (.p8)"
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
