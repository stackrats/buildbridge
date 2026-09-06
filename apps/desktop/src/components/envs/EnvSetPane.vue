<script setup lang="ts">
import { Eye, EyeOff, Lock, Pencil, Plus, Trash2, Variable } from '@lucide/vue';
import { onMounted, reactive, ref, watch } from 'vue';

import { formatDate } from '../../lib/format';
import { describeEnvSetSize } from '../../model/envs';
import { useEnvSetsStore } from '../../stores/envs';
import { useMachinesStore } from '../../stores/machines';
import type { EnvSetSummary } from '../../types/backend';
import ConfirmDialog from '../dialogs/ConfirmDialog.vue';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import EmptyState from '../ui/EmptyState.vue';
import Spinner from '../ui/Spinner.vue';
import EnvSetDialog from './EnvSetDialog.vue';

const envs = useEnvSetsStore();
const machines = useMachinesStore();

// `?envDialog=1` opens the form straight away and `?envDialog=<set id>` opens it on that set, so
// the browser preview can show either.
const previewDialog =
    import.meta.env.DEV && typeof location !== 'undefined'
        ? new URLSearchParams(location.search).get('envDialog')
        : null;
const dialogOpen = ref(previewDialog !== null);
const editing = ref<EnvSetSummary | null>(null);
const removing = ref<EnvSetSummary | null>(null);

// Secret values are fetched for a set the first time one of its eyes is opened, held only while
// this pane is mounted, and dropped as soon as the sets change. The mask is a fixed width so a
// hidden value gives away nothing, not even its length.
const MASK = '••••••••••••';
const secrets = reactive<Record<string, Record<string, string>>>({});
const shown = reactive(new Set<string>());
const revealing = reactive(new Set<string>());

function secretId(set: EnvSetSummary, key: string): string {
    return `${set.id}:${key}`;
}

/** The value on screen for one secret, or null while it is masked. */
function shownValue(set: EnvSetSummary, key: string): string | null {
    return shown.has(secretId(set, key)) ? (secrets[set.id]?.[key] ?? null) : null;
}

async function toggleSecret(set: EnvSetSummary, key: string): Promise<void> {
    const id = secretId(set, key);
    if (shown.has(id)) {
        shown.delete(id);
        return;
    }
    if (!secrets[set.id]) {
        revealing.add(set.id);
        const values = await envs.reveal(set.id);
        revealing.delete(set.id);
        if (!values) {
            return;
        }
        secrets[set.id] = Object.fromEntries(values.map((secret) => [secret.key, secret.value]));
    }
    if (secrets[set.id]?.[key] !== undefined) {
        shown.add(id);
    }
}

watch(
    () => envs.state.sets,
    () => {
        for (const id of Object.keys(secrets)) {
            delete secrets[id];
        }
        shown.clear();
    },
);

onMounted(async () => {
    await envs.load();
    if (previewDialog) {
        editing.value = envs.setById(previewDialog);
    }
});

function createSet(): void {
    editing.value = null;
    dialogOpen.value = true;
}

function editSet(set: EnvSetSummary): void {
    editing.value = set;
    dialogOpen.value = true;
}

async function remove(): Promise<void> {
    const set = removing.value;
    if (!set) {
        return;
    }
    const removed = await envs.remove(set.id);
    removing.value = null;
    if (removed) {
        await machines.loadList();
    }
}
</script>

