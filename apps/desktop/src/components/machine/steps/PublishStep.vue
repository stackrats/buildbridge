<script setup lang="ts">
import { computed } from 'vue';

import { isAndroid } from '../../../model/providers';
import type { JourneyStep } from '../../../model/steps';
import { useMachinesStore, type MachineSession } from '../../../stores/machines';
import { useUi } from '../../../stores/ui';
import PublishingGuide from '../PublishingGuide.vue';
import AppleArchiveUpload from '../AppleArchiveUpload.vue';
import GooglePlayUpload from '../GooglePlayUpload.vue';
import StepPanel from '../../ui/StepPanel.vue';

const { session } = defineProps<{ session: MachineSession; step: JourneyStep }>();
const machines = useMachinesStore();
const ui = useUi();
const view = computed(() => session.view!);
const android = computed(() => isAndroid(view.value.profile.provider));
const release = computed(() => view.value.android?.release);
const archive = computed(() => view.value.archive);
function build(): void {
    ui.selectStep(session.id, android.value ? 'release' : 'archive');
}
</script>

<template>
    <StepPanel :step="step">
        <!-- The guide is the whole step, so it takes the status slot: the default slot would
             draw a hairline above it with nothing to separate. -->
        <template #status>
            <PublishingGuide
                embedded
                :platform="android ? 'android' : 'ios'"
                :aab="release?.aab"
                :apk="release?.apk"
                :ipa="archive?.ipa"
                :app-identifier="android ? release?.applicationId : archive?.bundleIdentifier"
                :version="
                    android
                        ? release
                            ? `${release.versionName} (${release.versionCode})`
                            : null
                        : archive
                          ? `${archive.marketingVersion} (${archive.buildNumber})`
                          : null
                "
                :environment="android ? view.android?.releaseEnvSet : view.archiveEnvSet"
                :certificate-sha256="android ? release?.certificateSha256 : null"
                @reveal="
                    android
                        ? machines.revealRelease(session.id)
                        : machines.revealArchive(session.id)
                "
                @build="build"
            >
                <template v-if="!android" #upload>
                    <AppleArchiveUpload :session="session" />
                </template>
                <template v-if="android" #google-play-upload>
                    <GooglePlayUpload :session="session" />
                </template>
            </PublishingGuide>
        </template>
    </StepPanel>
</template>
