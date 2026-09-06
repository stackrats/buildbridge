<script setup lang="ts">
// The debug build: Gradle's assembleDebug in the container, after the toolchain, the locked
// dependencies, the web assets and Capacitor's sync. The first run also downloads the toolchain.
import { FolderOpen, Hammer, Package, ScrollText } from '@lucide/vue';
import { computed } from 'vue';

import { formatBytes, percent } from '../../../lib/format';
import { androidBuildPhaseLabel } from '../../../model/phases';
import type { JourneyStep } from '../../../model/steps';
import { useBuildFlowStore } from '../../../stores/build-flow';
import { activityLabel, useMachinesStore, type MachineSession } from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import Button from '../../ui/Button.vue';
import CopyButton from '../../ui/CopyButton.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import KeyValue from '../../ui/KeyValue.vue';
import ProgressRow from '../../ui/ProgressRow.vue';
import Spinner from '../../ui/Spinner.vue';
import StepPanel from '../../ui/StepPanel.vue';
import AndroidHttpOption from '../AndroidHttpOption.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const flows = useBuildFlowStore();
const ui = useUi();

const workspace = computed(() => session.view!.android?.workspace ?? null);
const lastBuild = computed(() => workspace.value?.lastBuild ?? null);
const busy = computed(
    () => !!session.operation || !!session.view?.busyOperation || flows.active(session.id),
);
const running = computed(() => step.status === 'running');
const lastLine = computed(() => session.buildLog.at(-1)?.text ?? null);

