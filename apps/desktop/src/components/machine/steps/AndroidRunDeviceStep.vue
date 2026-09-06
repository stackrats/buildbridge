<script setup lang="ts">
// Run a debug build on an Android phone or emulator this host reaches over ADB. The panel has the
// iPhone step's shape: the device and the primary action at the top, the state under them, and the
// choices and instructions below the hairline.
import { ExternalLink, FolderOpen, Hammer, Play, RefreshCw } from '@lucide/vue';
import { computed, ref, watch } from 'vue';

import { useBackend } from '../../../lib/backend';
import { formatBytes } from '../../../lib/format';
import { describeError } from '../../../lib/utils';
import { androidDeviceApks, androidInstallCommand } from '../../../model/android-device';
import { buildPrerequisite } from '../../../model/build-flow';
import type { JourneyStep } from '../../../model/steps';
import { useBuildFlowStore } from '../../../stores/build-flow';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import GuidedBuildStatus from '../GuidedBuildStatus.vue';
import AndroidHttpOption from '../AndroidHttpOption.vue';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import CopyButton from '../../ui/CopyButton.vue';
import DisclosureSummary from '../../ui/DisclosureSummary.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import Field from '../../ui/Field.vue';
import KeyValue from '../../ui/KeyValue.vue';
import Select from '../../ui/Select.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const flows = useBuildFlowStore();
const ui = useUi();
const apks = computed(() => androidDeviceApks(session.view?.android));
const selected = computed({
    get: () => session.androidDeviceApk,
    set: (value: string) => {
        session.androidDeviceApk = value === 'release' ? 'release' : 'debug';
    },
});
const serial = computed({
    get: () => session.androidDeviceSerial,
    set: (value: string) => {
        session.androidDeviceSerial = value;
    },
});
watch(
    apks,
    (available) => {
        if (!available.some((apk) => apk.value === selected.value))
            selected.value = available[0]?.value ?? 'debug';
    },
    { immediate: true },
);
const apk = computed(() => apks.value.find((apk) => apk.value === selected.value));
const busy = computed(
    () => !!session.operation || !!session.view?.busyOperation || flows.active(session.id),
);
const devices = computed(() => session.androidDevices?.devices ?? []);
const deviceReady = computed(
    () =>
        !!session.androidDevices?.available &&
        devices.value.some((device) => device.serial === serial.value && device.state === 'device'),
);
const command = computed(() =>
    apk.value ? androidInstallCommand(apk.value.artifact.path, serial.value) : '',
);
const run = computed(() =>
    session.androidDeviceRun?.sha256 === apk.value?.artifact.sha256 &&
    session.androidDeviceRun?.serial === serial.value
        ? session.androidDeviceRun
        : null,
);
const preview = computed(() =>
    flows.builds[session.id]?.androidDeviceSerial ? flows.builds[session.id] : null,
);
const prerequisite = computed(() => (session.view ? buildPrerequisite(session.view) : null));
const error = ref<string | null>(null);
const inspectorUrl = ref('chrome://inspect/#devices');
const inspectorOpening = ref(false);
const inspectorError = ref<string | null>(null);
// The state band only draws when there is state to show, so a ready device with an APK and
// nothing in flight has no empty space between the actions and the choices.
const showStatus = computed(
    () =>
        !!(session.androidDevicesError || error.value) ||
        !session.androidDevices ||
        !session.androidDevices.available ||
        !!session.androidDevices.issue ||
        devices.value.length === 0 ||
        devices.value.some((device) => device.state !== 'device') ||
        (prerequisite.value !== null && !preview.value) ||
        preview.value !== null ||
        !apk.value ||
        run.value !== null ||
        inspectorError.value !== null,
);
async function openInspector(): Promise<void> {
    if (inspectorOpening.value) return;
    inspectorOpening.value = true;
    inspectorError.value = null;
    try {
        inspectorUrl.value = await useBackend().openAndroidInspector();
    } catch (cause) {
        inspectorError.value = describeError(cause);
    } finally {
        inspectorOpening.value = false;
    }
}
async function setupHelp(): Promise<void> {
    try {
        await useBackend().openUrl('https://developer.android.com/tools/releases/platform-tools');
    } catch (cause) {
        error.value = describeError(cause);
    }
}
function install(): void {
    if (!apk.value || busy.value || !deviceReady.value) return;
    void machines.runAndroidDevice(session.id, {
        kind: apk.value.value,
        serial: serial.value,
        expectedSha256: apk.value.artifact.sha256,
    });
}
function build(): void {
    flows.draft(session.id).outcome = 'test';
    ui.state.machineSections[session.id] = 'build';
}
function reveal(): void {
    if (!apk.value) return;
    if (apk.value.value === 'debug') void machines.revealDebugApk(session.id);
    else void machines.revealRelease(session.id);
}
function revealPreview(): void {
    const sha256 = preview.value?.androidPreviewSha256;
    if (!sha256 || sha256 !== session.view?.android?.workspace?.lastBuild?.apk?.sha256) {
        error.value = 'The preview APK has been replaced. Review the current retained debug build.';
        return;
    }
    void machines.revealDebugApk(session.id);
}
</script>

