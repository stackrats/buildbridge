<script setup lang="ts">
// Run a Debug build on a phone plugged into this host. The panel is a ladder: the host lets the
// phone go, the container can take it, the phone is attached, trusted, signed for, in Developer
// Mode, and then it runs with its console in the drawer. One primary action per rung, read from
// the same pure model as the row summary.
import {
    Cable,
    Circle,
    CircleCheck,
    Globe,
    HardDrive,
    Play,
    RefreshCw,
    ScrollText,
    ShieldCheck,
    Smartphone,
    Unplug,
    Usb,
    X,
} from '@lucide/vue';
import { computed, onBeforeUnmount, ref, watch } from 'vue';

import { formatDate, percent, shortHash } from '../../../lib/format';
import type { SafariInspectorResult } from '../../../types/backend';
import type { ListboxOption } from '../../../lib/listbox';
import { isLive } from '../../../lib/status';
import { deviceChecks, deviceReadiness, type DeviceSubstate } from '../../../model/device';
import {
    devicePhaseLabel,
    deviceSigningPhaseLabel,
    containerRebuildPhaseLabel,
    usbAttachPhaseLabel,
    usbMigrationPhaseLabel,
} from '../../../model/phases';
import type { JourneyStep } from '../../../model/steps';
import {
    activityLabel,
    useMachinesStore,
    type MachineSession,
    type OperationId,
} from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import ConfirmDialog from '../../dialogs/ConfirmDialog.vue';
import Button from '../../ui/Button.vue';
import Callout from '../../ui/Callout.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import KeyValue from '../../ui/KeyValue.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import Select from '../../ui/Select.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const ui = useUi();

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
const lastLine = computed(() => session.deviceLog.at(-1)?.text ?? null);

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

// Web Inspector for the app on the phone lives in the guest's Safari, not here: BuildBridge
// opens Safari with its Develop menu on and then says what to click. It is not a machine
// operation — the time to inspect is while the run streams — so it keeps its own state here.
const inspector = ref<SafariInspectorResult | null>(null);
const inspecting = ref(false);
const inspectorError = ref<string | null>(null);
const showInspector = computed(
    () => readiness.value.substate === 'ready' && (run.value !== null || running.value),
);
async function openInspector(): Promise<void> {
    inspecting.value = true;
    inspectorError.value = null;
    try {
        inspector.value = await machines.openSafariInspector(session.id);
    } catch (error) {
        inspectorError.value = error instanceof Error ? error.message : String(error);
    } finally {
        inspecting.value = false;
    }
}

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
            label: progress ? deviceSigningPhaseLabel[progress.phase] : 'Checking the signing kit',
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
        const streaming = progress?.phase === 'running';
        return {
            label: progress ? devicePhaseLabel[progress.phase] : 'Preparing the recipe',
            detail: progress?.detail ?? null,
            elapsed: progress?.elapsedSeconds ?? null,
            value: null,
            completedBytes: null,
            totalBytes: null,
            lastLine: lastLine.value,
            stoppable: true,
            stopTitle: streaming
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

interface Primary {
    label: string;
    icon: typeof Usb;
    outline: boolean;
    operation: OperationId | null;
    disabledReason: string | null;
    run: () => unknown;
}

/** One primary action per rung; the ladder decides, the button only names it. */
const primary = computed<Primary | null>(() => {
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
                label: 'Refresh',
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
                      label: 'Refresh',
                      icon: RefreshCw,
                      outline: true,
                      operation: 'refresh',
                      disabledReason: needsLive(),
                      run: () => machines.refreshDevices(id),
                  };
        case 'ready':
            return {
                label: run.value ? `Run again on ${name}` : `Build and run on ${name}`,
                icon: Play,
                outline: false,
                operation: 'run-device',
                disabledReason:
                    needsLive() ??
                    (device.value?.udid ? null : 'The guest has not read the UDID yet'),
                run: () => machines.runOnDevice(id, device.value?.udid ?? ''),
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
const primarySpinning = computed(
    () =>
        primaryRefreshing.value ||
        (primary.value !== null &&
            primary.value.operation !== null &&
            session.operation === primary.value.operation),
);

const showDetach = computed(() =>
    (['trust', 'signing', 'developer-mode', 'ready'] as DeviceSubstate[]).includes(
        readiness.value.substate,
    ),
);
const showRefresh = computed(
    () => readiness.value.substate === 'signing' && readiness.value.canPrepareSigning,
);

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
    },
]);

