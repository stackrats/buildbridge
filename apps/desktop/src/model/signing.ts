// What a signing kit can do, and what it still needs.
//
// Editing a stored kit sends only the fields that changed — a blank box means "keep what is in
// the vault" — so readiness is the union of what is typed now and what is already stored. Keeping
// that rule here means the dialog, the attach step and their tests agree on it.

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

/**
 * How a kit gets a kind of signing done: with files it holds, or with a Team key that has
 * BuildBridge create the certificate and profile at Apple when a machine first needs them.
 */
export type SigningRoute = 'files' | 'team_key';

/**
 * Three things make a kit usable, any one of them together with the guest keychain password: a
 * Team key (App Store Connect API key), the distribution set — identity, export password and at
 * least one profile — which signs an App Store archive, or a development identity with its
 * password, which signs a Debug build for a registered phone and nothing more.
 */
export interface SigningKitReadiness {
    /** Stored distribution identity, its export password and at least one profile. */
    distribution: boolean;
    /** An Android upload key: keystore, key alias and keystore password. */
    android: boolean;
    /** Stored development identity with its export password. */
    development: boolean;
    /** An App Store Connect key: certificates and profiles are created at Apple when needed. */
    teamKey: boolean;
    keychain: boolean;
    provisionable: boolean;
    /** How an App Store archive gets signed, or null when it cannot be. */
    archive: SigningRoute | null;
    /** How a Debug build for a phone gets signed, or null when it cannot be. */
    phone: SigningRoute | null;
}

export function kitReadiness(kit: SigningKitSummary | null | undefined): SigningKitReadiness {
    const distribution =
        !!kit &&
        kit.signingCertificateConfigured &&
        kit.signingCertificatePasswordStored &&
        kit.provisioningProfileNames.length > 0;
    const development =
        !!kit && kit.developmentCertificateConfigured && kit.developmentCertificatePasswordStored;
    const teamKey = !!kit && kit.appStoreConnectConfigured;
    const keychain = !!kit && kit.guestKeychainConfigured;
    const android =
        !!kit &&
        kit.androidKeystoreConfigured &&
        kit.androidKeyAlias !== null &&
        kit.androidKeystorePasswordStored;
    return {
        distribution,
        android,
        development,
        teamKey,
        keychain,
        provisionable: keychain && (teamKey || distribution || development),
        archive: distribution ? 'files' : teamKey ? 'team_key' : null,
        phone: development ? 'files' : teamKey ? 'team_key' : null,
    };
}

export function kitIsProvisionable(kit: SigningKitSummary | null | undefined): boolean {
    return kitReadiness(kit).provisionable;
}

/** Whether a kit can sign an Android release. Its Apple material has no say in it. */
export function kitSignsAndroid(kit: SigningKitSummary | null | undefined): boolean {
    return kitReadiness(kit).android;
}

/** What a stored kit still lacks before it can sign an Android release. */
export function androidKitShortfall(kit: SigningKitSummary): string[] {
    const missing: string[] = [];
    if (!kit.androidKeystoreConfigured) {
        missing.push('an upload keystore');
    }
    if (kit.androidKeystoreConfigured && kit.androidKeyAlias === null) {
        missing.push('the key alias');
    }
    if (kit.androidKeystoreConfigured && !kit.androidKeystorePasswordStored) {
        missing.push('the keystore password');
    }
    return missing;
}

/** What a stored kit still lacks before it can provision, as the attach step lists it. */
export function kitShortfall(kit: SigningKitSummary): string[] {
    const readiness = kitReadiness(kit);
    const missing: string[] = [];
    if (!readiness.teamKey && !readiness.distribution && !readiness.development) {
        if (kit.signingCertificateConfigured) {
            if (!kit.signingCertificatePasswordStored) {
                missing.push('export password');
            }
            if (kit.provisioningProfileNames.length === 0) {
                missing.push('provisioning profile');
            }
        } else if (kit.developmentCertificateConfigured) {
            missing.push('development identity export password');
        } else {
            missing.push('a Team key, or a distribution or development identity');
        }
    }
    if (!readiness.keychain) {
        missing.push('keychain password (saving the credentials again invents one)');
    }
    return missing;
}

