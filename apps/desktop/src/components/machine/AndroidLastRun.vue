<script setup lang="ts">
// How the last run went, in one place. Both paths above end here, so the outcome of the run
// that just finished and the record of the one kept beside the machine are the same card: one
// line saying what is on the phone right now, its diagnostic when it failed, and the rest of
// the facts behind a disclosure rather than above the two actions that matter.
import { computed } from 'vue';

import { formatDate } from '../../lib/format';
import type { AndroidDeviceRun } from '../../model/android-device';
import { useMachinesStore, type MachineSession } from '../../stores/machines';
import { useUi } from '../../stores/ui';
import type { StoredAndroidDeviceRun } from '../../types/backend';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import FailureBlock from '../ui/FailureBlock.vue';
import KeyValue from '../ui/KeyValue.vue';
import Spinner from '../ui/Spinner.vue';

const { session, busy, running, run, kept, keptError } = defineProps<{
    session: MachineSession;
    busy: boolean;
    /** A run is in flight: its own strip says so, and a finished outcome is not the news yet. */
    running: boolean;
    run: AndroidDeviceRun | null;
    kept: StoredAndroidDeviceRun | null;
    keptError: string | null;
}>();
const machines = useMachinesStore();
const ui = useUi();
const device = computed(() =>
    kept
        ? kept.result.model
            ? `${kept.result.model} · ${kept.result.serial}`
            : kept.result.serial
        : '',
);
const installed = computed(() =>
    kept ? formatDate(new Date(kept.result.installedAtEpochSeconds * 1000).toISOString()) : '',
);
const facts = computed(() =>
    kept
        ? [
              { label: 'Device', value: device.value },
              { label: 'App identifier', value: kept.result.applicationId, mono: true },
              {
                  label: 'Version',
                  value: `${kept.versionName} (${kept.versionCode}) · ${
                      kept.kind === 'debug' ? 'debug APK' : 'release APK'
                  }`,
              },
              { label: 'Installed', value: installed.value },
              {
                  label: 'Log session ended',
                  value:
                      kept.result.consoleEnd === 'stopped'
                          ? 'stopped from here'
                          : kept.result.consoleEnd === 'exited'
                            ? 'the app exited'
                            : 'the device disconnected',
                  copyable: false,
              },
              {
                  label: 'Process',
                  value: kept.result.pid !== null ? String(kept.result.pid) : 'not read',
                  copyable: false,
              },
          ]
        : [],
);
function clearRun(): void {
    if (busy) return;
    void machines.clearAndroidDeviceRun(session.id);
}
</script>

<template>
    <Card v-if="run || kept || keptError" tone="well">
        <template #title>Last run</template>
        <template v-if="kept || keptError" #actions>
            <Button
                variant="ghost"
                size="sm"
                title="Removes the retained run and its diagnostic on this host"
                :disabled="busy"
                @click="clearRun"
            >
                <Spinner v-if="session.operation === 'clear-android-device-run'" />
                Clear last run
            </Button>
        </template>

        <div class="space-y-3">
            <div v-if="run && !running" aria-live="polite">
                <Callout v-if="run.status === 'complete'" tone="ok">
                    {{ run.result?.applicationId }} ran on {{ run.serial }}. The app stays installed
                    and its log is in the drawer.
                </Callout>
                <FailureBlock
                    v-else-if="run.status === 'failed'"
                    title="The device run did not finish"
                    cause="The diagnostic is below. Fix the cause, then install and open again."
                    :diagnostic="run.error"
                >
                    <template v-if="session.deviceLog.length" #actions>
                        <Button
                            variant="outline"
                            size="sm"
                            @click="ui.openLog(session.id, 'device')"
                        >
                            Show log
                        </Button>
                    </template>
                </FailureBlock>
            </div>
            <FailureBlock
                v-else-if="keptError && !running"
                title="The last device run failed"
                cause="The diagnostic is kept until the run is cleared. Fix the cause, then install and open again."
                :diagnostic="keptError"
            />

            <template v-if="kept">
                <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
                    {{ kept.result.applicationId }} {{ kept.versionName }} ({{ kept.versionCode }})
                    is installed on {{ device }}, from
                    {{ kept.kind === 'debug' ? 'a debug APK' : 'a release APK' }} on
                    {{ installed }}.
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
