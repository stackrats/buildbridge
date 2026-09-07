<script setup lang="ts">
import { computed, ref, useId, watch } from 'vue';

import { describeError } from '../../lib/utils';
import {
    availableSshPort,
    defaultMachineName,
    isAndroid,
    machinePorts,
    providerHostIssues,
    providerLabel,
    recommendedRelease,
} from '../../model/providers';
import { useMachinesStore } from '../../stores/machines';
import { useRunnerStore } from '../../stores/runner';
import { useUi } from '../../stores/ui';
import type { MachineConfig, MachineProvider, MachineTemplateSummary } from '../../types/backend';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Field from '../ui/Field.vue';
import Modal from '../ui/Modal.vue';
import Select from '../ui/Select.vue';
import Spinner from '../ui/Spinner.vue';
import MachineProfileForm from './MachineProfileForm.vue';

const open = defineModel<boolean>('open', { default: false });
const machines = useMachinesStore();
const runner = useRunnerStore();
const ui = useUi();
const formId = useId();
const configs = computed(() => machines.machines.value.map((machine) => machine.config));
const suggestedName = ref('');
const suggestedPort = ref(50922);

function blank(provider: MachineProvider = 'docker_osx'): MachineConfig {
    suggestedName.value = defaultMachineName(
        provider,
        configs.value.map((config) => config.name),
    );
    suggestedPort.value = availableSshPort(provider, configs.value) ?? 50922;
    return {
        name: suggestedName.value,
        macosRelease: recommendedRelease[provider],
        memoryGib: 8,
        cpuCores: 4,
        sshPort: suggestedPort.value,
        provider,
    };
}

const profile = ref<MachineConfig>(blank());
const saving = ref(false);
const initializing = ref(false);
const error = ref<string | null>(null);
// A fresh install is ''; a template id clones that provider's saved disk.
const startFrom = ref('');
const android = computed(() => isAndroid(profile.value.provider));
const readyTemplates = computed(() =>
    machines.state.templates.filter((template) => template.ready && !isAndroid(template.provider)),
);
const selectedTemplate = computed(() =>
    readyTemplates.value.find((template) => template.id === startFrom.value),
);
const startOptions = computed(() => [
    { value: '', label: 'Fresh install' },
    ...readyTemplates.value.map((template) => ({
        value: template.id,
        label: `${template.name} · ${providerLabel[template.provider]} · macOS ${template.macosVersion ?? '?'} · Xcode ${template.xcodeVersion ?? '?'}`,
    })),
]);

let resetting = false;
function reset(template?: MachineTemplateSummary): void {
    resetting = true;
    profile.value = blank(template?.provider);
    startFrom.value = template?.id ?? '';
    resetting = false;
}

function refreshSuggestions(): void {
    const nextName = defaultMachineName(
        profile.value.provider,
        configs.value.map((config) => config.name),
    );
    if (profile.value.name === suggestedName.value) {
        profile.value.name = nextName;
    }
    suggestedName.value = nextName;
    const nextPort = availableSshPort(profile.value.provider, configs.value) ?? 50922;
    if (profile.value.sshPort === suggestedPort.value) {
        profile.value.sshPort = nextPort;
    }
    suggestedPort.value = nextPort;
}

// A template chooses the image that owns its disk, so every saved macOS template is reachable
// without first learning which provider created it. A later manual provider change resets it.
watch(
    startFrom,
    () => {
        if (resetting || !selectedTemplate.value) {
            return;
        }
        resetting = true;
        profile.value.provider = selectedTemplate.value.provider;
        profile.value.macosRelease = recommendedRelease[selectedTemplate.value.provider];
        refreshSuggestions();
        resetting = false;
    },
    { flush: 'sync' },
);

watch(
    () => profile.value.provider,
    (provider) => {
        if (resetting) {
            return;
        }
        startFrom.value = '';
        profile.value.macosRelease = recommendedRelease[provider];
        refreshSuggestions();
    },
    { flush: 'sync' },
);

// The list can arrive after a dialog opened from a development URL, or change in another
// process. Recalculate only values that still match our suggestion; keep the person's edits.
watch(configs, () => {
    if (open.value && !saving.value) {
        refreshSuggestions();
    }
});

