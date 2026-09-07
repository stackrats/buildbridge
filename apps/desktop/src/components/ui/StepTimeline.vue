<script setup lang="ts" generic="Id extends string">
// The whole journey as one full-width timeline: a progress header, a section per phase, and a
// row per step carrying its result. The selected step opens in place; its essential actions stay
// visible while individual steps can disclose their technical reference separately.
import {
    ArrowRight,
    Check,
    ChevronDown,
    Hammer,
    Smartphone,
    TriangleAlert,
    Upload,
    Wrench,
    X,
} from '@lucide/vue';
import { computed, nextTick, ref, watch, type Component } from 'vue';

import { formatElapsed } from '../../lib/format';
import {
    completedCount,
    requiredSteps,
    focusStep,
    groupByPhase,
    stepKindDescription,
    type PhaseGroup,
    stepKindLabel,
    type Step,
    type StepPhase,
    type StepStatus,
} from '../../model/steps';
import { cn } from '../../lib/utils';
import Badge from './Badge.vue';
import Button from './Button.vue';
import Meter from './Meter.vue';
import Spinner from './Spinner.vue';

const selected = defineModel<Id | null>('selected', { required: true });
const {
    steps,
    headline,
    summaries = {},
    runningSeconds = null,
} = defineProps<{
    steps: Step<Id>[];
    /** Where the journey stands in a few words, from the model that knows the short names. */
    headline: string;
    /** What a finished phase achieved, shown on its header once it folds away. */
    summaries?: Partial<Record<StepPhase, string>>;
    /** Live elapsed time for the step that is running, when one is. */
    runningSeconds?: number | null;
}>();

const groups = computed(() => groupByPhase(steps));
// Optional steps live in their own section and stay out of the headline count.
const required = computed(() => requiredSteps(steps));
const done = computed(() => completedCount(required.value));
const focus = computed(() => focusStep(steps));

/** How many steps in a phase still need the person: said once here, not on every row. */
function yoursRemaining(phaseSteps: Step<Id>[]): number {
    return phaseSteps.filter(
        (step) => step.kind !== 'automatic' && step.status !== 'done' && !step.optional,
    ).length;
}

// Each phase keeps one glyph, tinted once the phase is complete, so a folded header still
// says what it holds.
const phaseIcon: Record<StepPhase, Component> = {
    setup: Wrench,
    build: Hammer,
    device: Smartphone,
    publish: Upload,
};

// A finished phase folds to its header so the timeline stays short; it reopens on click, and
// whenever the step you are looking at is inside it. What a person chose is the model, so a
// page comes back as it was left; a phase nobody has touched takes the default.
const chosen = defineModel<Record<string, boolean>>('expanded', { default: () => ({}) });
function phaseOf(id: Id | null): StepPhase | null {
    return steps.find((step) => step.id === id)?.phase ?? null;
}
function isExpanded(group: PhaseGroup<Id>): boolean {
    return (
        chosen.value[group.phase] ?? (!group.complete || group.phase === phaseOf(selected.value))
    );
}
function setExpanded(phase: StepPhase, open: boolean): void {
    chosen.value = { ...chosen.value, [phase]: open };
}
// A phase that completes folds, and the one holding the open step unfolds, whatever was chosen.
watch(
    () => groups.value.map((group) => `${group.phase}:${group.complete}`).join(','),
    () => {
        const open = phaseOf(selected.value);
        const next = { ...chosen.value };
        for (const group of groups.value) {
            next[group.phase] = !group.complete || group.phase === open;
        }
        chosen.value = next;
    },
);

const rows = ref<Record<string, HTMLElement | null>>({});
function keepRow(id: Id, element: unknown): void {
    rows.value[id] = (element as HTMLElement | null) ?? null;
}

// Opening a step scrolls just enough to bring it into view, never the whole page.
watch(selected, async (id) => {
    if (!id) {
        return;
    }
    const phase = phaseOf(id);
    if (phase) {
        setExpanded(phase, true);
    }
    await nextTick();
    rows.value[id]?.scrollIntoView({ block: 'nearest' });
});

/** The steps on screen: what the arrow keys walk, and what has to stay non-empty. */
const visible = computed(() =>
    groups.value.flatMap((group) => (isExpanded(group) ? group.steps : [])),
);

function togglePhase(group: PhaseGroup<Id>): void {
    setExpanded(group.phase, !isExpanded(group));
}

