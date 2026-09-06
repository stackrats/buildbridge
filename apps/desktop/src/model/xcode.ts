// Which Xcode a guest should get, from the macOS it runs. Apple ships one current Xcode per
// macOS generation and the older releases stop at the last Xcode that ran on them, so the
// recommendation is a major version and a search on Apple's downloads page.

export interface XcodeRecommendation {
    /** What the button and the search say, for example `Xcode 26`. */
    label: string;
    /** The query for Apple's downloads page. */
    query: string;
    /** Why this one, in a sentence, or null when the macOS is unknown. */
    reason: string | null;
}

export function recommendedXcode(macosVersion: string | null | undefined): XcodeRecommendation {
    const major = Number.parseInt((macosVersion ?? '').split('.')[0] ?? '', 10);
    if (major >= 15) {
        return {
            label: 'Xcode 26',
            query: 'Xcode 26',
            reason: `macOS ${macosVersion} runs the current Xcode, which the signed archive and the phone build were proven on.`,
        };
    }
    if (major === 14) {
        return {
            label: 'Xcode 16.2',
            query: 'Xcode 16.2',
            reason: `macOS ${macosVersion} stops at Xcode 16.2; Xcode 26 needs Sequoia or newer.`,
        };
    }
    if (major === 13) {
        return {
            label: 'Xcode 15.2',
            query: 'Xcode 15.2',
            reason: `macOS ${macosVersion} stops at Xcode 15.2; Xcode 26 needs Sequoia or newer.`,
        };
    }
    return { label: 'Xcode', query: 'Xcode', reason: null };
}
