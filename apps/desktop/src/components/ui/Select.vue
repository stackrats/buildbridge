<script setup lang="ts">
// A listbox, not a native <select>.
//
// The browser draws a native select's popup itself: its own font, its own row height, its own
// highlight colour, and on Linux a menu that ignores the page's dark mode entirely. This one is
// built from the same parts as every other control here, so the closed control matches Input and
// the open list matches the rest of the interface.
//
// The panel is teleported and positioned against the viewport because a select is often inside a
// dialog or a scrolling pane that would otherwise clip it.
import { Check, ChevronDown } from '@lucide/vue';
import { computed, inject, nextTick, onBeforeUnmount, ref, useAttrs, useId, watch } from 'vue';

import { dialogKey } from '../../lib/dialog';
import { fieldKey } from '../../lib/field';
import {
    firstEnabledIndex,
    lastEnabledIndex,
    nextEnabledIndex,
    typeaheadIndex,
    type ListboxOption,
} from '../../lib/listbox';
import { cn } from '../../lib/utils';
import PlatformIcon from './PlatformIcon.vue';

defineOptions({ inheritAttrs: false });

const field = inject(fieldKey, null);
const dialog = inject(dialogKey, null);
const attrs = useAttrs();
const controlAttrs = computed(() => {
    const { class: _class, style: _style, ...rest } = attrs;
    return rest;
});

const model = defineModel<string>({ default: '' });
const {
    options,
    placeholder,
    size = 'default',
    disabled = false,
} = defineProps<{
    options: ListboxOption[];
    placeholder?: string;
    disabled?: boolean;
    size?: 'default' | 'sm';
}>();

const id = useId();
const triggerId = computed(() =>
    typeof attrs.id === 'string' ? attrs.id : (field?.controlId ?? `${id}-trigger`),
);
const listLabel = computed(() => {
    if (typeof attrs['aria-labelledby'] === 'string') {
        return { 'aria-labelledby': attrs['aria-labelledby'] };
    }
    if (typeof attrs['aria-label'] === 'string') {
        return { 'aria-label': attrs['aria-label'] };
    }
    return { 'aria-labelledby': field?.labelId ?? triggerId.value };
});
const trigger = ref<HTMLButtonElement | null>(null);
const list = ref<HTMLElement | null>(null);
const open = ref(false);
const active = ref(-1);
// Resolved against the viewport when the panel opens, and again while the page moves under it.
const anchor = ref<Record<string, string>>({});

const selectedIndex = computed(() => options.findIndex((option) => option.value === model.value));
const selectedLabel = computed(() => options[selectedIndex.value]?.label ?? '');

// Heights mirror Button's and Input's so a Select sits flush with the controls beside it.
const sizes = {
    default: 'h-8 pl-2.5 pr-8 text-[13px]',
    sm: 'h-7 pl-2 pr-7 text-xs',
};

const PANEL_MARGIN = 8;
const MAX_PANEL_HEIGHT = 240;

function place(): void {
    const element = trigger.value;
    if (!element) {
        return;
    }
    const rect = element.getBoundingClientRect();
    const below = window.innerHeight - rect.bottom - PANEL_MARGIN;
    const above = rect.top - PANEL_MARGIN;
    // Open downwards unless the list would be cramped and there is more room the other way.
    const flip = below < Math.min(MAX_PANEL_HEIGHT, above) && above > below;
    const width = Math.min(Math.max(rect.width, 240), window.innerWidth - PANEL_MARGIN * 2);

    anchor.value = {
        left: `${Math.max(PANEL_MARGIN, Math.min(rect.left, window.innerWidth - width - PANEL_MARGIN))}px`,
        width: `${width}px`,
        maxHeight: `${Math.max(0, Math.min(MAX_PANEL_HEIGHT, (flip ? above : below) - 4))}px`,
        ...(flip
            ? { bottom: `${window.innerHeight - rect.top + 4}px` }
            : { top: `${rect.bottom + 4}px` }),
    };
}

