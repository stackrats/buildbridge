<script setup lang="ts">
// Create or update one env set.
//
// A set holds plain variables, shown as stored, and secrets, which are masked. Opening the
// dialog on a stored set fetches its secrets and holds them masked, so the eye on a row only
// changes how that field renders. If the fetch fails the rows stay blank, where blank keeps what
// is stored, and the eye tries the fetch again. Nothing outlives the dialog: reopening re-seeds
// every row.
import { Eye, EyeOff, Lock, Plus, Trash2 } from '@lucide/vue';
import { computed, onBeforeUnmount, reactive, ref, useId, watch } from 'vue';

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
import CopyButton from '../ui/CopyButton.vue';
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
const formId = useId();

const form = reactive({
    name: '',
    variables: [] as EditorRow[],
    secrets: [] as EditorRow[],
});
const loadingSecrets = ref(false);
// Bumped at every re-seed and close, so an earlier fetch cannot fill a later or closed dialog.
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
    clearRow(form[section][index]);
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
    if (!open.value || !set || !form.secrets.some(awaitingValue)) {
        return;
    }
    const generation = seeded;
    loadingSecrets.value = true;
    const secrets = await envs.reveal(set.id);
    if (generation !== seeded || !open.value) {
        return;
    }
    loadingSecrets.value = false;
    if (!secrets) {
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
    const generation = seeded;
    if (awaitingValue(row)) {
        await loadSecrets();
        if (awaitingValue(row)) {
            return;
        }
    }
    if (generation === seeded && open.value && form.secrets.includes(row)) {
        row.shown = true;
    }
}

function clearRow(row: EditorRow): void {
    row.value = '';
    row.shown = false;
}

function clearDrafts(): void {
    seeded += 1;
    for (const row of rows.value) {
        clearRow(row);
    }
    form.name = '';
    form.variables = [];
    form.secrets = [];
    loadingSecrets.value = false;
}

// Re-seeds whenever the dialog opens, or when it is pointed at a different set while open.
watch(
    [open, () => set],
    ([value]) => {
        clearDrafts();
        if (!value) {
            return;
        }
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

onBeforeUnmount(clearDrafts);

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
    <Modal
        v-model:open="open"
        :title="editing ? 'Edit environment' : 'New environment'"
        :busy="envs.state.saving"
        wide
    >
        <form :id="formId" class="space-y-4" @submit.prevent="save">
            <div class="border border-transparent px-3">
                <Field
                    label="Name"
                    required
                    hint="How you will recognise it when attaching a machine, for example production or staging."
                >
                    <Input
                        v-model="form.name"
                        placeholder="A name for this environment"
                        :maxlength="60"
                    />
                </Field>
            </div>

            <Callout tone="warn" title="Values bundled into the app are public">
                Masking a value here does not keep it secret inside the built app. Vite exposes
                <span class="font-mono">VITE_</span> values to the app by default. Keep private
                server credentials out of frontend configuration.
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
                        <Input
                            v-model="row.key"
                            :aria-label="`Variable ${index + 1} name`"
                            mono
                            placeholder="VITE_API_URL"
                            :maxlength="120"
                        />
                        <Input
                            v-model="row.value"
                            :aria-label="`Value for ${row.key || `variable ${index + 1}`}`"
                            mono
                            placeholder="Value"
                            autocomplete="off"
                        />
                        <Button
                            variant="ghost"
                            size="iconSm"
                            :title="`Remove ${row.key || 'this variable'}`"
                            :aria-label="`Remove ${row.key || 'this variable'}`"
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
                        Tokens, keys and passwords, masked here. Show or copy a value when you need
                        it elsewhere, or remove the row to drop it.
                    </p>
                </div>

                <div class="space-y-2">
                    <div
                        v-for="(row, index) in form.secrets"
                        :key="index"
                        class="grid grid-cols-[minmax(0,2fr)_minmax(0,3fr)_auto_auto_auto] items-center gap-2"
                    >
                        <Input
                            v-model="row.key"
                            :aria-label="`Secret ${index + 1} name`"
                            mono
                            placeholder="BUILD_TOKEN"
                            :maxlength="120"
                        />
                        <Input
                            v-model="row.value"
                            :aria-label="`Value for ${row.key || `secret ${index + 1}`}`"
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
                            :aria-label="`${row.shown ? 'Hide' : 'Show'} ${row.key || 'secret'} value`"
                            @click="toggleSecret(row)"
                        >
                            <Spinner v-if="loadingSecrets && awaitingValue(row)" />
                            <EyeOff
                                v-else-if="row.shown"
                                class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400"
                            />
                            <Eye v-else class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" />
                        </Button>
                        <!-- A stored secret can be copied once its value has arrived; the eye
                             fetches it again if that failed. -->
                        <CopyButton
                            :text="row.value"
                            :what="`Copy ${row.key || 'secret'} value`"
                            size="iconSm"
                            :disabled="row.value === ''"
                        />
                        <Button
                            variant="ghost"
                            size="iconSm"
                            :title="`Remove ${row.key || 'this secret'}`"
                            :aria-label="`Remove ${row.key || 'this secret'}`"
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

            <p class="px-3 text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                Changes apply when a machine next synchronizes with this default, or a release
                rebuilds web assets with this environment. Existing builds keep their values.
            </p>
        </form>

        <template #footer>
            <div class="w-full space-y-3">
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
                        :form="formId"
                        size="sm"
                        :disabled="
                            envs.state.saving || form.name.trim() === '' || issues.length > 0
                        "
                    >
                        <Spinner v-if="envs.state.saving" tone="text-white dark:text-zinc-950" />
                        <Lock v-else class="h-3.5 w-3.5" />
                        {{ editing ? 'Save changes' : 'Store in the OS vault' }}
                    </Button>
                </div>
            </div>
        </template>
    </Modal>
</template>
