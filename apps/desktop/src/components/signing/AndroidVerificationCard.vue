<script setup lang="ts">
import { ShieldCheck } from '@lucide/vue';
import { computed, ref, watch } from 'vue';

import { formatDate } from '../../lib/format';
import { compareAndroidCertificate } from '../../model/signing';
import { useSigningStore } from '../../stores/signing';
import type { SigningKitSummary } from '../../types/backend';
import Badge from '../ui/Badge.vue';
import Button from '../ui/Button.vue';
import Callout from '../ui/Callout.vue';
import DisclosureSummary from '../ui/DisclosureSummary.vue';
import Field from '../ui/Field.vue';
import Input from '../ui/Input.vue';
import KeyValue from '../ui/KeyValue.vue';
import Spinner from '../ui/Spinner.vue';

const { kit } = defineProps<{ kit: SigningKitSummary }>();
const signing = useSigningStore();
const result = computed(() => signing.state.androidVerification[kit.id]);
const pending = computed(() => signing.state.androidVerifying[kit.id]);
const error = computed(() => signing.state.androidVerificationErrors[kit.id]);
const expected = ref('');
watch(
    () => kit.id,
    () => {
        expected.value = '';
    },
);
const comparison = computed(() =>
    compareAndroidCertificate(expected.value, result.value?.certificateSha256),
);
const details = computed(() =>
    result.value
        ? [
              { label: 'Key alias', value: result.value.keyAlias, mono: true },
              {
                  label: 'Signing key',
                  value: `${result.value.algorithm} · ${result.value.keyBits} bits`,
              },
              {
                  label: 'Certificate expires',
                  value: formatDate(
                      new Date(result.value.validUntilEpochSeconds * 1000).toISOString(),
                  ),
              },
              { label: 'Certificate SHA-256', value: result.value.certificateSha256, mono: true },
              { label: 'Certificate SHA-1', value: result.value.certificateSha1, mono: true },
              {
                  label: 'Last checked',
                  value: new Date(result.value.verifiedAtEpochSeconds * 1000).toLocaleString(),
              },
          ]
        : [],
);
</script>

<template>
    <section
        aria-label="Android signing check"
        class="space-y-3 rounded-md border border-zinc-200 p-3 dark:border-zinc-800"
    >
        <div class="flex flex-wrap items-center justify-between gap-2">
            <div class="flex flex-wrap items-center gap-2">
                <h3 class="text-xs font-semibold text-zinc-900 dark:text-zinc-50">
                    Android signing key
                </h3>
                <Badge v-if="pending" tone="warn">Checking</Badge>
                <Badge v-else-if="error" tone="danger">Check failed</Badge>
                <Badge v-else-if="result" tone="ok">Key verified</Badge>
                <Badge v-else>Not checked</Badge>
            </div>
            <Button
                variant="outline"
                size="sm"
                :disabled="pending || signing.state.saving || signing.state.deleting"
                @click="signing.verifyAndroid(kit.id)"
            >
                <Spinner v-if="pending" />
                <ShieldCheck v-else class="h-3.5 w-3.5" />
                {{ pending ? 'Checking key' : result ? 'Recheck key' : 'Check signing key' }}
            </Button>
        </div>
        <p class="text-xs leading-5 text-zinc-600 dark:text-zinc-300">
            {{
                result
                    ? 'The passwords unlock a working private key with a valid certificate. Every release checks it again before building.'
                    : 'Check the passwords, private key and certificate before a release. Docker is required; the Android toolchain image is downloaded if needed.'
            }}
        </p>
        <Callout v-if="error" tone="danger">{{ error }}</Callout>
        <details v-if="result">
            <DisclosureSummary class="text-xs font-medium text-zinc-700 dark:text-zinc-200">
                Certificate details and comparison
            </DisclosureSummary>
            <div class="mt-3 space-y-3">
                <KeyValue :items="details" :columns="2" />
                <Field
                    label="Expected certificate SHA-256 (optional)"
                    hint="For Google Play, paste the upload key certificate fingerprint from Play Console. For direct APKs, use the certificate of the app you are updating."
                >
                    <Input
                        v-model="expected"
                        mono
                        placeholder="Paste the full SHA-256 fingerprint"
                        autocomplete="off"
                    />
                </Field>
                <p
                    v-if="comparison === 'invalid'"
                    class="text-xs text-amber-700 dark:text-amber-400"
                    role="status"
                >
                    Enter 64 hexadecimal characters, with optional colons or spaces.
                </p>
                <Callout
                    v-else-if="comparison === 'mismatch'"
                    tone="danger"
                    title="Different signing certificate"
                >
                    This key does not match the fingerprint you pasted. Choose the correct
                    credentials before releasing an update.
                </Callout>
                <Callout v-else-if="comparison === 'match'" tone="ok" title="Certificate matches">
                    The checked key matches the fingerprint you pasted.
                </Callout>
                <p class="text-[11px] leading-5 text-zinc-500 dark:text-zinc-400">
                    This comparison is for this check only. Google Play account access and app
                    permissions are not checked. Play's upload key can differ from the key Google
                    uses to sign installed apps.
                </p>
            </div>
        </details>
    </section>
</template>
