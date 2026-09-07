<script setup lang="ts">
// One control for every path buildbridge asks for: type it, drop it on the window, or browse.
// Browsing is the primary route, so nobody has to know a path by heart.
import { FolderOpen } from '@lucide/vue';
import { computed, ref, watch } from 'vue';

import { useBackend, type PathPickRequest } from '../../lib/backend';
import { matchesExtension, startDirectory } from '../../lib/paths';
import { describeError } from '../../lib/utils';
import { useMachinesStore } from '../../stores/machines';
import Button from './Button.vue';
import Input from './Input.vue';
import Spinner from './Spinner.vue';

const model = defineModel<string>({ default: '' });
const {
    kind,
    title,
    filter,
    placeholder = '',
    disabled = false,
    acceptDrop = true,
} = defineProps<{
    kind: PathPickRequest['kind'];
    /** Dialog title, e.g. "Choose the signing identity". */
    title: string;
    filter?: { name: string; extensions: string[] };
    placeholder?: string;
    disabled?: boolean;
    /** Whether a file dropped on the window should land in this field. */
    acceptDrop?: boolean;
}>();

const machines = useMachinesStore();
const picking = ref(false);
const error = ref<string | null>(null);

const highlighted = computed(() => acceptDrop && machines.state.dragActive);

// A drop only claims the field when it looks like what the field wants; the window has more
// than one path input open at a time.
watch(
    () => machines.state.lastDrop,
    (drop) => {
        if (!drop || !acceptDrop || disabled) {
            return;
        }
        const wanted = drop.paths.find((path) =>
            filter ? matchesExtension(path, filter.extensions) : !path.includes('.'),
        );
        if (wanted) {
            model.value = wanted;
            machines.consumeDrop();
        }
    },
);

async function browse(): Promise<void> {
    picking.value = true;
    error.value = null;
    try {
        const picked = await useBackend().pickPaths({
            kind,
            title,
            filter,
            startFrom: startDirectory(model.value),
        });
        if (picked.length > 0) {
            model.value = picked[0]!;
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
        <div
            class="relative rounded-md transition-colors"
            :class="
                highlighted
                    ? 'outline-2 -outline-offset-2 outline-zinc-400 outline-dashed dark:outline-zinc-600'
                    : ''
            "
        >
            <Input
                v-model="model"
                mono
                class="pr-24"
                :placeholder="placeholder"
                :disabled="disabled"
            />
            <Button
                variant="ghost"
                size="sm"
                class="absolute top-1/2 right-0.5 -translate-y-1/2"
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
