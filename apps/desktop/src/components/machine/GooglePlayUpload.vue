<script setup lang="ts">
import { Download, ExternalLink, FileKey, Upload } from '@lucide/vue';
import { computed, onBeforeUnmount, ref, watch } from 'vue';

import { useBackend } from '../../lib/backend';
import { describeError } from '../../lib/utils';
import { googlePlayUploadBlocker } from '../../model/google-play-upload';
import { useMachinesStore, type MachineSession } from '../../stores/machines';
import ConfirmDialog from '../dialogs/ConfirmDialog.vue';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import KeyValue from '../ui/KeyValue.vue';
import Spinner from '../ui/Spinner.vue';

const { session } = defineProps<{ session: MachineSession }>();
const machines = useMachinesStore();
const picking = ref(false);
const exporting = ref(false);
const exportNotice = ref<string | null>(null);
let exportGeneration = 0;
const error = ref<string | null>(null);
const disconnectOpen = ref(false);
const busy = computed(
    () =>
        !!session.operation ||
        !!session.view?.busyOperation ||
        session.googlePlayConnectionLoading ||
        picking.value ||
        exporting.value,
);
const blocker = computed(() =>
    googlePlayUploadBlocker(session.view, !!session.googlePlayConnection?.configured, busy.value),
);
const upload = computed(() =>
    session.googlePlayUpload?.sha256 === session.view?.android?.release?.aab?.sha256
        ? session.googlePlayUpload
        : null,
);
const uploading = computed(
    () =>
        session.operation === 'upload-google-play' ||
        session.view?.busyOperation === 'uploading_google_play',
);
watch(
    () => session.id,
    () => {
        void machines.loadGooglePlayConnection(session.id);
    },
    { immediate: true },
);
watch(
    () => [session.id, session.googlePlayConnection],
    () => {
        exportGeneration++;
        exporting.value = false;
        picking.value = false;
        exportNotice.value = null;
        error.value = null;
    },
);
onBeforeUnmount(() => exportGeneration++);

async function exportKey(): Promise<void> {
    if (busy.value || !session.googlePlayConnection?.configured) return;
    const id = session.id;
    const generation = ++exportGeneration;
    exporting.value = true;
    exportNotice.value = null;
    error.value = null;
    try {
        const path = await useBackend().pickSavePath({
            title: 'Export Google Play service account key',
            defaultPath: 'google-play-service-account.json',
            filter: { name: 'Service account JSON', extensions: ['json'] },
        });
        if (!path || generation !== exportGeneration) return;
        await useBackend().exportGooglePlayCredential(id, path);
        if (generation === exportGeneration) {
            exportNotice.value =
                'Service account key exported. Keep this private file in a secure backup.';
        }
    } catch (cause) {
        if (generation === exportGeneration) error.value = describeError(cause);
    } finally {
        if (generation === exportGeneration) exporting.value = false;
    }
}
async function importKey(): Promise<void> {
    if (busy.value) return;
    const id = session.id;
    const generation = ++exportGeneration;
    picking.value = true;
    exportNotice.value = null;
    error.value = null;
    try {
        const [path] = await useBackend().pickPaths({
            kind: 'file',
            title: 'Google Play service account key',
            filter: { name: 'Service account JSON', extensions: ['json'] },
        });
        if (path && generation === exportGeneration) await machines.configureGooglePlay(id, path);
    } catch (cause) {
        if (generation === exportGeneration) error.value = describeError(cause);
    } finally {
        if (generation === exportGeneration) picking.value = false;
    }
}
// The dialog stays up with its button spinning until the vault entry is gone; the reminder to
// export first is there because nothing can bring the key back afterwards.
async function disconnect(): Promise<void> {
    await machines.disconnectGooglePlay(session.id);
    disconnectOpen.value = false;
}
async function setupHelp(): Promise<void> {
    try {
        await useBackend().openUrl(
            'https://developers.google.com/android-publisher/getting_started',
        );
    } catch (cause) {
        error.value = describeError(cause);
    }
}
</script>

