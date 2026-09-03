<script setup lang="ts">
// The machine profile fields, shared by the new-machine and edit-machine dialogs.
import type { MacBuilderConfig, MacOsRelease } from '../../types/backend';
import Field from '../ui/Field.vue';
import Input from '../ui/Input.vue';
import NumberField from '../ui/NumberField.vue';
import Select from '../ui/Select.vue';

const model = defineModel<MacBuilderConfig>({ required: true });
const { hardwareLocked = false } = defineProps<{
    /** True while a container exists: memory, cores, port, and release cannot change. */
    hardwareLocked?: boolean;
}>();

// Newest first. Xcode 26 does not run on Sonoma or Ventura, so a machine built on either
// cannot reach a signed archive; say so here rather than letting it fail at the Xcode step.
const releases: { value: MacOsRelease; label: string }[] = [
    { value: 'tahoe', label: 'macOS Tahoe (recommended)' },
    { value: 'sequoia', label: 'macOS Sequoia' },
    { value: 'sonoma', label: 'macOS Sonoma · no Xcode 26' },
    { value: 'ventura', label: 'macOS Ventura · no Xcode 26' },
];
</script>

<template>
    <div class="space-y-3">
        <Field label="Name" hint="Shown in the sidebar and the tray. Can be changed at any time.">
            <Input v-model="model.name" placeholder="A name for this machine" :maxlength="80" />
        </Field>
        <Field
            label="macOS installer"
            :hint="
                hardwareLocked
                    ? 'Fixed once the container exists.'
                    : 'Fetched from Apple during the first boot; Xcode 26 needs Sequoia or newer.'
            "
        >
            <Select
                :model-value="model.macosRelease"
                :options="releases"
                :disabled="hardwareLocked"
                @update:model-value="model.macosRelease = $event as MacOsRelease"
            />
        </Field>
        <div class="grid grid-cols-3 gap-3">
            <Field label="Memory (GiB)" hint="4–64">
                <NumberField
                    v-model="model.memoryGib"
                    :min="4"
                    :max="64"
                    :disabled="hardwareLocked"
                />
            </Field>
            <Field label="CPU cores" hint="2–32">
                <NumberField
                    v-model="model.cpuCores"
                    :min="2"
                    :max="32"
                    :disabled="hardwareLocked"
                />
            </Field>
            <Field label="SSH port" hint="Unique per machine">
                <NumberField
                    v-model="model.sshPort"
                    :min="1024"
                    :max="65535"
                    :disabled="hardwareLocked"
                />
            </Field>
        </div>
    </div>
</template>
