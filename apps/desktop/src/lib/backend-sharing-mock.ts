// Browser-only fixtures for native Mac setup and owner-approved sharing.
import type { Backend, Unlisten } from './backend';
import type * as T from '../types/backend';

type Events = {
    emit(event: string, value: unknown): void;
    on<P>(event: string, handler: (value: P) => void): Unlisten;
};
const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

export function createSharingPreview(events: Events, query: URLSearchParams) {
    const supported = query.has('nativeMac');
    const configured = query.get('nativeMac') === 'ready';
    const config: T.NativeMacConfig = {
        project: configured
            ? {
                  path: '/Users/matt/Projects/example',
                  name: 'Example app',
                  repository: 'https://github.com/example/app.git',
                  bundleIdentifier: 'dev.example.app',
                  developmentTeam: 'TEAM123456',
                  scheme: 'App',
                  minXcodeVersion: '26.0',
                  minIosSdkVersion: '26.0',
              }
            : null,
        signing: configured
            ? {
                  identitySha1: 'a'.repeat(40),
                  identityName: 'Apple Distribution: Example',
                  profilePath:
                      '/Users/matt/Library/Application Support/buildbridge/profile.mobileprovision',
                  profile: {
                      uuid: 'profile-preview',
                      teamIdentifier: 'TEAM123456',
                      applicationIdentifier: 'TEAM123456.dev.example.app',
                      expiresAt: '2027-09-01T00:00:00Z',
                      expiresAtEpochSeconds: 1819756800,
                      sha256: 'b'.repeat(64),
                      certificateSha1s: ['a'.repeat(40)],
                  },
              }
            : null,
    };
    let busy = false;
    let cancelled = false;
    let paused = false;
    let login = false;
    let lastBuild: T.NativeMacBuildResult | null = null;
    const remote: T.SharingState = { protocol_version: 1, invitations: [], grants: [] };
    function status(): T.NativeMacStatus {
        const issues = !supported
            ? ['Native Xcode builds require a Mac.']
            : !config.project
              ? ['Approve a local project first.']
              : [];
        return structuredClone({
            supported,
            config,
            busy,
            launchAtLogin: login,
            lastBuild,
            toolchain: {
                architecture: 'aarch64',
                developerDirectory: '/Applications/Xcode.app/Contents/Developer',
                xcodeVersion: '26.5',
                iosSdk: '26.5',
                availableSdks: 'iPhoneOS 26.5',
                nodeVersion: 'v22.16.0',
                pnpmVersion: '10.11.0',
                cocoapodsVersion: '1.16.2',
                issues: [],
            },
            identities: [{ sha1: 'a'.repeat(40), name: 'Apple Distribution: Example' }],
            testReady: supported && !!config.project,
            archiveReady: supported && !!config.project && !!config.signing,
            issues,
            compatibilityWarnings: config.project?.minXcodeVersion
                ? []
                : [
                      'No minimum Xcode version is declared. Known checks do not guarantee that this commit will compile.',
                  ],
        });
    }
    function targets(): T.MachineReport[] {
        if (!supported) return [];
        return [
            {
                id: supported ? 'native-mac' : 'default',
                name: supported ? 'This Mac' : 'iOS builder',
                ready: supported ? status().archiveReady : true,
                project: supported ? (config.project?.name ?? null) : 'Example app',
                bundle_identifier: 'dev.example.app',
                repository: 'https://github.com/example/app.git',
                env_set: null,
                env_sets: ['Production', 'Staging'],
                platform: 'ios',
                executor: supported ? 'native_macos' : null,
                toolchain_version: '26.5',
                readiness_issues: supported ? status().issues : [],
                architecture: supported ? 'aarch64' : 'x86_64',
            },
        ];
    }
    const methods = {
        async nativeMacStatus(_forceRefresh = false) {
            await sleep(80);
            return status();
        },
        async approveNativeMacProject(input: T.NativeMacProjectInput) {
            config.project = {
                path: input.path,
                name: 'Example app',
                repository: 'https://github.com/example/app.git',
                bundleIdentifier: 'dev.example.app',
                developmentTeam: 'TEAM123456',
                scheme: 'App',
                minXcodeVersion: input.minXcodeVersion,
                minIosSdkVersion: input.minIosSdkVersion,
            };
            config.signing = null;
            events.emit('machine-changed', { machineId: 'native-mac' });
            return structuredClone(config);
        },
        async configureNativeMacSigning(input: T.NativeMacSigningInput) {
            config.signing = {
                identitySha1: input.identitySha1,
                identityName: 'Apple Distribution: Example',
                profilePath: input.profilePath,
                profile: {
                    uuid: 'profile-preview',
                    teamIdentifier: 'TEAM123456',
                    applicationIdentifier: 'TEAM123456.dev.example.app',
                    expiresAt: '2027-09-01T00:00:00Z',
                    expiresAtEpochSeconds: 1819756800,
                    sha256: 'b'.repeat(64),
                    certificateSha1s: [input.identitySha1],
                },
            };
            events.emit('machine-changed', { machineId: 'native-mac' });
            return structuredClone(config);
        },
        async setNativeMacLogin(enabled: boolean) {
            login = enabled;
        },
        async runNativeMacBuild(input: T.NativeMacBuildInput) {
            if (!/^(?:[a-f\d]{40}|[a-f\d]{64})$/i.test(input.commit))
                throw new Error('Enter a full Git commit hash.');
            if (!config.project || (input.outcome === 'archive' && !config.signing))
                throw new Error('Complete project and signing setup first.');
            busy = true;
            cancelled = false;
            try {
                for (const phase of [
                    'compatibility',
                    'source',
                    'dependencies',
                    'build',
                    'verify',
                ]) {
                    events.emit('native-mac-progress', {
                        machineId: 'native-mac',
                        phase,
                        label: `Preparing ${phase}`,
                        logLine: `${phase} on the approved source commit`,
                    });
                    await sleep(350);
                    if (cancelled) throw new Error('The native Mac build was stopped.');
                }
                lastBuild = {
                    outcome: input.outcome,
                    commit: input.commit,
                    repository: config.project.repository,
                    xcodeVersion: '26.5',
                    bundleIdentifier: config.project.bundleIdentifier,
                    envSet: input.envSet,
                    artifacts:
                        input.outcome === 'archive'
                            ? ['App-AppStore.ipa', 'App.xcarchive.zip'].map((name) => ({
                                  path: `/Users/matt/Library/Application Support/buildbridge/native-mac/builds/example/${name}`,
                                  bytes: 42_000_000,
                                  sha256: 'b'.repeat(64),
                              }))
                            : [],
                    outputTail: ['Build succeeded.'],
                };
                return structuredClone(lastBuild);
            } finally {
                busy = false;
                events.emit('machine-changed', { machineId: 'native-mac' });
            }
        },
        async cancelNativeMacBuild() {
            cancelled = true;
        },
        async revealNativeMacArtifacts() {},
        async onNativeMacProgress(handler: (value: T.NativeMacProgress) => void) {
            return events.on('native-mac-progress', handler);
        },
        async onRunnerActivity(handler: (value: T.RunOnceResult) => void) {
            return events.on('runner-activity', handler);
        },
        async getSharing() {
            return structuredClone({ paused, state: remote, targets: targets() });
        },
        async setSharingPaused(value: boolean) {
            paused = value;
        },
        async createSharingInvitation(input: T.ShareMachineInput) {
            const target = targets().find((target) => target.id === input.machineId);
            if (!target?.ready || paused)
                throw new Error('Prepare this destination and resume sharing first.');
            const invitation: T.SharingInvitation = {
                id: `invite-${remote.invitations.length + 1}`,
                code: 'BB-0123456789ABCDEF0123456789ABCDEF01234567',
                policy_id: 'preview-policy',
                machine_id: target.id,
                project: target.project,
                repository: target.repository!,
                bundle_identifier: target.bundle_identifier,
                kinds: ['apple_archive'],
                env_sets: [...input.envSets],
                expires_at: new Date(Date.now() + 600_000).toISOString(),
                access_hours: input.accessHours,
                status: 'pending',
            };
            remote.invitations.push({ ...invitation, code: null });
            if (query.has('sharingRequest'))
                remote.grants.push({
                    id: `grant-${remote.grants.length + 1}`,
                    invitation_id: invitation.id,
                    policy_id: invitation.policy_id,
                    machine_id: target.id,
                    project: target.project,
                    repository: target.repository!,
                    bundle_identifier: target.bundle_identifier,
                    kinds: ['apple_archive'],
                    env_sets: [...input.envSets],
                    requester_id: 'collaborator',
                    requester_name: 'Alex',
                    requester_email: 'alex@example.test',
                    status: 'pending',
                    expires_at: null,
                    created_at: new Date().toISOString(),
                });
            return structuredClone(invitation);
        },
        async approveSharingGrant(id: string) {
            const grant = remote.grants.find((grant) => grant.id === id);
            if (!grant) throw new Error('Request not found.');
            grant.status = 'approved';
            grant.expires_at = new Date(Date.now() + 86400_000).toISOString();
        },
        async revokeSharingGrant(id: string) {
            const grant = remote.grants.find((grant) => grant.id === id);
            if (grant) grant.status = 'revoked';
        },
    } satisfies Pick<
        Backend,
        | 'nativeMacStatus'
        | 'approveNativeMacProject'
        | 'configureNativeMacSigning'
        | 'setNativeMacLogin'
        | 'runNativeMacBuild'
        | 'cancelNativeMacBuild'
        | 'revealNativeMacArtifacts'
        | 'onNativeMacProgress'
        | 'onRunnerActivity'
        | 'getSharing'
        | 'setSharingPaused'
        | 'createSharingInvitation'
        | 'approveSharingGrant'
        | 'revokeSharingGrant'
    >;
    return methods;
}
