<script setup lang="ts">
import { ShieldCheck } from '@lucide/vue';
import { computed, ref } from 'vue';

import { formatDate, percent } from '../../../lib/format';
import type { JourneyStep } from '../../../model/steps';
import { signingPhaseLabel } from '../../../model/phases';
import { profileKindLabel } from '../../../model/signing';
import { activityLabel, useMachinesStore, type MachineSession } from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import ConfirmDialog from '../../dialogs/ConfirmDialog.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import KeyValue from '../../ui/KeyValue.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const ui = useUi();

const view = computed(() => session.view!);
const signing = computed(() => view.value.signing);
const busy = computed(() => session.operation !== null);
const running = computed(() => step.status === 'running');
const removeOpen = ref(false);

// Provisioning has phases and a Stop. Removing the guest signing runs on this step too, and
// says so instead of showing the last provisioning's phase with a Stop.
const strip = computed(() => {
    const operation = session.operation ?? view.value.busyOperation;
    if (operation === 'provision' || operation === 'provisioning_signing') {
        const progress = session.signing;
        return {
            label: progress ? signingPhaseLabel[progress.phase] : 'Preparing',
            detail: progress?.detail ?? null,
            elapsed: progress?.elapsedSeconds ?? null,
            value: progress ? percent(progress.completedBytes, progress.totalBytes) : null,
            stoppable: true,
        };
    }
    return {
        label: activityLabel(operation) ?? 'Working',
        detail:
            operation === 'clear-signing' || operation === 'clearing_signing'
                ? 'the guest keychain and its profiles; the credentials on the host are kept'
                : null,
        elapsed: null,
        value: null,
        stoppable: false,
    };
});
const failure = computed(() =>
    session.lastFailure?.operation === 'provision' ? session.lastFailure : null,
);
const expiredCertificate = computed(() => failure.value?.message.includes('expired') ?? false);
const orphaned = computed(
    () => view.value.signingHealth === 'kit_missing' && signing.value !== null,
);

async function remove(): Promise<void> {
    removeOpen.value = false;
    await machines.clearGuestSigning(session.id);
}
</script>

<template>
    <StepPanel :step="step">
        <template #action>
            <Button
                size="sm"
                :disabled="busy || step.status === 'pending'"
                @click="machines.provisionSigning(session.id)"
            >
                <Spinner
                    v-if="session.operation === 'provision'"
                    tone="text-white dark:text-zinc-950"
                />
                <ShieldCheck v-else class="h-3.5 w-3.5" />
                {{ signing ? 'Provision again' : 'Provision signing into macOS' }}
            </Button>
            <Button
                v-if="signing"
                variant="ghost"
                size="sm"
                title="The credentials on the host are kept"
                :disabled="busy"
                @click="removeOpen = true"
            >
                <Spinner v-if="session.operation === 'clear-signing'" />
                Remove provisioned signing
            </Button>
        </template>

        <template v-if="running || failure || orphaned" #status>
            <ProgressRow
                v-if="running"
                :label="strip.label"
                :detail="strip.detail"
                :elapsed-seconds="strip.elapsed"
                :value="strip.value"
                :stoppable="strip.stoppable"
                :stopping="session.cancelling"
                @stop="machines.cancelOperation(session.id)"
            />
            <FailureBlock
                v-if="failure && !running"
                title="Provisioning failed"
                :cause="
                    expiredCertificate
                        ? 'The exported identity has expired. On the Mac that owns the private key, open Keychain Access → login → My Certificates, select the unexpired Distribution identity, confirm it has a private key, and export only that item as a new .p12. Store the new path and password in the signing credentials; profiles and the Team key are kept.'
                        : null
                "
                :diagnostic="failure.message"
            >
                <template v-if="expiredCertificate" #actions>
                    <Button variant="outline" size="sm" @click="ui.navigate({ kind: 'signing' })">
                        Open the signing credentials
                    </Button>
                </template>
            </FailureBlock>
            <Callout
                v-if="orphaned"
                tone="warn"
                title="The credentials this machine was provisioned from are no longer stored"
            >
                The guest keychain from the earlier provisioning still exists, and this record still
                describes it. Its password lived only in the host vault, so a signed build cannot
                unlock it. Store the credentials again and provision once more; that recreates the
                keychain with the password you enter now.
            </Callout>
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                BuildBridge creates a dedicated keychain in the guest, imports the attached
                identities as non-extractable, installs the matching profiles, and proves each
                private key works by signing and strictly verifying a disposable binary. Passwords
                travel from the vault to a fixed native helper over protected SSH input and never
                appear in arguments or logs.
            </p>

            <Callout
                v-if="signing && !signing.distributionIdentity"
                tone="warn"
                title="Development identity only"
            >
                The guest keychain holds a development identity and no distribution identity, so the
                signed archive step stays locked; the signing credentials step says what they need.
            </Callout>

            <KeyValue
                v-if="signing"
                :items="[
                    {
                        label: 'Distribution identity',
                        value:
                            signing.distributionIdentity?.identityName ??
                            'None · needed to sign an App Store archive',
                    },
                    {
                        label: 'Certificate valid until',
                        value: formatDate(
                            (signing.distributionIdentity ?? signing.developmentIdentity)
                                ?.certificateExpiresAt ?? '',
                        ),
                    },
                    {
                        label: 'Development identity',
                        value:
                            signing.developmentIdentity?.identityName ??
                            'None · needed only for Debug builds on a phone',
                    },
                    { label: 'Team', value: signing.developmentTeam, mono: true },
                    { label: 'Bundle identifier', value: signing.bundleIdentifier, mono: true },
                    {
                        label: 'Profiles installed',
                        value:
                            signing.profiles
                                .map(
                                    (profile) =>
                                        `${profile.uuid} · ${profileKindLabel[profile.kind ?? 'app_store']}${
                                            profile.provisionedDeviceUdids?.length
                                                ? ` · ${profile.provisionedDeviceUdids.length} device${profile.provisionedDeviceUdids.length === 1 ? '' : 's'}`
                                                : ''
                                        }`,
                                )
                                .join(', ') || 'none',
                        mono: true,
                    },
                    { label: 'Keychain', value: signing.keychainPath, mono: true },
                ]"
            />
        </div>

        <ConfirmDialog
            v-model:open="removeOpen"
            title="Remove provisioned signing from the guest"
            confirm-label="Remove provisioned signing"
            @confirm="remove"
        >
            <p>
                The BuildBridge keychain and the installed profiles are deleted inside this machine.
                The signing credentials in the host vault are kept, so you can provision again at
                any time.
            </p>
        </ConfirmDialog>
    </StepPanel>
</template>
