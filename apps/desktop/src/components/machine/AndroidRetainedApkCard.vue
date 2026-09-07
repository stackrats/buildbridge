<script setup lang="ts">
// The second way onto the phone: install what is already built. A retained APK may predate the
// latest source, so it is a card of its own rather than a second button on the first path — the
// file, what that file is, and the action that installs it stay together, and the manual command
// and the compatibility notes stay out of the way until they are asked for.
import { FolderOpen, Play } from '@lucide/vue';
import { computed } from 'vue';

import { formatBytes } from '../../lib/format';
import { androidInstallCommand, type AndroidDeviceApk } from '../../model/android-device';
import { useMachinesStore, type MachineSession } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import CopyButton from '../ui/CopyButton.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import Field from '../ui/Field.vue';
import KeyValue from '../ui/KeyValue.vue';
import Select from '../ui/Select.vue';
import Spinner from '../ui/Spinner.vue';

const { session, apks, apk, busy, deviceReady, streaming, installing } = defineProps<{
    session: MachineSession;
    apks: AndroidDeviceApk[];
    apk: AndroidDeviceApk | undefined;
    busy: boolean;
    deviceReady: boolean;
    /** The app is up with its log streaming: this path cannot start another run until it ends. */
    streaming: boolean;
    installing: boolean;
}>();
const machines = useMachinesStore();
const ui = useUi();
const selected = computed({
    get: () => session.androidDeviceApk,
    set: (value: string) => {
        session.androidDeviceApk = value === 'release' ? 'release' : 'debug';
    },
});
const command = computed(() =>
    apk ? androidInstallCommand(apk.artifact.path, session.androidDeviceSerial) : '',
);

function install(): void {
    if (!apk || busy || !deviceReady) return;
    void machines.runAndroidDevice(session.id, {
        kind: apk.value,
        serial: session.androidDeviceSerial,
        expectedSha256: apk.artifact.sha256,
    });
}
function reveal(): void {
    if (!apk) return;
    if (apk.value === 'debug') void machines.revealDebugApk(session.id);
    else void machines.revealRelease(session.id);
}
function build(): void {
    ui.selectStep(session.id, 'test-build');
}
</script>

<template>
    <Card>
        <template #title>Install a retained APK</template>
        <template #description>
            Installs a build this machine already produced, without copying source again. This works
            with the build container stopped.
        </template>
        <template v-if="apk" #actions>
            <Button
                variant="outline"
                size="sm"
                :disabled="busy || !deviceReady"
                :title="
                    streaming
                        ? 'Stop the log session to run again'
                        : 'Installs and opens the retained APK, then streams the app\'s log'
                "
                @click="install"
            >
                <Spinner v-if="installing" />
                <Play v-else class="h-3.5 w-3.5" />
                {{ streaming ? 'Running on the device' : 'Install and open' }}
            </Button>
            <Button variant="ghost" size="sm" :disabled="busy" @click="reveal">
                <FolderOpen class="h-3.5 w-3.5" />
                Show in folder
            </Button>
        </template>

        <Callout v-if="!apk" tone="neutral" title="No retained APK">
            Build and run creates a debug APK. A release built with APK selected also works; an AAB
            cannot be installed directly.
            <div class="mt-2">
                <Button variant="outline" size="sm" @click="build">Review build options</Button>
            </div>
        </Callout>
        <div v-else class="space-y-4">
            <Field
                label="APK to install"
                hint="A retained APK may predate your latest source or environment changes."
            >
                <Select
                    v-model="selected"
                    :options="
                        apks.map((choice) => ({
                            value: choice.value,
                            label: `${choice.label} · ${choice.version}`,
                        }))
                    "
                    :disabled="busy"
                />
            </Field>
            <KeyValue
                :items="[
                    { label: 'App identifier', value: apk.applicationId, mono: true },
                    { label: 'Version', value: apk.version },
                    { label: 'APK size', value: formatBytes(apk.artifact.bytes) },
                    ...(apk.value === 'release'
                        ? [
                              {
                                  label: 'Environment',
                                  value: apk.environment || 'Not recorded',
                                  copyable: false,
                              },
                          ]
                        : [
                              {
                                  label: 'HTTP APIs',
                                  value: session.view?.android?.workspace?.lastBuild?.allowHttp
                                      ? 'Allowed for testing'
                                      : 'Project settings',
                                  copyable: false,
                              },
                          ]),
                ]"
                :columns="2"
            />
            <details>
                <DisclosureSummary quiet>Manual install command</DisclosureSummary>
                <div
                    class="mt-2 flex flex-wrap items-center justify-between gap-2 rounded-md bg-zinc-50 p-3 dark:bg-zinc-950"
                >
                    <code class="min-w-0 flex-1 font-mono text-xs break-all">{{ command }}</code
                    ><CopyButton
                        v-if="!busy"
                        :text="command"
                        what="Copy the install command"
                        size="iconXs"
                    />
                </div>
                <p class="mt-2 text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    Run it in this host's terminal and wait for Success, then open the app on your
                    phone. Copying a command does not mark this step complete.
                </p>
            </details>
            <details>
                <DisclosureSummary quiet>APK details and update compatibility</DisclosureSummary>
                <div class="mt-2 space-y-3">
                    <KeyValue
                        :items="[
                            { label: 'APK path', value: apk.artifact.path, mono: true },
                            { label: 'Artifact SHA-256', value: apk.artifact.sha256, mono: true },
                        ]"
                        :columns="1"
                    />
                    <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                        Updates require compatible signing certificates. A debug APK or a Google
                        Play upload-key-signed APK may differ from the installed store app.
                        buildbridge preserves the installed app if an update is incompatible. Use a
                        separate debug app identifier to keep both versions; manually uninstalling
                        the existing app removes its local data.
                    </p>
                </div>
            </details>
        </div>
    </Card>
</template>
