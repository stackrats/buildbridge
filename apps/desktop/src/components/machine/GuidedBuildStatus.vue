<script setup lang="ts">
import { computed } from 'vue';

import { activityLabel, type MachineSession } from '../../stores/machines';
import { useBuildFlowStore } from '../../stores/build-flow';
import { buildPrerequisite, projectWorkspace, releasePrerequisite } from '../../model/build-flow';
import type { JourneyStepId } from '../../model/steps';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import FailureBlock from '../ui/FailureBlock.vue';
import ProgressRow from '../ui/ProgressRow.vue';

const { session, now } = defineProps<{ session: MachineSession; now: number }>();
const emit = defineEmits<{ review: [step: JourneyStepId]; results: [] }>();
const flows = useBuildFlowStore();
const build = computed(() => flows.builds[session.id]);
const active = computed(() => flows.active(session.id));
const readyToContinue = computed(() => {
    const current = build.value;
    const view = session.view;
    if (!current || !view || current.status !== 'paused' || buildPrerequisite(view)) return false;
    const snapshot = projectWorkspace(view)?.lastSnapshotSha256;
    if (current.preparedSnapshot && snapshot !== current.preparedSnapshot) return false;
    if (!current.preparedSnapshot) return current.request.source === 'latest' || !!snapshot;
    return current.request.outcome === 'test' || releasePrerequisite(view) === null;
});
const detail = computed(() => {
    switch (session.operation) {
        case 'sync':
        case 'test-build':
            return session.androidBuild?.detail ?? session.project?.detail;
        case 'archive':
            return session.archive?.detail;
        case 'release':
            return session.androidRelease?.detail;
        case 'android-run-device':
            return session.androidDevice?.detail ?? null;
        default:
            return null;
    }
});
// The last stage of a build and run is the app itself, up on the device with its log
// streaming. It has arrived rather than being on its way, so this row goes live too: the
// guided flow and the step panel say the same thing about the same run.
const streaming = computed(
    () => session.operation === 'android-run-device' && session.androidDevice?.phase === 'running',
);
const lastLine = computed(() =>
    streaming.value ? (session.deviceLog.at(-1)?.text ?? null) : null,
);
const label = computed(() =>
    streaming.value
        ? `Live on ${build.value?.androidDeviceSerial ?? 'the device'}`
        : build.value?.androidDeviceSerial
          ? 'Building and running on Android'
          : build.value?.request.outcome === 'release'
            ? 'Creating a release'
            : 'Building for testing',
);
</script>

<template>
    <div v-if="build" class="space-y-3" aria-live="polite">
        <ProgressRow
            v-if="active"
            :label="streaming ? label : (activityLabel(session.operation) ?? label)"
            :detail="detail"
            :elapsed-seconds="Math.floor((now - build.startedAt) / 1000)"
            :last-line="lastLine"
            :state="streaming ? 'live' : 'running'"
            stoppable
            :stopping="build.status === 'stopping' || session.cancelling"
            :stop-title="
                streaming
                    ? 'Ends the log session. The app stays installed and running on the device.'
                    : 'Stop the current operation and the remaining build stages.'
            "
            @stop="flows.stop(session.id)"
        />
        <Callout
            v-else-if="build.status === 'paused' && build.blocker"
            :tone="readyToContinue ? 'ok' : 'warn'"
            :title="readyToContinue ? 'Ready to continue' : 'The build needs a decision'"
        >
            <p>
                {{
                    readyToContinue
                        ? 'The prerequisite is complete. Continue the build using the prepared source.'
                        : build.blocker.message
                }}
            </p>
            <p v-if="build.request.outcome === 'release' && build.environmentName" class="mt-1">
                The release uses the current saved values in {{ build.environmentName }}.
            </p>
            <div class="mt-3 flex flex-wrap gap-2">
                <Button size="sm" variant="outline" @click="emit('review', build.blocker.step)">{{
                    build.blocker.action
                }}</Button>
                <Button
                    size="sm"
                    :disabled="
                        !readyToContinue ||
                        session.operation !== null ||
                        session.view?.busyOperation != null
                    "
                    @click="flows.resume(session.id)"
                    >Continue build</Button
                >
                <Button size="sm" variant="ghost" @click="flows.dismiss(session.id)"
                    >Dismiss</Button
                >
            </div>
        </Callout>
        <Callout
            v-else-if="build.status === 'complete'"
            tone="ok"
            :title="
                build.androidDeviceSerial
                    ? 'App installed and opened'
                    : build.request.outcome === 'release'
                      ? 'Release ready'
                      : 'Test build passed'
            "
        >
            <p>
                {{
                    build.request.source === 'latest'
                        ? 'Built from the local source copied for this build.'
                        : 'Built from the saved snapshot.'
                }}
                {{
                    build.environmentName
                        ? `Environment: ${build.environmentName}.`
                        : build.request.source === 'latest'
                          ? 'No environment was applied.'
                          : 'Used the prepared assets and their existing environment.'
                }}
            </p>
            <div class="mt-2 flex flex-wrap gap-2">
                <Button variant="outline" size="sm" @click="emit('results')">View result</Button>
                <Button variant="ghost" size="sm" @click="flows.dismiss(session.id)"
                    >Dismiss</Button
                >
            </div>
        </Callout>
        <Callout v-else-if="build.status === 'stopped'" tone="neutral" title="Build stopped">
            <p>
                No further build stages will start. Review the current source and artifacts before
                trying again.
            </p>
            <div class="mt-2 flex flex-wrap gap-2">
                <Button variant="ghost" size="sm" @click="flows.dismiss(session.id)"
                    >Dismiss</Button
                >
            </div>
        </Callout>
        <FailureBlock
            v-else-if="build.status === 'failed'"
            :title="
                build.failure?.step === 'run-device'
                    ? 'App built, but did not run on the device'
                    : build.androidDeviceSerial
                      ? 'Build and run did not finish'
                      : 'Build did not finish'
            "
            cause="The last operation failed. Open its details for diagnostics, then try again."
            :diagnostic="build.failure?.message"
        >
            <template v-if="build.failure?.deviceSerial" #default>
                <p>Device: {{ build.failure.deviceSerial }}</p>
            </template>
            <template #actions>
                <Button
                    variant="outline"
                    size="sm"
                    @click="
                        emit(
                            'review',
                            build.failure?.step ??
                                (build.stage === 'attach-env' || build.stage === 'sync'
                                    ? 'project'
                                    : (build.stage ?? 'test-build')),
                        )
                    "
                    >Review failed step</Button
                >
                <Button variant="ghost" size="sm" @click="flows.dismiss(session.id)"
                    >Dismiss</Button
                >
            </template>
        </FailureBlock>
    </div>
</template>
