// Signing kits: named sets of Apple material held once in this host's operating-system vault.
//
// A kit is host-level so the files and passwords are entered once. Each machine is attached to
// one kit, and provisioning imports that kit's identity into that machine's guest keychain — so
// the same kit can serve several machines, and one host can hold a kit per developer team.

import { computed, reactive } from 'vue';

import { useBackend } from '../lib/backend';
import { describeError } from '../lib/utils';
import type {
    AppleTeamVerification,
    CreateAppleProfileResult,
    SigningKitInput,
    SigningKitSummary,
} from '../types/backend';

const state = reactive({
    kits: [] as SigningKitSummary[],
    loaded: false,
    loading: false,
    saving: false,
    deleting: false,
    error: null as string | null,
    notice: null as string | null,
    /**
     * The kit an error or notice is about, when it is about one. The page shows such a message
     * inside that kit's card, where the button that caused it is, rather than at the top of the
     * page where a refusal from Apple went unnoticed.
     */
    messageKitId: null as string | null,
    verification: null as AppleTeamVerification | null,
    verificationMachineId: null as string | null,
    verifying: false,
    verificationError: null as string | null,
    creatingProfile: false,
    creatingCertificateKitId: null as string | null,
    creatingDevelopmentCertificateKitId: null as string | null,
    creatingKeystoreKitId: null as string | null,
    downloadingProfileId: null as string | null,
    createdProfile: null as CreateAppleProfileResult | null,
    profileError: null as string | null,
});

/** A kit can provision only when it holds an identity, a profile, and a keychain password. */
export { kitIsProvisionable } from '../model/signing';

