<script setup lang="ts">
import { ExternalLink, FolderOpen, Package } from '@lucide/vue';
import { computed, ref, watch } from 'vue';

import { useBackend } from '../../lib/backend';
import { formatBytes } from '../../lib/format';
import { describeError } from '../../lib/utils';
import type { AndroidArtifact, MachinePlatform } from '../../types/backend';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import CopyButton from '../ui/CopyButton.vue';
import Field from '../ui/Field.vue';
import KeyValue from '../ui/KeyValue.vue';
import PlatformIcon from '../ui/PlatformIcon.vue';
import Select from '../ui/Select.vue';

const props = defineProps<{
    platform: MachinePlatform;
    aab?: AndroidArtifact | null;
    apk?: AndroidArtifact | null;
    ipa?: AndroidArtifact | null;
    appIdentifier?: string | null;
    version?: string | null;
    environment?: string | null;
    certificateSha256?: string | null;
    /** Inside a step card the step row already names the step, so the heading is left out. */
    embedded?: boolean;
}>();
const emit = defineEmits<{ reveal: [path: string]; build: [] }>();
const destination = ref('');
watch(
    () => props.platform,
    (platform) => {
        destination.value =
            platform === 'android' ? (props.aab || !props.apk ? 'play' : 'apk') : 'testflight';
    },
    { immediate: true },
);
const destinations = computed(() =>
    props.platform === 'android'
        ? [
              {
                  value: 'play',
                  label: 'Google Play',
                  description: 'Upload an AAB to a testing or production track',
              },
              {
                  value: 'apk',
                  label: 'Direct APK distribution',
                  description: 'Share an installable APK yourself',
              },
          ]
        : [
              {
                  value: 'testflight',
                  label: 'TestFlight',
                  description: 'Upload an IPA, then invite testers',
              },
              {
                  value: 'app-store',
                  label: 'App Store',
                  description: 'Upload an IPA, then submit a version for review',
              },
          ],
);
const artifact = computed(() =>
    destination.value === 'play' ? props.aab : destination.value === 'apk' ? props.apk : props.ipa,
);
const format = computed(() =>
    destination.value === 'play' ? 'AAB' : destination.value === 'apk' ? 'APK' : 'App Store IPA',
);
const portal = computed(() =>
    props.platform === 'android'
        ? 'https://play.google.com/console/'
        : 'https://appstoreconnect.apple.com/',
);
const help = computed(() =>
    props.platform === 'android'
        ? 'https://support.google.com/googleplay/android-developer/answer/9859348'
        : 'https://developer.apple.com/help/app-store-connect/manage-builds/upload-builds/',
);
const error = ref<string | null>(null);
async function open(url: string): Promise<void> {
    error.value = null;
    try {
        await useBackend().openUrl(url);
    } catch (cause) {
        error.value = describeError(cause);
    }
}
</script>

