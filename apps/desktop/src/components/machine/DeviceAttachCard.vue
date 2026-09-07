<script setup lang="ts">
// The phone the run needs, and every rung between a cable and a phone this machine can build
// for: the host rule, the container's USB controller, the attachment itself, Trust, Developer
// Mode and signing. It is a precondition rather than a path, so it stands above the run with
// its own action — the ladder's — and every message about the phone belongs to it rather than
// to a band shared with the build.
import { Circle, CircleCheck, RefreshCw, Smartphone, Unplug } from '@lucide/vue';
import { computed } from 'vue';

import type { ListboxOption } from '../../lib/listbox';
import type { DeviceCheck, DeviceReadiness } from '../../model/device';
import { useMachinesStore, type MachineSession } from '../../stores/machines';
import type { MachineView } from '../../types/backend';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import FailureBlock from '../ui/FailureBlock.vue';
import Field from '../ui/Field.vue';
import Select from '../ui/Select.vue';
import Spinner from '../ui/Spinner.vue';

const { session, readiness, usb, live, busy, checks } = defineProps<{
    session: MachineSession;
    readiness: DeviceReadiness;
    usb: MachineView['usb'];
    live: boolean;
    busy: boolean;
    /** The failure of a phone operation — attaching, pairing, signing — not of a run. */
    failureTitle: string | null;
    failureDiagnostic: string | null;
    usbOptions: ListboxOption[];
    checks: DeviceCheck[];
}>();
const selectedUsb = defineModel<string>('selectedUsb', { required: true });
const machines = useMachinesStore();
const device = computed(() => readiness.device);
const passed = computed(() => checks.filter((check) => check.ok).length);
// A phone this machine already holds can be handed back at any rung past the attachment.
const showDetach = computed(() =>
    (['trust', 'signing', 'developer-mode', 'ready'] as DeviceReadiness['substate'][]).includes(
        readiness.substate,
    ),
);
const showCheckAgain = computed(
    () => readiness.substate === 'signing' && readiness.canPrepareSigning,
);
const issues = computed(() =>
    usb.host.issues.filter((text) => !text.includes('Install the buildbridge')),
);
</script>

