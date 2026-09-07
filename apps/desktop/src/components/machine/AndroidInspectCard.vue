<script setup lang="ts">
// Looking inside the app while it runs: its own log in the drawer, and the browser's inspector
// for the web view. Neither is why anyone opens this tab, so they rest at the bottom in the
// quiet tone rather than competing with the two ways of getting the app onto the phone.
import { ExternalLink, ScrollText } from '@lucide/vue';
import { ref } from 'vue';

import { useBackend } from '../../lib/backend';
import { describeError } from '../../lib/utils';
import type { MachineSession } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import Button from '../ui/Button.vue';
import Card from '../ui/Card.vue';
import CopyButton from '../ui/CopyButton.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import FailureBlock from '../ui/FailureBlock.vue';
import Spinner from '../ui/Spinner.vue';

const { session } = defineProps<{ session: MachineSession }>();
const ui = useUi();
const inspectorUrl = ref('chrome://inspect/#devices');
const inspectorOpening = ref(false);
const inspectorError = ref<string | null>(null);
async function openInspector(): Promise<void> {
    if (inspectorOpening.value) return;
    inspectorOpening.value = true;
    inspectorError.value = null;
    try {
        inspectorUrl.value = await useBackend().openAndroidInspector();
    } catch (cause) {
        inspectorError.value = describeError(cause);
    } finally {
        inspectorOpening.value = false;
    }
}
</script>

<template>
    <Card tone="well">
        <template #title>Inspect the running app</template>
        <template #description>
            The app's own log streams into the drawer while it runs; the inspector adds the web
            view's console, network requests and elements.
        </template>
        <template #actions>
            <Button
                variant="ghost"
                size="sm"
                :disabled="inspectorOpening"
                title="Opens the browser's device inspector page, in the browser chosen in Settings when it has one; this app's web view is listed there"
                @click="openInspector"
            >
                <Spinner v-if="inspectorOpening" />
                <ExternalLink v-else class="h-3.5 w-3.5" />
                Open inspector
            </Button>
            <Button
                v-if="session.deviceLog.length"
                variant="ghost"
                size="sm"
                @click="ui.openLog(session.id, 'device')"
            >
                <ScrollText class="h-3.5 w-3.5" />
                Show log
            </Button>
        </template>

        <div class="space-y-3">
            <div class="group flex min-w-0 items-center gap-0.5">
                <code class="font-mono text-xs break-all text-zinc-500 dark:text-zinc-400">{{
                    inspectorUrl
                }}</code>
                <CopyButton
                    :text="inspectorUrl"
                    what="Copy the inspector URL"
                    size="iconXs"
                    class="opacity-45 group-hover:opacity-100"
                />
            </div>
            <details>
                <DisclosureSummary quiet>Using the inspector</DisclosureSummary>
                <p class="mt-2 text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                    Opens a separate browser window. Keep the app open on your phone. In Chrome,
                    Chromium or Edge, enable <b>Discover USB devices</b>, then choose
                    <b>Inspect</b> beside your app's WebView. The app must allow WebView debugging;
                    release apps usually do not appear.
                </p>
            </details>
            <FailureBlock
                v-if="inspectorError"
                title="Could not open inspector"
                :diagnostic="inspectorError"
            />
        </div>
    </Card>
</template>
