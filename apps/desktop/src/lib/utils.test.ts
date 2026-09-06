import { describe, expect, it } from 'vite-plus/test';

import { clamp, cn, describeError, pushBounded } from './utils';

describe('describeError', () => {
    it('surfaces the message from strings, errors, and message-bearing objects', () => {
        expect(describeError('Docker is not running')).toBe('Docker is not running');
        expect(describeError(new Error('the guest refused the key'))).toBe(
            'the guest refused the key',
        );
        expect(describeError({ message: 'permission denied', code: 13 })).toBe('permission denied');
    });

    it('never shows [object Object] and survives values it cannot serialize', () => {
        expect(describeError({ code: 42 })).toBe('{"code":42}');
        expect(describeError({ message: '   ', code: 42 })).toBe('{"message":"   ","code":42}');
        const circular: Record<string, unknown> = {};
        circular.self = circular;
        expect(describeError(circular)).toBe('An unreadable error was reported.');
        expect(describeError(null)).toBe('null');
        expect(describeError(undefined)).toBe('undefined');
        expect(describeError(7)).toBe('7');
    });
});

describe('small helpers', () => {
    it('keeps the newest entries of a bounded list in place', () => {
        const list = [1, 2, 3];
        pushBounded(list, 4, 3);
        expect(list).toEqual([2, 3, 4]);
        pushBounded(list, 5, 5);
        expect(list).toEqual([2, 3, 4, 5]);
        const none: number[] = [];
        pushBounded(none, 1, 0);
        expect(none).toEqual([]);
    });

    it('clamps into the range and joins only truthy class fragments', () => {
        expect(clamp(5, 0, 3)).toBe(3);
        expect(clamp(-1, 0, 3)).toBe(0);
        expect(clamp(2, 0, 3)).toBe(2);
        expect(cn('a', false, null, undefined, '', 'b')).toBe('a b');
        expect(cn()).toBe('');
    });
});
