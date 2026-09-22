import { describe, expect, it } from 'vite-plus/test';

import type {
    GuestDevice,
    HostUsbDevice,
    MachineView,
    MachineUsbStatus,
    SigningProvisioningResult,
} from '../types/backend';
import {
    deviceChecks,
    deviceNextSummary,
    deviceReadiness,
    deviceRungNeedsBuild,
    deviceSigningReady,
    deviceWorkingSummary,
    matchingDeviceDevelopmentProfile,
} from './device';
import type { DeviceSubstate } from './device';

const UDID = '00008030-000A1B2C3D4E5F6A';
const DEBUG_BUNDLE_IDENTIFIER = 'com.example.app.debug';

function hostDevice(): HostUsbDevice {
    return {
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
    };
}

function guestDevice(overrides: Partial<GuestDevice> = {}): GuestDevice {
    return {
        identifier: 'E3F1A2B4-5C6D-4E7F-8A9B-0C1D2E3F4A5B',
        udid: UDID,
        name: 'Matt’s iPhone',
        osVersion: '18.6',
        model: 'iPhone 15 Pro',
        developerMode: 'enabled',
        pairingState: 'paired',
        tunnelState: 'connected',
        transportType: 'wired',
        ready: true,
        issue: null,
        ...overrides,
    };
}

function signing(devices: string[] = [UDID]): SigningProvisioningResult {
    return {
        keychainPath: '/k',
        distributionIdentity: {
            identityName: 'iPhone Distribution: Example (TEAM123456)',
            identitySha1: 'sha1',
            certificateSha256: 'sha256',
            certificateExpiresAt: '2027-09-02T00:00:00Z',
        },
        developmentTeam: 'TEAM123456',
        bundleIdentifier: 'com.example.app',
        developmentIdentity: {
            identityName: 'Apple Development: Example (TEAM123456)',
            identitySha1: 'dev1',
            certificateSha256: 'dev256',
            certificateExpiresAt: '2027-09-02T00:00:00Z',
        },
        profiles: [
            {
                uuid: '22222222-3333-4444-5555-666666666666',
                teamIdentifier: 'TEAM123456',
                applicationIdentifier: `TEAM123456.${DEBUG_BUNDLE_IDENTIFIER}`,
                expiresAt: '2027-09-02T00:00:00Z',
                developerCertificateSha256: ['dev256'],
                kind: 'development',
                provisionedDeviceUdids: devices,
                getTaskAllow: true,
            },
        ],
    };
}

/** Only the facts the ladder reads; the rest of the view is not consulted. */
function usbView(usb: Partial<MachineUsbStatus>, extra: Partial<MachineView> = {}): MachineView {
    return {
        runtime: { state: 'running' },
        guest: { devices: [] },
        signing: null,
        signingKit: null,
        appleWorkspace: { debugBundleIdentifier: DEBUG_BUNDLE_IDENTIFIER },
        usb: {
            host: {
                supported: true,
                rule: 'installed',
                rulePath: '/etc/udev/rules.d/40-buildbridge-iphone.rules',
                usbmuxdActive: false,
                plugdevGid: 46,
                devices: [],
                issues: [],
            },
            diskOnHost: true,
            containerReady: true,
            containerIssue: null,
            qmpReachable: true,
            attached: null,
            ...usb,
        },
        ...extra,
    } as unknown as MachineView;
}

