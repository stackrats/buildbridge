<script setup lang="ts">
// Run a Debug build on a phone plugged into this host. The step is a ladder — the host lets the
// phone go, the container can take it, the phone is attached, trusted, signed for, in Developer
// Mode, and then it runs with its console in the drawer — and the ladder is the phone's, not the
// build's. So the phone and its rungs are one card carrying the ladder's single action, the run
// and the choices that change it are another carrying its own, and the outcome and the inspector
// are their own below them. This file owns only what they share: the readiness the ladder is read
// from, the one primary action it decides, the live strip, and the confirmations.
import { Cable, HardDrive, Play, RefreshCw, ShieldCheck, Unplug, Usb } from '@lucide/vue';
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';

import type { ListboxOption } from '../../../lib/listbox';
import { isLive } from '../../../lib/status';
import { percent } from '../../../lib/format';
import { deviceChecks, deviceNextSummary, deviceReadiness } from '../../../model/device';
import {
    devicePhaseLabel,
    deviceSigningPhaseLabel,
    containerRebuildPhaseLabel,
    usbAttachPhaseLabel,
    usbMigrationPhaseLabel,
} from '../../../model/phases';
import { requestedVersion } from '../../../model/build-flow';
import { hasWebAssets } from '../../../model/project-layout';
import type { JourneyStep } from '../../../model/steps';
import { useBuildFlowStore } from '../../../stores/build-flow';
import { useEnvSetsStore } from '../../../stores/envs';
import {
    activityLabel,
    useMachinesStore,
    type MachineSession,
    type OperationId,
} from '../../../stores/machines';
import ConfirmDialog from '../../dialogs/ConfirmDialog.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import DeviceAttachCard from '../DeviceAttachCard.vue';
import DeviceInspectCard from '../DeviceInspectCard.vue';
import DeviceLastRun from '../DeviceLastRun.vue';
import DevicePrimaryButton, { type DevicePrimary } from '../DevicePrimaryButton.vue';
import DeviceRunCard from '../DeviceRunCard.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
// The version fields share the machine's build draft with the build page and the test step.
const draft = useBuildFlowStore().draft(session.id);
// The env is chosen per run: the web assets are rebuilt inside the guest with it before the
// Debug build, without changing what the next build starts from. Preselects the set the last
// copy used.
const envs = useEnvSetsStore();
const attachedEnvSet = computed(() => session.view?.envSet ?? null);
const envSetId = ref(attachedEnvSet.value?.id ?? '');
watch(attachedEnvSet, (set, previous) => {
    if (envSetId.value === (previous?.id ?? '')) {
        envSetId.value = set?.id ?? '';
    }
});
onMounted(() => void envs.load());
const webAssets = computed(() => hasWebAssets(session.view?.appleWorkspace?.layout));
const envOptions = computed<ListboxOption[]>(() => [
    {
        value: '',
        label: webAssets.value ? 'Use prepared assets' : 'Project configuration only',
        description: webAssets.value
            ? 'Keep web assets from the most recent build, including their environment values.'
            : 'Build with the environment the last copy or build left in place.',
    },
    ...envs.sets.value.map((set) => ({
        value: set.id,
        label: set.name,
        description:
            set.id === attachedEnvSet.value?.id
                ? webAssets.value
                    ? 'Rebuild web assets · the set the last copy used'
                    : 'Export to the build · the set the last copy used'
                : webAssets.value
                  ? 'Rebuild web assets with this environment'
                  : 'Export this environment to the build',
    })),
]);

const view = computed(() => session.view!);
const usb = computed(() => view.value.usb);
const readiness = computed(() => deviceReadiness(view.value));
const device = computed(() => readiness.value.device);
const run = computed(() => view.value.deviceRun);
const signing = computed(() => view.value.signing);
const workspace = computed(() => view.value.appleWorkspace);
const live = computed(() => isLive(view.value.runtime.state));
const busy = computed(() => session.operation !== null);
const running = computed(() => step.status === 'running');
// The app is up on the phone with its console streaming: the run has arrived, and nothing here
// is waiting, so the strip, the node, and the button all say "live" rather than "loading".
const streaming = computed(() => step.live === true);
const ready = computed(() => readiness.value.substate === 'ready');