/** What the kit dialog holds: every box, typed or blank. */
export interface SigningKitDraft {
    certificatePath: string;
    certificatePassword: string;
    profilePaths: string[];
    keychainPassword: string;
    developmentCertificatePath: string;
    developmentCertificatePassword: string;
    teamKeyPath: string;
    teamKeyId: string;
    teamIssuerId: string;
    androidKeystorePath: string;
    androidKeystorePassword: string;
    androidKeyAlias: string;
    androidKeyPassword: string;
}

export const emptySigningKitDraft: SigningKitDraft = {
    certificatePath: '',
    certificatePassword: '',
    profilePaths: [],
    keychainPassword: '',
    developmentCertificatePath: '',
    developmentCertificatePassword: '',
    teamKeyPath: '',
    teamKeyId: '',
    teamIssuerId: '',
    androidKeystorePath: '',
    androidKeystorePassword: '',
    androidKeyAlias: '',
    androidKeyPassword: '',
};

/**
 * The kit as it would be stored if the dialog saved now: what is typed, else what the vault
 * holds. A half-typed Team key counts as none, since Apple needs all three parts together.
 */
export function draftKitSummary(
    draft: SigningKitDraft,
    stored: SigningKitSummary | null,
): SigningKitSummary {
    const teamKeyTyped =
        draft.teamKeyPath.trim() !== '' &&
        draft.teamKeyId.trim() !== '' &&
        draft.teamIssuerId.trim() !== '';
    return {
        id: stored?.id ?? '',
        name: stored?.name ?? '',
        appStoreConnectConfigured: teamKeyTyped || (stored?.appStoreConnectConfigured ?? false),
        appStoreConnectKeyId: teamKeyTyped
            ? draft.teamKeyId.trim()
            : (stored?.appStoreConnectKeyId ?? null),
        signingCertificateConfigured:
            draft.certificatePath.trim() !== '' || (stored?.signingCertificateConfigured ?? false),
        signingCertificateName: stored?.signingCertificateName ?? null,
        signingCertificatePasswordStored:
            draft.certificatePassword !== '' || (stored?.signingCertificatePasswordStored ?? false),
        provisioningProfileNames:
            draft.profilePaths.length > 0
                ? draft.profilePaths
                : (stored?.provisioningProfileNames ?? []),
        // Saving invents the keychain password when none is typed, so a kit always ends up with
        // one; only a stored kit from before that rule can still lack it.
        guestKeychainConfigured:
            draft.keychainPassword !== '' || stored === null || stored.guestKeychainConfigured,
        createdAtEpochSeconds: stored?.createdAtEpochSeconds ?? 0,
        attachedMachines: stored?.attachedMachines ?? [],
        developmentCertificateConfigured:
            draft.developmentCertificatePath.trim() !== '' ||
            (stored?.developmentCertificateConfigured ?? false),
        developmentCertificateName: stored?.developmentCertificateName ?? null,
        developmentCertificatePasswordStored:
            draft.developmentCertificatePassword !== '' ||
            (stored?.developmentCertificatePasswordStored ?? false),
        androidKeystoreConfigured:
            draft.androidKeystorePath.trim() !== '' || (stored?.androidKeystoreConfigured ?? false),
        androidKeystoreName: stored?.androidKeystoreName ?? null,
        androidKeyAlias: draft.androidKeyAlias.trim() || (stored?.androidKeyAlias ?? null),
        androidKeystorePasswordStored:
            draft.androidKeystorePassword !== '' ||
            (stored?.androidKeystorePasswordStored ?? false),
        androidKeyPasswordStored:
            draft.androidKeyPassword !== '' || (stored?.androidKeyPasswordStored ?? false),
    };
}

export interface SigningKitRequirement {
    id: 'certificate' | 'password' | 'profiles';
    label: string;
    satisfied: boolean;
}

/** The three files the distribution set is made of, and whether each is present. */
export function kitRequirements(
    draft: Pick<SigningKitDraft, 'certificatePath' | 'certificatePassword' | 'profilePaths'>,
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
    ];
}

export function missingKitRequirements(
    draft: Pick<SigningKitDraft, 'certificatePath' | 'certificatePassword' | 'profilePaths'>,
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
