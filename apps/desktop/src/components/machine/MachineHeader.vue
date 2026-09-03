<script setup lang="ts">
// The page header, unboxed: the name, one state pill, one action, and a line of facts. The
// facts that used to be chips live in the rail now, each on the step that produced it.
import { EllipsisVertical, Pencil, Play, RefreshCw, Square, Trash2, HardDrive } from '@lucide/vue';
import { computed, onBeforeUnmount, ref } from 'vue';

import { formatElapsed, secondsSince } from '../../lib/format';
import { isLive, machineStateBadge, machineStateLabel } from '../../lib/status';
import { busyKeyLabel, useMachinesStore, type MachineSession } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import Spinner from '../ui/Spinner.vue';
import ConfirmDialog from '../dialogs/ConfirmDialog.vue';
import EditMachineDialog from '../dialogs/EditMachineDialog.vue';

const { session, now } = defineProps<{ session: MachineSession; now: number }>();
const machines = useMachinesStore();
const ui = useUi();

const view = computed(() => session.view!);
const busy = computed(() => session.operation !== null || view.value.busyOperation !== null);
const busyLabel = computed(() =>
    view.value.busyOperation
        ? (busyKeyLabel[view.value.busyOperation] ?? view.value.busyOperation)
        : null,
);
const live = computed(() => isLive(view.value.runtime.state));
const canStart = computed(
    () => !busy.value && !live.value && view.value.runtime.prerequisites.ready,
);

const facts = computed(() => {
    const uptime = secondsSince(view.value.runtime.startedAt, now);
    const { diagnostics } = view.value.guest;
    return [
        `${view.value.profile.memoryGib} GiB`,
        `${view.value.profile.cpuCores} cores`,
        `ssh 127.0.0.1:${view.value.profile.sshPort}`,
        uptime === null ? null : `up ${formatElapsed(uptime)}`,
        diagnostics.macosVersion ? `macOS ${diagnostics.macosVersion}` : null,
        diagnostics.xcodeVersion ? `Xcode ${diagnostics.xcodeVersion}` : null,
    ]
        .filter(Boolean)
        .join(' · ');
});

const menuOpen = ref(false);
const editOpen = ref(false);
const discardOpen = ref(false);
const deleteOpen = ref(false);
const deleting = ref(false);

function closeMenu(): void {
    menuOpen.value = false;
}

function onWindowClick(): void {
    closeMenu();
}

function toggleMenu(event: MouseEvent): void {
    event.stopPropagation();
    menuOpen.value = !menuOpen.value;
    if (menuOpen.value) {
        window.addEventListener('click', onWindowClick, { once: true });
    }
}

onBeforeUnmount(() => window.removeEventListener('click', onWindowClick));

async function discard(): Promise<void> {
    discardOpen.value = false;
    await machines.discardContainer(session.id);
}

async function remove(): Promise<void> {
    deleting.value = true;
    const removed = await machines.deleteMachine(session.id);
    deleting.value = false;
    deleteOpen.value = false;
    if (removed) {
        ui.navigate({ kind: 'home' });
    }
}

const menuItemClass =
    'flex w-full cursor-default items-center gap-2 rounded px-2 py-1.5 text-left text-xs text-zinc-600 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300';
</script>

