// Small pure helpers for the path fields. Kept out of components so the merging rules that
// decide what a picker adds to an existing list are unit-tested rather than eyeballed.

/** The last path segment, for showing a file by name without losing the full value. */
export function baseName(path: string): string {
    const trimmed = path.replace(/[\\/]+$/, '');
    const parts = trimmed.split(/[\\/]/);

    return parts[parts.length - 1] ?? trimmed;
}

/** The directory a picker should open in, given a value that may be a file or a folder. */
export function startDirectory(path: string | null | undefined): string | null {
    const value = path?.trim();
    if (!value) {
        return null;
    }
    const separator = value.includes('\\') ? '\\' : '/';
    const index = value.lastIndexOf(separator);

    return index > 0 ? value.slice(0, index) : value;
}

/**
 * Merges picked paths into a newline-separated list.
 *
 * Picking twice should add, not replace, and never duplicate: a person choosing profiles one at
 * a time expects the list to grow.
 */
export function mergePathList(existing: string, picked: string[]): string {
    const lines = existing
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean);
    for (const path of picked) {
        const value = path.trim();
        if (value !== '' && !lines.includes(value)) {
            lines.push(value);
        }
    }

    return lines.join('\n');
}

/** Whether a dropped or picked path looks like the file a field is asking for. */
export function matchesExtension(path: string, extensions: string[]): boolean {
    const lowered = path.toLowerCase();

    return extensions.some((extension) => lowered.endsWith(`.${extension.toLowerCase()}`));
}
