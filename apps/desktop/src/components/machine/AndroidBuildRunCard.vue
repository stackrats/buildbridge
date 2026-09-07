<script setup lang="ts">
// The first of the two ways onto the phone: copy the approved local project, build a debug APK,
// then open it and stream its log. Every choice that changes what gets built — the environment,
// the version, the HTTP allowance — is in the card with the button that builds it, rather than
// in a form a screen away from its action.
import { Hammer } from '@lucide/vue';
import { computed, onMounted, ref, watch } from 'vue';

import { buildPrerequisite } from '../../model/build-flow';
import { useBuildFlowStore } from '../../stores/build-flow';
import { useEnvSetsStore } from '../../stores/envs';
import type { MachineSession } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import Field from '../ui/Field.vue';
import Select from '../ui/Select.vue';
import AndroidHttpOption from './AndroidHttpOption.vue';
import VersionFields from './VersionFields.vue';

const { session, busy, deviceReady } = defineProps<{
    session: MachineSession;
    busy: boolean;
    deviceReady: boolean;
}>();
const flows = useBuildFlowStore();
const envs = useEnvSetsStore();
const ui = useUi();
// The env for Build and run: written into the container with the source copy, the way the
// guided build does it, so the next build starts from the same choice. Preselects the set the
// last copy used.
const attachedEnvSet = computed(() => session.view?.envSet ?? null);
const envSetId = ref(attachedEnvSet.value?.id ?? '');
watch(attachedEnvSet, (set, previous) => {
    if (envSetId.value === (previous?.id ?? '')) {
        envSetId.value = set?.id ?? '';
    }
});
onMounted(() => void envs.load());
const envOptions = computed(() => [
    { value: '', label: 'No environment' },
    ...envs.sets.value.map((set) => ({ value: set.id, label: set.name })),
]);
const chosenEnvName = computed(() => envs.setById(envSetId.value)?.name ?? null);
// Shown until the guided build takes over: once it is running it says the same thing itself,
// above the tabs, and says it for every stage rather than only this one.
const prerequisite = computed(() =>
    session.view && !flows.active(session.id) ? buildPrerequisite(session.view) : null,
);

function buildAndRun(): void {
    void flows.startAndroidPreview(
        session.id,
        session.androidDeviceSerial,
        envSetId.value || null,
        chosenEnvName.value,
    );
}
</script>

<template>
    <Card>
        <template #title>Build and run the latest source</template>
        <template #description>
            Copies your approved local project, builds a debug APK, then installs and opens it on
            the selected device and streams the app's log until you stop it.
        </template>
        <template #actions>
            <Button size="sm" :disabled="busy || !deviceReady" @click="buildAndRun">
                <Hammer class="h-3.5 w-3.5" />
                Build and run
            </Button>
        </template>

        <div class="space-y-4">
            <Callout v-if="prerequisite" tone="neutral">
                {{ prerequisite.message }}
                <div class="mt-2">
                    <Button
                        variant="outline"
                        size="sm"
                        :disabled="busy"
                        @click="ui.openMachine(session.id, prerequisite.step)"
                    >
                        {{ prerequisite.action }}
                    </Button>
                </div>
            </Callout>
            <Field
                v-if="envs.sets.value.length"
                label="Environment"
                hint="Written into the container with the source copy and applied to the web build for this run."
            >
                <Select v-model="envSetId" :options="envOptions" :disabled="busy" />
            </Field>
            <VersionFields :session="session" :disabled="busy" />
            <AndroidHttpOption :session="session" :disabled="busy" />
        </div>
    </Card>
</template>
