<script setup lang="ts">
// The overview is a snapshot: every machine as a row with its journey and its next action,
// then the two shared resources as quiet wells. With no machine yet it is a welcome screen
// that says the three phases and offers one button.
import { ArrowRight, KeyRound, Plus, Radio } from '@lucide/vue';
import { computed, onMounted, watch } from 'vue';

import { useMachinesStore } from '../../stores/machines';
import { useRunnerStore } from '../../stores/runner';
import { kitIsProvisionable, useSigningStore } from '../../stores/signing';
import { useUi } from '../../stores/ui';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import MachineRow from './MachineRow.vue';

const ui = useUi();
const machines = useMachinesStore();
const runner = useRunnerStore();
const signing = useSigningStore();

const host = computed(() => machines.host.value);
const paired = computed(() => runner.state.status?.paired === true);

const kitSummary = computed(() => {
    const kits = signing.kits.value;
    if (!kits.length) {
        return {
            title: 'No signing kits',
            detail: 'Store a certificate and profile once to sign builds',
        };
    }
    const ready = kits.filter(kitIsProvisionable).length;
    return {
        title: `${kits.length} kit${kits.length === 1 ? '' : 's'} stored`,
        detail:
            ready === kits.length
                ? 'All ready to provision into a machine'
                : `${ready} of ${kits.length} ready to provision`,
    };
});

const phases = [
    {
        title: 'Prepare a macOS machine once',
        body: 'A persistent macOS guest under Docker-OSX or dockur/macos. Install macOS and Xcode in it; BuildBridge pins its SSH identity and keeps the disk between starts.',
    },
    {
        title: 'Approve a project and test build',
        body: 'Approve one local folder, synchronize a filtered snapshot, and run an unsigned test build on the prepared machine.',
    },
    {
        title: 'Sign and export an IPA',
        body: 'Store a Team key, or your certificate and profiles, once in the OS vault; provision them into the guest; export a verified App Store Connect IPA.',
    },
    {
        title: 'Optional: run it on your iPhone',
        body: 'Hand a plugged-in iPhone to the machine over USB, and BuildBridge installs a Debug build on it and streams its console. Not one of the thirteen.',
    },
];

const glossary = [
    {
        word: 'Machine',
        meaning: 'a macOS virtual machine BuildBridge runs on this Linux host through Docker.',
    },
    {
        word: 'Guest',
        meaning: 'the macOS inside that machine; the console is its screen, in its own window.',
    },
    {
        word: 'Pinned identity',
        meaning: "the guest's SSH fingerprint, checked once and refused if it ever changes.",
    },
    {
        word: 'Signing kit',
        meaning:
            "your Apple signing credentials, stored once in this host's OS vault and shared by every machine.",
    },
    {
        word: 'Team key',
        meaning:
            'an App Store Connect API key; with one, BuildBridge creates certificates and profiles for you.',
    },
    { word: 'Provision', meaning: "import a kit into a machine's own keychain, ready to sign." },
    {
        word: 'Env set',
        meaning: 'variables and secrets written into the project at every sync, for the web build.',
    },
    {
        word: 'Control plane',
        meaning: 'the optional web dashboard that can queue builds on this machine from anywhere.',
    },
    {
        word: 'Template',
        meaning:
            'a prepared machine saved on this host; a new machine cloned from it starts in seconds at the project step.',
    },
];

// Probe each machine once so the rows show the exact journey, not the coarse one.
function probeAll(): void {
    for (const machine of machines.machines.value) {
        if (machines.session(machine.id).view === null) {
            void machines.refreshMachine(machine.id, { silent: true });
        }
    }
}
onMounted(probeAll);
watch(() => machines.machines.value.map((machine) => machine.id).join(','), probeAll);

const wellClass =
    'flex items-center gap-3 rounded-lg bg-zinc-100 p-3 text-left hover:bg-zinc-200/70 dark:bg-zinc-900 dark:hover:bg-zinc-800 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300';
</script>

