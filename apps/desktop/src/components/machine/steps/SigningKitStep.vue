<script setup lang="ts">
// Signing kits live on the host so the files are entered once; this step chooses which kit this
// machine provisions. Provisioning itself is per machine — the next step.
import { ArrowRight, KeyRound, Link2, Plus } from '@lucide/vue';
import { computed, ref, watch } from 'vue';

import type { JourneyStep } from '../../../model/steps';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import { kitReadiness } from '../../../model/signing';
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

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const signing = useSigningStore();
const ui = useUi();

const view = computed(() => session.view!);
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
    ...kits.value.map((entry) => ({ value: entry.id, label: entry.name })),
]);
const detaching = computed(() => selected.value === '' && kit.value !== null);
const readiness = computed(() => kitReadiness(kit.value));
const complete = computed(() => readiness.value.provisionable);
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
    kit.value
        ? [
              {
                  label: 'Guest keychain password',
                  value: kit.value.guestKeychainConfigured ? 'Stored' : 'Not stored',
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
                Manage signing kits
                <ArrowRight class="h-3 w-3" />
            </Button>
        </template>

        <template v-if="vaultBroken || !kits.length" #status>
            <FailureBlock
                v-if="view.signingHealth === 'vault_unavailable'"
                title="The credential vault could not be read"
                cause="Signing kits live in this host's operating-system keyring. Unlock it, or sign in again, then refresh this machine. Nothing on this machine has been lost."
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
                title="This machine's signing kit is no longer in the vault"
                cause="Signing was provisioned here before, so the guest still holds a BuildBridge keychain, but the kit that created it is gone from this host's keyring. That happens when the operating-system keyring is reset or recreated; nothing inside the machine was touched."
            >
                Store the kit again, attach it here, then run
                <b>Provision signing into macOS</b> once more. Provisioning recreates the guest
                keychain, so the new keychain password does not have to match the old one.
                <template #actions>
                    <Button size="sm" @click="ui.navigate({ kind: 'signing' })">
                        <Plus class="h-3.5 w-3.5" />
                        Store the kit again
                    </Button>
                </template>
            </FailureBlock>
            <Callout v-else tone="warn" title="No signing kits stored yet">
                Store a Team key (an App Store Connect API key) and a keychain password once, or the
                identity and profiles exported from a Mac, then attach the kit here.
                <div class="mt-2">
                    <Button variant="outline" size="sm" @click="ui.navigate({ kind: 'signing' })">
                        <Plus class="h-3.5 w-3.5" />
                        Store a signing kit
                    </Button>
                </div>
            </Callout>
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Signing kits are stored once on this host and shared by every machine, so
                credentials are never re-entered. Each machine is attached to one kit; the next step
                imports that kit into this machine's own keychain, creating the certificate and
                profile at Apple first when the kit signs with a Team key.
            </p>

            <template v-if="kits.length && !vaultBroken">
                <Field
                    label="Signing kit for this machine"
                    :hint="kit ? undefined : 'Nothing is chosen for you, even with one kit stored.'"
                >
                    <Select
                        v-model="selected"
                        :options="options"
                        placeholder="Choose a kit"
                        :disabled="attaching"
                    />
                    <template #action>
                        <Button
                            title="Provisioning imports it next"
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
                        v-if="developmentOnly"
                        tone="warn"
                        title="Development identity only"
                        class="mt-3"
                    >
                        This kit can provision and run Debug builds on a registered iPhone, but it
                        holds no distribution identity and no Team key to create one, so the signed
                        archive step stays locked. Add either to the kit and provision again to
                        unlock it.
                    </Callout>
                    <Callout
                        v-else-if="createsOnProvision"
                        tone="neutral"
                        title="Created at Apple when you provision"
                        class="mt-3"
                    >
                        This kit signs with its Team key. The next step creates the Apple
                        Distribution certificate and the App Store profile for
                        <span class="font-mono">{{
                            view.appleWorkspace?.bundleIdentifier ?? 'the project'
                        }}</span>
                        at Apple, keeps them in the kit, and imports them into this machine's
                        keychain. Nothing at Apple is revoked.
                    </Callout>
                    <div class="flex flex-wrap items-center gap-1.5">
                        <Chip
                            :interactive="false"
                            :dot="complete ? 'bg-emerald-500' : 'bg-amber-500'"
                        >
                            {{
                                complete
                                    ? developmentOnly
                                        ? 'Ready to provision · phone only'
                                        : 'Ready to provision'
                                    : 'Incomplete kit'
                            }}
                        </Chip>
                        <Chip
                            v-if="kit.appStoreConnectKeyId"
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
                </template>
            </template>
        </div>
    </StepPanel>
</template>