async function move(from: Id, offset: number): Promise<void> {
    const list = visible.value;
    const index = list.findIndex((step) => step.id === from);
    const next = list[Math.min(Math.max(index + offset, 0), list.length - 1)];
    if (next) {
        selected.value = next.id;
        await nextTick();
        rows.value[next.id]
            ?.querySelector<HTMLButtonElement>('button')
            ?.focus({ preventScroll: true });
    }
}

// The node is the status: filled when done, ink when it is your turn, ringed while it runs,
// pulsing once it is live.
const nodeClass: Record<StepStatus, string> = {
    done: 'border-emerald-600 dark:border-emerald-500 bg-emerald-600 dark:bg-emerald-500 text-white dark:text-zinc-950',
    active: 'border-zinc-900 dark:border-zinc-100 bg-zinc-900 dark:bg-zinc-100 text-white dark:text-zinc-950',
    running:
        'border-zinc-900 dark:border-zinc-100 bg-white dark:bg-zinc-900 text-zinc-900 dark:text-zinc-100',
    failed: 'border-red-500 bg-red-500 text-white dark:text-zinc-950',
    pending:
        'border-zinc-300 dark:border-zinc-700 bg-white dark:bg-zinc-900 text-zinc-500 dark:text-zinc-400',
};

// A halo marks where the journey has actually reached, so it stays visible even when you are
// reading a different step. It sits outside the node, so it works over any row background.
const haloClass: Record<StepStatus, string> = {
    done: '',
    active: 'ring-4 ring-zinc-900/15 dark:ring-zinc-100/20',
    running: 'ring-4 ring-zinc-900/15 dark:ring-zinc-100/20',
    failed: 'ring-4 ring-red-500/20',
    pending: '',
};

// A live step — up, and staying up until it is stopped — pulses in the status colour instead of
// spinning: a spinner says "wait", and there is nothing to wait for.
const liveNodeClass =
    'border-emerald-600 dark:border-emerald-500 bg-white dark:bg-zinc-900 ring-4 ring-emerald-500/20';
const liveMetaClass = 'text-emerald-700 dark:text-emerald-400';

const titleClass: Record<StepStatus, string> = {
    done: 'text-zinc-700 dark:text-zinc-200',
    active: 'text-zinc-900 dark:text-zinc-50 font-semibold',
    running: 'text-zinc-900 dark:text-zinc-50 font-semibold',
    failed: 'text-red-700 dark:text-red-400 font-semibold',
    pending: 'text-zinc-500 dark:text-zinc-400',
};

const metaClass: Record<StepStatus, string> = {
    done: 'text-emerald-700 dark:text-emerald-400',
    active: 'text-zinc-900 dark:text-zinc-50',
    running: 'text-zinc-900 dark:text-zinc-50',
    failed: 'text-red-700 dark:text-red-400',
    pending: 'text-zinc-500 dark:text-zinc-400',
};

/**
 * The right-hand column says only what the node cannot: how long a run has taken, that a
 * failure needs a person, whose turn an open step is, and how long it usually takes.
 */
function meta(step: Step<Id>): string {
    const parts: string[] = [];
    switch (step.status) {
        case 'running': {
            const word = step.live ? 'live' : 'running';
            parts.push(
                runningSeconds === null ? word : `${word} · ${formatElapsed(runningSeconds)}`,
            );
            break;
        }
        case 'failed':
            parts.push('needs attention');
            break;
        case 'active':
            // Whose turn it is belongs to the phase header, said once; the row keeps the duration.
            if (step.expected) {
                parts.push(`usually ${step.expected}`);
            }
            break;
        default:
            break;
    }

    return parts.join(' · ');
}
</script>