const stepOperations = new Set<OperationId>([
    'usb-rule',
    'usb-migrate',
    'usb-attach',
    'usb-detach',
    'device-signing',
    'run-device',
]);
const failureTitle: Partial<Record<OperationId, string>> = {
    'usb-rule': 'The host could not be prepared',
    'usb-migrate': 'USB could not be enabled on this machine',
    'usb-attach': 'The iPhone could not be attached',
    'usb-detach': 'The iPhone could not be detached',
    'device-signing': 'Device signing could not be prepared',
    'run-device': 'The device run failed',
};
const failure = computed(() =>
    session.lastFailure && stepOperations.has(session.lastFailure.operation)
        ? session.lastFailure
        : null,
);
// A failure belongs to the card whose job it was: getting the phone ready, or running on it.
const phoneFailure = computed(() =>
    failure.value && failure.value.operation !== 'run-device' ? failure.value : null,
);
const runFailure = computed(() =>
    failure.value?.operation === 'run-device' ? failure.value : null,
);

// Which phone on the host to hand over; preselected when there is exactly one usable.
const usbOptions = computed<ListboxOption[]>(() =>
    usb.value.host.devices.map((candidate) => ({
        value: `${candidate.bus}:${candidate.port}`,
        label: `${candidate.product ?? 'Apple device'} · bus ${candidate.bus} port ${candidate.port}${
            candidate.nodeReady ? '' : ' · replug'
        }`,
        disabled: !candidate.nodeReady,
    })),
);
const selectedUsb = ref('');
watch(
    usbOptions,
    (options) => {
        if (!options.some((option) => option.value === selectedUsb.value)) {
            selectedUsb.value =
                options.find((option) => !option.disabled)?.value ?? options[0]?.value ?? '';
        }
    },
    { immediate: true },
);

const migrateOpen = ref(false);
const rebuildOpen = ref(false);
const signingOpen = ref(false);
const clearOpen = ref(false);

/** The live strip reads whichever progress belongs to the operation in flight. */
const strip = computed(() => {
    const operation = session.operation ?? view.value.busyOperation;
    // The background poll of the guest's device list is a probe, not an operation: a row that
    // appears for its two seconds and vanishes every five is a page that jumps.
    if (!operation || operation === 'listing_devices' || operation === 'refresh') {
        return null;
    }
    if (operation === 'usb-migrate' || operation === 'migrating_usb') {
        const progress = session.usbMigration;
        return {
            label: progress ? usbMigrationPhaseLabel[progress.phase] : 'Preparing',
            detail: progress?.detail ?? null,
            elapsed: progress?.elapsedSeconds ?? null,
            value: progress ? percent(progress.completedBytes, progress.totalBytes) : null,
            completedBytes: progress?.completedBytes ?? null,
            totalBytes: progress?.totalBytes ?? null,
            lastLine: null,
            stoppable: false,
            stopTitle: undefined,
        };
    }
    if (operation === 'usb-attach' || operation === 'settling_phone') {
        const progress = session.usbAttach;
        return {
            label: progress ? usbAttachPhaseLabel[progress.phase] : 'Passing the iPhone to QEMU',
            detail: progress?.detail ?? null,
            elapsed: progress?.elapsedSeconds ?? null,
            value: null,
            completedBytes: null,
            totalBytes: null,
            lastLine: null,
            stoppable: false,
            stopTitle: undefined,
        };
    }
    if (operation === 'usb-rebuild' || operation === 'rebuilding_container') {
        const progress = session.rebuild;
        return {
            label: progress ? containerRebuildPhaseLabel[progress.phase] : 'Preparing',
            detail: progress?.detail ?? null,
            elapsed: progress?.elapsedSeconds ?? null,
            value: null,
            completedBytes: null,
            totalBytes: null,
            lastLine: null,
            stoppable: false,
            stopTitle: undefined,
        };
    }
    if (operation === 'device-signing' || operation === 'preparing_device_signing') {
        const progress = session.deviceSigning ?? null;
        return {
            label: progress
                ? deviceSigningPhaseLabel[progress.phase]
                : 'Checking the signing credentials',
            detail: progress?.detail ?? session.signing?.detail ?? null,
            elapsed: progress?.elapsedSeconds ?? null,
            value: null,
            completedBytes: null,
            totalBytes: null,
            lastLine: null,
            stoppable: true,
            stopTitle: undefined,
        };
    }
    if (operation === 'run-device' || operation === 'running_on_device') {
        const progress = session.device;
        return {
            label: streaming.value
                ? `Live on ${readiness.value.name}`
                : progress
                  ? devicePhaseLabel[progress.phase]
                  : 'Preparing the recipe',
            detail: progress?.detail ?? null,
            elapsed: progress?.elapsedSeconds ?? null,
            value: null,
            completedBytes: null,
            totalBytes: null,
            lastLine: session.deviceLog.at(-1)?.text ?? null,
            stoppable: true,
            stopTitle: streaming.value
                ? 'Ends the console session. The app stays installed and running on the iPhone.'
                : undefined,
        };
    }
    return {
        label: activityLabel(operation) ?? 'Working',
        detail:
            operation === 'device-pair' || operation === 'pairing_device'
                ? 'unlock the phone and tap Trust when it asks'
                : null,
        elapsed: null,
        value: null,
        completedBytes: null,
        totalBytes: null,
        lastLine: null,
        stoppable: false,
        stopTitle: undefined,
    };
});

