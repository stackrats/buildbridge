// Presentation and arithmetic for resource usage: the top-bar readout, the facts line on a
// machine, and the rollups behind them. The engine reports one number per machine; every
// aggregate is derived here from a sample, so nothing has to be kept in step.

import type { UsageSample } from '../types/backend';

export interface UsageTotal {
    cpuCores: number;
    memoryBytes: number;
    /** How many machines the total covers. */
    machines: number;
}

const EMPTY: UsageTotal = { cpuCores: 0, memoryBytes: 0, machines: 0 };

export function rollupTotal(sample: UsageSample | null): UsageTotal {
    if (!sample) {
        return EMPTY;
    }
    return sample.machines.reduce<UsageTotal>(
        (total, machine) => ({
            cpuCores: total.cpuCores + machine.cpuCores,
            memoryBytes: total.memoryBytes + machine.memoryBytes,
            machines: total.machines + 1,
        }),
        EMPTY,
    );
}

/** One number out of each sample, oldest first: the order a sparkline is drawn in. */
export function series(samples: UsageSample[], pick: (sample: UsageSample) => number): number[] {
    return samples.map(pick);
}

/**
 * Cores at a precision that stays honest at both ends: an idle machine reads `0.02`, not
 * `0.0`, and a busy one `2.3`, not `2.31667`.
 */
export function formatCores(cores: number): string {
    if (!Number.isFinite(cores) || cores <= 0) {
        return '0.0';
    }
    return cores < 0.1 ? cores.toFixed(2) : cores.toFixed(1);
}

/**
 * Memory in the binary units a machine profile is written in ("8 GiB"), with a decimal only
 * while the figure is small enough for it to say something.
 */
export function formatMemory(bytes: number): string {
    if (!Number.isFinite(bytes) || bytes <= 0) {
        return '0 MiB';
    }
    const gib = bytes / 1024 ** 3;
    if (gib >= 10) {
        return `${Math.round(gib)} GiB`;
    }
    if (gib >= 1) {
        return `${gib.toFixed(1)} GiB`;
    }
    return `${Math.round(bytes / 1024 ** 2)} MiB`;
}

export type UsageTone = 'neutral' | 'warn' | 'danger';

/**
 * How alarming a memory figure is. Only a limit of the container's own can be approached
 * dangerously: passing it kills the container. Against the host's memory it is a headroom
 * question, so it never rises past a warning.
 */
export function memoryTone(bytes: number, limit: number, limited: boolean): UsageTone {
    if (limit <= 0) {
        return 'neutral';
    }
    const ratio = bytes / limit;
    if (limited) {
        return ratio >= 0.9 ? 'danger' : ratio >= 0.75 ? 'warn' : 'neutral';
    }
    return ratio >= 0.85 ? 'warn' : 'neutral';
}
