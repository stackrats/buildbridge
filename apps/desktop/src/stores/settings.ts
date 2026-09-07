// This host's preferences, read from and written to the engine so the desktop and the command
// line agree, plus where buildbridge keeps its files.

import { computed, reactive } from 'vue';

import { useBackend } from '../lib/backend';
import { describeError } from '../lib/utils';
import type { HostSettings, StorageDirectory, StorageLocations } from '../types/backend';

const state = reactive({
    settings: null as HostSettings | null,
    storage: null as StorageLocations | null,
    loading: false,
    saving: false,
    error: null as string | null,
});

export function useSettingsStore() {
    return {
        state,
        settings: computed(() => state.settings),
        storage: computed(() => state.storage),

        /**
         * Whether this host offers remote builds at all. Off is the answer while the settings
         * are still loading as well as when they say so, because the sidebar, the top bar and
         * the route all read it on the first frame, and a feature that appears and then
         * disappears is worse than one that arrives a moment late.
         */
        remoteBuilds: computed(() => state.settings?.remoteBuilds === true),

        async load(): Promise<void> {
            state.loading = state.settings === null;
            try {
                const backend = useBackend();
                [state.settings, state.storage] = await Promise.all([
                    backend.getHostSettings(),
                    backend.getStorageLocations(),
                ]);
                state.error = null;
            } catch (error) {
                state.error = describeError(error);
            } finally {
                state.loading = false;
            }
        },

        /** Saves and returns whether the engine accepted; the refusal stays in `error`. */
        async save(input: HostSettings): Promise<boolean> {
            state.saving = true;
            try {
                state.settings = await useBackend().saveHostSettings(input);
                state.error = null;
                return true;
            } catch (error) {
                state.error = describeError(error);
                return false;
            } finally {
                state.saving = false;
            }
        },

        async reveal(directory: StorageDirectory): Promise<void> {
            try {
                await useBackend().revealStorageDirectory(directory);
            } catch (error) {
                state.error = describeError(error);
            }
        },
    };
}