<template>
    <Card tone="well">
        <template #title>Your iPhone</template>
        <template #description>
            While it is attached the phone belongs to this machine and is unavailable to other apps
            on this host.
        </template>
        <template #actions>
            <slot name="action" />
            <Button
                v-if="showCheckAgain"
                variant="ghost"
                size="sm"
                :disabled="busy || session.refreshing"
                @click="machines.refreshDevices(session.id)"
            >
                <Spinner v-if="session.refreshing" />
                <RefreshCw v-else class="h-3.5 w-3.5" />
                Check again
            </Button>
            <Button
                v-if="showDetach"
                variant="ghost"
                size="sm"
                :disabled="busy"
                title="Returns the phone to this host. Attaching it to this machine again needs the machine restarted first."
                @click="machines.detachUsb(session.id)"
            >
                <Spinner v-if="session.operation === 'usb-detach'" />
                <Unplug v-else class="h-3.5 w-3.5" />
                Detach
            </Button>
        </template>

        <div class="space-y-3">
            <Field
                v-if="readiness.substate === 'attach' && usbOptions.length > 1"
                class="w-64 max-w-full"
                label="Phone"
                hint="The phone this machine takes over while it is attached."
            >
                <Select
                    v-model="selectedUsb"
                    :options="usbOptions"
                    placeholder="Choose a phone"
                    :disabled="busy"
                />
            </Field>

            <FailureBlock
                v-if="failureTitle"
                :title="failureTitle"
                :diagnostic="failureDiagnostic"
            />
            <Callout v-if="!live && readiness.substate !== 'host-rule'" tone="warn">
                Start the machine to continue; the phone attaches to a running guest.
            </Callout>
            <Callout
                v-if="readiness.substate === 'host-rule' && usb.host.usbmuxdActive"
                tone="neutral"
                title="usbmuxd holds iPhones on this host"
            >
                The rule tells udev not to start usbmuxd for iPhones and hands their device node to
                the plugdev group, which the machine's QEMU user is in. Installing it asks for
                authorization once; unplug the phone and plug it in again afterwards so it applies.
                Host-side iPhone sync stops working for as long as the rule is installed.
            </Callout>
            <Callout v-if="readiness.substate === 'attach' && !live" tone="neutral">
                {{ usb.host.devices.length }} Apple device{{
                    usb.host.devices.length === 1 ? '' : 's'
                }}
                on this host; the machine must be running to take one.
            </Callout>
            <Callout v-for="issue in issues" :key="issue" tone="warn">{{ issue }}</Callout>
            <Callout
                v-if="readiness.substate === 'unplugged'"
                tone="warn"
                title="The phone left this host"
            >
                The guest still holds the port; plug the phone back into the same port and it
                reappears by itself, or detach to hand the port back.
            </Callout>
            <Callout
                v-if="readiness.substate === 'replug'"
                tone="warn"
                title="QEMU holds the phone but could not read it"
            >
                QEMU reads a phone cleanly only the first time it opens it in a session, and this
                phone has been opened before: a detach and re-attach, or the phone re-enumerating
                under QEMU, both leave it here. Restart the machine, then attach once after it is
                up. The phone can stay plugged in; nothing on it changes.
            </Callout>
            <Callout v-if="readiness.substate === 'trust'" tone="neutral" title="On the phone">
                Press <b>Pair with the phone</b>, then unlock the phone and tap <b>Trust</b> when it
                asks about this computer and enter the passcode. Trust alone gives the guest the
                older pairing; the button adds the one <code class="font-mono">devicectl</code> and
                Xcode use, and waits for your tap.
            </Callout>
            <Callout
                v-if="readiness.substate === 'signing' && !readiness.canPrepareSigning"
                tone="warn"
                title="The attached credentials have no Team key"
            >
                Registering the phone and creating a development profile happen at Apple through the
                App Store Connect key in the credentials. Add one under Signing, or store a
                development identity and a profile that already lists this phone.
            </Callout>
            <Callout
                v-if="readiness.substate === 'developer-mode'"
                tone="neutral"
                title="On the phone"
            >
                Open Settings › Privacy &amp; Security › Developer Mode, turn it on, and restart
                when asked. The switch appears only after the first development-signed connection.
                <template v-if="device?.developerMode === 'unknown'">
                    The guest could not read the state yet; it refreshes by itself.
                </template>
            </Callout>

            <details>
                <!-- Every rung at once: the ladder says what to do next, this says where it stands. -->
                <DisclosureSummary quiet
                    >Every check, in order ·
                    <span class="tabular-nums">{{ passed }} of {{ checks.length }} passed</span>
                </DisclosureSummary>
                <div class="mt-2 space-y-3">
                    <ul class="grid gap-2 sm:grid-cols-2">
                        <li
                            v-for="check in checks"
                            :key="check.label"
                            class="flex items-start gap-2 rounded-md bg-white p-2.5 dark:bg-zinc-950"
                        >
                            <CircleCheck
                                v-if="check.ok"
                                class="mt-0.5 h-3.5 w-3.5 shrink-0 text-emerald-700 dark:text-emerald-400"
                            />
                            <Circle
                                v-else
                                class="mt-0.5 h-3.5 w-3.5 shrink-0 text-zinc-400 dark:text-zinc-500"
                            />
                            <span class="min-w-0">
                                <span
                                    class="block text-xs font-medium text-zinc-700 dark:text-zinc-200"
                                    >{{ check.label }}</span
                                >
                                <span
                                    class="block truncate text-xs text-zinc-500 dark:text-zinc-400"
                                    v-tip="check.detail"
                                    >{{ check.detail }}</span
                                >
                            </span>
                        </li>
                    </ul>
                    <Button
                        v-if="usb.host.rule === 'installed'"
                        variant="ghost"
                        size="sm"
                        :disabled="busy"
                        title="Restores usbmuxd handling of iPhones on this host"
                        @click="machines.removeUsbRule(session.id)"
                    >
                        <Spinner v-if="session.operation === 'usb-rule'" />
                        <Smartphone v-else class="h-3.5 w-3.5" />
                        Remove host rule
                    </Button>
                </div>
            </details>
        </div>
    </Card>
</template>
