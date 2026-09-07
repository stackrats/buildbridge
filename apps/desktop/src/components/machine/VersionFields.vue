<script setup lang="ts">
// The version this build carries, prefilled from the project's own files, with a stepper on
// every number: each part of the version, and the build number. Editing them has buildbridge
// write the change into those files before the build, the edit a person would otherwise make
// by hand, and apply the same values where the build runs, so the project, the artifact and
// this panel agree. Raising a part of the version starts the parts after it again, the way a
// minor bump resets the patch. A version or build that is not plain numbers falls back to a
// text field. Where a build is bound for a store, the build number can be checked against
// what the store already holds: the field turns red when the store would refuse it, and the
// next accepted number is one click away. Nothing is bumped on its own.
import { computed, useId } from 'vue';

import { isAndroid } from '../../model/providers';
import {
    buildExceeds,
    sameVersion,
    storeLabel,
    versionParts,
    withVersionPart,
} from '../../model/version';
import { useBuildFlowStore } from '../../stores/build-flow';
import { useMachinesStore, type MachineSession } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import type { ProjectVersion } from '../../types/backend';
import Button from '../ui/Button.vue';
import Field from '../ui/Field.vue';
import Input from '../ui/Input.vue';
import NumberField from '../ui/NumberField.vue';
import Spinner from '../ui/Spinner.vue';

const {
    session,
    disabled = false,
    storeCheck = false,
} = defineProps<{
    session: MachineSession;
    disabled?: boolean;
    /** Offer to ask the store which build it already holds: the archive and release steps. */
    storeCheck?: boolean;
}>();
const flows = useBuildFlowStore();
const machines = useMachinesStore();
const ui = useUi();
const view = computed(() => session.view!);
const android = computed(() => isAndroid(view.value.profile.provider));
const project = computed(() => view.value.projectVersion);
const draft = computed(() => flows.draft(session.id));
const current = computed(() => draft.value.version ?? project.value);
// One vocabulary on both platforms; only the file the values live in and the store's rule for
// the build number differ.
const labels = computed(() => ({
    version: 'Version',
    build: 'Build number',
    file: android.value ? 'android/app/build.gradle' : 'ios/App/App.xcodeproj/project.pbxproj',
    buildHint: android.value
        ? 'Google Play accepts an upload only with a higher version code than any it has already received.'
        : 'App Store Connect and TestFlight accept an upload only with a higher build number than the last one for this version.',
}));
const versionLabelId = useId();
// Google Play caps the version code; Apple's build number is open-ended in practice.
const buildMax = computed(() => (android.value ? 2_100_000_000 : 999_999_999));
const partNames = ['Major', 'Minor', 'Patch'];
function partName(index: number): string {
    return partNames[index] ?? `Part ${index + 1}`;
}

// An edit that lands back on the project's own values is no edit at all.
function update(patch: Partial<ProjectVersion>): void {
    const next = { ...(current.value ?? { version: '', build: '' }), ...patch };
    draft.value.version = sameVersion(next, project.value) ? null : next;
}
const version = computed({
    get: () => current.value?.version ?? '',
    set: (value: string) => update({ version: value.trim() }),
});
const parts = computed(() => versionParts(version.value));
function setPart(index: number, value: number): void {
    if (parts.value) update({ version: withVersionPart(parts.value, index, value) });
}
const build = computed({
    get: () => current.value?.build ?? '',
    set: (value: string) => update({ build: value.trim() }),
});
const buildNumber = computed(() => (/^\d+$/.test(build.value) ? Number(build.value) : null));
const versionChanged = computed(() => current.value?.version !== project.value?.version);
const buildChanged = computed(() => current.value?.build !== project.value?.build);

// The store's answer. Apple's rule is scoped to a version, so an answer about another version
// says nothing about this one; Google Play's is global.
const store = computed(() => (storeCheck ? session.storeBuilds : null));
const checking = computed(() => store.value?.status === 'checking');
const answer = computed(() =>
    store.value?.status === 'done' && (android.value || store.value.version === version.value)
        ? store.value.result
        : null,
);
const staleAnswer = computed(
    () => store.value?.status === 'done' && !answer.value && !android.value,
);
const mustExceed = computed(() => answer.value?.mustExceed ?? null);
const exceeds = computed(() =>
    mustExceed.value ? buildExceeds(build.value, mustExceed.value.build) : null,
);
const storeName = computed(() =>
    storeLabel(answer.value?.store ?? (android.value ? 'google_play' : 'app_store_connect')),
);
const buildError = computed(() =>
    exceeds.value === false && mustExceed.value
        ? `Not higher than ${mustExceed.value.build}, which ${storeName.value} already has.`
        : null,
);
const nextBuild = computed(() => answer.value?.nextBuild ?? null);
const storeLine = computed(() => {
    const result = answer.value;
    if (!result) return null;
    const held = result.mustExceed;
    if (android.value) {
        return held
            ? `${storeName.value} has version code ${held.build}${held.version ? ` (${held.version})` : ''}; the next upload needs ${result.nextBuild ?? 'a higher one'}.`
            : `${storeName.value} has no uploads for this app yet; any version code is accepted.`;
    }
    if (held) {
        const state = held.state && held.state !== 'VALID' ? `, ${held.state.toLowerCase()}` : '';
        return `${storeName.value} has ${result.version} (${held.build})${state}; the next upload needs ${result.nextBuild ?? 'a higher one'}.`;
    }
    const latest = result.latest
        ? ` Its latest build is ${result.latest.version ?? 'unversioned'} (${result.latest.build}).`
        : '';
    return `${storeName.value} has no build for ${result.version} yet; any build number is accepted.${latest}`;
});
function check(): void {
    void machines.checkStoreBuilds(session.id, version.value || null);
}
</script>

