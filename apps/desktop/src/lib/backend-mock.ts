// Development-only stand-in for the native layer. It reproduces the shapes and timing of the
// real commands closely enough to develop every screen in a browser: one machine that has
// completed the whole golden path and one that was just created. Long operations emit the
// same progress events the Rust side does.
//
// Never imported in production builds; see loadBackend() in ./backend.ts.

import type { Backend, DragDropEvent, Unlisten } from './backend';
import type * as T from '../types/backend';

type Handler<P> = (payload: P) => void;

class Emitter {
    private handlers = new Map<string, Set<Handler<unknown>>>();

    on<P>(event: string, handler: Handler<P>): Unlisten {
        const set = this.handlers.get(event) ?? new Set();
        set.add(handler as Handler<unknown>);
        this.handlers.set(event, set);
        return () => set.delete(handler as Handler<unknown>);
    }

    emit<P>(event: string, payload: P): void {
        for (const handler of this.handlers.get(event) ?? []) {
            handler(payload);
        }
    }
}

const sleep = (milliseconds: number) =>
    new Promise<void>((resolve) => setTimeout(resolve, milliseconds));

// `?device=ready` seeds the prepared machine with a phone attached, paired, and signed for, so
// the last rungs of the device step can be previewed without walking the ladder; `?usbRule=1`
// starts with the host rule installed.
const query =
    typeof window === 'undefined'
        ? new URLSearchParams()
        : new URLSearchParams(window.location.search);
const deviceReady = query.get('device') === 'ready';
// `?lockDrift` previews the archive blocked on a guest-refreshed Podfile.lock.
const lockDrift = query.has('lockDrift');
// `?usbReady` stops one rung short of `?device=ready`: the host and container are prepared
// and a phone is plugged in, so the attach action itself can be previewed.
const usbReady = query.has('usbReady');
let usbRuleInstalled = query.has('usbRule') || usbReady || deviceReady;

const hostUsbDevices: T.HostUsbDevice[] = [
    {
        bus: 1,
        port: '3',
        vendorId: '05ac',
        productId: '12a8',
        product: 'iPhone',
        serial: '00008030000A1B2C3D4E5F6A',
        manufacturer: 'Apple Inc.',
        deviceNode: '/dev/bus/usb/001/007',
        nodeReady: true,
        heldBy: null,
    },
    {
        bus: 1,
        port: '4.2',
        vendorId: '05ac',
        productId: '12ab',
        product: 'iPad',
        serial: '000081100000000000000A2C',
        manufacturer: 'Apple Inc.',
        deviceNode: '/dev/bus/usb/001/009',
        nodeReady: true,
        heldBy: null,
    },
];

const DEVICE_UDID = '00008030-000A1B2C3D4E5F6A';
const DEVELOPMENT_CERT_SHA256 = 'bbbb1111cccc2222dddd3333eeee4444ffff5555aaaa6666bbbb7777cccc8888';

function readyPhone(): T.GuestDevice {
    return {
        identifier: 'E3F1A2B4-5C6D-4E7F-8A9B-0C1D2E3F4A5B',
        udid: DEVICE_UDID,
        name: 'Matt’s iPhone',
        osVersion: '18.6',
        model: 'iPhone 15 Pro',
        developerMode: 'enabled',
        pairingState: 'paired',
        tunnelState: 'connected',
        transportType: 'wired',
        ready: true,
        issue: null,
    };
}

function developmentProfile(udid: string): T.ProvisioningProfileSummary {
    return {
        uuid: '22222222-3333-4444-5555-666666666666',
        teamIdentifier: 'TEAM123456',
        applicationIdentifier: 'TEAM123456.com.example.app',
        expiresAt: '2027-09-02T10:14:00Z',
        developerCertificateSha256: [DEVELOPMENT_CERT_SHA256],
        kind: 'development',
        provisionedDeviceUdids: [udid],
        getTaskAllow: true,
    };
}

function developmentIdentity(): T.ProvisionedIdentity {
    return {
        identityName: 'Apple Development: Example Developer (TEAM123456)',
        identitySha1: '2222333344445555666677778888999900001111',
        certificateSha256: DEVELOPMENT_CERT_SHA256,
        certificateExpiresAt: '2027-09-02T10:14:00Z',
    };
}

const hostReady: T.HostPrerequisites = {
    supportedHost: true,
    dockerCli: true,
    dockerDaemon: true,
    dockerVersion: 'Docker version 28.3.2, build 578ccf6',
    kvmAccess: true,
    tunAccess: true,
    displayAccess: true,
    display: ':1',
    ready: true,
    issues: [],
};

interface MockMachine {
    id: string;
    config: T.MachineConfig;
    createdAt: number;
    state: T.ContainerState;
    containerId: string | null;
    startedAt: string | null;
    username: string | null;
    publicKey: string | null;
    portOpen: boolean;
    reachable: boolean;
    pinned: boolean;
    fingerprint: string | null;
    authenticated: boolean;
    macosVersion: string | null;
    xcodeVersion: string | null;
    xcodeSelected: boolean;
    iosSimulatorRuntime: string | null;
    workspace: T.StoredAppleWorkspace | null;
    signing: T.SigningProvisioningResult | null;
    archive: T.AppleArchiveResult | null;
    archiveError: string | null;
    archiveEnvSet?: string | null;
    /** An Android machine's project and retained release; null on a macOS machine. */
    android: {
        workspace: T.StoredAndroidWorkspace | null;
        release: T.AndroidReleaseResult | null;
        releaseError: string | null;
        releaseEnvSet: string | null;
    } | null;
    busy: string | null;
    logs: string[];
    /** The container was created with its disk on the host, the control socket, and USB. */
    usbContainer: boolean;
    attached: T.AttachedUsbDevice | null;
    /** The phone on QEMU's command line, when the container was built with one. */
    phoneController: boolean;
    guestDevices: T.GuestDevice[];
    /** Listings since the phone was attached: the mock pairs on the first, enables Developer Mode on the second. */
    deviceRefreshes: number;
    deviceRun: T.AppleDeviceRunResult | null;
    deviceRunError: string | null;
    cancelRequested: boolean;
    /** The template this machine was cloned from; its first boot needs no console. */
    templateId: string | null;
}

function readyMachine(): MockMachine {
    return {
        id: 'default',
        config: {
            name: 'Local macOS builder',
            macosRelease: 'sequoia',
            memoryGib: 16,
            cpuCores: 8,
            sshPort: 50922,
            provider: 'docker_osx',
        },
        createdAt: 1_756_700_000,
        state: 'running',
        containerId: 'c9f4d1e2a7b3c9f4d1e2a7b3c9f4d1e2a7b3',
        startedAt: new Date(Date.now() - 3 * 3600 * 1000 - 12 * 60 * 1000).toISOString(),
        username: 'builder',
        publicKey:
            'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKeyBuildBridgeGuestAccess buildbridge-guest',
        portOpen: true,
        reachable: true,
        pinned: true,
        fingerprint: 'SHA256:Qm9vdFN0cmFwR3Vlc3RGaW5nZXJwcmludEV4YW1wbGU',
        authenticated: true,
        macosVersion: '26.6.2',
        xcodeVersion: '26.6',
        xcodeSelected: true,
        iosSimulatorRuntime: '26.5',
        workspace: {
            localPath: '/home/you/projects/example-app',
            name: 'com.example.app',
            iosWorkspace: 'ios/App/App.xcworkspace',
            scheme: 'App',
            developmentTeam: 'TEAM123456',
            bundleIdentifier: 'com.example.app',
            lastSnapshotSha256: '9999cccc8888dddd7777eeee6666ffff5555aaaa4444bbbb3333cccc2222dddd',
            lastSource: {
                kind: 'git',
                gitRef: 'release/1.4',
                commit: '3f9c2ab7d1e04c6b9a8f5e2d1c0b9a8f7e6d5c4b',
            },
            lastSyncFileCount: 1_842,
            lastSyncBytes: 48_213_770,
            lastBuildSucceeded: true,
            lastXcodeVersion: '26.6',
            lastNativeLockUpdated: lockDrift,
            lastBuildTarget: 'simulator',
            debugBundleIdentifier: null,
        },
        signing: {
            keychainPath: '/Users/builder/Library/Keychains/buildbridge-signing.keychain-db',
            distributionIdentity: {
                identityName: 'iPhone Distribution: Example Developer (TEAM123456)',
                identitySha1: '1111222233334444555566667777888899990000',
                certificateSha256:
                    'aaaa1111bbbb2222cccc3333dddd4444eeee5555ffff6666aaaa7777bbbb8888',
                certificateExpiresAt: '2027-09-02T10:14:00Z',
            },
            developmentTeam: 'TEAM123456',
            bundleIdentifier: 'com.example.app',
            profiles: [
                {
                    uuid: '11111111-2222-3333-4444-555555555555',
                    teamIdentifier: 'TEAM123456',
                    applicationIdentifier: 'TEAM123456.com.example.app',
                    expiresAt: '2027-09-02T10:14:00Z',
                    developerCertificateSha256: [
                        'aaaa1111bbbb2222cccc3333dddd4444eeee5555ffff6666aaaa7777bbbb8888',
                    ],
                    kind: 'app_store',
                    provisionedDeviceUdids: [],
                    getTaskAllow: false,
                },
                ...(deviceReady ? [developmentProfile(DEVICE_UDID)] : []),
            ],
            developmentIdentity: deviceReady ? developmentIdentity() : null,
        },
        archive: {
            scheme: 'App',
            configuration: 'Release',
            exportMethod: 'app-store-connect',
            bundleIdentifier: 'com.example.app',
            developmentTeam: 'TEAM123456',
            marketingVersion: '3.2.0',
            buildNumber: '15',
            provisioningProfileUuid: '11111111-2222-3333-4444-555555555555',
            ipa: {
                path: '/home/you/.local/share/dev.buildbridge.desktop/macos-builder/artifacts/archive-1756800000000-4242/App-AppStore.ipa',
                bytes: 7_096_076,
                sha256: 'AAAA1111BBBB2222CCCC3333DDDD4444EEEE5555FFFF6666AAAA7777BBBB8888',
            },
            archive: {
                path: '/home/you/.local/share/dev.buildbridge.desktop/macos-builder/artifacts/archive-1756800000000-4242/App.xcarchive.zip',
                bytes: 28_268_787,
                sha256: '9999CCCC8888DDDD7777EEEE6666FFFF5555AAAA4444BBBB3333CCCC2222DDDD',
            },
            outputTail: ['** ARCHIVE SUCCEEDED **', '** EXPORT SUCCEEDED **'],
        },
        archiveError: null,
        android: null,
        busy: null,
        usbContainer: deviceReady || usbReady,
        attached: deviceReady ? { bus: 1, port: '3', enumerated: true, issue: null } : null,
        phoneController: true,
        guestDevices: deviceReady ? [readyPhone()] : [],
        deviceRefreshes: deviceReady ? 2 : 0,
        deviceRun: null,
        deviceRunError: null,
        cancelRequested: false,
        templateId: null,
        logs: [
            'Docker-OSX: booting OpenCore with generated serial C02X1234ABCD',
            'qemu-system-x86_64: -display gtk,zoom-to-fit=on',
            'Forwarding host port 50922 to guest port 22',
            'Guest SSH available',
        ],
    };
}

