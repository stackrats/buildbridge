<script setup lang="ts">
import { computed, ref, watch } from 'vue';

import { describeError } from '../../lib/utils';
import { useMachinesStore } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import type { MacBuilderConfig } from '../../types/backend';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Modal from '../ui/Modal.vue';
import Spinner from '../ui/Spinner.vue';
import MachineProfileForm from './MachineProfileForm.vue';

const open = defineModel<boolean>('open', { default: false });
const machines = useMachinesStore();
const ui = useUi();

const nextPort = computed(() => {
    const used = new Set(machines.machines.value.map((machine) => machine.config.sshPort));
    let port = 50922;
    while (used.has(port)) {
        port += 1;
    }
    return port;
});

function blank(): MacBuilderConfig {
    return {
        name: machines.machines.value.length === 0 ? 'macOS builder' : '',
        macosRelease: 'tahoe',
        memoryGib: 8,
        cpuCores: 4,
        sshPort: nextPort.value,
    };
}

const profile = ref<MacBuilderConfig>(blank());
const saving = ref(false);
const error = ref<string | null>(null);

watch(open, (value) => {
    if (value) {
        profile.value = blank();
        error.value = null;
    }
});

const hostReady = computed(() => machines.host.value?.ready ?? true);

async function create(): Promise<void> {
    saving.value = true;
    error.value = null;
    try {
        const id = await machines.createMachine({
            ...profile.value,
            name: profile.value.name.trim(),
        });
        open.value = false;
        if (id) {
            ui.openMachine(id);
        }
    } catch (caught) {
        error.value = describeError(caught);
    } finally {
        saving.value = false;
    }
}
</script>

<template>
    <Modal v-model:open="open" title="New macOS machine">
        <form class="space-y-3" @submit.prevent="create">
            <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                A machine is a persistent Docker-OSX guest on this host. Install macOS and Xcode in
                it once, then reuse it for every project you build.
            </p>
            <Callout
                v-if="!hostReady"
                tone="warn"
                title="This host is not ready for Docker-OSX yet"
            >
                You can still create the machine now; the setup checklist shows what to fix before
                it can start.
            </Callout>
            <MachineProfileForm v-model="profile" />
            <Callout v-if="error" tone="danger">{{ error }}</Callout>
            <div class="flex justify-end gap-2">
                <Button variant="outline" size="sm" :disabled="saving" @click="open = false">
                    Cancel
                </Button>
                <Button type="submit" size="sm" :disabled="saving || profile.name.trim() === ''">
                    <Spinner v-if="saving" tone="text-white dark:text-zinc-950" />
                    Create machine
                </Button>
            </div>
        </form>
    </Modal>
</template>