describe('deviceReadiness', () => {
    it('starts at the host rule and walks every rung to ready', () => {
        const rule = usbView({ host: { ...usbView({}).usb.host, rule: 'missing' } });
        expect(deviceReadiness(rule).substate).toBe('host-rule');

        expect(deviceReadiness(usbView({ containerReady: false })).substate).toBe('container');
        expect(deviceReadiness(usbView({})).substate).toBe('plug-in');

        const plugged = usbView({ host: { ...usbView({}).usb.host, devices: [hostDevice()] } });
        expect(deviceReadiness(plugged).substate).toBe('attach');

        const attached = { bus: 1, port: '3', enumerated: true, issue: null };
        const unplugged = usbView({ attached });
        expect(deviceReadiness(unplugged).substate).toBe('unplugged');

        const withHost = { host: { ...usbView({}).usb.host, devices: [hostDevice()] }, attached };
        expect(deviceReadiness(usbView(withHost)).substate).toBe('trust');
        const unpaired = usbView(withHost, {
            guest: { devices: [guestDevice({ pairingState: 'unpaired' })] },
        } as Partial<MachineView>);
        expect(deviceReadiness(unpaired).substate).toBe('trust');

        const paired = usbView(withHost, {
            guest: { devices: [guestDevice({ developerMode: 'disabled' })] },
        } as Partial<MachineView>);
        expect(deviceReadiness(paired).substate).toBe('signing');

        const signed = usbView(withHost, {
            guest: { devices: [guestDevice({ developerMode: 'disabled' })] },
            signing: signing(),
        } as Partial<MachineView>);
        expect(deviceReadiness(signed).substate).toBe('developer-mode');

        const ready = usbView(withHost, {
            guest: { devices: [guestDevice()] },
            signing: signing(),
        } as Partial<MachineView>);
        const readiness = deviceReadiness(ready);
        expect(readiness.substate).toBe('ready');
        expect(readiness.name).toBe('Matt’s iPhone');
        expect(readiness.signingReady).toBe(true);
    });

    it('needs a Team key on the kit to prepare signing', () => {
        const attached = { bus: 1, port: '3', enumerated: true, issue: null };
        const base = { host: { ...usbView({}).usb.host, devices: [hostDevice()] }, attached };
        const withoutKey = usbView(base, {
            guest: { devices: [guestDevice()] },
        } as Partial<MachineView>);
        expect(deviceReadiness(withoutKey).canPrepareSigning).toBe(false);
        expect(deviceNextSummary(deviceReadiness(withoutKey), withoutKey)).toContain('Team key');

        const withKey = usbView(base, {
            guest: { devices: [guestDevice()] },
            signingKit: { appStoreConnectConfigured: true },
        } as Partial<MachineView>);
        expect(deviceReadiness(withKey).canPrepareSigning).toBe(true);
        expect(deviceNextSummary(deviceReadiness(withKey), withKey)).toContain('Register');
    });

    it.each(['production profile', 'unknown Debug identifier'])(
        'keeps Prepare signing reachable with a %s',
        (scenario) => {
            const prepared = signing();
            prepared.profiles[0]!.applicationIdentifier = 'TEAM123456.com.example.app';
            const view = usbView(
                {
                    host: { ...usbView({}).usb.host, devices: [hostDevice()] },
                    attached: { bus: 1, port: '3', enumerated: true, issue: null },
                },
                {
                    guest: { devices: [guestDevice()] },
                    signing: prepared,
                    signingKit: { appStoreConnectConfigured: true },
                    appleWorkspace: {
                        bundleIdentifier: 'com.example.app',
                        debugBundleIdentifier:
                            scenario === 'unknown Debug identifier'
                                ? null
                                : DEBUG_BUNDLE_IDENTIFIER,
                    },
                } as Partial<MachineView>,
            );

            const readiness = deviceReadiness(view);
            expect(readiness.substate).toBe('signing');
            expect(readiness.canPrepareSigning).toBe(true);
            expect(readiness.signingReady).toBe(false);
            expect(deviceChecks(view).find((check) => check.label === 'Signing')?.ok).toBe(false);
        },
    );

    it('lets manual signing wait for a Debug identifier without requiring a Team key', () => {
        const view = usbView(
            {
                host: { ...usbView({}).usb.host, devices: [hostDevice()] },
                attached: { bus: 1, port: '3', enumerated: true, issue: null },
            },
            {
                guest: { devices: [guestDevice()] },
                signing: signing(),
                appleWorkspace: {
                    bundleIdentifier: 'com.example.app',
                    debugBundleIdentifier: null,
                },
            } as Partial<MachineView>,
        );
        const readiness = deviceReadiness(view);
        expect(readiness.substate).toBe('signing');
        expect(readiness.canPrepareSigning).toBe(false);
        expect(deviceNextSummary(readiness, view)).toContain('Check again');

        view.appleWorkspace!.debugBundleIdentifier = DEBUG_BUNDLE_IDENTIFIER;
        expect(deviceReadiness(view).substate).toBe('ready');
    });

    it('names the phone from the host until the guest names it', () => {
        const attached = { bus: 1, port: '3', enumerated: true, issue: null };
        const host = usbView({
            host: { ...usbView({}).usb.host, devices: [hostDevice()] },
            attached,
        });
        expect(deviceReadiness(host).substate).toBe('trust');
        expect(deviceReadiness(host).name).toBe('iPhone');
        expect(deviceNextSummary(deviceReadiness(host), host)).toContain('Unlock iPhone');
        expect(deviceWorkingSummary(deviceReadiness(host))).toContain('state');
    });

    it('is a replug, not a trust, when QEMU holds a phone it could not read', () => {
        const attached = { bus: 1, port: '3', enumerated: false, issue: 'unreadable' };
        const host = usbView({
            host: { ...usbView({}).usb.host, devices: [hostDevice()] },
            attached,
        });
        expect(deviceReadiness(host).substate).toBe('replug');
        expect(deviceNextSummary(deviceReadiness(host), host)).toContain('restart the machine');
    });
});

describe('deviceRungNeedsBuild', () => {
    it('withholds only signing and the run from a pending step', () => {
        const rungs: DeviceSubstate[] = [
            'host-rule',
            'container',
            'plug-in',
            'attach',
            'unplugged',
            'replug',
            'trust',
            'signing',
            'developer-mode',
            'ready',
        ];
        expect(rungs.filter(deviceRungNeedsBuild)).toEqual(['signing', 'ready']);
    });
});

