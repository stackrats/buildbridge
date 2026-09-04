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
    ContainerRebuildProgress,
    DeviceSigningProgress,
    DiskMigrationProgress,
    GuestOptimizationsView,
    ImportMacXcodeResult,
    LaunchProgress,
    MacBuilderConfig,
    MacBuilderView,
    MachineListView,
    MachineTemplateSummary,
    SafariInspectorResult,
    SigningProvisioningProgress,
    TemplateSaveProgress,
    UnsignedBuildTarget,
    UsbAttachProgress,
    XcodeImportProgress,
} from '../types/backend';

import {
    activityLabel,
    busyKeyLabel,
    busyKeyStep,
    operationLabel,
    operationStep,
    type OperationId,
} from '../model/operations';

export { activityLabel, busyKeyLabel, operationLabel };
export type { OperationId };

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
    /** Rebuilding the container from its profile. */
    rebuild: ContainerRebuildProgress | null;
    /** After QEMU holds the phone: macOS enumerating it, then pairing. */
    usbAttach: UsbAttachProgress | null;
    deviceSigning: DeviceSigningProgress | null;
    device: AppleDeviceRunProgress | null;
    buildLog: LogLine[];
    archiveLog: LogLine[];
    /** The app's own console while it runs on the phone. */
    deviceLog: LogLine[];
    /** The guest's Podfile.lock adopted into the project, until the next test build. */
    lockAdoption: AdoptPodfileLockResult | null;
    /** Saving this machine as a template, while it runs. */
    templateSave: TemplateSaveProgress | null;
    /** A clone's bootstrap was tried once this session; a failure then waits for the person. */
    templateAdoptTried: boolean;
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
    templates: [] as MachineTemplateSummary[],
    templatesLoading: false,
    templatesError: null as string | null,
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
        rebuild: null,
        usbAttach: null,
        deviceSigning: null,
        device: null,
        buildLog: [],
        archiveLog: [],
        deviceLog: [],
        lockAdoption: null,
        templateSave: null,
        templateAdoptTried: false,
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
    maybeAdoptTemplate(target);
}

/**
 * A clone bootstraps itself: once its macOS answers, the identity the template recorded is
 * pinned and the clone's own key is installed through the template's. Tried once per session
 * so a failure is shown on the trust step rather than retried on every probe.
 */
function maybeAdoptTemplate(target: MachineSession): void {
    const view = target.view;
    if (
        !view?.template ||
        target.templateAdoptTried ||
        target.operation !== null ||
        view.busyOperation !== null ||
        !view.guest.ssh.reachable ||
        view.guest.ssh.trust === 'mismatch'
    ) {
        return;
    }
    const needsPin = view.guest.ssh.trust === 'untrusted';
    const needsKey = !view.guest.diagnostics.authenticated;
    if (!needsPin && !needsKey) {
        return;
    }
    target.templateAdoptTried = true;
    void adoptTemplate(target.id);
}

async function loadTemplates(): Promise<void> {
    state.templatesLoading = true;
    try {
        state.templates = await useBackend().listMachineTemplates();
        state.templatesError = null;
    } catch (error) {
        state.templatesError = describeError(error);
    } finally {
        state.templatesLoading = false;
    }
}

