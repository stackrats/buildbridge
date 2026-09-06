<script setup lang="ts">
// The single tooltip bubble, mounted once by the shell. It measures itself at its natural
// width before it is placed — positioning by `left` instead would size it from that offset to
// the window edge, squeezing a tooltip near the right into a column of wrapped words — then
// centres on the control, flips under it when it will not fit above, and pulls back inside
// whichever edge it would overrun. The arrow follows the control, not the bubble.
import { nextTick, ref, watch } from 'vue';

import { TOOLTIP_ID, tooltip } from '../../lib/tooltip';

const GAP = 6;
const EDGE = 8;
const ARROW = 5;

const bubble = ref<HTMLElement | null>(null);
const at = ref<{ x: number; y: number; arrow: number; below: boolean } | null>(null);

watch(
    () => [tooltip.open, tooltip.text, tooltip.anchor] as const,
    async () => {
        if (!tooltip.open) {
            at.value = null;
            return;
        }
        at.value = null;
        await nextTick();
        place();
    },
);

function place(): void {
    const element = bubble.value;
    const anchor = tooltip.anchor;
    if (!element || !anchor) {
        return;
    }
    const box = element.getBoundingClientRect();
    const control = anchor.getBoundingClientRect();
    const below = control.top < box.height + GAP + ARROW + EDGE;
    const centre = control.left + control.width / 2;
    const limit = Math.max(window.innerWidth - EDGE - box.width, EDGE);
    const x = Math.min(Math.max(centre - box.width / 2, EDGE), limit);
    at.value = {
        x,
        y: below ? control.bottom + GAP + ARROW : control.top - GAP - ARROW - box.height,
        // Keep the arrow inside the bubble's rounded corners when it has been pulled sideways.
        arrow: Math.min(Math.max(centre - x, 12), Math.max(box.width - 12, 12)),
        below,
    };
}
</script>

<template>
    <Teleport :to="tooltip.anchor?.closest('dialog') ?? 'body'">
        <div
            v-if="tooltip.open"
            :id="TOOLTIP_ID"
            ref="bubble"
            role="tooltip"
            class="tip pointer-events-none fixed top-0 left-0 z-100 max-w-72 rounded-md bg-zinc-900 px-2.5 py-1.5 text-[11px] leading-4 text-white shadow-md dark:bg-zinc-100 dark:text-zinc-950"
            :style="{
                transform: `translate3d(${at?.x ?? 0}px, ${at?.y ?? 0}px, 0)`,
                visibility: at ? 'visible' : 'hidden',
            }"
        >
            {{ tooltip.text }}
            <span
                class="absolute h-2 w-2 rotate-45 bg-zinc-900 dark:bg-zinc-100"
                :style="{
                    left: `${(at?.arrow ?? 0) - 4}px`,
                    top: at?.below ? '-3px' : undefined,
                    bottom: at?.below ? undefined : '-3px',
                }"
                aria-hidden="true"
            />
        </div>
    </Teleport>
</template>

<style scoped>
@keyframes tip-in {
    from {
        opacity: 0;
    }
    to {
        opacity: 1;
    }
}
.tip {
    animation: tip-in 0.12s ease-out;
}
@media (prefers-reduced-motion: reduce) {
    .tip {
        animation: none;
    }
}
</style>
