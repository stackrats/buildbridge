import { describe, expect, it } from 'vite-plus/test';

import type {
    GuestDevice,
    HostUsbDevice,
    MacBuilderView,
    MachineUsbStatus,
    SigningProvisioningResult,
} from '../types/backend';
import {
    deviceNextSummary,
    deviceReadiness,
    deviceSigningReady,
    deviceWorkingSummary,
} from './device';

const UDID = '00008030-000A1B2C3D4E5F6A';

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
        identityName: 'iPhone Distribution: Example (TEAM123456)',
        identitySha1: 'sha1',
        certificateSha256: 'sha256',
        certificateExpiresAt: '2027-09-02T00:00:00Z',
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
                applicationIdentifier: 'TEAM123456.com.example.app',
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
function usbView(
    usb: Partial<MachineUsbStatus>,
    extra: Partial<MacBuilderView> = {},
): MacBuilderView {
    return {
        runtime: { state: 'running' },
        guest: { devices: [] },
        signing: null,
        signingKit: null,
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
    } as unknown as MacBuilderView;
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
        } as Partial<MacBuilderView>);
        expect(deviceReadiness(unpaired).substate).toBe('trust');

        const paired = usbView(withHost, {
            guest: { devices: [guestDevice({ developerMode: 'disabled' })] },
        } as Partial<MacBuilderView>);
        expect(deviceReadiness(paired).substate).toBe('signing');

        const signed = usbView(withHost, {
            guest: { devices: [guestDevice({ developerMode: 'disabled' })] },
            signing: signing(),
        } as Partial<MacBuilderView>);
        expect(deviceReadiness(signed).substate).toBe('developer-mode');

        const ready = usbView(withHost, {
            guest: { devices: [guestDevice()] },
            signing: signing(),
        } as Partial<MacBuilderView>);
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
        } as Partial<MacBuilderView>);
        expect(deviceReadiness(withoutKey).canPrepareSigning).toBe(false);
        expect(deviceNextSummary(deviceReadiness(withoutKey), withoutKey)).toContain('Team key');

        const withKey = usbView(base, {
            guest: { devices: [guestDevice()] },
            signingKit: { appStoreConnectConfigured: true },
        } as Partial<MacBuilderView>);
        expect(deviceReadiness(withKey).canPrepareSigning).toBe(true);
        expect(deviceNextSummary(deviceReadiness(withKey), withKey)).toContain('Register');
    });

    it('names the phone from the host until the guest names it', () => {
        const attached = { bus: 1, port: '3', enumerated: false, issue: null };
        const host = usbView({
            host: { ...usbView({}).usb.host, devices: [hostDevice()] },
            attached,
        });
        expect(deviceReadiness(host).name).toBe('iPhone');
        expect(deviceNextSummary(deviceReadiness(host), host)).toContain('Unlock iPhone');
        expect(deviceWorkingSummary(deviceReadiness(host))).toContain('state');
    });
});

describe('deviceSigningReady', () => {
    it('requires the development identity and a development profile listing the phone', () => {
        expect(deviceSigningReady(null, UDID)).toBe(false);
        expect(deviceSigningReady({ ...signing(), developmentIdentity: null }, UDID)).toBe(false);
        expect(deviceSigningReady(signing(['00008030-FFFFFFFFFFFFFFFF']), UDID)).toBe(false);
        expect(deviceSigningReady(signing([UDID.toLowerCase()]), UDID)).toBe(true);
        const appStoreOnly = signing();
        appStoreOnly.profiles[0]!.kind = 'app_store';
        expect(deviceSigningReady(appStoreOnly, UDID)).toBe(false);
    });
});
