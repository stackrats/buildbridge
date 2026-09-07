<script setup lang="ts">
// Signing kits live on the host so the files are entered once; this step chooses which kit this
// machine provisions. Provisioning itself is per machine — the next step.
import { ArrowRight, KeyRound, Link2, Plus } from '@lucide/vue';
import { computed, ref, watch } from 'vue';

import { isAndroid, platformLabel } from '../../../model/providers';
import type { JourneyStep } from '../../../model/steps';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import { androidKitShortfall, kitReadiness, signingKitPlatforms } from '../../../model/signing';
import { useSigningStore } from '../../../stores/signing';
import { useUi } from '../../../stores/ui';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import Chip from '../../ui/Chip.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import Field from '../../ui/Field.vue';
import KeyValue from '../../ui/KeyValue.vue';
import Select from '../../ui/Select.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';
import AndroidVerificationCard from '../../signing/AndroidVerificationCard.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const signing = useSigningStore();
const ui = useUi();

const view = computed(() => session.view!);
const android = computed(() => isAndroid(view.value.profile.provider));
const kit = computed(() => view.value.signingKit);
const kits = computed(() => signing.kits.value);
const attaching = ref(false);
const selected = ref<string>(kit.value?.id ?? '');

watch(
    () => kit.value?.id,
    (value) => {
        selected.value = value ?? '';
    },
);

// "None" is a real choice: it detaches the machine, and the attach button says so.
const options = computed(() => [
    { value: '', label: 'None' },
    ...kits.value.map((entry) => {
        const platforms = signingKitPlatforms(entry);
        const ready = kitReadiness(entry);
        const material = platforms.length
            ? platforms.map((platform) => platformLabel[platform]).join(' and ')
            : 'No platform credentials yet';
        const status = android.value
            ? ready.android
                ? 'Android key configured'
                : entry.androidKeystoreConfigured
                  ? `Android needs ${androidKitShortfall(entry).join(' and ')}`
                  : 'No Android upload key'
            : ready.provisionable
              ? ready.archive
                  ? 'iOS release ready'
                  : 'iPhone builds only'
              : platforms.includes('ios')
                ? 'iOS credentials incomplete'
                : 'No iOS signing material';
        return {
            value: entry.id,
            label: entry.name,
            platforms,
            description: `${material} · ${status}`,
        };
    }),
]);
const detaching = computed(() => selected.value === '' && kit.value !== null);
const readiness = computed(() => kitReadiness(kit.value));
// An Android machine needs the kit's upload key and nothing of its Apple material.
const complete = computed(() =>
    android.value ? readiness.value.android : readiness.value.provisionable,
);
// Complete for the phone route only: it provisions, and the archive step stays locked.
const developmentOnly = computed(() => complete.value && readiness.value.archive === null);
// The Team key route: the next step creates the distribution files at Apple first.
const createsOnProvision = computed(() => complete.value && readiness.value.archive === 'team_key');
const changed = computed(() => selected.value !== (kit.value?.id ?? ''));
const vaultBroken = computed(
    () =>
        view.value.signingHealth === 'vault_unavailable' ||
        view.value.signingHealth === 'kit_missing',
);

