<script setup lang="ts">
// Run the app on an Android phone or emulator this host reaches over ADB. Two paths lead there
// — build the latest source, or install what is already retained — and they share one device
// and one outcome. Each path is a card carrying its own inputs and its own action, with the
// device above them and the outcome below, so no control here sits a screen away from the
// choices that change what it does. This step owns only what the paths share: which APK is
// selected, whether the device can be run on, and the live strip for a run it started itself.
import { computed, watch } from 'vue';

import { androidDeviceApks } from '../../../model/android-device';
import { androidDevicePhaseLabel } from '../../../model/phases';
import type { JourneyStep } from '../../../model/steps';
import { useBuildFlowStore } from '../../../stores/build-flow';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import ProgressRow from '../../ui/ProgressRow.vue';
import AndroidBuildRunCard from '../AndroidBuildRunCard.vue';
import AndroidDeviceStrip from '../AndroidDeviceStrip.vue';
import AndroidInspectCard from '../AndroidInspectCard.vue';
import AndroidLastRun from '../AndroidLastRun.vue';
import AndroidRetainedApkCard from '../AndroidRetainedApkCard.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const flows = useBuildFlowStore();
const apks = computed(() => androidDeviceApks(session.view?.android));
watch(
    apks,
    (available) => {
        if (!available.some((choice) => choice.value === session.androidDeviceApk)) {
            session.androidDeviceApk = available[0]?.value ?? 'debug';
        }
    },
    { immediate: true },
);
const apk = computed(() => apks.value.find((choice) => choice.value === session.androidDeviceApk));
const busy = computed(
    () => !!session.operation || !!session.view?.busyOperation || flows.active(session.id),
);
const devices = computed(() => session.androidDevices?.devices ?? []);
const deviceReady = computed(
    () =>
        !!session.androidDevices?.available &&
        devices.value.some(
            (device) => device.serial === session.androidDeviceSerial && device.state === 'device',
        ),
);
// The app is up on the device with its log streaming: the strip, the node and the button all
// say "live" rather than "loading".
const streaming = computed(() => step.live === true);
const running = computed(() => step.status === 'running');
const run = computed(() =>
    session.androidDeviceRun?.sha256 === apk.value?.artifact.sha256 &&
    session.androidDeviceRun?.serial === session.androidDeviceSerial
        ? session.androidDeviceRun
        : null,
);
const kept = computed(() => {
    const stored = session.view?.android?.deviceRun;
    return stored &&
        apks.value.some(
            (choice) => choice.artifact.sha256.toLowerCase() === stored.result.sha256.toLowerCase(),
        )
        ? stored
        : null;
});
const keptError = computed(() => session.view?.android?.deviceRunError ?? null);

/**
 * The strip belongs to the run alone: it reads that run's phases, and says nothing about an
 * operation this step does not own. A guided build and run draws its own row above the tabs,
 * which goes live for the same run, so this one stands down rather than showing the same thing
 * twice.
 */
const strip = computed(() => {
    const operation = session.operation ?? session.view?.busyOperation ?? null;
    if (
        flows.builds[session.id]?.androidDeviceSerial ||
        (operation !== 'android-run-device' && operation !== 'running_android_device')
    ) {
        return null;
    }
    const progress = session.androidDevice;
    return {
        label: streaming.value
            ? `Live on ${session.androidDeviceRun?.serial ?? session.androidDeviceSerial}`
            : progress
              ? androidDevicePhaseLabel[progress.phase]
              : 'Checking the device',
        detail: progress?.detail ?? null,
        elapsed: progress?.elapsedSeconds ?? null,
        lastLine: session.deviceLog.at(-1)?.text ?? null,
        stopTitle: streaming.value
            ? 'Ends the log session. The app stays installed and running on the device.'
            : undefined,
    };
});
</script>

<template>
    <div class="min-w-0 space-y-4" :data-step="step.id">
        <ProgressRow
            v-if="strip"
            :label="strip.label"
            :detail="strip.detail"
            :elapsed-seconds="strip.elapsed"
            :last-line="strip.lastLine"
            :state="streaming ? 'live' : 'running'"
            stoppable
            :stopping="session.cancelling"
            :stop-title="strip.stopTitle"
            @stop="machines.cancelOperation(session.id)"
        />
        <AndroidDeviceStrip :session="session" :disabled="busy" />
        <AndroidBuildRunCard :session="session" :busy="busy" :device-ready="deviceReady" />
        <AndroidRetainedApkCard
            :session="session"
            :apks="apks"
            :apk="apk"
            :busy="busy"
            :device-ready="deviceReady"
            :streaming="streaming"
            :installing="run?.status === 'installing' && !streaming"
        />
        <AndroidLastRun
            :session="session"
            :busy="busy"
            :running="running"
            :run="run"
            :kept="kept"
            :kept-error="keptError"
        />
        <AndroidInspectCard :session="session" />
    </div>
</template>
