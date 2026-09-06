import { describe, expect, it } from 'vite-plus/test';

import type { HostPrerequisites, MachineView, MachineSummary } from '../types/backend';
import {
    completedCount,
    deriveAndroidSteps,
    deriveBuildSteps,
    deriveJourney,
    derivePublishStep,
    deriveSetupSteps,
    focusStep,
    groupByPhase,
    requiredSteps,
    journeyHeadline,
    summarizeJourney,
} from './steps';

const readyHost: HostPrerequisites = {
    supportedHost: true,
    dockerCli: true,
    dockerDaemon: true,
    dockerVersion: 'Docker version 28.3.2',
    kvmAccess: true,
    tunAccess: true,
    displayAccess: true,
    display: ':1',
    ready: true,
    issues: [],
};

function baseView(overrides: Partial<MachineView> = {}): MachineView {
    return {
        machineId: 'default',
        profile: {
            name: 'Local macOS builder',
            macosRelease: 'sequoia',
            memoryGib: 8,
            cpuCores: 4,
            sshPort: 50922,
            provider: 'docker_osx',
        },
        displayUrl: null,
        busyOperation: null,
        runtime: {
            prerequisites: { ...readyHost },
            state: 'missing',
            containerId: null,
            startedAt: null,
        },
        signingKit: null,
        envSet: null,
        signingHealth: 'unconfigured',
        vaultIssue: null,
        guest: {
            username: null,
            publicKey: null,
            ssh: {
                portOpen: false,
                reachable: false,
                trust: 'unavailable',
                fingerprint: null,
                pinnedFingerprint: null,
                issue: null,
            },
            diagnostics: {
                authenticated: false,
                macosVersion: null,
                xcodeVersion: null,
                xcodePath: null,
                xcodeSelected: false,
                iosSimulatorRuntime: null,
                issue: null,
            },
            devices: [],
        },
        appleWorkspace: null,
        projectVersion: null,
        signing: null,
        archive: null,
        archiveEnvSet: null,
        archiveError: null,
        logs: [],
        usb: {
            host: {
                supported: true,
                rule: 'missing',
                rulePath: '/etc/udev/rules.d/40-buildbridge-iphone.rules',
                usbmuxdActive: true,
                plugdevGid: 46,
                devices: [],
                issues: [],
            },
            diskOnHost: false,
            containerReady: false,
            containerIssue: null,
            qmpReachable: false,
            attached: null,
            phoneController: true,
        },
        deviceRun: null,
        deviceRunError: null,
        template: null,
        android: null,
        ...overrides,
    };
}

function readyView(): MachineView {
    const view = baseView();
    view.runtime.state = 'running';
    view.runtime.containerId = 'abc';
    view.runtime.startedAt = new Date(Date.now() - 90_000).toISOString();
    view.guest.username = 'builder';
    view.guest.publicKey = 'ssh-ed25519 AAAA buildbridge-guest';
    view.guest.ssh = {
        portOpen: true,
        reachable: true,
        trust: 'trusted',
        fingerprint: 'SHA256:abc',
        pinnedFingerprint: 'SHA256:abc',
        issue: null,
    };
    view.guest.diagnostics = {
        authenticated: true,
        macosVersion: '26.6.2',
        xcodeVersion: '26.6',
        xcodePath: '/Users/builder/Applications/Xcode.app',
        xcodeSelected: true,
        iosSimulatorRuntime: '26.5',
        issue: null,
    };
    return view;
}

