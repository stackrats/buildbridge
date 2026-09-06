<script setup lang="ts">
// Label and value pairs. A value worth carrying elsewhere — an identifier, a path, a checksum
// — gets a copy control beside its label. It is visible at rest rather than on hover, because
// an affordance nobody can see is one nobody uses; it only brightens under the pointer.
import CopyButton from './CopyButton.vue';

export interface KeyValueItem {
    label: string;
    value: string | number | null | undefined;
    mono?: boolean;
    tone?: 'default' | 'warn' | 'danger' | 'ok';
    /** Defaults to true for identifiers: monospaced values, or any single unbroken token. */
    copyable?: boolean;
}

const { items, columns = 2 } = defineProps<{ items: KeyValueItem[]; columns?: 1 | 2 | 3 }>();

const tones = {
    default: 'text-zinc-900 dark:text-zinc-50',
    warn: 'text-amber-700 dark:text-amber-400',
    danger: 'text-red-700 dark:text-red-400',
    ok: 'text-emerald-700 dark:text-emerald-400',
};

const grids = {
    1: 'grid-cols-1',
    2: 'grid-cols-1 sm:grid-cols-2',
    3: 'grid-cols-1 sm:grid-cols-3',
};

function present(item: KeyValueItem): boolean {
    return item.value !== null && item.value !== undefined && item.value !== '';
}

/**
 * Worth copying when it is machine-readable: a monospaced value, or a single token such as a
 * team identifier or a scheme name. Prose like "Stored in the OS vault" is not.
 */
function canCopy(item: KeyValueItem): boolean {
    if (!present(item)) {
        return false;
    }
    if (item.copyable !== undefined) {
        return item.copyable;
    }
    return item.mono === true || !/\s/.test(String(item.value));
}
</script>

<template>
    <dl class="grid gap-x-5 gap-y-2.5" :class="grids[columns]">
        <div v-for="item in items" :key="item.label" class="group min-w-0">
            <dt class="flex h-5 items-center gap-0.5 text-[11px] text-zinc-500 dark:text-zinc-400">
                {{ item.label }}
                <CopyButton
                    v-if="canCopy(item)"
                    :text="String(item.value)"
                    :what="`Copy the ${item.label.toLowerCase()}`"
                    size="iconXs"
                    class="opacity-45 group-hover:opacity-100"
                />
            </dt>
            <dd
                class="text-xs"
                :class="[
                    tones[item.tone ?? 'default'],
                    item.mono ? 'font-mono text-[11px] break-all' : 'wrap-break-word',
                ]"
            >
                {{ present(item) ? item.value : '—' }}
            </dd>
        </div>
    </dl>
</template>
