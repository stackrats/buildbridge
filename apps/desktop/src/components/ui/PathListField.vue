<script setup lang="ts">
// A newline-separated list of paths. Browsing adds to the list rather than replacing it, so
// profiles can be picked one at a time or all at once.
import { FolderOpen } from '@lucide/vue';
import { ref } from 'vue';

import { useBackend } from '../../lib/backend';
import { mergePathList, startDirectory } from '../../lib/paths';
import { describeError } from '../../lib/utils';
import Button from './Button.vue';
import Spinner from './Spinner.vue';
import Textarea from './Textarea.vue';

const model = defineModel<string>({ default: '' });
const {
    title,
    filter,
    placeholder = '',
    rows = 2,
    disabled = false,
} = defineProps<{
    title: string;
    filter?: { name: string; extensions: string[] };
    placeholder?: string;
    rows?: number;
    disabled?: boolean;
}>();

const picking = ref(false);
const error = ref<string | null>(null);

async function browse(): Promise<void> {
    picking.value = true;
    error.value = null;
    try {
        const picked = await useBackend().pickPaths({
            kind: 'files',
            title,
            filter,
            startFrom: startDirectory(model.value.split('\n').pop() ?? null),
        });
        if (picked.length > 0) {
            model.value = mergePathList(model.value, picked);
        }
    } catch (caught) {
        error.value = describeError(caught);
    } finally {
        picking.value = false;
    }
}
</script>

<template>
    <div>
        <div class="relative">
            <Textarea
                v-model="model"
                mono
                class="pr-24"
                :rows="rows"
                :placeholder="placeholder"
                :disabled="disabled"
            />
            <Button
                variant="ghost"
                size="sm"
                class="absolute top-1 right-0.5"
                :disabled="disabled || picking"
                @click="browse"
            >
                <Spinner v-if="picking" />
                <FolderOpen v-else class="h-3.5 w-3.5" />
                Browse
            </Button>
        </div>
        <p v-if="error" class="mt-1 text-[11px] text-red-700 dark:text-red-400">{{ error }}</p>
    </div>
</template>