/** One primary action per rung; the ladder decides, the button only names it. */
const primary = computed<DevicePrimary | null>(() => {
    const id = session.id;
    const name = readiness.value.name;
    const needsLive = (reason = 'Start the machine first') => (live.value ? null : reason);
    switch (readiness.value.substate) {
        case 'host-rule':
            return {
                label:
                    usb.value.host.rule === 'modified'
                        ? 'Reinstall the host USB rule'
                        : 'Prepare the host for USB',
                icon: Usb,
                outline: false,
                operation: 'usb-rule',
                disabledReason: usb.value.host.supported
                    ? null
                    : 'USB passthrough needs a Linux host with sysfs',
                run: () => machines.installUsbRule(id),
            };
        case 'container':
            if (view.value.runtime.state === 'missing') {
                return {
                    label: 'Start the machine',
                    icon: Play,
                    outline: false,
                    operation: 'launch',
                    disabledReason: null,
                    run: () => machines.launch(id),
                };
            }
            if (usb.value.diskOnHost && !usb.value.phoneController) {
                return {
                    label: 'Rebuild the container',
                    icon: HardDrive,
                    outline: false,
                    operation: 'usb-rebuild',
                    disabledReason: null,
                    run: () => (rebuildOpen.value = true),
                };
            }
            return {
                label: 'Enable USB on this machine',
                icon: HardDrive,
                outline: false,
                operation: 'usb-migrate',
                disabledReason: null,
                run: () => (migrateOpen.value = true),
            };
        case 'plug-in':
            return {
                label: 'Check again',
                icon: RefreshCw,
                outline: true,
                operation: 'refresh',
                disabledReason: null,
                run: () => machines.refreshMachine(id),
            };
        case 'attach':
            return {
                label: 'Attach iPhone',
                icon: Cable,
                outline: false,
                operation: 'usb-attach',
                disabledReason: needsLive() ?? (selectedUsb.value ? null : 'Choose a phone'),
                run: () => {
                    const [bus, port] = selectedUsb.value.split(':');
                    return machines.attachUsb(id, Number(bus), port ?? '');
                },
            };
        case 'unplugged':
            return {
                label: 'Detach iPhone',
                icon: Unplug,
                outline: true,
                operation: 'usb-detach',
                disabledReason: needsLive(),
                run: () => machines.detachUsb(id),
            };
        case 'replug':
            return {
                label: 'Restart the machine',
                icon: RefreshCw,
                outline: false,
                operation: 'launch',
                disabledReason: null,
                run: () => machines.restartMachine(id),
            };
        case 'trust':
            return {
                label: 'Pair with the phone',
                icon: Cable,
                outline: false,
                operation: 'device-pair',
                disabledReason:
                    needsLive() ?? (device.value?.udid ? null : 'The phone has no UDID yet'),
                run: () => machines.pairDevice(id, device.value?.udid ?? ''),
            };
        case 'developer-mode':
            return {
                label: 'Check again',
                icon: RefreshCw,
                outline: true,
                operation: 'refresh',
                disabledReason: needsLive(),
                run: () => machines.refreshDevices(id),
            };
        case 'signing':
            return readiness.value.canPrepareSigning
                ? {
                      label: `Prepare signing for ${name}`,
                      icon: ShieldCheck,
                      outline: false,
                      operation: 'device-signing',
                      disabledReason:
                          needsLive() ??
                          (device.value?.udid ? null : 'The guest has not read the UDID yet'),
                      run: () => (signingOpen.value = true),
                  }
                : {
                      label: 'Check again',
                      icon: RefreshCw,
                      outline: true,
                      operation: 'refresh',
                      disabledReason: needsLive(),
                      run: () => machines.refreshDevices(id),
                  };
        case 'ready':
            return {
                label: streaming.value
                    ? `Running on ${name}`
                    : run.value
                      ? `Run again on ${name}`
                      : `Build and run on ${name}`,
                icon: Play,
                outline: false,
                operation: 'run-device',
                disabledReason: streaming.value
                    ? 'Stop the console session to run again'
                    : (needsLive() ??
                      (device.value?.udid ? null : 'The guest has not read the UDID yet')),
                run: () =>
                    machines.runOnDevice(
                        id,
                        device.value?.udid ?? '',
                        envSetId.value || null,
                        requestedVersion(view.value, draft),
                    ),
            };
    }
});
// A refresh is a probe rather than an operation: it sets `refreshing`, not `operation`, so
// the button that asks for one has to watch that flag to spin and to stay disabled.
const primaryRefreshing = computed(
    () => primary.value?.operation === 'refresh' && session.refreshing,
);
const primaryDisabled = computed(
    () =>
        busy.value ||
        primaryRefreshing.value ||
        step.status === 'pending' ||
        primary.value === null ||
        primary.value.disabledReason !== null,
);
// A live run is not starting: the button names it as running and does not spin.
const primarySpinning = computed(
    () =>
        primaryRefreshing.value ||
        (!streaming.value &&
            primary.value !== null &&
            primary.value.operation !== null &&
            session.operation === primary.value.operation),
);