function freshMachine(): MockMachine {
    return {
        id: 'team-mac',
        config: {
            name: 'Team Mac',
            macosRelease: 'sequoia',
            memoryGib: 8,
            cpuCores: 4,
            sshPort: 50923,
            provider: 'docker_osx',
        },
        createdAt: Math.floor(Date.now() / 1000) - 600,
        state: 'missing',
        containerId: null,
        startedAt: null,
        username: null,
        publicKey: null,
        portOpen: false,
        reachable: false,
        pinned: false,
        fingerprint: null,
        authenticated: false,
        macosVersion: null,
        xcodeVersion: null,
        xcodeSelected: false,
        iosSimulatorRuntime: null,
        workspace: null,
        signing: null,
        archive: null,
        archiveError: null,
        android: null,
        busy: null,
        // A new machine's container is created with USB access from the start.
        usbContainer: true,
        attached: null,
        phoneController: true,
        guestDevices: [],
        deviceRefreshes: 0,
        deviceRun: null,
        deviceRunError: null,
        cancelRequested: false,
        templateId: null,
        logs: [],
    };
}

/** An Android machine with a project approved, synchronized, built and released. */
function androidMachine(): MockMachine {
    return {
        ...freshMachine(),
        id: 'pixel-builder',
        config: {
            name: 'Android builder',
            macosRelease: 'sequoia',
            memoryGib: 6,
            cpuCores: 4,
            sshPort: 50924,
            provider: 'android_toolchain',
        },
        createdAt: 1_756_800_000,
        state: 'running',
        containerId: 'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6',
        startedAt: new Date(Date.now() - 40 * 60 * 1000).toISOString(),
        usbContainer: false,
        phoneController: false,
        android: {
            workspace: {
                localPath: '/home/you/projects/example-app',
                name: 'com.example.app',
                applicationId: 'com.example.app',
                lastSnapshotSha256:
                    '6c22009fda0b9467709394b5ae1c442af1da4544d63b5ac01b8778c03d7fcd7b',
                lastSyncFileCount: 1_397,
                lastSyncBytes: 28_278_463,
                lastBuildSucceeded: true,
                lastBuild: {
                    applicationId: 'com.example.app.debug',
                    versionName: '3.2.0',
                    versionCode: '12',
                    toolchain: {
                        jdkVersion: 'openjdk version "21.0.12.1" 2026-08-18 LTS',
                        buildToolsVersion: '35.0.0',
                    },
                    apk: {
                        path: '/home/you/.local/share/dev.buildbridge.desktop/machines/pixel-builder/artifacts/debug-1756890000000-4242/app-debug.apk',
                        bytes: 33_410_772,
                        sha256: '5555aaaa6666bbbb7777cccc8888dddd9999eeee0000ffff1111222233334444',
                    },
                    outputTail: ['BUILD SUCCESSFUL in 2m 41s'],
                },
                lastSource: null,
            },
            release: {
                applicationId: 'com.example.app',
                versionName: '3.2.0',
                versionCode: '12',
                keyAlias: 'upload',
                certificateSha256:
                    '2f7c1e9a4b3d5c6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6',
                aab: {
                    path: '/home/you/.local/share/dev.buildbridge.desktop/machines/pixel-builder/artifacts/release-1756900000000-4242/app-release.aab',
                    bytes: 6_412_090,
                    sha256: '1111aaaa2222bbbb3333cccc4444dddd5555eeee6666ffff7777000088881111',
                },
                apk: {
                    path: '/home/you/.local/share/dev.buildbridge.desktop/machines/pixel-builder/artifacts/release-1756900000000-4242/app-release.apk',
                    bytes: 9_803_211,
                    sha256: '9999aaaa8888bbbb7777cccc6666dddd5555eeee4444ffff3333000022221111',
                },
                outputTail: ['BUILD SUCCESSFUL in 3m 12s', 'Verifies'],
            },
            releaseError: null,
            releaseEnvSet: null,
        },
        logs: [],
    };
}

function slug(name: string): string {
    return (
        name
            .toLowerCase()
            .replace(/[^a-z0-9]+/g, '-')
            .replace(/^-|-$/g, '')
            .slice(0, 32) || 'machine'
    );
}

