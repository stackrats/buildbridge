<script setup lang="ts">
// The page header, unboxed: the name, one state pill, one action, and a line of facts. The
// facts that used to be chips live in the rail now, each on the step that produced it.
import {
    EllipsisVertical,
    ExternalLink,
    HardDrive,
    Layers,
    Pencil,
    Play,
    RefreshCw,
    Square,
    Trash2,
} from '@lucide/vue';
import { computed, onBeforeUnmount, ref } from 'vue';

import { formatElapsed, secondsSince } from '../../lib/format';
import { isAndroid, providerLabel } from '../../model/providers';
import { isLive, machineStateBadge, machineStateLabel } from '../../lib/status';
import { activityLabel, useMachinesStore, type MachineSession } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import Field from '../ui/Field.vue';
import Input from '../ui/Input.vue';
import Spinner from '../ui/Spinner.vue';
import ConfirmDialog from '../dialogs/ConfirmDialog.vue';
import EditMachineDialog from '../dialogs/EditMachineDialog.vue';
import MachineUsage from './MachineUsage.vue';

const { session, now } = defineProps<{ session: MachineSession; now: number }>();
const machines = useMachinesStore();
const ui = useUi();

const view = computed(() => session.view!);
const busy = computed(() => session.operation !== null || view.value.busyOperation !== null);
// A template save, this client's or one another buildbridge process holds the machine for; its
// percentage joins the busy badge, since the save's dialog is gone while it runs.
const savingTemplate = computed(
    () => session.operation === 'save-template' || view.value.busyOperation === 'saving_template',
);
const templateSavePercent = computed(() =>
    savingTemplate.value ? (session.templateSave?.percent ?? null) : null,
);
// The native busy key is the truth once the view refreshes; until then the operation this
// client started names what is happening, so a machine being stopped never reads as running.
const busyLabel = computed(() => {
    const label =
        activityLabel(view.value.busyOperation) ??
        view.value.busyOperation ??
        activityLabel(session.operation);
    return label && templateSavePercent.value !== null
        ? `${label} · ${templateSavePercent.value}%`
        : label;
});
const live = computed(() => isLive(view.value.runtime.state));
const canStart = computed(
    () => !busy.value && !live.value && view.value.runtime.prerequisites.ready,
);

const android = computed(() => isAndroid(view.value.profile.provider));