function adoptTemplate(id: string) {
    return runOperation(id, 'template-adopt', () => useBackend().adoptTemplateGuest(id), {
        started: 'Pinning the template’s identity and installing this machine’s key',
        finished: 'Adopted. The clone is reachable with its own key; the template’s is retired.',
    });
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

/** Operations return the machine's new view, or nothing when they changed nothing about it. */
type OperationOutcome = OperationResult | undefined;

async function runOperation<R extends OperationOutcome>(
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
        const outcome = result as OperationOutcome;
        if (outcome) {
            applyView(target, 'machineId' in outcome ? outcome : outcome.view);
        }
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
            backend.onContainerRebuildProgress((event) => {
                const target = session(event.machineId);
                const phaseChanged = target.rebuild?.phase !== event.phase;
                target.rebuild = event;
                if (phaseChanged) {
                    note(target, event.detail);
                }
            }),
            backend.onTemplateProgress((event) => {
                const target = session(event.machineId);
                const phaseChanged = target.templateSave?.phase !== event.phase;
                target.templateSave = event;
                if (phaseChanged) {
                    note(target, event.detail);
                }
            }),
            backend.onUsbAttachProgress((event) => {
                const target = session(event.machineId);
                const phaseChanged = target.usbAttach?.phase !== event.phase;
                target.usbAttach = event;
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

/** The operation behind `runningStepFor`, for the summaries and strips that name it. */
function runningOperationFor(id: string): string | null {
    const target = state.sessions[id];
    return target?.operation ?? target?.view?.busyOperation ?? null;
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
        /** The operation itself, so a step that hosts several can say which one. */
        runningOperation: runningOperationFor,

        /**
         * The machine's journey: exact once its view has been probed, coarse from the list
         * summary before that, so the overview and the sidebar can always say where it is.
         */
        journey(id: string): JourneyStep[] {
            const view = state.sessions[id]?.view;
            if (view) {
                return deriveJourney(view, {
                    runningStep: runningStepFor(id),
                    runningOperation: runningOperationFor(id),
                });
            }
            const summary = state.list?.machines.find((machine) => machine.id === id);
            return summary
                ? summarizeJourney(summary, state.list?.host ?? null, runningStepFor(id))
                : [];
        },

        async createMachine(
            profile: MacBuilderConfig,
            templateId: string | null = null,
        ): Promise<string | null> {
            const previous = new Set(state.list?.machines.map((machine) => machine.id));
            state.list = await useBackend().createMachine(profile, templateId);
            if (templateId) {
                void loadTemplates();
            }
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

        launch: (id: string) => {
            // Progress kept from an earlier start would label this one until its first event.
            session(id).launch = null;
            return runOperation(id, 'launch', () => useBackend().launchMachine(id), {
                started: 'Starting the machine',
                finished: 'The macOS machine is running; the Install step shows what comes next.',
            });
        },
        loadTemplates,
        adoptTemplate: (id: string) => {
            session(id).templateAdoptTried = true;
            return adoptTemplate(id);
        },
        /** Shuts macOS down and saves the disk; the machine is stopped afterwards. */
        saveTemplate: (id: string, name: string) => {
            session(id).templateSave = null;
            return runOperation(
                id,
                'save-template',
                async () => {
                    const template = await useBackend().saveMachineTemplate(id, name);
                    state.templates = [
                        template,
                        ...state.templates.filter((entry) => entry.id !== template.id),
                    ];
                    return undefined;
                },
                {
                    started: `Saving ${name}; macOS shuts down first and the machine stays stopped`,
                    finished: `Template ${name} saved. Start the machine again when you need it.`,
                },
            ).then(async (result) => {
                await refreshMachine(id, { silent: true });
                return result;
            });
        },
        async deleteTemplate(templateId: string): Promise<void> {
            state.templates = await useBackend().deleteMachineTemplate(templateId);
        },
        stop: (id: string) =>
            runOperation(id, 'stop', () => useBackend().stopMachine(id), {
                started: 'Stopping the machine safely',
                finished: 'Stopped. The macOS disk is retained and resumes on the next start.',
            }),
        /**
         * A stop and a start, for the one thing only a fresh QEMU fixes: a phone it has already
         * opened once in this session and can no longer read.
         */
        restartMachine: async (id: string) => {
            const stopped = await runOperation(id, 'stop', () => useBackend().stopMachine(id), {
                started: 'Stopping the machine safely',
            });
            if (!stopped) {
                return null;
            }
            session(id).launch = null;
            return runOperation(id, 'launch', () => useBackend().launchMachine(id), {
                started: 'Starting the machine again',
                finished: 'The machine is running again. Attach the phone once it is up.',
            });
        },
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
                    target.optimizations = await useBackend().applyGuestOptimization(
                        id,
                        optimizationId,
                    );
                    return undefined;
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
        attachUsb: (id: string, bus: number, port: string) => {
            session(id).usbAttach = null;
            return runOperation(
                id,
                'usb-attach',
                () => useBackend().attachUsbDevice(id, bus, port),
                {
                    started: 'Passing the iPhone into the guest',
                    finished: (view) =>
                        view.usb.attached?.enumerated
                            ? 'iPhone attached.'
                            : 'The port is handed to the guest, but the phone has not shown up in it yet.',
                },
            );
        },
        /**
         * Rebuilds the container from the machine's profile so it picks up the phone's USB
         * controller; macOS restarts once and the disk is kept.
         */
        rebuildContainer: async (id: string) => {
            session(id).rebuild = null;
            return runOperation(id, 'usb-rebuild', () => useBackend().rebuildMachineContainer(id), {
                started: 'Rebuilding the container with the phone controller',
                finished: 'Rebuilt. The machine is starting; attach the phone once it is up.',
            });
        },
        detachUsb: (id: string) =>
            runOperation(id, 'usb-detach', () => useBackend().detachUsbDevice(id), {
                finished:
                    'iPhone returned to this host. Unplug it and plug it in again before attaching it to this machine again.',
            }),
        /** Runs the CoreDevice pairing; the phone shows Trust if it has not yet, so this waits. */
        pairDevice: (id: string, udid: string) =>
            runOperation(id, 'device-pair', () => useBackend().pairGuestDevice(id, udid), {
                started: 'Pairing with the phone; tap Trust on it when it asks',
                finished: 'Paired. The guest can now install and launch on the phone.',
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
        /**
         * Opens Safari in the guest with its Develop menu; the caller shows what to click. Not an
         * operation: it runs beside a streaming device run, which is when it is wanted, so the
         * caller holds its own in-flight and error state. Throws with the reason.
         */
        openSafariInspector: async (id: string): Promise<SafariInspectorResult> => {
            const target = session(id);
            const result = await useBackend().openSafariWebInspector(id);
            applyView(target, result.view);
            note(
                target,
                result.inspector.developMenuEnabled
                    ? 'Safari is open in the guest console with its Develop menu on.'
                    : 'Safari is open in the guest console; turn its Develop menu on in Safari › Settings › Advanced.',
                'success',
            );
            return result.inspector;
        },
        /** Opens the machine's screen when its provider serves one as a web page. */
        openMachineScreen: async (id: string): Promise<void> => {
            const view = session(id).view;
            if (view?.displayUrl) {
                await useBackend().openMachineScreen(view.displayUrl, view.profile.name);
            }
        },
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
