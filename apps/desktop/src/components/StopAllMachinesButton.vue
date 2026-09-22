<script setup lang="ts">
import { Square } from '@lucide/vue';
import { computed, ref } from 'vue';

import { useMachinesStore } from '../stores/machines';
import type { StopAllMachinesResult } from '../types/backend';
import Button from './ui/Button.vue';
import Callout from './ui/Callout.vue';
import Modal from './ui/Modal.vue';
import Spinner from './ui/Spinner.vue';

const { compact = false, visible = true } = defineProps<{ compact?: boolean; visible?: boolean }>();
const machines = useMachinesStore();
const stopping = machines.stoppingAll;
const open = ref(false);
const result = ref<StopAllMachinesResult | null>(null);
const error = ref<string | null>(null);
const failures = computed(
    () => result.value?.results.filter((item) => item.outcome === 'failed') ?? [],
);
const stopped = computed(
    () => result.value?.results.filter((item) => item.outcome === 'stopped').length ?? 0,
);

function show(): void {
    result.value = null;
    error.value = null;
    open.value = true;
}

async function stop(): Promise<void> {
    if (stopping.value) return;
    result.value = null;
    error.value = null;
    result.value = await machines.stopAll();
    error.value = machines.stopAllError.value;
}
</script>

<template>
    <Button
        v-if="visible"
        variant="outline"
        size="sm"
        :disabled="stopping || machines.machines.value.length === 0"
        title="Stop all BuildBridge machines and cancel their running builds. Disks and saved data are kept."
        aria-label="Stop all machines"
        @click="show"
    >
        <Spinner v-if="stopping" />
        <Square v-else class="h-3.5 w-3.5" aria-hidden="true" />
        {{ stopping ? 'Stopping machines…' : compact ? 'Stop all' : 'Stop all machines' }}
    </Button>

    <Modal v-model:open="open" title="Stop all machines" :busy="stopping">
        <div
            class="space-y-3 text-xs leading-5 text-zinc-600 dark:text-zinc-300"
            aria-live="polite"
        >
            <template v-if="!result">
                <p>
                    Stop every BuildBridge machine on this host and cancel its running build.
                    Machine disks and saved data are kept. Other applications and virtual machines
                    are unaffected.
                </p>
                <p v-if="stopping">
                    Stopping machines safely. macOS may take a couple of minutes to shut down.
                </p>
            </template>
            <template v-else>
                <Callout :tone="failures.length ? 'warn' : 'ok'">
                    <template v-if="failures.length">
                        {{ stopped }} stopped; {{ failures.length }} could not be stopped.
                    </template>
                    <template v-else-if="stopped">
                        {{ stopped }} {{ stopped === 1 ? 'machine stopped' : 'machines stopped' }}.
                    </template>
                    <template v-else-if="result.results.length"
                        >All BuildBridge machines were already stopped.</template
                    >
                    <template v-else>No BuildBridge machines were found.</template>
                </Callout>
                <ul class="space-y-2">
                    <li v-for="item in result.results" :key="item.machineId">
                        <span class="font-medium text-zinc-900 dark:text-zinc-100">{{
                            item.name
                        }}</span>
                        —
                        {{
                            item.outcome === 'stopped'
                                ? 'Stopped'
                                : item.outcome === 'already_stopped'
                                  ? 'Already stopped'
                                  : 'Could not stop'
                        }}
                        <p v-if="item.error" class="text-red-700 dark:text-red-400">
                            {{ item.error }}
                        </p>
                    </li>
                </ul>
                <p v-if="!failures.length">
                    If there is still not enough memory to start a machine, close other applications
                    or virtual machines, or reduce the memory assigned to that machine.
                </p>
            </template>
            <Callout v-if="error" tone="danger">{{ error }}</Callout>
        </div>
        <template #footer>
            <Button variant="outline" size="sm" :disabled="stopping" @click="open = false">
                {{ result || error ? 'Close' : 'Cancel' }}
            </Button>
            <Button
                v-if="!result || failures.length"
                variant="danger"
                size="sm"
                :disabled="stopping"
                @click="stop"
            >
                <Spinner v-if="stopping" />
                {{ result || error ? 'Try again' : 'Stop all machines' }}
            </Button>
        </template>
    </Modal>
</template>
