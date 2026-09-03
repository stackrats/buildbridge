// How the control-plane connection reads on screen.
//
// The desktop builds on its own; pairing is what adds remote triggering and history. So "not
// paired" is a neutral fact, not a fault, and only a pairing that exists but is not working gets
// warning or failure colour.

import type { RealtimeState } from '../stores/runner';
import type { DesktopStatus } from '../types/backend';

export interface ControlPlaneChip {
    /** Stock Tailwind classes for the status dot. */
    dot: string;
    text: string;
    /** Stock Tailwind classes for the label. */
    tone: string;
    /** Whether the chip is reporting something that needs attention. */
    attention: boolean;
}

const quiet = 'text-zinc-500 dark:text-zinc-400';
const alarm = 'text-red-700 dark:text-red-400';

export function controlPlaneChip(
    status: DesktopStatus | null,
    realtime: RealtimeState,
): ControlPlaneChip {
    if (status?.credentialsMissing) {
        return { dot: 'bg-red-500', text: 'runner token missing', tone: alarm, attention: true };
    }
    if (!status?.paired) {
        return {
            dot: 'bg-zinc-300 dark:bg-zinc-600',
            text: 'local only',
            tone: quiet,
            attention: false,
        };
    }

    switch (realtime) {
        case 'connected':
            return {
                dot: 'bg-emerald-500',
                text: status.runnerName ?? 'paired',
                tone: quiet,
                attention: false,
            };
        case 'connecting':
            return { dot: 'bg-amber-500', text: 'connecting', tone: quiet, attention: false };
        case 'unavailable':
            return {
                dot: 'bg-red-500',
                text: 'control plane unreachable',
                tone: alarm,
                attention: true,
            };
        default:
            return {
                dot: 'bg-amber-500',
                text: 'paired, not connected',
                tone: quiet,
                attention: false,
            };
    }
}
