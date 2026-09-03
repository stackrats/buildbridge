// What a signing kit still needs before it can provision.
//
// Editing a stored kit sends only the fields that changed — a blank box means "keep what is in
// the vault" — so readiness is the union of what is typed now and what is already stored. Keeping
// that rule here means the dialog and its tests agree on it.

import type { ProfileKind, SigningKitSummary } from '../types/backend';

/** What a profile is for, in the words the interface uses. */
export const profileKindLabel: Record<ProfileKind, string> = {
    app_store: 'App Store',
    development: 'development',
    ad_hoc: 'ad hoc',
    enterprise: 'enterprise',
};

/**
 * The optional development identity: stored already, typed now, half-typed (a mistake worth
 * naming, since a `.p12` is useless without its passphrase), or absent.
 */
export function developmentIdentityStatus(
    draft: { developmentCertificatePath: string; developmentCertificatePassword: string },
    stored: SigningKitSummary | null,
): 'stored' | 'typed' | 'partial' | 'none' {
    const path = draft.developmentCertificatePath.trim() !== '';
    const password = draft.developmentCertificatePassword !== '';
    const storedPath = stored?.developmentCertificateConfigured ?? false;
    const storedPassword = stored?.developmentCertificatePasswordStored ?? false;
    if ((path || storedPath) && (password || storedPassword)) {
        return path || password ? 'typed' : 'stored';
    }
    if (path || password) {
        return 'partial';
    }
    return 'none';
}

export interface SigningKitDraft {
    certificatePath: string;
    certificatePassword: string;
    profilePaths: string[];
    keychainPassword: string;
}

export interface SigningKitRequirement {
    id: 'certificate' | 'password' | 'profiles' | 'keychain';
    label: string;
    satisfied: boolean;
}

/** The four things provisioning cannot do without, and whether each is present. */
export function kitRequirements(
    draft: SigningKitDraft,
    stored: SigningKitSummary | null,
): SigningKitRequirement[] {
    return [
        {
            id: 'certificate',
            label: 'Distribution identity',
            satisfied:
                draft.certificatePath.trim() !== '' ||
                (stored?.signingCertificateConfigured ?? false),
        },
        {
            id: 'password',
            label: 'Export password',
            satisfied:
                draft.certificatePassword !== '' ||
                (stored?.signingCertificatePasswordStored ?? false),
        },
        {
            id: 'profiles',
            label: 'Provisioning profile',
            satisfied:
                draft.profilePaths.length > 0 || (stored?.provisioningProfileNames.length ?? 0) > 0,
        },
        {
            id: 'keychain',
            label: 'Guest keychain password',
            satisfied: draft.keychainPassword !== '' || (stored?.guestKeychainConfigured ?? false),
        },
    ];
}

export function missingKitRequirements(
    draft: SigningKitDraft,
    stored: SigningKitSummary | null,
): SigningKitRequirement[] {
    return kitRequirements(draft, stored).filter((requirement) => !requirement.satisfied);
}

/**
 * Whether a kit already holds a copy of an Apple profile. Managed copies are named by UUID, so
 * the UUID prefix is what identifies them — the name Apple shows can change without the file
 * changing.
 */
export function kitHoldsProfile(kit: SigningKitSummary | null, uuid: string): boolean {
    if (!kit || uuid.trim() === '') {
        return false;
    }

    return kit.provisioningProfileNames.some((name) => name.startsWith(uuid));
}

/**
 * An App Store Connect key is all three parts or none: Apple needs the key, its ID and the
 * issuer together, so a half-filled set is a mistake worth naming before saving.
 */
export function appStoreConnectIsPartial(draft: {
    keyPath: string;
    keyId: string;
    issuerId: string;
}): boolean {
    const parts = [draft.keyPath.trim(), draft.keyId.trim(), draft.issuerId.trim()];
    const filled = parts.filter((part) => part !== '').length;

    return filled > 0 && filled < parts.length;
}
