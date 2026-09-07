<script setup lang="ts">
// How the last run went, in one place: one line saying what is on the phone right now, the
// diagnostic when it failed, and the rest of the facts behind a disclosure rather than above
// the action that runs again.
import { computed } from 'vue';

import { formatDate, shortHash } from '../../lib/format';
import type { MachineSession } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import type { AppleDeviceRunResult } from '../../types/backend';
import Button from '../ui/Button.vue';
import Card from '../ui/Card.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import FailureBlock from '../ui/FailureBlock.vue';
import KeyValue from '../ui/KeyValue.vue';
import Spinner from '../ui/Spinner.vue';

const { session, run, runError, running } = defineProps<{
    session: MachineSession;
    busy: boolean;
    /** A run is in flight: its own strip says so, and a finished outcome is not the news yet. */
    running: boolean;
    run: AppleDeviceRunResult | null;
    runError: string | null;
    failureTitle: string | null;
    failureDiagnostic: string | null;
}>();
const emit = defineEmits<{ clear: [] }>();
const ui = useUi();
const installed = computed(() =>
    run ? formatDate(new Date(run.installedAtEpochSeconds * 1000).toISOString()) : '',
);
const facts = computed(() =>
    run
        ? [
              {
                  label: 'Device',
                  value: `${run.device.name}${run.device.osVersion ? ` · iOS ${run.device.osVersion}` : ''}`,
              },
              {
                  label: 'Bundle identifier',
                  value: run.projectBundleIdentifier
                      ? `${run.bundleIdentifier} · the project's Debug identifier ${run.projectBundleIdentifier} has no development profile`
                      : run.bundleIdentifier,
                  mono: !run.projectBundleIdentifier,
              },
              { label: 'Version', value: `${run.marketingVersion} (${run.buildNumber})` },
              { label: 'Installed', value: installed.value },
              {
                  label: 'Console ended',
                  value:
                      run.consoleEnd === 'stopped'
                          ? 'stopped from here'
                          : run.consoleEnd === 'exited'
                            ? `the app exited${run.exitStatus !== null ? ` (${run.exitStatus})` : ''}`
                            : 'the bridge disconnected',
                  copyable: false,
              },
              {
                  label: 'Profile',
                  value: shortHash(run.provisioningProfileUuid, 8),
                  mono: true,
              },
              { label: 'App path', value: run.appPath, mono: true },
          ]
        : [],
);
</script>

<template>
    <Card v-if="run || runError || failureTitle" tone="well">
        <template #title>Last run</template>
        <template v-if="run || runError" #actions>
            <Button
                variant="ghost"
                size="sm"
                title="Removes the retained run and its diagnostic on this host"
                :disabled="busy"
                @click="emit('clear')"
            >
                <Spinner v-if="session.operation === 'clear-device-run'" />
                Clear last run
            </Button>
        </template>

        <div class="space-y-3">
            <FailureBlock
                v-if="runError && !running"
                title="The last device run failed"
                cause="The diagnostic is kept until the run is cleared. Fix the cause, then build and run again."
                :diagnostic="runError"
            >
                <template v-if="session.deviceLog.length" #actions>
                    <Button variant="outline" size="sm" @click="ui.openLog(session.id, 'device')">
                        Show console
                    </Button>
                </template>
            </FailureBlock>
            <FailureBlock
                v-else-if="failureTitle && !running"
                :title="failureTitle"
                :diagnostic="failureDiagnostic"
            />

            <template v-if="run">
                <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                    {{ run.bundleIdentifier }} {{ run.marketingVersion }} ({{ run.buildNumber }}) is
                    installed on {{ run.device.name }}, from a Debug build on {{ installed }}.
                </p>
                <details>
                    <DisclosureSummary quiet>Details of this run</DisclosureSummary>
                    <div class="mt-2">
                        <KeyValue :items="facts" :columns="3" />
                    </div>
                </details>
            </template>
        </div>
    </Card>
</template>
