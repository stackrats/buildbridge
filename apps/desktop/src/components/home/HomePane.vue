<script setup lang="ts">
// The overview is a snapshot: every machine as a row with its journey and its next action,
// then the two shared resources as quiet wells. With no machine yet it is a welcome screen
// that says the three phases and offers one button.
import { ArrowRight, KeyRound, Plus, Radio } from '@lucide/vue';
import { computed, onMounted, watch } from 'vue';

import { useMachinesStore } from '../../stores/machines';
import { providerHostIssues, providerLabel } from '../../model/providers';
import { kitSignsAndroid } from '../../model/signing';
import { useRunnerStore } from '../../stores/runner';
import { kitIsProvisionable, useSigningStore } from '../../stores/signing';
import { useUi } from '../../stores/ui';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import MachineRow from './MachineRow.vue';

const ui = useUi();
const machines = useMachinesStore();
const runner = useRunnerStore();
const signing = useSigningStore();

const host = computed(() => machines.host.value);
const paired = computed(() => runner.state.status?.paired === true);
const remoteDetail = computed(() => {
    if (runner.state.status?.credentialsMissing) {
        return 'The stored connection token is missing. Open Remote builds to reconnect.';
    }
    if (!paired.value) {
        return 'Use a shared builder, or connect this computer to accept builds.';
    }
    return runner.state.realtime === 'connected'
        ? 'Connected and accepting builds.'
        : runner.state.realtime === 'connecting'
          ? 'Connecting to accept builds.'
          : 'Not connected. Local builds are still available.';
});
const hostWarnings = computed(() =>
    [...new Set(machines.machines.value.map((machine) => machine.config.provider))]
        .map((provider) => ({
            provider,
            issues: providerHostIssues(host.value, provider, runner.state.status?.platform),
        }))
        .filter((entry) => entry.issues.length > 0),
);

const kitSummary = computed(() => {
    const kits = signing.kits.value;
    if (!kits.length) {
        return {
            title: 'Signing credentials',
            detail: 'Add these when you are ready to sign and export a release.',
        };
    }
    const ready = kits.filter((kit) => kitIsProvisionable(kit) || kitSignsAndroid(kit)).length;
    return {
        title:
            kits.length === 1
                ? 'Signing credentials stored'
                : `${kits.length} signing credentials stored`,
        detail:
            ready === kits.length
                ? 'Ready to use for their supported platforms'
                : `${ready} of ${kits.length} ready to use`,
    };
});

const phases = [
    {
        title: 'Prepare a machine once',
        body: 'Choose iOS or Android. BuildBridge guides you through setup and keeps the machine ready for your next project.',
    },
    {
        title: 'Build and preview',
        body: 'Choose your local Capacitor project and make a test build. Preview the app before preparing a release.',
    },
    {
        title: 'Sign and export',
        body: 'When you are ready to distribute, add signing credentials and export an iOS IPA or an Android app bundle and APK.',
    },
];

