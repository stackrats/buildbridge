<script setup lang="ts">
import { computed, ref, useId, watch } from 'vue';

import { useMachinesStore } from '../../stores/machines';
import type { MachineConfig, MachineView } from '../../types/backend';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Modal from '../ui/Modal.vue';
import Spinner from '../ui/Spinner.vue';
import MachineProfileForm from './MachineProfileForm.vue';

const open = defineModel<boolean>('open', { default: false });
const { view } = defineProps<{ view: MachineView }>();
const machines = useMachinesStore();

const profile = ref<MachineConfig>({ ...view.profile });
const saving = ref(false);
const formId = useId();

watch(open, (value) => {
    if (value) {
        profile.value = { ...view.profile };
    }
});

// Memory, cores and the SSH port reach a machine through the argv its container was created
// with, so they are fixed only while it runs; saving them on a stopped machine removes the
// container and the next start creates it again with the new profile.
const hardwareLocked = computed(() =>
    ['running', 'paused', 'restarting'].includes(view.runtime.state),
);
const containerExists = computed(
    () => view.runtime.state !== 'missing' && view.runtime.state !== 'unavailable',
);
const android = computed(() => view.profile.provider === 'android_toolchain');

async function save(): Promise<void> {
    saving.value = true;
    const result = await machines.configure(view.machineId, {
        ...profile.value,
        name: profile.value.name.trim(),
    });
    saving.value = false;
    if (result) {
        open.value = false;
    }
}
</script>

<template>
    <Modal v-model:open="open" title="Machine profile" :busy="saving">
        <form :id="formId" class="space-y-3" @submit.prevent="save">
            <Callout v-if="hardwareLocked && android" tone="neutral">
                The memory and CPU limits are fixed while the toolchain runs. Stop it to change
                them; its home with the SDK, the caches and the synchronized project stays on this
                host, so nothing is downloaded again.
            </Callout>
            <Callout v-else-if="hardwareLocked" tone="neutral">
                Memory, cores, the SSH port, and the installer are fixed while the machine runs.
                Stop it to change them; its macOS disk stays on this host.
            </Callout>
            <Callout v-else-if="containerExists && android" tone="neutral">
                Saving new limits recreates the toolchain container the next time it starts. Its
                home with the SDK, the caches and the synchronized project stays on this host.
            </Callout>
            <Callout v-else-if="containerExists" tone="neutral">
                Saving new memory, cores or an SSH port recreates the container the next time the
                machine starts. Its macOS disk stays on this host, so nothing is installed again.
            </Callout>
            <MachineProfileForm
                v-model="profile"
                :hardware-locked="hardwareLocked"
                provider-locked
            />
            <Callout v-if="machines.session(view.machineId).error" tone="danger">
                {{ machines.session(view.machineId).error }}
            </Callout>
        </form>
        <template #footer>
            <Button variant="outline" size="sm" :disabled="saving" @click="open = false">
                Cancel
            </Button>
            <Button
                type="submit"
                :form="formId"
                size="sm"
                :disabled="saving || profile.name.trim() === ''"
            >
                <Spinner v-if="saving" tone="text-white dark:text-zinc-950" />
                {{ saving ? 'Saving changes' : 'Save changes' }}
            </Button>
        </template>
    </Modal>
</template>
