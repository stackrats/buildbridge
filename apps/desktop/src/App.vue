<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted } from 'vue';

import HomePane from './components/home/HomePane.vue';
import MachinePane from './components/machine/MachinePane.vue';
import RunnerPane from './components/runner/RunnerPane.vue';
import Sidebar from './components/Sidebar.vue';
import SigningKitPane from './components/signing/SigningKitPane.vue';
import EnvSetPane from './components/envs/EnvSetPane.vue';
import TooltipLayer from './components/ui/TooltipLayer.vue';
import TopBar from './components/TopBar.vue';
import NewMachineDialog from './components/dialogs/NewMachineDialog.vue';
import { useEnvSetsStore } from './stores/envs';
import { useMachinesStore } from './stores/machines';
import { useRunnerStore } from './stores/runner';
import { useSigningStore } from './stores/signing';
import { useUi } from './stores/ui';

const ui = useUi();
const runner = useRunnerStore();
const machines = useMachinesStore();
const signing = useSigningStore();
const envs = useEnvSetsStore();

const selectedMachineId = computed(() =>
    ui.state.route.kind === 'machine' ? ui.state.route.id : null,
);

onMounted(async () => {
    await machines.listenForEvents();
    await Promise.all([runner.initialize(), machines.loadList(), signing.load(), envs.load()]);
    // A remembered machine that no longer exists falls back to the overview.
    if (
        selectedMachineId.value !== null &&
        !machines.machines.value.some((machine) => machine.id === selectedMachineId.value)
    ) {
        ui.navigate({ kind: 'home' });
    }
});

onBeforeUnmount(() => {
    runner.dispose();
    machines.dispose();
});
</script>

<template>
    <div class="flex h-screen flex-col bg-zinc-50 text-zinc-900 dark:bg-zinc-950 dark:text-zinc-50">
        <TopBar />
        <div class="flex min-h-0 flex-1">
            <Sidebar />
            <!-- A machine page manages its own scrolling so its log drawer can stay in view. -->
            <main
                class="min-w-0 flex-1"
                :class="selectedMachineId !== null ? 'overflow-hidden' : 'overflow-y-auto'"
            >
                <MachinePane
                    v-if="selectedMachineId !== null"
                    :key="selectedMachineId"
                    :machine-id="selectedMachineId"
                />
                <RunnerPane v-else-if="ui.state.route.kind === 'runner'" />
                <SigningKitPane v-else-if="ui.state.route.kind === 'signing'" />
                <EnvSetPane v-else-if="ui.state.route.kind === 'envs'" />
                <HomePane v-else />
            </main>
        </div>
        <NewMachineDialog v-model:open="ui.state.newMachineOpen" />
        <TooltipLayer />
    </div>
</template>
