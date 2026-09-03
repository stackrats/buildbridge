<script setup lang="ts">
import { ChevronRight, Terminal } from '@lucide/vue';
import { computed, ref } from 'vue';

import type { JourneyStep } from '../../../model/steps';
import { xcodePhaseLabel } from '../../../model/phases';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import CopyButton from '../../ui/CopyButton.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();

const view = computed(() => session.view!);
const busy = computed(() => session.operation !== null);
const activating = computed(() => step.status === 'running');
const showManual = ref(false);
const failure = computed(() =>
    session.lastFailure?.operation === 'xcode-activate' ? session.lastFailure : null,
);

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
                title="You type the macOS password there"
                :disabled="busy || step.status !== 'active'"
                @click="machines.activateXcode(session.id)"
            >
                <Spinner
                    v-if="session.operation === 'xcode-activate'"
                    tone="text-white dark:text-zinc-950"
                />
                <Terminal v-else class="h-3.5 w-3.5" />
                Activate Xcode in the guest Terminal
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
                        : 'Opening the guest Terminal'
                "
                :detail="session.xcode?.detail"
                :elapsed-seconds="session.xcode?.elapsedSeconds ?? null"
            />
            <FailureBlock
                v-else-if="failure"
                title="Activation did not complete"
                cause="Run it again and type the macOS password in the guest Terminal within 30 minutes, or use the manual commands below."
                :diagnostic="failure.message"
            />
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Activation selects the developer directory, accepts Apple's license, and runs
                Xcode's first-launch tasks. Because macOS needs an administrator authorization that
                SSH cannot provide, BuildBridge opens a short-lived command file in the guest's
                Terminal. Type the local macOS password there; it is never sent to BuildBridge. The
                step times out after 30 minutes.
            </p>

            <Callout tone="neutral" title="If Xcode shows a component chooser">
                The first time Xcode's graphical app opens it may ask which platforms to install.
                Keep iOS selected, leave other platforms unchecked, and confirm Download &amp;
                Install once. The iOS Simulator runtime is otherwise installed automatically during
                the first project build.
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
                            Run these in the guest Terminal if the automatic route fails
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