describe('deriveSetupSteps', () => {
    it('makes starting the machine the next step on a fresh host', () => {
        const steps = deriveSetupSteps(baseView(), { runningStep: null });

        expect(steps.map((step) => step.status)).toEqual([
            'done',
            'active',
            'pending',
            'pending',
            'pending',
            'pending',
            'pending',
        ]);
        expect(focusStep(steps)?.id).toBe('launch');
        expect(steps[1]?.summary).toContain('pulls the Docker-OSX image');
    });

    it('says what unlocks every pending step instead of "waiting"', () => {
        const steps = deriveSetupSteps(baseView(), { runningStep: null });

        for (const step of steps.filter((entry) => entry.status === 'pending')) {
            expect(step.summary.startsWith('after ')).toBe(true);
        }
        expect(steps.find((step) => step.id === 'install')?.summary).toBe(
            'after the machine starts',
        );
    });

    it('carries an expected duration on the one long manual step', () => {
        const steps = deriveSetupSteps(baseView(), { runningStep: null });

        expect(steps.find((step) => step.id === 'install')?.expected).toBe('30 to 60 min');
        expect(steps.find((step) => step.id === 'launch')?.expected).toBeUndefined();
    });

    it('fails the host check and blocks everything when prerequisites are missing', () => {
        const view = baseView();
        view.runtime.prerequisites.ready = false;
        view.runtime.prerequisites.kvmAccess = false;
        view.runtime.prerequisites.issues = ['Grant this user read/write access to /dev/kvm.'];

        const steps = deriveSetupSteps(view, { runningStep: null });

        expect(steps[0]?.status).toBe('failed');
        expect(steps[0]?.summary).toContain('/dev/kvm');
        expect(steps[1]?.status).toBe('pending');
        expect(steps[1]?.summary).toBe('after the host check passes');
        expect(focusStep(steps)?.id).toBe('host');
    });

    it('asks for the manual macOS installation once the machine runs', () => {
        const view = baseView();
        view.runtime.state = 'running';

        const steps = deriveSetupSteps(view, { runningStep: null });
        const install = steps.find((step) => step.id === 'install');

        expect(install?.status).toBe('active');
        expect(install?.kind).toBe('manual');
        expect(focusStep(steps)?.id).toBe('install');
    });

    it('surfaces a changed host key as a failed trust step', () => {
        const view = readyView();
        view.guest.ssh.trust = 'mismatch';
        view.guest.diagnostics.authenticated = false;

        const steps = deriveSetupSteps(view, { runningStep: null });

        expect(steps.find((step) => step.id === 'trust')?.status).toBe('failed');
        expect(focusStep(steps)?.id).toBe('trust');
    });

    it('marks every step done for a prepared machine and reports the reusable toolchain', () => {
        const steps = deriveSetupSteps(readyView(), { runningStep: null });

        expect(completedCount(steps)).toBe(steps.length);
        expect(focusStep(steps)).toBeNull();
        expect(steps.find((step) => step.id === 'xcode-import')?.summary).toBe('Xcode 26.6');
        expect(steps.find((step) => step.id === 'install')?.summary).toContain('macOS 26.6.2');
    });

    it('shows the in-flight operation as running', () => {
        const view = readyView();
        view.guest.diagnostics.xcodeSelected = false;

        const steps = deriveSetupSteps(view, { runningStep: 'xcode-activate' });

        expect(steps.find((step) => step.id === 'xcode-activate')?.status).toBe('running');
        expect(focusStep(steps)?.id).toBe('xcode-activate');
    });

    it('describes a stop on the launch step as a stop, not a start', () => {
        const launch = deriveSetupSteps(readyView(), {
            runningStep: 'launch',
            runningOperation: 'stop',
        }).find((step) => step.id === 'launch');

        expect(launch?.status).toBe('running');
        expect(launch?.summary).toBe('Stopping safely; the container and its macOS disk are kept');
    });

    it('reads the native busy key on the launch step the same way', () => {
        const launch = deriveSetupSteps(readyView(), {
            runningStep: 'launch',
            runningOperation: 'discarding',
        }).find((step) => step.id === 'launch');

        expect(launch?.summary).toBe('Discarding the container and its macOS disk');
    });

    it('names a template save on the launch step, which runs the machine stopped', () => {
        const launch = deriveSetupSteps(readyView(), {
            runningStep: 'launch',
            runningOperation: 'save-template',
        }).find((step) => step.id === 'launch');

        expect(launch?.status).toBe('running');
        expect(launch?.summary).toBe('Saving the machine as a template; macOS is shut down first');
    });

    it('keeps the start summary for the launch step’s own operation', () => {
        const launch = deriveSetupSteps(baseView(), {
            runningStep: 'launch',
            runningOperation: 'launch',
        }).find((step) => step.id === 'launch');

        expect(launch?.summary).toBe('Creating and starting the container');
    });
});

function provisionedView(): MachineView {
    const view = readyView();
    view.appleWorkspace = {
        localPath: '/home/you/projects/example-app',
        name: 'example-app',
        iosWorkspace: 'ios/App/App.xcworkspace',
        scheme: 'App',
        developmentTeam: 'TEAM123456',
        bundleIdentifier: 'com.example.app',
        lastSnapshotSha256: 'abcdef1234567890',
        lastSyncFileCount: 12,
        lastSyncBytes: 2048,
        lastBuildSucceeded: true,
        lastXcodeVersion: '26.6',
        lastNativeLockUpdated: false,
        lastBuildTarget: 'device_sdk',
        debugBundleIdentifier: null,
        lastSource: null,
    };
    view.signing = {
        keychainPath: '/k',
        distributionIdentity: {
            identityName: 'iPhone Distribution: Example Developer (TEAM123456)',
            identitySha1: 'sha1',
            certificateSha256: 'sha256',
            certificateExpiresAt: '2027-09-02T00:00:00Z',
        },
        developmentTeam: 'TEAM123456',
        bundleIdentifier: 'com.example.app',
        profiles: [],
        developmentIdentity: null,
    };
    return view;
}