const passedChecks = computed(() => checks.value.filter((check) => check.ok).length);

const checks = computed(() => deviceChecks(view.value, readiness.value));

const runFacts = computed(() =>
    run.value
        ? [
              {
                  label: 'Device',
                  value: `${run.value.device.name}${run.value.device.osVersion ? ` · iOS ${run.value.device.osVersion}` : ''}`,
              },
              {
                  label: 'Bundle identifier',
                  value: run.value.projectBundleIdentifier
                      ? `${run.value.bundleIdentifier} · the project's Debug identifier ${run.value.projectBundleIdentifier} has no development profile`
                      : run.value.bundleIdentifier,
                  mono: !run.value.projectBundleIdentifier,
              },
              {
                  label: 'Version',
                  value: `${run.value.marketingVersion} (${run.value.buildNumber})`,
              },
              {
                  label: 'Installed',
                  value: formatDate(
                      new Date(run.value.installedAtEpochSeconds * 1000).toISOString(),
                  ),
              },
              {
                  label: 'Console ended',
                  value:
                      run.value.consoleEnd === 'stopped'
                          ? 'stopped from here'
                          : run.value.consoleEnd === 'exited'
                            ? `the app exited${run.value.exitStatus !== null ? ` (${run.value.exitStatus})` : ''}`
                            : 'the bridge disconnected',
              },
              {
                  label: 'Profile',
                  value: shortHash(run.value.provisioningProfileUuid, 8),
                  mono: true,
              },
          ]
        : [],
);
</script>

