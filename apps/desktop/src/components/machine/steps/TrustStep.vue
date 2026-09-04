<script setup lang="ts">
import { Fingerprint, ShieldCheck } from '@lucide/vue';
import { computed, ref } from 'vue';

import type { JourneyStep } from '../../../model/steps';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import Button from '../../ui/Button.vue';
import CopyButton from '../../ui/CopyButton.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';
import ConfirmDialog from '../../dialogs/ConfirmDialog.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();

const view = computed(() => session.view!);
const ssh = computed(() => view.value.guest.ssh);
const busy = computed(() => session.operation !== null);
const forgetOpen = ref(false);

const compareCommand = 'ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub';

async function forget(): Promise<void> {
    forgetOpen.value = false;
    await machines.forgetTrust(session.id);
}
</script>

<template>
    <StepPanel :step="step">
        <template v-if="ssh.trust !== 'mismatch'" #action>
            <Button
                v-if="ssh.trust === 'untrusted' && ssh.fingerprint"
                size="sm"
                title="A different key is refused from now on"
                :disabled="busy"
                @click="machines.trustGuest(session.id, ssh.fingerprint)"
            >
                <Spinner
                    v-if="session.operation === 'trust'"
                    tone="text-white dark:text-zinc-950"
                />
                <ShieldCheck v-else class="h-3.5 w-3.5" />
                Trust this fingerprint
            </Button>
            <Button
                v-if="ssh.trust === 'trusted' && ssh.pinnedFingerprint"
                variant="outline"
                size="sm"
                :disabled="busy"
                @click="forgetOpen = true"
            >
                <Spinner v-if="session.operation === 'forget-trust'" />
                Forget pinned identity
            </Button>
        </template>

        <template v-if="ssh.trust === 'mismatch'" #status>
            <FailureBlock
                title="The guest identity changed"
                :cause="`The host key on port ${view.profile.sshPort} no longer matches the pinned fingerprint. If you rebuilt or reinstalled this machine yourself, forget the pin and trust the new key. Otherwise stop and investigate before continuing.`"
            >
                <template #actions>
                    <Button variant="outline" size="sm" :disabled="busy" @click="forgetOpen = true">
                        <Spinner v-if="session.operation === 'forget-trust'" />
                        Forget pinned identity
                    </Button>
                </template>
            </FailureBlock>
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                BuildBridge only ever talks to a guest whose SSH host key you have pinned. Compare
                the fingerprint it scanned with the one macOS reports, then trust it. A key that
                changes later is rejected until you explicitly forget the pin.
            </p>

            <div v-if="ssh.fingerprint" class="group rounded-md bg-zinc-50 p-2.5 dark:bg-zinc-950">
                <p class="flex items-center gap-1 text-xs text-zinc-500 dark:text-zinc-400">
                    <Fingerprint class="h-3.5 w-3.5" /> Scanned host key fingerprint
                    <CopyButton
                        :text="ssh.fingerprint"
                        what="Copy the scanned fingerprint"
                        size="iconXs"
                        class="opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100 motion-reduce:transition-none"
                    />
                </p>
                <p class="mt-1 font-mono text-xs break-all text-zinc-900 dark:text-zinc-50">
                    {{ ssh.fingerprint }}
                </p>
                <p
                    v-if="ssh.pinnedFingerprint && ssh.pinnedFingerprint !== ssh.fingerprint"
                    class="mt-2 text-xs text-red-700 dark:text-red-400"
                >
                    Pinned: <span class="font-mono break-all">{{ ssh.pinnedFingerprint }}</span>
                </p>
            </div>

            <div class="rounded-md bg-zinc-50 p-2.5 dark:bg-zinc-950">
                <p class="text-xs text-zinc-500 dark:text-zinc-400">
                    In the guest Terminal, print the same fingerprint with
                </p>
                <div class="mt-1 flex items-center gap-2">
                    <code
                        class="min-w-0 flex-1 font-mono text-[11px] break-all text-zinc-600 dark:text-zinc-300"
                        >{{ compareCommand }}</code
                    >
                    <CopyButton :text="compareCommand" size="iconSm" />
                </div>
            </div>
        </div>

        <ConfirmDialog
            v-model:open="forgetOpen"
            title="Forget the pinned guest identity"
            confirm-label="Forget pin"
            @confirm="forget"
        >
            <p>
                The stored host key for this machine is removed. BuildBridge will not authenticate
                to the guest again until you compare and trust a fingerprint.
            </p>
            <p>Only do this if you rebuilt the machine yourself and expect the key to differ.</p>
        </ConfirmDialog>
    </StepPanel>
</template>