const strip = computed(() => {
    const operation = session.operation ?? session.view!.busyOperation;
    if (operation === 'test-build' || operation === 'test_building') {
        const progress = session.androidBuild;
        return {
            label: progress ? androidBuildPhaseLabel[progress.phase] : 'Preparing',
            detail: progress?.detail ?? null,
            elapsed: progress?.elapsedSeconds ?? null,
            value: progress ? percent(progress.completedBytes, progress.totalBytes) : null,
            lastLine: lastLine.value,
            stoppable: true,
        };
    }
    return {
        label: activityLabel(operation) ?? 'Working',
        detail: null,
        elapsed: null,
        value: null,
        lastLine: null,
        stoppable: false,
    };
});
const failure = computed(() =>
    session.lastFailure?.operation === 'test-build' ? session.lastFailure : null,
);
// How the APK reaches a phone: adb on this host, with USB debugging on. The command is the
// whole instruction, so it is offered ready to paste.
const apk = computed(() => lastBuild.value?.apk ?? null);
const installCommand = computed(() =>
    apk.value ? `adb install -r '${apk.value.path.replaceAll("'", "'\\''")}'` : null,
);
const details = computed(() =>
    lastBuild.value
        ? [
              { label: 'Application identifier', value: lastBuild.value.applicationId, mono: true },
              {
                  label: 'Version',
                  value: `${lastBuild.value.versionName} (${lastBuild.value.versionCode})`,
              },
              {
                  label: 'JDK',
                  value: lastBuild.value.toolchain.jdkVersion
                      .replace(/^openjdk version /, '')
                      .replace(/"/g, ''),
              },
              { label: 'Build tools', value: lastBuild.value.toolchain.buildToolsVersion },
              {
                  label: 'HTTP APIs',
                  value: lastBuild.value.allowHttp ? 'Allowed for testing' : 'Project settings',
              },
          ]
        : [],
);
</script>

<template>
    <StepPanel :step="step">
        <template #action>
            <Button
                size="sm"
                :disabled="busy || step.status === 'pending'"
                title="Installs the locked dependencies, builds the web assets, synchronizes Capacitor's Android project, and compiles the debug APK"
                @click="machines.debugBuild(session.id, flows.draft(session.id).androidAllowHttp)"
            >
                <Spinner
                    v-if="session.operation === 'test-build'"
                    tone="text-white dark:text-zinc-950"
                />
                <Hammer v-else class="h-3.5 w-3.5" />
                {{
                    workspace?.lastBuildSucceeded
                        ? 'Run the debug build again'
                        : 'Run the debug build'
                }}
            </Button>
            <Button
                v-if="apk"
                variant="outline"
                size="sm"
                @click="machines.revealDebugApk(session.id)"
            >
                <FolderOpen class="h-3.5 w-3.5" />
                Show in folder
            </Button>
            <Button
                v-if="session.buildLog.length"
                variant="ghost"
                size="sm"
                @click="ui.openLog(session.id, 'build')"
            >
                <ScrollText class="h-3.5 w-3.5" />
                Show log
            </Button>
        </template>

        <template v-if="running || failure" #status>
            <ProgressRow
                v-if="running"
                :label="strip.label"
                :detail="strip.detail"
                :elapsed-seconds="strip.elapsed"
                :value="strip.value"
                :last-line="strip.lastLine"
                :stoppable="strip.stoppable"
                :stopping="session.cancelling"
                @stop="machines.cancelOperation(session.id)"
            />
            <FailureBlock
                v-if="failure && !running"
                title="The last debug build failed"
                cause="Review the diagnostic below and the build log, then retry the debug build. If you changed the source, synchronize it before retrying."
                :diagnostic="failure.message"
            >
                <template #actions>
                    <Button variant="outline" size="sm" @click="ui.openLog(session.id, 'build')">
                        Show log
                    </Button>
                </template>
            </FailureBlock>
        </template>

        <template v-if="lastBuild" #result>
            <KeyValue :items="details" :columns="2" />
            <div v-if="apk && installCommand" class="mt-3 space-y-2">
                <div class="flex items-center gap-3">
                    <Package class="h-3.5 w-3.5 shrink-0 text-zinc-500 dark:text-zinc-400" />
                    <span
                        class="w-32 shrink-0 text-xs font-medium text-zinc-700 dark:text-zinc-200"
                    >
                        Debug APK
                    </span>
                    <span
                        class="min-w-0 flex-1 truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400"
                        v-tip="`${apk.path}\nsha256 ${apk.sha256}`"
                    >
                        {{ apk.path }}
                    </span>
                    <span
                        class="shrink-0 font-mono text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
                    >
                        {{ formatBytes(apk.bytes) }}
                    </span>
                    <CopyButton :text="apk.path" label="Copy path" />
                </div>
                <div
                    class="flex items-center gap-2 rounded-md bg-zinc-50 p-2.5 text-xs dark:bg-zinc-950"
                >
                    <span
                        class="min-w-0 flex-1 truncate font-mono text-[11px] text-zinc-600 dark:text-zinc-300"
                        >{{ installCommand }}</span
                    >
                    <CopyButton :text="installCommand" label="Copy command" />
                </div>
                <p class="text-[11px] leading-4 text-zinc-500 dark:text-zinc-400">
                    Installs it on a phone plugged into this host with USB debugging on, or on an
                    emulator running here; the APK is also what any emulator takes by drag and drop.
                    Installing beside a store build requires a different application identifier in
                    your project's debug configuration. With the same identifier and a different
                    signing key, Android rejects the installation.
                </p>
            </div>
        </template>

        <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
            Builds a debug APK from the synchronized source without release credentials. Show it in
            its folder to install it on a phone or emulator. Synchronize first to include project
            changes; the first build also downloads the Android toolchain.
        </p>
        <AndroidHttpOption :session="session" :disabled="busy" />

        <template #details>
            Compiles the debug build type with Gradle. The first run downloads pinned Node and pnpm,
            Google's command-line tools, the build tools and a second JDK into the container's home
            on this host, and accepts the SDK licences; the platforms the project asks for follow
            through its own Gradle plugin, and the project's Gradle wrapper decides which JDK Gradle
            runs on. Gradle runs without a daemon, so a stopped container holds no memory. If this
            desktop restarts mid-build, running it again reattaches to the job instead of starting a
            second one.
        </template>
    </StepPanel>
</template>
