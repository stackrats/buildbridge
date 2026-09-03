// Every managed macOS machine, each with its own session: the last backend view, the client's
// in-flight operation, live progress for each long operation, and bounded logs. Sessions are
// keyed by machine id so switching machines in the sidebar never loses state.

import { computed, reactive } from 'vue';

import { useBackend, type DragDropEvent, type Unlisten } from '../lib/backend';
import { describeError, pushBounded } from '../lib/utils';
import { formatTime } from '../lib/format';
import type { LogLine } from '../components/ui/LogView.vue';
import { deriveJourney, summarizeJourney, type JourneyStep } from '../model/steps';
import type {
    AdoptPodfileLockResult,
    AppleArchiveProgress,
    AppleDeviceRunProgress,
    AppleProjectProgress,
    DeviceSigningProgress,
    DiskMigrationProgress,
    GuestOptimizationsView,
    ImportMacXcodeResult,
    LaunchProgress,
    MacBuilderConfig,
    MacBuilderView,
    MachineListView,
    SigningProvisioningProgress,
    XcodeImportProgress,
    UnsignedBuildTarget,
} from '../types/backend';

export type OperationId =
    | 'refresh'
    | 'launch'
    | 'stop'
    | 'configure'
    | 'guest-access'
    | 'guest-authorize'
    | 'trust'
    | 'forget-trust'
    | 'xcode-import'
    | 'xcode-activate'
    | 'approve'
    | 'clear-workspace'
    | 'sync'
    | 'test-build'
    | 'provision'
    | 'attach-kit'
    | 'attach-env'
    | 'clear-signing'
    | 'archive'
    | 'reveal'
    | 'clear-archive'
    | 'discard'
    | 'delete'
    | 'optimize'
    | 'usb-rule'
    | 'usb-migrate'
    | 'usb-attach'
    | 'usb-detach'
    | 'device-signing'
    | 'run-device'
    | 'clear-device-run'
    | 'adopt-lock';

/** Maps the native busy key (see src-tauri/src/lib.rs) to the step it blocks. */
const busyKeyStep: Record<string, string> = {
    starting: 'launch',
    stopping: 'launch',
    importing_xcode: 'xcode-import',
    activating_xcode: 'xcode-activate',
    provisioning_signing: 'provision',
    clearing_signing: 'provision',
    synchronizing: 'sync',
    test_building: 'test-build',
    adopting_lock: 'test-build',
    archiving: 'archive',
    deleting: 'launch',
    discarding: 'launch',
    migrating_usb: 'run-device',
    attaching_usb: 'run-device',
    detaching_usb: 'run-device',
    listing_devices: 'run-device',
    preparing_device_signing: 'run-device',
    running_on_device: 'run-device',
};

export const busyKeyLabel: Record<string, string> = {
    starting: 'Starting the machine',
    stopping: 'Stopping the machine',
    importing_xcode: 'Importing Xcode',
    activating_xcode: 'Activating Xcode',
    provisioning_signing: 'Provisioning signing',
    clearing_signing: 'Removing guest signing',
    synchronizing: 'Synchronizing source',
    test_building: 'Running the test build',
    adopting_lock: 'Adopting the guest Podfile.lock',
    archiving: 'Building the signed archive',
    deleting: 'Deleting the machine',
    discarding: 'Discarding the container',
    optimizing: 'Applying an optimization',
    migrating_usb: 'Enabling USB on the machine',
    attaching_usb: 'Attaching the iPhone',
    detaching_usb: 'Detaching the iPhone',
    listing_devices: 'Reading the phones the guest sees',
    preparing_device_signing: 'Preparing device signing',
    running_on_device: 'Running on the device',
};

const operationStep: Partial<Record<OperationId, string>> = {
    launch: 'launch',
    stop: 'launch',
    'xcode-import': 'xcode-import',
    'xcode-activate': 'xcode-activate',
    sync: 'sync',
    'test-build': 'test-build',
    'adopt-lock': 'test-build',
    provision: 'provision',
    'clear-signing': 'provision',
    archive: 'archive',
    'usb-rule': 'run-device',
    'usb-migrate': 'run-device',
    'usb-attach': 'run-device',
    'usb-detach': 'run-device',
    'device-signing': 'run-device',
    'run-device': 'run-device',
};

