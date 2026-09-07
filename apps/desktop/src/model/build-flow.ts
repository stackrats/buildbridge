// A guided interaction over the engine's existing typed operations. Deliberate decisions
// (project approval, signing provisioning and lockfile adoption) remain separate actions.
import type {
    AndroidReleaseOutputs,
    MachineView,
    ProjectVersion,
    UnsignedBuildTarget,
} from '../types/backend';
import { isAndroid } from './providers';
import { kitReadiness } from './signing';
import type { JourneyStepId } from './steps';

export interface BuildRequest {
    source: 'latest' | 'snapshot';
    outcome: 'test' | 'release';
    target: UnsignedBuildTarget;
    envSetId: string | null;
    androidOutputs: AndroidReleaseOutputs;
    androidAllowHttp: boolean;
    /** An edited version for the release, or null to build the one the project declares. */
    version?: ProjectVersion | null;
}

/**
 * The version a release asks for: the edit, else the project's own, so the artifact matches
 * what the panel shows even when the synced snapshot is older; null when the project declares
 * none buildbridge can read, which leaves the build to the project as synced.
 */
export function requestedVersion(
    view: MachineView,
    request: Pick<BuildRequest, 'version'>,
): ProjectVersion | null {
    return request.version ?? view.projectVersion ?? null;
}

export type BuildStage = 'attach-env' | 'sync' | 'test-build' | 'archive' | 'release';
export interface BuildBlocker {
    step: JourneyStepId;
    message: string;
    action: string;
}
export type BuildFlowResult =
    | { status: 'complete' | 'stopped' | 'failed' }
    | { status: 'paused'; blocker: BuildBlocker };

export function projectWorkspace(view: MachineView) {
    return isAndroid(view.profile.provider) ? view.android?.workspace : view.appleWorkspace;
}

export function buildPrerequisite(view: MachineView): BuildBlocker | null {
    if (!view.runtime.prerequisites.ready) {
        return {
            step: 'host',
            message: 'Resolve this machine’s host checks before building.',
            action: 'Review host checks',
        };
    }
    if (view.runtime.state !== 'running') {
        return {
            step: 'launch',
            message: 'Start this machine to use its prepared toolchain.',
            action: 'Start machine',
        };
    }
    if (!isAndroid(view.profile.provider)) {
        if (!view.guest.ssh.reachable) {
            return {
                step: 'install',
                message: 'Wait for macOS and enable Remote Login before building.',
                action: 'Check macOS access',
            };
        }
        if (view.guest.ssh.trust !== 'trusted') {
            return {
                step: 'trust',
                message: 'Verify the machine’s identity before connecting.',
                action: 'Review identity',
            };
        }
        if (!view.guest.diagnostics.authenticated) {
            return {
                step: 'access',
                message: 'Authorize access to this macOS machine.',
                action: 'Authorize access',
            };
        }
        if (!view.guest.diagnostics.xcodeSelected) {
            return {
                step: view.guest.diagnostics.xcodeVersion ? 'xcode-activate' : 'xcode-import',
                message: 'Prepare Xcode before building this project.',
                action: 'Set up Xcode',
            };
        }
    }
    if (!projectWorkspace(view)) {
        return {
            step: 'project',
            message: 'Choose and approve the local project you want to build.',
            action: 'Choose project',
        };
    }
    return null;
}

export function releasePrerequisite(view: MachineView): BuildBlocker | null {
    const kit = kitReadiness(view.signingKit);
    if (view.signingHealth === 'vault_unavailable' || view.signingHealth === 'kit_missing') {
        return {
            step: 'signing-kit',
            message: 'Restore access to the signing credentials before releasing.',
            action: 'Review signing',
        };
    }
    if (isAndroid(view.profile.provider)) {
        return kit.android
            ? null
            : {
                  step: 'signing-kit',
                  message: 'Choose signing credentials with an Android upload key.',
                  action: 'Choose signing credentials',
              };
    }
    if (view.appleWorkspace?.lastNativeLockUpdated) {
        return {
            step: 'test-build',
            message:
                'The test build updated Podfile.lock. Review and adopt it before releasing; the successful test build can be reused.',
            action: 'Review Podfile.lock',
        };
    }
    if (!kit.archive || !kit.provisionable) {
        return {
            step: 'signing-kit',
            message:
                'Choose signing credentials for an App Store release. A development-only identity can still run on an iPhone.',
            action: 'Choose signing credentials',
        };
    }
    if (!view.signing?.distributionIdentity || view.signingHealth !== 'ready') {
        return {
            step: 'provision',
            message:
                'Review and prepare signing in macOS. This may create certificates and profiles at Apple. Then continue this build.',
            action: 'Review signing setup',
        };
    }
    return null;
}

export interface BuildFlowPorts {
    view(): MachineView;
    run(stage: BuildStage): Promise<boolean>;
    stopped(): boolean;
    stage(stage: BuildStage): void;
    /** Saved only after a successful test, so continuing a paused release reuses that source. */
    prepared(snapshot: string): void;
}

export async function executeBuildFlow(
    request: BuildRequest,
    ports: BuildFlowPorts,
    resumeSnapshot: string | null = null,
): Promise<BuildFlowResult> {
    const initial = ports.view();
    const prerequisite = buildPrerequisite(initial);
    if (prerequisite) return { status: 'paused', blocker: prerequisite };
    if (resumeSnapshot && projectWorkspace(initial)?.lastSnapshotSha256 !== resumeSnapshot) {
        return {
            status: 'paused',
            blocker: {
                step: 'project',
                message:
                    'The prepared source changed while this build was paused. Start a new build to review the source and environment again.',
                action: 'Review source',
            },
        };
    }
    if (request.source === 'snapshot' && !projectWorkspace(initial)?.lastSnapshotSha256) {
        return {
            status: 'paused',
            blocker: {
                step: 'project',
                message:
                    'There is no saved snapshot yet. Choose Latest local source for the first build.',
                action: 'Review source',
            },
        };
    }
    const run = async (stage: BuildStage): Promise<BuildFlowResult | null> => {
        if (ports.stopped()) return { status: 'stopped' };
        ports.stage(stage);
        const succeeded = await ports.run(stage);
        if (ports.stopped()) return { status: 'stopped' };
        return succeeded ? null : { status: 'failed' };
    };
    if (!resumeSnapshot && request.source === 'latest') {
        if ((initial.envSet?.id ?? null) !== request.envSetId) {
            const result = await run('attach-env');
            if (result) return result;
        }
        const result = await run('sync');
        if (result) return result;
    }
    if (!resumeSnapshot || !projectWorkspace(ports.view())?.lastBuildSucceeded) {
        const result = await run('test-build');
        if (result) return result;
    }
    const snapshot = projectWorkspace(ports.view())?.lastSnapshotSha256;
    if (snapshot) ports.prepared(snapshot);
    if (request.outcome === 'test') return { status: 'complete' };
    const releaseBlocker = releasePrerequisite(ports.view());
    if (releaseBlocker) return { status: 'paused', blocker: releaseBlocker };
    const result = await run(isAndroid(ports.view().profile.provider) ? 'release' : 'archive');
    return result ?? { status: 'complete' };
}
