<script setup lang="ts">
// The buildbridge mark: two bridge spans, the deck of the upper one carrying across and down
// into the lower. Filled with currentColor so the mark is ink on either theme; the geometry is
// the one in assets/icons/buildbridge-appicon.svg, and the mark is sized by its height (the
// width follows).
</script>

<template>
    <svg viewBox="0 0 195 150" fill="currentColor" aria-hidden="true" class="shrink-0">
        <path
            d="M0 0 L16 18.4 V128 A6 6 0 0 0 22 134 H59 A9 9 0 0 0 68 125 V96.43 L84 114.83 V125 A25 25 0 0 1 59 150 H22 A22 22 0 0 1 0 128 Z"
        />
        <path
            d="M0 66 H58.33 A30 30 0 0 1 80.98 76.3 L126.94 129.19 A14 14 0 0 0 137.51 134 H171 A8 8 0 0 0 179 126 V90 A8 8 0 0 0 171 82 H111 V66 H171 A24 24 0 0 1 195 90 V126 A24 24 0 0 1 171 150 H137.51 A30 30 0 0 1 114.86 139.7 L68.9 86.81 A14 14 0 0 0 58.33 82 H0 Z"
        />
        <path d="M111 0 L127 18.4 V118.59 L111 100.19 Z" />
    </svg>
</template>
