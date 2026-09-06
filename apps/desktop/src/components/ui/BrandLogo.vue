<script setup lang="ts">
// The logo: the mark with the name beside it. Colour comes from the parent, so the lockup
// reads as ink wherever it sits.
import BrandMark from './BrandMark.vue';
</script>

<template>
    <span class="inline-flex items-center gap-2">
        <BrandMark class="h-5 w-auto" />
        <span class="text-[15px] font-semibold tracking-tight">BuildBridge</span>
    </span>
</template>