const developmentProfile = computed(
    () => signing.value?.profiles.find((profile) => profile.kind === 'development') ?? null,
);
const recipe = computed(() => [
    { label: 'Scheme', value: workspace.value?.scheme ?? 'App' },
    { label: 'Configuration', value: 'Debug · development signing' },
    {
        label: 'Identity',
        value: signing.value?.developmentIdentity?.identityName ?? 'no development identity yet',
    },
    { label: 'Profile', value: developmentProfile.value?.uuid ?? null, mono: true },
    {
        label: workspace.value?.debugBundleIdentifier
            ? 'Debug bundle identifier'
            : 'Bundle identifier',
        value: workspace.value?.debugBundleIdentifier ?? workspace.value?.bundleIdentifier,
        mono: true,
    },
    { label: 'Team', value: workspace.value?.developmentTeam, mono: true },
    {
        label: 'USB',
        value: usb.value.attached
            ? `bus ${usb.value.attached.bus} port ${usb.value.attached.port}${usb.value.attached.enumerated ? '' : ' · not yet seen by the guest'}`
            : 'not attached',
        copyable: false,
    },
]);
const checks = computed(() => deviceChecks(view.value, readiness.value));

async function prepareSigning(): Promise<void> {
    signingOpen.value = false;
    const phone = device.value;
    if (phone?.udid) {
        await machines.prepareDeviceSigning(session.id, phone.udid, phone.name);
    }
}

async function rebuild(): Promise<void> {
    rebuildOpen.value = false;
    await machines.rebuildContainer(session.id);
}

async function migrate(): Promise<void> {
    migrateOpen.value = false;
    await machines.migrateForUsb(session.id);
}

async function clear(): Promise<void> {
    clearOpen.value = false;
    await machines.clearDeviceRun(session.id);
}

// While the phone is being trusted or switched to Developer Mode, ask the guest again every
// few seconds; the panel is only mounted while the step is open, so this costs nothing else.
const POLL_MS = 5_000;
let pollTimer: ReturnType<typeof setTimeout> | null = null;
function schedulePoll(): void {
    if (pollTimer) {
        clearTimeout(pollTimer);
        pollTimer = null;
    }
    const substate = readiness.value.substate;
    if ((substate !== 'trust' && substate !== 'developer-mode') || !live.value) {
        return;
    }
    pollTimer = setTimeout(() => {
        pollTimer = null;
        if (session.operation === null && !session.refreshing) {
            void machines.refreshDevices(session.id).then(schedulePoll);
        } else {
            schedulePoll();
        }
    }, POLL_MS);
}
watch(() => readiness.value.substate, schedulePoll, { immediate: true });
onBeforeUnmount(() => {
    if (pollTimer) {
        clearTimeout(pollTimer);
    }
});
</script>

