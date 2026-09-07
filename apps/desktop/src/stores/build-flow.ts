// Keep the guided build alive when the user opens another machine or a prerequisite panel.
import { reactive } from 'vue';

import { describeError } from '../lib/utils';
import {
    executeBuildFlow,
    projectWorkspace,
    requestedVersion,
    type BuildBlocker,
    type BuildRequest,
    type BuildStage,
} from '../model/build-flow';
import { isAndroid } from '../model/providers';
import type { JourneyStepId } from '../model/steps';
import { useMachinesStore } from './machines';

export interface GuidedBuildFailure {
    message: string;
    step: JourneyStepId;
    artifactSha256: string | null;
    deviceSerial: string | null;
}

export interface GuidedBuild {
    request: BuildRequest;
    projectPath: string | null;
    environmentName: string | null;
    status: 'running' | 'stopping' | 'stopped' | 'paused' | 'failed' | 'complete';
    stage: BuildStage | null;
    startedAt: number;
    preparedSnapshot: string | null;
    blocker: BuildBlocker | null;
    /** Captured for this flow; machine refreshes and later operations may clear session.error. */
    failure: GuidedBuildFailure | null;
    /** A preview builds fresh debug output and only then installs that exact APK. */
    androidDeviceSerial?: string;
    androidPreviewSha256?: string | null;
}

const builds = reactive<Record<string, GuidedBuild>>({});
const drafts = reactive<Record<string, BuildRequest>>({});
const draftProjects = new Map<string, string | null>();

async function proceed(id: string, resume: boolean): Promise<void> {
    const build = builds[id]!;
    const machines = useMachinesStore();
    const session = machines.session(id);
    const stopping = () => build.status === 'stopping';
    let failureStep: JourneyStepId = 'test-build';
    const recordFailure = (message: string) => {
        build.failure = {
            message,
            step: failureStep,
            artifactSha256:
                failureStep === 'run-device' ? (build.androidPreviewSha256 ?? null) : null,
            deviceSerial: failureStep === 'run-device' ? (build.androidDeviceSerial ?? null) : null,
        };
    };
    build.status = 'running';
    build.blocker = null;
    build.failure = null;
    try {
        const result = await executeBuildFlow(
            build.request,
            {
                view: () => session.view!,
                stopped: () => build.status === 'stopping',
                stage: (stage) => {
                    build.stage = stage;
                    failureStep = stage === 'attach-env' || stage === 'sync' ? 'project' : stage;
                },
                prepared: (snapshot) => {
                    build.preparedSnapshot = snapshot;
                },
                async run(stage) {
                    if (
                        build.projectPath !== (projectWorkspace(session.view!)?.localPath ?? null)
                    ) {
                        session.error =
                            'The approved project changed. Start a new build for the selected project.';
                        recordFailure(session.error);
                        return false;
                    }
                    const request = build.request;
                    const result = await (() => {
                        switch (stage) {
                            case 'attach-env':
                                return machines.attachEnvSet(id, request.envSetId);
                            case 'sync':
                                return machines.sync(id);
                            case 'test-build':
                                return isAndroid(session.view!.profile.provider)
                                    ? machines
                                          .debugBuild(
                                              id,
                                              request.outcome === 'test' &&
                                                  request.androidAllowHttp,
                                              requestedVersion(session.view!, request),
                                          )
                                          .then((result) => {
                                              if (build.androidDeviceSerial)
                                                  build.androidPreviewSha256 =
                                                      result?.build.apk?.sha256 ?? null;
                                              return result;
                                          })
                                    : machines.testBuild(
                                          id,
                                          request.outcome === 'release'
                                              ? 'device_sdk'
                                              : request.target,
                                          requestedVersion(session.view!, request),
                                      );
                            case 'archive':
                                return machines.signedArchive(
                                    id,
                                    request.envSetId,
                                    requestedVersion(session.view!, request),
                                );
                            case 'release':
                                return machines.signedRelease(
                                    id,
                                    request.envSetId,
                                    request.androidOutputs,
                                    requestedVersion(session.view!, request),
                                );
                        }
                    })();
                    // runOperation treats a requested stop as a neutral null result.
                    if (result === null) {
                        if (session.error) recordFailure(session.error);
                        else build.status = 'stopping';
                    }
                    return result !== null;
                },
            },
            resume ? build.preparedSnapshot : null,
        );
        if (result.status === 'complete' && build.androidDeviceSerial) {
            const apk = session.view?.android?.workspace?.lastBuild?.apk;
            if (!apk || !build.androidPreviewSha256 || apk.sha256 !== build.androidPreviewSha256) {
                session.error =
                    'The preview build did not retain the expected APK. Build again before installing.';
                recordFailure(session.error);
                build.status = 'failed';
                return;
            }
            // Sync can remove the old debug record and make the UI select a retained release.
            // Point it back at the fresh output that this preview is about to install.
            session.androidDeviceApk = 'debug';
            failureStep = 'run-device';
            const previousRun = session.androidDeviceRun;
            const installed = await machines.runAndroidDevice(id, {
                kind: 'debug',
                serial: build.androidDeviceSerial,
                expectedSha256: build.androidPreviewSha256,
            });
            build.status = installed ? 'complete' : stopping() ? 'stopped' : 'failed';
            if (build.status === 'failed') {
                const run = session.androidDeviceRun;
                const matchingError =
                    run !== previousRun &&
                    run?.kind === 'debug' &&
                    run.status === 'failed' &&
                    run.serial === build.androidDeviceSerial &&
                    run.sha256 === build.androidPreviewSha256
                        ? run.error
                        : null;
                recordFailure(
                    matchingError ??
                        session.error ??
                        'The APK could not be installed and opened. Review the device details before retrying.',
                );
            }
        } else build.status = result.status;
        build.blocker = result.status === 'paused' ? result.blocker : null;
    } catch (error) {
        session.error = describeError(error);
        recordFailure(session.error);
        build.status = 'failed';
    }
}