export function createMockBackend(): Backend {
    const emitter = new Emitter();
    // A few optimizer items so the section can be previewed; the real catalogue lives in Rust.
    const appliedOptimizations = new Set<string>(['default:reduce-motion']);
    const optimizationCatalogue: Omit<T.GuestOptimization, 'applied'>[] = [
        {
            id: 'disable-spotlight',
            title: 'Disable Spotlight indexing',
            summary:
                'Stops the indexer that otherwise churns through every synchronized project and every Xcode install. The single biggest win for a virtual machine.',
            tier: 'recommended',
            warning:
                'Spotlight stops finding apps and files; `sudo mdutil -i on -a` turns it back on.',
            needsAdmin: true,
        },
        {
            id: 'reduce-motion',
            title: 'Reduce motion and transparency',
            summary:
                'Turns off the animations and blur the console window otherwise has to render through QEMU.',
            tier: 'recommended',
            warning: null,
            needsAdmin: false,
        },
        {
            id: 'disable-updates',
            title: 'Disable software updates',
            summary:
                'Stops macOS downloading multi-gigabyte updates in the background, which is what makes a virtual disk grow out of proportion.',
            tier: 'at_your_own_risk',
            warning:
                'At your own risk: the guest stops receiving security updates. Update it deliberately instead.',
            needsAdmin: true,
        },
        {
            id: 'disable-passwords',
            title: 'Disable passwords globally',
            summary:
                'Rewrites every PAM policy so no password is ever required: everyone is root, sudo never asks, and SSH password login accepts an empty password.',
            tier: 'extremely_insecure',
            warning:
                'These macOS optimizations should only be used in CI/CD, behind a VPN, and with no external connectivity. This is not a warning, it is absolutely essential, or anyone can just SSH into the remote mac.',
            needsAdmin: true,
        },
    ];
    const optimizationsFor = (machineId: string): T.GuestOptimizationsView => {
        const machine = machines.find((entry) => entry.id === machineId);
        const available = machine?.state === 'running';
        return {
            available,
            reason: available
                ? null
                : 'Start the machine, pin its identity and authorize the access key first.',
            items: optimizationCatalogue.map((item) => ({
                ...item,
                applied: available ? appliedOptimizations.has(`${machineId}:${item.id}`) : null,
            })),
        };
    };
    const machines: MockMachine[] = [readyMachine(), freshMachine(), androidMachine()];
    // `?unpaired=1` previews the desktop with no control plane at all.
    let paired = !(
        typeof location !== 'undefined' && new URLSearchParams(location.search).has('unpaired')
    );
    const kits: T.SigningKitSummary[] = [
        {
            id: 'example-team',
            name: 'Example team',
            appStoreConnectConfigured: true,
            appStoreConnectKeyId: 'KEYID12345',
            signingCertificateConfigured: true,
            signingCertificateName: 'iphone dist cert.p12',
            signingCertificatePasswordStored: true,
            provisioningProfileNames: ['11111111-2222-3333-4444-555555555555.mobileprovision'],
            guestKeychainConfigured: true,
            createdAtEpochSeconds: 1_756_700_000,
            attachedMachines: ['Local macOS builder'],
            developmentCertificateConfigured: deviceReady,
            developmentCertificateName: deviceReady ? 'development.p12' : null,
            developmentCertificatePasswordStored: deviceReady,
            androidKeystoreConfigured: true,
            androidKeystoreName: 'upload.keystore',
            androidKeyAlias: 'upload',
            androidKeystorePasswordStored: true,
            androidKeyPasswordStored: false,
        },
        {
            id: 'client-app',
            name: 'Second team',
            appStoreConnectConfigured: false,
            appStoreConnectKeyId: null,
            signingCertificateConfigured: true,
            signingCertificateName: 'client-dist.p12',
            signingCertificatePasswordStored: true,
            provisioningProfileNames: [],
            guestKeychainConfigured: false,
            createdAtEpochSeconds: 1_756_900_000,
            attachedMachines: [],
            developmentCertificateConfigured: false,
            developmentCertificateName: null,
            developmentCertificatePasswordStored: false,
            androidKeystoreConfigured: false,
            androidKeystoreName: null,
            androidKeyAlias: null,
            androidKeystorePasswordStored: false,
            androidKeyPasswordStored: false,
        },
    ];
    const attachments: Record<string, string | null> = {
        default: 'example-team',
        'team-mac': null,
        'pixel-builder': 'example-team',
    };
    const templates: T.MachineTemplateSummary[] = [
        {
            id: 'xcode-26-ready',
            name: 'Xcode 26 ready',
            createdAtEpochSeconds: 1_756_800_000,
            sourceMachineName: 'Local macOS builder',
            macosVersion: '26.6.2',
            xcodeVersion: '26.6',
            sizeBytes: 21_400_000_000,
            machineNames: [],
            ready: true,
            provider: 'docker_osx',
        },
    ];
    const templateFor = (machine: MockMachine) =>
        templates.find((template) => template.id === machine.templateId) ?? null;
    const envSets: T.EnvSetSummary[] = [
        {
            id: 'production',
            name: 'production',
            variables: [{ key: 'VITE_API_URL', value: 'https://api.example.com/v1' }],
            secretKeys: ['VITE_SENTRY_DSN'],
            createdAtEpochSeconds: 1_756_700_000,
            attachedMachines: ['Local macOS builder'],
        },
    ];
    // Secret values sit beside the summaries, as the vault holds them and the summary omits them.
    const envSecretValues: Record<string, Record<string, string>> = {
        production: { VITE_SENTRY_DSN: 'https://examplePublicKey@o0.ingest.sentry.io/0' },
    };
    const envAttachments: Record<string, string | null> = {
        default: 'production',
        'team-mac': null,
        'pixel-builder': null,
    };
    const envSetFor = (machineId: string): T.EnvSetSummary | null =>
        envSets.find((set) => set.id === envAttachments[machineId]) ?? null;
    const refreshEnvAttachments = (): void => {
        for (const set of envSets) {
            set.attachedMachines = machines
                .filter((machine) => envAttachments[machine.id] === set.id)
                .map((machine) => machine.config.name);
        }
    };
    // `?vaultCleared=1` reproduces the state an operating-system keyring reset leaves behind.
    const vaultCleared =
        typeof location !== 'undefined' && new URLSearchParams(location.search).has('vaultCleared');
    const storedKits = () => (vaultCleared ? [] : kits);
    const kitComplete = (kit: T.SigningKitSummary, machine: MockMachine) =>
        machine.android
            ? kit.androidKeystoreConfigured && kit.androidKeystorePasswordStored
            : kit.signingCertificateConfigured &&
              kit.provisioningProfileNames.length > 0 &&
              kit.guestKeychainConfigured;

    const find = (machineId: string): MockMachine => {
        const machine = machines.find((entry) => entry.id === machineId);
        if (!machine) {
            throw new Error('This machine is no longer registered.');
        }
        return machine;
    };

    const changed = (machineId: string | null) => {
        emitter.emit<T.MachineChangedEvent>('machine-changed', { machineId });
    };

    const busy = async <R>(
        machine: MockMachine,
        label: string,
        work: () => Promise<R>,
    ): Promise<R> => {
        if (machine.busy) {
            throw new Error(
                `${machine.busy} is already running on this machine. Wait for it to finish.`,
            );
        }
        machine.busy = label;
        changed(machine.id);
        try {
            return await work();
        } finally {
            machine.busy = null;
            changed(machine.id);
        }
    };

    const view = (machine: MockMachine): T.MachineView => {
        const running = machine.state === 'running';
        const trust: T.GuestTrustState =
            !running || !machine.reachable
                ? 'unavailable'
                : machine.pinned
                  ? 'trusted'
                  : 'untrusted';
        return {
            machineId: machine.id,
            template: machine.templateId
                ? { id: machine.templateId, name: templateFor(machine)?.name ?? machine.templateId }
                : null,
            profile: { ...machine.config },
            displayUrl:
                machine.config.provider === 'dockur_macos'
                    ? `http://127.0.0.1:${machine.config.sshPort + 1}/`
                    : null,
            busyOperation: machine.busy,
            runtime: {
                prerequisites: hostReady,
                state: machine.state,
                containerId: machine.containerId,
                startedAt: machine.startedAt,
            },
            signingKit: storedKits().find((kit) => kit.id === attachments[machine.id]) ?? null,
            envSet: envSetFor(machine.id),
            signingHealth: (() => {
                const attached = storedKits().find((kit) => kit.id === attachments[machine.id]);
                if (!attached) {
                    return machine.signing ? 'kit_missing' : 'unconfigured';
                }
                return kitComplete(attached, machine) ? 'ready' : 'incomplete';
            })(),
            vaultIssue: null,
            guest: {
                username: machine.username,
                publicKey: machine.publicKey,
                ssh: {
                    portOpen: running && machine.portOpen,
                    reachable: running && machine.reachable,
                    trust,
                    fingerprint: running && machine.reachable ? machine.fingerprint : null,
                    pinnedFingerprint: machine.pinned ? machine.fingerprint : null,
                    issue: !running
                        ? 'Start the macOS machine to probe guest SSH.'
                        : !machine.portOpen
                          ? 'Guest SSH is not reachable yet. Finish macOS setup and enable Remote Login.'
                          : null,
                },
                diagnostics: {
                    authenticated: trust === 'trusted' && machine.authenticated,
                    macosVersion:
                        trust === 'trusted' && machine.authenticated ? machine.macosVersion : null,
                    xcodeVersion:
                        trust === 'trusted' && machine.authenticated ? machine.xcodeVersion : null,
                    xcodePath:
                        trust === 'trusted' && machine.xcodeVersion
                            ? `/Users/${machine.username}/Applications/Xcode.app`
                            : null,
                    xcodeSelected:
                        trust === 'trusted' && machine.authenticated && machine.xcodeSelected,
                    iosSimulatorRuntime:
                        trust === 'trusted' && machine.authenticated && machine.xcodeSelected
                            ? machine.iosSimulatorRuntime
                            : null,
                    issue:
                        trust === 'trusted' && machine.username && !machine.authenticated
                            ? 'SSH authentication failed; add the BuildBridge public key to the guest user'
                            : null,
                },
                devices: running ? machine.guestDevices.map((device) => ({ ...device })) : [],
            },
            appleWorkspace: machine.workspace ? { ...machine.workspace } : null,
            signing: machine.signing ? { ...machine.signing } : null,
            archive: machine.archive ? { ...machine.archive } : null,
            archiveEnvSet: machine.archiveEnvSet ?? null,
            archiveError: machine.archiveError,
            logs: [...machine.logs],
            usb: {
                host: {
                    supported: true,
                    rule: usbRuleInstalled ? 'installed' : 'missing',
                    rulePath: '/etc/udev/rules.d/40-buildbridge-iphone.rules',
                    usbmuxdActive: !usbRuleInstalled,
                    plugdevGid: 46,
                    devices: hostUsbDevices.map((device) => ({
                        ...device,
                        nodeReady: usbRuleInstalled,
                        heldBy: usbRuleInstalled
                            ? machine.attached?.port === device.port
                                ? 'this_machine'
                                : null
                            : 'usbmuxd',
                    })),
                    issues: usbRuleInstalled
                        ? []
                        : [
                              'Install the BuildBridge iPhone rule so usbmuxd releases phones to the machine.',
                          ],
                },
                diskOnHost: machine.usbContainer,
                containerReady: machine.usbContainer && machine.state !== 'missing',
                containerIssue: machine.usbContainer
                    ? machine.state === 'missing'
                        ? 'The container gets USB access and a host-side disk when the machine is next started.'
                        : null
                    : 'Enable USB on this machine to recreate its container with USB access; the macOS disk is kept.',
                qmpReachable: machine.usbContainer && running,
                attached: running && machine.attached ? { ...machine.attached } : null,
                phoneController: machine.usbContainer && machine.phoneController,
            },
            deviceRun: machine.deviceRun ? { ...machine.deviceRun } : null,
            deviceRunError: machine.deviceRunError,
            android: machine.android
                ? {
                      workspace: machine.android.workspace
                          ? { ...machine.android.workspace }
                          : null,
                      release: machine.android.release ? { ...machine.android.release } : null,
                      releaseEnvSet: machine.android.releaseEnvSet,
                      releaseError: machine.android.releaseError,
                  }
                : null,
        };
    };

    const list = (): T.MachineListView => ({
        host: hostReady,
        machines: machines.map((machine) => ({
            id: machine.id,
            platform: machine.android ? 'android' : 'ios',
            templateName: templateFor(machine)?.name ?? null,
            config: { ...machine.config },
            createdAtEpochSeconds: machine.createdAt,
            state: machine.state,
            containerId: machine.containerId,
            busyOperation: machine.busy,
            guestConfigured: machine.username !== null,
            trustPinned: machine.pinned,
            workspaceName: machine.workspace?.name ?? machine.android?.workspace?.name ?? null,
            envSetName: envSetFor(machine.id)?.name ?? null,
            signingKitName:
                storedKits().find((kit) => kit.id === attachments[machine.id])?.name ?? null,
            signingProvisioned: machine.android
                ? (storedKits().find((kit) => kit.id === attachments[machine.id])
                      ?.androidKeystoreConfigured ?? false)
                : machine.signing !== null,
            signingIdentity:
                machine.signing?.distributionIdentity?.identityName ??
                machine.signing?.developmentIdentity?.identityName ??
                null,
            archiveRetained: machine.archive !== null || machine.android?.release != null,
            usbReady: machine.usbContainer && machine.state !== 'missing',
            deviceRunRetained: machine.deviceRun !== null,
        })),
    });

    const projectPhases: T.AppleProjectPhase[] = [
        'preparing_tools',
        'installing_dependencies',
        'building_web_assets',
        'syncing_ios',
        'resolving_pods',
        'building',
        'completed',
    ];

    return {
        async getRunnerStatus() {
            await sleep(150);
            return {
                paired,
                credentialsMissing: false,
                serverUrl: paired ? 'https://buildbridge.test' : null,
                runnerId: paired ? '9c1f2a3b-4d5e-4f60-8a7b-1c2d3e4f5a6b' : null,
                runnerName: paired ? 'linux-builder' : null,
                platform: 'linux',
                architecture: 'x86_64',
                version: '0.1.0',
            };
        },
        async pairRunner() {
            await sleep(600);
            paired = true;
            return this.getRunnerStatus();
        },
        async unpairRunner() {
            paired = false;
        },
        async getRealtimeConfiguration() {
            throw new Error('Realtime is unavailable in the browser preview.');
        },
        async authorizeRealtime() {
            throw new Error('Realtime is unavailable in the browser preview.');
        },
        async heartbeatRunner() {
            return { queuedBuilds: 0 };
        },
        async runOnce() {
            await sleep(300);
            return { state: 'idle', buildId: null, message: 'No queued builds.' };
        },

        async listMachines() {
            await sleep(120);
            return list();
        },
        async createMachine(profile, templateId) {
            await sleep(200);
            if (machines.some((machine) => machine.config.sshPort === profile.sshPort)) {
                throw new Error(
                    `SSH port ${profile.sshPort} is already used by another machine. Choose a different port.`,
                );
            }
            const machine = freshMachine();
            machine.id = slug(profile.name);
            machine.config = { ...profile };
            machine.createdAt = Math.floor(Date.now() / 1000);
            if (templateId) {
                const template = templates.find((entry) => entry.id === templateId);
                if (!template) {
                    throw new Error('That template is no longer stored.');
                }
                machine.templateId = templateId;
                template.machineNames.push(machine.config.name);
            }
            machines.push(machine);
            changed(null);
            return list();
        },
        async listMachineTemplates() {
            await sleep(120);
            return templates.map((template) => ({
                ...template,
                machineNames: [...template.machineNames],
            }));
        },
        async saveMachineTemplate(machineId, name) {
            const machine = find(machineId);
            return busy(machine, 'Saving the machine as a template', async () => {
                const phases: Array<[T.TemplateSavePhase, string, number | null]> = [
                    [
                        'shutting_down',
                        'Asking macOS to shut down so the disk is copied at rest',
                        null,
                    ],
                    ['checking_space', 'Checking free space for the compressed copy', null],
                    ['compressing_disk', 'Compressing the macOS disk into the template', 0],
                    ['compressing_disk', 'Compressing the macOS disk into the template', 20],
                    ['compressing_disk', 'Compressing the macOS disk into the template', 45],
                    ['compressing_disk', 'Compressing the macOS disk into the template', 70],
                    ['compressing_disk', 'Compressing the macOS disk into the template', 90],
                    ['copying_files', 'Copying the NVRAM and install media', 100],
                    ['completed', 'Template saved', 100],
                ];
                // Slow enough to leave the machine and watch it from the Templates page.
                let elapsed = 0;
                for (const [phase, detail, percent] of phases) {
                    emitter.emit<T.MachineEvent<T.TemplateSaveProgress>>(
                        'machine-template-progress',
                        { machineId, phase, elapsedSeconds: elapsed, detail, percent },
                    );
                    await sleep(900);
                    elapsed += 40;
                }
                machine.state = 'exited';
                machine.startedAt = null;
                const template: T.MachineTemplateSummary = {
                    id: slug(name),
                    name,
                    createdAtEpochSeconds: Math.floor(Date.now() / 1000),
                    sourceMachineName: machine.config.name,
                    provider: machine.config.provider,
                    macosVersion: machine.macosVersion,
                    xcodeVersion: machine.xcodeVersion,
                    sizeBytes: 20_800_000_000,
                    machineNames: [],
                    ready: true,
                };
                templates.unshift(template);
                return { ...template };
            });
        },
        async deleteMachineTemplate(templateId) {
            await sleep(200);
            const template = templates.find((entry) => entry.id === templateId);
            if (template?.machineNames.length) {
                throw new Error(
                    `Machines still read through this template: ${template.machineNames.join(', ')}. Delete those machines first.`,
                );
            }
            const index = templates.findIndex((entry) => entry.id === templateId);
            if (index >= 0) {
                templates.splice(index, 1);
            }
            return templates.map((entry) => ({ ...entry }));
        },
        async adoptTemplateGuest(machineId) {
            const machine = find(machineId);
            return busy(machine, 'Adopting the template', async () => {
                await sleep(900);
                machine.pinned = true;
                machine.username = 'builder';
                machine.publicKey =
                    'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKeyBuildBridgeGuestAccess buildbridge-guest';
                machine.authenticated = true;
                return view(machine);
            });
        },
        async deleteMachine(machineId) {
            const machine = find(machineId);
            if (machine.state === 'running') {
                throw new Error('Stop the machine before deleting it.');
            }
            machines.splice(machines.indexOf(machine), 1);
            changed(null);
            return list();
        },
        async discardMachineContainer(machineId) {
            const machine = find(machineId);
            if (machine.state === 'running') {
                throw new Error('Stop the machine before discarding its container.');
            }
            Object.assign(machine, {
                state: 'missing',
                containerId: null,
                startedAt: null,
                pinned: false,
                signing: null,
                logs: [],
            });
            return view(machine);
        },
        async getMachine(machineId) {
            await sleep(180);
            return view(find(machineId));
        },
        async configureMachine(machineId, profile) {
            const machine = find(machineId);
            machine.config = { ...profile };
            return view(machine);
        },
        async launchMachine(machineId) {
            const machine = find(machineId);
            return busy(machine, 'Starting the machine', async () => {
                const phases: Array<[T.LaunchPhase, string]> = [
                    ['preparing', 'Checking the host and Docker'],
                    [
                        'pulling_image',
                        'Pulling the Docker-OSX image; the first pull downloads several gigabytes',
                    ],
                    ['generating_identity', 'Generating a stable machine identity'],
                    ['creating_container', 'Creating the managed container'],
                    ['starting', 'Starting the macOS machine'],
                    ['completed', 'The macOS machine is running'],
                ];
                let elapsed = 0;
                for (const [phase, detail] of phases) {
                    emitter.emit<T.MachineEvent<T.LaunchProgress>>('machine-launch-progress', {
                        machineId,
                        phase,
                        elapsedSeconds: elapsed,
                        detail,
                    });
                    await sleep(phase === 'pulling_image' ? 1500 : 500);
                    elapsed += 2;
                }
                machine.state = 'running';
                machine.containerId ??= `${machine.id}-${Date.now().toString(16)}`;
                machine.startedAt = new Date().toISOString();
                if (machine.templateId) {
                    // A clone boots the template's macOS: reachable at once, its key still to come.
                    const template = templateFor(machine);
                    machine.portOpen = true;
                    machine.reachable = true;
                    machine.fingerprint = 'SHA256:Qm9vdFN0cmFwR3Vlc3RGaW5nZXJwcmludEV4YW1wbGU';
                    machine.macosVersion = template?.macosVersion ?? '26.6.2';
                    machine.xcodeVersion = template?.xcodeVersion ?? '26.6';
                    machine.xcodeSelected = true;
                }
                machine.logs = [
                    'Docker-OSX: booting OpenCore with generated serial',
                    'qemu-system-x86_64: -display gtk,zoom-to-fit=on',
                ];
                return view(machine);
            });
        },
        async stopMachine(machineId) {
            const machine = find(machineId);
            return busy(machine, 'Stopping the machine', async () => {
                await sleep(800);
                machine.state = 'exited';
                machine.startedAt = null;
                return view(machine);
            });
        },
        async configureGuestAccess(machineId, username) {
            const machine = find(machineId);
            machine.username = username;
            machine.publicKey ??=
                'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKeyBuildBridgeGuestAccess buildbridge-guest';
            return view(machine);
        },
        async authorizeGuestKey(machineId, username, password) {
            const machine = find(machineId);
            machine.username = username;
            machine.publicKey ??=
                'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKeyBuildBridgeGuestAccess buildbridge-guest';
            await sleep(900);
            if (!machine.pinned) {
                throw new Error(
                    'Pin the guest identity first, so the password only ever goes to the machine you verified.',
                );
            }
            // The preview accepts any password except the one that demonstrates a rejection.
            if (password === 'wrong') {
                throw new Error(
                    `macOS guest bridge failed: macOS did not accept the password for ${username}. Check the short username and the local macOS login password, or add the key from the guest Terminal instead.`,
                );
            }
            machine.authenticated = true;
            return view(machine);
        },
        async trustGuest(machineId) {
            const machine = find(machineId);
            machine.pinned = true;
            return view(machine);
        },
        async forgetGuestTrust(machineId) {
            const machine = find(machineId);
            machine.pinned = false;
            return view(machine);
        },
        async importXcode(machineId, path) {
            const machine = find(machineId);
            return busy(machine, 'Importing Xcode', async () => {
                const total = 8_500_000_000;
                for (let step = 0; step <= 6; step += 1) {
                    emitter.emit<T.MachineEvent<T.XcodeImportProgress>>('machine-xcode-progress', {
                        machineId,
                        phase:
                            step < 5
                                ? 'transferring'
                                : step === 5
                                  ? 'expanding'
                                  : 'awaiting_activation',
                        transferredBytes: Math.min(total, (total / 5) * step),
                        totalBytes: total,
                        elapsedSeconds: step * 40,
                        detail:
                            step < 5
                                ? `Transferring ${path.split('/').pop()}`
                                : step === 5
                                  ? 'Expanding the signed archive in the guest'
                                  : 'Xcode is installed; activate it to finish',
                    });
                    await sleep(500);
                }
                machine.xcodeVersion = '26.6';
                return {
                    view: view(machine),
                    installedPath: `/Users/${machine.username}/Applications/Xcode.app`,
                    activationCommands: [
                        `sudo xcode-select --switch /Users/${machine.username}/Applications/Xcode.app`,
                        'sudo xcodebuild -license accept',
                        'sudo xcodebuild -runFirstLaunch',
                        'xcodebuild -version',
                    ],
                };
            });
        },
        async activateXcode(machineId, password) {
            const machine = find(machineId);
            return busy(machine, 'Activating Xcode', async () => {
                if (password !== null) {
                    const stages = [
                        'Authorizing with sudo over the pinned bridge',
                        'Selecting the developer directory',
                        "Accepting Apple's license",
                        "Running Xcode's first-launch tasks and installing required components. This can take several minutes.",
                    ];
                    for (const [index, detail] of stages.entries()) {
                        emitter.emit<T.MachineEvent<T.XcodeImportProgress>>(
                            'machine-xcode-progress',
                            {
                                machineId,
                                phase: 'activating',
                                transferredBytes: 0,
                                totalBytes: 0,
                                elapsedSeconds: index,
                                detail,
                            },
                        );
                        await sleep(700);
                        // The preview rejects one password so the failure path can be seen.
                        if (index === 0 && password === 'wrong') {
                            throw new Error(
                                `macOS guest bridge failed: macOS did not accept the password for ${machine.username}; nothing was changed. Check the local macOS login password, or leave it blank to type it in the guest Terminal.`,
                            );
                        }
                    }
                    machine.xcodeSelected = true;
                    return view(machine);
                }
                for (let second = 0; second < 4; second += 1) {
                    emitter.emit<T.MachineEvent<T.XcodeImportProgress>>('machine-xcode-progress', {
                        machineId,
                        phase: 'awaiting_authorization',
                        transferredBytes: 0,
                        totalBytes: 0,
                        elapsedSeconds: second,
                        detail: 'Waiting for the administrator password in the macOS Terminal',
                    });
                    await sleep(700);
                }
                machine.xcodeSelected = true;
                return view(machine);
            });
        },
        async provisionSigning(machineId) {
            const machine = find(machineId);
            return busy(machine, 'Provisioning signing', async () => {
                const kit = storedKits().find((entry) => entry.id === attachments[machineId]);
                const creating =
                    kit?.appStoreConnectConfigured === true &&
                    !(
                        kit.signingCertificateConfigured &&
                        kit.signingCertificatePasswordStored &&
                        kit.provisioningProfileNames.length > 0
                    );
                const phases: T.SigningProvisioningPhase[] = [
                    ...(creating ? (['creating_certificate', 'creating_profile'] as const) : []),
                    'preparing',
                    'transferring',
                    'importing_certificate',
                    'inspecting_profiles',
                    'installing_profiles',
                    'verifying',
                    'completed',
                ];
                for (const [index, phase] of phases.entries()) {
                    emitter.emit<T.MachineEvent<T.SigningProvisioningProgress>>(
                        'machine-signing-progress',
                        {
                            machineId,
                            phase,
                            completedBytes: index,
                            totalBytes: phases.length - 1,
                            elapsedSeconds: index * 3,
                            detail: phase.replace(/_/g, ' '),
                        },
                    );
                    await sleep(450);
                }
                if (kit && creating) {
                    kit.signingCertificateConfigured = true;
                    kit.signingCertificateName ??= 'distribution.p12';
                    kit.signingCertificatePasswordStored = true;
                    if (kit.provisioningProfileNames.length === 0) {
                        kit.provisioningProfileNames.push(
                            '22222222-3333-4444-5555-666666666666.mobileprovision',
                        );
                    }
                }
                machine.signing = readyMachine().signing;
                return view(machine);
            });
        },
        async clearGuestSigning(machineId) {
            const machine = find(machineId);
            machine.signing = null;
            return view(machine);
        },
        async approveWorkspace(machineId, path) {
            const machine = find(machineId);
            await sleep(250);
            machine.workspace = {
                ...(readyMachine().workspace as T.StoredAppleWorkspace),
                localPath: path,
                name: path.split('/').filter(Boolean).pop() ?? 'project',
                lastSnapshotSha256: null,
                lastSource: null,
                lastSyncFileCount: null,
                lastSyncBytes: null,
                lastBuildSucceeded: false,
                lastXcodeVersion: null,
            };
            return view(machine);
        },
        async clearWorkspace(machineId) {
            const machine = find(machineId);
            machine.workspace = null;
            return view(machine);
        },
        async syncWorkspace(machineId) {
            const machine = find(machineId);
            return busy(machine, 'Synchronizing source', async () => {
                const total = 48_213_770;
                const phases: T.AppleProjectPhase[] = [
                    'snapshotting',
                    'transferring',
                    'extracting',
                    'completed',
                ];
                for (const [index, phase] of phases.entries()) {
                    emitter.emit<T.MachineEvent<T.AppleProjectProgress>>(
                        'machine-project-progress',
                        {
                            machineId,
                            phase,
                            completedBytes: (total / 3) * Math.min(index, 3),
                            totalBytes: total,
                            elapsedSeconds: index * 4,
                            detail: phase,
                            logLine: null,
                        },
                    );
                    await sleep(500);
                }
                if (machine.workspace) {
                    machine.workspace.lastSnapshotSha256 =
                        '9999cccc8888dddd7777eeee6666ffff5555aaaa4444bbbb3333cccc2222dddd';
                    machine.workspace.lastSyncFileCount = 1842;
                    machine.workspace.lastSyncBytes = total;
                    machine.workspace.lastBuildSucceeded = false;
                }
                return {
                    view: view(machine),
                    sync: {
                        guestPath: `/Users/${machine.username}/BuildBridge/workspaces/active`,
                        snapshotSha256:
                            '9999cccc8888dddd7777eeee6666ffff5555aaaa4444bbbb3333cccc2222dddd',
                        sourceFileCount: 1842,
                        sourceBytes: total,
                        archiveBytes: 12_400_000,
                    },
                };
            });
        },
        async runSmokeBuild(machineId, target) {
            const machine = find(machineId);
            // The Simulator target downloads Apple's platform once; the device SDK never does.
            const downloadsPlatform =
                target === 'simulator' && machine.iosSimulatorRuntime === null;
            const phases: T.AppleProjectPhase[] = downloadsPlatform
                ? ['preparing_tools', 'preparing_platform', ...projectPhases.slice(1)]
                : projectPhases;
            return busy(machine, 'Running the test build', async () => {
                for (const [index, phase] of phases.entries()) {
                    emitter.emit<T.MachineEvent<T.AppleProjectProgress>>(
                        'machine-project-progress',
                        {
                            machineId,
                            phase,
                            completedBytes: 0,
                            totalBytes: 0,
                            elapsedSeconds: index * 7,
                            detail: phase,
                            logLine:
                                phase === 'building'
                                    ? 'CompileSwift normal arm64 App/AppDelegate.swift'
                                    : null,
                        },
                    );
                    await sleep(500);
                }
                if (downloadsPlatform) {
                    machine.iosSimulatorRuntime = '26.5';
                }
                if (machine.workspace) {
                    machine.workspace.lastBuildSucceeded = true;
                    machine.workspace.lastXcodeVersion = machine.xcodeVersion;
                    machine.workspace.lastBuildTarget = target;
                }
                return {
                    view: view(machine),
                    build: {
                        target,
                        xcodeVersion: machine.xcodeVersion ?? '26.6',
                        nativeLockfileUpdated: false,
                        outputTail: ['** BUILD SUCCEEDED **'],
                    },
                };
            });
        },
        async openMachineScreen(_machineId, url) {
            window.open(url, '_blank', 'noopener');
        },
        async openUrl(url) {
            window.open(url, '_blank', 'noopener');
        },
        async openDeveloperTools() {
            // The browser preview already has its own developer tools.
        },
        async adoptGuestPodfileLock(machineId) {
            const machine = find(machineId);
            return busy(machine, 'Adopting the guest Podfile.lock', async () => {
                await sleep(600);
                if (machine.workspace) {
                    machine.workspace.lastNativeLockUpdated = false;
                }
                return {
                    view: view(machine),
                    changes: {
                        pods: [
                            { name: 'Capacitor', before: '8.3.4', after: '8.4.2' },
                            { name: 'CapacitorCordova', before: '8.3.4', after: '8.4.2' },
                            { name: 'CapacitorCamera', before: '8.2.0', after: '8.2.1' },
                        ],
                        linesAdded: 21,
                        linesRemoved: 21,
                        identical: false,
                    },
                    hostPath: '/home/you/projects/example-app/ios/App/Podfile.lock',
                    backupPath:
                        '/home/you/.config/dev.buildbridge.desktop/machines/default/Podfile.lock.previous',
                };
            });
        },
        async runSignedArchive(machineId, envSetId) {
            const machine = find(machineId);
            machine.archiveEnvSet = envSets.find((set) => set.id === envSetId)?.name ?? null;
            return busy(machine, 'Building the signed archive', async () => {
                const phases: T.AppleArchivePhase[] = [
                    'preparing',
                    ...(envSetId ? (['building_web_assets'] as const) : []),
                    'archiving',
                    'exporting',
                    'verifying',
                    'packaging_archive',
                    'transferring',
                    'completed',
                ];
                for (const [index, phase] of phases.entries()) {
                    emitter.emit<T.MachineEvent<T.AppleArchiveProgress>>(
                        'machine-archive-progress',
                        {
                            machineId,
                            phase,
                            completedBytes: index,
                            totalBytes: phases.length - 1,
                            elapsedSeconds: index * 25,
                            detail: phase,
                            logLine: phase === 'archiving' ? '** ARCHIVE SUCCEEDED **' : null,
                        },
                    );
                    await sleep(600);
                }
                machine.archive = readyMachine().archive;
                machine.archiveError = null;
                return { view: view(machine), archive: machine.archive as T.AppleArchiveResult };
            });
        },
        async revealArchive() {},
        async cancelMachineOperation(machineId) {
            // Every preview operation is a short timer except the device console, which streams
            // until asked to stop.
            find(machineId).cancelRequested = true;
        },
        async clearArchive(machineId) {
            const machine = find(machineId);
            machine.archive = null;
            machine.archiveError = null;
            return view(machine);
        },
        async approveAndroidWorkspace(machineId, path) {
            const machine = find(machineId);
            await sleep(250);
            machine.android ??= {
                workspace: null,
                release: null,
                releaseError: null,
                releaseEnvSet: null,
            };
            machine.android.workspace = {
                localPath: path,
                name: path.split('/').filter(Boolean).pop() ?? 'project',
                applicationId: 'com.example.app',
                lastSnapshotSha256: null,
                lastSyncFileCount: null,
                lastSyncBytes: null,
                lastBuildSucceeded: false,
                lastBuild: null,
                lastSource: null,
            };
            return view(machine);
        },
        async clearAndroidWorkspace(machineId) {
            const machine = find(machineId);
            if (machine.android) {
                machine.android.workspace = null;
            }
            return view(machine);
        },
        async syncAndroidWorkspace(machineId) {
            const machine = find(machineId);
            return busy(machine, 'Synchronizing source', async () => {
                const total = 28_278_463;
                const phases: T.AndroidBuildPhase[] = [
                    'snapshotting',
                    'transferring',
                    'extracting',
                    'completed',
                ];
                for (const [index, phase] of phases.entries()) {
                    emitter.emit<T.MachineEvent<T.AndroidBuildProgress>>(
                        'machine-android-build-progress',
                        {
                            machineId,
                            phase,
                            completedBytes: (total / 3) * Math.min(index, 3),
                            totalBytes: total,
                            elapsedSeconds: index * 2,
                            detail: phase,
                            logLine: null,
                        },
                    );
                    await sleep(400);
                }
                const workspace = machine.android?.workspace;
                if (workspace) {
                    workspace.lastSnapshotSha256 =
                        '6c22009fda0b9467709394b5ae1c442af1da4544d63b5ac01b8778c03d7fcd7b';
                    workspace.lastSyncFileCount = 1397;
                    workspace.lastSyncBytes = total;
                    workspace.lastBuildSucceeded = false;
                    workspace.lastBuild = null;
                }
                return {
                    view: view(machine),
                    sync: {
                        guestPath: '/root/BuildBridge/workspaces/active',
                        snapshotSha256:
                            '6c22009fda0b9467709394b5ae1c442af1da4544d63b5ac01b8778c03d7fcd7b',
                        sourceFileCount: 1397,
                        sourceBytes: total,
                        archiveBytes: 9_100_000,
                    },
                };
            });
        },
        async runAndroidDebugBuild(machineId) {
            const machine = find(machineId);
            return busy(machine, 'Running the debug build', async () => {
                const phases: T.AndroidBuildPhase[] = [
                    'preparing_tools',
                    'installing_dependencies',
                    'building_web_assets',
                    'syncing_android',
                    'building',
                    'inspecting',
                    'completed',
                ];
                for (const [index, phase] of phases.entries()) {
                    emitter.emit<T.MachineEvent<T.AndroidBuildProgress>>(
                        'machine-android-build-progress',
                        {
                            machineId,
                            phase,
                            completedBytes: 0,
                            totalBytes: 0,
                            elapsedSeconds: index * 9,
                            detail: phase,
                            logLine: phase === 'building' ? '> Task :app:compileDebugKotlin' : null,
                        },
                    );
                    await sleep(500);
                }
                const build: T.AndroidBuildResult = {
                    applicationId: 'com.example.app.debug',
                    versionName: '3.2.0',
                    versionCode: '12',
                    toolchain: {
                        jdkVersion: 'openjdk version "21.0.12.1" 2026-08-18 LTS',
                        buildToolsVersion: '35.0.0',
                    },
                    apk: {
                        path: `/home/you/.local/share/dev.buildbridge.desktop/machines/${machineId}/artifacts/debug-${Date.now()}-4242/app-debug.apk`,
                        bytes: 33_410_772,
                        sha256: '5555aaaa6666bbbb7777cccc8888dddd9999eeee0000ffff1111222233334444',
                    },
                    outputTail: ['BUILD SUCCESSFUL in 2m 41s'],
                };
                const workspace = machine.android?.workspace;
                if (workspace) {
                    workspace.lastBuildSucceeded = true;
                    workspace.lastBuild = build;
                }
                return { view: view(machine), build };
            });
        },
        async runAndroidRelease(machineId, envSetId) {
            const machine = find(machineId);
            return busy(machine, 'Building the signed release', async () => {
                const phases: T.AndroidReleasePhase[] = [
                    'preparing',
                    ...(envSetId ? (['building_web_assets'] as const) : []),
                    'bundling',
                    'signing',
                    'verifying',
                    'transferring',
                    'completed',
                ];
                for (const [index, phase] of phases.entries()) {
                    emitter.emit<T.MachineEvent<T.AndroidReleaseProgress>>(
                        'machine-android-release-progress',
                        {
                            machineId,
                            phase,
                            completedBytes: index,
                            totalBytes: phases.length - 1,
                            elapsedSeconds: index * 30,
                            detail: phase,
                            logLine: phase === 'bundling' ? '> Task :app:bundleRelease' : null,
                        },
                    );
                    await sleep(600);
                }
                const release = androidMachine().android?.release as T.AndroidReleaseResult;
                if (machine.android) {
                    machine.android.release = release;
                    machine.android.releaseError = null;
                    machine.android.releaseEnvSet =
                        envSets.find((set) => set.id === envSetId)?.name ?? null;
                }
                return { view: view(machine), release };
            });
        },
        async revealAndroidRelease() {},
        async revealAndroidDebugApk() {},
        async downloadXcode(machineId, query) {
            // The preview has no Apple to talk to: the archive arrives over a few seconds.
            const fileName = `${query.replace(/\s+/g, '_')}.xip`;
            const path = `/home/you/.local/share/dev.buildbridge.desktop/xcode/${fileName}`;
            const total = 3_100_000_000;
            void (async () => {
                for (let step = 1; step <= 6; step += 1) {
                    await sleep(500);
                    emitter.emit<T.XcodeDownloadProgress>('xcode-download-progress', {
                        machineId,
                        path,
                        fileName,
                        bytes: Math.round((total / 6) * step),
                        state: step === 6 ? 'finished' : 'downloading',
                    });
                }
            })();
        },
        async clearAndroidRelease(machineId) {
            const machine = find(machineId);
            if (machine.android) {
                machine.android.release = null;
                machine.android.releaseError = null;
            }
            return view(machine);
        },

        async installUsbReleaseRule() {
            await sleep(600);
            usbRuleInstalled = true;
            changed(null);
            return view(machines[0]!).usb.host;
        },
        async removeUsbReleaseRule() {
            await sleep(300);
            usbRuleInstalled = false;
            changed(null);
            return view(machines[0]!).usb.host;
        },
        async migrateMachineForUsb(machineId) {
            const machine = find(machineId);
            return busy(machine, 'Enabling USB on the machine', async () => {
                const total = 34_526_003_200;
                const phases: [T.DiskMigrationPhase, number, string][] = [
                    [
                        'checking_space',
                        0,
                        'Measuring the container and the free space on this host',
                    ],
                    ['stopping', 0, 'Stopping the machine so its disk is consistent'],
                    ['copying_disk', total / 3, 'Copying the macOS disk to this host'],
                    ['copying_disk', (total * 2) / 3, 'Copying the macOS disk to this host'],
                    ['copying_disk', total, 'Copying the macOS disk to this host'],
                    ['removing', total, 'Removing the old container; its disk is now on this host'],
                    [
                        'creating',
                        total,
                        'Creating the container with the host disk, the control socket, and USB access',
                    ],
                    ['starting', total, 'Starting the machine'],
                    ['completed', total, 'The machine is running from the host disk'],
                ];
                for (const [index, [phase, completedBytes, detail]] of phases.entries()) {
                    emitter.emit<T.MachineEvent<T.DiskMigrationProgress>>(
                        'machine-usb-migration-progress',
                        {
                            machineId,
                            phase,
                            completedBytes,
                            totalBytes: total,
                            elapsedSeconds: index * 20,
                            detail,
                        },
                    );
                    await sleep(phase === 'copying_disk' ? 900 : 500);
                }
                machine.usbContainer = true;
                machine.containerId = `${Date.now().toString(16)}c9f4d1e2a7b3c9f4d1e2a7b3`;
                machine.startedAt = new Date().toISOString();
                machine.state = 'running';
                return view(machine);
            });
        },
        async attachUsbDevice(machineId, bus, port) {
            const machine = find(machineId);
            return busy(machine, 'Attaching the iPhone', async () => {
                await sleep(700);
                const device = hostUsbDevices.find(
                    (candidate) => candidate.bus === bus && candidate.port === port,
                );
                if (!device) {
                    throw new Error(
                        'No Apple device is plugged into that port. Plug the phone in and refresh.',
                    );
                }
                machine.attached = {
                    bus: device.bus,
                    port: device.port,
                    enumerated: true,
                    issue: null,
                };
                machine.guestDevices = [
                    {
                        ...readyPhone(),
                        name: device.product ?? 'iPhone',
                        developerMode: 'unknown',
                        pairingState: 'unpaired',
                        tunnelState: 'disconnected',
                        ready: false,
                        issue: 'Unlock the phone and tap Trust when it asks about this computer.',
                    },
                ];
                machine.deviceRefreshes = 0;
                return view(machine);
            });
        },
        async rebuildMachineContainer(machineId) {
            const machine = find(machineId);
            return busy(machine, 'Rebuilding the container', async () => {
                const phases: T.ContainerRebuildPhase[] = [
                    'shutting_down',
                    'removing',
                    'creating',
                    'starting',
                    'completed',
                ];
                for (const [index, phase] of phases.entries()) {
                    emitter.emit<T.MachineEvent<T.ContainerRebuildProgress>>(
                        'machine-container-rebuild-progress',
                        { machineId, phase, elapsedSeconds: index * 6, detail: phase },
                    );
                    await sleep(400);
                }
                machine.phoneController = true;
                return view(machine);
            });
        },
        async detachUsbDevice(machineId) {
            const machine = find(machineId);
            return busy(machine, 'Detaching the iPhone', async () => {
                await sleep(300);
                machine.attached = null;
                machine.guestDevices = [];
                return view(machine);
            });
        },
        async openSafariWebInspector(machineId) {
            const machine = find(machineId);
            await sleep(900);
            return {
                view: view(machine),
                inspector: { developMenuEnabled: true, safariRestarted: false },
            };
        },
        async pairGuestDevice(machineId, udid) {
            const machine = find(machineId);
            return busy(machine, 'Pairing with the phone', async () => {
                await sleep(1500);
                for (const device of machine.guestDevices) {
                    if (device.udid === udid) {
                        device.pairingState = 'paired';
                        device.tunnelState = 'connected';
                    }
                }
                return view(machine);
            });
        },
        async listGuestDevices(machineId) {
            const machine = find(machineId);
            await sleep(400);
            machine.deviceRefreshes += 1;
            const device = machine.guestDevices[0];
            if (device) {
                if (machine.deviceRefreshes >= 1) {
                    device.pairingState = 'paired';
                    device.tunnelState = 'connected';
                    device.name = 'Matt’s iPhone';
                    device.developerMode = 'disabled';
                    device.issue =
                        'Turn on Developer Mode on the phone (Settings › Privacy & Security), then restart it if asked.';
                }
                if (machine.deviceRefreshes >= 2) {
                    device.developerMode = 'enabled';
                    device.ready = true;
                    device.issue = null;
                }
            }
            return view(machine);
        },
        async prepareAppleDeviceSigning(machineId, udid, deviceName) {
            const machine = find(machineId);
            const kit = storedKits().find((candidate) => candidate.id === attachments[machine.id]);
            if (!kit?.appStoreConnectConfigured) {
                throw new Error(
                    'The attached credentials have no App Store Connect Team key, so the iPhone cannot be registered.',
                );
            }
            return busy(machine, 'Preparing device signing', async () => {
                const created = !kit.developmentCertificateConfigured;
                const phases: T.DeviceSigningPhase[] = [
                    'checking_kit',
                    ...(created ? (['creating_certificate'] as const) : []),
                    'registering_device',
                    'checking_profiles',
                    'creating_profile',
                    'provisioning',
                    'completed',
                ];
                for (const [index, phase] of phases.entries()) {
                    emitter.emit<T.MachineEvent<T.DeviceSigningProgress>>(
                        'machine-device-signing-progress',
                        {
                            machineId,
                            phase,
                            elapsedSeconds: index * 6,
                            detail: `${phase.replace(/_/g, ' ')} for ${deviceName}`,
                        },
                    );
                    await sleep(600);
                }
                kit.developmentCertificateConfigured = true;
                kit.developmentCertificateName = 'development.p12';
                kit.developmentCertificatePasswordStored = true;
                if (machine.signing) {
                    machine.signing.developmentIdentity = developmentIdentity();
                    machine.signing.profiles = [
                        ...machine.signing.profiles.filter(
                            (profile) => profile.kind !== 'development',
                        ),
                        developmentProfile(udid.toUpperCase()),
                    ];
                }
                return {
                    view: view(machine),
                    certificateCreated: created,
                    deviceAlreadyRegistered: false,
                    profileCreated: true,
                };
            });
        },
        async runAppleDeviceBuild(machineId, udid) {
            const machine = find(machineId);
            machine.cancelRequested = false;
            return busy(machine, 'Running on the device', async () => {
                const device =
                    machine.guestDevices.find((candidate) => candidate.udid === udid) ??
                    machine.guestDevices[0];
                if (!device) {
                    throw new Error('The phone is no longer listed by the guest.');
                }
                const started = Date.now();
                const emit = (
                    phase: T.AppleDeviceRunPhase,
                    detail: string,
                    logLines: string[] = [],
                ) =>
                    emitter.emit<T.MachineEvent<T.AppleDeviceRunProgress>>(
                        'machine-device-progress',
                        {
                            machineId,
                            phase,
                            elapsedSeconds: Math.floor((Date.now() - started) / 1000),
                            detail,
                            logLines,
                        },
                    );
                emit('preparing', 'Preparing the recipe');
                await sleep(500);
                emit('resolving_target', 'Reading the Debug build settings');
                await sleep(400);
                emit('building', 'Compiling for the iPhone', [
                    'CompileSwift normal arm64 App/AppDelegate.swift',
                ]);
                await sleep(700);
                emit('building', 'Compiling for the iPhone', [
                    'Ld App.app/App normal',
                    '** BUILD SUCCEEDED **',
                ]);
                await sleep(600);
                emit('verifying', 'Verifying the signature');
                await sleep(400);
                emit('installing', 'Installing on the iPhone', ['App installed: com.example.app']);
                await sleep(700);
                emit('launching', 'Launching', [
                    'Launched application with com.example.app bundle identifier and pid 4211',
                ]);
                await sleep(500);
                const consoleLines = [
                    '[App] scene did become active',
                    'Capacitor: loading app at capacitor://localhost',
                    '[Network] GET /v1/session 200 (84 ms)',
                ];
                let tick = 0;
                while (!machine.cancelRequested && Date.now() - started < 5 * 60_000) {
                    emit('running', 'The app is running; its console streams here.', [
                        consoleLines[tick % consoleLines.length]!,
                    ]);
                    tick += 1;
                    await sleep(700);
                }
                emit('completed', 'Stopped');
                machine.deviceRun = {
                    device: { ...device },
                    bundleIdentifier: 'com.example.app',
                    appPath:
                        '/Users/builder/BuildBridge/workspaces/active/.buildbridge/DerivedData/Build/Products/Debug-iphoneos/App.app',
                    marketingVersion: '3.2.0',
                    buildNumber: '15',
                    provisioningProfileUuid: '22222222-3333-4444-5555-666666666666',
                    installedAtEpochSeconds: Math.floor(Date.now() / 1000),
                    consoleEnd: 'stopped',
                    exitStatus: null,
                    reattached: false,
                    buildTail: ['** BUILD SUCCEEDED **'],
                    consoleTail: consoleLines,
                    projectBundleIdentifier: null,
                };
                machine.deviceRunError = null;
                return { view: view(machine), run: machine.deviceRun };
            });
        },
        async clearAppleDeviceRun(machineId) {
            const machine = find(machineId);
            machine.deviceRun = null;
            machine.deviceRunError = null;
            return view(machine);
        },

        async listEnvSets() {
            await sleep(120);
            refreshEnvAttachments();
            return envSets.map((set) => ({ ...set }));
        },
        async saveEnvSet(input) {
            await sleep(500);
            const existing = envSets.find((set) => set.id === input.setId) ?? null;
            const setId = existing?.id ?? input.name.toLowerCase().replace(/[^a-z0-9]+/g, '-');
            const storedSecrets = envSecretValues[setId] ?? {};
            const variables = input.variables
                .filter((variable) => !variable.secret)
                .map((variable) => ({
                    key: variable.key,
                    value:
                        variable.value ??
                        existing?.variables.find((stored) => stored.key === variable.key)?.value ??
                        '',
                }));
            const secrets = input.variables.filter((variable) => variable.secret);
            envSecretValues[setId] = Object.fromEntries(
                secrets.map((secret) => [
                    secret.key,
                    secret.value ?? storedSecrets[secret.key] ?? '',
                ]),
            );
            const secretKeys = secrets.map((secret) => secret.key);
            if (input.setId) {
                if (existing) {
                    existing.name = input.name;
                    existing.variables = variables;
                    existing.secretKeys = secretKeys;
                }
            } else {
                envSets.push({
                    id: setId,
                    name: input.name,
                    variables,
                    secretKeys,
                    createdAtEpochSeconds: Math.floor(Date.now() / 1000),
                    attachedMachines: [],
                });
            }
            return this.listEnvSets();
        },
        async deleteEnvSet(setId) {
            await sleep(300);
            const index = envSets.findIndex((set) => set.id === setId);
            if (index >= 0) {
                envSets.splice(index, 1);
            }
            delete envSecretValues[setId];
            for (const id of Object.keys(envAttachments)) {
                if (envAttachments[id] === setId) {
                    envAttachments[id] = null;
                }
            }
            return this.listEnvSets();
        },
        async attachEnvSet(machineId, setId) {
            await sleep(300);
            envAttachments[machineId] = setId;
            refreshEnvAttachments();
            const machine = machines.find((entry) => entry.id === machineId)!;
            return view(machine);
        },
        async revealEnvSecrets(setId) {
            await sleep(250);
            const values = envSecretValues[setId];
            if (!values) {
                throw new Error('This environment is no longer stored.');
            }
            return Object.entries(values).map(([key, value]) => ({ key, value }));
        },
        async listSigningKits() {
            await sleep(80);
            return storedKits().map((kit) => ({ ...kit }));
        },
        async saveSigningKit(input) {
            await sleep(250);
            const name = input.name.trim();
            if (name === '') {
                throw new Error('Give the signing credentials a name of 1 to 60 characters.');
            }
            const existing = input.kitId ? kits.find((kit) => kit.id === input.kitId) : null;
            const merged: T.SigningKitSummary = {
                id: existing?.id ?? slug(name),
                name,
                developmentCertificateConfigured:
                    (existing?.developmentCertificateConfigured ?? false) ||
                    input.developmentCertificatePath.trim() !== '',
                developmentCertificateName:
                    input.developmentCertificatePath.trim() !== ''
                        ? (input.developmentCertificatePath.trim().split('/').at(-1) ?? null)
                        : (existing?.developmentCertificateName ?? null),
                developmentCertificatePasswordStored:
                    (existing?.developmentCertificatePasswordStored ?? false) ||
                    input.developmentCertificatePassword !== '',
                appStoreConnectConfigured:
                    existing?.appStoreConnectConfigured || input.appStoreConnectKeyId.trim() !== '',
                appStoreConnectKeyId:
                    input.appStoreConnectKeyId.trim() || (existing?.appStoreConnectKeyId ?? null),
                signingCertificateConfigured:
                    (existing?.signingCertificateConfigured ?? false) ||
                    input.signingCertificatePath.trim() !== '',
                signingCertificateName:
                    input.signingCertificatePath.split('/').pop() ||
                    (existing?.signingCertificateName ?? null),
                signingCertificatePasswordStored:
                    (existing?.signingCertificatePasswordStored ?? false) ||
                    input.signingCertificatePassword !== '',
                provisioningProfileNames: input.provisioningProfilePaths.length
                    ? input.provisioningProfilePaths.map((path) => path.split('/').pop() ?? path)
                    : (existing?.provisioningProfileNames ?? []),
                // Saving invents the keychain password when none is typed.
                guestKeychainConfigured: true,
                createdAtEpochSeconds:
                    existing?.createdAtEpochSeconds ?? Math.floor(Date.now() / 1000),
                attachedMachines: existing?.attachedMachines ?? [],
                androidKeystoreConfigured:
                    (existing?.androidKeystoreConfigured ?? false) ||
                    input.androidKeystorePath.trim() !== '',
                androidKeystoreName:
                    input.androidKeystorePath.trim().split('/').pop() ||
                    (existing?.androidKeystoreName ?? null),
                androidKeyAlias:
                    input.androidKeyAlias.trim() || (existing?.androidKeyAlias ?? null),
                androidKeystorePasswordStored:
                    (existing?.androidKeystorePasswordStored ?? false) ||
                    input.androidKeystorePassword !== '',
                androidKeyPasswordStored:
                    (existing?.androidKeyPasswordStored ?? false) ||
                    input.androidKeyPassword !== '',
            };
            if (existing) {
                kits[kits.indexOf(existing)] = merged;
            } else {
                kits.push(merged);
            }
            return kits.map((kit) => ({ ...kit }));
        },
        async deleteSigningKit(kitId) {
            const index = kits.findIndex((kit) => kit.id === kitId);
            if (index < 0) {
                throw new Error('These signing credentials are no longer stored.');
            }
            kits.splice(index, 1);
            for (const [machineId, attached] of Object.entries(attachments)) {
                if (attached === kitId) {
                    attachments[machineId] = null;
                }
            }
            return kits.map((kit) => ({ ...kit }));
        },
        async attachSigningKit(machineId, kitId) {
            const machine = find(machineId);
            attachments[machineId] = kitId;
            for (const kit of kits) {
                kit.attachedMachines = Object.entries(attachments)
                    .filter(([, attached]) => attached === kit.id)
                    .map(([id]) => machines.find((entry) => entry.id === id)?.config.name ?? id);
            }
            return view(machine);
        },

        async verifyAppleTeam() {
            await sleep(900);
            return {
                keyId: 'KEYID12345',
                projectDevelopmentTeam: 'TEAM123456',
                bundleIdentifier: 'com.example.app',
                appStoreRecordFound: true,
                appStoreAppName: 'Example App',
                appStoreAppId: '1234567890',
                bundleIdFound: true,
                bundleIdName: 'Example App',
                bundleIdPlatform: 'UNIVERSAL',
                appIdPrefix: 'TEAM123456',
                bundleLookupFallbackUsed: true,
                developerResourcesAccessible: true,
                developerResourcesIssue: null,
                profilesAccessible: true,
                profilesIssue: null,
                profiles: [
                    {
                        id: 'prof-1',
                        name: 'BuildBridge App Store profile',
                        platform: 'IOS',
                        profileType: 'IOS_APP_STORE',
                        profileState: 'ACTIVE',
                        uuid: '11111111-2222-3333-4444-555555555555',
                        createdDate: '2026-09-02T09:00:00Z',
                        expirationDate: '2027-09-02T10:14:00Z',
                        deviceUdids: [],
                        certificateIds: ['cert-1'],
                    },
                    {
                        id: 'prof-3',
                        name: 'BuildBridge Development 1756900000',
                        platform: 'IOS',
                        profileType: 'IOS_APP_DEVELOPMENT',
                        profileState: 'ACTIVE',
                        uuid: '22222222-3333-4444-5555-666666666666',
                        createdDate: '2026-09-03T09:00:00Z',
                        expirationDate: '2027-09-02T10:14:00Z',
                        deviceUdids: [DEVICE_UDID],
                        certificateIds: ['cert-2'],
                    },
                    {
                        id: 'prof-2',
                        name: 'Example App Store',
                        platform: 'IOS',
                        profileType: 'IOS_APP_STORE',
                        profileState: 'EXPIRED',
                        uuid: '66666666-7777-8888-9999-000000000000',
                        createdDate: '2025-07-28T09:00:00Z',
                        expirationDate: '2026-07-28T09:00:00Z',
                        deviceUdids: [],
                        certificateIds: [],
                    },
                ],
                certificatesAccessible: true,
                certificatesIssue: null,
                certificates: [
                    {
                        id: 'cert-1',
                        name: 'iOS Distribution',
                        displayName: 'Example Developer',
                        certificateType: 'IOS_DISTRIBUTION',
                        serialNumber: '1111222233334444',
                        platform: 'IOS',
                        expirationDate: '2027-09-02T10:14:00Z',
                    },
                    {
                        id: 'cert-2',
                        name: 'Apple Development',
                        displayName: 'Example Developer',
                        certificateType: 'DEVELOPMENT',
                        serialNumber: '5555666677778888',
                        platform: 'IOS',
                        expirationDate: '2027-03-11T10:14:00Z',
                    },
                ],
                devicesAccessible: true,
                devicesIssue: null,
                devices: [
                    {
                        id: 'dev-1',
                        name: 'Matt’s iPhone',
                        udid: DEVICE_UDID,
                        platform: 'IOS',
                        status: 'ENABLED',
                        deviceClass: 'IPHONE',
                        model: 'iPhone 15 Pro',
                        addedDate: '2026-09-03T09:00:00Z',
                    },
                ],
                verifiedAtEpochSeconds: Math.floor(Date.now() / 1000),
            };
        },
        async createAppleProfile(machineId: string) {
            await sleep(900);
            const verification = await this.verifyAppleTeam(machineId);
            const kit = kits.find((entry) => entry.id === attachments[machineId]) ?? kits[0]!;
            kit.provisioningProfileNames = [
                '11111111-2222-3333-4444-555555555555.mobileprovision',
                ...kit.provisioningProfileNames,
            ];
            return {
                profile: verification.profiles[0] as T.AppleProvisioningProfile,
                certificate: verification.certificates[0] as T.AppleCertificate,
                savedPath:
                    '/home/you/.config/dev.buildbridge.desktop/macos-builder/profiles/11111111-2222-3333-4444-555555555555.mobileprovision',
                kit: { ...kit },
            };
        },

        async createAppleCertificate(kitId: string) {
            await sleep(1200);
            const kit = kits.find((entry) => entry.id === kitId)!;
            kit.signingCertificateConfigured = true;
            kit.signingCertificateName = 'distribution.p12';
            kit.signingCertificatePasswordStored = true;
            return {
                certificate: {
                    id: 'cert-new',
                    name: 'Apple Distribution: Example Developer (TEAM123456)',
                    displayName: 'Example Developer',
                    certificateType: 'DISTRIBUTION',
                    serialNumber: '0123456789ABCDEF',
                    platform: 'IOS',
                    expirationDate: '2027-09-03T10:00:00Z',
                },
                savedPath:
                    '/home/you/.config/dev.buildbridge.desktop/macos-builder/certificates/distribution-1756800000/distribution.p12',
                kit: { ...kit },
            };
        },
        async createAppleDevelopmentCertificate(kitId: string) {
            await sleep(1200);
            const kit = kits.find((entry) => entry.id === kitId)!;
            kit.developmentCertificateConfigured = true;
            kit.developmentCertificateName = 'development.p12';
            kit.developmentCertificatePasswordStored = true;
            return {
                certificate: {
                    id: 'cert-2',
                    name: 'Apple Development: Example Developer (TEAM123456)',
                    displayName: 'Example Developer',
                    certificateType: 'DEVELOPMENT',
                    serialNumber: '5555666677778888',
                    platform: 'IOS',
                    expirationDate: '2027-03-11T10:14:00Z',
                },
                savedPath:
                    '/home/you/.config/dev.buildbridge.desktop/macos-builder/certificates/development-1756900000/development.p12',
                kit: { ...kit },
            };
        },
        async listGuestOptimizations(machineId: string) {
            await sleep(200);
            return optimizationsFor(machineId);
        },
        async applyGuestOptimization(machineId: string, optimizationId: string) {
            const machine = find(machineId);
            return busy(machine, 'Applying an optimization', async () => {
                await sleep(1200);
                appliedOptimizations.add(`${machineId}:${optimizationId}`);
                return optimizationsFor(machineId);
            });
        },
        async listManagedAppleProfiles() {
            await sleep(120);
            return [
                {
                    fileName: '11111111-2222-3333-4444-555555555555.mobileprovision',
                    path: '/home/you/.config/dev.buildbridge.desktop/macos-builder/profiles/11111111-2222-3333-4444-555555555555.mobileprovision',
                    savedAtEpochSeconds: Math.floor(Date.now() / 1000) - 86_400,
                },
            ];
        },

        async createAndroidKeystore(kitId, input) {
            await sleep(900);
            const kit = kits.find((entry) => entry.id === kitId);
            if (!kit) {
                throw new Error('These signing credentials are no longer stored.');
            }
            if (kit.androidKeystoreConfigured) {
                throw new Error(
                    'These credentials already hold an Android keystore. Remove it before creating another.',
                );
            }
            if (input.password.length < 6) {
                throw new Error('Choose a keystore password of six to 512 characters on one line.');
            }
            kit.androidKeystoreConfigured = true;
            kit.androidKeystoreName = 'upload.keystore';
            kit.androidKeyAlias = input.keyAlias.trim() || 'upload';
            kit.androidKeystorePasswordStored = true;
            kit.androidKeyPasswordStored = false;
            return {
                keystore: {
                    path: `/home/you/.config/dev.buildbridge.desktop/android-builder/keystores/upload-${Math.floor(Date.now() / 1000)}/upload.keystore`,
                    keyAlias: kit.androidKeyAlias,
                    certificateSha256:
                        '2f7c1e9a4b3d5c6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6',
                },
                kit: { ...kit },
            };
        },

        async downloadAppleProfile(machineId: string, profileId: string) {
            await sleep(700);
            const verification = await this.verifyAppleTeam(machineId);
            const profile = (verification.profiles.find((entry) => entry.id === profileId) ??
                verification.profiles[0]) as T.AppleProvisioningProfile;
            const kit = kits.find((entry) => entry.id === attachments[machineId]) ?? kits[0]!;
            const fileName = `${profile.uuid}.mobileprovision`;
            if (!kit.provisioningProfileNames.includes(fileName)) {
                kit.provisioningProfileNames = [fileName, ...kit.provisioningProfileNames];
            }
            return {
                profile,
                savedPath: `/home/you/.config/dev.buildbridge.desktop/macos-builder/profiles/${fileName}`,
                kit: { ...kit },
            };
        },

        onMachineChanged: async (handler) => emitter.on('machine-changed', handler),
        onLaunchProgress: async (handler) => emitter.on('machine-launch-progress', handler),
        onXcodeProgress: async (handler) => emitter.on('machine-xcode-progress', handler),
        onSigningProgress: async (handler) => emitter.on('machine-signing-progress', handler),
        onProjectProgress: async (handler) => emitter.on('machine-project-progress', handler),
        onArchiveProgress: async (handler) => emitter.on('machine-archive-progress', handler),
        onUsbMigrationProgress: async (handler) =>
            emitter.on('machine-usb-migration-progress', handler),
        onContainerRebuildProgress: async (handler) =>
            emitter.on('machine-container-rebuild-progress', handler),
        onTemplateProgress: async (handler) => emitter.on('machine-template-progress', handler),
        onUsbAttachProgress: async (handler) => emitter.on('machine-usb-attach-progress', handler),
        onDeviceSigningProgress: async (handler) =>
            emitter.on('machine-device-signing-progress', handler),
        onDeviceProgress: async (handler) => emitter.on('machine-device-progress', handler),
        onAndroidBuildProgress: async (handler) =>
            emitter.on('machine-android-build-progress', handler),
        onAndroidReleaseProgress: async (handler) =>
            emitter.on('machine-android-release-progress', handler),
        onXcodeDownloadProgress: async (handler) => emitter.on('xcode-download-progress', handler),
        onDragDrop: async (handler: (event: DragDropEvent) => void) => {
            void handler;
            return () => {};
        },
        async pickPaths(request) {
            // The browser preview has no native dialog; return a plausible path so the field's
            // behaviour after a pick can still be exercised.
            await sleep(150);
            const extension = request.filter?.extensions[0];
            if (request.kind === 'directory') {
                return ['/path/to/example-app'];
            }
            if (request.kind === 'files') {
                return [`/path/to/AppStore.${extension ?? 'file'}`];
            }
            return [`/path/to/chosen.${extension ?? 'file'}`];
        },
    };
}
