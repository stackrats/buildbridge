<template>
    <div class="flex items-center justify-between gap-2">
        <h4 class="text-[11px] font-semibold text-zinc-500 dark:text-zinc-400"><slot /></h4>
        <div v-if="$slots.actions" class="flex items-center gap-1"><slot name="actions" /></div>
    </div>
</template>
