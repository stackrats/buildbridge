import type { AndroidArtifact, AndroidDeviceRunResult, AndroidMachineView } from '../types/backend';

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
