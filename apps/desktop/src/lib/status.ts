// How a machine or guest state is rendered. One map per meaning, so a colour carries the same
// message everywhere it appears.

import type { ContainerState } from '../types/backend';

export type BadgeTone = 'neutral' | 'ok' | 'warn' | 'danger' | 'info' | 'primary' | 'outline';

export const machineStateLabel: Record<ContainerState, string> = {
    missing: 'Not created',
    created: 'Created',
    running: 'Running',
    paused: 'Paused',
    restarting: 'Restarting',
    exited: 'Stopped',
    dead: 'Needs attention',
    unavailable: 'Docker unavailable',
    unknown: 'Unknown',
};

export const machineStateBadge: Record<ContainerState, BadgeTone> = {
    missing: 'outline',
    created: 'neutral',
    running: 'ok',
    paused: 'warn',
    restarting: 'warn',
    exited: 'neutral',
    dead: 'danger',
    unavailable: 'danger',
    unknown: 'neutral',
};

/** Dot colour for a machine, where there is only room for a dot. */
export const machineStateDot: Record<ContainerState, string> = {
    missing: 'bg-zinc-300 dark:bg-zinc-600',
    created: 'bg-zinc-300 dark:bg-zinc-600',
    running: 'bg-emerald-500',
    paused: 'bg-amber-500',
    restarting: 'bg-amber-500',
    exited: 'bg-zinc-300 dark:bg-zinc-600',
    dead: 'bg-red-500',
    unavailable: 'bg-red-500',
    unknown: 'bg-zinc-300 dark:bg-zinc-600',
};

export function isLive(state: ContainerState): boolean {
    return state === 'running' || state === 'paused' || state === 'restarting';
}
