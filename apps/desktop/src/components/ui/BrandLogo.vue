<script setup lang="ts">
// The logo: the mark with the name beside it. Both are outlines filled from currentColor, so
// the lockup reads as ink wherever it sits and is the same drawing on every machine.
//
// The two align on their baselines, not on their boxes. The mark's box ends on the baseline;
// the wordmark's carries the g's descender below it, so its own baseline sits at 75.808% of
// its height and centring the boxes would leave the name riding high. Starting both at the top
// and dropping the wordmark by the difference puts one baseline through the pair:
// 18 - 16 x 0.75808 = 5.87px.
import BrandMark from './BrandMark.vue';
import BrandWordmark from './BrandWordmark.vue';
</script>

<template>
    <span class="inline-flex shrink-0 items-start gap-2.5">
        <BrandMark class="h-[18px] w-auto" />
        <BrandWordmark class="mt-[5.87px] h-4 w-auto" />
    </span>
</template>