async function show(index: number): Promise<void> {
    if (disabled) {
        return;
    }
    place();
    active.value = index;
    open.value = true;
    await nextTick();
    list.value?.focus();
    scrollActiveIntoView();
}

function hide(restoreFocus = true): void {
    open.value = false;
    active.value = -1;
    if (restoreFocus) {
        trigger.value?.focus();
    }
}

function choose(index: number): void {
    const option = options[index];
    if (!option || option.disabled) {
        return;
    }
    model.value = option.value;
    hide();
}

function moveTo(index: number): void {
    if (index < 0) {
        return;
    }
    active.value = index;
    scrollActiveIntoView();
}

function scrollActiveIntoView(): void {
    if (active.value < 0) {
        return;
    }
    list.value
        ?.querySelector(`[data-index="${active.value}"]`)
        ?.scrollIntoView({ block: 'nearest' });
}

function onTriggerKeydown(event: KeyboardEvent): void {
    if (event.key === 'ArrowDown' || event.key === 'Enter' || event.key === ' ') {
        event.preventDefault();
        void show(selectedIndex.value >= 0 ? selectedIndex.value : firstEnabledIndex(options));
        return;
    }
    if (event.key === 'ArrowUp') {
        event.preventDefault();
        void show(selectedIndex.value >= 0 ? selectedIndex.value : lastEnabledIndex(options));
    }
}

let typed = '';
let typedAt = 0;

function onListKeydown(event: KeyboardEvent): void {
    switch (event.key) {
        case 'ArrowDown':
            event.preventDefault();
            moveTo(nextEnabledIndex(options, active.value, 1));
            return;
        case 'ArrowUp':
            event.preventDefault();
            moveTo(nextEnabledIndex(options, active.value, -1));
            return;
        case 'Home':
            event.preventDefault();
            moveTo(firstEnabledIndex(options));
            return;
        case 'End':
            event.preventDefault();
            moveTo(lastEnabledIndex(options));
            return;
        case 'Enter':
        case ' ':
            event.preventDefault();
            choose(active.value);
            return;
        case 'Escape':
            // Close this popup before Escape can dismiss its containing dialog.
            event.preventDefault();
            event.stopPropagation();
            hide();
            return;
        case 'Tab':
            // Restore the trigger before the browser advances so Tab continues at the next
            // field, rather than at the popup's position at the end of the document or dialog.
            hide();
            return;
        default:
            break;
    }

    if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey) {
        const now = Date.now();
        typed = now - typedAt > 800 ? event.key : typed + event.key;
        typedAt = now;
        moveTo(typeaheadIndex(options, typed, typed.length > 1 ? active.value - 1 : active.value));
    }
}

function onPointerDown(event: PointerEvent): void {
    const target = event.target as Node;
    if (!list.value?.contains(target) && !trigger.value?.contains(target)) {
        hide(false);
    }
}

watch(open, (value) => {
    if (value) {
        document.addEventListener('pointerdown', onPointerDown, true);
        window.addEventListener('resize', place);
        window.addEventListener('scroll', place, true);
    } else {
        document.removeEventListener('pointerdown', onPointerDown, true);
        window.removeEventListener('resize', place);
        window.removeEventListener('scroll', place, true);
    }
});

onBeforeUnmount(() => {
    document.removeEventListener('pointerdown', onPointerDown, true);
    window.removeEventListener('resize', place);
    window.removeEventListener('scroll', place, true);
});
</script>