const glossary = [
    {
        word: 'Machine',
        meaning:
            'a macOS virtual machine, or an Android toolchain container, BuildBridge runs on this Linux host through Docker.',
    },
    {
        word: 'Guest',
        meaning: 'the macOS inside a macOS machine; the console is its screen, in its own window.',
    },
    {
        word: 'Pinned identity',
        meaning: "the guest's SSH fingerprint, checked once and refused if it ever changes.",
    },
    {
        word: 'Signing credentials',
        meaning:
            "your Apple signing credentials and your Android upload key, stored once in this host's OS vault and shared by every machine.",
    },
    {
        word: 'Team key',
        meaning:
            'an App Store Connect API key; with one, BuildBridge creates certificates and profiles for you.',
    },
    {
        word: 'Provision',
        meaning: "import signing credentials into a machine's own keychain, ready to sign.",
    },
    {
        word: 'Environment',
        meaning: 'variables and secrets written into the project at every sync, for the web build.',
    },
    {
        word: 'Remote builds',
        meaning: 'optional; use a shared builder, or connect this computer to accept builds.',
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
    'flex items-center gap-3 rounded-lg border border-zinc-200 bg-zinc-50 p-3 text-left hover:bg-zinc-100 dark:border-zinc-800 dark:bg-zinc-900/50 dark:hover:bg-zinc-800 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-700 dark:focus-visible:outline-zinc-300';
</script>

<template>
    <div class="mx-auto max-w-4xl space-y-5 p-5">
        <header class="flex items-start justify-between gap-4">
            <div>
                <h1 class="text-lg font-bold text-zinc-900 dark:text-zinc-50">Overview</h1>
                <p class="mt-0.5 text-xs text-zinc-500 dark:text-zinc-400">
                    Your machines, projects and builds. Set up once, then build and preview locally.
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
            v-if="runner.state.status?.platform === 'macos'"
            tone="neutral"
            title="Build directly on this Mac"
        >
            Use your installed Xcode and signing identity for iOS builds. Set it up once, then
            optionally share access through your BuildBridge server.
            <div class="mt-3">
                <Button size="sm" @click="ui.navigate({ kind: 'native_mac' })"
                    >Use this Mac<ArrowRight class="h-3.5 w-3.5"
                /></Button>
            </div>
        </Callout>

        <Callout
            v-for="warning in hostWarnings"
            :key="warning.provider"
            tone="warn"
            :title="`${providerLabel[warning.provider]} needs attention`"
        >
            <ul class="mt-1 list-disc space-y-0.5 pl-4">
                <li v-for="issue in warning.issues" :key="issue">{{ issue }}</li>
            </ul>
        </Callout>

        <section v-if="machines.machines.value.length" class="space-y-2">
            <h2 class="text-xs font-semibold text-zinc-500 dark:text-zinc-400">Machines</h2>
            <div class="space-y-2">
                <MachineRow
                    v-for="machine in machines.dashboardMachines.value"
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
                Your first app starts here
            </h2>
            <p class="mt-1 max-w-xl text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                Prepare a machine for iOS or Android, then reuse it for every project. You can make
                your first test build without release signing credentials or a remote account.
            </p>
            <div class="mt-4">
                <Button size="sm" @click="ui.state.newMachineOpen = true">
                    <Plus class="h-3.5 w-3.5" />
                    Create your first machine
                </Button>
            </div>
            <ol class="mt-6 grid gap-4 sm:grid-cols-3">
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
            <p class="mt-4 text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                Device previews are optional. Your guided steps explain when a phone, signing
                credentials or an action inside macOS is needed.
            </p>
            <details class="mt-5 border-t border-zinc-200 pt-3 dark:border-zinc-800">
                <DisclosureSummary class="text-xs font-medium text-zinc-600 dark:text-zinc-300">
                    How it works: machines, signing and other terms
                </DisclosureSummary>
                <dl class="mt-3 grid gap-x-6 gap-y-2 text-xs leading-5 sm:grid-cols-2">
                    <div v-for="term in glossary" :key="term.word" class="flex gap-2">
                        <dt class="shrink-0 font-medium text-zinc-900 dark:text-zinc-50">
                            {{ term.word }}
                        </dt>
                        <dd class="text-zinc-500 dark:text-zinc-400">{{ term.meaning }}</dd>
                    </div>
                </dl>
            </details>
        </section>

        <section class="grid gap-3 md:grid-cols-2">
            <button type="button" :class="wellClass" @click="ui.navigate({ kind: 'signing' })">
                <KeyRound class="h-4 w-4 shrink-0 text-zinc-500 dark:text-zinc-400" />
                <span class="min-w-0 flex-1">
                    <span class="block text-xs font-medium text-zinc-900 dark:text-zinc-50">{{
                        kitSummary.title
                    }}</span>
                    <span class="mt-0.5 block text-xs leading-5 text-zinc-500 dark:text-zinc-400">{{
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
                                ? (runner.state.status?.runnerName ?? 'Remote builds')
                                : 'Remote builds · Optional'
                        }}
                    </span>
                    <span class="mt-0.5 block text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                        {{ remoteDetail }}
                    </span>
                </span>
                <ArrowRight class="h-3.5 w-3.5 shrink-0 text-zinc-500 dark:text-zinc-400" />
            </button>
        </section>
    </div>
</template>
