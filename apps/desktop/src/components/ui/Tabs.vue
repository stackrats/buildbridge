<script setup lang="ts" generic="T extends string">
// A segmented control: the strip is a well, the selected tab is the raised surface inside it.
import { nextTick, ref } from 'vue';

import { cn } from '../../lib/utils';

export interface TabOption<Value extends string> {
    value: Value;
    label: string;
    /** A panel can use this ID for its aria-labelledby relationship. */
    id?: string;
    panelId?: string;
    badge?: string | number | null;
    badgeTone?: 'neutral' | 'warn' | 'danger' | 'ok';
}

const model = defineModel<T>({ required: true });
const { tabs } = defineProps<{ tabs: TabOption<T>[] }>();
const strip = ref<HTMLElement | null>(null);

const badgeTones = {
    neutral: 'text-zinc-500 dark:text-zinc-400',
    warn: 'text-amber-700 dark:text-amber-400',
    danger: 'text-red-700 dark:text-red-400',
    ok: 'text-emerald-700 dark:text-emerald-400',
};

async function select(index: number): Promise<void> {
    const next = tabs[index];
    if (next) {
        model.value = next.value;
        await nextTick();
        strip.value?.querySelector<HTMLButtonElement>('[aria-selected="true"]')?.focus();
    }
}

function move(offset: number): void {
    const index = tabs.findIndex((tab) => tab.value === model.value);
    void select((index + offset + tabs.length) % tabs.length);
}
</script>

<template>
    <div
        ref="strip"
        role="tablist"
        class="inline-flex max-w-full items-center gap-0.5 overflow-x-auto rounded-md bg-zinc-100 p-0.5 dark:bg-zinc-900"
    >
        <button
            v-for="tab in tabs"
            :key="tab.value"
            :id="tab.id"
            type="button"
            role="tab"
            :aria-selected="model === tab.value"
            :aria-controls="tab.panelId"
            :tabindex="model === tab.value ? 0 : -1"
            :class="
                cn(
                    'flex h-7 shrink-0 items-center gap-1.5 rounded-[5px] px-2.5 text-xs font-medium whitespace-nowrap focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300',
                    model === tab.value
                        ? 'bg-white text-zinc-900 shadow-[0_1px_2px_rgb(0_0_0/0.06)] dark:bg-zinc-800 dark:text-zinc-50'
                        : 'text-zinc-500 hover:text-zinc-600 dark:text-zinc-400 dark:hover:text-zinc-300',
                )
            "
            @click="model = tab.value"
            @keydown.left.prevent="move(-1)"
            @keydown.right.prevent="move(1)"
            @keydown.home.prevent="select(0)"
            @keydown.end.prevent="select(tabs.length - 1)"
        >
            {{ tab.label }}
            <span
                v-if="tab.badge !== undefined && tab.badge !== null && tab.badge !== ''"
                class="text-[11px] tabular-nums"
                :class="badgeTones[tab.badgeTone ?? 'neutral']"
            >
                {{ tab.badge }}
            </span>
        </button>
    </div>
</template>
