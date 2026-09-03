/** Bounded whole numbers, shared by the number field and its tests. */

/**
 * Keeps a typed number inside its field's range. Anything that is not a finite number falls back
 * to the value already held, so a cleared box never becomes zero behind the user's back.
 */
export function clampNumber(value: number, min: number, max: number, fallback: number): number {
    if (!Number.isFinite(value)) {
        return fallback;
    }

    return Math.min(max, Math.max(min, Math.round(value)));
}