describe('deriveBuildSteps', () => {
    it('names the operation on a step that hosts more than one', () => {
        const view = provisionedView();
        const at = (id: string, runningOperation: string) =>
            deriveBuildSteps(view, { runningStep: id, runningOperation }).find(
                (step) => step.id === id,
            );

        expect(at('provision', 'clear-signing')?.summary).toBe(
            'Removing the guest keychain and installed profiles',
        );
        expect(at('provision', 'provision')?.summary).toBe(
            'Importing the identity into a dedicated guest keychain',
        );
        expect(at('test-build', 'adopt-lock')?.summary).toBe(
            'Adopting the guest’s Podfile.lock into the project',
        );
        expect(at('test-build', 'test_building')?.summary).toBe(
            'Preparing tools, dependencies, and the unsigned build',
        );
    });

    it('blocks project approval until the machine is prepared', () => {
        const steps = deriveBuildSteps(baseView(), { runningStep: null });

        expect(steps[0]?.status).toBe('pending');
        expect(steps[0]?.summary).toBe('after the machine setup is complete');
    });

    it('walks approve, sync, build, kit, provision, archive in order', () => {
        const view = readyView();
        view.appleWorkspace = {
            localPath: '/home/you/projects/example-app',
            name: 'app',
            iosWorkspace: 'ios/App/App.xcworkspace',
            scheme: 'App',
            developmentTeam: 'TEAM123456',
            bundleIdentifier: 'nz.co.example.app',
            lastSnapshotSha256: 'abcdef1234567890',
            lastSyncFileCount: 12,
            lastSyncBytes: 2048,
            lastBuildSucceeded: true,
            lastXcodeVersion: '26.6',
            lastNativeLockUpdated: false,
            lastBuildTarget: 'device_sdk',
            debugBundleIdentifier: null,
            lastSource: null,
        };

        const steps = deriveBuildSteps(view, { runningStep: null });

        expect(steps.map((step) => [step.id, step.status])).toEqual([
            ['approve', 'done'],
            ['sync', 'done'],
            ['test-build', 'done'],
            ['signing-kit', 'active'],
            ['provision', 'pending'],
            ['archive', 'pending'],
            ['run-device', 'pending'],
        ]);
        expect(focusStep(steps)?.id).toBe('signing-kit');
        expect(steps.find((step) => step.id === 'provision')?.summary).toBe(
            'after complete credentials are attached',
        );
    });

    it('blocks the signed archive while the guest lockfile drifted', () => {
        const view = readyView();
        view.appleWorkspace = {
            localPath: '/home/you/projects/example-app',
            name: 'app',
            iosWorkspace: 'ios/App/App.xcworkspace',
            scheme: 'App',
            developmentTeam: 'TEAM123456',
            bundleIdentifier: 'nz.co.example.app',
            lastSnapshotSha256: 'abcdef1234567890',
            lastSyncFileCount: 12,
            lastSyncBytes: 2048,
            lastBuildSucceeded: true,
            lastXcodeVersion: '26.6',
            lastNativeLockUpdated: true,
            lastBuildTarget: 'simulator',
            debugBundleIdentifier: null,
            lastSource: null,
        };
        view.signingKit = {
            id: 'team',
            name: 'Example team',
            appStoreConnectConfigured: false,
            appStoreConnectKeyId: null,
            signingCertificateConfigured: true,
            signingCertificateName: 'dist.p12',
            signingCertificatePasswordStored: true,
            provisioningProfileNames: ['app.mobileprovision'],
            guestKeychainConfigured: true,
            createdAtEpochSeconds: 0,
            attachedMachines: ['Local macOS builder'],
            developmentCertificateConfigured: false,
            developmentCertificateName: null,
            developmentCertificatePasswordStored: false,
            androidKeystoreConfigured: false,
            androidKeystoreName: null,
            androidKeyAlias: null,
            androidKeystorePasswordStored: false,
            androidKeyPasswordStored: false,
        };
        view.signing = {
            keychainPath: '/k',
            distributionIdentity: {
                identityName: 'iPhone Distribution: Example (TEAM123456)',
                identitySha1: 'sha1',
                certificateSha256: 'sha256',
                certificateExpiresAt: '2027-09-02T00:00:00Z',
            },
            developmentTeam: 'TEAM123456',
            bundleIdentifier: 'nz.co.example.app',
            profiles: [],
            developmentIdentity: null,
        };

        const steps = deriveBuildSteps(view, { runningStep: null });
        const archive = steps.find((step) => step.id === 'archive');

        expect(archive?.status).toBe('active');
        expect(archive?.summary).toContain('Podfile.lock');
    });

    it('reports a retained archive failure', () => {
        const view = readyView();
        view.archiveError = 'xcodebuild: error: exportArchive failed';

        const steps = deriveBuildSteps(view, { runningStep: null });

        expect(steps.find((step) => step.id === 'archive')?.status).toBe('failed');
        expect(focusStep(steps)?.id).toBe('archive');
    });
    it('reports a cleared keyring as a failure, not as a fresh install', () => {
        // The state a recreated operating-system keyring leaves: signing.json survives on disk
        // while the vault that produced it is empty.
        const view = provisionedView();
        view.signingKit = null;
        view.signingHealth = 'kit_missing';

        const steps = deriveBuildSteps(view, { runningStep: null });
        const kitStep = steps.find((step) => step.id === 'signing-kit');

        expect(kitStep?.status).toBe('failed');
        expect(kitStep?.summary).toContain('no longer stored');
        expect(focusStep(steps)?.id).toBe('signing-kit');
    });

    it('blocks a signed archive while the kit that unlocks the keychain is missing', () => {
        const view = provisionedView();
        view.signingKit = null;
        view.signingHealth = 'kit_missing';

        const archive = deriveBuildSteps(view, { runningStep: null }).find(
            (step) => step.id === 'archive',
        );

        expect(archive?.status).toBe('pending');
        expect(archive?.summary).toContain('keychain password is needed');
    });

    it('surfaces an unreadable vault with the reason the backend gave', () => {
        const view = provisionedView();
        view.signingKit = null;
        view.signingHealth = 'vault_unavailable';
        view.vaultIssue = 'the collection is locked';

        const kitStep = deriveBuildSteps(view, { runningStep: null }).find(
            (step) => step.id === 'signing-kit',
        );

        expect(kitStep?.status).toBe('failed');
        expect(kitStep?.summary).toContain('the collection is locked');
    });

    it('still reads as done when a complete kit is attached', () => {
        const view = provisionedView();
        view.signingHealth = 'ready';
        view.signingKit = {
            id: 'team',
            name: 'Example team',
            appStoreConnectConfigured: false,
            appStoreConnectKeyId: null,
            signingCertificateConfigured: true,
            signingCertificateName: 'dist.p12',
            signingCertificatePasswordStored: true,
            provisioningProfileNames: ['app.mobileprovision'],
            guestKeychainConfigured: true,
            createdAtEpochSeconds: 0,
            attachedMachines: ['macOS builder'],
            developmentCertificateConfigured: false,
            developmentCertificateName: null,
            developmentCertificatePasswordStored: false,
            androidKeystoreConfigured: false,
            androidKeystoreName: null,
            androidKeyAlias: null,
            androidKeystorePasswordStored: false,
            androidKeyPasswordStored: false,
        };

        const steps = deriveBuildSteps(view, { runningStep: null });

        expect(steps.find((step) => step.id === 'signing-kit')?.status).toBe('done');
        expect(steps.find((step) => step.id === 'archive')?.status).toBe('active');
    });
});

