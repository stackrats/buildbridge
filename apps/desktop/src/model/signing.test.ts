import { describe, expect, it } from 'vite-plus/test';

import type { SigningKitSummary } from '../types/backend';
import {
    androidKitShortfall,
    appStoreConnectIsPartial,
    compareAndroidCertificate,
    draftKitSummary,
    emptySigningKitDraft,
    kitHoldsProfile,
    kitReadiness,
    kitSignsAndroid,
    kitShortfall,
    missingKitRequirements,
    signingKitPlatforms,
} from './signing';

const emptyDraft = emptySigningKitDraft;

describe('Android certificate fingerprint comparison', () => {
    const actual = 'abcdef0123456789'.repeat(4);

    it('matches complete fingerprints regardless of case, colon separators, or whitespace', () => {
        expect(compareAndroidCertificate(actual.toUpperCase(), actual)).toBe('match');
        expect(compareAndroidCertificate(actual, actual.toUpperCase())).toBe('match');
        const pairs = actual.toUpperCase().match(/.{2}/g)!;
        expect(compareAndroidCertificate(`  ${pairs.join(':')}  `, actual)).toBe('match');
        expect(compareAndroidCertificate(pairs.join(' \n\t'), actual)).toBe('match');
    });

    it('rejects prefixes, arbitrary punctuation, non-hex characters, and incomplete hashes', () => {
        for (const expected of [
            `SHA-256: ${actual}`,
            `0x${actual}`,
            `${actual.slice(0, 32)}-${actual.slice(32)}`,
            `${actual}!`,
            `g${actual.slice(1)}`,
            actual.slice(0, 63),
            `${actual}0`,
            ': :',
        ]) {
            expect(compareAndroidCertificate(expected, actual)).toBe('invalid');
        }
    });

    it('keeps missing or malformed checked fingerprints unverified', () => {
        for (const unchecked of [undefined, '', 'not a hash', actual.slice(0, 63)]) {
            expect(compareAndroidCertificate(actual, unchecked)).toBe('unchecked');
        }
    });

    it('distinguishes an empty comparison from a valid different certificate', () => {
        expect(compareAndroidCertificate(' \n\t', actual)).toBe('empty');
        expect(compareAndroidCertificate('0'.repeat(64), actual)).toBe('mismatch');
    });
});

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
        developmentCertificateConfigured: false,
        developmentCertificateName: null,
        developmentCertificatePasswordStored: false,
        androidKeystoreConfigured: false,
        androidKeystoreName: null,
        androidKeyAlias: null,
        androidKeystorePasswordStored: false,
        androidKeyPasswordStored: false,
        ...overrides,
    };
}

describe('signing credential platforms', () => {
    it('does not classify names or the automatically created guest password as signing material', () => {
        expect(
            signingKitPlatforms(
                storedKit({ name: 'Android and iOS', guestKeychainConfigured: true }),
            ),
        ).toEqual([]);
    });

    it('includes incomplete Apple credentials before they are ready to provision', () => {
        expect(signingKitPlatforms(storedKit({ signingCertificateConfigured: true }))).toEqual([
            'ios',
        ]);
        expect(
            signingKitPlatforms(storedKit({ provisioningProfileNames: ['app.mobileprovision'] })),
        ).toEqual(['ios']);
    });

    it('keeps Android credentials discoverable while the upload-key setup is incomplete', () => {
        expect(signingKitPlatforms(storedKit({ androidKeystoreConfigured: true }))).toEqual([
            'android',
        ]);
        expect(signingKitPlatforms(storedKit({ androidKeyAlias: 'upload' }))).toEqual(['android']);
    });

    it('identifies shared credentials from the material held for both platforms', () => {
        expect(
            signingKitPlatforms(
                storedKit({ appStoreConnectConfigured: true, androidKeystoreConfigured: true }),
            ),
        ).toEqual(['ios', 'android']);
    });
});

