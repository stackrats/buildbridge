<script setup lang="ts">
// Create or update one env set.
//
// A set holds plain variables, shown as stored, and secrets, which are masked. Opening the
// dialog on a stored set fetches its secrets and holds them masked, so the eye on a row only
// changes how that field renders. If the fetch fails the rows stay blank, where blank keeps what
// is stored, and the eye tries the fetch again. Nothing outlives the dialog: reopening re-seeds
// every row.
import { Eye, EyeOff, Lock, Plus, Trash2 } from '@lucide/vue';
import { computed, reactive, ref, watch } from 'vue';

import {
    envDraftIssues,
    envDraftRows,
    envDraftVariables,
    type EnvVariableDraft,
} from '../../model/envs';
import { useEnvSetsStore } from '../../stores/envs';
import type { EnvSetSummary } from '../../types/backend';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Field from '../ui/Field.vue';
import Input from '../ui/Input.vue';
import Modal from '../ui/Modal.vue';
import Spinner from '../ui/Spinner.vue';

type Section = 'variables' | 'secrets';

interface EditorRow extends EnvVariableDraft {
    /** The value is readable on screen; for a secret, the eye has been opened. */
    shown: boolean;
}

const open = defineModel<boolean>('open', { default: false });
const { set = null } = defineProps<{ set?: EnvSetSummary | null }>();

const envs = useEnvSetsStore();

const form = reactive({
    name: '',
    variables: [] as EditorRow[],
    secrets: [] as EditorRow[],
});
const loadingSecrets = ref(false);
// Bumped at every re-seed, so a fetch started for an earlier set cannot fill in a later one.
let seeded = 0;

const editing = computed(() => set !== null);
const rows = computed(() => [...form.variables, ...form.secrets]);
const named = computed(() => rows.value.filter((row) => row.key.trim() !== ''));
const issues = computed(() => envDraftIssues(named.value));
const blankRows = computed(() => named.value.length < rows.value.length);

function editorRow(draft: EnvVariableDraft): EditorRow {
    return { ...draft, shown: false };
}

function addRow(section: Section): void {
    form[section].push(
        editorRow({ key: '', value: '', secret: section === 'secrets', stored: false }),
    );
}

function removeRow(section: Section, index: number): void {
    form[section].splice(index, 1);
    if (form[section].length === 0) {
        addRow(section);
    }
}

/** A stored secret whose value has not arrived, or been typed over, yet. */
function awaitingValue(row: EditorRow): boolean {
    return row.stored && row.value === '';
}

async function loadSecrets(): Promise<void> {
    if (!set || !form.secrets.some(awaitingValue)) {
        return;
    }
    const generation = seeded;
    loadingSecrets.value = true;
    const secrets = await envs.reveal(set.id);
    loadingSecrets.value = false;
    if (generation !== seeded || !secrets) {
        return;
    }
    for (const row of form.secrets.filter(awaitingValue)) {
        row.value = secrets.find((secret) => secret.key === row.key.trim())?.value ?? '';
    }
}

async function toggleSecret(row: EditorRow): Promise<void> {
    if (row.shown) {
        row.shown = false;
        return;
    }
    if (awaitingValue(row)) {
        await loadSecrets();
        if (awaitingValue(row)) {
            return;
        }
    }
    row.shown = true;
}

// Re-seeds whenever the dialog opens, or when it is pointed at a different set while open.
watch(
    [open, () => set],
    ([value]) => {
        if (!value) {
            return;
        }
        seeded += 1;
        const drafts = envDraftRows(set);
        form.name = set?.name ?? '';
        form.variables = drafts.variables.map(editorRow);
        form.secrets = drafts.secrets.map(editorRow);
        for (const section of ['variables', 'secrets'] as const) {
            if (form[section].length === 0) {
                addRow(section);
            }
        }
        envs.clearMessages();
        void loadSecrets();
    },
    { immediate: true },
);

async function save(): Promise<void> {
    if (issues.value.length > 0) {
        return;
    }
    const saved = await envs.save({
        setId: set?.id ?? null,
        name: form.name.trim(),
        variables: envDraftVariables(named.value),
    });
    if (saved) {
        open.value = false;
    }
}
</script>