describe('development-only kit', () => {
    function developmentOnlyView(): MachineView {
        const view = provisionedView();
        view.signingHealth = 'ready';
        view.signingKit = {
            id: 'dev-kit',
            name: 'Dev kit',
            appStoreConnectConfigured: false,
            appStoreConnectKeyId: null,
            signingCertificateConfigured: false,
            signingCertificateName: null,
            signingCertificatePasswordStored: false,
            provisioningProfileNames: [],
            guestKeychainConfigured: true,
            createdAtEpochSeconds: 0,
            attachedMachines: ['Local macOS builder'],
            developmentCertificateConfigured: true,
            developmentCertificateName: 'development.p12',
            developmentCertificatePasswordStored: true,
            androidKeystoreConfigured: false,
            androidKeystoreName: null,
            androidKeyAlias: null,
            androidKeystorePasswordStored: false,
            androidKeyPasswordStored: false,
        };
        view.signing = {
            ...view.signing!,
            distributionIdentity: null,
            developmentIdentity: {
                identityName: 'Apple Development: Example Developer (TEAM123456)',
                identitySha1: 'dev1',
                certificateSha256: 'devsha256',
                certificateExpiresAt: '2027-09-02T00:00:00Z',
            },
        };
        return view;
    }

    it('attaches and provisions, and says the archive stays locked', () => {
        const steps = deriveBuildSteps(developmentOnlyView(), { runningStep: null });
        const byId = (id: string) => steps.find((step) => step.id === id)!;
        expect(byId('signing-kit').status).toBe('done');
        expect(byId('signing-kit').summary).toContain('development identity only');
        expect(byId('provision').status).toBe('done');
        expect(byId('provision').summary).toContain('development only');
        expect(byId('archive').status).toBe('pending');
        expect(byId('archive').summary).toContain(
            'Locked: the credentials hold only a development identity',
        );
        expect(byId('run-device').status).toBe('active');
        expect(journeyHeadline(steps)).toBe('device builds ready · release signing needed');
    });

    it('describes the Team key route as creating distribution signing when provisioned', () => {
        const view = developmentOnlyView();
        view.signingKit!.appStoreConnectConfigured = true;
        view.signingKit!.appStoreConnectKeyId = 'KEYID12345';
        view.signing = null;
        const steps = deriveBuildSteps(view, { runningStep: null });
        const credentials = steps.find((step) => step.id === 'signing-kit')!;
        expect(credentials.status).toBe('done');
        expect(credentials.summary).toContain('distribution signing created when provisioned');
        expect(steps.find((step) => step.id === 'provision')?.status).toBe('active');
    });

    it('does not report completion when a required step is pending without an active step', () => {
        const steps = deriveBuildSteps(developmentOnlyView(), { runningStep: null });
        const device = steps.find((step) => step.id === 'run-device')!;
        device.status = 'pending';
        expect(journeyHeadline(steps)).toBe('signed archive is not ready');
    });

    it('names what an unfinished kit still lacks', () => {
        const view = developmentOnlyView();
        // Without a Team key nothing would create the missing piece, so the gap is real.
        view.signingKit = {
            ...view.signingKit!,
            developmentCertificatePasswordStored: false,
            androidKeystoreConfigured: false,
            androidKeystoreName: null,
            androidKeyAlias: null,
            androidKeystorePasswordStored: false,
            androidKeyPasswordStored: false,
            appStoreConnectConfigured: false,
            appStoreConnectKeyId: null,
        };
        view.signing = null;
        const step = deriveBuildSteps(view, { runningStep: null }).find(
            (step) => step.id === 'signing-kit',
        )!;
        expect(step.status).toBe('active');
        expect(step.summary).toContain('development identity export password missing');
    });
});

describe('unsigned build target', () => {
    it('names the SDK the test build compiled against, and nothing for older records', () => {
        const view = provisionedView();
        view.appleWorkspace!.lastBuildTarget = 'device_sdk';
        expect(
            deriveBuildSteps(view, { runningStep: null }).find((step) => step.id === 'test-build')
                ?.summary,
        ).toBe('Built with Xcode 26.6 · device SDK');
        view.appleWorkspace!.lastBuildTarget = 'simulator';
        expect(
            deriveBuildSteps(view, { runningStep: null }).find((step) => step.id === 'test-build')
                ?.summary,
        ).toBe('Built with Xcode 26.6 · Simulator');
        view.appleWorkspace!.lastBuildTarget = null;
        expect(
            deriveBuildSteps(view, { runningStep: null }).find((step) => step.id === 'test-build')
                ?.summary,
        ).toBe('Built with Xcode 26.6');
    });
});

