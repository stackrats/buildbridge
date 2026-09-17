import type {
    AndroidArtifact,
    AndroidDevice,
    AndroidDeviceRunResult,
    AndroidMachineView,
} from '../types/backend';
import { liveReloadUrlIssue } from './live-reload';

export interface AndroidDeviceRun {
    kind: 'debug' | 'release';
    serial: string;
    sha256: string;
    status: 'installing' | 'complete' | 'failed';
    result: AndroidDeviceRunResult | null;
    error: string | null;
}

export interface AndroidDeviceApk {
    value: 'debug' | 'release';
    label: string;
    artifact: AndroidArtifact;
    applicationId: string;
    version: string;
    environment: string | null;
    liveReloadUrl: string | null;
}

/** Only an actual retained APK can be installed; a retained AAB is not sufficient. */
export function androidDeviceApks(
    android: AndroidMachineView | null | undefined,
): AndroidDeviceApk[] {
    const apks: AndroidDeviceApk[] = [];
    const debug = android?.workspace?.lastBuild;
    if (debug?.apk)
        apks.push({
            value: 'debug',
            label: debug.liveReloadUrl ? 'Live reload debug APK' : 'Debug APK',
            artifact: debug.apk,
            applicationId: debug.applicationId,
            version: `${debug.versionName} (${debug.versionCode})`,
            environment: null,
            liveReloadUrl: debug.liveReloadUrl ?? null,
        });
    const release = android?.release;
    if (release?.apk)
        apks.push({
            value: 'release',
            label: 'Retained release APK',
            artifact: release.apk,
            applicationId: release.applicationId,
            version: `${release.versionName} (${release.versionCode})`,
            environment: android?.releaseEnvSet ?? null,
            liveReloadUrl: null,
        });
    return apks;
}

/** Early feedback; the engine validates the URL again before building or connecting. */
export function androidLiveReloadUrlIssue(value: string): string | null {
    return liveReloadUrlIssue(value, 'android');
}

export function androidLiveReloadUsesLocalhost(value: string): boolean {
    try {
        const host = new URL(value).hostname;
        return host === 'localhost' || /^127\.\d+\.\d+\.\d+$/.test(host);
    } catch {
        return false;
    }
}

export function androidRunStopDescription(liveReloadUrl: string | null): string {
    return liveReloadUrl && androidLiveReloadUsesLocalhost(liveReloadUrl)
        ? 'Ends the log session and removes the dev server connection created by buildbridge. The APK stays installed; live reload needs an active dev server connection.'
        : 'Ends the log session. The app stays installed and running on the device.';
}

/** Displayed for the host's terminal, never executed by the desktop. */
export function androidInstallCommand(path: string, serial = ''): string {
    const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
    const target = serial.trim();
    return `adb${target ? ` -s ${quote(target)}` : ''} install -r ${quote(path)}`;
}

/**
 * Why the selected phone would get no response from an API served on this computer, or null
 * when it shares a network with it or was not asked: an emulator, or a device that is not
 * ready. Said before a run, so the app's Network Error is not the first sign.
 */
export function androidNetworkWarning(
    device: AndroidDevice | null | undefined,
    hostNetworks: string[],
): { title: string; message: string } | null {
    const network = device?.network;
    if (!network || network.onHostNetwork) return null;
    const join = hostNetworks.length
        ? `Join the Wi-Fi network this computer is on (${hostNetworks.join(', ')}), then refresh devices.`
        : 'Join the Wi-Fi network this computer is on, then refresh devices.';
    return network.address
        ? {
              title: 'The phone is on a different network',
              message: `It is on ${network.address}, which does not reach this computer, so an app that calls an API served here gets no response. ${join}`,
          }
        : {
              title: 'The phone is not on Wi-Fi',
              message: `Mobile data is all it has, and that does not reach this computer, so an app that calls an API served here gets no response. ${join}`,
          };
}
