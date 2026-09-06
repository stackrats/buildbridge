<script setup lang="ts">
import { Upload } from '@lucide/vue';
import { computed } from 'vue';

import { appleUploadBlocker } from '../../model/apple-upload';
import { useMachinesStore, type MachineSession } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Spinner from '../ui/Spinner.vue';

const { session } = defineProps<{ session: MachineSession }>();
const machines = useMachinesStore();
const ui = useUi();
const blocker = computed(() => appleUploadBlocker(session.view, session.operation !== null));
const upload = computed(() =>
    session.archiveUpload?.sha256 === session.view?.archive?.ipa.sha256
        ? session.archiveUpload
        : null,
);
const uploading = computed(
    () =>
        session.operation === 'upload-archive' ||
        session.view?.busyOperation === 'uploading_archive',
);
</script>

<template>
    <section class="space-y-3" aria-label="Transporter upload">
        <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
            Upload this retained IPA from {{ session.view?.profile.name }} using its attached App
            Store Connect Team API key. The key must have permission to upload to this app.
        </p>
        <div class="flex flex-wrap gap-2">
            <Button
                :disabled="!!blocker || upload?.status === 'uploaded'"
                @click="machines.uploadAppleArchive(session.id)"
            >
                <Spinner v-if="uploading" />
                <Upload v-else class="h-3.5 w-3.5" />
                {{ uploading ? 'Uploading with Transporter' : 'Upload with Transporter' }}
            </Button>
            <Button
                v-if="blocker?.action === 'start'"
                variant="outline"
                :disabled="!session.view?.runtime.prerequisites.ready"
                @click="machines.launch(session.id)"
                >Start machine</Button
            >
            <Button
                v-else-if="blocker?.action === 'credentials'"
                variant="outline"
                @click="ui.openMachine(session.id, 'signing-kit')"
                >{{
                    session.view?.signingKit
                        ? 'Review signing credentials'
                        : 'Choose signing credentials'
                }}</Button
            >
            <Button
                v-else-if="blocker?.action === 'guest'"
                variant="outline"
                @click="
                    ui.openMachine(
                        session.id,
                        session.view?.guest.ssh.trust === 'trusted' ? 'access' : 'trust',
                    )
                "
                >Set up guest access</Button
            >
        </div>
        <p
            v-if="blocker && !uploading && upload?.status !== 'uploaded'"
            class="text-xs leading-5 text-zinc-500 dark:text-zinc-400"
        >
            {{ blocker.message }}
        </p>
        <div aria-live="polite">
            <Callout v-if="uploading" tone="info">
                Transporter is delivering the IPA to Apple. You can switch pages while it runs.
            </Callout>
            <Callout v-else-if="upload?.status === 'uploaded'" tone="ok">
                Uploaded to App Store Connect. Apple still needs to process the build.
            </Callout>
            <Callout
                v-else-if="upload?.status === 'failed'"
                tone="danger"
                title="Upload did not finish"
            >
                <p>{{ upload.error }}</p>
                <p class="mt-1">
                    Check App Store Connect before retrying; Apple may already have received the
                    build.
                </p>
            </Callout>
        </div>
    </section>
</template>