<template>
    <div v-if="project" class="space-y-2">
        <div class="grid gap-4 sm:grid-cols-2">
            <div role="group" :aria-labelledby="versionLabelId" class="block min-w-0">
                <span class="flex items-baseline justify-between gap-2">
                    <span
                        :id="versionLabelId"
                        class="text-[13px] font-medium text-zinc-700 dark:text-zinc-200"
                    >
                        {{ labels.version }}
                    </span>
                    <span class="text-[11px] text-zinc-500 dark:text-zinc-400">
                        {{ versionChanged ? `was ${project.version}` : 'in project' }}
                    </span>
                </span>
                <span class="mt-1.5 block">
                    <span v-if="parts" class="flex items-start gap-1">
                        <template v-for="(part, index) in parts" :key="index">
                            <span
                                v-if="index"
                                class="pt-1.5 text-sm text-zinc-500 dark:text-zinc-400"
                                aria-hidden="true"
                            >
                                .
                            </span>
                            <span class="flex min-w-0 flex-1 flex-col gap-0.5">
                                <NumberField
                                    :model-value="part"
                                    :min="0"
                                    :max="999999"
                                    :disabled="disabled"
                                    :placeholder="partName(index)"
                                    :aria-label="`${labels.version} ${partName(index)}`"
                                    @update:model-value="setPart(index, $event)"
                                />
                                <span
                                    class="text-center text-[11px] text-zinc-500 dark:text-zinc-400"
                                    aria-hidden="true"
                                >
                                    {{ partName(index) }}
                                </span>
                            </span>
                        </template>
                    </span>
                    <Input
                        v-else
                        v-model="version"
                        mono
                        :disabled="disabled"
                        :aria-label="labels.version"
                        placeholder="3.2.1"
                    />
                </span>
            </div>
            <Field :label="labels.build" :hint="labels.buildHint" :error="buildError">
                <template #trailing>{{
                    buildChanged ? `was ${project.build}` : 'in project'
                }}</template>
                <NumberField
                    v-if="buildNumber !== null"
                    :model-value="buildNumber"
                    :min="1"
                    :max="buildMax"
                    :disabled="disabled"
                    @update:model-value="update({ build: String($event) })"
                />
                <Input v-else v-model="build" mono :disabled="disabled" placeholder="16" />
                <template v-if="storeCheck" #action>
                    <Button
                        variant="outline"
                        :disabled="disabled || checking"
                        :title="`Ask ${storeName} which builds it already holds for this app`"
                        @click="check"
                    >
                        <Spinner v-if="checking" />
                        Check the store
                    </Button>
                </template>
            </Field>
        </div>
        <div
            v-if="storeCheck && store && !checking"
            class="flex flex-wrap items-center gap-2 text-xs leading-5"
        >
            <template v-if="store.status === 'failed'">
                <p class="text-red-700 dark:text-red-400">{{ store.error }}</p>
                <!-- Google Play answers through the service account on the publish step; the
                     failure says a key is needed without saying where it is imported. -->
                <Button
                    v-if="android"
                    variant="ghost"
                    size="sm"
                    @click="ui.selectStep(session.id, 'publish')"
                >
                    Connect Google Play
                </Button>
            </template>
            <template v-else-if="answer">
                <p
                    :class="
                        exceeds === false
                            ? 'text-red-700 dark:text-red-400'
                            : 'text-zinc-600 dark:text-zinc-300'
                    "
                >
                    {{ storeLine }}
                </p>
                <Button
                    v-if="nextBuild && build !== nextBuild"
                    variant="ghost"
                    size="sm"
                    :disabled="disabled"
                    @click="update({ build: nextBuild })"
                >
                    Use {{ nextBuild }}
                </Button>
            </template>
            <p v-else-if="staleAnswer" class="text-zinc-500 dark:text-zinc-400">
                The store was asked about {{ store.version }}; check again for
                {{ version || 'this version' }}.
            </p>
        </div>
        <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
            <template v-if="draft.version">
                Written into <span class="font-mono">{{ labels.file }}</span> before the build and
                applied to it, so commit the change with your next commit.
            </template>
            <template v-else>
                Read from <span class="font-mono">{{ labels.file }}</span> and applied to this
                build. Change either to set it; buildbridge writes it into the project as well.
            </template>
        </p>
    </div>
    <p v-else class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
        buildbridge could not read one version and build number from
        <span class="font-mono">{{ labels.file }}</span
        >, so this build carries whatever the project sets. Declare both once in the project, and
        they can be set here.
    </p>
</template>