describe('deriveJourney', () => {
    it('offers publishing only for a retained release and never treats a local build as published', () => {
        const missing = derivePublishStep(false);
        const retained = derivePublishStep(true);
        expect(missing.status).toBe('pending');
        expect(retained.status).toBe('active');
        expect(retained.optional).toBe(true);
        expect(requiredSteps([retained])).toEqual([]);
        expect(focusStep([retained])).toBeNull();
        expect(completedCount([retained])).toBe(0);
        expect(retained.summary).toContain('publication is not checked');
    });
    it('keeps device preview and publishing optional after setup and build', () => {
        const steps = deriveJourney(baseView(), { runningStep: null });

        expect(steps).toHaveLength(15);
        expect(steps.slice(0, 7).every((step) => step.phase === 'setup')).toBe(true);
        expect(steps.slice(7, 13).every((step) => step.phase === 'build')).toBe(true);
        expect(steps[13]?.phase).toBe('device');
        expect(steps[14]?.phase).toBe('publish');
        // The count a machine is measured by leaves the optional step out.
        expect(requiredSteps(steps)).toHaveLength(13);
    });

    it('focuses a setup failure before anything in the build phase', () => {
        const view = provisionedView();
        view.guest.ssh.trust = 'mismatch';
        view.archiveError = 'exportArchive failed';

        const steps = deriveJourney(view, { runningStep: null });

        expect(focusStep(steps)?.id).toBe('trust');
    });

    it('groups into phases with counts, completeness and a focus per phase', () => {
        const groups = groupByPhase(deriveJourney(readyView(), { runningStep: null }));

        expect(groups.map((group) => [group.phase, group.done, group.complete])).toEqual([
            ['setup', 7, true],
            ['build', 0, false],
            ['device', 0, false],
            ['publish', 0, false],
        ]);
        expect(groups[0]?.focus).toBeNull();
        expect(groups[1]?.focus?.id).toBe('approve');
    });
});

function summary(overrides: Partial<MachineSummary> = {}): MachineSummary {
    return {
        id: 'team-mac',
        platform: 'ios',
        config: {
            name: 'Team Mac',
            macosRelease: 'sequoia',
            memoryGib: 8,
            cpuCores: 4,
            sshPort: 50923,
            provider: 'docker_osx',
        },
        createdAtEpochSeconds: 0,
        state: 'missing',
        containerId: null,
        busyOperation: null,
        guestConfigured: false,
        trustPinned: false,
        workspaceName: null,
        signingKitName: null,
        signingProvisioned: false,
        signingIdentity: null,
        archiveRetained: false,
        envSetName: null,
        usbReady: false,
        deviceRunRetained: false,
        templateName: null,
        ...overrides,
    };
}

describe('summarizeJourney', () => {
    it('reads a fresh machine as ready to start', () => {
        const steps = summarizeJourney(summary(), readyHost);

        expect(steps).toHaveLength(15);
        expect(focusStep(steps)?.id).toBe('launch');
        expect(steps.filter((step) => step.status === 'done').map((step) => step.id)).toEqual([
            'host',
        ]);
        expect(steps.find((step) => step.id === 'install')?.summary).toBe(
            'after the machine starts',
        );
    });

    it('infers the steps between the flags the list carries', () => {
        const steps = summarizeJourney(
            summary({
                state: 'running',
                guestConfigured: true,
                trustPinned: true,
                workspaceName: 'example-app',
                signingKitName: 'Example team',
                signingProvisioned: true,
                signingIdentity: 'iPhone Distribution: Example',
                archiveRetained: true,
                deviceRunRetained: true,
                templateName: null,
            }),
            readyHost,
        );

        expect(requiredSteps(steps).every((step) => step.status === 'done')).toBe(true);
        expect(steps.find((step) => step.id === 'publish')?.status).toBe('active');
        expect(steps.find((step) => step.id === 'approve')?.summary).toBe('example-app');
        expect(journeyHeadline(steps)).toBe('signed IPA retained');
    });

    it('blocks the journey on a host that is not ready', () => {
        const steps = summarizeJourney(summary(), {
            ...readyHost,
            ready: false,
            kvmAccess: false,
            issues: ['No KVM'],
        });

        expect(steps[0]?.status).toBe('failed');
        expect(steps.slice(1).every((step) => step.status === 'pending')).toBe(true);
        expect(journeyHeadline(steps)).toBe('check the host needs attention');
    });

    it('checks only the requirements of each provider before the machine probe arrives', () => {
        const host = {
            ...readyHost,
            ready: false,
            kvmAccess: false,
            displayAccess: false,
            issues: ['No KVM or display'],
        };
        const android = summary();
        android.config.provider = 'android_toolchain';
        expect(summarizeJourney(android, host)[0]?.status).toBe('done');
        const dockur = summary();
        dockur.config.provider = 'dockur_macos';
        const steps = summarizeJourney(dockur, { ...host, kvmAccess: true });
        expect(steps[0]?.status).toBe('done');
        expect(steps[0]?.summary).toBe('Docker, KVM and tun ready');
    });

    it('shows an unprobed host as pending, with a reason', () => {
        const steps = summarizeJourney(summary(), null);
        expect(steps[0]?.status).toBe('pending');
        expect(steps[0]?.summary).toBe('Checking host requirements');
        expect(steps.some((step) => step.status === 'failed')).toBe(false);
    });

    it('shows the running step when the list reports a busy machine', () => {
        const steps = summarizeJourney(summary({ state: 'running' }), readyHost, 'launch');

        expect(steps.find((step) => step.id === 'launch')?.status).toBe('running');
        expect(journeyHeadline(steps)).toBe('running: start the machine');
    });
});

