import { describe, expect, it } from 'vite-plus/test';

import { formatCores, formatMemory, memoryTone, rollupTotal, series } from './usage';
import type { UsageSample } from '../types/backend';

const GIB = 1024 ** 3;

function sample(machines: [string, number, number][]): UsageSample {
    return {
        atUnixMs: 0,
        hostCores: 16,
        hostMemoryBytes: 32 * GIB,
        machines: machines.map(([machineId, cpuCores, memoryBytes]) => ({
            machineId,
            cpuCores,
            memoryBytes,
            memoryLimitBytes: 32 * GIB,
            memoryLimited: false,
        })),
    };
}

describe('usage presentation', () => {
    it('sums every machine in a sample and counts them', () => {
        expect(rollupTotal(null)).toEqual({ cpuCores: 0, memoryBytes: 0, machines: 0 });
        expect(
            rollupTotal(
                sample([
                    ['a', 1.5, 2 * GIB],
                    ['b', 0.25, GIB],
                ]),
            ),
        ).toEqual({ cpuCores: 1.75, memoryBytes: 3 * GIB, machines: 2 });
    });

    it('keeps cores honest at both ends', () => {
        expect(formatCores(0)).toBe('0.0');
        expect(formatCores(0.016)).toBe('0.02');
        expect(formatCores(0.5)).toBe('0.5');
        expect(formatCores(2.31667)).toBe('2.3');
        expect(formatCores(Number.NaN)).toBe('0.0');
    });

    it('writes memory in the units a machine profile uses', () => {
        expect(formatMemory(0)).toBe('0 MiB');
        expect(formatMemory(280 * 1024 ** 2)).toBe('280 MiB');
        expect(formatMemory(5.24 * GIB)).toBe('5.2 GiB');
        expect(formatMemory(12.7 * GIB)).toBe('13 GiB');
    });

    it('draws a series oldest first', () => {
        const samples = [sample([['a', 1, 0]]), sample([['a', 2, 0]]), sample([['a', 3, 0]])];
        expect(series(samples, (entry) => rollupTotal(entry).cpuCores)).toEqual([1, 2, 3]);
    });

    it('only a limit of the containers own can be dangerous', () => {
        expect(memoryTone(9, 10, true)).toBe('danger');
        expect(memoryTone(8, 10, true)).toBe('warn');
        expect(memoryTone(5, 10, true)).toBe('neutral');
        expect(memoryTone(9, 10, false)).toBe('warn');
        expect(memoryTone(5, 10, false)).toBe('neutral');
        expect(memoryTone(5, 0, true)).toBe('neutral');
    });
});
