// Native builds and their drafts outlive the pane. One subscription follows local and shared
// builds, including when the owner navigates away while Xcode is working.
import { computed, reactive } from 'vue';

import { useBackend, type Backend, type Unlisten } from '../lib/backend';
import { describeError } from '../lib/utils';
import type {
    NativeMacBuildInput,
    NativeMacBuildOutcome,
    NativeMacBuildResult,
    NativeMacConfig,
    NativeMacProgress,
    NativeMacStatus,
} from '../types/backend';

type NativeBackend = Pick<
    Backend,
    | 'nativeMacStatus'
    | 'approveNativeMacProject'
    | 'configureNativeMacSigning'
    | 'runNativeMacBuild'
    | 'cancelNativeMacBuild'
    | 'revealNativeMacArtifacts'
    | 'setNativeMacLogin'
    | 'onNativeMacProgress'
    | 'onMachineChanged'
>;

export function validNativeCommit(commit: string): boolean {
    return /^(?:[a-f\d]{40}|[a-f\d]{64})$/i.test(commit.trim());
}

export function validNativeMinimum(version: string): boolean {
    const value = version.trim();
    return value === '' || (value.length <= 32 && /^\d+(?:\.\d+)*$/.test(value));
}

export function createNativeMacStore(backend: () => NativeBackend = useBackend) {
    const state = reactive({
        status: null as NativeMacStatus | null,
        loading: true,
        refreshing: false,
        saving: null as 'project' | 'signing' | 'login' | null,
        running: false,
        stopping: false,
        tab: 'setup' as 'setup' | 'build',
        error: null as string | null,
        statusError: null as string | null,
        notice: null as string | null,
        progress: null as NativeMacProgress | null,
        logs: [] as { text: string; tone?: 'system' | 'success' | 'stderr' }[],
        result: null as NativeMacBuildResult | null,
        startedAt: null as number | null,
        buildState: 'idle' as 'idle' | 'running' | 'stopped' | 'failed' | 'complete',
        projectPath: '',
        minXcode: '',
        minIosSdk: '',
        identitySha1: '',
        profilePath: '',
        commit: '',
        outcome: 'test' as NativeMacBuildOutcome,
        envSetName: '',
    });
    const busy = computed(() => state.running || state.status?.busy === true || !!state.saving);
    let initialized = false;
    let generation = 0;
    let initializePromise: Promise<void> | null = null;
    let refreshPromise: Promise<void> | null = null;
    let refreshAgain = false;
    let forceNextRefresh = false;
    let unlisteners: Unlisten[] = [];

    function loadDrafts(config: NativeMacConfig): void {
        state.projectPath = config.project?.path ?? '';
        state.minXcode = config.project?.minXcodeVersion ?? '';
        state.minIosSdk = config.project?.minIosSdkVersion ?? '';
        state.identitySha1 = config.signing?.identitySha1 ?? '';
        state.profilePath = config.signing?.profilePath ?? '';
    }

    function append(text: string, tone?: 'system' | 'success' | 'stderr'): void {
        state.logs.push({ text, ...(tone ? { tone } : {}) });
        if (state.logs.length > 500) state.logs.splice(0, state.logs.length - 500);
    }

    function refresh(force = false): Promise<void> {
        forceNextRefresh ||= force;
        if (refreshPromise) {
            refreshAgain = true;
            return refreshPromise;
        }
        state.refreshing = true;
        refreshPromise = (async () => {
            do {
                refreshAgain = false;
                const forceRefresh = forceNextRefresh;
                forceNextRefresh = false;
                try {
                    const status = await backend().nativeMacStatus(forceRefresh);
                    state.status = status;
                    state.statusError = null;
                    if (status.lastBuild) state.result = status.lastBuild;
                    if (!initialized) {
                        loadDrafts(status.config);
                        state.tab = status.testReady ? 'build' : 'setup';
                        if (status.lastBuild && !state.logs.length) {
                            state.logs = status.lastBuild.outputTail
                                .slice(-500)
                                .map((text) => ({ text }));
                        }
                        initialized = true;
                    }
                    if (!status.busy && !state.running && state.buildState === 'running') {
                        state.buildState = 'idle';
                        state.notice =
                            'The shared native operation finished. Its outcome is available in Remote builds.';
                    }
                    if (!status.busy && !state.running) state.stopping = false;
                } catch (error) {
                    state.statusError = describeError(error);
                }
            } while (refreshAgain);
        })().finally(() => {
            state.loading = false;
            state.refreshing = false;
            refreshPromise = null;
        });
        return refreshPromise;
    }

    function receiveProgress(event: NativeMacProgress): void {
        if (event.machineId !== 'native-mac') return;
        if (!state.running && state.buildState !== 'running' && event.phase !== 'completed') {
            state.startedAt = Date.now();
            state.logs = [];
            state.buildState = 'running';
        }
        if (event.phase !== state.progress?.phase) append(event.label, 'system');
        state.progress = event;
        if (event.logLine) append(event.logLine);
        if (event.phase === 'completed') {
            if (!state.running) state.buildState = 'complete';
            void refresh();
        } else if (state.status) {
            state.status.busy = true;
        }
    }

    async function save(
        kind: 'project' | 'signing',
        action: () => Promise<NativeMacConfig>,
    ): Promise<boolean> {
        if (busy.value) return false;
        state.saving = kind;
        state.error = null;
        state.notice = null;
        try {
            const config = await action();
            loadDrafts(config);
            state.notice =
                kind === 'project'
                    ? 'Project approved for this Mac.'
                    : 'App Store signing approved for this project.';
            await refresh();
            return true;
        } catch (error) {
            state.error = describeError(error);
            return false;
        } finally {
            state.saving = null;
        }
    }

    return {
        state,
        busy,
        refresh,
        recheck(): Promise<void> {
            return refresh(true);
        },
        resetSetupDrafts(): void {
            if (state.status) loadDrafts(state.status.config);
        },
        initialize(): Promise<void> {
            if (initializePromise) return initializePromise;
            const current = generation;
            initializePromise = (async () => {
                const subscriptions = await Promise.allSettled([
                    backend().onNativeMacProgress(receiveProgress),
                    backend().onMachineChanged((event) => {
                        if (!event.machineId || event.machineId === 'native-mac') void refresh();
                    }),
                ]);
                for (const result of subscriptions) {
                    if (result.status === 'fulfilled') {
                        if (current === generation) unlisteners.push(result.value);
                        else result.value();
                    } else {
                        state.error = `Could not follow native Mac progress: ${describeError(result.reason)}`;
                    }
                }
                if (current === generation) await refresh();
            })();
            return initializePromise;
        },
        dispose(): void {
            generation += 1;
            for (const unlisten of unlisteners) unlisten();
            unlisteners = [];
            initializePromise = null;
        },
        approveProject(): Promise<boolean> {
            return save('project', () =>
                backend().approveNativeMacProject({
                    path: state.projectPath.trim(),
                    minXcodeVersion: state.minXcode.trim() || null,
                    minIosSdkVersion: state.minIosSdk.trim() || null,
                }),
            );
        },
        configureSigning(): Promise<boolean> {
            return save('signing', () =>
                backend().configureNativeMacSigning({
                    identitySha1: state.identitySha1,
                    profilePath: state.profilePath.trim(),
                }),
            );
        },
        async setLaunchAtLogin(enabled: boolean): Promise<void> {
            if (busy.value) return;
            state.saving = 'login';
            state.error = null;
            try {
                await backend().setNativeMacLogin(enabled);
                await refresh();
            } catch (error) {
                state.error = describeError(error);
            } finally {
                state.saving = null;
            }
        },
        async runBuild(): Promise<void> {
            if (busy.value) return;
            if (!validNativeCommit(state.commit)) {
                state.error = 'Enter the full Git commit ID: 40 or 64 hexadecimal characters.';
                return;
            }
            const ready =
                state.outcome === 'test' ? state.status?.testReady : state.status?.archiveReady;
            if (!ready) {
                state.error = 'Complete the required Mac setup before starting this build.';
                return;
            }
            const input: NativeMacBuildInput = {
                commit: state.commit.trim().toLowerCase(),
                outcome: state.outcome,
                envSet: state.envSetName || null,
            };
            state.running = true;
            state.stopping = false;
            state.error = null;
            state.notice = null;
            state.progress = null;
            state.logs = [];
            state.startedAt = Date.now();
            state.buildState = 'running';
            state.tab = 'build';
            try {
                state.result = await backend().runNativeMacBuild(input);
                state.buildState = 'complete';
                append(
                    input.outcome === 'archive'
                        ? 'Signed IPA and Xcode archive retained.'
                        : 'Compile test passed. This does not launch a preview.',
                    'success',
                );
            } catch (error) {
                if (state.stopping) {
                    state.buildState = 'stopped';
                    state.notice =
                        'Native build stopped. Earlier completed results are still available.';
                } else {
                    state.error = describeError(error);
                    state.buildState = 'failed';
                    append(state.error, 'stderr');
                }
            } finally {
                state.running = false;
                state.stopping = false;
                await refresh();
            }
        },
        async cancel(): Promise<void> {
            if (!busy.value || state.stopping || state.saving) return;
            state.stopping = true;
            try {
                await backend().cancelNativeMacBuild();
            } catch (error) {
                state.stopping = false;
                state.error = describeError(error);
            }
        },
        async reveal(path: string): Promise<void> {
            try {
                await backend().revealNativeMacArtifacts(path);
            } catch (error) {
                state.error = describeError(error);
            }
        },
    };
}

const nativeMac = createNativeMacStore();

export function useNativeMacStore() {
    return nativeMac;
}
