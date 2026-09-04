<script setup lang="ts">
import { computed, ref, watch } from 'vue';

import { useMachinesStore } from '../../stores/machines';
import type { MacBuilderConfig, MacBuilderView } from '../../types/backend';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Modal from '../ui/Modal.vue';
import Spinner from '../ui/Spinner.vue';
import MachineProfileForm from './MachineProfileForm.vue';

const open = defineModel<boolean>('open', { default: false });
const { view } = defineProps<{ view: MacBuilderView }>();
const machines = useMachinesStore();

const profile = ref<MacBuilderConfig>({ ...view.profile });
const saving = ref(false);

watch(open, (value) => {
    if (value) {
        profile.value = { ...view.profile };
    }
});

const hardwareLocked = computed(
    () => view.runtime.state !== 'missing' && view.runtime.state !== 'unavailable',
);

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
    <Modal v-model:open="open" title="Machine profile">
        <form class="space-y-3" @submit.prevent="save">
            <Callout v-if="hardwareLocked" tone="neutral">
                Memory, cores, the SSH port, and the installer are fixed while the container exists.
                To change them, stop the machine and discard its container from the machine menu;
                that deletes its macOS disk.
            </Callout>
            <MachineProfileForm
                v-model="profile"
                :hardware-locked="hardwareLocked"
                provider-locked
            />
            <Callout v-if="machines.session(view.machineId).error" tone="danger">
                {{ machines.session(view.machineId).error }}
            </Callout>
            <div class="flex justify-end gap-2">
                <Button variant="outline" size="sm" :disabled="saving" @click="open = false">
                    Cancel
                </Button>
                <Button
                    type="submit"
                    size="sm"
                    title="Memory, cores and port apply at the next start"
                    :disabled="saving || profile.name.trim() === ''"
                >
                    <Spinner v-if="saving" tone="text-white dark:text-zinc-950" />
                    Save
                </Button>
            </div>
        </form>
    </Modal>
</template>