const facts = computed(() => {
    const uptime = secondsSince(view.value.runtime.startedAt, now);
    const { diagnostics } = view.value.guest;
    if (android.value) {
        const lastBuild = view.value.android?.workspace?.lastBuild ?? null;
        return [
            providerLabel[view.value.profile.provider],
            `${view.value.profile.memoryGib} GiB limit`,
            `${view.value.profile.cpuCores} cores`,
            lastBuild
                ? lastBuild.toolchain.jdkVersion
                      .replace(/^openjdk version /, 'JDK ')
                      .replace(/"/g, '')
                      .split(' ')
                      .slice(0, 2)
                      .join(' ')
                : null,
            lastBuild ? `build tools ${lastBuild.toolchain.buildToolsVersion}` : null,
            // Last, so a figure that changes every minute reflows nothing before it.
            uptime === null ? null : `up ${formatElapsed(uptime)}`,
        ]
            .filter(Boolean)
            .join(' · ');
    }
    return [
        providerLabel[view.value.profile.provider],
        `${view.value.profile.memoryGib} GiB`,
        `${view.value.profile.cpuCores} cores`,
        `ssh 127.0.0.1:${view.value.profile.sshPort}`,
        view.value.displayUrl
            ? `screen ${view.value.displayUrl.replace(/^https?:\/\//, '').replace(/\/$/, '')}`
            : null,
        diagnostics.macosVersion ? `macOS ${diagnostics.macosVersion}` : null,
        diagnostics.xcodeVersion ? `Xcode ${diagnostics.xcodeVersion}` : null,
        uptime === null ? null : `up ${formatElapsed(uptime)}`,
    ]
        .filter(Boolean)
        .join(' · ');
});

const menuOpen = ref(false);
const editOpen = ref(false);
const discardOpen = ref(false);
const deleteOpen = ref(false);
const deleting = ref(false);
const templateOpen = ref(false);
const templateName = ref('');

// A template carries the pinned identity and the access key, so both must exist; a machine
// keeping its disk inside the container has nothing on this host to copy.
const templateBlocker = computed(() => {
    if (android.value) {
        return 'Templates are for macOS machines; a toolchain has no disk to save';
    }
    if (!view.value.guest.ssh.pinnedFingerprint) {
        return 'Pin the guest identity first';
    }
    if (!view.value.guest.username) {
        return 'Authorize the buildbridge key first';
    }
    if (!view.value.usb.diskOnHost) {
        return 'Enable USB on this machine first; that moves its disk to this host';
    }
    return null;
});
function openTemplateDialog(): void {
    templateName.value = view.value.guest.diagnostics.xcodeVersion
        ? `Xcode ${view.value.guest.diagnostics.xcodeVersion} ready`
        : `${view.value.profile.name} template`;
    templateOpen.value = true;
}

// The save runs on its own for minutes, so the dialog closes as soon as it starts; the
// progress shows on the launch step, on the Templates page and in the sidebar, and a failure
// lands on this page like any other operation's.
function saveTemplate(): void {
    const name = templateName.value.trim();
    if (!name) {
        return;
    }
    templateOpen.value = false;
    void machines.saveTemplate(session.id, name);
}

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

// The dialog stays up with its button spinning until the container is gone: removing it takes
// a few seconds, and nothing else on the page is open to say so.
async function discard(): Promise<void> {
    await machines.discardContainer(session.id);
    discardOpen.value = false;
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
                <Badge v-if="busyLabel" tone="warn" class="tabular-nums">
                    <Spinner size="h-3 w-3" />
                    {{ busyLabel }}
                </Badge>
                <Badge v-if="!busyLabel" :tone="machineStateBadge[view.runtime.state]">
                    {{ machineStateLabel[view.runtime.state] }}
                </Badge>
            </div>
            <p class="mt-1 font-mono text-[11px] text-zinc-500 dark:text-zinc-400">{{ facts }}</p>
            <MachineUsage :machine-id="session.id" :running="view.runtime.state === 'running'" />
        </div>
        <div class="flex shrink-0 items-center gap-1.5">
            <Button
                v-if="view.displayUrl && live"
                variant="outline"
                size="sm"
                title="Opens the machine's screen in its own window; any browser on this host can open the same address"
                @click="machines.openMachineScreen(session.id)"
            >
                <ExternalLink class="h-3.5 w-3.5" />
                Screen
            </Button>
            <Button
                v-if="live"
                variant="outline"
                size="sm"
                :disabled="busy"
                @click="machines.stop(session.id)"
            >
                <Spinner v-if="session.operation === 'stop'" />
                <Square v-else class="h-3.5 w-3.5" />
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
                size="iconSm"
                title="Reads the machine's state again"
                :disabled="session.operation !== null || session.refreshing"
                @click="machines.refreshMachine(session.id)"
            >
                <Spinner v-if="session.loading || session.refreshing" />
                <RefreshCw v-else class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" />
            </Button>
            <div class="relative">
                <Button
                    variant="ghost"
                    size="iconSm"
                    title="More actions: profile, template, discard, delete"
                    aria-label="More actions"
                    @click="toggleMenu"
                >
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
                    <button
                        type="button"
                        :class="menuItemClass"
                        :disabled="busy || templateBlocker !== null"
                        class="disabled:opacity-45"
                        v-tip="
                            savingTemplate
                                ? 'The save is running; its progress is on the Launch step and the Templates page'
                                : (templateBlocker ??
                                  'Saves this machine\'s disk as a template new machines clone in seconds')
                        "
                        @click="
                            closeMenu();
                            openTemplateDialog();
                        "
                    >
                        <Spinner v-if="savingTemplate" size="h-3.5 w-3.5" />
                        <Layers v-else class="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" />
                        <template v-if="savingTemplate">
                            <span class="tabular-nums"
                                >Saving template{{
                                    templateSavePercent !== null ? ` · ${templateSavePercent}%` : ''
                                }}</span
                            >
                        </template>
                        <template v-else>Save as template</template>
                    </button>
                    <div class="my-1 h-px bg-zinc-200 dark:bg-zinc-800" />
                    <button
                        type="button"
                        :class="menuItemClass"
                        :disabled="live || view.runtime.state === 'missing'"
                        class="disabled:opacity-45"
                        v-tip="
                            live
                                ? 'Stop the machine first'
                                : view.runtime.state === 'missing'
                                  ? 'There is no container to discard yet'
                                  : android
                                    ? 'Removes the container, the SDK, the caches and the synchronized project; the machine profile stays'
                                    : 'Removes the container and its macOS disk; the machine profile stays'
                        "
                        @click="
                            closeMenu();
                            discardOpen = true;
                        "
                    >
                        <HardDrive class="h-3.5 w-3.5 text-amber-700 dark:text-amber-400" />
                        {{
                            android
                                ? 'Discard container and toolchain'
                                : 'Discard container and disk'
                        }}
                    </button>
                    <button
                        type="button"
                        :class="menuItemClass"
                        :disabled="live"
                        class="disabled:opacity-45"
                        v-tip="
                            live
                                ? 'Stop the machine first'
                                : 'Removes the machine and everything it owns on this host'
                        "
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
            v-model:open="templateOpen"
            title="Save this machine as a template"
            confirm-label="Save template"
            :destructive="false"
            :confirm-disabled="!templateName.trim()"
            @confirm="saveTemplate"
        >
            <p>
                macOS is asked to shut down, the disk is compressed into a template on this host (a
                few gigabytes per ten on the disk, taking some minutes), and the machine stays
                stopped afterwards. New machines cloned from it start with macOS, Xcode and their
                access in place, and begin at the first project step.
            </p>
            <p>
                The template carries this machine's access key and pinned identity so a clone needs
                no console or password; it is stored owner-only and retired from each clone once the
                clone has its own key. Nothing leaves this host.
            </p>
            <p>
                The save runs in the background: this dialog closes, and the progress shows on the
                Launch step, on the Templates page and in the sidebar, where it can be stopped.
            </p>
            <Field label="Template name" required>
                <Input
                    v-model="templateName"
                    placeholder="A name for this template"
                    :maxlength="60"
                />
            </Field>
        </ConfirmDialog>

        <ConfirmDialog
            v-if="android"
            v-model:open="discardOpen"
            title="Discard the container and its toolchain"
            confirm-label="Discard container"
            acknowledgement="I understand the SDK, the Gradle caches, and the synchronized project will be deleted"
            :busy="session.operation === 'discard'"
            @confirm="discard"
        >
            <p>
                The container for <b>{{ view.profile.name }}</b> is removed together with its home
                on this host: the Android SDK and tools it downloaded, the Gradle caches, and the
                synchronized project. The next start creates a fresh container, and the first build
                downloads the toolchain again.
            </p>
            <p>
                The machine profile, the approved project record, the attached signing credentials,
                and retained artifacts on this host are kept.
            </p>
        </ConfirmDialog>
        <ConfirmDialog
            v-else
            v-model:open="discardOpen"
            title="Discard the container and its macOS disk"
            confirm-label="Discard container"
            acknowledgement="I understand the macOS installation, Xcode, and everything inside the guest will be deleted"
            :busy="session.operation === 'discard'"
            @confirm="discard"
        >
            <p>
                The Docker container for <b>{{ view.profile.name }}</b> is removed together with the
                macOS disk stored inside it. The next start creates a fresh container, and macOS
                must be installed again from the console.
            </p>
            <p v-if="view.template">
                This machine was cloned from the template <b>{{ view.template.name }}</b
                >, so the next start clones it again in seconds instead: this is how a clone is
                reset.
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
            :acknowledgement="
                android
                    ? 'I understand the container, its toolchain, and retained artifacts will be deleted'
                    : 'I understand the container, macOS disk, access key, and retained artifacts will be deleted'
            "
            :busy="deleting"
            @confirm="remove"
        >
            <p v-if="android">
                <b>{{ view.profile.name }}</b> is removed from buildbridge: its container and the
                home with the SDK and caches, the approved project record, and any retained bundle
                or APK on this host.
            </p>
            <p v-else>
                <b>{{ view.profile.name }}</b> is removed from buildbridge: its container and macOS
                disk, its SSH access key and identity pin, the approved project record, and any
                retained IPA or archive on this host.
            </p>
            <p>
                The host project folder and the signing credentials in the operating-system vault
                are not touched.
            </p>
        </ConfirmDialog>
    </header>
</template>
