<script setup lang="ts">
// The one action of the current rung. The ladder decides what it is; this only names it, and
// says why it cannot be pressed on the rungs where something else has to happen first.
import type { Component } from 'vue';

import type { OperationId } from '../../stores/machines';
import Button from '../ui/Button.vue';
import Spinner from '../ui/Spinner.vue';

export interface DevicePrimary {
    label: string;
    icon: Component;
    outline: boolean;
    operation: OperationId | null;
    disabledReason: string | null;
    run: () => unknown;
}

defineProps<{ primary: DevicePrimary; disabled: boolean; spinning: boolean }>();
</script>

<template>
    <Button
        size="sm"
        :variant="primary.outline ? 'outline' : 'default'"
        :disabled="disabled"
        :title="primary.disabledReason ?? undefined"
        @click="primary.run()"
    >
        <Spinner
            v-if="spinning"
            :tone="primary.outline ? undefined : 'text-white dark:text-zinc-950'"
        />
        <component :is="primary.icon" v-else class="h-3.5 w-3.5" />
        {{ primary.label }}
    </Button>
</template>
