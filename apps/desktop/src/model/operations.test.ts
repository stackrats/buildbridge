import { describe, expect, it } from 'vite-plus/test';

import {
    activityLabel,
    busyKeyLabel,
    busyKeyStep,
    operationLabel,
    operationStep,
} from './operations';
import { stepShortTitle } from './steps';

describe('operation vocabulary', () => {
    it('labels a native busy key, this client’s operation id, or nothing at all', () => {
        expect(activityLabel('uploading_google_play')).toBe('Uploading to Google Play');
        expect(activityLabel('upload-google-play')).toBe(operationLabel['upload-google-play']);
        expect(activityLabel('optimizing')).toBe('Applying an optimization');
        expect(activityLabel(null)).toBeNull();
        expect(activityLabel(undefined)).toBeNull();
        expect(activityLabel('')).toBeNull();
        expect(activityLabel('not_a_known_operation')).toBeNull();
        for (const inherited of ['constructor', 'toString', '__proto__', 'hasOwnProperty']) {
            expect(activityLabel(inherited)).toBeNull();
        }
    });

    it('gives every busy key that blocks a step a label', () => {
        for (const key of Object.keys(busyKeyStep)) {
            expect(busyKeyLabel[key], `busy key ${key}`).toBeTruthy();
        }
    });

    it('points every busy key and operation at a step the timeline draws', () => {
        const drawn = new Set(Object.keys(stepShortTitle));
        for (const [key, step] of [
            ...Object.entries(busyKeyStep),
            ...Object.entries(operationStep),
        ]) {
            expect(drawn.has(step), `${key} -> ${step}`).toBe(true);
        }
    });
});
