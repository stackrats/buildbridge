<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted } from 'vue';

import HomePane from './components/home/HomePane.vue';
import NativeMacPane from './components/native/NativeMacPane.vue';
import { useNativeMacStore } from './stores/native-mac';
import MachinePane from './components/machine/MachinePane.vue';
import RunnerPane from './components/runner/RunnerPane.vue';
import Sidebar from './components/Sidebar.vue';
import SigningKitPane from './components/signing/SigningKitPane.vue';
import EnvSetPane from './components/envs/EnvSetPane.vue';
import TemplatesPane from './components/templates/TemplatesPane.vue';
import TooltipLayer from './components/ui/TooltipLayer.vue';
import TopBar from './components/TopBar.vue';
import NewMachineDialog from './components/dialogs/NewMachineDialog.vue';
import SettingsDialog from './components/dialogs/SettingsDialog.vue';
import { useEnvSetsStore } from './stores/envs';
import { useMachinesStore } from './stores/machines';
import { useRunnerStore } from './stores/runner';
import { useSettingsStore } from './stores/settings';
import { useSigningStore } from './stores/signing';
import { useUi } from './stores/ui';
import { useUsageStore } from './stores/usage';

const ui = useUi();
const runner = useRunnerStore();
const machines = useMachinesStore();
const signing = useSigningStore();
const envs = useEnvSetsStore();
const nativeMac = useNativeMacStore();
const usage = useUsageStore();
const settings = useSettingsStore();
/** Off until the settings say otherwise, so the pane never flashes up during the first load. */
const remoteBuilds = settings.remoteBuilds;

// Measuring what the machines cost is worth nothing while nobody can see it, so the engine
// samples only while this window is showing.
function syncUsageToVisibility(): void {
    void usage.watch(!document.hidden);
}

const selectedMachineId = computed(() =>
    ui.state.route.kind === 'machine' ? ui.state.route.id : null,
);

onMounted(async () => {
    // The settings come first because they say whether this host offers remote builds at all,
    // and the runner store, the sidebar and the route all read that answer.
    await Promise.all([machines.listenForEvents(), usage.listen(), settings.load()]);
    syncUsageToVisibility();
    document.addEventListener('visibilitychange', syncUsageToVisibility);
    // A remembered remote-builds page on a host that no longer offers them falls back too.
    if (!settings.remoteBuilds.value && ui.state.route.kind === 'runner') {
        ui.navigate({ kind: 'home' });
    }
    await Promise.all([
        runner.initialize(settings.remoteBuilds.value),
        machines.loadList(),
        signing.load(),
        envs.load(),
    ]);
    if (runner.state.status?.platform === 'macos') await nativeMac.initialize();
    // A remembered machine that no longer exists falls back to the overview.
    if (
        selectedMachineId.value !== null &&
        !machines.machines.value.some((machine) => machine.id === selectedMachineId.value)
    ) {
        ui.navigate({ kind: 'home' });
    }
});

onBeforeUnmount(() => {
    document.removeEventListener('visibilitychange', syncUsageToVisibility);
    void usage.watch(false);
    usage.dispose();
    runner.dispose();
    machines.dispose();
    nativeMac.dispose();
});
</script>

<template>
    <div class="flex h-screen flex-col bg-zinc-50 text-zinc-900 dark:bg-zinc-950 dark:text-zinc-50">
        <TopBar />
        <div class="flex min-h-0 flex-1">
            <Sidebar />
            <!-- A machine page manages its own scrolling so its log drawer can stay in view. Scroll
                 containers are positioned so hidden helper elements (sr-only) scroll with their pane
                 instead of stretching the window, which focusing one would then scroll. -->
            <main
                class="relative min-w-0 flex-1"
                :class="selectedMachineId !== null ? 'overflow-hidden' : 'overflow-y-auto'"
            >
                <MachinePane
                    v-if="selectedMachineId !== null"
                    :key="selectedMachineId"
                    :machine-id="selectedMachineId"
                />
                <RunnerPane v-else-if="ui.state.route.kind === 'runner' && remoteBuilds" />
                <NativeMacPane v-else-if="ui.state.route.kind === 'native_mac'" />
                <SigningKitPane v-else-if="ui.state.route.kind === 'signing'" />
                <EnvSetPane v-else-if="ui.state.route.kind === 'envs'" />
                <TemplatesPane v-else-if="ui.state.route.kind === 'templates'" />
                <HomePane v-else />
            </main>
        </div>
        <NewMachineDialog v-model:open="ui.state.newMachineOpen" />
        <SettingsDialog v-model:open="ui.state.settingsOpen" />
        <TooltipLayer />
    </div>
</template>