// A missing file is a warning only when nothing will create it.
const details = computed(() =>
    kit.value && android.value
        ? [
              {
                  label: 'Upload keystore',
                  value: kit.value.androidKeystoreName ?? 'Not stored',
                  mono: kit.value.androidKeystoreName !== null,
                  tone: kit.value.androidKeystoreConfigured
                      ? ('default' as const)
                      : ('warn' as const),
              },
              {
                  label: 'Key alias',
                  value: kit.value.androidKeyAlias ?? 'Not stored',
                  mono: kit.value.androidKeyAlias !== null,
                  tone: kit.value.androidKeyAlias ? ('default' as const) : ('warn' as const),
              },
              {
                  label: 'Keystore password',
                  value: kit.value.androidKeystorePasswordStored ? 'Stored' : 'Not stored',
                  copyable: false,
                  tone: kit.value.androidKeystorePasswordStored
                      ? ('default' as const)
                      : ('warn' as const),
              },
              {
                  label: 'Key password',
                  value: kit.value.androidKeyPasswordStored
                      ? 'Stored'
                      : 'Same as the keystore password',
                  copyable: false,
              },
          ]
        : kit.value
          ? [
                {
                    label: 'Guest keychain password',
                    value: kit.value.guestKeychainConfigured ? 'Stored' : 'Not stored',
                    copyable: false,
                    tone: kit.value.guestKeychainConfigured
                        ? ('default' as const)
                        : ('warn' as const),
                },
                {
                    label: 'Team key',
                    value: kit.value.appStoreConnectKeyId ?? 'None',
                    mono: kit.value.appStoreConnectKeyId !== null,
                },
                {
                    label: 'Distribution identity',
                    value:
                        kit.value.signingCertificateName ??
                        (readiness.value.teamKey ? 'Created when provisioning' : 'Not stored'),
                    tone:
                        kit.value.signingCertificateConfigured || readiness.value.teamKey
                            ? ('default' as const)
                            : ('warn' as const),
                },
                {
                    label: 'Profiles',
                    value: kit.value.provisioningProfileNames.length
                        ? kit.value.provisioningProfileNames.join(', ')
                        : readiness.value.teamKey
                          ? 'Created when provisioning'
                          : 'None stored',
                    mono: kit.value.provisioningProfileNames.length > 0,
                    tone:
                        kit.value.provisioningProfileNames.length > 0 || readiness.value.teamKey
                            ? ('default' as const)
                            : ('warn' as const),
                },
                {
                    label: 'Development identity',
                    value:
                        kit.value.developmentCertificateName ??
                        (readiness.value.teamKey
                            ? 'Created when a phone is prepared'
                            : 'Not stored · optional, for iPhone builds'),
                },
            ]
          : [],
);

async function attach(): Promise<void> {
    attaching.value = true;
    await machines.attachSigningKit(session.id, selected.value || null);
    await signing.load();
    attaching.value = false;
}
</script>

