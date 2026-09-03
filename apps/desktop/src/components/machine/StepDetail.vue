<script setup lang="ts">
// One step's panel, chosen by id. Kept as an explicit list so every step component keeps its
// typed props; the machine page renders this beside the rail, or under the selected row when
// the window is narrow.
import type { JourneyStep } from '../../model/steps';
import type { MachineSession } from '../../stores/machines';
import AccessStep from './steps/AccessStep.vue';
import ApproveStep from './steps/ApproveStep.vue';
import ArchiveStep from './steps/ArchiveStep.vue';
import HostStep from './steps/HostStep.vue';
import InstallStep from './steps/InstallStep.vue';
import LaunchStep from './steps/LaunchStep.vue';
import ProvisionStep from './steps/ProvisionStep.vue';
import RunDeviceStep from './steps/RunDeviceStep.vue';
import SigningKitStep from './steps/SigningKitStep.vue';
import SyncStep from './steps/SyncStep.vue';
import TestBuildStep from './steps/TestBuildStep.vue';
import TrustStep from './steps/TrustStep.vue';
import XcodeActivateStep from './steps/XcodeActivateStep.vue';
import XcodeImportStep from './steps/XcodeImportStep.vue';

defineProps<{ session: MachineSession; step: JourneyStep }>();
</script>

<template>
    <HostStep v-if="step.id === 'host'" :session="session" :step="step" />
    <LaunchStep v-else-if="step.id === 'launch'" :session="session" :step="step" />
    <InstallStep v-else-if="step.id === 'install'" :session="session" :step="step" />
    <TrustStep v-else-if="step.id === 'trust'" :session="session" :step="step" />
    <AccessStep v-else-if="step.id === 'access'" :session="session" :step="step" />
    <XcodeImportStep v-else-if="step.id === 'xcode-import'" :session="session" :step="step" />
    <XcodeActivateStep v-else-if="step.id === 'xcode-activate'" :session="session" :step="step" />
    <ApproveStep v-else-if="step.id === 'approve'" :session="session" :step="step" />
    <SyncStep v-else-if="step.id === 'sync'" :session="session" :step="step" />
    <TestBuildStep v-else-if="step.id === 'test-build'" :session="session" :step="step" />
    <SigningKitStep v-else-if="step.id === 'signing-kit'" :session="session" :step="step" />
    <ProvisionStep v-else-if="step.id === 'provision'" :session="session" :step="step" />
    <ArchiveStep v-else-if="step.id === 'archive'" :session="session" :step="step" />
    <RunDeviceStep v-else :session="session" :step="step" />
</template>
