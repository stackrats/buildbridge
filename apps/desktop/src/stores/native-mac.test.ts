import { describe, expect, it, vi } from 'vite-plus/test';

import type {
    MachineChangedEvent,
    NativeMacBuildInput,
    NativeMacBuildResult,
    NativeMacConfig,
    NativeMacProgress,
    NativeMacProjectInput,
    NativeMacStatus,
} from '../types/backend';
import { createNativeMacStore, validNativeCommit, validNativeMinimum } from './native-mac';
import { capacitorLayout } from '../model/project-layout';

function fixture() {
    const result: NativeMacBuildResult = {
        outcome: 'test',
        commit: 'a'.repeat(40),
        repository: 'https://example.com/app.git',
        xcodeVersion: 'Xcode 16.4',
        bundleIdentifier: 'com.example.app',
        envSet: null,
        artifacts: [],
        outputTail: ['Build succeeded'],
    };
    const status: NativeMacStatus = {
        supported: true,
        toolchain: {
            architecture: 'aarch64',
            developerDirectory: '/Applications/Xcode.app/Contents/Developer',
            xcodeVersion: 'Xcode 16.4',
            iosSdk: '18.5',
            availableSdks: 'iOS 18.5',
            nodeVersion: 'v22.0.0',
            pnpmVersion: '10.0.0',
            cocoapodsVersion: '1.16.2',
            issues: [],
        },
        config: {
            project: {
                path: '/Users/Owner/Projects/App',
                name: 'App',
                repository: result.repository,
                bundleIdentifier: result.bundleIdentifier,
                developmentTeam: 'TEAM123456',
                scheme: 'App',
                layout: capacitorLayout(),
                minXcodeVersion: null,
                minIosSdkVersion: null,
            },
            signing: null,
        },
        identities: [],
        testReady: true,
        archiveReady: false,
        issues: [],
        compatibilityWarnings: [],
        busy: false,
        launchAtLogin: false,
        lastBuild: null,
    };
    let progress: (event: NativeMacProgress) => void = () => {};
    let changed: (event: MachineChangedEvent) => void = () => {};
    const unlistenProgress = vi.fn();
    const unlistenChanged = vi.fn();
    const backend = {
        nativeMacStatus: vi.fn(async (_forceRefresh = false) => structuredClone(status)),
        approveNativeMacProject: vi.fn(
            async (input: NativeMacProjectInput): Promise<NativeMacConfig> => {
                status.config = {
                    project: { ...status.config.project!, ...input },
                    signing: null,
                };
                return structuredClone(status.config);
            },
        ),
        configureNativeMacSigning: vi.fn(async () => structuredClone(status.config)),
        runNativeMacBuild: vi.fn(async (input: NativeMacBuildInput) => ({ ...result, ...input })),
        cancelNativeMacBuild: vi.fn(async () => {}),
        setNativeMacLogin: vi.fn(async (enabled: boolean) => {
            status.launchAtLogin = enabled;
        }),
        revealNativeMacArtifacts: vi.fn(async (_path: string) => {}),
        onNativeMacProgress: vi.fn(async (handler: typeof progress) => {
            progress = handler;
            return unlistenProgress;
        }),
        onMachineChanged: vi.fn(async (handler: typeof changed) => {
            changed = handler;
            return unlistenChanged;
        }),
    };
    const store = createNativeMacStore(() => backend);
    return {
        backend,
        store,
        status,
        result,
        progress: (event: NativeMacProgress) => progress(event),
        changed: (event: MachineChangedEvent) => changed(event),
        unlistenProgress,
        unlistenChanged,
    };
}

