<script setup lang="ts">
// The version this build carries, prefilled from the project's own files. Editing it here has
// BuildBridge write the change into those files before the build, the edit a person would
// otherwise make by hand, and apply the same values where the build runs, so the project, the
// artifact and this panel agree. The next build number is one click away, because every store
// upload needs a new one.
import { Plus } from '@lucide/vue';
import { computed } from 'vue';

import { isAndroid } from '../../model/providers';
import { nextBuild, sameVersion } from '../../model/version';
import { useBuildFlowStore } from '../../stores/build-flow';
import type { MachineSession } from '../../stores/machines';
import type { ProjectVersion } from '../../types/backend';
import Button from '../ui/Button.vue';
import Field from '../ui/Field.vue';
import Input from '../ui/Input.vue';

const { session, disabled = false } = defineProps<{
    session: MachineSession;
    disabled?: boolean;
}>();
const flows = useBuildFlowStore();
const view = computed(() => session.view!);
const android = computed(() => isAndroid(view.value.profile.provider));
const project = computed(() => view.value.projectVersion);
const draft = computed(() => flows.draft(session.id));
const current = computed(() => draft.value.version ?? project.value);
const labels = computed(() =>
    android.value
        ? { version: 'Version name', build: 'Version code', file: 'android/app/build.gradle' }
        : {
              version: 'Version',
              build: 'Build number',
              file: 'ios/App/App.xcodeproj/project.pbxproj',
          },
);

// An edit that lands back on the project's own values is no edit at all.
function update(patch: Partial<ProjectVersion>): void {
    const next = { ...(current.value ?? { version: '', build: '' }), ...patch };
    draft.value.version = sameVersion(next, project.value) ? null : next;
}
const version = computed({
    get: () => current.value?.version ?? '',
    set: (value: string) => update({ version: value.trim() }),
});
const build = computed({
    get: () => current.value?.build ?? '',
    set: (value: string) => update({ build: value.trim() }),
});
const versionChanged = computed(() => current.value?.version !== project.value?.version);
const buildChanged = computed(() => current.value?.build !== project.value?.build);
</script>

<template>
    <div v-if="project" class="space-y-2">
        <div class="grid gap-4 sm:grid-cols-2">
            <Field :label="labels.version">
                <template #trailing>{{
                    versionChanged ? `was ${project.version}` : 'in project'
                }}</template>
                <Input v-model="version" mono :disabled="disabled" placeholder="3.2.1" />
            </Field>
            <Field :label="labels.build">
                <template #trailing>{{
                    buildChanged ? `was ${project.build}` : 'in project'
                }}</template>
                <Input v-model="build" mono :disabled="disabled" placeholder="16" />
                <template #action>
                    <Button
                        variant="outline"
                        size="sm"
                        :disabled="disabled"
                        title="The next build number; every store upload needs a new one"
                        @click="update({ build: nextBuild(build) })"
                    >
                        <Plus class="h-3.5 w-3.5" />
                        Next
                    </Button>
                </template>
            </Field>
        </div>
        <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
            <template v-if="draft.version">
                Written into <span class="font-mono">{{ labels.file }}</span> before the build and
                applied to it, so commit the change with your next commit.
            </template>
            <template v-else>
                Read from <span class="font-mono">{{ labels.file }}</span> and applied to this
                build. Change either to set it; BuildBridge writes it into the project as well.
            </template>
        </p>
    </div>
    <p v-else class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
        BuildBridge could not read one version and build number from
        <span class="font-mono">{{ labels.file }}</span
        >, so this build carries whatever the project sets. Declare both once in the project, and
        they can be set here.
    </p>
</template>