<template>
    <section class="space-y-3" aria-label="Google Play upload">
        <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
            Upload this retained AAB as a draft on the internal testing track. Review and finish the
            rollout in Play Console.
        </p>
        <Callout v-if="error || session.googlePlayConnectionError" tone="danger">{{
            error || session.googlePlayConnectionError
        }}</Callout>
        <Callout v-if="exportNotice" tone="ok" role="status">{{ exportNotice }}</Callout>
        <KeyValue
            v-if="session.googlePlayConnection?.configured"
            :items="[
                {
                    label: 'Service account',
                    value: session.googlePlayConnection.clientEmail,
                    mono: true,
                },
                { label: 'Cloud project', value: session.googlePlayConnection.projectId },
            ]"
        />
        <div class="flex flex-wrap gap-2">
            <Button
                :disabled="!!blocker || upload?.status === 'uploaded'"
                @click="machines.uploadGooglePlay(session.id)"
                ><Spinner v-if="uploading" /><Upload v-else class="h-3.5 w-3.5" />{{
                    uploading ? 'Uploading to Google Play' : 'Upload to Google Play'
                }}</Button
            >
            <Button variant="outline" :disabled="busy" @click="importKey"
                ><Spinner v-if="picking || session.googlePlayConnectionLoading" /><FileKey
                    v-else
                    class="h-3.5 w-3.5"
                />{{
                    session.googlePlayConnection?.configured
                        ? 'Replace service account'
                        : 'Import service account JSON'
                }}</Button
            >
            <Button
                v-if="session.googlePlayConnection?.configured"
                variant="outline"
                :disabled="busy"
                @click="exportKey"
                ><Spinner v-if="exporting" /><Download v-else class="h-3.5 w-3.5" />Export service
                account key</Button
            >
            <Button
                v-if="session.googlePlayConnection?.configured"
                variant="ghost"
                :disabled="busy"
                @click="disconnectOpen = true"
                >Disconnect</Button
            >
            <Button
                v-else-if="session.googlePlayConnectionError"
                variant="outline"
                :disabled="busy"
                @click="machines.loadGooglePlayConnection(session.id)"
                >Recheck credentials</Button
            >
        </div>
        <p
            v-if="blocker && !uploading && upload?.status !== 'uploaded'"
            class="text-xs leading-5 text-zinc-500 dark:text-zinc-400"
        >
            {{ blocker }}
        </p>
        <div aria-live="polite">
            <Callout v-if="uploading" tone="info"
                >Sending the AAB and saving the internal testing draft. You can switch pages while
                it runs.</Callout
            >
            <Callout v-else-if="upload?.status === 'uploaded'" tone="ok"
                >Version {{ upload.result?.versionCode }} uploaded as an internal testing draft.
                Complete release notes, testers and rollout in Play Console.</Callout
            >
            <Callout
                v-else-if="upload?.status === 'failed'"
                tone="danger"
                title="Upload did not finish"
                ><p>{{ upload.error }}</p>
                <p class="mt-1">
                    Check Play Console before retrying; Google may already have received this
                    version.
                </p></Callout
            >
        </div>
        <details>
            <DisclosureSummary class="text-xs font-medium text-zinc-600 dark:text-zinc-300"
                >Google Play connection setup</DisclosureSummary
            >
            <div class="mt-2 space-y-2 text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                <p>
                    Set up this app and its internal testing track in Play Console first. Enable the
                    Google Play Developer API in your Cloud project, create a service account, and
                    invite its email under Play Console Users and permissions with access to this
                    app and testing releases.
                </p>
                <p>
                    Import its JSON key here. BuildBridge stores the key in your operating system's
                    credential vault for this machine.
                </p>
                <p v-if="session.googlePlayConnection?.configured">
                    Export the saved key before disconnecting or deleting this machine if you need a
                    backup. The JSON contains the saved key and account details, with Google's
                    authentication addresses; it is reconstructed from the vault.
                </p>
                <p>
                    The retained AAB must use this app's package name, accepted upload key and an
                    unused version code. Store listing, app content and account requirements are
                    completed in Play Console.
                </p>
                <Button variant="outline" size="sm" @click="setupHelp"
                    ><ExternalLink class="h-3.5 w-3.5" />Google API setup instructions</Button
                >
            </div>
        </details>
        <ConfirmDialog
            v-model:open="disconnectOpen"
            title="Disconnect Google Play"
            confirm-label="Disconnect"
            :busy="session.googlePlayConnectionLoading"
            @confirm="disconnect"
        >
            <p>
                The service account key is removed from the operating-system vault for this machine,
                and uploads from here stop until a key is imported again.
            </p>
            <p>
                Export the service account key first if you need a backup; it cannot be recovered.
            </p>
        </ConfirmDialog>
    </section>
</template>
