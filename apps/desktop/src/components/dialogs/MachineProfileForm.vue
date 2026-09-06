<script setup lang="ts">
// The machine profile fields, shared by the new-machine and edit-machine dialogs. The platform
// is chosen first and picks the provider under it; only the provider is stored, since it
// decides the platform, and both are fixed once the machine exists.
import { Check, ExternalLink } from '@lucide/vue';
import { computed } from 'vue';

import { useBackend } from '../../lib/backend';
import {
    defaultProviderFor,
    isAndroid,
    platformLabel,
    platforms,
    providerLabel,
    providerPlatform,
    providerSource,
    providersFor,
} from '../../model/providers';
import type {
    MachineConfig,
    MacOsRelease,
    MachinePlatform,
    MachineProvider,
} from '../../types/backend';
import Field from '../ui/Field.vue';
import Callout from '../ui/Callout.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import Input from '../ui/Input.vue';
import NumberField from '../ui/NumberField.vue';
import Select from '../ui/Select.vue';

const model = defineModel<MachineConfig>({ required: true });
const {
    hardwareLocked = false,
    providerLocked = false,
    templateName = null,
    templateVersion = null,
} = defineProps<{
    /** True while a container exists: memory, cores, port, and release cannot change. */
    hardwareLocked?: boolean;
    /** True for an existing machine: its directory belongs to the provider that made it. */
    providerLocked?: boolean;
    templateName?: string | null;
    templateVersion?: string | null;
}>();

const platform = computed<MachinePlatform>(() => providerPlatform[model.value.provider]);
const android = computed(() => isAndroid(model.value.provider));

// Changing the platform moves the provider to that platform's recommended one; the form never
// holds a provider that builds for another platform.
function choosePlatform(value: MachinePlatform): void {
    if (!providerLocked && value !== platform.value) {
        model.value.provider = defaultProviderFor(value);
    }
}

const providerOptions = computed(() =>
    providersFor[platform.value].map((provider) => ({
        value: provider,
        label: providerLabel[provider],
    })),
);

// The two macOS providers run macOS under QEMU with KVM in a container BuildBridge creates;
// installs, builds, signing and templates are the same on either. The Android one is a
// toolchain container with no virtual machine. What differs is below, so the choice is an
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
    android_toolchain: [
        {
            label: 'Builds',
            detail: 'A debug APK, then a signed app bundle and APK with an upload key from the signing credentials.',
        },
        {
            label: 'Host',
            detail: 'Docker and nothing else: no KVM, no display, no ports. Memory and cores are limits on its builds.',
        },
        {
            label: 'Disk',
            detail: 'A home directory beside the machine on this host, holding the SDK, two JDKs, the Gradle caches and the synchronized project; a few gigabytes.',
        },
        {
            label: 'Track record',
            detail: "A pinned Eclipse Temurin JDK image with Google's command-line tools and pinned Node downloaded into it; a real Capacitor 8 project released through it on 2026-09-06. No templates and no phone passthrough; a phone takes the APK over adb from this host.",
        },
    ],
};

