import { describe, expect, it } from 'vite-plus/test';

import { formatVersion, nextBuild, sameVersion } from './version';

describe('project versions', () => {
    it('prints the version the way the stores do', () => {
        expect(formatVersion({ version: '3.2.0', build: '15' })).toBe('3.2.0 (15)');
        expect(formatVersion(null)).toBeNull();
        expect(formatVersion(undefined)).toBeNull();
    });

    it('compares both halves', () => {
        const version = { version: '3.2.0', build: '15' };
        expect(sameVersion(version, { ...version })).toBe(true);
        expect(sameVersion(version, { ...version, build: '16' })).toBe(false);
        expect(sameVersion(null, null)).toBe(true);
        expect(sameVersion(version, null)).toBe(false);
    });

    it('steps the last run of digits, keeping its width', () => {
        expect(nextBuild('15')).toBe('16');
        expect(nextBuild(' 99 ')).toBe('100');
        expect(nextBuild('1.0.3')).toBe('1.0.4');
        expect(nextBuild('007')).toBe('008');
        expect(nextBuild('')).toBe('1');
        expect(nextBuild('beta')).toBe('1');
    });
});