describe('deviceSigningReady', () => {
    it('requires the development identity and a development profile listing the phone', () => {
        expect(deviceSigningReady(null, UDID, DEBUG_BUNDLE_IDENTIFIER)).toBe(false);
        expect(
            deviceSigningReady(
                { ...signing(), developmentIdentity: null },
                UDID,
                DEBUG_BUNDLE_IDENTIFIER,
            ),
        ).toBe(false);
        expect(
            deviceSigningReady(
                signing(['00008030-FFFFFFFFFFFFFFFF']),
                UDID,
                DEBUG_BUNDLE_IDENTIFIER,
            ),
        ).toBe(false);
        expect(
            deviceSigningReady(signing([UDID.toLowerCase()]), UDID, DEBUG_BUNDLE_IDENTIFIER),
        ).toBe(true);
        const appStoreOnly = signing();
        appStoreOnly.profiles[0]!.kind = 'app_store';
        expect(deviceSigningReady(appStoreOnly, UDID, DEBUG_BUNDLE_IDENTIFIER)).toBe(false);
    });

    it.each([null, undefined, '', ' '])(
        'requires a known Debug bundle identifier (%s)',
        (bundle) => {
            expect(deviceSigningReady(signing(), UDID, bundle)).toBe(false);
        },
    );

    it.each([
        ['TEAM123456.com.example.app.debug', true],
        ['TEAM123456.com.example.app', false],
        ['TEAM123456.com.example.*', true],
        ['TEAM123456.*', true],
        ['TEAM123456.com.other.*', false],
        ['TEAM123456.com.example.app*', false],
        ['TEAM123456.com.*.debug', false],
        ['TEAM123456.com.*.*', false],
        ['TEAM123456', false],
    ])('checks the profile application identifier %s', (identifier, ready) => {
        const prepared = signing();
        prepared.profiles[0]!.applicationIdentifier = identifier;
        expect(deviceSigningReady(prepared, UDID, DEBUG_BUNDLE_IDENTIFIER)).toBe(ready);
    });

    it('selects the matching Debug profile even when a production profile comes first', () => {
        const prepared = signing();
        const debugProfile = prepared.profiles[0]!;
        prepared.profiles.unshift({
            ...debugProfile,
            uuid: 'production-profile',
            applicationIdentifier: 'TEAM123456.com.example.app',
        });
        expect(matchingDeviceDevelopmentProfile(prepared, UDID, DEBUG_BUNDLE_IDENTIFIER)).toBe(
            debugProfile,
        );
    });

    it('requires the provisioned development identity and team to match the profile', () => {
        const prepared = signing();
        prepared.profiles[0]!.developerCertificateSha256 = ['other-certificate'];
        expect(deviceSigningReady(prepared, UDID, DEBUG_BUNDLE_IDENTIFIER)).toBe(false);
        prepared.profiles[0]!.developerCertificateSha256 = ['dev256'];
        prepared.profiles[0]!.teamIdentifier = 'OTHERTEAM';
        expect(deviceSigningReady(prepared, UDID, DEBUG_BUNDLE_IDENTIFIER)).toBe(false);
    });
});

describe('device checks', () => {
    it('passes every rung below the current one and names why the rest wait', () => {
        const missingRule = usbView({ host: { ...usbView({}).usb.host, rule: 'missing' } });
        const checks = deviceChecks(missingRule);

        expect(checks.map((check) => check.label)).toEqual([
            'Host rule',
            'Container',
            'Attached',
            'Trusted',
            'Signing',
            'Developer Mode',
        ]);
        expect(checks.every((check) => !check.ok)).toBe(true);
        expect(checks[2]?.detail).toBe('plug the phone into this host');
    });

    it('treats a phone QEMU holds but the guest has not seen as not attached', () => {
        const held = usbView({
            host: { ...usbView({}).usb.host, devices: [hostDevice()] },
            attached: { bus: 1, port: '3', enumerated: false, issue: null },
        });
        const attached = deviceChecks(held).find((check) => check.label === 'Attached')!;

        expect(attached.ok).toBe(false);
        expect(attached.detail).toBe('held by QEMU, not yet seen by the guest');
    });

    it('passes all six once the phone is ready', () => {
        const ready = usbView(
            {
                host: { ...usbView({}).usb.host, devices: [hostDevice()] },
                attached: { bus: 1, port: '3', enumerated: true, issue: null },
            },
            { guest: { devices: [guestDevice()] } as MachineView['guest'], signing: signing() },
        );
        const readiness = deviceReadiness(ready);

        expect(readiness.substate).toBe('ready');
        expect(deviceChecks(ready, readiness).every((check) => check.ok)).toBe(true);
    });
});