<template>
    <StepPanel :step="step" details-label="APK details and update compatibility">
        <template #action>
            <span v-if="devices.length" class="w-64 max-w-full">
                <Select
                    v-model="serial"
                    size="sm"
                    aria-label="Android phone or emulator"
                    placeholder="Choose an authorized device"
                    :disabled="busy || session.androidDevicesLoading"
                    :options="
                        devices.map((device) => ({
                            value: device.serial,
                            label: `${device.model || device.serial} · ${device.state}`,
                            description: device.model ? device.serial : undefined,
                            disabled: device.state !== 'device',
                        }))
                    "
                />
            </span>
            <Button
                size="sm"
                :disabled="busy || session.androidDevicesLoading || !deviceReady"
                @click="flows.startAndroidPreview(session.id, serial)"
            >
                <Hammer class="h-3.5 w-3.5" />
                Build and run
            </Button>
            <Button
                v-if="apk"
                variant="outline"
                size="sm"
                :disabled="busy || session.androidDevicesLoading || !deviceReady"
                @click="install"
            >
                <Spinner v-if="run?.status === 'installing'" />
                <Play v-else class="h-3.5 w-3.5" />
                Install and open
            </Button>
            <Button
                v-if="prerequisite"
                variant="outline"
                size="sm"
                :disabled="busy"
                @click="ui.openMachine(session.id, prerequisite.step)"
            >
                {{ prerequisite.action }}
            </Button>
            <Button
                variant="ghost"
                size="sm"
                :disabled="busy || session.androidDevicesLoading"
                @click="machines.refreshAndroidDevices(session.id)"
            >
                <Spinner v-if="session.androidDevicesLoading" />
                <RefreshCw v-else class="h-3.5 w-3.5" />
                Refresh devices
            </Button>
            <Button variant="ghost" size="sm" :disabled="inspectorOpening" @click="openInspector">
                <Spinner v-if="inspectorOpening" />
                <ExternalLink v-else class="h-3.5 w-3.5" />
                Open inspector
            </Button>
            <Button v-if="apk" variant="ghost" size="sm" :disabled="busy" @click="reveal">
                <FolderOpen class="h-3.5 w-3.5" />
                Show in folder
            </Button>
        </template>

        <template v-if="showStatus" #status>
            <FailureBlock
                v-if="session.androidDevicesError || error"
                :title="
                    session.androidDevicesError
                        ? 'Devices could not be listed'
                        : 'The step did not complete'
                "
                :diagnostic="session.androidDevicesError || error"
            />
            <Callout
                v-if="session.androidDevices && !session.androidDevices.available"
                tone="warn"
                title="Set up ADB on this computer"
            >
                {{
                    session.androidDevices.issue ||
                    'Install Android SDK Platform-Tools and make ADB available to BuildBridge.'
                }}
                <div class="mt-2">
                    <Button variant="outline" size="sm" @click="setupHelp">
                        <ExternalLink class="h-3.5 w-3.5" />
                        Get Platform-Tools
                    </Button>
                </div>
            </Callout>
            <Callout v-else-if="session.androidDevices?.issue" tone="warn">
                {{ session.androidDevices.issue }}
            </Callout>
            <template v-if="!devices.length">
                <Callout v-if="session.androidDevices?.available" tone="neutral">
                    No devices found. Unlock your phone, use a data-capable USB cable and refresh.
                    For an emulator, start it on this computer first.
                </Callout>
                <Callout v-else-if="!session.androidDevices" tone="neutral">
                    Refresh devices to check ADB and find connected phones and emulators.
                </Callout>
            </template>
            <Callout v-if="devices.some((device) => device.state !== 'device')" tone="warn">
                For an unauthorized device, unlock it and accept the debugging prompt. Reconnect
                offline devices, then refresh.
            </Callout>
            <Callout v-if="prerequisite && !preview" tone="neutral">
                {{ prerequisite.message }}
            </Callout>
            <GuidedBuildStatus
                v-if="preview"
                :session="session"
                :now="Date.now()"
                @review="ui.openMachine(session.id, $event)"
                @results="revealPreview"
            />
            <Callout v-if="!apk" tone="neutral" title="No retained APK">
                Build and run creates a debug APK. A release built with APK selected also works; an
                AAB cannot be installed directly.
                <div class="mt-2">
                    <Button variant="outline" size="sm" @click="build">Review build options</Button>
                </div>
            </Callout>
            <div v-if="run" aria-live="polite">
                <Callout v-if="run.status === 'installing'" tone="info">
                    Installing and opening the selected APK. You can switch pages while it runs.
                </Callout>
                <Callout v-else-if="run.status === 'complete'" tone="ok">
                    Installed and opened {{ run.result?.applicationId }} on {{ run.serial }}.
                </Callout>
                <FailureBlock
                    v-else-if="run.status === 'failed'"
                    title="Installation did not finish"
                    :diagnostic="run.error"
                />
            </div>
            <FailureBlock
                v-if="inspectorError"
                title="Could not open inspector"
                :diagnostic="inspectorError"
            />
        </template>

        <div class="space-y-5">
            <section class="space-y-3">
                <h3 class="text-sm font-semibold">Choose a device</h3>
                <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                    Enable USB debugging, connect your phone to this computer and accept its
                    authorization prompt. A running emulator on this host works too.
                </p>
                <details>
                    <DisclosureSummary class="text-xs font-medium text-zinc-600 dark:text-zinc-300">
                        ADB setup and connection help
                    </DisclosureSummary>
                    <div class="mt-2 space-y-2 text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                        <p>
                            Install Android SDK Platform-Tools on this computer. Add its
                            platform-tools folder to the PATH available to BuildBridge, or set
                            ANDROID_HOME or ANDROID_SDK_ROOT to the SDK directory, then restart
                            BuildBridge. The build container's tools do not install ADB on the host.
                        </p>
                        <Button variant="outline" size="sm" @click="setupHelp"
                            >Platform-Tools instructions<ExternalLink class="h-3.5 w-3.5"
                        /></Button>
                        <div
                            class="flex items-center justify-between gap-2 rounded-md bg-zinc-50 p-2 dark:bg-zinc-950"
                        >
                            <code>adb devices</code
                            ><CopyButton text="adb devices" label="Copy device check" />
                        </div>
                    </div>
                </details>
            </section>
            <section class="space-y-3">
                <h3 class="text-sm font-semibold">Build and run the latest source</h3>
                <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    Copy your approved local project, build a debug APK, then install and open it on
                    the selected device. Environment:
                    {{ session.view?.envSet?.name || 'No environment' }}.
                </p>
                <AndroidHttpOption :session="session" :disabled="busy" />
            </section>
            <section v-if="apk" class="space-y-3">
                <h3 class="text-sm font-semibold">Or install a retained APK</h3>
                <Field
                    label="APK to install"
                    hint="A retained APK may predate your latest source or environment changes. Installation works with the build container stopped."
                >
                    <Select
                        v-model="selected"
                        :options="
                            apks.map((apk) => ({
                                value: apk.value,
                                label: `${apk.label} · ${apk.version}`,
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
                                  },
                              ]
                            : [
                                  {
                                      label: 'HTTP APIs',
                                      value: session.view?.android?.workspace?.lastBuild?.allowHttp
                                          ? 'Allowed for testing'
                                          : 'Project settings',
                                  },
                              ]),
                    ]"
                />
                <details>
                    <DisclosureSummary class="text-xs font-medium text-zinc-600 dark:text-zinc-300"
                        >Manual install command</DisclosureSummary
                    >
                    <div
                        class="mt-2 flex flex-wrap items-center justify-between gap-2 rounded-md bg-zinc-50 p-3 dark:bg-zinc-950"
                    >
                        <code class="min-w-0 flex-1 text-xs break-all">{{ command }}</code
                        ><CopyButton v-if="!busy" :text="command" label="Copy install command" />
                    </div>
                    <p class="mt-2 text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                        Run it in this host's terminal and wait for Success, then open the app on
                        your phone. Copying a command does not mark this step complete.
                    </p>
                </details>
            </section>
            <section class="space-y-3">
                <h3 class="text-sm font-semibold">Inspect the running app</h3>
                <div class="group flex min-w-0 items-center gap-0.5">
                    <code class="text-xs break-all text-zinc-500 dark:text-zinc-400">{{
                        inspectorUrl
                    }}</code>
                    <CopyButton
                        :text="inspectorUrl"
                        what="Copy the inspector URL"
                        size="iconXs"
                        class="opacity-45 group-hover:opacity-100"
                    />
                </div>
                <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    Opens a separate browser window. Keep the app open on your phone. In Chrome,
                    Chromium or Edge, enable
                    <strong>Discover USB devices</strong>, then choose <strong>Inspect</strong>
                    beside your app's WebView. The app must allow WebView debugging; release apps
                    usually do not appear.
                </p>
            </section>
        </div>
        <template v-if="apk" #details>
            <KeyValue
                :items="[
                    { label: 'APK path', value: apk.artifact.path, mono: true },
                    { label: 'Artifact SHA-256', value: apk.artifact.sha256, mono: true },
                ]"
                :columns="1"
            />
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Updates require compatible signing certificates. A debug APK or a Google Play
                upload-key-signed APK may differ from the installed store app. BuildBridge preserves
                the installed app if an update is incompatible. Use a separate debug app identifier
                to keep both versions; manually uninstalling the existing app removes its local
                data.
            </p>
        </template>
    </StepPanel>
</template>