// The repository behind the chosen provider, shown without its scheme and opened in the
// host's browser, where the person can read it before trusting a machine to it.
const source = computed(() => providerSource[model.value.provider]);
const sourceLabel = computed(() => source.value.replace(/^https:\/\//, ''));

function openSource(): void {
    void useBackend().openUrl(source.value);
}

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

const releaseWarning = computed(() => {
    if (android.value || templateName) {
        return null;
    }
    if (model.value.macosRelease === 'sonoma' || model.value.macosRelease === 'ventura') {
        return 'This installer cannot run Xcode 26. Choose Sequoia or newer for current iOS builds.';
    }
    return model.value.provider === 'dockur_macos' && model.value.macosRelease === 'tahoe'
        ? 'Tahoe can run very slowly with dockur/macos. Sequoia is recommended.'
        : null;
});
</script>

<template>
    <div class="space-y-4">
        <fieldset v-if="!providerLocked">
            <legend class="mb-2 text-xs font-medium text-zinc-600 dark:text-zinc-300">
                What would you like to build?
            </legend>
            <div class="grid grid-cols-2 gap-2">
                <button
                    v-for="option in platforms"
                    :key="option.value"
                    type="button"
                    :aria-pressed="platform === option.value"
                    class="rounded-lg border p-3 text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300"
                    :class="
                        platform === option.value
                            ? 'border-zinc-900 bg-zinc-50 dark:border-zinc-100 dark:bg-zinc-800'
                            : 'border-zinc-200 hover:bg-zinc-50 dark:border-zinc-700 dark:hover:bg-zinc-800'
                    "
                    @click="choosePlatform(option.value)"
                >
                    <span
                        class="flex items-center justify-between gap-2 text-sm font-semibold text-zinc-900 dark:text-zinc-50"
                    >
                        {{ option.label }}
                        <Check
                            v-if="platform === option.value"
                            class="h-3.5 w-3.5"
                            aria-hidden="true"
                        />
                    </span>
                    <span class="mt-1 block text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                        {{ option.detail }}
                    </span>
                </button>
            </div>
        </fieldset>
        <p v-else class="text-xs text-zinc-500 dark:text-zinc-400">
            {{ platformLabel[platform] }} · {{ providerLabel[model.provider] }}
            <span class="mt-1 block">Platform and provider are fixed for this machine.</span>
        </p>
        <Field label="Machine name" hint="Shown in the sidebar. Rename it at any time." required>
            <Input
                v-model="model.name"
                placeholder="A name for this machine"
                :maxlength="80"
                required
            />
        </Field>
        <slot name="after-basics" />
        <Callout v-if="releaseWarning" tone="warn">{{ releaseWarning }}</Callout>
        <details class="rounded-lg border border-zinc-200 dark:border-zinc-800">
            <DisclosureSummary class="p-3">
                <span class="min-w-0">
                    <span class="block text-xs font-medium text-zinc-900 dark:text-zinc-50"
                        >Machine options</span
                    >
                    <span
                        class="mt-0.5 block text-[11px] leading-4 text-zinc-500 dark:text-zinc-400"
                    >
                        {{ providerLabel[model.provider] }} · {{ model.memoryGib }} GiB ·
                        {{ model.cpuCores }} cores
                        <template v-if="!android">
                            · {{ templateName ? 'From template' : model.macosRelease }} · SSH
                            {{ model.sshPort }}</template
                        >
                    </span>
                </span>
            </DisclosureSummary>
            <div class="space-y-4 border-t border-zinc-200 p-3 dark:border-zinc-800">
                <Field
                    label="Provider"
                    :hint="
                        providerLocked
                            ? 'Fixed once the machine exists: its directory belongs to this provider.'
                            : android
                              ? 'One provider builds Android.'
                              : 'Which image runs macOS. Either installs, builds, signs and clones the same way.'
                    "
                >
                    <Select
                        :model-value="model.provider"
                        :options="providerOptions"
                        :disabled="providerLocked || providerOptions.length === 1"
                        @update:model-value="model.provider = $event as MachineProvider"
                    />
                </Field>
                <details class="rounded-md bg-zinc-50 p-2.5 dark:bg-zinc-950">
                    <DisclosureSummary
                        class="cursor-pointer rounded-sm text-xs text-zinc-600 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:text-zinc-300 dark:focus-visible:outline-zinc-300"
                    >
                        About {{ providerLabel[model.provider] }}
                    </DisclosureSummary>
                    <dl class="mt-3 space-y-1.5">
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
                        <div class="grid grid-cols-[6.5rem_1fr] gap-2">
                            <dt class="text-xs font-medium text-zinc-700 dark:text-zinc-200">
                                Source
                            </dt>
                            <dd class="text-xs leading-5">
                                <button
                                    type="button"
                                    class="inline-flex items-center gap-1 text-zinc-500 underline decoration-zinc-300 underline-offset-2 hover:text-zinc-800 dark:text-zinc-400 dark:decoration-zinc-600 dark:hover:text-zinc-100"
                                    v-tip="source"
                                    @click="openSource"
                                >
                                    {{ sourceLabel }}
                                    <ExternalLink class="h-3 w-3" aria-hidden="true" />
                                </button>
                            </dd>
                        </div>
                    </dl>
                </details>
                <p
                    v-if="templateName && !android"
                    class="text-xs leading-5 text-zinc-500 dark:text-zinc-400"
                >
                    macOS {{ templateVersion ?? '(version not recorded)' }} and Xcode come from
                    <b class="font-medium text-zinc-700 dark:text-zinc-200">{{ templateName }}</b
                    >. No installer download is needed.
                </p>
                <Field
                    v-else-if="!android"
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
                <div class="grid gap-3" :class="android ? 'grid-cols-2' : 'grid-cols-3'">
                    <Field
                        label="Memory (GiB)"
                        :hint="android ? '4–64 · a limit on its builds' : '4–64'"
                    >
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
                    <Field
                        v-if="!android"
                        label="SSH port"
                        :hint="
                            model.provider === 'dockur_macos'
                                ? `Screen uses ${model.sshPort + 1}`
                                : 'Unique per machine'
                        "
                    >
                        <NumberField
                            v-model="model.sshPort"
                            :min="1024"
                            :max="model.provider === 'dockur_macos' ? 65534 : 65535"
                            :disabled="hardwareLocked"
                        />
                    </Field>
                </div>
            </div>
        </details>
    </div>
</template>
