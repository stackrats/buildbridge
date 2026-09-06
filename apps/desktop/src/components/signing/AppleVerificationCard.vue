<script setup lang="ts">
// Read-only App Store Connect verification of the stored Team key against one machine's
// approved project, plus the single confirmed mutation BuildBridge performs at Apple: creating
// a replacement App Store profile when none is active.
import { BadgeCheck, Download } from '@lucide/vue';
import { computed, onMounted, ref } from 'vue';

import { formatDate, isExpired } from '../../lib/format';
import { kitHoldsProfile } from '../../model/signing';
import { useMachinesStore } from '../../stores/machines';
import { useSigningStore } from '../../stores/signing';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import Card from '../ui/Card.vue';
import ConfirmDialog from '../dialogs/ConfirmDialog.vue';
import Field from '../ui/Field.vue';
import KeyValue from '../ui/KeyValue.vue';
import Select from '../ui/Select.vue';
import Spinner from '../ui/Spinner.vue';

const signing = useSigningStore();
const machines = useMachinesStore();

const machineOptions = computed(() =>
    machines.machines.value.map((machine) => ({
        value: machine.id,
        label: machine.workspaceName
            ? `${machine.config.name} · ${machine.workspaceName}`
            : `${machine.config.name} · no approved project`,
        disabled: machine.workspaceName === null,
    })),
);
const machineId = ref(
    machines.machines.value.find((machine) => machine.workspaceName !== null)?.id ?? '',
);

// `?verify=1` runs the check on load, so the browser preview can show the result list.
if (
    import.meta.env.DEV &&
    typeof location !== 'undefined' &&
    new URLSearchParams(location.search).has('verify') &&
    machineId.value !== ''
) {
    onMounted(() => void signing.verify(machineId.value));
}

const verification = computed(() => signing.state.verification);
// Verification runs against a machine, because the bundle identifier comes from the project
// approved there, and the credentials come from that machine's attached kit.
const attachedKit = computed(() => {
    const machine = machines.machines.value.find((entry) => entry.id === machineId.value);
    return machine ? signing.kits.value.find((kit) => kit.name === machine.signingKitName) : null;
});
const configured = computed(() => attachedKit.value?.appStoreConnectConfigured ?? false);

const activeProfiles = computed(() =>
    (verification.value?.profiles ?? []).filter(
        (profile) => profile.profileType === 'IOS_APP_STORE' && !isExpired(profile.expirationDate),
    ),
);
const needsReplacement = computed(
    () => verification.value?.profilesAccessible === true && activeProfiles.value.length === 0,
);
const usableCertificates = computed(() =>
    (verification.value?.certificates ?? []).filter(
        (certificate) =>
            (certificate.certificateType === 'DISTRIBUTION' ||
                certificate.certificateType === 'IOS_DISTRIBUTION') &&
            !isExpired(certificate.expirationDate),
    ),
);
const certificateId = ref('');
const selectedCertificate = computed(
    () =>
        usableCertificates.value.find((certificate) => certificate.id === certificateId.value) ??
        null,
);
const confirmOpen = ref(false);

function heldInKit(profile: { uuid: string }): boolean {
    return kitHoldsProfile(attachedKit.value ?? null, profile.uuid);
}

/** Apple's upper-case enum values ("ENABLED", "PROCESSING") as a label. */
function sentenceCase(value: string): string {
    const words = value.toLowerCase().replace(/_/g, ' ');
    return words.charAt(0).toUpperCase() + words.slice(1);
}

function activeProfile(profile: { profileState: string; expirationDate: string }): boolean {
    return !isExpired(profile.expirationDate) && profile.profileState.toUpperCase() === 'ACTIVE';
}

function profileState(profile: { profileState: string; expirationDate: string }) {
    if (isExpired(profile.expirationDate)) {
        return { label: 'Expired', tone: 'danger' as const };
    }
    if (activeProfile(profile)) {
        return { label: 'Active', tone: 'ok' as const };
    }
    return { label: sentenceCase(profile.profileState), tone: 'warn' as const };
}