describe('distribution files', () => {
    it('names all three files for an empty new kit', () => {
        expect(missingKitRequirements(emptyDraft, null).map((item) => item.id)).toEqual([
            'certificate',
            'password',
            'profiles',
        ]);
    });

    it('counts what is typed now', () => {
        const draft = {
            certificatePath: '/path/to/dist.p12',
            certificatePassword: 'secret',
            profilePaths: ['/path/to/app.mobileprovision'],
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

        expect(missing.map((item) => item.id)).toEqual(['profiles']);
        expect(missing[0]?.label).toBe('Provisioning profile');
    });
});

describe('the kit a dialog would store', () => {
    it('is what is typed, else what the vault holds', () => {
        const stored = storedKit({
            signingCertificateConfigured: true,
            signingCertificatePasswordStored: true,
            provisioningProfileNames: ['stored.mobileprovision'],
        });
        const summary = draftKitSummary(
            {
                ...emptyDraft,
                keychainPassword: 'keychain',
                profilePaths: ['/typed.mobileprovision'],
            },
            stored,
        );

        expect(summary.signingCertificateConfigured).toBe(true);
        expect(summary.guestKeychainConfigured).toBe(true);
        expect(summary.provisioningProfileNames).toEqual(['/typed.mobileprovision']);
        expect(kitReadiness(summary).provisionable).toBe(true);
    });

    it('counts a Team key only when all three parts are typed', () => {
        const partial = draftKitSummary(
            { ...emptyDraft, teamKeyPath: '/AuthKey.p8', teamKeyId: 'KEYID12345' },
            null,
        );
        const whole = draftKitSummary(
            {
                ...emptyDraft,
                teamKeyPath: '/AuthKey.p8',
                teamKeyId: 'KEYID12345',
                teamIssuerId: 'issuer',
            },
            null,
        );

        expect(partial.appStoreConnectConfigured).toBe(false);
        expect(whole.appStoreConnectConfigured).toBe(true);
        expect(whole.appStoreConnectKeyId).toBe('KEYID12345');
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

describe('kit readiness', () => {
    const distribution = {
        signingCertificateConfigured: true,
        signingCertificatePasswordStored: true,
        provisioningProfileNames: ['11111111-2222-3333-4444-555555555555.mobileprovision'],
        guestKeychainConfigured: true,
    };
    const development = {
        developmentCertificateConfigured: true,
        developmentCertificatePasswordStored: true,
        guestKeychainConfigured: true,
    };

    const teamKey = {
        appStoreConnectConfigured: true,
        appStoreConnectKeyId: 'KEYID12345',
        guestKeychainConfigured: true,
    };

    it('provisions with the distribution set, the development identity, or both', () => {
        expect(kitReadiness(storedKit(distribution))).toEqual({
            distribution: true,
            android: false,
            development: false,
            teamKey: false,
            keychain: true,
            provisionable: true,
            archive: 'files',
            phone: null,
        });
        expect(kitReadiness(storedKit(development))).toEqual({
            distribution: false,
            android: false,
            development: true,
            teamKey: false,
            keychain: true,
            provisionable: true,
            archive: null,
            phone: 'files',
        });
        expect(kitReadiness(storedKit({ ...distribution, ...development })).provisionable).toBe(
            true,
        );
    });

    it('provisions with a Team key alone, creating certificates and profiles on demand', () => {
        expect(kitReadiness(storedKit(teamKey))).toEqual({
            distribution: false,
            android: false,
            development: false,
            teamKey: true,
            keychain: true,
            provisionable: true,
            archive: 'team_key',
            phone: 'team_key',
        });
    });

    it('prefers stored files over the Team key for whichever route has them', () => {
        const readiness = kitReadiness(storedKit({ ...teamKey, ...distribution }));

        expect(readiness.archive).toBe('files');
        expect(readiness.phone).toBe('team_key');
    });

    it('always needs the keychain password, and a .p12 without its password is not an identity', () => {
        expect(
            kitReadiness(storedKit({ ...development, guestKeychainConfigured: false }))
                .provisionable,
        ).toBe(false);
        expect(
            kitReadiness(storedKit({ ...development, developmentCertificatePasswordStored: false }))
                .provisionable,
        ).toBe(false);
        expect(kitReadiness(null).provisionable).toBe(false);
    });

    it('lists the shortest route to provisioning', () => {
        expect(kitShortfall(storedKit())).toEqual([
            'a Team key, or a distribution or development identity',
            'keychain password (saving the credentials again invents one)',
        ]);
        expect(kitShortfall(storedKit({ appStoreConnectConfigured: true }))).toEqual([
            'keychain password (saving the credentials again invents one)',
        ]);
        expect(
            kitShortfall(
                storedKit({ signingCertificateConfigured: true, guestKeychainConfigured: true }),
            ),
        ).toEqual(['export password', 'provisioning profile']);
        expect(
            kitShortfall(
                storedKit({
                    developmentCertificateConfigured: true,
                    guestKeychainConfigured: true,
                }),
            ),
        ).toEqual(['development identity export password']);
        expect(kitShortfall(storedKit(development))).toEqual([]);
    });
});

describe('the Android upload key', () => {
    const uploadKey = {
        androidKeystoreConfigured: true,
        androidKeystoreName: 'upload.keystore',
        androidKeyAlias: 'upload',
        androidKeystorePasswordStored: true,
    };

    it('signs a release with a keystore, an alias and the keystore password', () => {
        expect(kitSignsAndroid(storedKit(uploadKey))).toBe(true);
        expect(kitReadiness(storedKit(uploadKey)).android).toBe(true);
        // The key password defaults to the keystore's; it is never required.
        expect(kitSignsAndroid(storedKit({ ...uploadKey, androidKeyPasswordStored: true }))).toBe(
            true,
        );
    });

    it('has no say in the Apple side of the kit and the reverse', () => {
        expect(kitReadiness(storedKit(uploadKey)).provisionable).toBe(false);
        expect(kitSignsAndroid(storedKit({ appStoreConnectConfigured: true }))).toBe(false);
        expect(kitSignsAndroid(null)).toBe(false);
    });

    it('names what is missing, in the order it is asked for', () => {
        expect(androidKitShortfall(storedKit())).toEqual(['an upload keystore']);
        expect(androidKitShortfall(storedKit({ ...uploadKey, androidKeyAlias: null }))).toEqual([
            'the key alias',
        ]);
        expect(
            androidKitShortfall(storedKit({ ...uploadKey, androidKeystorePasswordStored: false })),
        ).toEqual(['the keystore password']);
        expect(androidKitShortfall(storedKit(uploadKey))).toEqual([]);
    });

    it('counts a typed keystore path and password as stored once saved', () => {
        const draft = {
            ...emptyDraft,
            androidKeystorePath: '/keys/upload.keystore',
            androidKeystorePassword: 'secret-1',
            androidKeyAlias: 'release',
        };
        const summary = draftKitSummary(draft, null);
        expect(summary.androidKeystoreConfigured).toBe(true);
        expect(summary.androidKeyAlias).toBe('release');
        expect(summary.androidKeystorePasswordStored).toBe(true);
        expect(kitSignsAndroid(summary)).toBe(true);
        // A blank draft keeps what the vault holds.
        expect(kitSignsAndroid(draftKitSummary(emptyDraft, storedKit(uploadKey)))).toBe(true);
    });
});
