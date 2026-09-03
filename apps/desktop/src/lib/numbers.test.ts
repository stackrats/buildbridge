import { describe, expect, it } from 'vite-plus/test';

import { clampNumber } from './numbers';

describe('bounded whole numbers', () => {
    it('keeps a value inside the range', () => {
        expect(clampNumber(8, 4, 64, 8)).toBe(8);
        expect(clampNumber(2, 4, 64, 8)).toBe(4);
        expect(clampNumber(128, 4, 64, 8)).toBe(64);
    });

    it('rounds a fractional entry', () => {
        expect(clampNumber(8.6, 4, 64, 8)).toBe(9);
    });

    it('falls back rather than collapsing to zero when the box is not a number', () => {
        expect(clampNumber(Number.NaN, 4, 64, 8)).toBe(8);
        expect(clampNumber(Number.POSITIVE_INFINITY, 1024, 65535, 50922)).toBe(50922);
    });
});
