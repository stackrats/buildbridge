<script setup lang="ts">
import { X } from '@lucide/vue';
import { nextTick, onBeforeUnmount, provide, ref, useId, watch } from 'vue';

import { containDialogTab, dialogKey } from '../../lib/dialog';

const open = defineModel<boolean>('open', { default: false });
const { wide = false, busy = false } = defineProps<{
    title: string;
    wide?: boolean;
    /** Keep a save in view until it finishes. Operations that outlive a dialog can omit this. */
    busy?: boolean;
}>();

const panel = ref<HTMLDialogElement | null>(null);
const titleId = `${useId()}-title`;
let returnFocus: HTMLElement | null = null;
const pointerStartedOutside = ref(false);

provide(dialogKey, panel);

function dismiss(): void {
    if (!busy) {
        open.value = false;
    }
}

function outside(event: MouseEvent): boolean {
    const dialog = panel.value;
    if (!dialog || event.target !== dialog) {
        return false;
    }
    const bounds = dialog.getBoundingClientRect();
    return (
        event.clientX < bounds.left ||
        event.clientX > bounds.right ||
        event.clientY < bounds.top ||
        event.clientY > bounds.bottom
    );
}

function close(): void {
    panel.value?.close();
    if (returnFocus?.isConnected) {
        returnFocus.focus({ preventScroll: true });
    }
    returnFocus = null;
}

watch(
    open,
    async (value) => {
        if (!value) {
            close();
            return;
        }
        returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        await nextTick();
        if (!open.value || !panel.value || panel.value.open) {
            return;
        }
        // The browser supplies background inertness, focus containment and nested dialog
        // stacking. Popups use the provided dialog as their host to stay in the same top layer.
        panel.value.showModal();
        const fields = panel.value.querySelectorAll<HTMLElement>(
            '[autofocus], input:not([type="hidden"]):not(:disabled), textarea:not(:disabled), [role="combobox"]:not(:disabled)',
        );
        const initial = [...fields].find((element) => element.getClientRects().length > 0);
        (initial ?? panel.value).focus({ preventScroll: true });
    },
    { immediate: true },
);

onBeforeUnmount(close);
</script>

<template>
    <Teleport to="body">
        <dialog
            v-if="open"
            ref="panel"
            tabindex="-1"
            aria-modal="true"
            :aria-labelledby="titleId"
            :aria-busy="busy || undefined"
            class="fixed inset-0 m-auto max-h-[calc(100dvh-2rem)] w-[calc(100vw-2rem)] flex-col overflow-visible rounded-xl border border-zinc-200 bg-white p-0 text-zinc-900 shadow-[0_24px_60px_-20px_rgb(0_0_0/0.35)] outline-none open:flex dark:border-zinc-800 dark:bg-zinc-900 dark:text-zinc-50"
            :class="wide ? 'max-w-2xl' : 'max-w-md'"
            @cancel.prevent="dismiss"
            @keydown="panel && containDialogTab(panel, $event)"
            @pointerdown="pointerStartedOutside = outside($event)"
            @click="pointerStartedOutside && outside($event) && dismiss()"
        >
            <div
                class="flex shrink-0 items-center justify-between gap-3 border-b border-zinc-200 px-5 py-4 dark:border-zinc-800"
            >
                <h2 :id="titleId" class="text-base font-semibold">
                    {{ title }}
                </h2>
                <button
                    type="button"
                    class="flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-zinc-500 hover:bg-zinc-100 hover:text-zinc-900 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 disabled:pointer-events-none disabled:opacity-45 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-50 dark:focus-visible:outline-zinc-300"
                    aria-label="Close dialog"
                    :disabled="busy"
                    @click="dismiss"
                >
                    <X class="h-4 w-4" aria-hidden="true" />
                </button>
            </div>
            <!-- Keep the content's intrinsic height: a zero flex basis collapses auto-sized
                 dialogs in WebKit. The body can still shrink and scroll at the viewport cap. -->
            <div class="min-h-0 flex-auto overflow-y-auto overscroll-contain p-5">
                <slot />
            </div>
            <div
                v-if="$slots.footer"
                class="flex shrink-0 flex-wrap items-center justify-end gap-2 border-t border-zinc-200 px-5 py-4 dark:border-zinc-800"
            >
                <slot name="footer" />
            </div>
        </dialog>
    </Teleport>
</template>

<style scoped>
dialog::backdrop {
    background: rgb(9 9 11 / 40%);
    backdrop-filter: blur(3px);
}
</style>
