import { describe, expect, it } from 'vite-plus/test';

import type { HostPrerequisites, MacBuilderView, MachineSummary } from '../types/backend';
import {
    completedCount,
    deriveBuildSteps,
    deriveJourney,
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
    displayAccess: true,
    display: ':1',
    ready: true,
    issues: [],
};

function baseView(overrides: Partial<MacBuilderView> = {}): MacBuilderView {
    return {
        machineId: 'default',
        profile: {
            name: 'Local macOS builder',
            macosRelease: 'sequoia',
            memoryGib: 8,
            cpuCores: 4,
            sshPort: 50922,
        },
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
            bootUsb: null,
        },
        deviceRun: null,
        deviceRunError: null,
        ...overrides,
    };
}

function readyView(): MacBuilderView {
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
});

function provisionedView(): MacBuilderView {
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
            'after a complete kit is attached',
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
        expect(kitStep?.summary).toContain('no kit remains in the vault');
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
        };

        const steps = deriveBuildSteps(view, { runningStep: null });

        expect(steps.find((step) => step.id === 'signing-kit')?.status).toBe('done');
        expect(steps.find((step) => step.id === 'archive')?.status).toBe('active');
    });
});

describe('development-only kit', () => {
    function developmentOnlyView(): MacBuilderView {
        const view = provisionedView();
        view.signingHealth = 'ready';
        view.signingKit = {
            id: 'dev-kit',
            name: 'Dev kit',
            appStoreConnectConfigured: true,
            appStoreConnectKeyId: 'KEYID12345',
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
            'Locked: the kit holds only a development identity',
        );
        expect(byId('run-device').status).toBe('active');
    });

    it('names what an unfinished kit still lacks', () => {
        const view = developmentOnlyView();
        view.signingKit = { ...view.signingKit!, developmentCertificatePasswordStored: false };
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
    it('is fourteen steps: setup, build, then the optional device run in its own phase', () => {
        const steps = deriveJourney(baseView(), { runningStep: null });

        expect(steps).toHaveLength(14);
        expect(steps.slice(0, 7).every((step) => step.phase === 'setup')).toBe(true);
        expect(steps.slice(7, 13).every((step) => step.phase === 'build')).toBe(true);
        expect(steps[13]?.phase).toBe('device');
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
        ]);
        expect(groups[0]?.focus).toBeNull();
        expect(groups[1]?.focus?.id).toBe('approve');
    });
});

function summary(overrides: Partial<MachineSummary> = {}): MachineSummary {
    return {
        id: 'team-mac',
        config: {
            name: 'Team Mac',
            macosRelease: 'sequoia',
            memoryGib: 8,
            cpuCores: 4,
            sshPort: 50923,
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
        ...overrides,
    };
}

describe('summarizeJourney', () => {
    it('reads a fresh machine as ready to start', () => {
        const steps = summarizeJourney(summary(), readyHost);

        expect(steps).toHaveLength(14);
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
            }),
            readyHost,
        );

        expect(steps.every((step) => step.status === 'done')).toBe(true);
        expect(steps.find((step) => step.id === 'approve')?.summary).toBe('example-app');
        expect(journeyHeadline(steps)).toBe('signed IPA retained');
    });

    it('blocks the journey on a host that is not ready', () => {
        const steps = summarizeJourney(summary(), {
            ...readyHost,
            ready: false,
            issues: ['No KVM'],
        });

        expect(steps[0]?.status).toBe('failed');
        expect(steps.slice(1).every((step) => step.status === 'pending')).toBe(true);
        expect(journeyHeadline(steps)).toBe('check the host needs attention');
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
        };
        expect(journeyHeadline(deriveJourney(view, { runningStep: null }))).toBe(
            'signed 3.2.0 (15)',
        );
    });
});

describe('run on the device', () => {
    const at = (view: MacBuilderView, runningStep: string | null = null) =>
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
        expect(step.summary).toContain('udev rule');
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

    it('is the last coarse row, pending until a run was retained', () => {
        const fresh = summarizeJourney(summary(), readyHost).at(-1)!;
        expect(fresh.id).toBe('run-device');
        expect(fresh.optional).toBe(true);
        expect(fresh.status).toBe('pending');
        expect(fresh.summary).toBe('after signing is provisioned');

        const retained = summarizeJourney(summary({ deviceRunRetained: true }), readyHost).at(-1)!;
        expect(retained.status).toBe('done');
        expect(retained.summary).toBe('Ran on the iPhone');
    });
});