<template>
    <div class="mx-auto max-w-4xl space-y-4 p-5">
        <header class="flex items-start justify-between gap-4">
            <div>
                <h1 class="text-lg font-bold text-zinc-900 dark:text-zinc-50">Environments</h1>
                <p class="mt-0.5 max-w-2xl text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    An environment is the variables and secrets a build runs with, held in this
                    host's operating-system vault. Keep one for each stage — production, staging —
                    then attach one to each machine. At every sync it is written into that machine's
                    guest workspace for the web build and the build shell, and it never leaves this
                    host in any other way.
                </p>
            </div>
            <Button size="sm" @click="createSet">
                <Plus class="h-3.5 w-3.5" />
                New environment
            </Button>
        </header>

        <Callout v-if="envs.state.error" tone="danger">{{ envs.state.error }}</Callout>
        <Callout v-else-if="envs.state.notice" tone="ok">{{ envs.state.notice }}</Callout>

        <Card v-for="set in envs.sets.value" :key="set.id">
            <template #title>
                <span class="flex items-center gap-2">
                    {{ set.name }}
                    <Badge tone="neutral">{{ describeEnvSetSize(set) }}</Badge>
                </span>
            </template>
            <template #description
                >Added
                {{ formatDate(new Date(set.createdAtEpochSeconds * 1000).toISOString()) }}</template
            >
            <template #actions>
                <Button variant="outline" size="sm" @click="editSet(set)">
                    <Pencil class="h-3.5 w-3.5" />
                    Edit
                </Button>
                <Button variant="ghost" size="sm" @click="removing = set">
                    <Trash2 class="h-3.5 w-3.5 text-red-700 dark:text-red-400" />
                    Remove
                </Button>
            </template>

            <p
                v-if="!set.variables.length && !set.secretKeys.length"
                class="text-xs text-zinc-500 dark:text-zinc-400"
            >
                Empty. A build attached to this environment gets no variables from it.
            </p>
            <template v-else>
                <p class="text-[11px] text-zinc-500 dark:text-zinc-400">Variables</p>
                <dl
                    v-if="set.variables.length"
                    class="mt-1 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 font-mono text-[11px]"
                >
                    <template v-for="variable in set.variables" :key="variable.key">
                        <dt class="text-zinc-700 dark:text-zinc-200">{{ variable.key }}</dt>
                        <dd
                            class="truncate text-zinc-500 dark:text-zinc-400"
                            :title="variable.value"
                        >
                            {{ variable.value }}
                        </dd>
                    </template>
                </dl>
                <p v-else class="mt-1 text-xs text-zinc-500 dark:text-zinc-400">None.</p>

                <p class="mt-3 text-[11px] text-zinc-500 dark:text-zinc-400">Secrets</p>
                <dl
                    v-if="set.secretKeys.length"
                    class="mt-1 grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-3 gap-y-1 font-mono text-[11px]"
                >
                    <template v-for="key in set.secretKeys" :key="key">
                        <dt class="flex items-center gap-1 text-zinc-700 dark:text-zinc-200">
                            <Lock class="h-3 w-3 text-zinc-400 dark:text-zinc-500" />
                            {{ key }}
                        </dt>
                        <dd
                            class="flex min-w-0 items-center gap-1.5 text-zinc-500 dark:text-zinc-400"
                        >
                            <span class="truncate" :title="shownValue(set, key) ?? undefined">
                                {{ shownValue(set, key) ?? MASK }}
                            </span>
                            <Button
                                variant="ghost"
                                size="iconXs"
                                :disabled="revealing.has(set.id)"
                                :title="shownValue(set, key) === null ? 'Show value' : 'Hide value'"
                                @click="toggleSecret(set, key)"
                            >
                                <Spinner v-if="revealing.has(set.id)" size="h-3 w-3" />
                                <EyeOff
                                    v-else-if="shownValue(set, key) !== null"
                                    class="h-3 w-3 text-zinc-500 dark:text-zinc-400"
                                />
                                <Eye v-else class="h-3 w-3 text-zinc-500 dark:text-zinc-400" />
                            </Button>
                        </dd>
                    </template>
                </dl>
                <p v-else class="mt-1 text-xs text-zinc-500 dark:text-zinc-400">None.</p>
            </template>

            <p class="mt-3 text-[11px] text-zinc-500 dark:text-zinc-400">Attached machines</p>
            <div v-if="set.attachedMachines.length" class="mt-1 flex flex-wrap gap-1.5">
                <span
                    v-for="name in set.attachedMachines"
                    :key="name"
                    class="inline-flex items-center gap-1.5 rounded-full border border-zinc-200 px-2.5 py-0.5 text-xs text-zinc-700 dark:border-zinc-800 dark:text-zinc-200"
                >
                    <span class="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                    {{ name }}
                </span>
            </div>
            <p v-else class="mt-1 text-xs text-zinc-500 dark:text-zinc-400">
                Not attached to any machine yet. Open a machine and attach it at its Synchronize
                source step.
            </p>
        </Card>

        <EmptyState
            v-if="!envs.sets.value.length && envs.state.loaded"
            title="No environments yet"
            description="Builds run with the project's committed configuration until an environment is attached. Store one when the web build needs values that are not in the repository."
        >
            <template #icon><Variable class="h-4 w-4" /></template>
            <Button size="sm" @click="createSet">
                <Plus class="h-3.5 w-3.5" />
                New environment
            </Button>
        </EmptyState>

        <EnvSetDialog v-model:open="dialogOpen" :set="editing" />

        <ConfirmDialog
            :open="removing !== null"
            title="Remove this environment"
            confirm-label="Remove environment"
            :busy="envs.state.deleting"
            destructive
            @update:open="(value) => (removing = value ? removing : null)"
            @confirm="remove"
        >
            <p>
                <b>{{ removing?.name }}</b> is removed from the operating-system vault and every
                machine attached to it is detached. Their next sync runs without these variables.
            </p>
        </ConfirmDialog>
    </div>
</template>
