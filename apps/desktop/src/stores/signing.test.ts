import { beforeEach, describe, expect, it, vi } from 'vite-plus/test';

import type {
    AndroidSigningVerification,
    SigningKitInput,
    SigningKitSummary,
} from '../types/backend';

const backend = vi.hoisted(() => ({
    verifyAndroidSigningKit: vi.fn<(kitId: string) => Promise<AndroidSigningVerification>>(),
    saveSigningKit: vi.fn<(input: SigningKitInput) => Promise<SigningKitSummary[]>>(),
    deleteSigningKit: vi.fn<(kitId: string) => Promise<SigningKitSummary[]>>(),
}));

vi.mock('../lib/backend', () => ({ useBackend: () => backend }));

function deferred<T>() {
    let resolve!: (value: T) => void;
    let reject!: (reason: Error) => void;
    const promise = new Promise<T>((accept, refuse) => {
        resolve = accept;
        reject = refuse;
    });
    return { promise, resolve, reject };
}

function verification(kitId: string): AndroidSigningVerification {
    return {
        kitId,
        keyAlias: 'upload',
        certificateSha256: 'ab'.repeat(32),
        certificateSha1: 'cd'.repeat(20),
        algorithm: 'RSA',
        keyBits: 2048,
        validFromEpochSeconds: 1_700_000_000,
        validUntilEpochSeconds: 2_000_000_000,
        verifiedAtEpochSeconds: 1_800_000_000,
    };
}

function savedInput(kitId: string): SigningKitInput {
    return {
        kitId,
        name: 'Upload key',
        appStoreConnectKeyId: '',
        appStoreConnectIssuerId: '',
        appStoreConnectPrivateKeyPath: '',
        signingCertificatePath: '',
        signingCertificatePassword: '',
        provisioningProfilePaths: [],
        guestKeychainPassword: '',
        developmentCertificatePath: '',
        developmentCertificatePassword: '',
        androidKeystorePath: '',
        androidKeystorePassword: '',
        androidKeyAlias: '',
        androidKeyPassword: '',
    };
}

