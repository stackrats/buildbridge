<script setup lang="ts">
// Getting Xcode into the guest. Apple keeps the archive behind an Apple ID sign-in and
// BuildBridge never handles Apple credentials, so the download is hosted rather than made: a
// window of this app opens Apple's page, the person signs in there, the .xip lands in
// BuildBridge's folder with its size shown here, and the import starts the moment it is
// complete. An archive already on this host can still be imported by path.
import { Download, Upload } from '@lucide/vue';
import { computed, ref } from 'vue';

import { formatBytes, percent } from '../../../lib/format';
import type { JourneyStep } from '../../../model/steps';
import { xcodePhaseLabel } from '../../../model/phases';
import { recommendedXcode } from '../../../model/xcode';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import Button from '../../ui/Button.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import Field from '../../ui/Field.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import PathField from '../../ui/PathField.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();

const path = ref('');
const busy = computed(() => session.operation !== null);
const recommendation = computed(() =>
    recommendedXcode(session.view!.guest.diagnostics.macosVersion),
);
const download = computed(() => session.xcodeDownload);
const downloading = computed(() => download.value?.state === 'downloading');
// A finished download that has not been imported yet: the machine was busy when it landed.
const awaitingImport = computed(
    () => download.value?.state === 'finished' && !busy.value && step.status === 'active',
);
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
        <template v-if="step.status === 'active' && !downloading && !awaitingImport" #action>
            <Button
                size="sm"
                :disabled="busy"
                :title="`Opens Apple's downloads page in a window of BuildBridge, searched for ${recommendation.label}`"
                @click="machines.downloadXcode(session.id, recommendation.query)"
            >
                <Download class="h-3.5 w-3.5" />
                Download {{ recommendation.label }} from Apple
            </Button>
        </template>

        <template v-if="importing || downloading || awaitingImport || failure" #status>
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
            <ProgressRow
                v-else-if="downloading && download"
                :label="`Downloading ${download.fileName}`"
                :detail="`${formatBytes(download.bytes)} so far; the import starts when Apple's window finishes`"
                :elapsed-seconds="null"
                :value="null"
            />
            <div
                v-else-if="awaitingImport && download"
                class="flex flex-wrap items-center gap-2 rounded-md bg-zinc-50 p-2.5 text-xs dark:bg-zinc-950"
            >
                <span class="min-w-0 flex-1 text-zinc-600 dark:text-zinc-300">
                    <span class="font-mono">{{ download.fileName }}</span> is downloaded ({{
                        formatBytes(download.bytes)
                    }}) and waiting to be imported.
                </span>
                <Button size="sm" @click="machines.importXcode(session.id, download.path)">
                    <Upload class="h-3.5 w-3.5" />
                    Import it
                </Button>
            </div>
            <FailureBlock
                v-else-if="failure"
                title="The import did not complete"
                cause="Check that the file is the Universal Xcode .xip from Apple and that the guest has room for it, then import again."
                :diagnostic="failure.message"
            />
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Apple keeps Xcode behind an Apple ID sign-in, and BuildBridge never sees that
                sign-in. The button opens Apple's downloads page in a window of this app, searched
                for the right version; sign in there, choose the Universal
                <span class="font-mono">.xip</span>, and the download lands in BuildBridge's own
                folder with its progress shown here. The import starts by itself when it is
                complete: the archive is streamed through the pinned SSH bridge and macOS verifies
                and expands Apple's signature.
            </p>
            <p
                v-if="recommendation.reason"
                class="text-xs leading-5 text-zinc-500 dark:text-zinc-400"
            >
                {{ recommendation.reason }}
            </p>

            <Field
                v-if="step.status !== 'done'"
                label="Or import a .xip already on this host"
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
                        variant="outline"
                        :disabled="!canImport"
                        @click="machines.importXcode(session.id, path.trim())"
                    >
                        <Spinner v-if="session.operation === 'xcode-import'" />
                        <Upload v-else class="h-3.5 w-3.5" />
                        Import
                    </Button>
                </template>
            </Field>
        </div>
    </StepPanel>
</template>
