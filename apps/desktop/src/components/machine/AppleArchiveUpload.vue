<script setup lang="ts">
import { Download, ExternalLink, Upload } from '@lucide/vue';
import { computed } from 'vue';

import { useBackend } from '../../lib/backend';
import { appleUploadBlocker, TRANSPORTER_PACKAGE_URL } from '../../model/apple-upload';
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
const installing = computed(
    () =>
        session.operation === 'install-transporter' ||
        session.view?.busyOperation === 'installing_transporter',
);
function openTransporterPackage(): void {
    void useBackend().openUrl(TRANSPORTER_PACKAGE_URL);
}
</script>

<template>
    <section class="space-y-3" aria-label="Transporter upload">
        <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
            Upload this retained IPA from {{ session.view?.profile.name }} using its attached App
            Store Connect Team API key. The key must have permission to upload to this app.
        </p>
        <div class="flex flex-wrap gap-2">
            <Button
                size="sm"
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
                size="sm"
                :disabled="!session.view?.runtime.prerequisites.ready"
                @click="machines.launch(session.id)"
                >Start machine</Button
            >
            <Button
                v-else-if="blocker?.action === 'credentials'"
                variant="outline"
                size="sm"
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
                size="sm"
                @click="
                    ui.openMachine(
                        session.id,
                        session.view?.guest.ssh.trust === 'trusted' ? 'access' : 'trust',
                    )
                "
                >Set up guest access</Button
            >
            <Button
                v-else-if="blocker?.action === 'transporter' || installing"
                variant="outline"
                size="sm"
                :disabled="installing"
                @click="machines.installTransporter(session.id)"
            >
                <Spinner v-if="installing" />
                <Download v-else class="h-3.5 w-3.5" />
                {{ installing ? 'Installing Transporter' : 'Install Transporter' }}
            </Button>
        </div>
        <p
            v-if="blocker && !uploading && !installing && upload?.status !== 'uploaded'"
            class="text-xs leading-5 text-zinc-500 dark:text-zinc-400"
        >
            {{ blocker.message }}
        </p>
        <p
            v-if="blocker?.action === 'transporter' && !installing"
            class="text-xs leading-5 text-zinc-500 dark:text-zinc-400"
        >
            Installing accepts Apple's Transporter licence, which Apple's own installer would show
            first. To read it beforehand,
            <button
                type="button"
                class="inline-flex items-center gap-1 text-zinc-500 underline decoration-zinc-300 underline-offset-2 hover:text-zinc-800 dark:text-zinc-400 dark:decoration-zinc-600 dark:hover:text-zinc-100"
                @click="openTransporterPackage"
            >
                download Apple's package
                <ExternalLink class="h-3 w-3" aria-hidden="true" />
            </button>
            and open it on a Mac; Installer shows the licence before it installs anything.
        </p>
        <div aria-live="polite">
            <Callout v-if="uploading" tone="info">
                Transporter is delivering the IPA to Apple. You can switch pages while it runs.
            </Callout>
            <Callout v-else-if="installing" tone="info">
                The macOS machine is downloading Apple's Transporter package and checking Apple's
                signature before unpacking it. You can switch pages while it runs.
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
