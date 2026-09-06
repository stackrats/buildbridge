// Browser-only sample credentials. Native exports deliberately require the desktop app.
import type { Backend } from './backend';
import type { SigningCredential, SigningKitInput, SigningKitSummary } from '../types/backend';

export function createCredentialPreview(kits: () => SigningKitSummary[]) {
    const entered = new Map<string, Record<string, string>>();
    const find = (kitId: string) => {
        const kit = kits().find((entry) => entry.id === kitId);
        if (!kit) throw new Error('These signing credentials are no longer stored.');
        return kit;
    };
    const list = (kitId: string): SigningCredential[] => {
        const kit = find(kitId);
        const entries: SigningCredential[] = [];
        const secret = (
            id: string,
            label: string,
            available: boolean,
            fileName: string | null = null,
        ) => {
            if (available)
                entries.push({ id, label, kind: 'secret', value: null, fileName, available });
        };
        const value = (id: string, label: string, text: string | null) => {
            if (text !== null)
                entries.push({
                    id,
                    label,
                    kind: 'value',
                    value: text,
                    fileName: null,
                    available: true,
                });
        };
        const file = (id: string, label: string, fileName: string | null) => {
            if (fileName)
                entries.push({ id, label, kind: 'file', value: null, fileName, available: true });
        };
        value('app_store_connect_key_id', 'App Store Connect key ID', kit.appStoreConnectKeyId);
        if (kit.appStoreConnectConfigured) {
            value(
                'app_store_connect_issuer_id',
                'App Store Connect issuer ID',
                entered.get(kitId)?.app_store_connect_issuer_id ??
                    '11111111-2222-3333-4444-555555555555',
            );
        }
        secret(
            'app_store_connect_private_key',
            'App Store Connect private key',
            kit.appStoreConnectConfigured,
            `AuthKey_${kit.appStoreConnectKeyId}.p8`,
        );
        secret(
            'signing_certificate_password',
            'Distribution export password',
            kit.signingCertificatePasswordStored,
        );
        secret(
            'development_certificate_password',
            'Development export password',
            kit.developmentCertificatePasswordStored,
        );
        secret('guest_keychain_password', 'Guest keychain password', kit.guestKeychainConfigured);
        secret('android_keystore_password', 'Keystore password', kit.androidKeystorePasswordStored);
        secret(
            'android_key_password',
            kit.androidKeyPasswordStored ? 'Key password' : 'Key password (same as keystore)',
            kit.androidKeyPasswordStored || kit.androidKeystorePasswordStored,
        );
        value('android_key_alias', 'Android key alias', kit.androidKeyAlias);
        file('signing_certificate', 'Distribution identity', kit.signingCertificateName);
        file('development_certificate', 'Development identity', kit.developmentCertificateName);
        file('android_keystore', 'Android keystore', kit.androidKeystoreName);
        kit.provisioningProfileNames.forEach((name, index) =>
            file(`provisioning_profile_${index}`, 'Provisioning profile', name),
        );
        return entries;
    };
    const backend: Pick<
        Backend,
        'listSigningCredentials' | 'revealSigningCredential' | 'exportSigningCredential'
    > = {
        async listSigningCredentials(kitId) {
            return list(kitId);
        },
        async revealSigningCredential(kitId, credentialId) {
            const kit = find(kitId);
            const entry = list(kitId).find((item) => item.id === credentialId);
            if (!entry || entry.kind !== 'secret')
                throw new Error('This saved secret is unavailable.');
            if (credentialId === 'app_store_connect_private_key') {
                return '-----BEGIN PRIVATE KEY-----\nBrowser preview only: not a real key\n-----END PRIVATE KEY-----\n';
            }
            const field =
                credentialId === 'android_key_password' && !kit.androidKeyPasswordStored
                    ? 'android_keystore_password'
                    : credentialId;
            return entered.get(kitId)?.[field] ?? 'preview-only-password';
        },
        async exportSigningCredential() {
            throw new Error(
                'Open the desktop app to export credential files. The browser preview has not saved a file.',
            );
        },
    };
    return {
        backend,
        forget(kitId: string) {
            entered.delete(kitId);
        },
        remember(kitId: string, input: Partial<SigningKitInput>) {
            const saved = entered.get(kitId) ?? {};
            const fields = {
                signing_certificate_password: input.signingCertificatePassword,
                development_certificate_password: input.developmentCertificatePassword,
                guest_keychain_password: input.guestKeychainPassword,
                android_keystore_password: input.androidKeystorePassword,
                android_key_password: input.androidKeyPassword,
                app_store_connect_issuer_id: input.appStoreConnectIssuerId,
            };
            for (const [field, text] of Object.entries(fields)) {
                if (text) saved[field] = text;
            }
            entered.set(kitId, saved);
        },
    };
}
