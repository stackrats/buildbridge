import { afterEach, describe, expect, it, vi } from 'vite-plus/test';

import { createSecretValue } from './secret-value';

function deferred<T>() {
    let resolve!: (value: T) => void;
    let reject!: (reason: Error) => void;
    const promise = new Promise<T>((accept, refuse) => {
        resolve = accept;
        reject = refuse;
    });
    return { promise, resolve, reject };
}

afterEach(() => vi.useRealTimers());

describe('saved secret controls', () => {
    it('loads only on demand and copies without revealing the value', async () => {
        const load = vi.fn(async () => 'test secret');
        const write = vi.fn(async (_value: string) => {});
        const control = createSecretValue(load, write);
        expect(load).not.toHaveBeenCalled();
        await control.copy();
        expect(write).toHaveBeenCalledWith('test secret');
        expect(control.state.value).toBeNull();
        expect(control.state.copied).toBe(true);
        control.dispose();
    });

    it('hides a revealed value after thirty seconds', async () => {
        vi.useFakeTimers();
        const control = createSecretValue(
            async () => 'test secret',
            async () => {},
        );
        await control.reveal();
        expect(control.state.value).toBe('test secret');
        vi.advanceTimersByTime(30_000);
        expect(control.state.value).toBeNull();
        control.dispose();
    });

    it.each(['clear', 'dispose'] as const)(
        'does not reveal a late result after %s',
        async (action) => {
            const pending = deferred<string>();
            const control = createSecretValue(
                () => pending.promise,
                async () => {},
            );
            const request = control.reveal();
            control[action]();
            pending.resolve('test secret');
            await request;
            expect(control.state.value).toBeNull();
            expect(control.state.pending).toBeNull();
            control.dispose();
        },
    );

    it('ignores an old reveal when a newer request finishes first', async () => {
        const old = deferred<string>();
        const recent = deferred<string>();
        const load = vi.fn().mockReturnValueOnce(old.promise).mockReturnValueOnce(recent.promise);
        const control = createSecretValue(load, async () => {});
        const first = control.reveal();
        const second = control.reveal();
        recent.resolve('current secret');
        await second;
        old.resolve('old secret');
        await first;
        expect(control.state.value).toBe('current secret');
        control.dispose();
    });

    it('does not copy a loaded value after the control is cleared', async () => {
        const pending = deferred<string>();
        const write = vi.fn(async (_value: string) => {});
        const control = createSecretValue(() => pending.promise, write);
        const request = control.copy();
        control.clear();
        pending.resolve('test secret');
        await request;
        expect(write).not.toHaveBeenCalled();
        expect(control.state.copied).toBe(false);
        control.dispose();
    });

    it('waits for clipboard success and suppresses feedback after navigation', async () => {
        const clipboard = deferred<void>();
        const control = createSecretValue(
            async () => 'test secret',
            () => clipboard.promise,
        );
        const request = control.copy();
        await Promise.resolve();
        expect(control.state.copied).toBe(false);
        control.clear();
        clipboard.resolve();
        await request;
        expect(control.state.copied).toBe(false);
        control.dispose();
    });

    it('reports a clipboard failure without copying its error payload to the screen', async () => {
        const control = createSecretValue(
            async () => 'test secret',
            async () => {
                throw new Error('clipboard rejected test secret');
            },
        );
        await control.copy();
        expect(control.state.copied).toBe(false);
        expect(control.state.error).toContain('Could not copy to the clipboard');
        expect(control.state.error).not.toContain('test secret');
        expect(control.state.value).toBeNull();
        control.dispose();
    });

    it('does not expose a secret through a read error or a late error', async () => {
        const pending = deferred<string>();
        const control = createSecretValue(
            () => pending.promise,
            async () => {},
        );
        const request = control.reveal();
        control.clear();
        pending.reject(new Error('vault returned test secret'));
        await request;
        expect(control.state.error).toBeNull();
        await control.copy();
        expect(control.state.error).toContain('Could not read the saved value');
        expect(control.state.error).not.toContain('test secret');
        control.dispose();
    });
});
