<script setup lang="ts">
// Templates: prepared machines saved once on this host, so a new machine is a clone made in
// seconds rather than an install made in an hour. Saving starts from a machine's own menu and
// runs in the background; this page shows each save as it runs, lists what is saved and what
// depends on it, and starts a clone.
import { Layers, Plus, Trash2 } from '@lucide/vue';
import { computed, onMounted, ref } from 'vue';

import { formatBytes, formatDate } from '../../lib/format';
import { templateSavePhaseLabel } from '../../model/phases';
import { providerPlatform } from '../../model/providers';
import { useMachinesStore } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import type { MachineTemplateSummary } from '../../types/backend';
import ConfirmDialog from '../dialogs/ConfirmDialog.vue';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import EmptyState from '../ui/EmptyState.vue';
import KeyValue from '../ui/KeyValue.vue';
import PlatformIcon from '../ui/PlatformIcon.vue';
import ProgressRow from '../ui/ProgressRow.vue';
import Spinner from '../ui/Spinner.vue';

const machines = useMachinesStore();
const ui = useUi();

const templates = computed(() => machines.state.templates);
// Saves under way sit ahead of the saved templates, this window's with their progress and a
// Stop, another BuildBridge process's by the lock it holds; a failed one stays until its
// machine's next operation succeeds.
const saves = computed(() => machines.templateSaves());
const failures = computed(() => machines.templateSaveFailures());
const removing = ref<MachineTemplateSummary | null>(null);
const deleting = ref(false);
const error = ref<string | null>(null);

onMounted(() => {
    void machines.loadTemplates();
});

function cloneFrom(template: MachineTemplateSummary): void {
    ui.state.newMachineTemplateId = template.id;
    ui.state.newMachineOpen = true;
}

async function remove(): Promise<void> {
    const template = removing.value;
    if (!template) {
        return;
    }
    deleting.value = true;
    error.value = null;
    try {
        await machines.deleteTemplate(template.id);
        removing.value = null;
    } catch (caught) {
        error.value = caught instanceof Error ? caught.message : String(caught);
    } finally {
        deleting.value = false;
    }
}

function detailsFor(template: MachineTemplateSummary) {
    return [
        { label: 'Saved from', value: template.sourceMachineName },
        {
            label: 'Provider',
            value: template.provider === 'dockur_macos' ? 'dockur/macos' : 'Docker-OSX',
        },
        { label: 'macOS', value: template.macosVersion ?? 'not recorded' },
        { label: 'Xcode', value: template.xcodeVersion ?? 'not recorded' },
        { label: 'Size on disk', value: formatBytes(template.sizeBytes) },
        {
            label: 'Saved',
            value: formatDate(new Date(template.createdAtEpochSeconds * 1000).toISOString()),
        },
        {
            label: 'Machines cloned from it',
            value: template.machineNames.length ? template.machineNames.join(', ') : 'none',
        },
    ];
}
</script>

