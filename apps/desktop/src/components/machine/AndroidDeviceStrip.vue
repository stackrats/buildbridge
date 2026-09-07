<script setup lang="ts">
// The device both preview paths need, and everything that can be wrong with it. Choosing a
// phone is a precondition rather than a path of its own, so it sits above the paths once
// instead of inside either, and every message about the device is attached to the picker it is
// about rather than pooled with messages about the build.
//
// The list stays current while this is mounted: it is read on open, then host ADB is asked
// again every few seconds, so a phone that is plugged in, authorized or unplugged shows without
// a refresh. The probe is quiet, so the controls stay as they are; a listing that failed stops
// it until a refresh succeeds.
import { ExternalLink, RefreshCw } from '@lucide/vue';
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';

import { useBackend } from '../../lib/backend';
import { describeError } from '../../lib/utils';
import { androidNetworkWarning } from '../../model/android-device';
import { useMachinesStore, type MachineSession } from '../../stores/machines';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import CopyButton from '../ui/CopyButton.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import FailureBlock from '../ui/FailureBlock.vue';
import Select from '../ui/Select.vue';
import Spinner from '../ui/Spinner.vue';

const { session, disabled = false } = defineProps<{
    session: MachineSession;
    disabled?: boolean;
}>();
const machines = useMachinesStore();
const devices = computed(() => session.androidDevices?.devices ?? []);
const serial = computed({
    get: () => session.androidDeviceSerial,
    set: (value: string) => {
        session.androidDeviceSerial = value;
    },
});
// The selected phone beside this computer's network: an app calling an API served here reaches
// it from the same Wi-Fi alone, so this says so before a run rather than the app after one.
const networkWarning = computed(() =>
    androidNetworkWarning(
        devices.value.find((device) => device.serial === serial.value),
        session.androidDevices?.hostNetworks ?? [],
    ),
);
const helpError = ref<string | null>(null);
async function setupHelp(): Promise<void> {
    try {
        await useBackend().openUrl('https://developer.android.com/tools/releases/platform-tools');
    } catch (cause) {
        helpError.value = describeError(cause);
    }
}

const POLL_MS = 5_000;
let pollTimer: ReturnType<typeof setTimeout> | null = null;
function schedulePoll(): void {
    if (pollTimer) clearTimeout(pollTimer);
    pollTimer = setTimeout(() => {
        pollTimer = null;
        if (session.androidDevicesError) {
            schedulePoll();
            return;
        }
        void machines.refreshAndroidDevices(session.id, { quiet: true }).then(schedulePoll);
    }, POLL_MS);
}
onMounted(() => {
    // The first look shows the spinner and says what it is doing; a list read before is only
    // brought up to date.
    void machines.refreshAndroidDevices(session.id, { quiet: session.androidDevices !== null });
    schedulePoll();
});
onBeforeUnmount(() => {
    if (pollTimer) clearTimeout(pollTimer);
});
</script>

<template>
    <Card tone="well">
        <div class="flex flex-wrap items-center gap-2">
            <span class="min-w-0 flex-1 sm:max-w-xs">
                <Select
                    v-model="serial"
                    size="sm"
                    aria-label="Android phone or emulator"
                    placeholder="Choose an authorized device"
                    :disabled="disabled || session.androidDevicesLoading || !devices.length"
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
                variant="ghost"
                size="sm"
                :disabled="disabled || session.androidDevicesLoading"
                @click="machines.refreshAndroidDevices(session.id)"
            >
                <Spinner v-if="session.androidDevicesLoading" />
                <RefreshCw v-else class="h-3.5 w-3.5" />
                Refresh devices
            </Button>
        </div>

        <div class="mt-3 space-y-3 empty:hidden">
            <FailureBlock
                v-if="session.androidDevicesError"
                title="Devices could not be listed"
                :diagnostic="session.androidDevicesError"
            />
            <Callout
                v-if="session.androidDevices && !session.androidDevices.available"
                tone="warn"
                title="Set up ADB on this computer"
            >
                {{
                    session.androidDevices.issue ||
                    'Install Android SDK Platform-Tools and make ADB available to buildbridge.'
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
                    No devices yet. Unlock your phone and connect it with a data-capable USB cable,
                    or start an emulator on this computer; it shows here within a few seconds.
                </Callout>
                <Callout
                    v-else-if="!session.androidDevices && !session.androidDevicesError"
                    tone="neutral"
                >
                    Checking ADB on this computer for phones and emulators.
                </Callout>
            </template>
            <Callout v-if="devices.some((device) => device.state !== 'device')" tone="warn">
                For an unauthorized device, unlock it and accept the debugging prompt; reconnect an
                offline one. The list updates on its own within a few seconds.
            </Callout>
            <Callout v-if="networkWarning" tone="warn" :title="networkWarning.title">
                {{ networkWarning.message }}
            </Callout>
            <FailureBlock
                v-if="helpError"
                title="Could not open the Platform-Tools instructions"
                :diagnostic="helpError"
            />
        </div>

        <details class="mt-3">
            <DisclosureSummary quiet>ADB setup and connection help</DisclosureSummary>
            <div class="mt-2 space-y-2 text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                <p>
                    Enable USB debugging on the phone, connect it to this computer and accept its
                    authorization prompt. A running emulator on this host works too.
                </p>
                <p>
                    Install Android SDK Platform-Tools on this computer. Add its platform-tools
                    folder to the PATH available to buildbridge, or set ANDROID_HOME or
                    ANDROID_SDK_ROOT to the SDK directory, then restart buildbridge. The build
                    container's tools do not install ADB on the host.
                </p>
                <Button variant="outline" size="sm" @click="setupHelp"
                    >Platform-Tools instructions<ExternalLink class="h-3.5 w-3.5"
                /></Button>
                <div
                    class="flex items-center justify-between gap-2 rounded-md bg-white p-2 dark:bg-zinc-950"
                >
                    <code class="font-mono">adb devices</code
                    ><CopyButton text="adb devices" what="Copy the device check" size="iconXs" />
                </div>
            </div>
        </details>
    </Card>
</template>
