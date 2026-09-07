<script setup lang="ts">
// The few things about this computer buildbridge does not decide for itself. Appearance is
// this window's and applies as it is chosen; the browser is the host's, shared with the
// command line, and is saved; the storage locations are facts with a way to get there.
import { FolderOpen } from '@lucide/vue';
import { computed, ref, useId, watch } from 'vue';

import { applyTheme, loadTheme, saveTheme, type Theme } from '../../lib/prefs';
import { useRunnerStore } from '../../stores/runner';
import { useSettingsStore } from '../../stores/settings';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Field from '../ui/Field.vue';
import KeyValue from '../ui/KeyValue.vue';
import Modal from '../ui/Modal.vue';
import PathField from '../ui/PathField.vue';
import SectionHeading from '../ui/SectionHeading.vue';
import Spinner from '../ui/Spinner.vue';
import Tabs from '../ui/Tabs.vue';

const open = defineModel<boolean>('open', { default: false });
const settings = useSettingsStore();
const runner = useRunnerStore();
const formId = useId();

const theme = ref<Theme>(loadTheme());
const themeTabs: { value: Theme; label: string }[] = [
    { value: 'system', label: 'System' },
    { value: 'light', label: 'Light' },
    { value: 'dark', label: 'Dark' },
];
watch(theme, (next) => {
    saveTheme(next);
    applyTheme(next);
});

const browser = ref('');
// Immediate, because the preview can open the dialog before the window has mounted.
watch(
    open,
    (value) => {
        if (value) {
            theme.value = loadTheme();
            void settings.load().then(() => {
                browser.value = settings.state.settings?.browser ?? '';
            });
        }
    },
    { immediate: true },
);

/** Saving is a no-op until the browser field differs from what the engine holds. */
const dirty = computed(
    () => (browser.value.trim() || null) !== (settings.state.settings?.browser ?? null),
);

async function save(): Promise<void> {
    // Saved on top of what the engine holds, so a field this dialog does not show — remote
    // builds, today — is carried through rather than reset to its default by a browser change.
    const current = settings.state.settings;
    if (current && (await settings.save({ ...current, browser: browser.value.trim() || null }))) {
        open.value = false;
    }
}

const storageItems = computed(() => [
    { label: 'Configuration', value: settings.state.storage?.configDir, mono: true },
    { label: 'Data', value: settings.state.storage?.dataDir, mono: true },
]);
const about = computed(() => {
    const status = runner.state.status;
    return status
        ? `buildbridge ${status.version} · ${status.platform} ${status.architecture}`
        : null;
});
</script>

<template>
    <Modal v-model:open="open" title="Settings" :busy="settings.state.saving">
        <form :id="formId" class="space-y-5" @submit.prevent="save">
            <section class="space-y-2">
                <SectionHeading>Appearance</SectionHeading>
                <Tabs v-model="theme" :tabs="themeTabs" aria-label="Theme" />
                <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    System follows the operating system as it changes.
                </p>
            </section>

            <section class="space-y-2">
                <SectionHeading>Browser</SectionHeading>
                <Field
                    label="Browser for pages outside buildbridge"
                    hint="A command such as firefox, or the browser's full path. Blank uses the desktop's default browser. Store consoles, Apple's downloads and other pages open there; the Android inspector opens in it when it is a Chromium-family browser."
                >
                    <PathField
                        v-model="browser"
                        kind="file"
                        title="Choose the browser"
                        placeholder="default browser"
                        :accept-drop="false"
                        :disabled="settings.state.loading || settings.state.saving"
                    />
                </Field>
            </section>

            <section class="space-y-2">
                <SectionHeading>Storage</SectionHeading>
                <KeyValue :items="storageItems" :columns="1" />
                <div class="flex flex-wrap gap-2">
                    <Button
                        variant="outline"
                        size="sm"
                        title="Machine records, settings and signing profiles live here"
                        @click="settings.reveal('config')"
                    >
                        <FolderOpen class="h-3.5 w-3.5" />
                        Show configuration folder
                    </Button>
                    <Button
                        variant="outline"
                        size="sm"
                        title="Machine disks, retained artifacts and downloads live here"
                        @click="settings.reveal('data')"
                    >
                        <FolderOpen class="h-3.5 w-3.5" />
                        Show data folder
                    </Button>
                </div>
                <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    The command line uses the same folders, so both see the same machines. Passwords
                    and keys are in the operating system's vault, not in either folder.
                </p>
            </section>

            <p v-if="about" class="font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                {{ about }}
            </p>

            <Callout v-if="settings.state.error" tone="danger">{{ settings.state.error }}</Callout>
        </form>
        <template #footer>
            <Button
                variant="outline"
                size="sm"
                :disabled="settings.state.saving"
                @click="open = false"
            >
                Cancel
            </Button>
            <Button
                type="submit"
                :form="formId"
                size="sm"
                :disabled="settings.state.saving || settings.state.loading || !dirty"
                title="Checks that the browser exists on this computer, then keeps it"
            >
                <Spinner v-if="settings.state.saving" tone="text-white dark:text-zinc-950" />
                {{ settings.state.saving ? 'Saving changes' : 'Save changes' }}
            </Button>
        </template>
    </Modal>
</template>