const LOG_LIMIT = 600;

export interface MachineSession {
    id: string;
    view: MacBuilderView | null;
    loading: boolean;
    error: string | null;
    notice: string | null;
    operation: OperationId | null;
    operationStartedAt: number | null;
    /** A stop was requested and the operation has not returned yet. */
    cancelling: boolean;
    /** A refresh the person asked for is in flight. */
    refreshing: boolean;
    /** The optimizer catalogue with this guest's state, once asked for. */
    optimizations: GuestOptimizationsView | null;
    optimizationsLoading: boolean;
    launch: LaunchProgress | null;
    xcode: XcodeImportProgress | null;
    xcodeImport: ImportMacXcodeResult | null;
    signing: SigningProvisioningProgress | null;
    project: AppleProjectProgress | null;
    archive: AppleArchiveProgress | null;
    usbMigration: DiskMigrationProgress | null;
    deviceSigning: DeviceSigningProgress | null;
    device: AppleDeviceRunProgress | null;
    buildLog: LogLine[];
    archiveLog: LogLine[];
    /** The app's own console while it runs on the phone. */
    deviceLog: LogLine[];
    /** The guest's Podfile.lock adopted into the project, until the next test build. */
    lockAdoption: AdoptPodfileLockResult | null;
    activity: LogLine[];
    lastFailure: { operation: OperationId; message: string; at: number } | null;
}

const state = reactive({
    list: null as MachineListView | null,
    listLoading: false,
    listError: null as string | null,
    sessions: {} as Record<string, MachineSession>,
    dragActive: false,
    lastDrop: null as { paths: string[]; at: number } | null,
});

let unlisteners: Unlisten[] = [];
let listeningPromise: Promise<void> | null = null;
let refreshTimers: Record<string, ReturnType<typeof setTimeout>> = {};

function createSession(id: string): MachineSession {
    return {
        id,
        view: null,
        loading: false,
        error: null,
        notice: null,
        operation: null,
        operationStartedAt: null,
        cancelling: false,
        refreshing: false,
        optimizations: null,
        optimizationsLoading: false,
        launch: null,
        xcode: null,
        xcodeImport: null,
        signing: null,
        project: null,
        archive: null,
        usbMigration: null,
        deviceSigning: null,
        device: null,
        buildLog: [],
        archiveLog: [],
        deviceLog: [],
        lockAdoption: null,
        activity: [],
        lastFailure: null,
    };
}

function session(id: string): MachineSession {
    state.sessions[id] ??= createSession(id);
    return state.sessions[id];
}

function note(target: MachineSession, text: string, tone: LogLine['tone'] = 'system'): void {
    pushBounded(target.activity, { text: `${formatTime(Date.now())}  ${text}`, tone }, LOG_LIMIT);
}

function applyView(target: MachineSession, view: MacBuilderView): void {
    target.view = view;
}

async function loadList(): Promise<void> {
    state.listLoading = true;
    try {
        state.list = await useBackend().listMachines();
        state.listError = null;
        const known = new Set(state.list.machines.map((machine) => machine.id));
        for (const id of Object.keys(state.sessions)) {
            if (!known.has(id)) {
                delete state.sessions[id];
            }
        }
    } catch (error) {
        state.listError = describeError(error);
    } finally {
        state.listLoading = false;
    }
}

async function refreshMachine(id: string, options: { silent?: boolean } = {}): Promise<void> {
    const target = session(id);
    if (!options.silent) {
        target.loading = target.view === null;
        target.refreshing = true;
    }
    try {
        applyView(target, await useBackend().getMachine(id));
        if (!options.silent) {
            target.error = null;
        }
    } catch (error) {
        target.error = describeError(error);
    } finally {
        target.loading = false;
        target.refreshing = false;
    }
}

