<script setup lang="ts">
import { computed } from 'vue';

import { useBuildFlowStore } from '../../stores/build-flow';
import type { MachineSession } from '../../stores/machines';
import Checkbox from '../ui/Checkbox.vue';

const { session, disabled = false } = defineProps<{
    session: MachineSession;
    disabled?: boolean;
}>();
const flows = useBuildFlowStore();
const allowHttp = computed({
    get: () => flows.draft(session.id).androidAllowHttp,
    set: (value: boolean) => {
        flows.draft(session.id).androidAllowHttp = value;
    },
});
</script>

<template>
    <div class="space-y-1.5">
        <Checkbox
            v-model="allowHttp"
            block
            :disabled="disabled || !session.view?.android?.workspace"
            >Allow HTTP APIs for debug builds</Checkbox
        >
        <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
            Allows unencrypted API requests for local testing. Applies to the next debug build for
            this project; build and reinstall to change an installed app. Release builds use the
            project's settings.
        </p>
    </div>
</template>