<template>
    <div class="mx-auto max-w-4xl space-y-5 p-5">
        <header class="flex items-start justify-between gap-4">
            <div>
                <h1 class="text-lg font-bold text-zinc-900 dark:text-zinc-50">Overview</h1>
                <p class="mt-0.5 text-xs text-zinc-500 dark:text-zinc-400">
                    Build and sign iOS apps from this Linux host. Set a macOS machine up once, then
                    point it at any project.
                </p>
            </div>
            <Button
                v-if="machines.machines.value.length"
                size="sm"
                @click="ui.state.newMachineOpen = true"
            >
                <Plus class="h-3.5 w-3.5" />
                New machine
            </Button>
        </header>

        <Callout
            v-if="host && !host.ready"
            tone="warn"
            title="This host cannot run a macOS machine yet"
        >
            <ul class="mt-1 list-disc space-y-0.5 pl-4">
                <li v-for="issue in host.issues" :key="issue">{{ issue }}</li>
            </ul>
        </Callout>

        <section v-if="machines.machines.value.length" class="space-y-2">
            <h2 class="text-xs font-semibold text-zinc-500 dark:text-zinc-400">macOS machines</h2>
            <div class="space-y-2">
                <MachineRow
                    v-for="machine in machines.machines.value"
                    :key="machine.id"
                    :machine="machine"
                />
            </div>
        </section>

        <section
            v-else-if="!machines.state.listLoading"
            class="rounded-lg border border-zinc-200 bg-white p-5 dark:border-zinc-800 dark:bg-zinc-900"
        >
            <h2 class="text-[15px] font-semibold text-zinc-900 dark:text-zinc-50">
                Thirteen steps, one machine
            </h2>
            <p class="mt-1 max-w-xl text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                The machine is prepared once and then builds any number of projects. Every step says
                who does it, and the ones that are yours come with instructions.
            </p>
            <ol class="mt-4 space-y-3">
                <li v-for="(phase, index) in phases" :key="phase.title" class="flex gap-3">
                    <span
                        class="mt-px flex h-6 w-6 shrink-0 items-center justify-center rounded-full border border-zinc-300 text-[11px] font-semibold text-zinc-600 tabular-nums dark:border-zinc-700 dark:text-zinc-300"
                    >
                        {{ index + 1 }}
                    </span>
                    <span class="min-w-0">
                        <span class="block text-xs font-medium text-zinc-900 dark:text-zinc-50">{{
                            phase.title
                        }}</span>
                        <span class="block text-xs leading-5 text-zinc-500 dark:text-zinc-400">{{
                            phase.body
                        }}</span>
                    </span>
                </li>
            </ol>
            <!-- The words the steps use, defined once, before the steps use them. -->
            <dl class="mt-4 grid gap-x-6 gap-y-1.5 text-xs leading-5 sm:grid-cols-2">
                <div v-for="term in glossary" :key="term.word" class="flex gap-2">
                    <dt class="shrink-0 font-medium text-zinc-900 dark:text-zinc-50">
                        {{ term.word }}
                    </dt>
                    <dd class="text-zinc-500 dark:text-zinc-400">{{ term.meaning }}</dd>
                </div>
            </dl>
            <div class="mt-5">
                <Button size="sm" @click="ui.state.newMachineOpen = true">
                    <Plus class="h-3.5 w-3.5" />
                    Create the first machine
                </Button>
            </div>
        </section>

        <section class="grid gap-3 md:grid-cols-2">
            <button type="button" :class="wellClass" @click="ui.navigate({ kind: 'signing' })">
                <KeyRound class="h-4 w-4 shrink-0 text-zinc-500 dark:text-zinc-400" />
                <span class="min-w-0 flex-1">
                    <span class="block text-xs font-medium text-zinc-900 dark:text-zinc-50">{{
                        kitSummary.title
                    }}</span>
                    <span class="block truncate text-[11px] text-zinc-500 dark:text-zinc-400">{{
                        kitSummary.detail
                    }}</span>
                </span>
                <ArrowRight class="h-3.5 w-3.5 shrink-0 text-zinc-500 dark:text-zinc-400" />
            </button>
            <button type="button" :class="wellClass" @click="ui.navigate({ kind: 'runner' })">
                <Radio class="h-4 w-4 shrink-0 text-zinc-500 dark:text-zinc-400" />
                <span class="min-w-0 flex-1">
                    <span class="block text-xs font-medium text-zinc-900 dark:text-zinc-50">
                        {{
                            paired
                                ? (runner.state.status?.runnerName ?? 'Control plane paired')
                                : 'Control plane not paired'
                        }}
                    </span>
                    <span class="block truncate text-[11px] text-zinc-500 dark:text-zinc-400">
                        {{
                            paired
                                ? `Realtime ${runner.state.realtime}`
                                : 'Optional. Pair to start builds from anywhere and keep their history.'
                        }}
                    </span>
                </span>
                <ArrowRight class="h-3.5 w-3.5 shrink-0 text-zinc-500 dark:text-zinc-400" />
            </button>
        </section>
    </div>
</template>