export function useBuildFlowStore() {
    const machines = useMachinesStore();
    return {
        builds,
        draft(id: string): BuildRequest {
            const view = machines.session(id).view;
            const projectPath = view ? (projectWorkspace(view)?.localPath ?? null) : null;
            drafts[id] ??= {
                source: 'latest',
                outcome: view?.archive || view?.android?.release ? 'release' : 'test',
                target: 'device_sdk',
                androidOutputs: 'both',
                androidAllowHttp: false,
                envSetId: view?.envSet?.id ?? null,
                version: null,
            };
            // Read it back out of the record rather than taking what `??=` returned: that
            // operator evaluates to the raw object it assigned, not the reactive one the store
            // keeps, and a step that captured the raw object would write to something nothing
            // is watching — its own choice would never reach the screen.
            const draft = drafts[id];
            if (draftProjects.get(id) !== projectPath) {
                draft.androidAllowHttp = false;
                draft.version = null;
            }
            draftProjects.set(id, projectPath);
            return draft;
        },
        active(id: string): boolean {
            return builds[id]?.status === 'running' || builds[id]?.status === 'stopping';
        },
        async start(
            id: string,
            request: BuildRequest,
            environmentName: string | null,
            androidDeviceSerial?: string,
        ): Promise<void> {
            const session = machines.session(id);
            if (!session.view || session.operation || session.view.busyOperation || this.active(id))
                return;
            builds[id] = {
                request: {
                    ...request,
                    androidAllowHttp:
                        request.outcome === 'test' &&
                        !!projectWorkspace(session.view) &&
                        request.androidAllowHttp,
                },
                projectPath: projectWorkspace(session.view)?.localPath ?? null,
                environmentName,
                status: 'running',
                stage: null,
                startedAt: Date.now(),
                preparedSnapshot: null,
                blocker: null,
                failure: null,
                androidDeviceSerial,
                androidPreviewSha256: null,
            };
            await proceed(id, false);
        },
        async startAndroidPreview(
            id: string,
            serial: string,
            envSetId?: string | null,
            environmentName?: string | null,
        ): Promise<void> {
            const session = machines.session(id);
            if (
                !session.view ||
                !isAndroid(session.view.profile.provider) ||
                !session.androidDevices?.available ||
                !session.androidDevices.devices.some(
                    (device) => device.serial === serial && device.state === 'device',
                )
            ) {
                session.error = 'Refresh devices and select an authorized phone or emulator first.';
                return;
            }
            session.androidDeviceApk = 'debug';
            await this.start(
                id,
                {
                    source: 'latest',
                    outcome: 'test',
                    target: 'device_sdk',
                    androidOutputs: 'both',
                    androidAllowHttp: this.draft(id).androidAllowHttp,
                    envSetId: envSetId === undefined ? (session.view.envSet?.id ?? null) : envSetId,
                    version: this.draft(id).version ?? null,
                },
                envSetId === undefined
                    ? (session.view.envSet?.name ?? null)
                    : (environmentName ?? null),
                serial,
            );
        },
        async resume(id: string): Promise<void> {
            const build = builds[id];
            const session = machines.session(id);
            if (
                !build ||
                build.status !== 'paused' ||
                !session.view ||
                session.operation ||
                session.view.busyOperation
            )
                return;
            // A pause before project approval has no source to preserve yet.
            if (!build.preparedSnapshot) {
                if (build.projectPath !== (projectWorkspace(session.view)?.localPath ?? null))
                    build.request.androidAllowHttp = false;
                build.projectPath = projectWorkspace(session.view)?.localPath ?? null;
            }
            await proceed(id, true);
        },
        async stop(id: string): Promise<void> {
            const build = builds[id];
            if (!build || build.status !== 'running') return;
            build.status = 'stopping';
            await machines.cancelOperation(id);
        },
        dismiss(id: string): void {
            if (!this.active(id)) delete builds[id];
        },
    };
}