function scheduleRefresh(id: string): void {
    if (refreshTimers[id]) {
        clearTimeout(refreshTimers[id]);
    }
    refreshTimers[id] = setTimeout(() => {
        delete refreshTimers[id];
        const target = state.sessions[id];
        if (target && target.operation === null) {
            void refreshMachine(id, { silent: true });
        }
        void loadList();
    }, 150);
}

type OperationResult = MacBuilderView | { view: MacBuilderView };

async function runOperation<R extends OperationResult>(
    id: string,
    operation: OperationId,
    work: () => Promise<R>,
    options: {
        started?: string;
        finished?: string | ((result: R) => string);
        /** What a stop means for this operation, when it is not "nothing changed". */
        stopped?: string;
    } = {},
): Promise<R | null> {
    const target = session(id);
    if (target.operation !== null) {
        target.error = 'Another operation is still running on this machine. Wait for it to finish.';
        return null;
    }
    target.operation = operation;
    target.operationStartedAt = Date.now();
    target.error = null;
    target.notice = null;
    if (options.started) {
        note(target, options.started);
    }
    try {
        const result = await work();
        applyView(target, 'machineId' in result ? result : result.view);
        const finished =
            typeof options.finished === 'function' ? options.finished(result) : options.finished;
        if (finished) {
            target.notice = finished;
            note(target, finished, 'success');
        }
        target.lastFailure = null;
        return result;
    } catch (error) {
        const message = describeError(error);
        if (target.cancelling || message === 'Stopped.') {
            // A stop the user asked for is an outcome, not a failure.
            target.notice = options.stopped ?? 'Stopped. Nothing already retained was changed.';
            note(target, 'Stopped by request.');
            return null;
        }
        target.error = message;
        target.lastFailure = { operation, message, at: Date.now() };
        note(target, message, 'stderr');
        return null;
    } finally {
        target.operation = null;
        target.operationStartedAt = null;
        target.cancelling = false;
        void loadList();
    }
}

async function listenForEvents(): Promise<void> {
    if (listeningPromise) {
        return listeningPromise;
    }
    listeningPromise = (async () => {
        const backend = useBackend();
        unlisteners = await Promise.all([
            backend.onMachineChanged((event) => {
                if (event.machineId) {
                    scheduleRefresh(event.machineId);
                } else {
                    void loadList();
                    for (const id of Object.keys(state.sessions)) {
                        scheduleRefresh(id);
                    }
                }
            }),
            backend.onLaunchProgress((event) => {
                const target = session(event.machineId);
                target.launch = event;
                note(target, event.detail);
            }),
            backend.onXcodeProgress((event) => {
                session(event.machineId).xcode = event;
            }),
            backend.onSigningProgress((event) => {
                session(event.machineId).signing = event;
            }),
            backend.onProjectProgress((event) => {
                const target = session(event.machineId);
                const phaseChanged = target.project?.phase !== event.phase;
                target.project = event;
                if (phaseChanged) {
                    pushBounded(
                        target.buildLog,
                        { text: `— ${event.detail}`, tone: 'system' },
                        LOG_LIMIT,
                    );
                }
                if (event.logLine) {
                    pushBounded(target.buildLog, { text: event.logLine }, LOG_LIMIT);
                }
            }),
            backend.onArchiveProgress((event) => {
                const target = session(event.machineId);
                const phaseChanged = target.archive?.phase !== event.phase;
                target.archive = event;
                if (phaseChanged) {
                    pushBounded(
                        target.archiveLog,
                        { text: `— ${event.detail}`, tone: 'system' },
                        LOG_LIMIT,
                    );
                }
                if (event.logLine) {
                    pushBounded(target.archiveLog, { text: event.logLine }, LOG_LIMIT);
                }
            }),
            backend.onUsbMigrationProgress((event) => {
                const target = session(event.machineId);
                const phaseChanged = target.usbMigration?.phase !== event.phase;
                target.usbMigration = event;
                if (phaseChanged) {
                    note(target, event.detail);
                }
            }),
            backend.onDeviceSigningProgress((event) => {
                const target = session(event.machineId);
                const phaseChanged = target.deviceSigning?.phase !== event.phase;
                target.deviceSigning = event;
                if (phaseChanged) {
                    note(target, event.detail);
                }
            }),
            backend.onDeviceProgress((event) => {
                const target = session(event.machineId);
                const phaseChanged = target.device?.phase !== event.phase;
                target.device = event;
                if (phaseChanged) {
                    pushBounded(
                        target.deviceLog,
                        { text: `— ${event.detail}`, tone: 'system' },
                        LOG_LIMIT,
                    );
                }
                // The app console is not throttled like build output: every line lands.
                for (const line of event.logLines) {
                    pushBounded(target.deviceLog, { text: line }, LOG_LIMIT);
                }
            }),
            backend.onDragDrop((event: DragDropEvent) => {
                if (event.type === 'enter' || event.type === 'over') {
                    state.dragActive = true;
                } else if (event.type === 'leave') {
                    state.dragActive = false;
                } else if (event.type === 'drop') {
                    state.dragActive = false;
                    state.lastDrop = { paths: event.paths, at: Date.now() };
                }
            }),
        ]);
    })();
    return listeningPromise;
}

