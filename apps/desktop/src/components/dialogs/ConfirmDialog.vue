<script setup lang="ts">
// A deliberate destructive action: what will be lost is stated in full, a high-risk action
// needs an explicit acknowledgement, and the confirm button names the action.
import { TriangleAlert } from '@lucide/vue';
import { ref, watch } from 'vue';

import Button from '../ui/Button.vue';
import Checkbox from '../ui/Checkbox.vue';
import Modal from '../ui/Modal.vue';
import Spinner from '../ui/Spinner.vue';

const open = defineModel<boolean>('open', { default: false });
const {
    confirmLabel = 'Confirm',
    acknowledgement = null,
    busy = false,
    destructive = true,
    confirmDisabled = false,
} = defineProps<{
    title: string;
    confirmLabel?: string;
    /** When set, a checkbox with this text must be ticked before confirming. */
    acknowledgement?: string | null;
    busy?: boolean;
    destructive?: boolean;
    /** The dialog's own inputs are not valid yet, so confirming would be refused. */
    confirmDisabled?: boolean;
}>();

const emit = defineEmits<{ confirm: [] }>();

const acknowledged = ref(false);

watch(open, (value) => {
    if (!value) {
        acknowledged.value = false;
    }
});
</script>

<template>
    <Modal v-model:open="open" :title="title" :busy="busy">
        <div class="space-y-3">
            <div class="flex items-start gap-2 text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                <TriangleAlert
                    v-if="destructive"
                    class="mt-0.5 h-3.5 w-3.5 shrink-0 text-amber-700 dark:text-amber-400"
                />
                <div class="min-w-0 flex-1 space-y-2">
                    <slot />
                </div>
            </div>
            <Checkbox
                v-if="acknowledgement"
                v-model="acknowledged"
                block
                :label="acknowledgement"
                :disabled="busy"
            />
        </div>
        <template #footer>
            <Button variant="outline" size="sm" :disabled="busy" @click="open = false">
                Cancel
            </Button>
            <Button
                :variant="destructive ? 'danger' : 'default'"
                size="sm"
                :disabled="busy || confirmDisabled || (acknowledgement !== null && !acknowledged)"
                @click="emit('confirm')"
            >
                <Spinner
                    v-if="busy"
                    :tone="
                        destructive
                            ? 'text-red-700 dark:text-red-400'
                            : 'text-white dark:text-zinc-950'
                    "
                />
                {{ confirmLabel }}
            </Button>
        </template>
    </Modal>
</template>
