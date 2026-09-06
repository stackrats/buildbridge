import { describe, expect, it } from 'vite-plus/test';

import { recommendedXcode } from './xcode';

describe('recommendedXcode', () => {
    it('points Sequoia and Tahoe at the current Xcode', () => {
        expect(recommendedXcode('26.6.2').label).toBe('Xcode 26');
        expect(recommendedXcode('15.7').query).toBe('Xcode 26');
    });

    it('caps Sonoma and Ventura at the last Xcode that ran on them', () => {
        expect(recommendedXcode('14.7.1').label).toBe('Xcode 16.2');
        expect(recommendedXcode('13.6').label).toBe('Xcode 15.2');
        expect(recommendedXcode('13.6').reason).toContain('Sequoia or newer');
    });

    it('asks for Xcode without a version when the macOS is unknown', () => {
        expect(recommendedXcode(null)).toEqual({ label: 'Xcode', query: 'Xcode', reason: null });
        expect(recommendedXcode('').label).toBe('Xcode');
    });
});