<template>
    <div class="mx-auto max-w-4xl space-y-4 p-5">
        <header class="flex items-start justify-between gap-4">
            <div>
                <h1
                    class="flex items-center gap-2 text-lg font-bold text-zinc-900 dark:text-zinc-50"
                >
                    <PlatformIcon platform="ios" class="h-4 w-4" />
                    macOS templates
                </h1>
                <p class="mt-1 max-w-2xl text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    Start new macOS machines with macOS and Xcode already prepared. Save a template
                    from a ready machine's menu, then reuse it here to skip installation.
                </p>
            </div>
        </header>

        <Callout v-if="machines.state.templatesError" tone="danger">
            {{ machines.state.templatesError }}
        </Callout>

        <Card v-for="save in saves" :key="`saving-${save.machineId}`">
            <template #title>
                <span class="flex flex-wrap items-center gap-2">
                    <PlatformIcon platform="ios" class="h-4 w-4" />
                    {{ save.name ?? 'A template' }}
                    <Badge tone="warn">Saving</Badge>
                </span>
            </template>
            <template #description>
                From {{ save.machineName }}; macOS is shut down first and the machine stays stopped
                afterwards.
            </template>
            <template #actions>
                <Button
                    variant="outline"
                    size="sm"
                    title="Opens the machine; its Launch step follows the save too"
                    @click="ui.openMachine(save.machineId)"
                >
                    Open machine
                </Button>
            </template>
            <ProgressRow
                :label="
                    save.progress
                        ? templateSavePhaseLabel[save.progress.phase]
                        : 'Saving the machine as a template'
                "
                :detail="
                    save.progress?.detail ??
                    (save.own ? null : 'run by another BuildBridge process, which has its progress')
                "
                :elapsed-seconds="save.progress?.elapsedSeconds ?? null"
                :value="save.progress?.percent == null ? null : save.progress.percent / 100"
                :stoppable="save.own"
                :stopping="save.cancelling"
                stop-title="Stop the save. The partial template is removed; the machine stays stopped."
                @stop="machines.cancelOperation(save.machineId)"
            />
        </Card>

        <Callout
            v-for="failure in failures"
            :key="`failed-${failure.machineId}`"
            tone="danger"
            :title="`Saving ${failure.name ?? 'the template'} from ${failure.machineName} failed`"
        >
            {{ failure.message }}
        </Callout>

        <EmptyState
            v-if="!templates.length && !saves.length && !machines.state.templatesLoading"
            title="No templates yet"
            description="Open a machine whose setup is complete, choose Save as template from its menu, and it becomes the starting point for every machine after it."
        >
            <template #icon><Layers class="h-4 w-4" /></template>
        </EmptyState>

        <Card v-for="template in templates" :key="template.id">
            <template #title>
                <span class="flex flex-wrap items-center gap-2">
                    <PlatformIcon :platform="providerPlatform[template.provider]" class="h-4 w-4" />
                    {{ template.name }}
                    <Badge v-if="template.ready" tone="ok">Ready</Badge>
                    <Badge v-else tone="warn">Incomplete</Badge>
                </span>
            </template>
            <template #actions>
                <Button
                    size="sm"
                    :disabled="!template.ready"
                    title="Opens the new machine dialog with this template chosen"
                    @click="cloneFrom(template)"
                >
                    <Plus class="h-3.5 w-3.5" />
                    Create machine
                </Button>
                <Button
                    variant="ghost"
                    size="sm"
                    :disabled="template.machineNames.length > 0"
                    :title="
                        template.machineNames.length
                            ? 'Machines cloned from it still read through it; delete those first'
                            : 'Deletes the template files on this host'
                    "
                    @click="removing = template"
                >
                    <Trash2 class="h-3.5 w-3.5 text-red-700 dark:text-red-400" />
                    Delete
                </Button>
            </template>
            <KeyValue :items="detailsFor(template)" :columns="3" />
            <p
                v-if="!template.ready"
                class="mt-2 text-[11px] leading-4 text-amber-700 dark:text-amber-400"
            >
                The save did not finish; save the template again from its source machine.
            </p>
        </Card>

        <details
            class="rounded-lg border border-zinc-200 bg-zinc-50 p-4 dark:border-zinc-800 dark:bg-zinc-950"
        >
            <DisclosureSummary
                class="cursor-pointer text-xs font-medium text-zinc-700 dark:text-zinc-200"
            >
                How templates share storage and access
            </DisclosureSummary>
            <p class="mt-3 text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                A clone's disk is a copy-on-write overlay over the template, so it costs nothing
                until it writes and the template can only be deleted once every clone is gone. A
                clone boots with the template's SSH identity, which BuildBridge pins for it, and is
                given its own access key through the template's, which is then retired from the
                clone. Templates stay on this host: nothing of Apple's is redistributed.
            </p>
        </details>

        <ConfirmDialog
            :open="removing !== null"
            title="Delete this template"
            confirm-label="Delete template"
            :busy="deleting"
            @update:open="(value) => (removing = value ? removing : null)"
            @confirm="remove"
        >
            <p>
                <b>{{ removing?.name }}</b> and its {{ formatBytes(removing?.sizeBytes ?? 0) }} of
                files are deleted from this host. This template has no dependent clones. The source
                machine keeps its own disk.
            </p>
            <Callout v-if="error" tone="danger">{{ error }}</Callout>
            <p v-if="deleting" class="flex items-center gap-2">
                <Spinner />
                Removing the template files
            </p>
        </ConfirmDialog>
    </div>
</template>
