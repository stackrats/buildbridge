<script setup lang="ts" generic="T extends string">
// A segmented control: the strip is a well, the selected tab is the raised surface inside it.
import { cn } from '../../lib/utils';

export interface TabOption<Value extends string> {
    value: Value;
    label: string;
    badge?: string | number | null;
    badgeTone?: 'neutral' | 'warn' | 'danger' | 'ok';
}

const model = defineModel<T>({ required: true });
const { tabs } = defineProps<{ tabs: TabOption<T>[] }>();

const badgeTones = {
    neutral: 'text-zinc-500 dark:text-zinc-400',
    warn: 'text-amber-700 dark:text-amber-400',
    danger: 'text-red-700 dark:text-red-400',
    ok: 'text-emerald-700 dark:text-emerald-400',
};

function move(offset: number): void {
    const index = tabs.findIndex((tab) => tab.value === model.value);
    const next = tabs[(index + offset + tabs.length) % tabs.length];
    if (next) {
        model.value = next.value;
    }
}
</script>

<template>
    <div
        role="tablist"
        class="inline-flex items-center gap-0.5 rounded-md bg-zinc-100 p-0.5 dark:bg-zinc-900"
    >
        <button
            v-for="tab in tabs"
            :key="tab.value"
            type="button"
            role="tab"
            :aria-selected="model === tab.value"
            :tabindex="model === tab.value ? 0 : -1"
            :class="
                cn(
                    'flex h-7 items-center gap-1.5 rounded-[5px] px-2.5 text-xs font-medium focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300',
                    model === tab.value
                        ? 'bg-white text-zinc-900 shadow-[0_1px_2px_rgb(0_0_0/0.06)] dark:bg-zinc-800 dark:text-zinc-50'
                        : 'text-zinc-500 hover:text-zinc-600 dark:text-zinc-400 dark:hover:text-zinc-300',
                )
            "
            @click="model = tab.value"
            @keydown.left.prevent="move(-1)"
            @keydown.right.prevent="move(1)"
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
