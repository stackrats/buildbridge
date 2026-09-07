<script setup lang="ts">
// Every file the machine has retained, under its own heading at the end of the journey: the
// name, what it is for, the path and the size, with the two things anyone wants of a file, to
// see it in its folder and to copy its path. Open by default, so the files are found at a glance.
import { ChevronDown, FolderOpen, Package } from '@lucide/vue';
import { computed } from 'vue';

import { formatBytes } from '../../lib/format';
import { retainedArtifacts, type RetainedArtifact } from '../../model/artifacts';
import { useMachinesStore, type MachineSession } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import Button from '../ui/Button.vue';
import CopyButton from '../ui/CopyButton.vue';

const { session } = defineProps<{ session: MachineSession }>();
const machines = useMachinesStore();
const ui = useUi();
const open = computed({
    get: () => ui.sectionOpen(session.id, 'artifacts') ?? true,
    set: (value: boolean) => ui.setSectionOpen(session.id, 'artifacts', value),
});
const artifacts = computed(() => (session.view ? retainedArtifacts(session.view) : []));
const totalBytes = computed(() =>
    artifacts.value.reduce((sum, artifact) => sum + artifact.file.bytes, 0),
);

function reveal(artifact: RetainedArtifact): void {
    if (artifact.reveal === 'debug') {
        void machines.revealDebugApk(session.id);
    } else if (artifact.reveal === 'release') {
        void machines.revealRelease(session.id);
    } else {
        void machines.revealArchive(session.id);
    }
}
</script>

<template>
    <!-- On the phases' own rhythm: one gap below the last of them, headed the same way. -->
    <section class="mt-1">
        <button
            type="button"
            class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-zinc-100/70 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-zinc-700 dark:hover:bg-zinc-800/40 dark:focus-visible:outline-zinc-300"
            :aria-expanded="open"
            @click="open = !open"
        >
            <Package class="h-3.5 w-3.5 shrink-0 text-zinc-500 dark:text-zinc-400" />
            <span class="text-[13px] font-semibold text-zinc-700 dark:text-zinc-200">
                Retained files
            </span>
            <span
                class="min-w-0 flex-1 truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400"
            >
                <template v-if="artifacts.length"
                    >{{ artifacts.length }} {{ artifacts.length === 1 ? 'file' : 'files' }} ·
                    {{ formatBytes(totalBytes) }}</template
                >
                <template v-else>nothing retained yet; a build leaves its files here</template>
            </span>
            <ChevronDown
                class="h-3.5 w-3.5 shrink-0 text-zinc-500 transition-transform duration-200 motion-reduce:transition-none dark:text-zinc-400"
                :class="open ? '' : '-rotate-90'"
            />
        </button>

        <div v-if="open" class="mt-2 px-2">
            <ul
                v-if="artifacts.length"
                class="divide-y divide-zinc-200 rounded-lg border border-zinc-200 bg-white px-4 dark:divide-zinc-800 dark:border-zinc-800 dark:bg-zinc-900"
            >
                <li
                    v-for="artifact in artifacts"
                    :key="artifact.id"
                    class="flex flex-wrap items-center gap-3 py-3"
                >
                    <Package class="h-4 w-4 shrink-0 text-zinc-500 dark:text-zinc-400" />
                    <div class="min-w-0 flex-1">
                        <p class="text-sm font-medium">
                            {{ artifact.label }}
                            <span class="ml-1 text-xs font-normal text-zinc-500 dark:text-zinc-400">
                                {{ artifact.destination }}
                            </span>
                        </p>
                        <p
                            class="truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400"
                            v-tip="`${artifact.file.path}\nsha256 ${artifact.file.sha256}`"
                        >
                            {{ artifact.file.path }}
                        </p>
                    </div>
                    <span
                        class="w-[4.5rem] shrink-0 text-right font-mono text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
                    >
                        {{ formatBytes(artifact.file.bytes) }}
                    </span>
                    <Button size="sm" variant="outline" @click="reveal(artifact)">
                        <FolderOpen class="h-3.5 w-3.5" />
                        Show in folder
                    </Button>
                    <CopyButton
                        :text="artifact.file.path"
                        :what="`Copy the ${artifact.label} path`"
                        size="iconSm"
                    />
                </li>
            </ul>
            <p v-else class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                A test or debug build, a signed archive or a release leaves its files here on this
                host until they are cleared from their step.
            </p>
        </div>
    </section>
</template>
