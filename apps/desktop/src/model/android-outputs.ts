import type { AndroidReleaseOutputs } from '../types/backend';

export const androidOutputOptions = [
    { value: 'both', label: 'AAB and APK', description: 'Google Play and direct installation' },
    { value: 'aab', label: 'App bundle (AAB)', description: 'Upload to Google Play' },
    { value: 'apk', label: 'APK', description: 'Install or distribute directly' },
] satisfies Array<{ value: AndroidReleaseOutputs; label: string; description: string }>;

export const androidOutputLabel: Record<AndroidReleaseOutputs, string> = {
    both: 'AAB and APK',
    aab: 'app bundle (AAB)',
    apk: 'APK',
};
