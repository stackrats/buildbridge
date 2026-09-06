<script setup lang="ts">
import { X } from '@lucide/vue';
import { onBeforeUnmount, watch } from 'vue';

const open = defineModel<boolean>('open', { default: false });
const { wide = false } = defineProps<{ title: string; wide?: boolean }>();

function onKeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape') {
        open.value = false;
    }
}

watch(
    open,
    (value) => {
        if (value) {
            window.addEventListener('keydown', onKeydown);
        } else {
            window.removeEventListener('keydown', onKeydown);
        }
    },
    { immediate: true },
);

onBeforeUnmount(() => window.removeEventListener('keydown', onKeydown));
</script>

<template>
    <Teleport to="body">
        <div v-if="open" class="fixed inset-0 z-40 flex items-center justify-center p-6">
            <div
                class="absolute inset-0 bg-zinc-950/40 backdrop-blur-[3px]"
                @click="open = false"
            />
            <div
                role="dialog"
                aria-modal="true"
                :aria-label="title"
                class="relative z-50 flex max-h-[84vh] w-full flex-col overflow-hidden rounded-lg border border-zinc-200 bg-white shadow-[0_24px_60px_-20px_rgb(0_0_0/0.35)] dark:border-zinc-800 dark:bg-zinc-900"
                :class="wide ? 'max-w-2xl' : 'max-w-md'"
            >
                <div
                    class="flex items-center justify-between gap-3 border-b border-zinc-200 px-4 py-3 dark:border-zinc-800"
                >
                    <h2 class="text-[13px] font-semibold text-zinc-900 dark:text-zinc-50">
                        {{ title }}
                    </h2>
                    <button
                        type="button"
                        class="rounded-md p-1 text-zinc-500 hover:bg-zinc-100 hover:text-zinc-900 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-50 dark:focus-visible:outline-zinc-300"
                        aria-label="Close"
                        @click="open = false"
                    >
                        <X class="h-4 w-4" />
                    </button>
                </div>
                <div class="min-h-0 flex-1 overflow-y-auto p-4">
                    <slot />
                </div>
            </div>
        </div>
    </Teleport>
</template>
