import { reactive } from 'vue';

/** Kept by one mounted control, never by a shared store. */
export function createSecretValue(
    load: () => Promise<string>,
    copyText: (value: string) => Promise<void>,
) {
    const state = reactive({
        value: null as string | null,
        pending: null as 'reveal' | 'copy' | null,
        copied: false,
        error: null as string | null,
    });
    let revision = 0;
    let disposed = false;
    let revealTimer: ReturnType<typeof setTimeout> | null = null;
    let copiedTimer: ReturnType<typeof setTimeout> | null = null;

    function clear(): void {
        revision += 1;
        if (revealTimer !== null) clearTimeout(revealTimer);
        if (copiedTimer !== null) clearTimeout(copiedTimer);
        revealTimer = null;
        copiedTimer = null;
        state.value = null;
        state.pending = null;
        state.copied = false;
        state.error = null;
    }

    async function reveal(): Promise<void> {
        if (disposed) return;
        clear();
        const request = revision;
        state.pending = 'reveal';
        try {
            const value = await load();
            if (disposed || request !== revision) return;
            state.value = value;
            revealTimer = setTimeout(clear, 30_000);
        } catch {
            if (!disposed && request === revision) {
                state.error =
                    'Could not read the saved value. Check that the vault or source file is available, then try again.';
            }
        } finally {
            if (!disposed && request === revision) state.pending = null;
        }
    }

    async function copy(): Promise<void> {
        if (disposed) return;
        const request = ++revision;
        if (copiedTimer !== null) clearTimeout(copiedTimer);
        state.pending = 'copy';
        state.copied = false;
        state.error = null;
        let value: string | null = null;
        let reading = true;
        try {
            value = await load();
            if (disposed || request !== revision) return;
            reading = false;
            await copyText(value);
            if (disposed || request !== revision) return;
            state.copied = true;
            copiedTimer = setTimeout(() => (state.copied = false), 1500);
        } catch {
            if (!disposed && request === revision) {
                state.error = reading
                    ? 'Could not read the saved value. Check that the vault or source file is available, then try again.'
                    : 'Could not copy to the clipboard. Try again, or show the value and copy it manually.';
            }
        } finally {
            value = null;
            if (!disposed && request === revision) state.pending = null;
        }
    }

    function dispose(): void {
        clear();
        disposed = true;
    }

    return { state, reveal, copy, clear, dispose };
}