<template>
    <div class="min-w-0 space-y-4" :data-step="step.id">
        <ProgressRow
            v-if="strip"
            :label="strip.label"
            :detail="strip.detail"
            :elapsed-seconds="strip.elapsed"
            :value="strip.value"
            :completed-bytes="strip.completedBytes"
            :total-bytes="strip.totalBytes"
            :last-line="strip.lastLine"
            :state="streaming ? 'live' : 'running'"
            :stoppable="strip.stoppable"
            :stopping="session.cancelling"
            :stop-title="strip.stopTitle"
            @stop="machines.cancelOperation(session.id)"
        />

        <DeviceAttachCard
            v-model:selected-usb="selectedUsb"
            :session="session"
            :readiness="readiness"
            :usb="usb"
            :live="live"
            :busy="busy"
            :failure-title="
                phoneFailure
                    ? (failureTitle[phoneFailure.operation] ?? 'The step did not complete')
                    : null
            "
            :failure-diagnostic="phoneFailure?.message ?? null"
            :usb-options="usbOptions"
            :checks="checks"
        >
            <template v-if="primary && !ready" #action>
                <DevicePrimaryButton
                    :primary="primary"
                    :disabled="primaryDisabled"
                    :spinning="primarySpinning"
                />
            </template>
        </DeviceAttachCard>

        <DeviceRunCard
            v-model:env-set-id="envSetId"
            :session="session"
            :busy="busy"
            :pending="step.status === 'pending'"
            :next-step="ready ? null : deviceNextSummary(readiness, view)"
            :env-options="envOptions"
            :recipe="recipe"
        >
            <template v-if="primary && ready" #action>
                <DevicePrimaryButton
                    :primary="primary"
                    :disabled="primaryDisabled"
                    :spinning="primarySpinning"
                />
            </template>
        </DeviceRunCard>

        <DeviceLastRun
            :session="session"
            :busy="busy"
            :running="running"
            :run="run"
            :run-error="view.deviceRunError"
            :failure-title="runFailure ? (failureTitle['run-device'] ?? null) : null"
            :failure-diagnostic="runFailure?.message ?? null"
            @clear="clearOpen = true"
        />

        <DeviceInspectCard
            :session="session"
            :available="ready && (run !== null || running)"
            :device-name="device?.name ?? 'the phone'"
        />

        <ConfirmDialog
            v-model:open="rebuildOpen"
            title="Rebuild the container"
            confirm-label="Rebuild and restart"
            @confirm="rebuild"
        >
            <p>
                This container was created before phones had their own USB controller in the
                machine. Rebuilding it from the machine's profile adds the controller; the macOS
                disk, the pinned identity, Xcode and provisioned signing are all kept. macOS is
                asked to shut down first and then starts again, which takes a few minutes. Once
                only.
            </p>
        </ConfirmDialog>

        <ConfirmDialog
            v-model:open="migrateOpen"
            title="Enable USB on this machine"
            confirm-label="Recreate container"
            @confirm="migrate"
        >
            <p>
                The macOS disk is copied out of the container to this host, the container is
                removed, and a new one is created around that disk with the control socket and USB
                access. macOS, the pinned identity, Xcode, and provisioned signing are kept. The
                copy takes a few minutes for a disk of tens of gigabytes and needs that much free
                space under the machine's data directory; the machine restarts afterwards.
            </p>
        </ConfirmDialog>

        <ConfirmDialog
            v-model:open="signingOpen"
            :title="`Prepare signing for ${readiness.name}`"
            confirm-label="Register and provision"
            acknowledgement="The Team key in the credentials has the Admin role"
            @confirm="prepareSigning"
        >
            <p>
                Registers this phone's UDID and name with the team at Apple, which counts toward the
                yearly allowance of 100 iPhones and cannot be undone here. Creates an Apple
                Development identity if the credentials hold none, creates a development profile for
                the bundle identifier listing the phone, and provisions both into the guest keychain
                next to the distribution identity. Nothing at Apple is revoked.
            </p>
            <p>
                If the project's Debug configuration carries its own bundle identifier, that App ID
                is registered at Apple too and the debug build installs <b>beside</b> the store
                build. The main App ID's capabilities are copied onto it — push notifications, for
                one — so the debug build keeps the entitlements the app relies on. Two things to
                know: an App ID is a permanent entry in the developer portal (harmless, and it can
                be deleted there), and capabilities that are their own resources at Apple, such as
                app groups or iCloud containers, are not copied; if the app uses one, enable it on
                the new App ID in the portal once. The other route is to sign the debug build under
                the main identifier, which replaces the store build on that phone; buildbridge does
                that only when the credentials hold no profile for the Debug identifier, and says
                so.
            </p>
        </ConfirmDialog>

        <ConfirmDialog
            v-model:open="clearOpen"
            title="Clear the last device run"
            confirm-label="Clear"
            @confirm="clear"
        >
            <p>
                The retained run record and its diagnostic are removed from this host. The app on
                the phone is not touched.
            </p>
        </ConfirmDialog>
    </div>
</template>
