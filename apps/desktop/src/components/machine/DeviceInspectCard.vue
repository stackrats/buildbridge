<script setup lang="ts">
// Looking inside the app while it runs: its console in the drawer, and Safari's Web Inspector
// for the web view. Neither is why anyone opened this step, so they rest at the bottom in the
// quiet tone rather than competing with the phone and the run.
//
// Web Inspector for the app on the phone lives in the guest's Safari, not here: buildbridge
// opens Safari with its Develop menu on and then says what to click. It is not a machine
// operation — the time to inspect is while the run streams — so it keeps its own state here.
import { Globe, ScrollText, X } from '@lucide/vue';
import { computed, ref } from 'vue';

import { useMachinesStore, type MachineSession } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import type { SafariInspectorResult } from '../../types/backend';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import CopyButton from '../ui/CopyButton.vue';
import FailureBlock from '../ui/FailureBlock.vue';
import Spinner from '../ui/Spinner.vue';

const { session, available } = defineProps<{
    session: MachineSession;
    /** The app has run on the phone: only then does Safari list a web view to inspect. */
    available: boolean;
    deviceName: string;
}>();
const machines = useMachinesStore();
const ui = useUi();
// Only a Capacitor app's web view answers at capacitor://localhost; any other web view is
// listed by its page title.
const capacitor = computed(() => session.view?.appleWorkspace?.layout.kind === 'capacitor');
const inspector = ref<SafariInspectorResult | null>(null);
const inspecting = ref(false);
const inspectorError = ref<string | null>(null);
async function openInspector(): Promise<void> {
    inspecting.value = true;
    inspectorError.value = null;
    try {
        inspector.value = await machines.openSafariInspector(session.id);
    } catch (error) {
        inspectorError.value = error instanceof Error ? error.message : String(error);
    } finally {
        inspecting.value = false;
    }
}
</script>

<template>
    <Card tone="well">
        <template #title>Inspect the running app</template>
        <template #description>
            The app's own console streams into the drawer while it runs; Safari's Web Inspector adds
            the web view's console, network requests, elements and storage.
        </template>
        <template #actions>
            <Button
                variant="ghost"
                size="sm"
                :disabled="inspecting || !available"
                :title="
                    available
                        ? 'Opens Safari in the guest console with its Develop menu; the app\'s web view is listed there under the phone\'s name. Works while the app runs.'
                        : 'Run the app on the phone first; Safari lists its web view only once it is installed.'
                "
                @click="openInspector"
            >
                <Spinner v-if="inspecting" />
                <Globe v-else class="h-3.5 w-3.5" />
                Inspect in Safari
            </Button>
            <Button
                v-if="session.deviceLog.length"
                variant="ghost"
                size="sm"
                @click="ui.openLog(session.id, 'device')"
            >
                <ScrollText class="h-3.5 w-3.5" />
                Show console
            </Button>
        </template>

        <div class="space-y-3 empty:hidden">
            <FailureBlock
                v-if="inspectorError"
                title="Safari could not be opened in the guest"
                cause="Safari needs the guest's graphical session: log in on the machine's screen, then try again."
                :diagnostic="inspectorError"
            />
            <Callout
                v-if="inspector"
                tone="neutral"
                title="Web Inspector: Safari is open on the guest's screen"
            >
                <ol class="list-decimal space-y-1 pl-4">
                    <li>
                        On the phone, once: Settings › Safari › Advanced › <b>Web Inspector</b>
                        (under Settings › Apps › Safari on iOS 18 and later).
                    </li>
                    <li v-if="!inspector.developMenuEnabled">
                        In Safari: Safari › Settings › Advanced ›
                        <b>Show features for web developers</b>, then quit and reopen Safari. macOS
                        did not let buildbridge set this from outside the graphical session.
                    </li>
                    <li v-else-if="inspector.safariRestarted">
                        Safari's Develop menu is on; Safari was reopened so it appears.
                    </li>
                    <li v-else>Safari's Develop menu is on.</li>
                    <li>
                        In Safari's menu bar: <b>Develop</b> › <b>{{ deviceName }}</b> › the app's
                        web view, listed as
                        <span
                            v-if="capacitor"
                            class="group inline-flex items-center gap-0.5 align-middle"
                            ><code class="font-mono">capacitor://localhost</code>
                            <CopyButton
                                text="capacitor://localhost"
                                what="Copy the web view address"
                                size="iconXs"
                                class="opacity-45 group-hover:opacity-100"
                        /></span>
                        <template v-else>its address</template>
                        or its page title. The inspector shows its console, network requests,
                        elements and storage.
                    </li>
                </ol>
                <p class="mt-2">
                    Only the Debug build buildbridge installs is inspectable (a web view is
                    inspectable in Debug builds), so run the app first. The console is this
                    machine's QEMU window on this host; Ctrl+Alt+G releases the mouse.
                </p>
                <div class="mt-2">
                    <Button variant="ghost" size="sm" @click="inspector = null">
                        <X class="h-3.5 w-3.5" />
                        Hide
                    </Button>
                </div>
            </Callout>
        </div>
    </Card>
</template>
