// The version a build carries, as both stores print it, and the one piece of arithmetic the
// version fields offer: the next build number, because every store upload needs a new one.
import type { ProjectVersion } from '../types/backend';

/** `3.2.0 (15)`, or null when the project declares no version BuildBridge can read. */
export function formatVersion(version: ProjectVersion | null | undefined): string | null {
    return version ? `${version.version} (${version.build})` : null;
}

export function sameVersion(
    a: ProjectVersion | null | undefined,
    b: ProjectVersion | null | undefined,
): boolean {
    return a?.version === b?.version && a?.build === b?.build;
}

/**
 * The build after this one: the last run of digits plus one, keeping its width, so `15`
 * becomes `16`, `1.0.3` becomes `1.0.4` and `007` becomes `008`. A build with no digits
 * starts at 1.
 */
export function nextBuild(build: string): string {
    const match = /^(.*?)(\d+)(\D*)$/.exec(build.trim());
    if (!match) return '1';
    const [, head = '', digits = '0', tail = ''] = match;
    return `${head}${String(Number(digits) + 1).padStart(digits.length, '0')}${tail}`;
}