/** The step the in-flight or native operation belongs to, for the step lists. */
function runningStepFor(id: string): string | null {
    const target = state.sessions[id];
    if (!target) {
        return null;
    }
    if (target.operation && operationStep[target.operation]) {
        return operationStep[target.operation] ?? null;
    }
    const busy = target.view?.busyOperation;
    return busy ? (busyKeyStep[busy] ?? null) : null;
}

export function useMachinesStore() {
    return {
        state,
        machines: computed(() => state.list?.machines ?? []),
        host: computed(() => state.list?.host ?? null),
        session,
        loadList,
        refreshMachine,
        listenForEvents,
        dispose(): void {
            for (const unlisten of unlisteners) {
                unlisten();
            }
            unlisteners = [];
            listeningPromise = null;
            for (const timer of Object.values(refreshTimers)) {
                clearTimeout(timer);
            }
            refreshTimers = {};
        },

        /** The step the in-flight or native operation belongs to, for the step lists. */
        runningStep: runningStepFor,

        /**
         * The machine's journey: exact once its view has been probed, coarse from the list
         * summary before that, so the overview and the sidebar can always say where it is.
         */
        journey(id: string): JourneyStep[] {
            const view = state.sessions[id]?.view;
            if (view) {
                return deriveJourney(view, { runningStep: runningStepFor(id) });
            }
            const summary = state.list?.machines.find((machine) => machine.id === id);
            return summary
                ? summarizeJourney(summary, state.list?.host ?? null, runningStepFor(id))
                : [];
        },

        async createMachine(profile: MacBuilderConfig): Promise<string | null> {
            const previous = new Set(state.list?.machines.map((machine) => machine.id));
            state.list = await useBackend().createMachine(profile);
            const created = state.list.machines.find((machine) => !previous.has(machine.id));
            return created?.id ?? null;
        },

        async deleteMachine(id: string): Promise<boolean> {
            const target = session(id);
            target.error = null;
            try {
                state.list = await useBackend().deleteMachine(id);
                delete state.sessions[id];
                return true;
            } catch (error) {
                target.error = describeError(error);
                return false;
            }
        },

        launch: (id: string) =>
            runOperation(id, 'launch', () => useBackend().launchMachine(id), {
                started: 'Starting the machine',
                finished: 'The macOS machine is running. Open its console window to continue.',
            }),
        stop: (id: string) =>
            runOperation(id, 'stop', () => useBackend().stopMachine(id), {
                started: 'Stopping the machine safely',
                finished: 'Stopped. The macOS disk is retained and resumes on the next start.',
            }),
        configure: (id: string, profile: MacBuilderConfig) =>
            runOperation(id, 'configure', () => useBackend().configureMachine(id, profile), {
                finished: 'Machine profile saved.',
            }),
        configureGuestAccess: (id: string, username: string) =>
            runOperation(
                id,
                'guest-access',
                () => useBackend().configureGuestAccess(id, username),
                {
                    finished:
                        'Access key ready. Install it with the macOS password, or add it from the guest Terminal.',
                },
            ),
        authorizeGuestKey: async (id: string, username: string, password: string) => {
            const result = await runOperation(
                id,
                'guest-authorize',
                () => useBackend().authorizeGuestKey(id, username, password),
                {
                    started:
                        'Installing the access key over one password-authenticated SSH session',
                    finished: (view) =>
                        view.guest.diagnostics.authenticated
                            ? 'Key installed. BuildBridge signs in with its own key from now on; the password was discarded.'
                            : 'The key was written, but signing in with it still fails. See the step for the reason.',
                },
            );
            if (!result) {
                // The username and key are kept even when the password was rejected, so the
                // Terminal route can show its commands.
                void refreshMachine(id, { silent: true });
            }
            return result;
        },
        trustGuest: (id: string, fingerprint: string) =>
            runOperation(id, 'trust', () => useBackend().trustGuest(id, fingerprint), {
                finished: 'Guest identity pinned. A changed key will be rejected from now on.',
            }),
        forgetTrust: (id: string) =>
            runOperation(id, 'forget-trust', () => useBackend().forgetGuestTrust(id), {
                finished: 'Identity pin removed. Verify and trust the new fingerprint.',
            }),
        importXcode: async (id: string, path: string) => {
            const target = session(id);
            target.xcode = null;
            const result = await runOperation(
                id,
                'xcode-import',
                () => useBackend().importXcode(id, path),
                {
                    started: 'Importing Xcode',
                    finished: (imported) =>
                        `Xcode expanded at ${imported.installedPath}. Activate it next.`,
                },
            );
            if (result) {
                target.xcodeImport = result;
            }
            return result;
        },
        activateXcode: async (id: string, password: string | null) => {
            const target = session(id);
            target.xcode = null;
            return runOperation(
                id,
                'xcode-activate',
                () => useBackend().activateXcode(id, password),
                {
                    started:
                        password === null
                            ? 'Waiting for Xcode activation in the guest Terminal'
                            : 'Activating Xcode over the bridge; the password is used for this session only',
                    finished: 'Xcode is active. The machine can build projects now.',
                },
            );
        },
        approveWorkspace: (id: string, path: string) =>
            runOperation(id, 'approve', () => useBackend().approveWorkspace(id, path), {
                finished: 'Project approved. Nothing has been copied yet.',
            }),
        clearWorkspace: (id: string) =>
            runOperation(id, 'clear-workspace', () => useBackend().clearWorkspace(id), {
                finished: 'Project approval removed. The host project was not modified.',
            }),
        sync: async (id: string) => {
            const target = session(id);
            target.project = null;
            target.buildLog = [];
            return runOperation(id, 'sync', () => useBackend().syncWorkspace(id), {
                started: 'Synchronizing source into the guest',
                finished: (result) =>
                    `Synchronized ${result.sync.sourceFileCount} files into ${result.sync.guestPath}.`,
            });
        },
        testBuild: async (id: string, buildTarget: UnsignedBuildTarget = 'device_sdk') => {
            const target = session(id);
            target.project = null;
            target.lockAdoption = null;
            return runOperation(
                id,
                'test-build',
                () => useBackend().runSmokeBuild(id, buildTarget),
                {
                    started: 'Running the unsigned test build',
                    finished: (result) =>
                        result.build.nativeLockfileUpdated
                            ? `Unsigned build succeeded with Xcode ${result.build.xcodeVersion}. The guest refreshed Podfile.lock; review it before a signed build.`
                            : `Unsigned build succeeded with Xcode ${result.build.xcodeVersion}.`,
                },
            ).then((result) => {
                if (result) {
                    for (const line of result.build.outputTail) {
                        pushBounded(target.buildLog, { text: line }, LOG_LIMIT);
                    }
                }
                return result;
            });
        },
        provisionSigning: async (id: string) => {
            const target = session(id);
            target.signing = null;
            return runOperation(id, 'provision', () => useBackend().provisionSigning(id), {
                started: 'Provisioning signing into the guest keychain',
                finished: 'Signing provisioned and verified with a real code-sign probe.',
            });
        },
        attachSigningKit: (id: string, kitId: string | null) =>
            runOperation(id, 'attach-kit', () => useBackend().attachSigningKit(id, kitId), {
                finished: kitId
                    ? 'Signing kit attached to this machine.'
                    : 'Signing kit detached from this machine.',
            }),
        cancelOperation: async (id: string) => {
            const target = session(id);
            if (target.operation === null || target.cancelling) {
                return;
            }
            target.cancelling = true;
            try {
                await useBackend().cancelMachineOperation(id);
            } catch (error) {
                target.cancelling = false;
                target.error = describeError(error);
            }
        },
        loadOptimizations: async (id: string) => {
            const target = session(id);
            target.optimizationsLoading = true;
            try {
                target.optimizations = await useBackend().listGuestOptimizations(id);
            } catch (error) {
                target.error = describeError(error);
            } finally {
                target.optimizationsLoading = false;
            }
        },
        applyOptimization: async (id: string, optimizationId: string, title: string) => {
            const target = session(id);
            const result = await runOperation(
                id,
                'optimize',
                async () => {
                    const view = await useBackend().applyGuestOptimization(id, optimizationId);
                    target.optimizations = view;
                    return { view: target.view! };
                },
                {
                    started: `Applying ${title}`,
                    finished: `${title} applied to the guest.`,
                },
            );
            return result !== null;
        },
        attachEnvSet: (id: string, setId: string | null) =>
            runOperation(id, 'attach-env', () => useBackend().attachEnvSet(id, setId), {
                finished: setId
                    ? 'Env set attached. It is written into the guest at the next sync.'
                    : 'Env set detached. The next sync runs without it.',
            }),
        clearGuestSigning: (id: string) =>
            runOperation(id, 'clear-signing', () => useBackend().clearGuestSigning(id), {
                finished: 'Guest keychain and installed profiles removed.',
            }),
        signedArchive: async (id: string, envSetId: string | null) => {
            const target = session(id);
            target.archive = null;
            target.archiveLog = [];
            return runOperation(id, 'archive', () => useBackend().runSignedArchive(id, envSetId), {
                started: 'Building the signed Release archive and IPA',
                finished: (result) =>
                    `Signed ${result.archive.marketingVersion} (${result.archive.buildNumber}) exported and verified.`,
            }).then((result) => {
                if (result) {
                    for (const line of result.archive.outputTail) {
                        pushBounded(target.archiveLog, { text: line }, LOG_LIMIT);
                    }
                }
                return result;
            });
        },
        revealArchive: async (id: string) => {
            const target = session(id);
            try {
                await useBackend().revealArchive(id);
            } catch (error) {
                target.error = describeError(error);
            }
        },
        clearArchive: (id: string) =>
            runOperation(id, 'clear-archive', () => useBackend().clearArchive(id), {
                finished: 'Retained artifacts removed.',
            }),
        discardContainer: (id: string) =>
            runOperation(id, 'discard', () => useBackend().discardMachineContainer(id), {
                finished: 'Container discarded. The next start creates a fresh macOS disk.',
            }),
        installUsbRule: (id: string) =>
            runOperation(
                id,
                'usb-rule',
                async () => {
                    await useBackend().installUsbReleaseRule();
                    return useBackend().getMachine(id);
                },
                {
                    started:
                        'Installing the USB release rule; a system prompt asks for authorization',
                    finished:
                        'USB release rule installed. Unplug the iPhone and plug it in again so it applies.',
                },
            ),
        removeUsbRule: (id: string) =>
            runOperation(
                id,
                'usb-rule',
                async () => {
                    await useBackend().removeUsbReleaseRule();
                    return useBackend().getMachine(id);
                },
                {
                    finished:
                        'USB release rule removed. usbmuxd handles iPhones on this host again.',
                },
            ),
        migrateForUsb: async (id: string) => {
            session(id).usbMigration = null;
            return runOperation(id, 'usb-migrate', () => useBackend().migrateMachineForUsb(id), {
                started: 'Moving the macOS disk to this host and recreating the container',
                finished:
                    'USB access enabled. The machine restarted from its disk on this host with its identity and signing intact.',
            });
        },
        attachUsb: (id: string, bus: number, port: string) =>
            runOperation(id, 'usb-attach', () => useBackend().attachUsbDevice(id, bus, port), {
                started: 'Passing the iPhone into the guest',
                finished: (view) =>
                    view.usb.attached?.enumerated
                        ? 'iPhone attached. Unlock it and tap Trust when it asks about this computer.'
                        : 'The port is handed to the guest, but the phone has not shown up in it yet.',
            }),
        detachUsb: (id: string) =>
            runOperation(id, 'usb-detach', () => useBackend().detachUsbDevice(id), {
                finished: 'iPhone returned to this host. The app stays installed on it.',
            }),
        /** A probe like refreshMachine, not an operation: the drawer and rows do not react. */
        refreshDevices: async (id: string) => {
            const target = session(id);
            if (target.operation !== null || target.refreshing) {
                return;
            }
            target.refreshing = true;
            try {
                applyView(target, await useBackend().listGuestDevices(id));
                target.error = null;
            } catch (error) {
                target.error = describeError(error);
            } finally {
                target.refreshing = false;
            }
        },
        prepareDeviceSigning: async (id: string, udid: string, deviceName: string) => {
            const target = session(id);
            target.deviceSigning = null;
            target.signing = null;
            return runOperation(
                id,
                'device-signing',
                () => useBackend().prepareAppleDeviceSigning(id, udid, deviceName),
                {
                    started:
                        'Registering the iPhone at Apple and provisioning a development identity',
                    finished: (result) =>
                        `${deviceName} is registered and signed for. ${
                            result.profileCreated
                                ? 'A development profile was created.'
                                : 'An existing development profile lists it.'
                        }`,
                },
            );
        },
        runOnDevice: async (id: string, udid: string) => {
            const target = session(id);
            target.device = null;
            target.deviceLog = [];
            return runOperation(
                id,
                'run-device',
                () => useBackend().runAppleDeviceBuild(id, udid),
                {
                    started: 'Building the Debug configuration for the iPhone',
                    finished: (result) =>
                        `${result.run.marketingVersion} (${result.run.buildNumber}) ran on ${result.run.device.name}.`,
                    stopped:
                        'Stopped. The app stays installed on the iPhone and the run is retained.',
                },
            ).then((result) => {
                // Lines already streamed; the tail only fills a log that never saw them.
                if (result && target.deviceLog.length === 0) {
                    for (const line of result.run.consoleTail) {
                        pushBounded(target.deviceLog, { text: line }, LOG_LIMIT);
                    }
                }
                return result;
            });
        },
        /**
         * Copies the Podfile.lock CocoaPods wrote in the guest into the approved project and
         * lifts the archive's drift block; the guest already compiled with exactly that lock.
         */
        adoptGuestLock: (id: string) =>
            runOperation(id, 'adopt-lock', () => useBackend().adoptGuestPodfileLock(id), {
                started: 'Reading the Podfile.lock the guest resolved',
                finished: (result) =>
                    result.changes.identical
                        ? 'The project already held the guest’s Podfile.lock; the block is lifted.'
                        : `Adopted the guest’s Podfile.lock into the project: ${result.changes.pods.length} pod${result.changes.pods.length === 1 ? '' : 's'} repinned. Commit it in the project.`,
            }).then((result) => {
                if (result) {
                    session(id).lockAdoption = result;
                }
                return result;
            }),
        clearDeviceRun: (id: string) =>
            runOperation(id, 'clear-device-run', () => useBackend().clearAppleDeviceRun(id), {
                finished: 'Last device run cleared.',
            }),
        clearDeviceLog(id: string): void {
            session(id).deviceLog = [];
        },
        clearMessages(id: string): void {
            const target = session(id);
            target.error = null;
            target.notice = null;
        },
        clearBuildLog(id: string): void {
            session(id).buildLog = [];
        },
        clearArchiveLog(id: string): void {
            session(id).archiveLog = [];
        },
        clearActivity(id: string): void {
            session(id).activity = [];
        },
        consumeDrop(): string[] | null {
            const drop = state.lastDrop;
            state.lastDrop = null;
            return drop?.paths ?? null;
        },
    };
}
