<script setup lang="ts">
// The machine profile fields, shared by the new-machine and edit-machine dialogs.
import { computed } from 'vue';

import type { MacBuilderConfig, MacOsRelease, MachineProvider } from '../../types/backend';
import Field from '../ui/Field.vue';
import Input from '../ui/Input.vue';
import NumberField from '../ui/NumberField.vue';
import Select from '../ui/Select.vue';

const model = defineModel<MacBuilderConfig>({ required: true });
const { hardwareLocked = false, providerLocked = false } = defineProps<{
    /** True while a container exists: memory, cores, port, and release cannot change. */
    hardwareLocked?: boolean;
    /** True for an existing machine: its disk directory belongs to the provider that made it. */
    providerLocked?: boolean;
}>();

const providers: { value: MachineProvider; label: string }[] = [
    { value: 'docker_osx', label: 'Docker-OSX' },
    { value: 'dockur_macos', label: 'dockur/macos' },
];

// Both run macOS under QEMU with KVM in a container BuildBridge creates; installs, builds,
// signing and templates are the same on either. What differs is below, so the choice is an
// informed one rather than a label.
const differences: Record<MachineProvider, { label: string; detail: string }[]> = {
    docker_osx: [
        {
            label: 'Screen',
            detail: "A window on this host's X display, opened by the machine itself.",
        },
        { label: 'Host', detail: 'Docker with KVM and an X11 display; nothing else.' },
        {
            label: 'Disk',
            detail: 'A qcow2 BuildBridge keeps beside the machine on this host, with an identity it generates once.',
        },
        {
            label: 'Track record',
            detail: 'The original. Templates, signing, archives and iPhone passthrough have all been proven on it; its upstream has been quiet since late 2025.',
        },
    ],
    dockur_macos: [
        {
            label: 'Screen',
            detail: 'A web page on the port after the SSH port, opened from the Install step or any browser, so a headless host works too.',
        },
        {
            label: 'Host',
            detail: "Docker with KVM, plus /dev/net/tun and the NET_ADMIN capability for its network. It refuses to start unless the machine's memory is actually free.",
        },
        {
            label: 'Disk',
            detail: "A qcow2 under the machine's storage directory on this host, with an identity the image generates itself.",
        },
        {
            label: 'Track record',
            detail: 'Actively maintained, with a newer QEMU. Installs, builds and templates work the same; iPhone passthrough uses the same controller but has not been proven on it yet.',
        },
    ],
};

// Newest first. Xcode 26 does not run on Sonoma or Ventura, so a machine built on either
// cannot reach a signed archive; say so here rather than letting it fail at the Xcode step.
// dockur/macos's own authors do not recommend Tahoe on it yet ("runs very slow"), and the
// first Tahoe install here hung in its second stage, so Sequoia is the recommendation there.
const releases = computed((): { value: MacOsRelease; label: string }[] =>
    model.value.provider === 'dockur_macos'
        ? [
              { value: 'sequoia', label: 'macOS Sequoia (recommended)' },
              { value: 'tahoe', label: 'macOS Tahoe · very slow on dockur/macos, its authors say' },
              { value: 'sonoma', label: 'macOS Sonoma · no Xcode 26' },
              { value: 'ventura', label: 'macOS Ventura · no Xcode 26' },
          ]
        : [
              { value: 'tahoe', label: 'macOS Tahoe (recommended)' },
              { value: 'sequoia', label: 'macOS Sequoia' },
              { value: 'sonoma', label: 'macOS Sonoma · no Xcode 26' },
              { value: 'ventura', label: 'macOS Ventura · no Xcode 26' },
          ],
);

/** The release each provider does best with; applied when the provider is chosen. */
export const recommendedRelease: Record<MachineProvider, MacOsRelease> = {
    docker_osx: 'tahoe',
    dockur_macos: 'sequoia',
};
</script>

<template>
    <div class="space-y-3">
        <Field label="Name" hint="Shown in the sidebar and the tray. Can be changed at any time.">
            <Input v-model="model.name" placeholder="A name for this machine" :maxlength="80" />
        </Field>
        <Field
            label="Provider"
            :hint="
                providerLocked
                    ? 'Fixed once the machine exists: its disk directory belongs to this provider.'
                    : 'Which image runs macOS. Either installs, builds, signs and clones the same way.'
            "
        >
            <Select
                :model-value="model.provider"
                :options="providers"
                :disabled="providerLocked"
                @update:model-value="model.provider = $event as MachineProvider"
            />
            <dl class="mt-2 space-y-1.5 rounded-md bg-zinc-50 p-2.5 dark:bg-zinc-950">
                <div
                    v-for="difference in differences[model.provider]"
                    :key="difference.label"
                    class="grid grid-cols-[6.5rem_1fr] gap-2"
                >
                    <dt class="text-xs font-medium text-zinc-700 dark:text-zinc-200">
                        {{ difference.label }}
                    </dt>
                    <dd class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                        {{ difference.detail }}
                    </dd>
                </div>
            </dl>
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