describe('Android signing verification state', () => {
    let store: ReturnType<typeof import('./signing').useSigningStore>;

    beforeEach(async () => {
        vi.resetModules();
        vi.resetAllMocks();
        backend.saveSigningKit.mockResolvedValue([]);
        backend.deleteSigningKit.mockResolvedValue([]);
        store = (await import('./signing')).useSigningStore();
    });

    it('deduplicates checks for the same kit until the pending check finishes', async () => {
        const check = deferred<AndroidSigningVerification>();
        backend.verifyAndroidSigningKit.mockReturnValueOnce(check.promise);

        const first = store.verifyAndroid('team-a');
        await store.verifyAndroid('team-a');

        expect(backend.verifyAndroidSigningKit).toHaveBeenCalledExactlyOnceWith('team-a');
        expect(store.state.androidVerifying['team-a']).toBe(true);
        check.resolve(verification('team-a'));
        await first;
        expect(store.state.androidVerifying['team-a']).toBeUndefined();
        expect(store.state.androidVerification['team-a']).toEqual(verification('team-a'));
    });

    it('keeps concurrent kit results and errors separate from each other and Apple errors', async () => {
        const first = deferred<AndroidSigningVerification>();
        const second = deferred<AndroidSigningVerification>();
        backend.verifyAndroidSigningKit
            .mockReturnValueOnce(first.promise)
            .mockReturnValueOnce(second.promise);
        store.state.error = 'An unrelated operation failed.';
        store.state.verificationError = 'Apple needs a new Team key.';

        const checkingFirst = store.verifyAndroid('team-a');
        const checkingSecond = store.verifyAndroid('team-b');
        second.reject(new Error('The upload-key password is incorrect.'));
        await checkingSecond;
        expect(store.state.androidVerificationErrors['team-b']).toBe(
            'The upload-key password is incorrect.',
        );
        expect(store.state.androidVerifying['team-a']).toBe(true);
        expect(store.state.androidVerifying['team-b']).toBeUndefined();

        first.resolve(verification('team-a'));
        await checkingFirst;
        expect(store.state.androidVerification['team-a']).toEqual(verification('team-a'));
        expect(store.state.androidVerificationErrors['team-a']).toBeUndefined();
        expect(store.state.androidVerification['team-b']).toBeUndefined();
        expect(store.state.androidVerificationErrors['team-b']).toBe(
            'The upload-key password is incorrect.',
        );
        expect(store.state.error).toBe('An unrelated operation failed.');
        expect(store.state.verificationError).toBe('Apple needs a new Team key.');
    });

    it('clears the previous error or result as soon as a fresh check starts', async () => {
        backend.verifyAndroidSigningKit.mockRejectedValueOnce(new Error('Wrong password.'));
        await store.verifyAndroid('team-a');
        expect(store.state.androidVerificationErrors['team-a']).toBe('Wrong password.');

        const retry = deferred<AndroidSigningVerification>();
        backend.verifyAndroidSigningKit.mockReturnValueOnce(retry.promise);
        const retrying = store.verifyAndroid('team-a');
        expect(store.state.androidVerificationErrors['team-a']).toBeUndefined();
        retry.resolve(verification('team-a'));
        await retrying;

        const next = deferred<AndroidSigningVerification>();
        backend.verifyAndroidSigningKit.mockReturnValueOnce(next.promise);
        const checkingAgain = store.verifyAndroid('team-a');
        expect(store.state.androidVerification['team-a']).toBeUndefined();
        next.resolve(verification('team-a'));
        await checkingAgain;
    });

    for (const mutation of ['save', 'remove'] as const) {
        function mutate(target: typeof store) {
            return mutation === 'save'
                ? target.save(savedInput('team-a'))
                : target.remove('team-a');
        }

        for (const outcome of ['success', 'error'] as const) {
            it(`${mutation} immediately clears the kit's cached ${outcome} and preserves other kits`, async () => {
                backend.verifyAndroidSigningKit.mockResolvedValueOnce(verification('team-b'));
                await store.verifyAndroid('team-b');
                if (outcome === 'success') {
                    backend.verifyAndroidSigningKit.mockResolvedValueOnce(verification('team-a'));
                } else {
                    backend.verifyAndroidSigningKit.mockRejectedValueOnce(new Error('Old error.'));
                }
                await store.verifyAndroid('team-a');

                const change = deferred<SigningKitSummary[]>();
                const method =
                    mutation === 'save' ? backend.saveSigningKit : backend.deleteSigningKit;
                method.mockReturnValueOnce(change.promise);
                const changing = mutate(store);
                expect(store.state.androidVerification['team-a']).toBeUndefined();
                expect(store.state.androidVerificationErrors['team-a']).toBeUndefined();
                expect(store.state.androidVerification['team-b']).toEqual(verification('team-b'));
                change.resolve([]);
                expect(await changing).toBe(true);
            });

            it(`${mutation} discards a stale ${outcome} without ending a newer check`, async () => {
                const stale = deferred<AndroidSigningVerification>();
                backend.verifyAndroidSigningKit.mockReturnValueOnce(stale.promise);
                const checkingOld = store.verifyAndroid('team-a');
                const changing = mutate(store);
                expect(store.state.androidVerifying['team-a']).toBeUndefined();
                expect(await changing).toBe(true);

                const fresh = deferred<AndroidSigningVerification>();
                backend.verifyAndroidSigningKit.mockReturnValueOnce(fresh.promise);
                const checkingNew = store.verifyAndroid('team-a');
                if (outcome === 'success') stale.resolve(verification('team-a'));
                else stale.reject(new Error('Stale failure.'));
                await checkingOld;
                expect(store.state.androidVerifying['team-a']).toBe(true);
                expect(store.state.androidVerification['team-a']).toBeUndefined();
                expect(store.state.androidVerificationErrors['team-a']).toBeUndefined();

                const updated = { ...verification('team-a'), certificateSha256: 'ef'.repeat(32) };
                fresh.resolve(updated);
                await checkingNew;
                expect(store.state.androidVerification['team-a']).toEqual(updated);
                expect(store.state.androidVerifying['team-a']).toBeUndefined();
            });
        }

        it(`${mutation} also invalidates a check started while the mutation was pending`, async () => {
            const change = deferred<SigningKitSummary[]>();
            const method = mutation === 'save' ? backend.saveSigningKit : backend.deleteSigningKit;
            method.mockReturnValueOnce(change.promise);
            const changing = mutate(store);
            const check = deferred<AndroidSigningVerification>();
            backend.verifyAndroidSigningKit.mockReturnValueOnce(check.promise);
            const checking = store.verifyAndroid('team-a');

            change.resolve([]);
            expect(await changing).toBe(true);
            check.resolve(verification('team-a'));
            await checking;
            expect(store.state.androidVerification['team-a']).toBeUndefined();
            expect(store.state.androidVerificationErrors['team-a']).toBeUndefined();
            expect(store.state.androidVerifying['team-a']).toBeUndefined();
        });
    }

    it('rejects a result for different credentials instead of displaying it for either kit', async () => {
        backend.verifyAndroidSigningKit.mockResolvedValueOnce(verification('team-b'));
        await store.verifyAndroid('team-a');

        expect(store.state.androidVerification['team-a']).toBeUndefined();
        expect(store.state.androidVerification['team-b']).toBeUndefined();
        expect(store.state.androidVerificationErrors['team-a']).toBe(
            'The signing check returned different credentials. Check again.',
        );
        expect(store.state.androidVerificationErrors['team-b']).toBeUndefined();
        expect(store.state.androidVerifying['team-a']).toBeUndefined();
    });
});
