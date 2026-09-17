<script setup lang="ts">
import { computed } from 'vue';

import { liveReloadUrlIssue, type LiveReloadPlatform } from '../../model/live-reload';
import Callout from '../ui/Callout.vue';
import Checkbox from '../ui/Checkbox.vue';
import Field from '../ui/Field.vue';
import Input from '../ui/Input.vue';

const { platform } = defineProps<{ platform: LiveReloadPlatform; disabled: boolean }>();
const enabled = defineModel<boolean>('enabled', { required: true });
const url = defineModel<string>('url', { required: true });
const issue = computed(() => (enabled.value ? liveReloadUrlIssue(url.value, platform) : null));
</script>

<template>
    <div class="space-y-3">
        <Checkbox v-model="enabled" block :disabled="disabled">
            Live reload from a dev server
        </Checkbox>
        <p class="text-xs leading-5 text-zinc-500 dark:text-zinc-400">
            Build the native app once, then see web edits through your dev server's hot reload.
            <template v-if="platform === 'ios'">
                For native code, plugin, or Capacitor settings changes, copy the updated project to
                the machine and build and run again.
            </template>
            <template v-else>
                Changes to native code, plugins, or Capacitor settings need another build and run.
            </template>
        </p>
        <template v-if="enabled">
            <Field
                label="Dev server URL"
                hint="Start your project's dev server before building, and keep it running."
                :error="issue ?? undefined"
            >
                <Input
                    v-model="url"
                    type="url"
                    :placeholder="
                        platform === 'ios' ? 'http://192.168.1.10:5173' : 'http://localhost:5173'
                    "
                    :disabled="disabled"
                    autocomplete="off"
                    mono
                />
            </Field>
            <Callout tone="neutral">
                The dev server's environment controls the live web app.
                <template v-if="platform === 'ios'">
                    Use this computer's LAN address and connect the iPhone to the same Wi-Fi, or use
                    an HTTPS dev server the phone can reach. Listen on the network interface, and
                    allow Local Network access when iOS asks. The hot reload connection must also be
                    reachable from the phone. Stop ends the console session; live reload continues
                    while the server is reachable. The installed app still needs this server when
                    opened again.
                </template>
                <template v-else>
                    For localhost, buildbridge connects the device through ADB; serve hot reload on
                    the same port as the page. Stop removes connections created by buildbridge. The
                    installed APK still needs this server when opened again. For a LAN URL, listen
                    on the network interface and connect the device to a network that can reach the
                    server.
                </template>
            </Callout>
        </template>
    </div>
</template>