<template>
    <StepPanel :step="step">
        <template #action>
            <span
                v-if="readiness.substate === 'attach' && usbOptions.length > 1"
                class="w-64 max-w-full"
            >
                <Select
                    v-model="selectedUsb"
                    :options="usbOptions"
                    size="sm"
                    placeholder="Choose a phone"
                    :disabled="busy"
                />
            </span>
            <Button
                v-if="primary"
                size="sm"
                :variant="primary.outline ? 'outline' : 'default'"
                :disabled="primaryDisabled"
                :title="primary.disabledReason ?? undefined"
                @click="primary.run()"
            >
                <Spinner
                    v-if="primarySpinning"
                    :tone="primary.outline ? undefined : 'text-white dark:text-zinc-950'"
                />
                <component :is="primary.icon" v-else class="h-3.5 w-3.5" />
                {{ primary.label }}
            </Button>
            <Button
                v-if="showRefresh"
                variant="ghost"
                size="sm"
                :disabled="busy || session.refreshing"
                @click="machines.refreshDevices(session.id)"
            >
                <Spinner v-if="session.refreshing" />
                <RefreshCw v-else class="h-3.5 w-3.5" />
                Refresh
            </Button>
            <Button
                v-if="showDetach"
                variant="ghost"
                size="sm"
                :disabled="busy"
                title="Returns the phone to this host. Attaching it to this machine again needs the machine restarted first."
                @click="machines.detachUsb(session.id)"
            >
                <Spinner v-if="session.operation === 'usb-detach'" />
                <Unplug v-else class="h-3.5 w-3.5" />
                Detach
            </Button>
            <Button
                v-if="session.deviceLog.length"
                variant="ghost"
                size="sm"
                @click="ui.openLog(session.id, 'device')"
            >
                <ScrollText class="h-3.5 w-3.5" />
                Show console
            </Button>
            <Button
                v-if="showInspector"
                variant="ghost"
                size="sm"
                :disabled="inspecting"
                title="Opens Safari in the guest console with its Develop menu; the app's web view is listed there under the phone's name. Works while the app runs."
                @click="openInspector"
            >
                <Spinner v-if="inspecting" />
                <Globe v-else class="h-3.5 w-3.5" />
                Inspect in Safari
            </Button>
            <Button
                v-if="run || view.deviceRunError"
                variant="ghost"
                size="sm"
                title="Removes the retained run and its diagnostic on this host"
                :disabled="busy"
                @click="clearOpen = true"
            >
                <Spinner v-if="session.operation === 'clear-device-run'" />
                Clear last run
            </Button>
        </template>

        <template
            v-if="
                running ||
                view.deviceRunError ||
                failure ||
                !live ||
                [
                    'host-rule',
                    'trust',
                    'signing',
                    'developer-mode',
                    'unplugged',
                    'replug',
                    'attach',
                ].includes(readiness.substate)
            "
            #status
        >
            <ProgressRow
                v-if="strip"
                :label="strip.label"
                :detail="strip.detail"
                :elapsed-seconds="strip.elapsed"
                :value="strip.value"
                :completed-bytes="strip.completedBytes"
                :total-bytes="strip.totalBytes"
                :last-line="strip.lastLine"
                :stoppable="strip.stoppable"
                :stopping="session.cancelling"
                :stop-title="strip.stopTitle"
                @stop="machines.cancelOperation(session.id)"
            />
            <FailureBlock
                v-if="view.deviceRunError && !running"
                title="The last device run failed"
                cause="The diagnostic is kept until the run is cleared. Fix the cause, then build and run again."
                :diagnostic="view.deviceRunError"
            >
                <template v-if="session.deviceLog.length" #actions>
                    <Button variant="outline" size="sm" @click="ui.openLog(session.id, 'device')">
                        Show console
                    </Button>
                </template>
            </FailureBlock>
            <FailureBlock
                v-else-if="failure && !running"
                :title="failureTitle[failure.operation] ?? 'The step did not complete'"
                :diagnostic="failure.message"
            />
            <Callout v-if="!live && readiness.substate !== 'host-rule'" tone="neutral">
                Start the machine to continue; the phone attaches to a running guest.
            </Callout>
            <Callout
                v-if="readiness.substate === 'host-rule' && usb.host.usbmuxdActive"
                tone="neutral"
                title="usbmuxd holds iPhones on this host"
            >
                The rule tells udev not to start usbmuxd for iPhones and hands their device node to
                the plugdev group, which the machine's QEMU user is in. Installing it asks for
                authorization once; unplug the phone and plug it in again afterwards so it applies.
                Host-side iPhone sync stops working for as long as the rule is installed.
            </Callout>
            <Callout v-if="readiness.substate === 'attach' && !live" tone="neutral">
                {{ usb.host.devices.length }} Apple device{{
                    usb.host.devices.length === 1 ? '' : 's'
                }}
                on this host; the machine must be running to take one.
            </Callout>
            <Callout
                v-for="issue in usb.host.issues.filter(
                    (text) => !text.includes('Install the BuildBridge'),
                )"
                :key="issue"
                tone="warn"
            >
                {{ issue }}
            </Callout>
            <Callout
                v-if="readiness.substate === 'unplugged'"
                tone="warn"
                title="The phone left this host"
            >
                The guest still holds the port; plug the phone back into the same port and it
                reappears by itself, or detach to hand the port back.
            </Callout>
            <Callout
                v-if="readiness.substate === 'replug'"
                tone="warn"
                title="QEMU holds the phone but could not read it"
            >
                QEMU reads a phone cleanly only the first time it opens it in a session, and this
                phone has been opened before: a detach and re-attach, or the phone re-enumerating
                under QEMU, both leave it here. Restart the machine, then attach once after it is
                up. The phone can stay plugged in; nothing on it changes.
            </Callout>
            <Callout v-if="readiness.substate === 'trust'" tone="neutral" title="On the phone">
                Press <b>Pair with the phone</b>, then unlock the phone and tap <b>Trust</b> when it
                asks about this computer and enter the passcode. Trust alone gives the guest the
                older pairing; the button adds the one <code>devicectl</code> and Xcode use, and
                waits for your tap.
            </Callout>
            <Callout
                v-if="readiness.substate === 'signing' && !readiness.canPrepareSigning"
                tone="warn"
                title="The attached kit has no Team key"
            >
                Registering the phone and creating a development profile happen at Apple through the
                kit's App Store Connect key. Add one to the kit under Signing kits, or store a
                development identity and a profile that already lists this phone.
            </Callout>
            <Callout
                v-if="readiness.substate === 'developer-mode'"
                tone="neutral"
                title="On the phone"
            >
                Open Settings › Privacy &amp; Security › Developer Mode, turn it on, and restart
                when asked. The switch appears only after the first development-signed connection.
                <template v-if="device?.developerMode === 'unknown'">
                    The guest could not read the state yet; it refreshes by itself.
                </template>
            </Callout>
        </template>

        <template v-if="run || inspector || inspectorError" #result>
            <FailureBlock
                v-if="inspectorError"
                title="Safari could not be opened in the guest"
                cause="Safari needs the guest's graphical session: log in on the machine's screen, then try again."
                :diagnostic="inspectorError"
                class="mb-3"
            />
            <Callout
                v-if="inspector"
                tone="neutral"
                title="Web Inspector: Safari is open on the guest's screen"
                class="mb-3"
            >
                <ol class="list-decimal space-y-1 pl-4">
                    <li>
                        On the phone, once: Settings › Safari › Advanced › <b>Web Inspector</b>
                        (under Settings › Apps › Safari on iOS 18 and later).
                    </li>
                    <li v-if="!inspector.developMenuEnabled">
                        In Safari: Safari › Settings › Advanced ›
                        <b>Show features for web developers</b>, then quit and reopen Safari. macOS
                        did not let BuildBridge set this from outside the graphical session.
                    </li>
                    <li v-else-if="inspector.safariRestarted">
                        Safari's Develop menu is on; Safari was reopened so it appears.
                    </li>
                    <li v-else>Safari's Develop menu is on.</li>
                    <li>
                        In Safari's menu bar: <b>Develop</b> ›
                        <b>{{ device?.name ?? 'the phone' }}</b> › the app's web view, listed as
                        <span class="font-mono">capacitor://localhost</span> or its page title. The
                        inspector shows its console, network requests, elements and storage.
                    </li>
                </ol>
                <p class="mt-2">
                    Only the Debug build BuildBridge installs is inspectable (Capacitor enables it
                    in Debug), so run the app first. The console is this machine's QEMU window on
                    this host; Ctrl+Alt+G releases the mouse.
                </p>
                <div class="mt-2">
                    <Button variant="ghost" size="sm" @click="inspector = null">
                        <X class="h-3.5 w-3.5" />
                        Hide
                    </Button>
                </div>
            </Callout>
            <KeyValue v-if="run" :items="runFacts" :columns="3" />
            <p v-if="run" class="mt-2 font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                {{ run.appPath }}
            </p>
        </template>

        <div class="space-y-3">
            <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                Builds the App scheme in the Debug configuration with the development identity,
                installs it on the phone through <span class="font-mono">devicectl</span>, launches
                it, and streams its console into the log drawer until you stop. The phone is handed
                to QEMU by USB bus and port, so it disappears from this host while attached.
            </p>
            <KeyValue :items="recipe" :columns="3" />
            <!-- Every rung at once: the ladder says what to do next, this says where it stands. -->
            <p class="text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400">
                Every check, in order · {{ passedChecks }} of {{ checks.length }} passed
            </p>
            <ul class="grid gap-2 sm:grid-cols-2">
                <li
                    v-for="check in checks"
                    :key="check.label"
                    class="flex items-start gap-2 rounded-md bg-zinc-50 p-2.5 dark:bg-zinc-950"
                >
                    <CircleCheck
                        v-if="check.ok"
                        class="mt-0.5 h-3.5 w-3.5 shrink-0 text-emerald-700 dark:text-emerald-400"
                    />
                    <Circle
                        v-else
                        class="mt-0.5 h-3.5 w-3.5 shrink-0 text-zinc-400 dark:text-zinc-500"
                    />
                    <span class="min-w-0">
                        <span class="block text-xs font-medium text-zinc-700 dark:text-zinc-200">{{
                            check.label
                        }}</span>
                        <span
                            class="block truncate text-xs text-zinc-500 dark:text-zinc-400"
                            :title="check.detail"
                            >{{ check.detail }}</span
                        >
                    </span>
                </li>
            </ul>
            <div class="flex flex-wrap items-center gap-2">
                <Button
                    v-if="usb.host.rule === 'installed'"
                    variant="ghost"
                    size="sm"
                    :disabled="busy"
                    title="Restores usbmuxd handling of iPhones on this host"
                    @click="machines.removeUsbRule(session.id)"
                >
                    <Spinner v-if="session.operation === 'usb-rule'" />
                    <Smartphone v-else class="h-3.5 w-3.5" />
                    Remove host rule
                </Button>
                <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    Self-hosted and experimental. Passthrough depends on the host releasing the
                    phone and on QEMU's USB stack; Apple's supported route to a physical device is a
                    Mac. Xcode's debugger and Instruments are not part of this step.
                </p>
            </div>
        </div>

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
            acknowledgement="The kit's Team key has the Admin role"
            @confirm="prepareSigning"
        >
            <p>
                Registers this phone's UDID and name with the team at Apple, which counts toward the
                yearly allowance of 100 iPhones and cannot be undone here. Creates an Apple
                Development identity if the kit has none, creates a development profile for the
                bundle identifier listing the phone, and provisions both into the guest keychain
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
                the main identifier, which replaces the store build on that phone; BuildBridge does
                that only when the kit holds no profile for the Debug identifier, and says so.
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
    </StepPanel>
</template>