describe('journeyHeadline', () => {
    it('names the next step and its version once signed', () => {
        expect(journeyHeadline(deriveJourney(baseView(), { runningStep: null }))).toBe(
            'next: start the machine',
        );
        const view = provisionedView();
        view.archive = {
            scheme: 'App',
            configuration: 'Release',
            exportMethod: 'app-store-connect',
            bundleIdentifier: 'com.example.app',
            developmentTeam: 'TEAM123456',
            marketingVersion: '3.2.0',
            buildNumber: '15',
            provisioningProfileUuid: 'uuid',
            ipa: { path: '/a.ipa', bytes: 10, sha256: 'a' },
            archive: { path: '/a.zip', bytes: 20, sha256: 'b' },
            outputTail: [],
        };
        view.signingKit = {
            id: 'team',
            name: 'Example team',
            appStoreConnectConfigured: false,
            appStoreConnectKeyId: null,
            signingCertificateConfigured: true,
            signingCertificateName: 'dist.p12',
            signingCertificatePasswordStored: true,
            provisioningProfileNames: ['app.mobileprovision'],
            guestKeychainConfigured: true,
            createdAtEpochSeconds: 0,
            attachedMachines: ['Local macOS builder'],
            developmentCertificateConfigured: false,
            developmentCertificateName: null,
            developmentCertificatePasswordStored: false,
            androidKeystoreConfigured: false,
            androidKeystoreName: null,
            androidKeyAlias: null,
            androidKeystorePasswordStored: false,
            androidKeyPasswordStored: false,
        };
        expect(journeyHeadline(deriveJourney(view, { runningStep: null }))).toBe(
            'signed 3.2.0 (15)',
        );
    });
});

describe('run on the device', () => {
    const at = (view: MachineView, runningStep: string | null = null) =>
        deriveJourney(view, { runningStep }).find((step) => step.id === 'run-device')!;
    /** Provisioned with a complete kit attached, so the archive is the only required step left. */
    const kittedView = () => {
        const view = provisionedView();
        view.signingKit = {
            id: 'team',
            name: 'Example team',
            appStoreConnectConfigured: true,
            appStoreConnectKeyId: 'KEYID12345',
            signingCertificateConfigured: true,
            signingCertificateName: 'dist.p12',
            signingCertificatePasswordStored: true,
            provisioningProfileNames: ['app.mobileprovision'],
            guestKeychainConfigured: true,
            createdAtEpochSeconds: 0,
            attachedMachines: ['Local macOS builder'],
            developmentCertificateConfigured: false,
            developmentCertificateName: null,
            developmentCertificatePasswordStored: false,
            androidKeystoreConfigured: false,
            androidKeystoreName: null,
            androidKeyAlias: null,
            androidKeystorePasswordStored: false,
            androidKeyPasswordStored: false,
        };
        return view;
    };

    it('waits for signing like the archive does', () => {
        const step = at(baseView());
        expect(step.status).toBe('pending');
        expect(step.summary).toBe('after signing is provisioned');
        expect(step.optional).toBe(true);
        expect(step.experimental).toBe(true);
    });

    it('opens with the archive but never takes the focus from it', () => {
        const view = kittedView();
        const steps = deriveJourney(view, { runningStep: null });
        const step = steps.find((candidate) => candidate.id === 'run-device')!;
        expect(step.status).toBe('active');
        expect(step.summary).toContain('udev rule');
        expect(focusStep(steps)?.id).toBe('archive');
    });

    it('leaves the headline to the signed archive once that is retained', () => {
        const view = kittedView();
        view.archive = {
            scheme: 'App',
            configuration: 'Release',
            exportMethod: 'app-store-connect',
            bundleIdentifier: 'com.example.app',
            developmentTeam: 'TEAM123456',
            marketingVersion: '3.2.0',
            buildNumber: '15',
            provisioningProfileUuid: 'uuid',
            ipa: { path: '/a.ipa', bytes: 10, sha256: 'a' },
            archive: { path: '/a.zip', bytes: 20, sha256: 'b' },
            outputTail: [],
        };
        const steps = deriveJourney(view, { runningStep: null });
        expect(steps.find((step) => step.id === 'run-device')?.status).toBe('active');
        expect(focusStep(steps)).toBeNull();
        expect(journeyHeadline(steps)).toBe('signed 3.2.0 (15)');
    });

    it('describes the running operation from the facts', () => {
        const step = at(provisionedView(), 'run-device');
        expect(step.status).toBe('running');
        expect(step.live).toBeUndefined();
        expect(step.summary).toContain('udev rule');
    });

    it('is live, not merely running, once the app is up on the phone', () => {
        const step = deriveJourney(provisionedView(), {
            runningStep: 'run-device',
            runningLive: true,
        }).find((candidate) => candidate.id === 'run-device')!;
        expect(step.status).toBe('running');
        expect(step.live).toBe(true);
        expect(step.summary).toBe('Running on the iPhone; console streaming');
        // Live belongs to the device run alone; nothing else in the journey picks it up.
        const others = deriveJourney(provisionedView(), {
            runningStep: 'archive',
            runningLive: true,
        });
        expect(others.every((candidate) => candidate.live === undefined)).toBe(true);
    });

    it('is done with the last run and failed when the last run failed', () => {
        const view = provisionedView();
        view.deviceRun = {
            device: {
                identifier: 'E3F1A2B4-5C6D-4E7F-8A9B-0C1D2E3F4A5B',
                udid: '00008030-000A1B2C3D4E5F6A',
                name: 'Matt’s iPhone',
                osVersion: '18.6',
                model: 'iPhone 15 Pro',
                developerMode: 'enabled',
                pairingState: 'paired',
                tunnelState: 'connected',
                transportType: 'wired',
                ready: true,
                issue: null,
            },
            bundleIdentifier: 'com.example.app',
            appPath: '/Users/builder/App.app',
            marketingVersion: '3.2.0',
            buildNumber: '15',
            provisioningProfileUuid: '22222222-3333-4444-5555-666666666666',
            installedAtEpochSeconds: Math.floor(Date.now() / 1000) - 120,
            consoleEnd: 'stopped',
            exitStatus: null,
            reattached: false,
            buildTail: [],
            consoleTail: [],
            projectBundleIdentifier: null,
        };
        const done = at(view);
        expect(done.status).toBe('done');
        expect(done.summary).toContain('Matt’s iPhone · 3.2.0 (15)');

        view.deviceRunError = 'devicectl: install failed';
        const steps = deriveJourney(view, { runningStep: null });
        expect(steps.find((step) => step.id === 'run-device')?.status).toBe('failed');
        expect(focusStep(steps)?.id).toBe('run-device');
    });

    it('is blocked with the archive when the kit is gone', () => {
        const view = provisionedView();
        view.signingHealth = 'kit_missing';
        expect(at(view).summary).toContain('Blocked');
    });

    it('has its own coarse row, pending until a run was retained', () => {
        const fresh = summarizeJourney(summary(), readyHost).find(
            (step) => step.id === 'run-device',
        )!;
        expect(fresh.id).toBe('run-device');
        expect(fresh.optional).toBe(true);
        expect(fresh.status).toBe('pending');
        expect(fresh.summary).toBe('after signing is provisioned');

        const retained = summarizeJourney(summary({ deviceRunRetained: true }), readyHost).find(
            (step) => step.id === 'run-device',
        )!;
        expect(retained.status).toBe('done');
        expect(retained.summary).toBe('Ran on the iPhone');
    });
});

