import type {
    AndroidArtifact,
    AndroidDevice,
    AndroidDeviceRunResult,
    AndroidMachineView,
} from '../types/backend';

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
            label: 'Debug APK',
            artifact: debug.apk,
            applicationId: debug.applicationId,
            version: `${debug.versionName} (${debug.versionCode})`,
            environment: null,
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
        });
    return apks;
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
