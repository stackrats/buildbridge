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
    <Modal v-model:open="open" title="Machine profile" :busy="saving">
        <form :id="formId" class="space-y-3" @submit.prevent="save">
            <Callout
                v-if="hardwareLocked && view.profile.provider === 'android_toolchain'"
                tone="neutral"
            >
                The memory and CPU limits are fixed while the container exists. To change them, stop
                the toolchain and discard its container from the machine menu; the first build
                afterwards downloads the SDK again.
            </Callout>
            <Callout v-else-if="hardwareLocked" tone="neutral">
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
        </form>
        <template #footer>
            <div class="flex justify-end gap-2">
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
            </div>
        </template>
    </Modal>
</template>
