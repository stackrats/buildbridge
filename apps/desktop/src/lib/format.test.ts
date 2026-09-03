import { describe, expect, it } from 'vite-plus/test';

import { formatBytes, formatElapsed, percent, secondsSince, shortHash } from './format';

describe('format helpers', () => {
    it('formats bytes with sensible precision', () => {
        expect(formatBytes(0)).toBe('0 B');
        expect(formatBytes(999)).toBe('999 B');
        expect(formatBytes(7_096_076)).toBe('7.10 MB');
        expect(formatBytes(28_268_787)).toBe('28.3 MB');
        expect(formatBytes(8_500_000_000)).toBe('8.50 GB');
        expect(formatBytes(null)).toBe('—');
    });

    it('formats elapsed time compactly', () => {
        expect(formatElapsed(0)).toBe('0s');
        expect(formatElapsed(49)).toBe('49s');
        expect(formatElapsed(125)).toBe('2m 5s');
        expect(formatElapsed(3 * 3600 + 12 * 60)).toBe('3h 12m');
    });

    it('tolerates Docker timestamps with nine fractional digits', () => {
        const now = Date.parse('2026-09-02T12:00:00.000Z');

        expect(secondsSince('2026-09-02T11:58:30.000123456Z', now)).toBe(90);
        expect(secondsSince(null, now)).toBeNull();
    });

    it('clamps percentages and rejects empty totals', () => {
        expect(percent(50, 100)).toBe(0.5);
        expect(percent(150, 100)).toBe(1);
        expect(percent(10, 0)).toBeNull();
    });

    it('shortens hashes', () => {
        expect(shortHash('abcdef0123456789', 8)).toBe('abcdef01…');
        expect(shortHash('short', 8)).toBe('short');
    });
});
