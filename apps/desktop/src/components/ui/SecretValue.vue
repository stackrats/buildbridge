<script setup lang="ts">
import { Check, Copy, Eye, EyeOff } from '@lucide/vue';
import { onBeforeUnmount, onMounted, useId, watch } from 'vue';

import { createSecretValue } from '../../lib/secret-value';
import { copyText } from '../../lib/utils';
import Button from './Button.vue';
import Spinner from './Spinner.vue';

const {
    label,
    load,
    identity,
    disabled = false,
} = defineProps<{
    label: string;
    load: () => Promise<string>;
    identity?: string;
    disabled?: boolean;
}>();

const valueId = useId();
const secret = createSecretValue(() => load(), copyText);
const { state } = secret;

watch(() => [identity, disabled], secret.clear, { flush: 'sync' });

function visibilityChanged(): void {
    if (document.hidden) secret.clear();
}

onMounted(() => {
    window.addEventListener('blur', secret.clear);
    document.addEventListener('visibilitychange', visibilityChanged);
});

onBeforeUnmount(() => {
    secret.dispose();
    window.removeEventListener('blur', secret.clear);
    document.removeEventListener('visibilitychange', visibilityChanged);
});
</script>

<template>
    <div class="space-y-1.5">
        <div class="flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
            <span
                :id="`${valueId}-label`"
                class="text-xs font-medium text-zinc-700 dark:text-zinc-200"
            >
                {{ label }}
            </span>
            <div class="flex items-center gap-1">
                <Button
                    variant="ghost"
                    size="sm"
                    :disabled="disabled || (state.pending === 'copy' && state.value === null)"
                    :aria-label="`${state.value !== null || state.pending === 'reveal' ? 'Hide' : 'Show'} ${label}`"
                    :aria-controls="valueId"
                    :aria-expanded="state.value !== null"
                    @click="
                        state.value !== null || state.pending === 'reveal'
                            ? secret.clear()
                            : secret.reveal()
                    "
                >
                    <Spinner v-if="state.pending === 'reveal'" />
                    <EyeOff v-else-if="state.value !== null" class="h-3.5 w-3.5" />
                    <Eye v-else class="h-3.5 w-3.5" />
                    {{ state.value !== null || state.pending === 'reveal' ? 'Hide' : 'Show' }}
                </Button>
                <Button
                    variant="ghost"
                    size="sm"
                    :disabled="disabled || state.pending !== null"
                    :aria-label="`Copy ${label}`"
                    @click="secret.copy"
                >
                    <Spinner v-if="state.pending === 'copy'" />
                    <Check
                        v-else-if="state.copied"
                        class="h-3.5 w-3.5 text-emerald-700 dark:text-emerald-400"
                    />
                    <Copy v-else class="h-3.5 w-3.5" />
                    <span aria-live="polite">{{ state.copied ? 'Copied' : 'Copy' }}</span>
                </Button>
            </div>
        </div>
        <div
            :id="valueId"
            :aria-labelledby="`${valueId}-label`"
            class="max-h-48 overflow-auto rounded-md bg-zinc-50 px-2.5 py-2 font-mono text-xs break-all whitespace-pre-wrap select-text dark:bg-zinc-950/50"
        >
            {{
                disabled
                    ? 'Unavailable'
                    : state.value !== null
                      ? state.value || '(empty value)'
                      : '••••••••'
            }}
        </div>
        <p v-if="state.error" role="alert" class="text-xs text-red-700 dark:text-red-400">
            {{ state.error }}
        </p>
    </div>
</template>
