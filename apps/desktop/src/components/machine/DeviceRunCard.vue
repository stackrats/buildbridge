<script setup lang="ts">
// What the run itself needs: the web assets to build with, the version the app will carry, and
// the one action that builds, installs and launches it. The phone's rungs are the card above;
// this one names what the phone still needs rather than repeating how to fix it, so the choices
// that change what gets built sit with the button that builds it.
import { computed } from 'vue';

import type { ListboxOption } from '../../lib/listbox';
import { buildPrerequisite } from '../../model/build-flow';
import { useEnvSetsStore } from '../../stores/envs';
import type { MachineSession } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import Field from '../ui/Field.vue';
import type { KeyValueItem } from '../ui/KeyValue.vue';
import KeyValue from '../ui/KeyValue.vue';
import Select from '../ui/Select.vue';
import VersionFields from './VersionFields.vue';

const { session } = defineProps<{
    session: MachineSession;
    busy: boolean;
    /** The project is not ready for a phone: no passing test build, or signing not provisioned. */
    pending: boolean;
    /** What the phone still needs before a run, or null when it is ready. */
    nextStep: string | null;
    envOptions: ListboxOption[];
    recipe: KeyValueItem[];
}>();
const envSetId = defineModel<string>('envSetId', { required: true });
const envs = useEnvSetsStore();
const ui = useUi();
// Whatever blocks the run, one thing at a time, in the order it gets fixed: the machine, then
// the project, then the phone. The phone's own card says how to fix its rung; this only names it.
const blocker = computed(() => (session.view ? buildPrerequisite(session.view) : null));
</script>

<template>
    <Card>
        <template #title>Build and run on your iPhone</template>
        <template #description>
            Builds, installs and launches a Debug version on the attached phone, then streams its
            console. Stop ends the console session; the app stays installed.
        </template>
        <template #actions>
            <slot name="action" />
        </template>

        <div class="space-y-4">
            <Callout v-if="blocker" tone="neutral">
                {{ blocker.message }}
                <div class="mt-2">
                    <Button
                        variant="outline"
                        size="sm"
                        :disabled="busy"
                        @click="ui.openMachine(session.id, blocker.step)"
                    >
                        {{ blocker.action }}
                    </Button>
                </div>
            </Callout>
            <Callout
                v-else-if="pending"
                tone="neutral"
                title="Prepare the project before running on the phone"
            >
                Complete a test build, attach development-capable signing credentials, and prepare
                signing in macOS. A release IPA is not required.
                <div class="mt-2 flex flex-wrap gap-2">
                    <Button
                        variant="outline"
                        size="sm"
                        @click="ui.openMachine(session.id, 'signing-kit')"
                    >
                        {{
                            session.view?.signingKit
                                ? 'Review signing credentials'
                                : 'Choose signing credentials'
                        }}
                    </Button>
                    <Button
                        variant="outline"
                        size="sm"
                        @click="ui.openMachine(session.id, 'provision')"
                    >
                        Prepare signing in macOS
                    </Button>
                </div>
            </Callout>
            <Callout v-else-if="nextStep" tone="neutral" title="The phone is not ready yet">
                {{ nextStep }}
            </Callout>
            <Field
                v-if="envs.sets.value.length"
                label="Environment"
                hint="Rebuilds the web assets with this environment for this run."
            >
                <Select v-model="envSetId" :options="envOptions" :disabled="busy" />
            </Field>
            <VersionFields :session="session" :disabled="busy" />
            <details>
                <DisclosureSummary quiet>Build recipe</DisclosureSummary>
                <div class="mt-2 space-y-3">
                    <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                        Builds the App scheme in Debug with the development identity. Xcode's
                        <code class="font-mono">devicectl</code> installs and launches the app on
                        the attached phone; its console streams until you stop the session.
                    </p>
                    <KeyValue :items="recipe" :columns="3" />
                </div>
            </details>
            <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                Self-hosted and experimental. USB passthrough depends on the host and machine.
                Apple's supported route to a physical device is a Mac. Xcode's debugger and
                Instruments are not part of this step. The iOS Simulator is a compile target of a
                test build rather than a preview: choose it as the compile target on the test build
                step.
            </p>
        </div>
    </Card>
</template>
