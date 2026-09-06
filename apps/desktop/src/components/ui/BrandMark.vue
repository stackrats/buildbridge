<script setup lang="ts">
// The BuildBridge mark: two B's, the lower bowl of the first sweeping across to become the
// lower bowl of the second. Strokes inherit currentColor so the mark is ink on either theme.
// The mask opens the gap where the sweep crosses the letters; the geometry is the one in
// assets/icons/BuildBridge-appicon.svg, and the mark is sized by its height (the width follows).
import { useId } from 'vue';

// Several marks can share a page, so each one masks with its own id.
const gapId = useId();
</script>

<template>
    <svg viewBox="0 0 818 516" fill="none" aria-hidden="true" class="shrink-0">
        <defs>
            <mask :id="gapId" maskUnits="userSpaceOnUse" x="0" y="0" width="818" height="516">
                <rect width="818" height="516" fill="#fff" />
                <path d="M296.43,266.33 L556.27,485.79" stroke="#000" stroke-width="129" />
            </mask>
        </defs>
        <g stroke="currentColor" stroke-width="53" stroke-linejoin="miter" stroke-miterlimit="10">
            <path
                :mask="`url(#${gapId})`"
                d="M287.91,290.98 A90.5,117.5 0 0 1 222,489.5 H116.5 A90,90 0 0 1 26.5,399.5 V63.5 L63.5,26.5 H241 A71.5,113.5 0 0 1 241,253.5 M505.5,452.5 V63.5 L542.5,26.5 H720 A71.5,113.5 0 0 1 720,253.5 M505.5,253.5 H720"
            />
            <path
                d="M26.5,253.5 H241 A110,110 0 0 1 311.98,279.46 L535.51,468.26 A90,90 0 0 0 593.58,489.5 H701 A90.5,117.5 0 0 0 701,253.5"
            />
        </g>
    </svg>
</template>
