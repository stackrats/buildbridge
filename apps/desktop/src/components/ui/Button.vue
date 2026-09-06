<script setup lang="ts">
// The shared tooltip is attached to a wrapper rather than to the button itself: a disabled
// button has `pointer-events: none`, so it never sees a hover, and its reason for being
// disabled is the one you most want to read. `title` here is our own prop and is stripped
// before it reaches the DOM, so no native tooltip is ever produced.
import { computed, onBeforeUnmount, ref, useAttrs, useId } from 'vue';

import { hideTip, showTip } from '../../lib/tooltip';

import { cn } from '../../lib/utils';

defineOptions({ inheritAttrs: false });

const {
    variant = 'default',
    size = 'default',
    disabled = false,
    type = 'button',
} = defineProps<{
    variant?: 'default' | 'secondary' | 'outline' | 'ghost' | 'danger';
    size?: 'default' | 'sm' | 'icon' | 'iconSm' | 'iconXs';
    disabled?: boolean;
    type?: 'button' | 'submit';
}>();

const attrs = useAttrs();
const id = useId();
const buttonId = computed(() => (typeof attrs.id === 'string' ? attrs.id : `${id}-button`));
const descriptionId = `${id}-description`;
const tip = computed(() => {
    const value = attrs.title;
    return typeof value === 'string' && value.trim() !== '' ? value : null;
});
const describedBy = computed(
    () =>
        [
            typeof attrs['aria-describedby'] === 'string' ? attrs['aria-describedby'] : '',
            tip.value ? descriptionId : '',
        ]
            .filter(Boolean)
            .join(' ') || undefined,
);
/** Everything except the title, which this component draws itself. */
const passed = computed(() => {
    const { title: _title, ...rest } = attrs;
    return rest;
});

const anchor = ref<HTMLElement | null>(null);

function show(): void {
    if (tip.value && anchor.value) {
        showTip(anchor.value, tip.value);
    }
}

function hide(): void {
    if (anchor.value) {
        hideTip(anchor.value);
    }
}

function onKeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape') {
        hide();
    }
}

onBeforeUnmount(hide);
</script>

<template>
    <span
        ref="anchor"
        :class="
            tip
                ? 'inline-flex max-w-full rounded-md focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300'
                : 'contents'
        "
        :tabindex="disabled && tip ? 0 : undefined"
        :role="disabled && tip ? 'group' : undefined"
        :aria-labelledby="disabled && tip ? buttonId : undefined"
        :aria-describedby="disabled && tip ? descriptionId : undefined"
        @mouseenter="show"
        @mouseleave="hide"
        @focusin="show"
        @focusout="hide"
        @keydown="onKeydown"
    >
        <button
            v-bind="passed"
            :id="buttonId"
            :aria-describedby="describedBy"
            :type="type"
            :class="
                cn(
                    'inline-flex items-center justify-center gap-1.5 rounded-md font-medium whitespace-nowrap focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 disabled:pointer-events-none disabled:opacity-45 dark:focus-visible:outline-zinc-300',
                    {
                        default:
                            'bg-zinc-900 text-white hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-950 dark:hover:bg-zinc-300',
                        secondary:
                            'bg-zinc-100 text-zinc-900 hover:bg-zinc-200 dark:bg-zinc-800 dark:text-zinc-50 dark:hover:bg-zinc-700',
                        outline:
                            'border border-zinc-200 bg-white text-zinc-900 hover:border-zinc-300 hover:bg-zinc-100 dark:border-zinc-800 dark:bg-zinc-900 dark:text-zinc-50 dark:hover:border-zinc-600 dark:hover:bg-zinc-800',
                        ghost: 'text-zinc-600 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-300 dark:hover:bg-zinc-800 dark:hover:text-zinc-50',
                        danger: 'bg-red-50 text-red-700 hover:brightness-95 dark:bg-red-950/50 dark:text-red-400 dark:hover:brightness-125',
                    }[variant],
                    {
                        default: 'h-8 px-3 text-[13px]',
                        sm: 'h-7 px-2.5 text-xs',
                        icon: 'h-8 w-8 p-0',
                        iconSm: 'h-7 w-7 p-0',
                        iconXs: 'h-5 w-5 p-0',
                    }[size],
                )
            "
            :disabled="disabled"
        >
            <slot />
        </button>
        <span v-if="tip" :id="descriptionId" class="sr-only">{{ tip }}</span>
    </span>
</template>
