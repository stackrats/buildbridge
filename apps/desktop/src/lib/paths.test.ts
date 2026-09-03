import { describe, expect, it } from 'vite-plus/test';

import { baseName, matchesExtension, mergePathList, startDirectory } from './paths';

describe('path helpers', () => {
    it('takes the last segment, with or without a trailing separator', () => {
        expect(baseName('/path/to/distribution.p12')).toBe('distribution.p12');
        expect(baseName('/path/to/project/')).toBe('project');
        expect(baseName('C:\\path\\to\\Xcode.xip')).toBe('Xcode.xip');
        expect(baseName('bare')).toBe('bare');
    });

    it('opens a picker beside the current value', () => {
        expect(startDirectory('/path/to/distribution.p12')).toBe('/path/to');
        expect(startDirectory('C:\\path\\to\\file.p8')).toBe('C:\\path\\to');
        expect(startDirectory('   ')).toBeNull();
        expect(startDirectory(null)).toBeNull();
    });

    it('adds picked paths to a list instead of replacing it', () => {
        const existing = '/path/to/first.mobileprovision';

        expect(mergePathList(existing, ['/path/to/second.mobileprovision'])).toBe(
            '/path/to/first.mobileprovision\n/path/to/second.mobileprovision',
        );
    });

    it('never duplicates a path that is already listed', () => {
        const existing = '/path/to/first.mobileprovision';

        expect(mergePathList(existing, ['/path/to/first.mobileprovision'])).toBe(existing);
    });

    it('tidies blank lines and whitespace while merging', () => {
        expect(mergePathList('  /a.mobileprovision \n\n', ['  /b.mobileprovision  ', '  '])).toBe(
            '/a.mobileprovision\n/b.mobileprovision',
        );
    });

    it('matches extensions case-insensitively', () => {
        expect(matchesExtension('/path/Xcode.XIP', ['xip'])).toBe(true);
        expect(matchesExtension('/path/dist.p12', ['p12', 'pfx'])).toBe(true);
        expect(matchesExtension('/path/dist.cer', ['p12', 'pfx'])).toBe(false);
        expect(matchesExtension('/path/to/folder', ['xip'])).toBe(false);
    });
});
