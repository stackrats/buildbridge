<script setup lang="ts">
import { Download, FileKey } from '@lucide/vue';
import { onBeforeUnmount, ref, watch } from 'vue';

import { useBackend } from '../../lib/backend';
import type { SigningCredential, SigningKitSummary } from '../../types/backend';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import CopyButton from '../ui/CopyButton.vue';
import Modal from '../ui/Modal.vue';
import SecretValue from '../ui/SecretValue.vue';
import Spinner from '../ui/Spinner.vue';

const open = defineModel<boolean>('open', { default: false });
const { kit } = defineProps<{ kit: SigningKitSummary }>();
const credentials = ref<SigningCredential[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const actionError = ref<string | null>(null);
const exported = ref<string | null>(null);
const exportingId = ref<string | null>(null);
let revision = 0;

function clear(): void {
    revision += 1;
    credentials.value = [];
    loading.value = false;
    error.value = null;
    actionError.value = null;
    exported.value = null;
    exportingId.value = null;
}

async function refresh(): Promise<void> {
    clear();
    if (!open.value) return;
    const request = revision;
    loading.value = true;
    try {
        const result = await useBackend().listSigningCredentials(kit.id);
        if (request === revision) credentials.value = result;
    } catch {
        if (request === revision)
            error.value =
                'Could not read these credentials. Check that the OS vault is available, then try again.';
    } finally {
        if (request === revision) loading.value = false;
    }
}

watch(() => [open.value, kit.id], refresh, { immediate: true, flush: 'sync' });
onBeforeUnmount(clear);

async function exportFile(credential: SigningCredential): Promise<void> {
    const request = revision;
    const kitId = kit.id;
    exportingId.value = credential.id;
    actionError.value = null;
    exported.value = null;
    try {
        const path = await useBackend().pickSavePath({
            title: `Export ${credential.label}`,
            defaultPath: credential.fileName ?? `${credential.id}.bin`,
        });
        if (!path || request !== revision) return;
        await useBackend().exportSigningCredential(kitId, credential.id, path);
        if (request === revision) exported.value = credential.label;
    } catch {
        if (request === revision)
            actionError.value =
                'Could not export the file. Choose a new filename and check that the stored file is still available, then try again.';
    } finally {
        if (request === revision) exportingId.value = null;
    }
}
</script>

<template>
    <Modal
        v-model:open="open"
        :title="`Review credentials · ${kit.name}`"
        wide
        :busy="exportingId !== null"
    >
        <div class="space-y-4">
            <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                Show or copy saved values and export credential files for your backup. Keep copies
                securely, together with their passwords. Values hide after 30 seconds or when you
                leave this window. Credentials already lost from the vault or disk cannot be
                recovered here.
            </p>

            <div
                v-if="loading"
                class="flex items-center gap-2 text-xs text-zinc-500 dark:text-zinc-400"
            >
                <Spinner /> Loading saved credentials
            </div>
            <Callout v-else-if="error" tone="danger">
                {{ error }}
                <Button variant="outline" size="sm" class="mt-2" @click="refresh">Try again</Button>
            </Callout>
            <p v-else-if="!credentials.length" class="text-xs text-zinc-500 dark:text-zinc-400">
                No credential values or files are stored yet.
            </p>

            <div
                v-for="credential in credentials"
                :key="credential.id"
                class="space-y-2 rounded-lg border border-zinc-200 p-3 dark:border-zinc-800"
            >
                <SecretValue
                    v-if="credential.kind === 'secret'"
                    :label="credential.label"
                    :identity="`${kit.id}:${credential.id}`"
                    :disabled="!credential.available"
                    :load="() => useBackend().revealSigningCredential(kit.id, credential.id)"
                />
                <div v-else class="flex items-start justify-between gap-3">
                    <div class="min-w-0 space-y-1">
                        <p class="text-xs font-medium text-zinc-700 dark:text-zinc-200">
                            {{ credential.label }}
                        </p>
                        <p
                            class="font-mono text-xs break-all whitespace-pre-wrap text-zinc-600 select-text dark:text-zinc-300"
                        >
                            {{
                                credential.available
                                    ? (credential.value ?? credential.fileName ?? 'Stored file')
                                    : 'Unavailable'
                            }}
                        </p>
                    </div>
                    <CopyButton
                        v-if="credential.kind === 'value' && credential.available"
                        :text="credential.value ?? ''"
                        :what="`Copy ${credential.label}`"
                        size="iconSm"
                    />
                    <FileKey
                        v-else-if="credential.kind === 'file'"
                        class="h-4 w-4 shrink-0 text-zinc-400 dark:text-zinc-500"
                    />
                </div>
                <p v-if="!credential.available" class="text-xs text-amber-700 dark:text-amber-400">
                    This value or file is not available on this host. Restore a saved copy to use it
                    again.
                </p>
                <Button
                    v-if="credential.kind === 'file' || credential.fileName !== null"
                    variant="outline"
                    size="sm"
                    :disabled="!credential.available || exportingId !== null"
                    :aria-label="`Export ${credential.label}`"
                    @click="exportFile(credential)"
                >
                    <Spinner v-if="exportingId === credential.id" />
                    <Download v-else class="h-3.5 w-3.5" />
                    Export file
                </Button>
            </div>

            <Callout v-if="actionError" tone="danger">{{ actionError }}</Callout>
            <Callout v-if="exported" tone="ok"
                >{{ exported }} exported. Keep the copy securely with its password.</Callout
            >
        </div>
        <template #footer>
            <Button variant="outline" size="sm" @click="open = false">Close</Button>
        </template>
    </Modal>
</template>
