<script setup lang="ts">
// Copy one value to the clipboard, with the tick standing in for a message. Where a value is
// worth copying, this sits beside its label: the conventional place to reach for it.
import { Check, Copy } from '@lucide/vue';
import { computed, onBeforeUnmount, ref } from 'vue';

import { copyText } from '../../lib/utils';
import Button from './Button.vue';

const {
    text,
    label,
    what,
    size = 'sm',
} = defineProps<{
    text: string;
    /** Visible label; omit for an icon-only button. */
    label?: string;
    /** What is being copied, for the tooltip: "Copy the bundle identifier". */
    what?: string;
    size?: 'sm' | 'icon' | 'iconSm' | 'iconXs';
}>();

const copied = ref(false);
let timer: ReturnType<typeof setTimeout> | null = null;

const iconClass = computed(() => (size === 'iconXs' ? 'h-3 w-3' : 'h-3.5 w-3.5'));
// A visible label already says what the button does and shows "Copied" itself; only an
// icon-only button, or one copying something the label does not name, gets a tooltip.
const tip = computed(() => {
    if (label && !what) {
        return undefined;
    }
    return copied.value ? 'Copied' : (what ?? 'Copy');
});

async function copy(): Promise<void> {
    await copyText(text);
    copied.value = true;
    if (timer) {
        clearTimeout(timer);
    }
    timer = setTimeout(() => (copied.value = false), 1500);
}

onBeforeUnmount(() => {
    if (timer) {
        clearTimeout(timer);
    }
});
</script>

<template>
    <Button variant="ghost" :size="size" :title="tip" :aria-label="tip" @click="copy">
        <Check v-if="copied" :class="[iconClass, 'text-emerald-700 dark:text-emerald-400']" />
        <Copy v-else :class="iconClass" />
        <template v-if="label">{{ copied ? 'Copied' : label }}</template>
    </Button>
</template>
