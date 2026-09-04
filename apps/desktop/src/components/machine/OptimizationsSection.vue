<script setup lang="ts">
// The osx-optimizer catalogue for this machine's guest, in three tiers that keep the source's
// own words for how much each one gives up. Every item shows whether the guest already has it;
// applying one is a button, and the riskier the tier the more the button asks first.
import { ChevronDown, RefreshCw, Terminal, Zap } from '@lucide/vue';
import { computed, onMounted, ref, watch } from 'vue';

import { useMachinesStore, type MachineSession } from '../../stores/machines';
import type { GuestOptimizationView, OptimizationTier } from '../../types/backend';
import ConfirmDialog from '../dialogs/ConfirmDialog.vue';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Spinner from '../ui/Spinner.vue';

const { session } = defineProps<{ session: MachineSession }>();
const machines = useMachinesStore();

const open = ref(false);
const pending = ref<GuestOptimizationView | null>(null);
const applyingId = ref<string | null>(null);

const view = computed(() => session.optimizations);
const busy = computed(() => session.operation !== null);
const guestReady = computed(
    () =>
        session.view?.runtime.state === 'running' &&
        session.view.guest.ssh.trust === 'trusted' &&
        session.view.guest.diagnostics.authenticated,
);

const tiers: { tier: OptimizationTier; label: string; note: string; tone: string }[] = [
    {
        tier: 'recommended',
        label: 'Recommended for a build machine',
        note: 'Faster builds; nothing about who can do what on the machine changes.',
        tone: 'text-zinc-600 dark:text-zinc-300',
    },
    {
        tier: 'at_your_own_risk',
        label: 'At your own risk',
        note: 'Each trades some protection for convenience. The guest only listens on this host, which is why they are offered at all.',
        tone: 'text-amber-700 dark:text-amber-400',
    },
    {
        tier: 'extremely_insecure',
        label: 'Extremely insecure',
        note: 'In the source’s words: these should only be used in CI/CD, behind a VPN, and with no external connectivity. This is not a warning, it is absolutely essential.',
        tone: 'text-red-700 dark:text-red-400',
    },
];

const groups = computed(() =>
    tiers
        .map((entry) => ({
            ...entry,
            items: (view.value?.items ?? []).filter((item) => item.tier === entry.tier),
        }))
        .filter((entry) => entry.items.length > 0),
);

const appliedCount = computed(
    () => (view.value?.items ?? []).filter((item) => item.applied === true).length,
);
const total = computed(() => view.value?.items.length ?? 0);

function load(): void {
    void machines.loadOptimizations(session.id);
}

onMounted(load);
// A guest that just became reachable can now be asked; a guest that went away cannot.
watch(guestReady, load);

function request(item: GuestOptimizationView): void {
    if (item.tier === 'recommended') {
        void apply(item);
        return;
    }
    pending.value = item;
}

async function apply(item: GuestOptimizationView): Promise<void> {
    pending.value = null;
    applyingId.value = item.id;
    try {
        await machines.applyOptimization(session.id, item.id, item.title);
    } finally {
        applyingId.value = null;
    }
}

const stateBadge = (item: GuestOptimizationView) =>
    item.applied === true
        ? { tone: 'ok' as const, label: 'applied' }
        : item.applied === false
          ? { tone: 'neutral' as const, label: 'not applied' }
          : { tone: 'neutral' as const, label: 'unknown' };
</script>