describe('native Mac build state', () => {
    it('accepts only complete commit IDs and plain optional minimum versions', () => {
        expect(validNativeCommit(` ${'A'.repeat(40)} `)).toBe(true);
        expect(validNativeCommit('b'.repeat(64))).toBe(true);
        for (const invalid of [
            'main',
            'abc1234',
            '--upload-pack=evil',
            'a'.repeat(41),
            'x'.repeat(40),
        ]) {
            expect(validNativeCommit(invalid)).toBe(false);
        }
        for (const version of ['', '16', '16.4', '26.0.1'])
            expect(validNativeMinimum(version)).toBe(true);
        for (const version of ['>=16', '16..4', '16; command', 'v16'])
            expect(validNativeMinimum(version)).toBe(false);
    });

    it('subscribes once, recovers retained results, and keeps drafts during refresh', async () => {
        const { store, backend, status, result, unlistenProgress, unlistenChanged } = fixture();
        status.lastBuild = result;
        await Promise.all([store.initialize(), store.initialize()]);
        expect(backend.onNativeMacProgress).toHaveBeenCalledTimes(1);
        expect(backend.onMachineChanged).toHaveBeenCalledTimes(1);
        expect(store.state.result?.commit).toBe(result.commit);
        expect(store.state.logs).toEqual([{ text: 'Build succeeded' }]);
        expect(store.state.tab).toBe('build');
        store.state.projectPath = '/Users/Owner/Another project';
        store.state.commit = 'c'.repeat(40);
        await store.refresh();
        expect(store.state.projectPath).toBe('/Users/Owner/Another project');
        expect(store.state.commit).toBe('c'.repeat(40));
        store.dispose();
        expect(unlistenProgress).toHaveBeenCalledTimes(1);
        expect(unlistenChanged).toHaveBeenCalledTimes(1);
    });

    it('submits the exact normalized commit and saved environment name once', async () => {
        const { store, backend, result } = fixture();
        await store.initialize();
        let complete!: (value: NativeMacBuildResult) => void;
        backend.runNativeMacBuild.mockImplementation(
            () =>
                new Promise((resolve) => {
                    complete = resolve;
                }),
        );
        store.state.commit = ` ${'C'.repeat(40)} `;
        store.state.envSetName = 'Production';
        const running = store.runBuild();
        await store.runBuild();
        expect(store.busy.value).toBe(true);
        expect(backend.runNativeMacBuild).toHaveBeenCalledTimes(1);
        expect(backend.runNativeMacBuild).toHaveBeenCalledWith({
            commit: 'c'.repeat(40),
            outcome: 'test',
            envSet: 'Production',
        });
        complete({ ...result, commit: 'c'.repeat(40), envSet: 'Production' });
        await running;
        expect(store.state.buildState).toBe('complete');
        expect(store.busy.value).toBe(false);
        store.dispose();
    });

    it('forces an explicit Recheck even when a cached background refresh is already in flight', async () => {
        const { store, backend, status } = fixture();
        await store.initialize();
        expect(backend.nativeMacStatus).toHaveBeenLastCalledWith(false);
        let complete!: (value: NativeMacStatus) => void;
        backend.nativeMacStatus.mockImplementationOnce(
            () =>
                new Promise((resolve) => {
                    complete = resolve;
                }),
        );
        const background = store.refresh();
        const recheck = store.recheck();
        complete(structuredClone(status));
        await Promise.all([background, recheck]);
        expect(backend.nativeMacStatus.mock.calls.slice(-2)).toEqual([[false], [true]]);
        await store.refresh();
        expect(backend.nativeMacStatus).toHaveBeenLastCalledWith(false);
        store.dispose();
    });

    it('blocks unsigned setup gaps and signed archives without signing approval', async () => {
        const { store, backend, status } = fixture();
        await store.initialize();
        store.state.commit = 'a'.repeat(40);
        store.state.outcome = 'archive';
        await store.runBuild();
        expect(backend.runNativeMacBuild).not.toHaveBeenCalled();
        store.state.outcome = 'test';
        status.testReady = false;
        await store.refresh();
        await store.runBuild();
        expect(backend.runNativeMacBuild).not.toHaveBeenCalled();
        store.dispose();
    });

    it('keeps earlier completed artifacts when a requested stop ends the current build', async () => {
        const { store, backend, status, result } = fixture();
        status.lastBuild = result;
        await store.initialize();
        let rejectBuild!: (reason: Error) => void;
        backend.runNativeMacBuild.mockImplementation(
            () =>
                new Promise((_resolve, reject) => {
                    rejectBuild = reject;
                }),
        );
        backend.cancelNativeMacBuild.mockImplementation(async () => {
            rejectBuild(new Error('Operation cancelled.'));
        });
        store.state.commit = 'c'.repeat(40);
        const running = store.runBuild();
        await store.cancel();
        await running;
        expect(store.state.buildState).toBe('stopped');
        expect(store.state.error).toBeNull();
        expect(store.state.result?.commit).toBe(result.commit);
        expect(store.state.stopping).toBe(false);
        store.dispose();
    });

    it('saves trimmed owner input and leaves compatibility minimums explicitly unknown', async () => {
        const { store, backend } = fixture();
        await store.initialize();
        store.state.projectPath = ' /Users/Owner/New app ';
        store.state.minXcode = ' 16.4 ';
        store.state.minIosSdk = '';
        store.state.identitySha1 = 'old identity';
        expect(await store.approveProject()).toBe(true);
        expect(backend.approveNativeMacProject).toHaveBeenCalledWith({
            path: '/Users/Owner/New app',
            minXcodeVersion: '16.4',
            minIosSdkVersion: null,
        });
        expect(store.state.identitySha1).toBe('');
        expect(store.state.status?.config.project?.path).toBe('/Users/Owner/New app');
        store.dispose();
    });

    it('follows native progress with a bounded log and ignores other targets', async () => {
        const { store, progress, changed, backend } = fixture();
        await store.initialize();
        progress({
            machineId: 'some-vm',
            phase: 'build',
            label: 'Other build',
            logLine: 'ignore me',
        });
        expect(store.state.logs).toEqual([]);
        for (let line = 0; line < 600; line += 1) {
            progress({
                machineId: 'native-mac',
                phase: 'build',
                label: 'Compile',
                logLine: `line ${line}`,
            });
        }
        expect(store.state.logs.length).toBe(500);
        expect(store.state.logs.at(-1)?.text).toBe('line 599');
        expect(store.busy.value).toBe(true);
        const calls = backend.nativeMacStatus.mock.calls.length;
        changed({ machineId: 'some-vm' });
        expect(backend.nativeMacStatus).toHaveBeenCalledTimes(calls);
        store.dispose();
    });
});