<template>
    <Modal v-model:open="open" :title="editing ? 'Edit env set' : 'New env set'" wide>
        <form class="space-y-4" @submit.prevent="save">
            <div class="border border-transparent px-3">
                <Field
                    label="Set name"
                    required
                    hint="How you will recognise it when attaching a machine, for example production or staging."
                >
                    <Input v-model="form.name" placeholder="A name for this set" :maxlength="60" />
                </Field>
            </div>

            <Callout tone="neutral">
                Variables and secrets are both written into the guest at every sync as
                <span class="font-mono">.env.production.local</span> for the web build and exported
                to the build shell. Vite only exposes keys that start with
                <span class="font-mono">VITE_</span> to the app.
            </Callout>

            <section class="space-y-3 rounded-lg border border-zinc-200 p-3 dark:border-zinc-800">
                <div>
                    <h4 class="text-[13px] font-semibold text-zinc-900 dark:text-zinc-50">
                        Variables
                    </h4>
                    <p class="mt-0.5 text-[11px] leading-4 text-zinc-500 dark:text-zinc-400">
                        Configuration that is not sensitive, like an API base URL or a feature flag.
                        Shown here as stored.
                    </p>
                </div>

                <div class="space-y-2">
                    <div
                        v-for="(row, index) in form.variables"
                        :key="index"
                        class="grid grid-cols-[minmax(0,2fr)_minmax(0,3fr)_auto] items-center gap-2"
                    >
                        <Input v-model="row.key" mono placeholder="VITE_API_URL" :maxlength="120" />
                        <Input v-model="row.value" mono placeholder="Value" autocomplete="off" />
                        <Button
                            variant="ghost"
                            size="iconSm"
                            :title="`Remove ${row.key || 'this variable'}`"
                            @click="removeRow('variables', index)"
                        >
                            <Trash2 class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" />
                        </Button>
                    </div>
                </div>

                <Button variant="outline" size="sm" @click="addRow('variables')">
                    <Plus class="h-3.5 w-3.5" />
                    Add variable
                </Button>
            </section>

            <section class="space-y-3 rounded-lg border border-zinc-200 p-3 dark:border-zinc-800">
                <div>
                    <h4
                        class="flex items-center gap-1.5 text-[13px] font-semibold text-zinc-900 dark:text-zinc-50"
                    >
                        <Lock class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" />
                        Secrets
                    </h4>
                    <p class="mt-0.5 text-[11px] leading-4 text-zinc-500 dark:text-zinc-400">
                        Tokens, keys and passwords, masked here. Use the eye to show one, or remove
                        the row to drop it.
                    </p>
                </div>

                <div class="space-y-2">
                    <div
                        v-for="(row, index) in form.secrets"
                        :key="index"
                        class="grid grid-cols-[minmax(0,2fr)_minmax(0,3fr)_auto_auto] items-center gap-2"
                    >
                        <Input
                            v-model="row.key"
                            mono
                            placeholder="VITE_SENTRY_DSN"
                            :maxlength="120"
                        />
                        <Input
                            v-model="row.value"
                            :type="row.shown ? 'text' : 'password'"
                            mono
                            :placeholder="row.stored ? 'Stored, blank keeps it' : 'Value'"
                            autocomplete="new-password"
                        />
                        <Button
                            variant="ghost"
                            size="iconSm"
                            :disabled="loadingSecrets && awaitingValue(row)"
                            :title="row.shown ? 'Hide value' : 'Show value'"
                            @click="toggleSecret(row)"
                        >
                            <Spinner v-if="loadingSecrets && awaitingValue(row)" />
                            <EyeOff
                                v-else-if="row.shown"
                                class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400"
                            />
                            <Eye v-else class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" />
                        </Button>
                        <Button
                            variant="ghost"
                            size="iconSm"
                            :title="`Remove ${row.key || 'this secret'}`"
                            @click="removeRow('secrets', index)"
                        >
                            <Trash2 class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" />
                        </Button>
                    </div>
                </div>

                <Button variant="outline" size="sm" @click="addRow('secrets')">
                    <Plus class="h-3.5 w-3.5" />
                    Add secret
                </Button>
            </section>

            <ul
                v-if="issues.length"
                class="space-y-0.5 px-3 text-[11px] leading-4 text-amber-700 dark:text-amber-400"
            >
                <li v-for="issue in issues" :key="issue">{{ issue }}</li>
            </ul>
            <p
                v-else-if="blankRows"
                class="px-3 text-[11px] leading-4 text-zinc-500 dark:text-zinc-400"
            >
                Rows without a name are ignored.
            </p>

            <Callout v-if="envs.state.error" tone="danger">{{ envs.state.error }}</Callout>

            <div class="flex items-center justify-end gap-2">
                <Button
                    variant="outline"
                    size="sm"
                    :disabled="envs.state.saving"
                    @click="open = false"
                >
                    Cancel
                </Button>
                <Button
                    type="submit"
                    size="sm"
                    :disabled="envs.state.saving || form.name.trim() === '' || issues.length > 0"
                >
                    <Spinner v-if="envs.state.saving" tone="text-white dark:text-zinc-950" />
                    <Lock v-else class="h-3.5 w-3.5" />
                    {{ editing ? 'Save changes' : 'Store in the OS vault' }}
                </Button>
            </div>
        </form>
    </Modal>
</template>