<template>
    <section class="mt-5">
        <button
            type="button"
            class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-zinc-100/70 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-zinc-700 dark:hover:bg-zinc-800/40 dark:focus-visible:outline-zinc-300"
            :aria-expanded="open"
            @click="open = !open"
        >
            <Zap class="h-3.5 w-3.5 shrink-0 text-zinc-500 dark:text-zinc-400" />
            <span class="text-[13px] font-semibold text-zinc-700 dark:text-zinc-200">
                Guest optimizations
            </span>
            <span class="min-w-0 flex-1 truncate text-[11px] text-zinc-500 dark:text-zinc-400">
                <template v-if="view?.available"
                    >{{ appliedCount }} of {{ total }} applied</template
                >
                <template v-else>tweaks from osx-optimizer for a faster virtual machine</template>
            </span>
            <ChevronDown
                class="h-3.5 w-3.5 shrink-0 text-zinc-500 transition-transform duration-200 motion-reduce:transition-none dark:text-zinc-400"
                :class="open ? '' : '-rotate-90'"
            />
        </button>

        <div v-if="open" class="mt-2 space-y-4 px-2">
            <div class="flex flex-wrap items-start justify-between gap-3">
                <p class="max-w-2xl text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                    macOS defaults from
                    <span class="font-mono">sickcodes/osx-optimizer</span>, each run as a fixed
                    script over the pinned bridge. Anything needing an administrator opens the
                    guest's Terminal, where sudo reads the password; BuildBridge never sees it. Each
                    item says what it costs, in the source's words.
                </p>
                <Button
                    variant="outline"
                    size="sm"
                    :disabled="session.optimizationsLoading || busy"
                    @click="load"
                >
                    <Spinner v-if="session.optimizationsLoading" />
                    <RefreshCw v-else class="h-3.5 w-3.5" />
                    Check again
                </Button>
            </div>

            <Callout v-if="view && !view.available" tone="neutral">
                {{ view.reason ?? 'The guest cannot be asked right now.' }}
            </Callout>

            <div v-for="group in groups" :key="group.tier">
                <h3 class="text-xs font-semibold" :class="group.tone">{{ group.label }}</h3>
                <p class="mt-0.5 text-[11px] leading-4 text-zinc-500 dark:text-zinc-400">
                    {{ group.note }}
                </p>
                <ul class="mt-2 divide-y divide-zinc-200 dark:divide-zinc-800">
                    <li
                        v-for="item in group.items"
                        :key="item.id"
                        class="flex items-start gap-3 py-2.5"
                    >
                        <div class="min-w-0 flex-1">
                            <p
                                class="flex flex-wrap items-center gap-2 text-xs font-medium text-zinc-900 dark:text-zinc-50"
                            >
                                {{ item.title }}
                                <Badge :tone="stateBadge(item).tone">
                                    {{ stateBadge(item).label }}
                                </Badge>
                            </p>
                            <p class="mt-0.5 text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                                {{ item.summary }}
                            </p>
                            <p
                                v-if="item.warning"
                                class="mt-0.5 text-[11px] leading-4"
                                :class="
                                    item.tier === 'recommended'
                                        ? 'text-zinc-500 dark:text-zinc-400'
                                        : group.tone
                                "
                            >
                                {{ item.warning }}
                            </p>
                        </div>
                        <Button
                            :variant="item.tier === 'extremely_insecure' ? 'danger' : 'outline'"
                            size="sm"
                            class="shrink-0"
                            :title="
                                item.needsAdmin ? 'sudo asks for the password there' : undefined
                            "
                            :disabled="busy || !view?.available || item.applied === true"
                            @click="request(item)"
                        >
                            <Spinner v-if="applyingId === item.id" />
                            <Terminal v-else-if="item.needsAdmin" class="h-3.5 w-3.5" />
                            <Zap v-else class="h-3.5 w-3.5" />
                            {{ item.needsAdmin ? 'Run in guest Terminal' : 'Apply' }}
                        </Button>
                    </li>
                </ul>
            </div>
        </div>

        <ConfirmDialog
            :open="pending !== null"
            :title="pending?.title ?? ''"
            :confirm-label="pending?.needsAdmin ? 'Open guest Terminal' : 'Apply'"
            :destructive="pending?.tier === 'extremely_insecure'"
            :acknowledgement="
                pending?.tier === 'extremely_insecure'
                    ? 'I understand this removes authentication inside the guest and is only defensible for a machine nothing else can reach'
                    : null
            "
            @update:open="(value) => (pending = value ? pending : null)"
            @confirm="pending && apply(pending)"
        >
            <p>{{ pending?.summary }}</p>
            <p
                v-if="pending?.warning"
                :class="
                    pending?.tier === 'extremely_insecure'
                        ? 'text-red-700 dark:text-red-400'
                        : 'text-amber-700 dark:text-amber-400'
                "
            >
                {{ pending?.warning }}
            </p>
            <p v-if="pending?.needsAdmin">
                A Terminal window opens inside the guest and sudo asks for the macOS password there.
                The password stays in that window.
            </p>
        </ConfirmDialog>
    </section>
</template>