function certificateState(certificate: { certificateType: string; expirationDate: string }) {
    if (isExpired(certificate.expirationDate)) {
        return { label: 'Expired', tone: 'danger' as const };
    }
    if (
        certificate.certificateType === 'DISTRIBUTION' ||
        certificate.certificateType === 'IOS_DISTRIBUTION'
    ) {
        return { label: 'Distribution · usable', tone: 'ok' as const };
    }
    if (
        certificate.certificateType === 'DEVELOPMENT' ||
        certificate.certificateType === 'IOS_DEVELOPMENT'
    ) {
        return { label: 'Development · for iPhone builds', tone: 'neutral' as const };
    }
    return { label: 'Not for App Store signing', tone: 'neutral' as const };
}

/** How many phones a development or ad hoc profile lists, when Apple let us read them. */
function profileDevices(profile: { profileType: string; deviceUdids?: string[] }): string | null {
    if (profile.profileType !== 'IOS_APP_DEVELOPMENT' && profile.profileType !== 'IOS_APP_ADHOC') {
        return null;
    }
    const count = profile.deviceUdids?.length ?? 0;
    return `${count} device${count === 1 ? '' : 's'}`;
}

async function createProfile(): Promise<void> {
    confirmOpen.value = false;
    await signing.createReplacementProfile(machineId.value, certificateId.value);
}
</script>