<template>
    <span
        class="relative block w-full"
        :class="attrs.class"
        :style="attrs.style as string | undefined"
    >
        <button
            :id="triggerId"
            ref="trigger"
            type="button"
            role="combobox"
            aria-haspopup="listbox"
            :aria-expanded="open"
            :aria-controls="open ? `${id}-list` : undefined"
            :aria-labelledby="attrs['aria-label'] ? undefined : field?.labelId"
            :aria-describedby="field?.descriptionId.value"
            :aria-required="field?.required.value || undefined"
            :aria-invalid="field?.invalid.value || undefined"
            v-bind="controlAttrs"
            :disabled="disabled"
            :class="
                cn(
                    'flex w-full items-center rounded-md border bg-white text-left whitespace-nowrap transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 disabled:opacity-50 dark:bg-zinc-900 dark:focus-visible:outline-zinc-300',
                    field?.invalid.value
                        ? 'border-red-500'
                        : 'border-zinc-200 dark:border-zinc-800',
                    sizes[size],
                    open && 'border-zinc-700 dark:border-zinc-300',
                    selectedLabel === ''
                        ? 'text-zinc-400 dark:text-zinc-500'
                        : 'text-zinc-900 dark:text-zinc-50',
                )
            "
            @click="
                open
                    ? hide()
                    : show(selectedIndex >= 0 ? selectedIndex : firstEnabledIndex(options))
            "
            @keydown="onTriggerKeydown"
        >
            <span
                v-if="options[selectedIndex]?.platforms?.length"
                class="mr-2 flex shrink-0 items-center gap-1"
            >
                <PlatformIcon
                    v-for="platform in options[selectedIndex]?.platforms"
                    :key="platform"
                    :platform="platform"
                    class="h-3.5 w-3.5"
                />
            </span>
            <span class="min-w-0 flex-1 truncate">
                {{ selectedLabel || placeholder || 'Choose one' }}
            </span>
        </button>
        <ChevronDown
            class="pointer-events-none absolute top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-zinc-500 transition-transform dark:text-zinc-400"
            :class="[size === 'sm' ? 'right-2' : 'right-2.5', open && 'rotate-180']"
        />

        <Teleport :to="dialog ?? 'body'">
            <div
                v-if="open"
                :id="`${id}-list`"
                ref="list"
                role="listbox"
                tabindex="-1"
                v-bind="listLabel"
                :aria-activedescendant="active >= 0 ? `${id}-option-${active}` : undefined"
                class="fixed z-[60] overflow-y-auto overscroll-contain rounded-md border border-zinc-200 bg-white p-1 shadow-[0_16px_40px_-16px_rgb(0_0_0/0.35)] outline-none dark:border-zinc-800 dark:bg-zinc-900"
                :style="anchor"
                @keydown="onListKeydown"
            >
                <p
                    v-if="options.length === 0"
                    class="px-2 py-1.5 text-xs text-zinc-500 dark:text-zinc-400"
                >
                    Nothing to choose from
                </p>
                <div
                    v-for="(option, index) in options"
                    :id="`${id}-option-${index}`"
                    :key="option.value"
                    role="option"
                    :data-index="index"
                    :aria-selected="option.value === model"
                    :aria-disabled="option.disabled || undefined"
                    class="flex cursor-pointer items-start gap-2 rounded-sm px-2 py-2 text-[13px] leading-5"
                    :class="[
                        option.disabled
                            ? 'cursor-not-allowed text-zinc-400 dark:text-zinc-600'
                            : 'text-zinc-900 dark:text-zinc-50',
                        index === active && !option.disabled && 'bg-zinc-100 dark:bg-zinc-800',
                    ]"
                    @pointerenter="option.disabled ? null : (active = index)"
                    @click="choose(index)"
                >
                    <Check
                        class="mt-0.5 h-3.5 w-3.5 shrink-0 transition-opacity"
                        :class="option.value === model ? 'opacity-100' : 'opacity-0'"
                        aria-hidden="true"
                    />
                    <span class="min-w-0 flex-1 break-words">
                        <span class="flex items-center gap-1.5">
                            <PlatformIcon
                                v-for="platform in option.platforms"
                                :key="platform"
                                :platform="platform"
                                class="h-3.5 w-3.5"
                            />
                            <span>{{ option.label }}</span>
                        </span>
                        <span
                            v-if="option.description"
                            class="mt-0.5 block text-xs leading-5 text-zinc-500 dark:text-zinc-400"
                        >
                            {{ option.description }}
                        </span>
                    </span>
                </div>
            </div>
        </Teleport>
    </span>
</template>