export function useSigningStore() {
    return {
        state,
        kits: computed(() => state.kits),
        kitById: (id: string | null | undefined) =>
            id ? (state.kits.find((kit) => kit.id === id) ?? null) : null,

        async load(): Promise<void> {
            state.loading = !state.loaded;
            try {
                state.kits = await useBackend().listSigningKits();
                state.loaded = true;
                state.error = null;
            } catch (error) {
                state.error = describeError(error);
            } finally {
                state.loading = false;
            }
        },

        async save(input: SigningKitInput): Promise<boolean> {
            state.saving = true;
            state.error = null;
            state.notice = null;
            state.messageKitId = null;
            try {
                state.kits = await useBackend().saveSigningKit(input);
                state.notice = input.kitId
                    ? 'Signing credentials updated. Secret values are not shown again.'
                    : 'Signing credentials stored in the operating-system vault.';
                state.verification = null;
                return true;
            } catch (error) {
                state.error = describeError(error);
                return false;
            } finally {
                state.saving = false;
            }
        },

        async remove(kitId: string): Promise<boolean> {
            state.deleting = true;
            state.error = null;
            state.notice = null;
            state.messageKitId = null;
            try {
                state.kits = await useBackend().deleteSigningKit(kitId);
                state.verification = null;
                state.createdProfile = null;
                state.notice =
                    'Signing credentials removed from the vault. Machines using them are now unattached.';
                return true;
            } catch (error) {
                state.error = describeError(error);
                return false;
            } finally {
                state.deleting = false;
            }
        },

        async verify(machineId: string): Promise<void> {
            state.verifying = true;
            state.verificationError = null;
            state.profileError = null;
            try {
                state.verification = await useBackend().verifyAppleTeam(machineId);
                state.verificationMachineId = machineId;
            } catch (error) {
                state.verificationError = describeError(error);
            } finally {
                state.verifying = false;
            }
        },

        async createReplacementProfile(machineId: string, certificateId: string): Promise<boolean> {
            state.creatingProfile = true;
            state.profileError = null;
            try {
                const result = await useBackend().createAppleProfile(machineId, certificateId);
                state.createdProfile = result;
                const index = state.kits.findIndex((kit) => kit.id === result.kit.id);
                if (index >= 0) {
                    state.kits[index] = {
                        ...result.kit,
                        attachedMachines: state.kits[index].attachedMachines,
                    };
                }
                if (state.verification) {
                    state.verification.profiles = [result.profile, ...state.verification.profiles];
                }
                return true;
            } catch (error) {
                state.profileError = describeError(error);
                return false;
            } finally {
                state.creatingProfile = false;
            }
        },

        /**
         * Creates an Apple Distribution identity for a kit with no Mac: the key is generated on
         * this host, Apple signs it through the kit's Team key, and the .p12 lands in the kit.
         */
        async createCertificate(kitId: string): Promise<boolean> {
            state.creatingCertificateKitId = kitId;
            state.error = null;
            state.notice = null;
            state.messageKitId = kitId;
            try {
                const result = await useBackend().createAppleCertificate(kitId);
                const index = state.kits.findIndex((kit) => kit.id === result.kit.id);
                if (index >= 0) {
                    state.kits[index] = {
                        ...result.kit,
                        attachedMachines: state.kits[index].attachedMachines,
                    };
                }
                state.notice = `${result.certificate.name} created at Apple and stored in ${result.kit.name}. Next, create or download an App Store profile for it.`;
                return true;
            } catch (error) {
                state.error = describeError(error);
                return false;
            } finally {
                state.creatingCertificateKitId = null;
            }
        },

        /**
         * The development counterpart: an Apple Development identity for a key generated on
         * this host, packaged into the kit next to the distribution one.
         */
        async createDevelopmentCertificate(kitId: string): Promise<boolean> {
            state.creatingDevelopmentCertificateKitId = kitId;
            state.error = null;
            state.notice = null;
            state.messageKitId = kitId;
            try {
                const result = await useBackend().createAppleDevelopmentCertificate(kitId);
                const index = state.kits.findIndex((kit) => kit.id === result.kit.id);
                if (index >= 0) {
                    state.kits[index] = {
                        ...result.kit,
                        attachedMachines: state.kits[index].attachedMachines,
                    };
                }
                state.notice = `${result.certificate.name} created at Apple and stored in ${result.kit.name}. Prepare an iPhone from a machine's Run on the device step to use it.`;
                return true;
            } catch (error) {
                state.error = describeError(error);
                return false;
            } finally {
                state.creatingDevelopmentCertificateKitId = null;
            }
        },

        /**
         * Creates an Android upload key for a kit in a throwaway container of the toolchain
         * image. The password is the person's: nothing can recover an upload key without it.
         */
        async createKeystore(
            kitId: string,
            input: { password: string; keyAlias: string; certificateName: string },
        ): Promise<boolean> {
            state.creatingKeystoreKitId = kitId;
            state.error = null;
            state.notice = null;
            state.messageKitId = kitId;
            try {
                const result = await useBackend().createAndroidKeystore(kitId, input);
                const index = state.kits.findIndex((kit) => kit.id === result.kit.id);
                if (index >= 0) {
                    state.kits[index] = {
                        ...result.kit,
                        attachedMachines: state.kits[index].attachedMachines,
                    };
                }
                state.notice = `Upload key ${result.keystore.keyAlias} created and stored in ${result.kit.name}. Keep the password: neither Google Play nor BuildBridge can recover it.`;
                return true;
            } catch (error) {
                state.error = describeError(error);
                return false;
            } finally {
                state.creatingKeystoreKitId = null;
            }
        },

        /** Pulls an existing Apple profile back into the machine's attached kit. */
        async downloadProfile(machineId: string, profileId: string): Promise<boolean> {
            state.downloadingProfileId = profileId;
            state.profileError = null;
            try {
                const result = await useBackend().downloadAppleProfile(machineId, profileId);
                const index = state.kits.findIndex((kit) => kit.id === result.kit.id);
                if (index >= 0) {
                    state.kits[index] = {
                        ...result.kit,
                        attachedMachines: state.kits[index].attachedMachines,
                    };
                }
                state.notice = `Added ${result.profile.name} to the credentials.`;
                return true;
            } catch (error) {
                state.profileError = describeError(error);
                return false;
            } finally {
                state.downloadingProfileId = null;
            }
        },

        clearMessages(): void {
            state.error = null;
            state.notice = null;
            state.messageKitId = null;
        },
    };
}
