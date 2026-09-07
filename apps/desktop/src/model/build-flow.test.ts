import { describe, expect, it } from 'vite-plus/test';

import { createMockBackend } from '../lib/backend-mock';
import {
    executeBuildFlow,
    projectWorkspace,
    type BuildRequest,
    type BuildStage,
} from './build-flow';

const request: BuildRequest = {
    androidOutputs: 'both',
    androidAllowHttp: false,
    source: 'latest',
    outcome: 'release',
    target: 'device_sdk',
    envSetId: null,
};

async function fixture(machineId = 'default') {
    const view = await createMockBackend().getMachine(machineId);
    view.busyOperation = null;
    view.envSet = null;
    const calls: BuildStage[] = [];
    let stopped = false;
    let prepared: string | null = null;
    const ports = {
        view: () => view,
        stopped: () => stopped,
        stage: (_stage: BuildStage) => {},
        prepared: (snapshot: string) => {
            prepared = snapshot;
        },
        async run(this: void, stage: BuildStage) {
            calls.push(stage);
            if (stage === 'sync') {
                projectWorkspace(view)!.lastSnapshotSha256 = 'fresh-source';
                projectWorkspace(view)!.lastBuildSucceeded = false;
            }
            if (stage === 'test-build') projectWorkspace(view)!.lastBuildSucceeded = true;
            return true;
        },
    };
    return {
        view,
        calls,
        ports,
        stop: () => {
            stopped = true;
        },
        prepared: () => prepared,
    };
}

describe('guided build', () => {
    it('copies latest source, tests it, and releases in order', async () => {
        const { calls, ports, prepared } = await fixture();
        expect(await executeBuildFlow(request, ports)).toEqual({ status: 'complete' });
        expect(calls).toEqual(['sync', 'test-build', 'archive']);
        expect(prepared()).toBe('fresh-source');
    });

    it('applies a deliberately selected environment before taking the snapshot', async () => {
        const { calls, ports } = await fixture();
        await executeBuildFlow({ ...request, envSetId: 'staging' }, ports);
        expect(calls).toEqual(['attach-env', 'sync', 'test-build', 'archive']);
    });

    it('stops the sequence when copying source fails', async () => {
        const { calls, ports } = await fixture();
        ports.run = async (stage) => {
            calls.push(stage);
            return false;
        };
        expect(await executeBuildFlow(request, ports)).toEqual({ status: 'failed' });
        expect(calls).toEqual(['sync']);
    });

    it('does not begin another stage after cancellation races with a successful operation', async () => {
        const { calls, ports, stop } = await fixture();
        const run = ports.run;
        ports.run = async (stage) => {
            const result = await run(stage);
            stop();
            return result;
        };
        expect(await executeBuildFlow(request, ports)).toEqual({ status: 'stopped' });
        expect(calls).toEqual(['sync']);
    });

    it('pauses for signing approval without provisioning or exporting automatically', async () => {
        const { view, calls, ports, prepared } = await fixture();
        view.signing = null;
        view.signingHealth = 'unconfigured';
        expect(await executeBuildFlow(request, ports)).toMatchObject({
            status: 'paused',
            blocker: { step: 'provision' },
        });
        expect(calls).toEqual(['sync', 'test-build']);
        expect(prepared()).toBe('fresh-source');
    });

    it('pauses for lockfile adoption and continues the same tested snapshot afterward', async () => {
        const { view, calls, ports, prepared } = await fixture();
        view.appleWorkspace!.lastNativeLockUpdated = true;
        expect(await executeBuildFlow(request, ports)).toMatchObject({
            status: 'paused',
            blocker: { step: 'test-build' },
        });
        view.appleWorkspace!.lastNativeLockUpdated = false;
        expect(await executeBuildFlow(request, ports, prepared())).toEqual({ status: 'complete' });
        expect(calls).toEqual(['sync', 'test-build', 'archive']);
    });

    it('refuses to continue a different source snapshot after a pause', async () => {
        const { ports, calls } = await fixture();
        expect(await executeBuildFlow(request, ports, 'different-source')).toMatchObject({
            status: 'paused',
            blocker: { step: 'project' },
        });
        expect(calls).toEqual([]);
    });

    it('completes an unsigned test without requiring signing credentials', async () => {
        const { view, ports, calls } = await fixture();
        view.signingKit = null;
        view.signing = null;
        expect(await executeBuildFlow({ ...request, outcome: 'test' }, ports)).toEqual({
            status: 'complete',
        });
        expect(calls).toEqual(['sync', 'test-build']);
    });

    it('does not silently replace local changes when saved source is selected', async () => {
        const { calls, ports } = await fixture();
        expect(
            await executeBuildFlow({ ...request, source: 'snapshot', outcome: 'test' }, ports),
        ).toEqual({ status: 'complete' });
        expect(calls).toEqual(['test-build']);
    });

    it('exports an Android release through the Android stages', async () => {
        const { calls, ports } = await fixture('pixel-builder');
        expect(await executeBuildFlow(request, ports)).toEqual({ status: 'complete' });
        expect(calls).toEqual(['sync', 'test-build', 'release']);
    });

    it('completes an Android debug build without an upload key', async () => {
        const { view, calls, ports } = await fixture('pixel-builder');
        view.signingKit = null;
        expect(await executeBuildFlow({ ...request, outcome: 'test' }, ports)).toEqual({
            status: 'complete',
        });
        expect(calls).toEqual(['sync', 'test-build']);
    });

    it('requires explicit source copying when no saved snapshot exists', async () => {
        const { view, calls, ports } = await fixture();
        view.appleWorkspace!.lastSnapshotSha256 = null;
        expect(await executeBuildFlow({ ...request, source: 'snapshot' }, ports)).toMatchObject({
            status: 'paused',
            blocker: { step: 'project' },
        });
        expect(calls).toEqual([]);
    });
});
