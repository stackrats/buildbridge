<script setup lang="ts">
import { providerLabel } from '../../model/providers';
import type { MachineSummary } from '../../types/backend';
import Spinner from '../ui/Spinner.vue';

defineProps<{ machine?: MachineSummary }>();
</script>

<template>
    <div class="flex flex-1 flex-col">
        <header class="min-w-0">
            <h1 class="truncate text-lg font-bold text-zinc-900 dark:text-zinc-50">
                {{ machine?.config.name ?? 'Machine' }}
            </h1>
            <p class="mt-1 font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                {{ machine ? providerLabel[machine.config.provider] : 'Loading machine details…' }}
            </p>
        </header>

        <div class="flex flex-1 items-center justify-center py-10 sm:py-14">
            <div
                role="status"
                aria-live="polite"
                aria-atomic="true"
                class="w-full max-w-md rounded-xl border border-zinc-200 bg-white px-6 py-8 text-center sm:px-10 sm:py-10 dark:border-zinc-800 dark:bg-zinc-900"
            >
                <div
                    class="mx-auto flex h-14 w-14 items-center justify-center rounded-2xl bg-zinc-100 dark:bg-zinc-800"
                >
                    <Spinner size="h-6 w-6" class="motion-reduce:animate-none" />
                </div>
                <h2 class="mt-5 text-base font-semibold text-zinc-900 dark:text-zinc-50">
                    Checking your machine
                </h2>
                <p class="mx-auto mt-2 max-w-xs text-sm leading-6 text-zinc-500 dark:text-zinc-400">
                    Reading its current status and saved setup so you can pick up where you left
                    off.
                </p>
                <p
                    class="mt-6 border-t border-zinc-200 pt-5 text-xs leading-5 text-zinc-500 dark:border-zinc-800 dark:text-zinc-400"
                >
                    You can switch pages while this runs.
                </p>
            </div>
        </div>
    </div>
</template>
