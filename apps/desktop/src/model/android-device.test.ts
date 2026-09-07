import { describe, expect, it } from 'vite-plus/test';

import type { AndroidDevice, AndroidMachineView } from '../types/backend';
import { androidDeviceApks, androidInstallCommand, androidNetworkWarning } from './android-device';

function androidView(): AndroidMachineView {
    return {
        workspace: {
            localPath: '/home/you/app',
            name: 'app',
            applicationId: 'com.example.app',
            lastSnapshotSha256: 'snapshot',
            lastSyncFileCount: 10,
            lastSyncBytes: 1000,
            lastSyncedAtEpochSeconds: null,
            lastBuildSucceeded: true,
            lastBuild: {
                allowHttp: false,
                applicationId: 'com.example.app.debug',
                versionName: '2.0',
                versionCode: '20',
                toolchain: { jdkVersion: '17', buildToolsVersion: '35.0.0' },
                apk: { path: '/artifacts/debug/app-debug.apk', bytes: 2000, sha256: 'debug' },
                outputTail: [],
            },
            lastSource: null,
        },
        release: {
            applicationId: 'com.example.previous',
            versionName: '1.0',
            versionCode: '10',
            keyAlias: 'upload',
            certificateSha256: 'certificate',
            aab: { path: '/artifacts/release/app.aab', bytes: 3000, sha256: 'bundle' },
            apk: { path: '/artifacts/release/app.apk', bytes: 4000, sha256: 'release' },
            outputTail: [],
        },
        releaseEnvSet: 'Production',
        releaseError: null,
        deviceRun: null,
        deviceRunError: null,
    };
}

describe('Android device APKs', () => {
    it('prefers the debug APK and keeps each retained build’s own metadata', () => {
        const view = androidView();

        expect(androidDeviceApks(view)).toEqual([
            expect.objectContaining({
                value: 'debug',
                artifact: view.workspace!.lastBuild!.apk,
                applicationId: 'com.example.app.debug',
                version: '2.0 (20)',
                environment: null,
            }),
            expect.objectContaining({
                value: 'release',
                artifact: view.release!.apk,
                applicationId: 'com.example.previous',
                version: '1.0 (10)',
                environment: 'Production',
            }),
        ]);
    });

    it('offers no installation for an AAB without a retained APK', () => {
        const view = androidView();
        view.workspace!.lastBuild!.apk = null;
        view.release!.apk = null;

        expect(androidDeviceApks(view)).toEqual([]);
    });

    it('offers a retained release APK even when the workspace has been cleared', () => {
        const view = androidView();
        view.workspace = null;
        view.releaseEnvSet = null;

        expect(androidDeviceApks(view)).toEqual([
            expect.objectContaining({
                value: 'release',
                artifact: view.release!.apk,
                applicationId: 'com.example.previous',
                environment: null,
            }),
        ]);
    });

    it('allows a debug APK without release signing credentials or artifacts', () => {
        const view = androidView();
        view.release = null;
        view.releaseEnvSet = null;

        expect(androidDeviceApks(view).map((apk) => apk.value)).toEqual(['debug']);
    });

    it('has no candidates before Android build records are available', () => {
        expect(androidDeviceApks(null)).toEqual([]);
        expect(androidDeviceApks(undefined)).toEqual([]);
        expect(
            androidDeviceApks({
                workspace: null,
                release: null,
                releaseEnvSet: null,
                releaseError: null,
                deviceRun: null,
                deviceRunError: null,
            }),
        ).toEqual([]);
    });
});

describe('Android install command', () => {
    it('quotes the host APK path as one argument', () => {
        expect(androidInstallCommand('/home/you/My App/app-debug.apk')).toBe(
            "adb install -r '/home/you/My App/app-debug.apk'",
        );
    });

    it('puts a trimmed device serial before the install subcommand', () => {
        expect(androidInstallCommand('/app.apk', '  emulator-5554  ')).toBe(
            "adb -s 'emulator-5554' install -r '/app.apk'",
        );
    });

    it('does not select a device for an empty or whitespace-only serial', () => {
        expect(androidInstallCommand('/app.apk', '')).toBe("adb install -r '/app.apk'");
        expect(androidInstallCommand('/app.apk', ' \t ')).toBe("adb install -r '/app.apk'");
    });

    it('keeps quotes and shell syntax in the path and serial inside their arguments', () => {
        expect(
            androidInstallCommand("/tmp/it's $(touch marker); `id`.apk", "device'; echo injected"),
        ).toBe(
            "adb -s 'device'\\''; echo injected' install -r '/tmp/it'\\''s $(touch marker); `id`.apk'",
        );
    });
});

describe('Android device network warning', () => {
    const phone = (network: AndroidDevice['network']): AndroidDevice => ({
        serial: 'phone',
        state: 'device',
        model: 'Pixel 9',
        network,
    });
    const home = ['192.168.110.0/24'];

    it('says nothing for a phone on this computer’s network, or one that was not asked', () => {
        expect(
            androidNetworkWarning(
                phone({ address: '192.168.110.252/24', onHostNetwork: true }),
                home,
            ),
        ).toBeNull();
        expect(androidNetworkWarning(phone(null), home)).toBeNull();
        expect(androidNetworkWarning(null, home)).toBeNull();
        expect(androidNetworkWarning(undefined, home)).toBeNull();
    });

    it('names the network to join when the phone is on mobile data alone', () => {
        const warning = androidNetworkWarning(phone({ address: null, onHostNetwork: false }), home);
        expect(warning?.title).toBe('The phone is not on Wi-Fi');
        expect(warning?.message).toContain('Mobile data is all it has');
        expect(warning?.message).toContain('(192.168.110.0/24)');
        expect(warning?.message).toContain('then refresh devices');
    });

    it('names the phone’s own network when it is on a different one', () => {
        const warning = androidNetworkWarning(
            phone({ address: '10.1.2.3/24', onHostNetwork: false }),
            ['192.168.110.0/24', '10.0.1.0/24'],
        );
        expect(warning?.title).toBe('The phone is on a different network');
        expect(warning?.message).toContain('It is on 10.1.2.3/24');
        expect(warning?.message).toContain('(192.168.110.0/24, 10.0.1.0/24)');
    });

    it('still asks for this computer’s Wi-Fi when the computer’s networks are unknown', () => {
        const warning = androidNetworkWarning(phone({ address: null, onHostNetwork: false }), []);
        expect(warning?.message).toContain(
            'Join the Wi-Fi network this computer is on, then refresh devices.',
        );
        expect(warning?.message).not.toContain('(');
    });
});