<template>
    <section class="space-y-4" aria-label="Publishing guide">
        <header>
            <h2 v-if="!embedded" class="flex items-center gap-2 text-base font-semibold">
                <PlatformIcon :platform="platform" class="h-4 w-4" />Publish the release
            </h2>
            <p
                class="text-xs leading-5 text-zinc-500 dark:text-zinc-400"
                :class="{ 'mt-1': !embedded }"
            >
                Choose where this release will go.
                <template v-if="platform === 'android' && $slots['google-play-upload']">
                    Upload an internal testing draft to Google Play, then finish the rollout in Play
                    Console. Direct APK distribution is also available.
                </template>
                <template v-else-if="$slots.upload">
                    Upload with Transporter or follow the manual guide below. Apple processes the
                    build before you can select it for testing or review.
                </template>
                <template v-else>
                    Follow the upload guide below; publication is not tracked here.
                </template>
            </p>
        </header>
        <Field label="Destination"><Select v-model="destination" :options="destinations" /></Field>
        <Callout v-if="error" tone="danger">{{ error }}</Callout>
        <Callout v-if="!artifact" tone="neutral" :title="`Build a signed ${format} first`">
            This destination needs a retained {{ format }}. A debug build cannot be published as a
            store release.
            <Button variant="outline" size="sm" class="mt-2" @click="emit('build')"
                >Choose release outputs</Button
            >
        </Callout>
        <div
            v-else
            class="space-y-3 rounded-lg border border-zinc-200 bg-white p-4 dark:border-zinc-800 dark:bg-zinc-900"
        >
            <h3 class="flex items-center gap-2 text-sm font-semibold">
                <Package class="h-4 w-4" />Retained {{ format }}
            </h3>
            <KeyValue
                :items="[
                    { label: 'App identifier', value: appIdentifier, mono: true },
                    { label: 'Version', value: version },
                    { label: 'Environment', value: environment || 'Not recorded' },
                    { label: 'File size', value: formatBytes(artifact.bytes) },
                    { label: 'Artifact SHA-256', value: artifact.sha256, mono: true },
                    ...(certificateSha256
                        ? [
                              {
                                  label: 'Signing certificate SHA-256',
                                  value: certificateSha256,
                                  mono: true,
                              },
                          ]
                        : []),
                ]"
            />
            <p class="font-mono text-[11px] break-all text-zinc-500 dark:text-zinc-400">
                {{ artifact.path }}
            </p>
            <div class="flex flex-wrap gap-2">
                <Button variant="outline" size="sm" @click="emit('reveal', artifact.path)"
                    ><FolderOpen class="h-3.5 w-3.5" />Show in folder</Button
                >
                <CopyButton :text="artifact.path" what="Copy the file path" size="iconSm" />
            </div>
            <p class="text-[11px] leading-5 text-zinc-500 dark:text-zinc-400">
                This is the retained release, which may predate your latest source or environment
                changes. Review it before uploading.
            </p>
        </div>
        <!-- The Google Play card holds the service-account key as well as the upload, so it
             stays on the step before a release is retained: the connection is made once, and
             the upload button names whatever is still missing. -->
        <slot v-if="destination === 'play'" name="google-play-upload" />
        <slot v-else-if="artifact && platform !== 'android'" name="upload" />
        <template v-if="artifact">
            <ol
                class="list-decimal space-y-3 pl-5 text-xs leading-5 text-zinc-700 dark:text-zinc-300"
            >
                <template v-if="destination === 'play'">
                    <li>
                        Open this app in Play Console. Confirm its package name and upload key
                        certificate match the app identifier and signing certificate shown above.
                    </li>
                    <li>
                        <template v-if="$slots['google-play-upload']"
                            >Use Upload to Google Play above to save an internal testing draft, or
                            upload the AAB manually to your chosen track.</template
                        >
                        <template v-else
                            >Choose a testing or production track, create a release, and upload this
                            AAB.</template
                        >
                        Resolve Play's validation messages and add release notes.
                    </li>
                    <li>
                        Review the track, countries, testers and rollout settings before submitting.
                        Store listing, app content and account requirements are completed in Play
                        Console.
                    </li>
                </template>
                <template v-else-if="destination === 'apk'">
                    <li>
                        Test the signed APK on a phone or emulator. Updates need a compatible
                        signing certificate and a higher version code.
                    </li>
                    <li>
                        Share the APK file with testers or upload it to your download service. An
                        AAB cannot be installed directly.
                    </li>
                    <li>
                        Provide installation instructions for your users. A Play upload-key-signed
                        APK may not update an app installed from Google Play when its app signing
                        key differs.
                    </li>
                </template>
                <template v-else>
                    <li>
                        Open the matching app in App Store Connect and confirm the bundle
                        identifier, version and account access.
                    </li>
                    <li>
                        <template v-if="$slots.upload"
                            >Use Upload with Transporter above, or on</template
                        >
                        <template v-else>On</template>
                        a Mac, deliver this IPA with Apple's Transporter app. Alternatively, open
                        the extracted Xcode archive in Xcode's Organizer and upload it. Wait for
                        Apple to process the build.
                    </li>
                    <li v-if="destination === 'testflight'">
                        Select the processed build in TestFlight, complete compliance questions and
                        configure testers. External testing may require beta review.
                    </li>
                    <li v-else>
                        Select the processed build for an App Store version, complete the listing
                        and compliance details, then review and submit it to Apple.
                    </li>
                </template>
            </ol>
            <div v-if="destination !== 'apk'" class="flex flex-wrap gap-2">
                <Button size="sm" @click="open(portal)"
                    ><ExternalLink class="h-3.5 w-3.5" />Open
                    {{ platform === 'android' ? 'Play Console' : 'App Store Connect' }}</Button
                >
                <Button variant="outline" size="sm" @click="open(help)">Upload instructions</Button>
            </div>
        </template>
    </section>
</template>
