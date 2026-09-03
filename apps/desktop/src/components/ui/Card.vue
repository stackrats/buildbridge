<script setup lang="ts">
import { cn } from '../../lib/utils';

const { padded = true, tone = 'surface' } = defineProps<{
    padded?: boolean;
    tone?: 'surface' | 'well';
}>();

const tones = {
    surface: 'bg-white dark:bg-zinc-900',
    well: 'bg-zinc-50 dark:bg-zinc-900/50',
};
</script>

<template>
    <section
        :class="
            cn(
                'rounded-lg border border-zinc-200 dark:border-zinc-800',
                tones[tone],
                padded && 'p-4',
            )
        "
    >
        <header
            v-if="$slots.title || $slots.actions"
            class="mb-3 flex items-start justify-between gap-3"
        >
            <div class="min-w-0">
                <h3
                    v-if="$slots.title"
                    class="text-[13px] font-semibold text-zinc-900 dark:text-zinc-50"
                >
                    <slot name="title" />
                </h3>
                <p
                    v-if="$slots.description"
                    class="mt-1 text-xs leading-5 text-zinc-500 dark:text-zinc-400"
                >
                    <slot name="description" />
                </p>
            </div>
            <div v-if="$slots.actions" class="flex shrink-0 items-center gap-1.5">
                <slot name="actions" />
            </div>
        </header>
        <slot />
    </section>
</template>
