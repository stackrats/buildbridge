// The version a build carries, as both stores print it, and the small arithmetic the version
// fields do on its parts: a bump of one part starts the parts after it again, the way a minor
// bump resets the patch.
import type { ProjectVersion, StoreBuildsCheck } from '../types/backend';

/** A store's answer about the builds it holds, as the session keeps it while and after asking. */
export interface StoreBuildsState {
    /** The version the store was asked about; Apple's rule is scoped to it. */
    version: string;
    status: 'checking' | 'done' | 'failed';
    result: StoreBuildsCheck | null;
    error: string | null;
}

export function storeLabel(store: string): string {
    return store === 'google_play' ? 'Google Play' : 'TestFlight';
}

/** `3.2.0 (15)`, or null when the project declares no version buildbridge can read. */
export function formatVersion(version: ProjectVersion | null | undefined): string | null {
    return version ? `${version.version} (${version.build})` : null;
}

export function sameVersion(
    a: ProjectVersion | null | undefined,
    b: ProjectVersion | null | undefined,
): boolean {
    return a?.version === b?.version && a?.build === b?.build;
}

/** The whole-number parts of a dotted version such as `3.2.0`; null when any part is not one. */
export function versionParts(version: string): number[] | null {
    const parts = version.trim().split('.');
    return parts.every((part) => /^\d+$/.test(part)) ? parts.map(Number) : null;
}

/**
 * The version with one part replaced. Raising a part resets the parts after it to 0, so a
 * minor bump reads `3.3.0` rather than `3.3.7`; lowering or retyping one leaves them alone.
 */
export function withVersionPart(parts: number[], index: number, value: number): string {
    const raised = value > (parts[index] ?? 0);
    return parts
        .map((part, at) => (at === index ? value : at > index && raised ? 0 : part))
        .join('.');
}

/**
 * Dotted whole numbers compared part by part, so `16` beats `15` and `1.0.10` beats `1.0.3`;
 * null when either is not that shape, since the store's own comparison is then unknowable.
 */
export function compareBuilds(a: string, b: string): number | null {
    const parts = (value: string): number[] | null => {
        const pieces = value.trim().split('.');
        return pieces.every((part) => /^\d+$/.test(part)) ? pieces.map(Number) : null;
    };
    const left = parts(a);
    const right = parts(b);
    if (!left || !right) return null;
    const length = Math.max(left.length, right.length);
    for (let index = 0; index < length; index += 1) {
        const difference = (left[index] ?? 0) - (right[index] ?? 0);
        if (difference !== 0) return difference;
    }
    return 0;
}

/** Whether a build number would be accepted over the one the store holds; null when unknown. */
export function buildExceeds(entered: string, held: string): boolean | null {
    const comparison = compareBuilds(entered, held);
    return comparison === null ? null : comparison > 0;
}
