<script setup lang="ts">
import { ChevronRight, KeyRound, Terminal } from '@lucide/vue';
import { computed, ref } from 'vue';

import type { JourneyStep } from '../../../model/steps';
import { xcodePhaseLabel } from '../../../model/phases';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import CopyButton from '../../ui/CopyButton.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import Field from '../../ui/Field.vue';
import Input from '../../ui/Input.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();

const view = computed(() => session.view!);
const busy = computed(() => session.operation !== null);
const activating = computed(() => step.status === 'running');
const showManual = ref(false);
// Held only until activation starts; cleared before the call goes out either way.
const password = ref('');
// Which of the two routes is running, for the spinner on the button that started it.
const route = ref<'ssh' | 'terminal' | null>(null);
const failure = computed(() =>
    session.lastFailure?.operation === 'xcode-activate' ? session.lastFailure : null,
);

async function activate(via: 'ssh' | 'terminal'): Promise<void> {
    if (busy.value || step.status !== 'active' || (via === 'ssh' && password.value === '')) {
        return;
    }
    const secret = via === 'ssh' ? password.value : null;
    password.value = '';
    route.value = via;
    try {
        await machines.activateXcode(session.id, secret);
    } finally {
        route.value = null;
    }
}

const commands = computed(() => {
    if (session.xcodeImport?.activationCommands.length) {
        return session.xcodeImport.activationCommands;
    }
    const xcodePath =
        view.value.guest.diagnostics.xcodePath ??
        `/Users/${view.value.guest.username ?? 'builder'}/Applications/Xcode.app`;
    return [
        `sudo xcode-select --switch ${xcodePath}`,
        'sudo xcodebuild -license accept',
        'sudo xcodebuild -runFirstLaunch',
        'xcodebuild -version',
    ];
});
</script>

<template>
    <StepPanel :step="step">
        <template v-if="step.status !== 'done'" #action>
            <Button
                size="sm"
                :title="
                    password === ''
                        ? 'Type the macOS login password below first'
                        : 'Runs over the pinned SSH bridge; the password is used once and discarded'
                "
                :disabled="busy || step.status !== 'active' || password === ''"
                @click="activate('ssh')"
            >
                <Spinner
                    v-if="session.operation === 'xcode-activate' && route === 'ssh'"
                    tone="text-white dark:text-zinc-950"
                />
                <KeyRound v-else class="h-3.5 w-3.5" />
                Activate Xcode
            </Button>
            <Button
                variant="outline"
                size="sm"
                title="Opens a command file in the guest's Terminal, where you type the password yourself"
                :disabled="busy || step.status !== 'active'"
                @click="activate('terminal')"
            >
                <Spinner v-if="session.operation === 'xcode-activate' && route === 'terminal'" />
                <Terminal v-else class="h-3.5 w-3.5" />
                Use the guest Terminal instead
            </Button>
        </template>

        <template v-if="activating || failure" #status>
            <ProgressRow
                stoppable
                :stopping="session.cancelling"
                @stop="machines.cancelOperation(session.id)"
                v-if="activating"
                :label="
                    session.xcode
                        ? xcodePhaseLabel[session.xcode.phase]
                        : 'Starting Xcode activation'
                "
                :detail="session.xcode?.detail"
                :elapsed-seconds="session.xcode?.elapsedSeconds ?? null"
            />
            <FailureBlock
                v-else-if="failure"
                title="Activation did not complete"
                cause="Check the password and run it again, use the guest Terminal route instead, or use the manual commands below."
                :diagnostic="failure.message"
            />
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Activation selects the developer directory, accepts Apple's license, and runs
                Xcode's first-launch tasks, all of which need an administrator. Type the local macOS
                password here and BuildBridge runs them over the pinned SSH bridge with sudo, using
                the password for that one session and then discarding it. If you would rather not
                type it here, the guest Terminal route opens a short-lived command file inside macOS
                where you type the password yourself; it times out after 30 minutes.
            </p>

            <Field
                v-if="step.status !== 'done'"
                label="macOS login password"
                hint="The account created during installation. Not the Apple Account password. Used once over SSH, then discarded; never stored."
            >
                <Input
                    v-model="password"
                    type="password"
                    autocomplete="off"
                    :maxlength="512"
                    :disabled="busy || step.status !== 'active'"
                    @keydown.enter.prevent="activate('ssh')"
                />
            </Field>

            <Callout tone="neutral" title="If Xcode shows a component chooser">
                The first time Xcode's graphical app opens it may ask which platforms to install.
                You can leave every platform unchecked, iOS included: signed archives and phone
                builds use the SDK inside Xcode, and the test build downloads the iOS Simulator
                runtime only if you choose the Simulator target there.
            </Callout>

            <div>
                <button
                    type="button"
                    class="flex items-center gap-1 text-xs text-zinc-500 hover:text-zinc-600 dark:text-zinc-400 dark:hover:text-zinc-300"
                    @click="showManual = !showManual"
                >
                    <ChevronRight
                        class="h-3 w-3 transition-transform motion-reduce:transition-none"
                        :class="showManual ? 'rotate-90' : ''"
                    />
                    Manual recovery commands
                </button>
                <div v-if="showManual" class="mt-2 rounded-md bg-zinc-50 p-2.5 dark:bg-zinc-950">
                    <div class="flex items-center justify-between gap-2">
                        <p class="text-xs text-zinc-500 dark:text-zinc-400">
                            Run these in the guest Terminal if both automatic routes fail
                        </p>
                        <CopyButton :text="commands.join('\n')" label="Copy" />
                    </div>
                    <pre
                        class="mt-1 overflow-x-auto font-mono text-[11px] leading-4 whitespace-pre text-zinc-600 dark:text-zinc-300"
                        >{{ commands.join('\n') }}</pre>
                </div>
            </div>
        </div>
    </StepPanel>
</template>
