<script setup lang="ts">
import { ExternalLink } from '@lucide/vue';
import { computed } from 'vue';

import { secondsSince } from '../../../lib/format';
import type { JourneyStep } from '../../../model/steps';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();

const view = computed(() => session.view!);
const uptime = computed(() => secondsSince(view.value.runtime.startedAt));
const dockur = computed(() => view.value.profile.provider === 'dockur_macos');

// dockur/macos serves the screen as a web page and presents the disk as VirtIO; the picker
// and the installer are Apple's either way.
const dockurInstructions = [
    {
        title: 'Open the screen',
        body: 'Use Open screen above; the installer appears in its own window once the machine has downloaded macOS from Apple, which the first start does before anything shows.',
    },
    {
        title: 'Erase the target disk once',
        body: 'Open Disk Utility, choose View → Show All Devices, and select the largest "Apple Inc. VirtIO Block Media" disk. Erase it as "Macintosh HD" with APFS and a GUID Partition Map. Leave the smaller installer disk alone.',
    },
    {
        title: 'Install macOS and create the account',
        body: 'Quit Disk Utility, choose "Reinstall macOS", and target Macintosh HD. The guest restarts several times; keep the screen open. Create a local user with a password you will type once more when Xcode is activated. Skip Apple Account sign-in: BuildBridge never needs it.',
    },
    {
        title: 'Enable Remote Login',
        body: 'In System Settings, open General → Sharing and turn on Remote Login for that user. BuildBridge detects the SSH service and moves to the next step automatically.',
    },
];

const dockerOsxInstructions = [
    {
        title: 'Boot the installer',
        body: 'In the console window, choose "macOS Base System" in the OpenCore picker. Do not choose EFI Shell or Reset NVRAM.',
    },
    {
        title: 'Erase the target disk once',
        body: 'Open Disk Utility, choose View → Show All Devices, and select the largest "QEMU HARDDISK Media" (about 275 GB). Erase it as "Macintosh HD" with APFS and a GUID Partition Map. Leave the smaller installer disk alone.',
    },
    {
        title: 'Install macOS and create the account',
        body: 'Quit Disk Utility, choose "Reinstall macOS", and target Macintosh HD. The guest restarts several times; keep the console open. Create a local user with a password you will type once more when Xcode is activated. Skip Apple Account sign-in: BuildBridge never needs it.',
    },
    {
        title: 'Enable Remote Login',
        body: 'In System Settings, open General → Sharing and turn on Remote Login for that user. BuildBridge detects the SSH service and moves to the next step automatically.',
    },
];

const instructions = computed(() => (dockur.value ? dockurInstructions : dockerOsxInstructions));
</script>

<template>
    <StepPanel :step="step">
        <template v-if="view.displayUrl" #action>
            <Button variant="outline" size="sm" @click="machines.openMachineScreen(session.id)">
                <ExternalLink class="h-3.5 w-3.5" />
                Open screen
            </Button>
        </template>

        <template v-if="step.status === 'active'" #status>
            <ProgressRow
                label="Waiting for Remote Login"
                :detail="`checking port ${view.profile.sshPort} every 10 seconds`"
                :elapsed-seconds="uptime"
            />
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                This happens once per machine and is the only part BuildBridge cannot do for you.
                Apple shows the remaining installation time inside the installer; it usually takes
                30 to 60 minutes.
            </p>
            <ol class="space-y-2">
                <li
                    v-for="(item, index) in instructions"
                    :key="item.title"
                    class="flex gap-3 rounded-md bg-zinc-50 p-2.5 dark:bg-zinc-950"
                >
                    <span
                        class="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-zinc-200 text-[10px] font-semibold text-zinc-700 tabular-nums dark:bg-zinc-800 dark:text-zinc-200"
                    >
                        {{ index + 1 }}
                    </span>
                    <span class="min-w-0">
                        <span class="block text-xs font-medium text-zinc-700 dark:text-zinc-200">{{
                            item.title
                        }}</span>
                        <span class="block text-xs leading-5 text-zinc-500 dark:text-zinc-400">{{
                            item.body
                        }}</span>
                    </span>
                </li>
            </ol>
            <Callout v-if="dockur" tone="neutral" title="About the screen">
                It is the machine's own web viewer, served on this host's loopback at
                {{ view.displayUrl }}; any browser on this host can open it too. Closing it does not
                stop the machine. Erasing the disk is destructive only inside the guest.
            </Callout>
            <Callout v-else tone="neutral" title="About the console window">
                Closing it does not stop the machine; use Stop here or in the tray. Ctrl+Alt+G
                releases the mouse, Ctrl+Alt+- and Ctrl+Alt++ zoom. Erasing the disk is destructive
                only inside the guest.
            </Callout>
        </div>
    </StepPanel>
</template>