<template>
    <div>
        <header class="flex items-center gap-3 px-2 pb-3">
            <span class="text-xs font-semibold text-zinc-900 dark:text-zinc-50">
                {{ done }} of {{ required.length }} done
            </span>
            <Meter
                class="max-w-40 flex-1"
                :value="required.length ? done / required.length : 0"
                :tone="
                    focus?.status === 'failed'
                        ? 'danger'
                        : done === required.length
                          ? 'ok'
                          : 'primary'
                "
            />
            <!-- Where the journey stands is a button while there is a step to go to. -->
            <Button
                v-if="focus"
                variant="ghost"
                size="sm"
                class="-my-1 min-w-0"
                :title="`Opens ${focus.title}`"
                @click="selected = focus.id"
            >
                <span
                    class="flex min-w-0 items-center gap-1.5"
                    :class="focus.status === 'failed' ? 'text-red-700 dark:text-red-400' : ''"
                >
                    <span class="truncate">{{ headline }}</span>
                    <ArrowRight class="h-3.5 w-3.5 shrink-0" />
                </span>
            </Button>
            <span v-else class="min-w-0 truncate text-xs text-zinc-500 dark:text-zinc-400">
                {{ headline }}
            </span>
        </header>

        <ol class="flex flex-col gap-1">
            <li v-for="group in groups" :key="group.phase">
                <button
                    type="button"
                    class="flex w-full items-center justify-between gap-3 rounded-md px-2 py-1.5 text-left hover:bg-zinc-100/70 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-zinc-700 dark:hover:bg-zinc-800/40 dark:focus-visible:outline-zinc-300"
                    :aria-expanded="isExpanded(group)"
                    @click="togglePhase(group)"
                >
                    <span class="flex min-w-0 items-center gap-2">
                        <component
                            :is="phaseIcon[group.phase]"
                            class="h-3.5 w-3.5 shrink-0"
                            :class="
                                group.complete
                                    ? 'text-emerald-700 dark:text-emerald-400'
                                    : 'text-zinc-500 dark:text-zinc-400'
                            "
                        />
                        <span
                            class="shrink-0 text-[13px] font-semibold text-zinc-700 dark:text-zinc-200"
                        >
                            {{ group.label }}
                        </span>
                        <span
                            v-if="!isExpanded(group) && summaries[group.phase]"
                            class="min-w-0 truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400"
                        >
                            {{ summaries[group.phase] }}
                        </span>
                    </span>
                    <span class="flex shrink-0 items-center gap-1.5">
                        <span
                            v-if="yoursRemaining(group.steps)"
                            class="text-[11px] text-zinc-500 dark:text-zinc-400"
                            v-tip="'Steps you do yourself; buildbridge checks the result'"
                        >
                            {{ yoursRemaining(group.steps) }} need you
                        </span>
                        <span
                            v-if="yoursRemaining(group.steps)"
                            class="text-[11px] text-zinc-400 dark:text-zinc-600"
                            aria-hidden="true"
                            >·</span
                        >
                        <!-- A phase made only of optional steps is not measured. -->
                        <span
                            v-if="group.steps.some((step) => !step.optional)"
                            class="text-[11px] tabular-nums"
                            :class="
                                group.focus?.status === 'failed'
                                    ? 'text-red-700 dark:text-red-400'
                                    : group.complete
                                      ? 'text-emerald-700 dark:text-emerald-400'
                                      : 'text-zinc-500 dark:text-zinc-400'
                            "
                        >
                            {{ group.done }} of {{ group.steps.length }}
                        </span>
                        <ChevronDown
                            class="h-3.5 w-3.5 text-zinc-500 transition-transform duration-200 motion-reduce:transition-none dark:text-zinc-400"
                            :class="isExpanded(group) ? '' : '-rotate-90'"
                        />
                    </span>
                </button>

                <ol v-if="isExpanded(group)" class="mt-0.5">
                    <li
                        v-for="(step, index) in group.steps"
                        :key="step.id"
                        :ref="(element) => keepRow(step.id, element)"
                        class="relative"
                    >
                        <!-- The connector runs behind the nodes, through an open step, and
                             stops at the last node of the phase. -->
                        <span
                            v-if="index < group.steps.length - 1"
                            class="absolute top-9 bottom-0 left-[19px] w-0.5 rounded-full transition-colors duration-500 motion-reduce:transition-none"
                            :class="
                                step.status === 'done'
                                    ? 'bg-emerald-500/45'
                                    : 'bg-zinc-300/70 dark:bg-zinc-600/70'
                            "
                            aria-hidden="true"
                        />
                        <button
                            type="button"
                            :class="
                                cn(
                                    'group/row relative flex w-full items-start gap-3 rounded-md px-2 py-2 text-left focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300',
                                    selected === step.id
                                        ? 'bg-zinc-100 dark:bg-zinc-800/60'
                                        : 'hover:bg-zinc-100/70 dark:hover:bg-zinc-800/40',
                                )
                            "
                            :aria-current="selected === step.id ? 'step' : undefined"
                            :aria-expanded="selected === step.id"
                            @click="selected = selected === step.id ? null : step.id"
                            @keydown.up.prevent.stop="move(step.id, -1)"
                            @keydown.down.prevent.stop="move(step.id, 1)"
                            @keydown.home.prevent.stop="move(step.id, -visible.length)"
                            @keydown.end.prevent.stop="move(step.id, visible.length)"
                        >
                            <span
                                :class="
                                    cn(
                                        'step-node relative z-10 flex h-6 w-6 shrink-0 items-center justify-center rounded-full border text-[11px] font-semibold tabular-nums transition-[background-color,border-color,box-shadow] duration-300 motion-reduce:transition-none',
                                        step.live ? liveNodeClass : nodeClass[step.status],
                                        step.live ? '' : haloClass[step.status],
                                    )
                                "
                                :data-status="step.status"
                            >
                                <Check v-if="step.status === 'done'" class="h-3.5 w-3.5" />
                                <X v-else-if="step.status === 'failed'" class="h-3.5 w-3.5" />
                                <span
                                    v-else-if="step.status === 'running' && step.live"
                                    class="h-2 w-2 animate-pulse rounded-full bg-emerald-500 motion-reduce:animate-none"
                                />
                                <Spinner v-else-if="step.status === 'running'" size="h-3.5 w-3.5" />
                                <template v-else>{{ steps.indexOf(step) + 1 }}</template>
                            </span>
                            <span class="min-w-0 flex-1">
                                <span
                                    class="block text-sm leading-5"
                                    :class="titleClass[step.status]"
                                >
                                    {{ step.title }}
                                </span>
                                <span
                                    class="mt-0.5 flex items-center gap-1.5 text-xs"
                                    :class="
                                        step.status === 'failed'
                                            ? 'text-red-700 dark:text-red-400'
                                            : 'text-zinc-500 dark:text-zinc-400'
                                    "
                                >
                                    <TriangleAlert
                                        v-if="step.status === 'failed'"
                                        class="h-3 w-3 shrink-0"
                                    />
                                    <span class="leading-5">{{ step.summary }}</span>
                                </span>
                            </span>
                            <span
                                v-if="meta(step)"
                                class="shrink-0 pt-0.5 text-[11px] whitespace-nowrap tabular-nums"
                                :class="
                                    step.live
                                        ? liveMetaClass
                                        : step.status === 'active' && step.kind === 'automatic'
                                          ? 'text-zinc-500 dark:text-zinc-400'
                                          : metaClass[step.status]
                                "
                            >
                                {{ meta(step) }}
                            </span>
                            <ChevronDown
                                class="mt-0.5 h-3.5 w-3.5 shrink-0 text-zinc-500 transition-transform duration-200 group-hover/row:opacity-100 group-focus-visible/row:opacity-100 motion-reduce:transition-none dark:text-zinc-400"
                                :class="
                                    selected === step.id ? 'opacity-100' : '-rotate-90 opacity-0'
                                "
                                aria-hidden="true"
                            />
                        </button>

                        <div v-if="selected === step.id" class="pt-1 pb-3 pl-11">
                            <div
                                class="rounded-lg border border-zinc-200 bg-white p-4 dark:border-zinc-800 dark:bg-zinc-900"
                            >
                                <p
                                    class="mb-3 text-[11px] leading-4 text-zinc-500 dark:text-zinc-400"
                                >
                                    <span
                                        class="cursor-help font-medium text-zinc-600 underline decoration-zinc-300 decoration-dotted underline-offset-2 dark:text-zinc-300 dark:decoration-zinc-600"
                                        v-tip="stepKindDescription[step.kind]"
                                        >{{ stepKindLabel[step.kind] }}</span
                                    >
                                    <Badge
                                        v-if="step.experimental"
                                        tone="outline"
                                        class="ml-2 align-middle"
                                        v-tip="
                                            'Self-hosted and experimental: USB passthrough into a QEMU guest is not a route Apple supports.'
                                        "
                                        >experimental</Badge
                                    >
                                </p>
                                <slot name="detail" :step="step" />
                            </div>
                        </div>
                    </li>
                </ol>
            </li>
        </ol>
    </div>
</template>

<style scoped>
/* A node that just completed pops once; nothing else in the timeline moves on its own. */
@keyframes node-in {
    from {
        transform: scale(0.7);
    }
    60% {
        transform: scale(1.12);
    }
    to {
        transform: scale(1);
    }
}
.step-node[data-status='done'] {
    animation: node-in 0.28s ease-out;
}
@media (prefers-reduced-motion: reduce) {
    .step-node[data-status='done'] {
        animation: none;
    }
}
</style>