// Inherit the template before showing choices. The synchronous provider watcher is suppressed
// only for this reset so it cannot clear the requested clone on a later Vue update.
watch(
    open,
    async (value, _previous, onCleanup) => {
        if (!value) {
            return;
        }
        let cancelled = false;
        onCleanup(() => {
            cancelled = true;
        });
        error.value = null;
        initializing.value = false;
        const templateId = ui.state.newMachineTemplateId;
        ui.state.newMachineTemplateId = null;
        const template = machines.state.templates.find(
            (entry) => entry.id === templateId && entry.ready,
        );
        reset(template);
        if (templateId && !template) {
            initializing.value = true;
            await machines.loadTemplates();
            if (cancelled) {
                return;
            }
            const loaded = machines.state.templates.find(
                (entry) => entry.id === templateId && entry.ready,
            );
            if (loaded) {
                reset(loaded);
            } else {
                error.value =
                    'That template is no longer available. Choose a fresh install or another saved template.';
            }
            initializing.value = false;
        } else {
            void machines.loadTemplates();
        }
    },
    { immediate: true },
);

const hostIssues = computed(() =>
    providerHostIssues(machines.host.value, profile.value.provider, runner.state.status?.platform),
);
const portConflict = computed(() => {
    const ports = machinePorts(profile.value);
    return machines.machines.value.find((machine) =>
        machinePorts(machine.config).some((port) => ports.includes(port)),
    );
});
const templateMissing = computed(() => startFrom.value !== '' && !selectedTemplate.value);
const canCreate = computed(
    () =>
        !saving.value &&
        !initializing.value &&
        profile.value.name.trim() !== '' &&
        !portConflict.value &&
        !templateMissing.value,
);

async function create(): Promise<void> {
    if (!canCreate.value) {
        return;
    }
    saving.value = true;
    error.value = null;
    try {
        const id = await machines.createMachine(
            { ...profile.value, name: profile.value.name.trim() },
            startFrom.value || null,
        );
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
    <Modal v-model:open="open" title="New machine" :busy="saving">
        <form :id="formId" class="space-y-4" @submit.prevent="create">
            <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                Set up once, then reuse for every project.
            </p>
            <div
                v-if="initializing"
                class="flex items-center gap-2 py-4 text-xs text-zinc-500 dark:text-zinc-400"
                role="status"
            >
                <Spinner /> Loading your template
            </div>
            <fieldset v-else :disabled="saving" class="min-w-0">
                <MachineProfileForm
                    v-model="profile"
                    :template-name="selectedTemplate?.name"
                    :template-version="selectedTemplate?.macosVersion"
                >
                    <template #after-basics>
                        <Field
                            v-if="(readyTemplates.length || startFrom) && !android"
                            label="Start from"
                            :hint="
                                startFrom
                                    ? `Uses ${providerLabel[profile.provider]} with the saved macOS, Xcode and access setup. Your project and signing choices stay separate.`
                                    : 'Install macOS (about an hour), then add Xcode.'
                            "
                        >
                            <Select v-model="startFrom" :options="startOptions" />
                        </Field>
                        <p v-else class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                            {{
                                android
                                    ? 'Needs Docker. The first start prepares the Android SDK, Gradle and Node automatically.'
                                    : 'Needs Docker and KVM. A fresh macOS install takes about an hour, followed by Xcode setup.'
                            }}
                        </p>
                        <Callout
                            v-if="hostIssues.length"
                            tone="warn"
                            :title="`${providerLabel[profile.provider]} needs attention`"
                        >
                            <ul class="list-disc space-y-1 pl-4">
                                <li v-for="issue in hostIssues" :key="issue">{{ issue }}</li>
                            </ul>
                            <p class="mt-2">
                                You can create the machine now and fix these in Setup before
                                starting it.
                            </p>
                        </Callout>
                    </template>
                </MachineProfileForm>
            </fieldset>
            <Callout v-if="portConflict" tone="warn">
                This port is used by {{ portConflict.config.name }}. Choose another SSH port in
                Machine options; dockur/macos also reserves the next port for its screen.
            </Callout>
            <Callout v-if="templateMissing" tone="warn"
                >The selected template is unavailable. Choose a fresh install or another
                template.</Callout
            >
            <Callout v-if="error" tone="danger">{{ error }}</Callout>
        </form>
        <template #footer>
            <Button variant="outline" size="sm" :disabled="saving" @click="open = false">
                Cancel
            </Button>
            <Button :form="formId" type="submit" size="sm" :disabled="!canCreate">
                <Spinner v-if="saving" tone="text-white dark:text-zinc-950" />
                {{ saving ? 'Creating machine' : 'Create machine' }}
            </Button>
        </template>
    </Modal>
</template>
