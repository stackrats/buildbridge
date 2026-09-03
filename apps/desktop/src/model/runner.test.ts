import { describe, expect, it } from 'vite-plus/test';

import type { DesktopStatus } from '../types/backend';
import { controlPlaneChip } from './runner';

function status(overrides: Partial<DesktopStatus> = {}): DesktopStatus {
    return {
        paired: false,
        credentialsMissing: false,
        serverUrl: null,
        runnerId: null,
        runnerName: null,
        platform: 'linux',
        architecture: 'x86_64',
        version: '0.1.0',
        ...overrides,
    };
}

describe('control plane chip', () => {
    it('treats an unpaired desktop as a normal, local-only state', () => {
        const chip = controlPlaneChip(status(), 'disconnected');

        expect(chip.text).toBe('local only');
        expect(chip.attention).toBe(false);
        expect(chip.tone).not.toContain('red');
    });

    it('does the same before status has loaded at all', () => {
        expect(controlPlaneChip(null, 'disconnected').attention).toBe(false);
    });

    it('names the runner once connected', () => {
        const chip = controlPlaneChip(
            status({ paired: true, runnerName: 'linux-builder' }),
            'connected',
        );

        expect(chip.text).toBe('linux-builder');
        expect(chip.dot).toContain('emerald');
    });

    it('raises attention only when a pairing exists and is not working', () => {
        expect(controlPlaneChip(status({ paired: true }), 'unavailable').attention).toBe(true);
        expect(controlPlaneChip(status({ paired: true }), 'disconnected').attention).toBe(false);
        expect(
            controlPlaneChip(status({ credentialsMissing: true }), 'disconnected'),
        ).toMatchObject({ text: 'runner token missing', attention: true });
    });
});
