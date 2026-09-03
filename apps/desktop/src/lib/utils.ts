/** Joins class fragments, dropping falsy entries. */
export function cn(...values: Array<string | false | null | undefined>): string {
    return values.filter(Boolean).join(' ');
}

export function clamp(value: number, min: number, max: number): number {
    return Math.min(max, Math.max(min, value));
}

/** Appends to a bounded list in place, dropping the oldest entries past `limit`. */
export function pushBounded<T>(list: T[], item: T, limit: number): void {
    list.push(item);
    if (list.length > limit) {
        list.splice(0, list.length - limit);
    }
}

export async function copyText(text: string): Promise<void> {
    await navigator.clipboard.writeText(text);
}

export function describeError(error: unknown): string {
    if (typeof error === 'string') {
        return error;
    }
    if (error instanceof Error) {
        return error.message;
    }
    // A plain object stringifies to "[object Object]", which tells nobody anything. Prefer a
    // message field, then the object itself, so the reason survives to the screen.
    if (error !== null && typeof error === 'object') {
        const message = (error as { message?: unknown }).message;
        if (typeof message === 'string' && message.trim() !== '') {
            return message;
        }
        try {
            return JSON.stringify(error);
        } catch {
            return 'An unreadable error was reported.';
        }
    }

    return String(error);
}
