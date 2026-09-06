// Env sets: named sets of build variables held once in this host's operating-system vault.
//
// Like a signing kit, a set is host-level and attached per machine. Unlike a kit there is no
// implicit fallback: a machine builds with exactly the set it is attached to, or with none. At
// every sync the attached set is written into the guest workspace for the web build and the
// build shell. Plain variables read back with their values; secrets come back only to the editor,
// which holds them masked.

import { computed, reactive } from 'vue';

import { useBackend } from '../lib/backend';
import { describeError } from '../lib/utils';
import type { EnvSetInput, EnvSetSummary, EnvVariableSummary } from '../types/backend';

const state = reactive({
    sets: [] as EnvSetSummary[],
    loaded: false,
    loading: false,
    saving: false,
    deleting: false,
    error: null as string | null,
    notice: null as string | null,
});

export function useEnvSetsStore() {
    return {
        state,
        sets: computed(() => state.sets),
        setById: (id: string | null | undefined) =>
            id ? (state.sets.find((set) => set.id === id) ?? null) : null,

        async load(): Promise<void> {
            state.loading = !state.loaded;
            try {
                state.sets = await useBackend().listEnvSets();
                state.loaded = true;
                state.error = null;
            } catch (error) {
                state.error = describeError(error);
            } finally {
                state.loading = false;
            }
        },

        async save(input: EnvSetInput): Promise<boolean> {
            state.saving = true;
            state.error = null;
            try {
                state.sets = await useBackend().saveEnvSet(input);
                state.loaded = true;
                state.notice = input.setId
                    ? 'Environment updated in the OS vault.'
                    : 'Environment stored in the OS vault.';
                return true;
            } catch (error) {
                state.error = describeError(error);
                return false;
            } finally {
                state.saving = false;
            }
        },

        async remove(setId: string): Promise<boolean> {
            state.deleting = true;
            state.error = null;
            try {
                state.sets = await useBackend().deleteEnvSet(setId);
                state.notice =
                    'Environment removed from the vault. Machines using it are now detached.';
                return true;
            } catch (error) {
                state.error = describeError(error);
                return false;
            } finally {
                state.deleting = false;
            }
        },

        /** Every secret in one set with its value, fetched for the editor and nowhere else. */
        async reveal(setId: string): Promise<EnvVariableSummary[] | null> {
            state.error = null;
            try {
                return await useBackend().revealEnvSecrets(setId);
            } catch (error) {
                state.error = describeError(error);
                return null;
            }
        },

        clearMessages(): void {
            state.error = null;
            state.notice = null;
        },
    };
}
