import { describe, expect, it } from 'vite-plus/test';

import {
    buildExceeds,
    compareBuilds,
    formatVersion,
    sameVersion,
    storeLabel,
    versionParts,
    withVersionPart,
} from './version';

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

    it('splits a dotted version into whole-number parts and nothing else', () => {
        expect(versionParts('3.2.0')).toEqual([3, 2, 0]);
        expect(versionParts(' 3.2 ')).toEqual([3, 2]);
        expect(versionParts('3.2.0-beta')).toBeNull();
        expect(versionParts('3..0')).toBeNull();
        expect(versionParts('')).toBeNull();
    });

    it('compares build numbers part by part and only when they are whole', () => {
        expect(compareBuilds('16', '15')).toBeGreaterThan(0);
        expect(compareBuilds('15', '15')).toBe(0);
        expect(compareBuilds('9', '15')).toBeLessThan(0);
        expect(compareBuilds('1.0.10', '1.0.3')).toBeGreaterThan(0);
        expect(compareBuilds('1.0', '1.0.0')).toBe(0);
        expect(compareBuilds('1.0-beta', '1')).toBeNull();
        expect(buildExceeds('16', '15')).toBe(true);
        expect(buildExceeds('15', '15')).toBe(false);
        expect(buildExceeds('beta', '15')).toBeNull();
        expect(storeLabel('google_play')).toBe('Google Play');
        expect(storeLabel('app_store_connect')).toBe('TestFlight');
    });

    it('raising a part starts the parts after it again; lowering leaves them', () => {
        expect(withVersionPart([3, 2, 7], 2, 8)).toBe('3.2.8');
        expect(withVersionPart([3, 2, 7], 1, 3)).toBe('3.3.0');
        expect(withVersionPart([3, 2, 7], 0, 4)).toBe('4.0.0');
        expect(withVersionPart([3, 2, 7], 0, 2)).toBe('2.2.7');
        expect(withVersionPart([3, 2, 7], 1, 2)).toBe('3.2.7');
        expect(withVersionPart([3, 2], 1, 3)).toBe('3.3');
    });
});
