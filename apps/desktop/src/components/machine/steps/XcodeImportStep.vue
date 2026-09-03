<script setup lang="ts">
import { Download, Upload } from '@lucide/vue';
import { computed, ref } from 'vue';

import { percent } from '../../../lib/format';
import type { JourneyStep } from '../../../model/steps';
import { xcodePhaseLabel } from '../../../model/phases';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import Button from '../../ui/Button.vue';
import CopyButton from '../../ui/CopyButton.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import Field from '../../ui/Field.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import PathField from '../../ui/PathField.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();

const APPLE_DOWNLOADS_URL = 'https://developer.apple.com/download/all/?q=xcode';

const path = ref('');
const busy = computed(() => session.operation !== null);
const canImport = computed(
    () =>
        !busy.value && step.status === 'active' && path.value.trim().toLowerCase().endsWith('.xip'),
);
const progress = computed(() => session.xcode);
const importing = computed(() => step.status === 'running' && session.operation === 'xcode-import');
const failure = computed(() =>
    session.lastFailure?.operation === 'xcode-import' ? session.lastFailure : null,
);
</script>

<template>
    <StepPanel :step="step">
        <template v-if="importing || failure" #status>
            <ProgressRow
                stoppable
                :stopping="session.cancelling"
                @stop="machines.cancelOperation(session.id)"
                v-if="importing"
                :label="progress ? xcodePhaseLabel[progress.phase] : 'Preparing'"
                :detail="progress?.detail"
                :elapsed-seconds="progress?.elapsedSeconds ?? null"
                :value="progress ? percent(progress.transferredBytes, progress.totalBytes) : null"
                :completed-bytes="progress?.transferredBytes ?? null"
                :total-bytes="progress?.totalBytes ?? null"
            />
            <FailureBlock
                v-else-if="failure"
                title="The import did not complete"
                cause="Check that the file is the Universal Xcode .xip from Apple and that the guest has room for it, then import again."
                :diagnostic="failure.message"
            />
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Download the Universal Xcode <span class="font-mono">.xip</span> from Apple in a
                trusted browser on this host, then import it. BuildBridge streams the archive
                through the pinned SSH bridge and lets macOS verify and expand Apple's signature; no
                App Store login is involved.
            </p>
            <div
                class="flex items-center gap-2 rounded-md bg-zinc-50 p-2.5 text-xs dark:bg-zinc-950"
            >
                <Download class="h-3.5 w-3.5 shrink-0 text-zinc-500 dark:text-zinc-400" />
                <span
                    class="min-w-0 flex-1 truncate font-mono text-[11px] text-zinc-600 dark:text-zinc-300"
                    >{{ APPLE_DOWNLOADS_URL }}</span
                >
                <CopyButton :text="APPLE_DOWNLOADS_URL" label="Copy link" />
            </div>

            <Field
                v-if="step.status !== 'done'"
                label="Xcode .xip on this host"
                hint="Drop the file anywhere in this window or paste its absolute path."
            >
                <PathField
                    v-model="path"
                    kind="file"
                    title="Choose the Xcode archive"
                    :filter="{ name: 'Xcode archive', extensions: ['xip'] }"
                    placeholder="/path/to/Xcode.xip"
                    :disabled="busy"
                />
                <template #action>
                    <Button
                        :disabled="!canImport"
                        @click="machines.importXcode(session.id, path.trim())"
                    >
                        <Spinner
                            v-if="session.operation === 'xcode-import'"
                            tone="text-white dark:text-zinc-950"
                        />
                        <Upload v-else class="h-3.5 w-3.5" />
                        Import Xcode
                    </Button>
                </template>
            </Field>
        </div>
    </StepPanel>
</template>
