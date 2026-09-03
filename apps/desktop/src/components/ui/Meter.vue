<script setup lang="ts">
// A filled bar for a bounded quantity, or a sweeping bar when the total is unknown.
import { cn } from '../../lib/utils';

const {
    value = null,
    tone = 'primary',
    width = 'w-full',
} = defineProps<{
    /** 0–1 when known; null renders an indeterminate sweep. */
    value?: number | null;
    tone?: 'primary' | 'ok' | 'warn' | 'danger';
    width?: string;
}>();

const fills = {
    primary: 'bg-zinc-900 dark:bg-zinc-100',
    ok: 'bg-emerald-500',
    warn: 'bg-amber-500',
    danger: 'bg-red-500',
};
</script>

<template>
    <div
        :class="cn('relative h-1 overflow-hidden rounded-full bg-zinc-200 dark:bg-zinc-700', width)"
        role="progressbar"
        :aria-valuenow="
            value === null ? undefined : Math.round(Math.min(1, Math.max(0, value)) * 100)
        "
        aria-valuemin="0"
        aria-valuemax="100"
    >
        <div
            v-if="value !== null"
            class="h-full rounded-full transition-[width] duration-300"
            :class="fills[tone]"
            :style="{ width: `${Math.min(1, Math.max(0, value)) * 100}%` }"
        />
        <div
            v-else
            class="meter-sweep absolute inset-y-0 w-1/3 rounded-full"
            :class="fills[tone]"
        />
    </div>
</template>

<style scoped>
@keyframes meter-sweep {
    from {
        transform: translateX(-120%);
    }
    to {
        transform: translateX(320%);
    }
}
.meter-sweep {
    animation: meter-sweep 1.6s ease-in-out infinite;
}
</style>