describe('a machine cloned from a template', () => {
    function cloneView(): MachineView {
        const view = readyView();
        view.template = { id: 'xcode-26-ready', name: 'Xcode 26 ready' };
        view.guest.ssh = { ...view.guest.ssh, trust: 'untrusted', pinnedFingerprint: null };
        view.guest.diagnostics = { ...view.guest.diagnostics, authenticated: false };
        return view;
    }

    it('turns the install, trust and access steps into BuildBridge’s own', () => {
        const steps = deriveSetupSteps(cloneView(), { runningStep: null });
        const byId = (id: string) => steps.find((step) => step.id === id)!;

        expect(byId('install').kind).toBe('automatic');
        expect(byId('install').title).toBe('Boot macOS from the template');
        expect(byId('install').expected).toBeUndefined();
        expect(byId('trust').kind).toBe('automatic');
        expect(byId('trust').status).toBe('active');
        expect(byId('trust').summary).toContain('Xcode 26 ready');
        expect(byId('access').kind).toBe('automatic');
    });

    it('names the template while macOS is still booting', () => {
        const view = cloneView();
        view.guest.ssh = { ...view.guest.ssh, reachable: false, portOpen: false };
        const install = deriveSetupSteps(view, { runningStep: null }).find(
            (step) => step.id === 'install',
        )!;

        expect(install.status).toBe('active');
        expect(install.summary).toBe(
            'Starting the macOS saved in Xcode 26 ready; nothing to install',
        );
    });
});

/** An Android machine's view: no guest, an `android` section instead. */
function androidView(overrides: Partial<MachineView> = {}): MachineView {
    return baseView({
        machineId: 'pixel',
        profile: {
            name: 'Android builder',
            macosRelease: 'sequoia',
            memoryGib: 6,
            cpuCores: 4,
            sshPort: 50924,
            provider: 'android_toolchain',
        },
        runtime: {
            prerequisites: { ...readyHost, kvmAccess: false, displayAccess: false },
            state: 'running',
            containerId: 'abc',
            startedAt: null,
        },
        android: { workspace: null, release: null, releaseEnvSet: null, releaseError: null },
        ...overrides,
    });
}

function androidKit(complete = true): NonNullable<MachineView['signingKit']> {
    return {
        id: 'team',
        name: 'Team kit',
        appStoreConnectConfigured: false,
        appStoreConnectKeyId: null,
        signingCertificateConfigured: false,
        signingCertificateName: null,
        signingCertificatePasswordStored: false,
        provisioningProfileNames: [],
        guestKeychainConfigured: false,
        createdAtEpochSeconds: 0,
        attachedMachines: [],
        developmentCertificateConfigured: false,
        developmentCertificateName: null,
        developmentCertificatePasswordStored: false,
        androidKeystoreConfigured: complete,
        androidKeystoreName: complete ? 'upload.keystore' : null,
        androidKeyAlias: complete ? 'upload' : null,
        androidKeystorePasswordStored: complete,
        androidKeyPasswordStored: false,
    };
}

