<script setup lang="ts">
import { Lock, Pencil, Plus, Trash2, Variable } from '@lucide/vue';
import { onMounted, ref, watch } from 'vue';

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
import Chip from '../ui/Chip.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import EmptyState from '../ui/EmptyState.vue';
import SecretValue from '../ui/SecretValue.vue';
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

// Each control retains only its requested value while visible. Refreshing the summaries also
// invalidates pending reads, so a replaced or removed secret cannot reappear afterwards.
const secretsRevision = ref(0);

async function loadSecret(setId: string, key: string): Promise<string> {
    const values = await envs.reveal(setId);
    const secret = values?.find((value) => value.key === key);
    if (!secret) {
        throw new Error('The saved value could not be loaded. Try again.');
    }
    return secret.value;
}

watch(
    () => envs.state.sets,
    () => {
        secretsRevision.value += 1;
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
                <p class="mt-1 max-w-2xl text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    Reuse build settings for production, staging, or another environment. Store them
                    in this host's vault and choose one on a build, release or device step. Every
                    environment works with iOS and Android. Values compiled into the app are visible
                    to its users.
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
            <details v-else>
                <DisclosureSummary> Variables and secrets </DisclosureSummary>
                <p class="mt-3 text-[11px] text-zinc-500 dark:text-zinc-400">Variables</p>
                <dl
                    v-if="set.variables.length"
                    class="mt-1 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 font-mono text-[11px]"
                >
                    <template v-for="variable in set.variables" :key="variable.key">
                        <dt class="text-zinc-700 dark:text-zinc-200">{{ variable.key }}</dt>
                        <dd
                            class="truncate text-zinc-500 dark:text-zinc-400"
                            v-tip="variable.value"
                        >
                            {{ variable.value }}
                        </dd>
                    </template>
                </dl>
                <p v-else class="mt-1 text-xs text-zinc-500 dark:text-zinc-400">None.</p>

                <p
                    class="mt-3 flex items-center gap-1 text-[11px] text-zinc-500 dark:text-zinc-400"
                >
                    <Lock class="h-3 w-3" />
                    Secrets
                </p>
                <div v-if="set.secretKeys.length" class="mt-1 space-y-2">
                    <SecretValue
                        v-for="key in set.secretKeys"
                        :key="key"
                        :label="key"
                        :identity="`${secretsRevision}:${set.id}:${key}`"
                        :load="() => loadSecret(set.id, key)"
                    />
                </div>
                <p v-else class="mt-1 text-xs text-zinc-500 dark:text-zinc-400">None.</p>
            </details>

            <div class="mt-3 border-t border-zinc-200 pt-3 dark:border-zinc-800">
                <p class="text-[11px] text-zinc-500 dark:text-zinc-400">Attached machines</p>
                <div v-if="set.attachedMachines.length" class="mt-1.5 flex flex-wrap gap-1.5">
                    <Chip
                        v-for="name in set.attachedMachines"
                        :key="name"
                        :interactive="false"
                        dot="bg-emerald-500"
                        tip="Uses this environment as its default"
                    >
                        {{ name }}
                        <span class="sr-only">, uses this environment as its default</span>
                    </Chip>
                </div>
                <p v-else class="mt-1 text-[11px] text-zinc-500 dark:text-zinc-400">
                    Choose this environment on a build, release or device step; it applies to that
                    build.
                </p>
            </div>
        </Card>

        <EmptyState
            v-if="!envs.sets.value.length && envs.state.loaded"
            title="No environments yet"
            description="Builds use the project's configuration by default. Add an environment when you need values such as an API URL or feature flags for a particular stage."
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
