import type { MachineView } from '../types/backend';
import { isAndroid } from './providers';

export interface AppleUploadBlocker {
    message: string;
    action: 'start' | 'credentials' | 'guest' | null;
}

/** Upload uses the retained IPA and a Team API key; it does not need signing provisioned again. */
export function appleUploadBlocker(
    view: MachineView | null,
    operationRunning = false,
): AppleUploadBlocker | null {
    if (operationRunning || view?.busyOperation) {
        return { message: 'Wait for the current machine operation to finish.', action: null };
    }
    if (!view || isAndroid(view.profile.provider) || !view.archive?.ipa) {
        return { message: 'Build and retain an App Store IPA first.', action: null };
    }
    if (view.vaultIssue) {
        return { message: view.vaultIssue, action: 'credentials' };
    }
    if (!view.signingKit?.appStoreConnectConfigured) {
        return {
            message:
                'Attach signing credentials with an App Store Connect Team API key, Key ID and Issuer ID to upload.',
            action: 'credentials',
        };
    }
    if (view.runtime.state !== 'running') {
        return { message: 'Start this macOS machine to upload with Transporter.', action: 'start' };
    }
    if (
        !view.guest.ssh.reachable ||
        view.guest.ssh.trust !== 'trusted' ||
        !view.guest.diagnostics.authenticated
    ) {
        return {
            message: 'Connect to macOS, trust its identity and authorize guest access to upload.',
            action: 'guest',
        };
    }
    return null;
}

/** A result belongs to one IPA for this app session, never to every build of the machine. */
export interface AppleArchiveUpload {
    sha256: string;
    status: 'uploading' | 'uploaded' | 'failed';
    error: string | null;
}