describe('an Android machine', () => {
    it('has seven required steps followed by optional device preview and publishing', () => {
        const steps = deriveJourney(androidView(), { runningStep: null });

        expect(steps.map((step) => step.id)).toEqual([
            'host',
            'launch',
            'approve',
            'sync',
            'test-build',
            'signing-kit',
            'release',
            'run-device',
            'publish',
        ]);
        expect(steps.slice(0, 2).every((step) => step.phase === 'setup')).toBe(true);
        expect(steps.slice(2, 7).every((step) => step.phase === 'build')).toBe(true);
        expect(requiredSteps(steps)).toHaveLength(7);
        expect(groupByPhase(steps).map((group) => group.phase)).toEqual([
            'setup',
            'build',
            'device',
            'publish',
        ]);
    });

    it('requires an APK for the device guide, but not running Docker or release credentials', () => {
        const view = androidView();
        view.runtime.state = 'exited';
        view.signingKit = null;
        view.android!.release = {
            applicationId: 'com.example.retained',
            versionName: '2.0',
            versionCode: '20',
            keyAlias: 'upload',
            certificateSha256: 'a'.repeat(64),
            outputTail: [],
            aab: { path: '/tmp/app.aab', bytes: 120, sha256: 'b'.repeat(64) },
            apk: null,
        };
        const device = () =>
            deriveJourney(view, { runningStep: null }).find((step) => step.id === 'run-device')!;
        expect(device().status).toBe('pending');
        view.android!.release.apk = { path: '/tmp/app.apk', bytes: 200, sha256: 'c'.repeat(64) };
        expect(device().status).toBe('active');
        expect(device().optional).toBe(true);
        expect(requiredSteps([device()])).toHaveLength(0);
        expect(completedCount([device()])).toBe(0);
    });

    it('does not infer a device APK from the coarse retained-release flag', () => {
        const row = summary({ archiveRetained: true });
        row.config.provider = 'android_toolchain';
        const device = summarizeJourney(row, readyHost).find((step) => step.id === 'run-device')!;
        expect(device.status).toBe('pending');
        expect(device.summary).toContain('check for a retained APK');
        expect(device.optional).toBe(true);
    });

    it('passes the host check without KVM or a display', () => {
        const steps = deriveAndroidSteps(androidView(), { runningStep: null });

        expect(steps[0]?.status).toBe('done');
        expect(steps[0]?.summary).toContain('no virtual machine');
        expect(steps[1]?.status).toBe('done');
        expect(steps[2]?.status).toBe('active');
    });

    it('walks approve, sync, debug build, kit, release in order', () => {
        const view = androidView();
        view.android!.workspace = {
            localPath: '/home/you/app',
            name: 'app',
            applicationId: 'com.example.app',
            lastSnapshotSha256: 'abc123def456',
            lastSyncFileCount: 10,
            lastSyncBytes: 1000,
            lastBuildSucceeded: true,
            lastBuild: {
                allowHttp: false,
                applicationId: 'com.example.app.debug',
                versionName: '1.0',
                versionCode: '3',
                toolchain: {
                    jdkVersion: 'openjdk version "17.0.20" 2026',
                    buildToolsVersion: '35.0.0',
                },
                apk: { path: '/a/debug-1-1/app-debug.apk', bytes: 33_000_000, sha256: 'cc' },
                outputTail: [],
            },
            lastSource: null,
        };
        view.signingKit = androidKit();
        view.signingHealth = 'ready';

        const steps = deriveAndroidSteps(view, { runningStep: null });

        expect(steps.map((step) => [step.id, step.status])).toEqual([
            ['host', 'done'],
            ['launch', 'done'],
            ['approve', 'done'],
            ['sync', 'done'],
            ['test-build', 'done'],
            ['signing-kit', 'done'],
            ['release', 'active'],
        ]);
        expect(steps[4]?.summary).toContain('JDK 17.0.20');
        expect(steps[5]?.summary).toContain('upload.keystore');
    });

    it('keeps the release locked while the kit has no upload key', () => {
        const view = androidView();
        view.android!.workspace = {
            localPath: '/home/you/app',
            name: 'app',
            applicationId: 'com.example.app',
            lastSnapshotSha256: 'abc',
            lastSyncFileCount: 10,
            lastSyncBytes: 1000,
            lastBuildSucceeded: true,
            lastBuild: null,
            lastSource: null,
        };
        view.signingKit = androidKit(false);
        view.signingHealth = 'incomplete';

        const steps = deriveAndroidSteps(view, { runningStep: null });

        expect(steps.find((step) => step.id === 'signing-kit')?.status).toBe('active');
        expect(steps.find((step) => step.id === 'signing-kit')?.summary).toContain(
            'an upload keystore',
        );
        expect(steps.find((step) => step.id === 'release')?.status).toBe('pending');
    });

    it('reports a retained release in the headline', () => {
        const view = androidView();
        view.android!.workspace = {
            localPath: '/home/you/app',
            name: 'app',
            applicationId: 'com.example.app',
            lastSnapshotSha256: 'abc',
            lastSyncFileCount: 10,
            lastSyncBytes: 1000,
            lastBuildSucceeded: true,
            lastBuild: null,
            lastSource: null,
        };
        view.signingKit = androidKit();
        view.signingHealth = 'ready';
        view.android!.release = {
            applicationId: 'com.example.app',
            versionName: '3.2.0',
            versionCode: '12',
            keyAlias: 'upload',
            certificateSha256: 'ff',
            aab: { path: '/a.aab', bytes: 6_000_000, sha256: 'aa' },
            apk: { path: '/a.apk', bytes: 9_000_000, sha256: 'bb' },
            outputTail: [],
        };

        const steps = deriveJourney(view, { runningStep: null });

        expect(focusStep(steps)).toBeNull();
        expect(journeyHeadline(steps)).toBe('signed 3.2.0 (12)');
    });

    it('summarizes the coarse journey from the list row', () => {
        const steps = summarizeJourney(
            summary({
                id: 'pixel',
                platform: 'android',
                config: {
                    name: 'Android builder',
                    macosRelease: 'sequoia',
                    memoryGib: 6,
                    cpuCores: 4,
                    sshPort: 50924,
                    provider: 'android_toolchain',
                },
                state: 'running',
                workspaceName: 'app',
                signingKitName: 'Team kit',
                signingProvisioned: true,
            }),
            readyHost,
        );

        expect(steps).toHaveLength(9);
        expect(steps.map((step) => step.status)).toEqual([
            'done',
            'done',
            'done',
            'done',
            'done',
            'done',
            'active',
            'pending',
            'pending',
        ]);
    });
});