<template>
    <StepPanel :step="step">
        <template #action>
            <Button variant="outline" size="sm" @click="ui.navigate({ kind: 'signing' })">
                <KeyRound class="h-3.5 w-3.5" />
                {{ kit ? 'Review signing credentials' : 'Choose signing credentials' }}
                <ArrowRight class="h-3.5 w-3.5" />
            </Button>
        </template>

        <template v-if="vaultBroken || !kits.length" #status>
            <FailureBlock
                v-if="view.signingHealth === 'vault_unavailable'"
                title="The credential vault could not be read"
                cause="Signing credentials live in this host's operating-system keyring. Unlock it, or sign in again, then refresh this machine. Nothing on this machine has been lost."
                :diagnostic="view.vaultIssue"
            >
                <template #actions>
                    <Button
                        variant="outline"
                        size="sm"
                        :disabled="session.operation !== null || session.refreshing"
                        @click="machines.refreshMachine(session.id)"
                    >
                        <Spinner v-if="session.refreshing" />
                        Refresh this machine
                    </Button>
                </template>
            </FailureBlock>
            <FailureBlock
                v-else-if="view.signingHealth === 'kit_missing'"
                title="The credentials this machine was provisioned from are no longer stored"
                cause="Signing was provisioned here before, so the guest still holds a buildbridge keychain, but the credentials that created it are gone from this host's keyring. That happens when the operating-system keyring is reset or recreated; nothing inside the machine was touched."
            >
                Store the credentials again, attach them here, then run
                <b>Provision signing into macOS</b> once more. Provisioning recreates the guest
                keychain, so the new keychain password does not have to match the old one.
                <template #actions>
                    <Button size="sm" @click="ui.navigate({ kind: 'signing' })">
                        <Plus class="h-3.5 w-3.5" />
                        Store the credentials again
                    </Button>
                </template>
            </FailureBlock>
            <Callout v-else-if="android" tone="warn" title="No signing credentials stored yet">
                Store credentials and create an upload key in them, or point them at a keystore you
                already hold, then attach them here.
                <div class="mt-2">
                    <Button variant="outline" size="sm" @click="ui.navigate({ kind: 'signing' })">
                        <Plus class="h-3.5 w-3.5" />
                        Store signing credentials
                    </Button>
                </div>
            </Callout>
            <Callout v-else tone="warn" title="No signing credentials stored yet">
                Store a Team key (an App Store Connect API key), or the identity and profiles
                exported from a Mac, then attach the credentials here. buildbridge generates the
                guest keychain password for you unless you choose your own.
                <div class="mt-2">
                    <Button variant="outline" size="sm" @click="ui.navigate({ kind: 'signing' })">
                        <Plus class="h-3.5 w-3.5" />
                        Store signing credentials
                    </Button>
                </div>
            </Callout>
        </template>

        <div class="space-y-3">
            <p v-if="android" class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Signing credentials are stored once on this host and shared by every machine. An
                Android release signs with the upload key in the attached credentials: a keystore on
                this host, the alias of the key in it, and its password in the vault. Nothing is
                provisioned into the container; the key is streamed in for each release and removed
                afterwards. The same credentials can hold Apple material beside it, so they can
                serve both a macOS and an Android machine.
            </p>
            <p v-else class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Signing credentials are stored once on this host and shared by every machine, so
                nothing is re-entered. Each machine is attached to one; the next step imports those
                credentials into this machine's own keychain, creating the certificate and profile
                at Apple first when they sign with a Team key.
            </p>

            <template v-if="kits.length && !vaultBroken">
                <Field
                    label="Signing credentials for this machine"
                    :hint="
                        kit ? undefined : 'Nothing is chosen for you, even with only one stored.'
                    "
                >
                    <Select
                        v-model="selected"
                        :options="options"
                        placeholder="Choose credentials"
                        :disabled="attaching"
                    />
                    <template #action>
                        <Button
                            :title="
                                android
                                    ? 'The release signs with it'
                                    : 'Provisioning imports it next'
                            "
                            :disabled="attaching || !changed"
                            @click="attach"
                        >
                            <Spinner v-if="attaching" tone="text-white dark:text-zinc-950" />
                            <Link2 v-else class="h-3.5 w-3.5" />
                            {{ detaching ? 'Detach' : 'Attach' }}
                        </Button>
                    </template>
                </Field>

                <template v-if="kit">
                    <KeyValue :items="details" :columns="3" />
                    <Callout
                        v-if="android && !complete"
                        tone="warn"
                        title="These credentials hold no upload key"
                        class="mt-3"
                    >
                        Still needs {{ androidKitShortfall(kit).join(' and ') }}. On the Signing
                        page, create an upload key in these credentials, or edit them to point at a
                        keystore you already use for Google Play.
                    </Callout>
                    <Callout
                        v-else-if="!android && developmentOnly"
                        tone="warn"
                        title="Development identity only"
                        class="mt-3"
                    >
                        These credentials can provision and run Debug builds on a registered iPhone,
                        but they hold no distribution identity and no Team key to create one, so the
                        signed archive step stays locked. Add either and provision again to unlock
                        it.
                    </Callout>
                    <Callout
                        v-else-if="!android && createsOnProvision"
                        tone="neutral"
                        title="Created at Apple when you provision"
                        class="mt-3"
                    >
                        These credentials sign with a Team key. The next step creates the Apple
                        Distribution certificate and the App Store profile for
                        <span class="font-mono">{{
                            view.appleWorkspace?.bundleIdentifier ?? 'the project'
                        }}</span>
                        at Apple, keeps them in the credentials, and imports them into this
                        machine's keychain. Nothing at Apple is revoked.
                    </Callout>
                    <div class="flex flex-wrap items-center gap-1.5">
                        <Chip
                            :interactive="false"
                            :dot="complete ? 'bg-emerald-500' : 'bg-amber-500'"
                        >
                            {{
                                complete
                                    ? android
                                        ? 'Credentials configured'
                                        : developmentOnly
                                          ? 'Ready to provision · phone only'
                                          : 'Ready to provision'
                                    : 'Incomplete'
                            }}
                        </Chip>
                        <Chip
                            v-if="!android && kit.appStoreConnectKeyId"
                            :interactive="false"
                            dot="bg-emerald-500"
                        >
                            Team key {{ kit.appStoreConnectKeyId }}
                        </Chip>
                        <Chip v-if="kit.attachedMachines.length > 1" :interactive="false">
                            Shared with {{ kit.attachedMachines.length - 1 }} other machine{{
                                kit.attachedMachines.length === 2 ? '' : 's'
                            }}
                        </Chip>
                    </div>
                    <AndroidVerificationCard v-if="android && complete" :kit="kit" />
                </template>
            </template>
        </div>
    </StepPanel>
</template>