<template>
    <Card>
        <template #title>Check a team against Apple</template>
        <template #description>
            Signs a five-minute token locally and makes read-only requests for the app, bundle
            identifier, profiles, and certificates. Nothing is created, revoked, or downloaded. It
            uses the Team key from the credentials attached to the machine you choose, and the
            bundle identifier from the project approved on it.
        </template>

        <div>
            <Field label="Check against the project approved on">
                <Select
                    v-model="machineId"
                    :options="machineOptions"
                    placeholder="Choose a machine"
                />
                <template #action>
                    <Button
                        :disabled="signing.state.verifying || !machineId || !configured"
                        @click="signing.verify(machineId)"
                    >
                        <Spinner
                            v-if="signing.state.verifying"
                            tone="text-white dark:text-zinc-950"
                        />
                        <BadgeCheck v-else class="h-3.5 w-3.5" />
                        {{ verification ? 'Verify again' : 'Verify' }}
                    </Button>
                </template>
            </Field>
            <Callout v-if="!configured" tone="neutral" class="mt-3">
                The credentials attached to this machine have no App Store Connect Team key. Add one
                by editing them; the file route works without it.
            </Callout>
            <Callout v-if="signing.state.verificationError" tone="danger" class="mt-3">
                {{ signing.state.verificationError }}
            </Callout>

            <template v-if="verification">
                <KeyValue
                    class="mt-3"
                    :columns="3"
                    :items="[
                        { label: 'Team key', value: verification.keyId, mono: true },
                        {
                            label: 'Project team',
                            value: verification.projectDevelopmentTeam,
                            mono: true,
                        },
                        {
                            label: 'Bundle identifier',
                            value: verification.bundleIdentifier,
                            mono: true,
                        },
                        {
                            label: 'App Store app',
                            value: verification.appStoreRecordFound
                                ? `${verification.appStoreAppName} (${verification.appStoreAppId})`
                                : 'Not found for this identifier',
                            tone: verification.appStoreRecordFound ? 'ok' : 'warn',
                        },
                        {
                            label: 'Developer bundle ID',
                            value: verification.bundleIdFound
                                ? `${verification.bundleIdName ?? ''} · ${verification.bundleIdPlatform ?? ''}${verification.bundleLookupFallbackUsed ? ' · resolved via inventory' : ''}`
                                : (verification.developerResourcesIssue ?? 'Not found'),
                            tone: verification.bundleIdFound ? 'ok' : 'warn',
                        },
                        {
                            label: 'Verified',
                            value: formatDate(
                                new Date(verification.verifiedAtEpochSeconds * 1000).toISOString(),
                            ),
                        },
                    ]"
                />

                <Callout v-if="verification.profilesIssue" tone="warn" class="mt-3">{{
                    verification.profilesIssue
                }}</Callout>

                <div v-if="verification.profilesAccessible" class="mt-3">
                    <p class="text-xs font-semibold text-zinc-500 dark:text-zinc-400">
                        Provisioning profiles
                    </p>
                    <ul
                        class="mt-1 divide-y divide-zinc-200 rounded-md border border-zinc-200 dark:divide-zinc-800 dark:border-zinc-800"
                    >
                        <li
                            v-for="profile in verification.profiles.slice(0, 8)"
                            :key="profile.id"
                            class="flex items-center gap-2 px-2.5 py-1.5 text-xs"
                        >
                            <span class="min-w-0 flex-1">
                                <span class="block truncate text-zinc-600 dark:text-zinc-300">{{
                                    profile.name
                                }}</span>
                                <span
                                    class="block font-mono text-[11px] text-zinc-500 dark:text-zinc-400"
                                    >{{ profile.profileType }} · {{ profile.uuid
                                    }}<template v-if="profileDevices(profile)">
                                        · {{ profileDevices(profile) }}</template
                                    ></span
                                >
                            </span>
                            <span class="shrink-0 text-[11px] text-zinc-500 dark:text-zinc-400"
                                >until {{ formatDate(profile.expirationDate) }}</span
                            >
                            <Badge :tone="profileState(profile).tone">{{
                                profileState(profile).label
                            }}</Badge>
                            <Button
                                v-if="activeProfile(profile)"
                                variant="outline"
                                size="sm"
                                class="shrink-0"
                                :disabled="
                                    signing.state.downloadingProfileId !== null ||
                                    heldInKit(profile)
                                "
                                :title="
                                    heldInKit(profile)
                                        ? 'Already in the attached credentials'
                                        : undefined
                                "
                                @click="signing.downloadProfile(machineId, profile.id)"
                            >
                                <Spinner v-if="signing.state.downloadingProfileId === profile.id" />
                                <Download v-else class="h-3.5 w-3.5" />
                                {{ heldInKit(profile) ? 'Added' : 'Add' }}
                            </Button>
                        </li>
                        <li
                            v-if="!verification.profiles.length"
                            class="px-2.5 py-2 text-xs text-zinc-500 dark:text-zinc-400"
                        >
                            No profiles for this bundle identifier.
                        </li>
                    </ul>
                    <p
                        v-if="verification.profiles.length > 8"
                        class="mt-1 text-[11px] text-zinc-500 dark:text-zinc-400"
                    >
                        Showing 8 of {{ verification.profiles.length }}.
                    </p>
                </div>

                <div class="mt-3">
                    <p class="text-xs font-semibold text-zinc-500 dark:text-zinc-400">
                        Registered iPhones
                    </p>
                    <ul
                        v-if="verification.devicesAccessible"
                        class="mt-1 divide-y divide-zinc-200 rounded-md border border-zinc-200 dark:divide-zinc-800 dark:border-zinc-800"
                    >
                        <li
                            v-for="device in verification.devices.slice(0, 8)"
                            :key="device.id"
                            class="flex items-center gap-2 px-2.5 py-1.5 text-xs"
                        >
                            <span class="min-w-0 flex-1">
                                <span class="block truncate text-zinc-600 dark:text-zinc-300"
                                    >{{ device.name
                                    }}<template v-if="device.model">
                                        · {{ device.model }}</template
                                    ></span
                                >
                                <span
                                    class="block font-mono text-[11px] text-zinc-500 dark:text-zinc-400"
                                    >{{ device.udid }}</span
                                >
                            </span>
                            <Badge :tone="device.status === 'ENABLED' ? 'ok' : 'warn'">{{
                                sentenceCase(device.status)
                            }}</Badge>
                        </li>
                        <li
                            v-if="!verification.devices.length"
                            class="px-2.5 py-2 text-xs text-zinc-500 dark:text-zinc-400"
                        >
                            No iPhones registered with this team yet; a machine's Run on the device
                            step registers one.
                        </li>
                    </ul>
                    <p v-else class="mt-1 text-[11px] text-zinc-500 dark:text-zinc-400">
                        {{ verification.devicesIssue ?? 'Devices were not accessible.' }}
                    </p>
                    <p
                        v-if="verification.devices.length > 8"
                        class="mt-1 text-[11px] text-zinc-500 dark:text-zinc-400"
                    >
                        Showing 8 of {{ verification.devices.length }}.
                    </p>
                </div>

                <Callout
                    v-if="signing.state.createdProfile"
                    tone="ok"
                    class="mt-3"
                    title="Replacement profile created and retained"
                >
                    {{ signing.state.createdProfile.profile.name }} was saved to
                    <span class="font-mono break-all">{{
                        signing.state.createdProfile.savedPath
                    }}</span>
                    and added to the signing credentials.
                </Callout>

                <template v-else-if="needsReplacement">
                    <Callout tone="warn" class="mt-3" title="No active App Store profile remains">
                        <p>
                            Choose an unexpired distribution certificate whose private key is inside
                            your stored .p12. BuildBridge creates exactly one new App Store profile
                            for the exact bundle identifier and never revokes existing profiles.
                            This needs an Admin Team key.
                        </p>
                        <ul v-if="verification.certificatesAccessible" class="mt-2 space-y-1">
                            <li
                                v-for="certificate in verification.certificates.slice(0, 8)"
                                :key="certificate.id"
                                class="flex items-center gap-2 text-xs"
                            >
                                <span
                                    class="min-w-0 flex-1 truncate text-zinc-600 dark:text-zinc-300"
                                >
                                    {{ certificate.name }} · {{ certificate.displayName }}
                                    <span
                                        class="font-mono text-[11px] text-zinc-500 dark:text-zinc-400"
                                        >{{ certificate.serialNumber }}</span
                                    >
                                </span>
                                <span class="shrink-0 text-[11px] text-zinc-500 dark:text-zinc-400"
                                    >until {{ formatDate(certificate.expirationDate) }}</span
                                >
                                <Badge :tone="certificateState(certificate).tone">{{
                                    certificateState(certificate).label
                                }}</Badge>
                            </li>
                        </ul>
                        <p v-else class="mt-2 text-[11px] text-zinc-500 dark:text-zinc-400">
                            {{
                                verification.certificatesIssue ??
                                'Certificates were not accessible.'
                            }}
                        </p>
                        <Field label="Certificate for the new profile" class="mt-3">
                            <Select
                                v-model="certificateId"
                                :options="
                                    usableCertificates.map((certificate) => ({
                                        value: certificate.id,
                                        label: `${certificate.name} · ${certificate.serialNumber}`,
                                    }))
                                "
                                placeholder="Choose a distribution certificate"
                            />
                            <template #action>
                                <Button
                                    :disabled="
                                        !selectedCertificate || signing.state.creatingProfile
                                    "
                                    @click="confirmOpen = true"
                                >
                                    <Spinner
                                        v-if="signing.state.creatingProfile"
                                        tone="text-white dark:text-zinc-950"
                                    />
                                    Create replacement profile
                                </Button>
                            </template>
                        </Field>
                    </Callout>
                    <Callout v-if="signing.state.profileError" tone="danger" class="mt-2">{{
                        signing.state.profileError
                    }}</Callout>
                </template>
            </template>
        </div>

        <ConfirmDialog
            v-model:open="confirmOpen"
            title="Create a replacement App Store profile at Apple"
            confirm-label="Create profile"
            :destructive="false"
            acknowledgement="The stored .p12 contains the private key for this certificate and the Team key has the Admin role"
            :busy="signing.state.creatingProfile"
            @confirm="createProfile"
        >
            <p>
                Apple will create one new <b>iOS App Store</b> profile for
                <span class="font-mono">{{ verification?.bundleIdentifier }}</span> using
                <b>{{ selectedCertificate?.name }}</b> ({{ selectedCertificate?.serialNumber }}).
            </p>
            <p>
                No existing Apple resource is deleted or revoked. The profile is downloaded to an
                owner-only file and added to the signing credentials.
            </p>
        </ConfirmDialog>
    </Card>
</template>
