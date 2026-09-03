import { describe, expect, it } from 'vite-plus/test';

import type { SigningKitSummary } from '../types/backend';
import { appStoreConnectIsPartial, kitHoldsProfile, missingKitRequirements } from './signing';

const emptyDraft = {
    certificatePath: '',
    certificatePassword: '',
    profilePaths: [] as string[],
    keychainPassword: '',
};

function storedKit(overrides: Partial<SigningKitSummary> = {}): SigningKitSummary {
    return {
        id: 'team',
        name: 'Example team',
        appStoreConnectConfigured: false,
        appStoreConnectKeyId: null,
        signingCertificateConfigured: false,
        signingCertificateName: null,
        signingCertificatePasswordStored: false,
        provisioningProfileNames: [],
        guestKeychainConfigured: false,
        createdAtEpochSeconds: 0,
        attachedMachines: [],
        ...overrides,
    };
}

describe('kit requirements', () => {
    it('names all four requirements for an empty new kit', () => {
        expect(missingKitRequirements(emptyDraft, null).map((item) => item.id)).toEqual([
            'certificate',
            'password',
            'profiles',
            'keychain',
        ]);
    });

    it('counts what is typed now', () => {
        const draft = {
            certificatePath: '/path/to/dist.p12',
            certificatePassword: 'secret',
            profilePaths: ['/path/to/app.mobileprovision'],
            keychainPassword: 'keychain',
        };

        expect(missingKitRequirements(draft, null)).toEqual([]);
    });

    it('counts what the vault already holds, because a blank box keeps it', () => {
        const stored = storedKit({
            signingCertificateConfigured: true,
            signingCertificatePasswordStored: true,
            provisioningProfileNames: ['app.mobileprovision'],
            guestKeychainConfigured: true,
        });

        expect(missingKitRequirements(emptyDraft, stored)).toEqual([]);
    });

    it('reports only the gap when a stored kit is partly filled', () => {
        const stored = storedKit({
            signingCertificateConfigured: true,
            signingCertificatePasswordStored: true,
        });

        const missing = missingKitRequirements(emptyDraft, stored);

        expect(missing.map((item) => item.id)).toEqual(['profiles', 'keychain']);
        expect(missing[0]?.label).toBe('Provisioning profile');
    });
});

describe('App Store Connect completeness', () => {
    it('accepts all three parts or none', () => {
        expect(appStoreConnectIsPartial({ keyPath: '', keyId: '', issuerId: '' })).toBe(false);
        expect(
            appStoreConnectIsPartial({
                keyPath: '/path/to/AuthKey_KEYID.p8',
                keyId: 'KEYID12345',
                issuerId: 'issuer-uuid',
            }),
        ).toBe(false);
    });

    it('flags a half-filled key before Apple rejects it', () => {
        expect(
            appStoreConnectIsPartial({
                keyPath: '/path/to/AuthKey_KEYID.p8',
                keyId: '',
                issuerId: '',
            }),
        ).toBe(true);
        expect(
            appStoreConnectIsPartial({ keyPath: '', keyId: 'KEYID12345', issuerId: 'issuer-uuid' }),
        ).toBe(true);
    });
});

describe('profile already in the kit', () => {
    const uuid = '11111111-2222-3333-4444-555555555555';

    it('matches a managed copy by its UUID prefix', () => {
        const kit = storedKit({ provisioningProfileNames: [`${uuid}.mobileprovision`] });

        expect(kitHoldsProfile(kit, uuid)).toBe(true);
    });

    it('does not match a different profile, or no kit at all', () => {
        const kit = storedKit({ provisioningProfileNames: [`${uuid}.mobileprovision`] });

        expect(kitHoldsProfile(kit, '99999999-0000-0000-0000-000000000000')).toBe(false);
        expect(kitHoldsProfile(null, uuid)).toBe(false);
        expect(kitHoldsProfile(kit, '')).toBe(false);
    });

    it('does not match a hand-added file named after the app instead of the UUID', () => {
        const kit = storedKit({ provisioningProfileNames: ['AppStore.mobileprovision'] });

        expect(kitHoldsProfile(kit, uuid)).toBe(false);
    });
});