<template>
    <header class="flex items-start justify-between gap-3">
        <div class="min-w-0">
            <div class="flex flex-wrap items-center gap-2">
                <h1 class="truncate text-lg font-bold text-zinc-900 dark:text-zinc-50">
                    {{ view.profile.name }}
                </h1>
                <Badge v-if="busyLabel" tone="warn">
                    <Spinner size="h-3 w-3" tone="text-amber-700 dark:text-amber-400" />
                    {{ busyLabel }}
                </Badge>
                <Badge v-else :tone="machineStateBadge[view.runtime.state]">
                    {{ machineStateLabel[view.runtime.state] }}
                </Badge>
            </div>
            <p class="mt-1 font-mono text-[11px] text-zinc-500 dark:text-zinc-400">{{ facts }}</p>
        </div>
        <div class="flex shrink-0 items-center gap-1.5">
            <Button
                v-if="live"
                variant="outline"
                size="sm"
                :disabled="busy"
                @click="machines.stop(session.id)"
            >
                <Spinner v-if="session.operation === 'stop'" />
                <Square v-else class="h-3.5 w-3.5 text-red-700 dark:text-red-400" />
                Stop
            </Button>
            <Button v-else size="sm" :disabled="!canStart" @click="machines.launch(session.id)">
                <Spinner
                    v-if="session.operation === 'launch'"
                    tone="text-white dark:text-zinc-950"
                />
                <Play v-else class="h-3.5 w-3.5" />
                {{ view.runtime.state === 'missing' ? 'Create and start' : 'Start' }}
            </Button>
            <Button
                variant="ghost"
                size="icon"
                title="Refresh"
                :disabled="session.operation !== null || session.refreshing"
                @click="machines.refreshMachine(session.id)"
            >
                <RefreshCw
                    class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400"
                    :class="{ 'animate-spin': session.loading || session.refreshing }"
                />
            </Button>
            <div class="relative">
                <Button variant="ghost" size="icon" title="More" @click="toggleMenu">
                    <EllipsisVertical class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" />
                </Button>
                <div
                    v-if="menuOpen"
                    class="absolute top-full right-0 z-30 mt-1 min-w-52 rounded-md border border-zinc-200 bg-white p-1 shadow-lg dark:border-zinc-800 dark:bg-zinc-900"
                    @click.stop
                >
                    <button
                        type="button"
                        :class="menuItemClass"
                        @click="
                            closeMenu();
                            editOpen = true;
                        "
                    >
                        <Pencil class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" />
                        Machine profile
                    </button>
                    <div class="my-1 h-px bg-zinc-200 dark:bg-zinc-700" />
                    <button
                        type="button"
                        :class="menuItemClass"
                        :disabled="live || view.runtime.state === 'missing'"
                        class="disabled:opacity-40"
                        @click="
                            closeMenu();
                            discardOpen = true;
                        "
                    >
                        <HardDrive class="h-3.5 w-3.5 text-amber-700 dark:text-amber-400" />
                        Discard container and disk
                    </button>
                    <button
                        type="button"
                        :class="menuItemClass"
                        :disabled="live"
                        class="disabled:opacity-40"
                        @click="
                            closeMenu();
                            deleteOpen = true;
                        "
                    >
                        <Trash2 class="h-3.5 w-3.5 text-red-700 dark:text-red-400" /> Delete machine
                    </button>
                </div>
            </div>
        </div>

        <EditMachineDialog v-model:open="editOpen" :view="view" />

        <ConfirmDialog
            v-model:open="discardOpen"
            title="Discard the container and its macOS disk"
            confirm-label="Discard container"
            acknowledgement="I understand the macOS installation, Xcode, and everything inside the guest will be deleted"
            @confirm="discard"
        >
            <p>
                The Docker container for <b>{{ view.profile.name }}</b> is removed together with the
                macOS disk stored inside it. The next start creates a fresh container, and macOS
                must be installed again from the console.
            </p>
            <p>
                The pinned SSH identity and the provisioned signing record are cleared because they
                belong to the old guest. The machine profile, its access key, the approved project,
                and retained artifacts on this host are kept.
            </p>
        </ConfirmDialog>

        <ConfirmDialog
            v-model:open="deleteOpen"
            title="Delete this machine"
            confirm-label="Delete machine"
            acknowledgement="I understand the container, macOS disk, access key, and retained artifacts will be deleted"
            :busy="deleting"
            @confirm="remove"
        >
            <p>
                <b>{{ view.profile.name }}</b> is removed from BuildBridge: its container and macOS
                disk, its SSH access key and identity pin, the approved project record, and any
                retained IPA or archive on this host.
            </p>
            <p>
                The host project folder and the signing kit in the operating-system vault are not
                touched.
            </p>
        </ConfirmDialog>
    </header>
</template>
