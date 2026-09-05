<script setup lang="ts">
// Templates: prepared machines saved once on this host, so a new machine is a clone made in
// seconds rather than an install made in an hour. Saving happens from a machine's own menu;
// this page lists what is saved, what depends on it, and starts a clone.
import { Layers, Plus, Trash2 } from '@lucide/vue';
import { computed, onMounted, ref } from 'vue';

import { formatBytes, formatDate } from '../../lib/format';
import { useMachinesStore } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import type { MachineTemplateSummary } from '../../types/backend';
import ConfirmDialog from '../dialogs/ConfirmDialog.vue';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import EmptyState from '../ui/EmptyState.vue';
import KeyValue from '../ui/KeyValue.vue';
import Spinner from '../ui/Spinner.vue';

const machines = useMachinesStore();
const ui = useUi();

const templates = computed(() => machines.state.templates);
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
                <h1 class="text-lg font-bold text-zinc-900 dark:text-zinc-50">Templates</h1>
                <p class="mt-1 max-w-2xl text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    A template is a machine you prepared once, saved on this host as a compressed
                    copy of its disk. A new machine cloned from it starts in seconds with macOS,
                    Xcode and its access already in place, and its journey begins at the first
                    project step. Save one from a machine's menu once it has Xcode activated.
                </p>
            </div>
        </header>

        <Callout v-if="machines.state.templatesError" tone="danger">
            {{ machines.state.templatesError }}
        </Callout>

        <EmptyState
            v-if="!templates.length && !machines.state.templatesLoading"
            title="No templates yet"
            description="Open a machine whose setup is complete, choose Save as template from its menu, and it becomes the starting point for every machine after it."
        >
            <template #icon><Layers class="h-4 w-4" /></template>
        </EmptyState>

        <Card v-for="template in templates" :key="template.id">
            <template #title>
                <span class="flex flex-wrap items-center gap-2">
                    {{ template.name }}
                    <Badge v-if="template.ready" tone="ok">ready</Badge>
                    <Badge v-else tone="warn">incomplete</Badge>
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
                    New machine from this template
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

        <Card tone="well">
            <template #title>
                <span class="flex items-center gap-2">
                    <Layers class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" />
                    What a clone shares with its template
                </span>
            </template>
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                A clone's disk is a copy-on-write overlay over the template, so it costs nothing
                until it writes and the template can only be deleted once every clone is gone. A
                clone boots with the template's SSH identity, which BuildBridge pins for it, and is
                given its own access key through the template's, which is then retired from the
                clone. Templates stay on this host: nothing of Apple's is redistributed.
            </p>
        </Card>

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
                files are deleted from this host. Machines already cloned from it are not affected
                only because none exist; the source machine keeps its own disk.
            </p>
            <Callout v-if="error" tone="danger">{{ error }}</Callout>
            <p v-if="deleting" class="flex items-center gap-2">
                <Spinner />
                Removing the template files
            </p>
        </ConfirmDialog>
    </div>
</template>
