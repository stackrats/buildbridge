<script setup lang="ts">
// The logo: the mark with the name beside it. Colour comes from the parent, so the lockup
// reads as ink wherever it sits.
import BrandMark from './BrandMark.vue';
</script>

<template>
    <span class="inline-flex h-6 shrink-0 items-center gap-2.5 whitespace-nowrap">
        <BrandMark class="h-[18px] w-auto" />
        <span class="font-mono text-base leading-none font-medium tracking-tight">BuildBridge</span>
    </span>
</template>
