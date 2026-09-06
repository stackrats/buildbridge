<script setup lang="ts">
import { ArrowRight, Smartphone, Monitor } from '@lucide/vue';
import { computed, ref } from 'vue';

import { isAndroid } from '../../model/providers';
import type { JourneyStep, JourneyStepId } from '../../model/steps';
import type { MachineSession } from '../../stores/machines';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import PlatformIcon from '../ui/PlatformIcon.vue';
import StepDetail from './StepDetail.vue';

const { session, steps } = defineProps<{ session: MachineSession; steps: JourneyStep[] }>();
const emit = defineEmits<{ build: []; step: [id: JourneyStepId] }>();
const android = computed(() => isAndroid(session.view!.profile.provider));
const deviceStep = computed(() => steps.find((step) => step.id === 'run-device'));
const showDevice = ref(true);
</script>

<template>
    <div class="space-y-5">
        <header class="flex flex-wrap items-start justify-between gap-3">
            <div class="min-w-0 flex-1">
                <h2 class="text-base font-semibold">Preview your app</h2>
                <p class="mt-1 text-sm leading-6 text-zinc-500 dark:text-zinc-400">
                    {{
                        android
                            ? 'Build and open your app on a connected Android phone or a running host emulator, or install a retained APK.'
                            : 'Run a development build on your iPhone, or check compilation for the iOS Simulator.'
                    }}
                </p>
            </div>
            <Button @click="emit('build')">Build for testing<ArrowRight class="h-4 w-4" /></Button>
        </header>
        <div
            v-if="android && deviceStep"
            class="space-y-3 rounded-lg border border-zinc-200 bg-white p-4 dark:border-zinc-800 dark:bg-zinc-900"
        >
            <h3 class="flex items-center gap-2 text-sm font-semibold">
                <PlatformIcon platform="android" class="h-4 w-4" />On a real device
            </h3>
            <StepDetail :session="session" :step="deviceStep" />
        </div>
        <template v-else>
            <div class="grid gap-3 sm:grid-cols-2">
                <div
                    class="rounded-lg border border-zinc-200 bg-white p-4 dark:border-zinc-800 dark:bg-zinc-900"
                >
                    <Smartphone class="mb-3 h-5 w-5 text-zinc-500 dark:text-zinc-400" />
                    <h3 class="text-sm font-semibold">Run on iPhone</h3>
                    <p class="mt-2 text-sm leading-6 text-zinc-500 dark:text-zinc-400">
                        Guided USB connection, development signing, installation and a live console.
                        Experimental; the phone moves to this macOS machine while attached.
                    </p>
                    <Button
                        class="mt-3"
                        size="sm"
                        :variant="showDevice ? 'outline' : 'default'"
                        :aria-expanded="showDevice"
                        @click="showDevice = !showDevice"
                        >{{ showDevice ? 'Hide device setup' : 'Prepare and run' }}</Button
                    >
                </div>
                <div
                    class="rounded-lg border border-zinc-200 bg-white p-4 dark:border-zinc-800 dark:bg-zinc-900"
                >
                    <Monitor class="mb-3 h-5 w-5 text-zinc-500 dark:text-zinc-400" />
                    <h3 class="text-sm font-semibold">Build for iOS Simulator</h3>
                    <p class="mt-2 text-sm leading-6 text-zinc-500 dark:text-zinc-400">
                        Checks compilation against the Simulator SDK. A runtime may need
                        downloading. This does not open a simulator or provide an interactive
                        preview.
                    </p>
                    <Button
                        class="mt-3"
                        size="sm"
                        variant="outline"
                        @click="emit('step', 'test-build')"
                        >Review compile targets</Button
                    >
                </div>
            </div>
            <div v-if="showDevice && deviceStep" class="space-y-3">
                <Callout
                    v-if="deviceStep.status === 'pending'"
                    tone="neutral"
                    title="Prepare the project before running on iPhone"
                >
                    Complete a test build, attach development-capable signing credentials, and
                    prepare signing in macOS. A release IPA is not required.
                    <div class="mt-3 flex flex-wrap gap-2">
                        <Button size="sm" variant="outline" @click="emit('step', 'signing-kit')">
                            {{
                                session.view?.signingKit
                                    ? 'Review signing credentials'
                                    : 'Choose signing credentials'
                            }}
                        </Button>
                        <Button size="sm" variant="outline" @click="emit('step', 'provision')"
                            >Prepare signing in macOS</Button
                        >
                    </div>
                </Callout>
                <div
                    class="rounded-lg border border-zinc-200 bg-white p-4 dark:border-zinc-800 dark:bg-zinc-900"
                >
                    <StepDetail :session="session" :step="deviceStep" />
                </div>
            </div>
        </template>
    </div>
</template>
