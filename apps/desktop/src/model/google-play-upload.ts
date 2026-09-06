import type { GooglePlayUploadResult, MachineView } from '../types/backend';

export interface GooglePlayUpload {
    sha256: string;
    status: 'uploading' | 'uploaded' | 'failed';
    result: GooglePlayUploadResult | null;
    error: string | null;
}

/** Uploading a retained AAB needs credentials and host network access, not a running container. */
export function googlePlayUploadBlocker(
    view: MachineView | null,
    configured: boolean,
    busy: boolean,
): string | null {
    if (!view || view.profile.provider !== 'android_toolchain')
        return 'Choose an Android machine first.';
    if (busy || view.busyOperation) return 'Wait for the current operation to finish.';
    if (!view.android?.release?.aab) return 'Build a signed AAB before uploading to Google Play.';
    if (!configured)
        return 'Import a Google Play service account JSON key with access to this app.';
    return null;
}
