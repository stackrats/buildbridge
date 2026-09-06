<script setup lang="ts">
import { CircleCheck, CircleX, RefreshCw } from '@lucide/vue';
import { computed } from 'vue';

import { isAndroid, providerLabel as providerLabels } from '../../../model/providers';
import type { JourneyStep } from '../../../model/steps';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import Button from '../../ui/Button.vue';
import Spinner from '../../ui/Spinner.vue';
import FailureBlock from '../../ui/FailureBlock.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session, step } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();

const prerequisites = computed(() => session.view!.runtime.prerequisites);
const provider = computed(() => session.view!.profile.provider);
const providerLabel = computed(() => providerLabels[provider.value]);
const android = computed(() => isAndroid(provider.value));

// The toolchain container needs Docker and nothing else of the host: no KVM, no display.
const checks = computed(() => [
    {
        label: android.value ? 'Unix host' : 'Linux x86_64 host',
        ok: prerequisites.value.supportedHost,
        detail: prerequisites.value.supportedHost
            ? 'Supported'
            : android.value
              ? `${providerLabel.value} needs a Unix host with Docker`
              : `${providerLabel.value} needs x86_64 Linux with KVM`,
    },
    {
        label: 'Docker engine',
        ok: prerequisites.value.dockerCli && prerequisites.value.dockerDaemon,
        detail: prerequisites.value.dockerVersion ?? 'Install Docker and allow this user to use it',
    },
    ...(android.value ? [] : macChecks.value),
]);

const macChecks = computed(() => [
    {
        label: 'KVM acceleration',
        ok: prerequisites.value.kvmAccess,
        detail: prerequisites.value.kvmAccess
            ? '/dev/kvm is readable and writable'
            : 'Grant read/write access to /dev/kvm',
    },
    provider.value === 'dockur_macos'
        ? {
              label: 'Network tunnel device',
              ok: prerequisites.value.tunAccess,
              detail: prerequisites.value.tunAccess
                  ? '/dev/net/tun is present'
                  : 'dockur/macos needs /dev/net/tun; load the tun module on this host',
          }
        : {
              label: 'X11 display',
              ok: prerequisites.value.displayAccess,
              detail: prerequisites.value.display
                  ? `DISPLAY ${prerequisites.value.display}`
                  : 'Needed for the first-boot macOS console window',
          },
]);
</script>

<template>
    <StepPanel :step="step">
        <template #action>
            <Button
                variant="outline"
                size="sm"
                :disabled="session.operation !== null || session.refreshing"
                @click="machines.refreshMachine(session.id)"
            >
                <Spinner v-if="session.refreshing" />
                <RefreshCw v-else class="h-3.5 w-3.5" />
                Check again
            </Button>
        </template>

        <template v-if="step.status === 'failed'" #status>
            <FailureBlock
                :title="
                    android
                        ? 'This host cannot run the Android toolchain yet'
                        : 'This host cannot run a macOS machine yet'
                "
                cause="Fix what is marked below on the host, then check again. Nothing else in the journey can start until every check passes."
                :diagnostic="prerequisites.issues.join('\n')"
            />
        </template>

        <div class="space-y-3">
            <ul class="grid gap-2 sm:grid-cols-2">
                <li
                    v-for="check in checks"
                    :key="check.label"
                    class="flex items-start gap-2 rounded-md bg-zinc-50 p-2.5 dark:bg-zinc-950"
                >
                    <CircleCheck
                        v-if="check.ok"
                        class="mt-0.5 h-3.5 w-3.5 shrink-0 text-emerald-700 dark:text-emerald-400"
                    />
                    <CircleX
                        v-else
                        class="mt-0.5 h-3.5 w-3.5 shrink-0 text-red-700 dark:text-red-400"
                    />
                    <span class="min-w-0">
                        <span class="block text-xs font-medium text-zinc-700 dark:text-zinc-200">{{
                            check.label
                        }}</span>
                        <span class="block text-xs text-zinc-500 dark:text-zinc-400">{{
                            check.detail
                        }}</span>
                    </span>
                </li>
            </ul>
            <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
                Needed once per host, not per machine.
            </p>
        </div>
    </StepPanel>
</template>
