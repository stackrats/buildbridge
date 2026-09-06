<script setup lang="ts">
import { KeyRound, RefreshCw } from '@lucide/vue';
import { computed, ref, watch } from 'vue';

import type { JourneyStep } from '../../../model/steps';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import CopyButton from '../../ui/CopyButton.vue';
import DisclosureSummary from '../../ui/DisclosureSummary.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import Field from '../../ui/Field.vue';
import Input from '../../ui/Input.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();

const view = computed(() => session.view!);
const guest = computed(() => view.value.guest);
const busy = computed(() => session.operation !== null);
const done = computed(() => step.status === 'done');
const username = ref(guest.value.username ?? '');
// Held only until the install is attempted; cleared before the call goes out either way.
const password = ref('');
const failure = computed(() =>
    session.lastFailure?.operation === 'guest-authorize' ? session.lastFailure : null,
);
const canAuthorize = computed(
    () => !busy.value && username.value.trim() !== '' && password.value !== '',
);

watch(
    () => guest.value.username,
    (value) => {
        if (value && !username.value) {
            username.value = value;
        }
    },
);

async function authorize(): Promise<void> {
    if (!canAuthorize.value) {
        return;
    }
    const secret = password.value;
    password.value = '';
    await machines.authorizeGuestKey(session.id, username.value.trim(), secret);
}

const installCommand = computed(() =>
    guest.value.publicKey
        ? [
              'mkdir -p ~/.ssh && chmod 700 ~/.ssh',
              `echo '${guest.value.publicKey}' >> ~/.ssh/authorized_keys`,
              'chmod 600 ~/.ssh/authorized_keys',
          ].join('\n')
        : '',
);
</script>

<template>
    <StepPanel :step="step">
        <template v-if="guest.publicKey && !done" #action>
            <Button
                variant="outline"
                size="sm"
                :disabled="busy || session.refreshing"
                @click="machines.refreshMachine(session.id)"
            >
                <Spinner v-if="session.refreshing" />
                <RefreshCw v-else class="h-3.5 w-3.5" />
                Verify access
            </Button>
        </template>

        <template v-if="(failure || guest.diagnostics.issue) && !done" #status>
            <FailureBlock
                v-if="failure"
                title="The key was not installed"
                cause="Check the macOS short username and the local login password, then try again, or add the key from the guest Terminal below."
                :diagnostic="failure.message"
            />
            <Callout v-else-if="guest.diagnostics.issue" tone="warn">
                {{ guest.diagnostics.issue }}
            </Callout>
        </template>

        <form class="space-y-3" @submit.prevent="authorize">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                BuildBridge signs in to the guest with a dedicated Ed25519 key it generates on this
                host. Installing that key takes the macOS login password once: it is sent over a
                single SSH session to the pinned guest identity, then discarded. It is not stored or
                logged, and nothing after this step asks for it. If you would rather not type it
                here, add the key from the guest Terminal instead.
            </p>

            <Field
                label="macOS short username"
                hint="The account created during installation, for example builder."
            >
                <Input
                    v-model="username"
                    mono
                    placeholder="builder"
                    autocomplete="off"
                    :maxlength="32"
                />
                <template v-if="done" #action>
                    <Button
                        variant="outline"
                        :disabled="busy || !username.trim()"
                        @click="machines.configureGuestAccess(session.id, username.trim())"
                    >
                        <Spinner v-if="session.operation === 'guest-access'" />
                        <KeyRound v-else class="h-3.5 w-3.5" />
                        Save username
                    </Button>
                </template>
            </Field>

            <Field
                v-if="!done"
                label="macOS login password"
                hint="Used once to install the key, then discarded. Not the Apple Account password."
            >
                <Input
                    v-model="password"
                    type="password"
                    autocomplete="off"
                    :maxlength="512"
                    :disabled="busy"
                />
                <template #action>
                    <Button type="submit" :disabled="!canAuthorize">
                        <Spinner
                            v-if="session.operation === 'guest-authorize'"
                            tone="text-white dark:text-zinc-950"
                        />
                        <KeyRound v-else class="h-3.5 w-3.5" />
                        Install the key
                    </Button>
                </template>
            </Field>

            <details v-if="!done">
                <DisclosureSummary class="text-xs font-medium text-zinc-600 dark:text-zinc-300">
                    Add the key from the guest Terminal instead
                </DisclosureSummary>
                <div
                    v-if="guest.publicKey"
                    class="mt-2 rounded-md bg-zinc-50 p-2.5 dark:bg-zinc-950"
                >
                    <div class="flex items-center justify-between gap-2">
                        <p class="text-xs text-zinc-500 dark:text-zinc-400">
                            Run this in the guest Terminal as {{ guest.username }}, then verify
                            access
                        </p>
                        <div class="flex items-center gap-1">
                            <CopyButton
                                :text="guest.publicKey"
                                what="Copy the public key on its own"
                                size="iconSm"
                            />
                            <CopyButton :text="installCommand" label="Copy commands" />
                        </div>
                    </div>
                    <pre
                        class="mt-1 overflow-x-auto font-mono text-[11px] leading-4 whitespace-pre text-zinc-600 dark:text-zinc-300"
                        >{{ installCommand }}</pre>
                </div>
                <div
                    v-else
                    class="mt-2 flex items-center justify-between gap-3 rounded-md bg-zinc-50 p-2.5 dark:bg-zinc-950"
                >
                    <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                        No password is needed for this route. Generate the key first; the commands
                        to type appear here.
                    </p>
                    <Button
                        variant="outline"
                        size="sm"
                        :disabled="busy || !username.trim()"
                        @click="machines.configureGuestAccess(session.id, username.trim())"
                    >
                        <Spinner v-if="session.operation === 'guest-access'" />
                        <KeyRound v-else class="h-3.5 w-3.5" />
                        Generate access key
                    </Button>
                </div>
            </details>
        </form>
    </StepPanel>
</template>
