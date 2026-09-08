// Narrowing the sidebar's machine list to the one being worked on. A machine answers to its
// name, to the platform it builds for, to the identifier its directories are named by, and to
// whatever its second line currently says — so "android", "running" and "signed" all find
// something without anyone having to remember what a machine was called.
import type { MachineSummary } from '../types/backend';
import { providerPlatform } from './providers';

/** What a machine can be found by, beyond the second line the sidebar renders for it. */
function searchableFields(machine: MachineSummary): string[] {
    return [machine.config.name, machine.id, providerPlatform[machine.config.provider]];
}

/**
 * Whether one machine answers to a query. Matching is case-insensitive and anywhere in the
 * field, since a query is what someone half-remembers rather than a prefix they are sure of.
 */
export function machineMatches(machine: MachineSummary, query: string, secondary: string): boolean {
    const needle = query.trim().toLowerCase();
    if (!needle) {
        return true;
    }
    return [...searchableFields(machine), secondary].some((field) =>
        field?.toLowerCase().includes(needle),
    );
}

/** The machines a query leaves, in the order they were given. An empty query keeps them all. */
export function filterMachines(
    machines: readonly MachineSummary[],
    query: string,
    secondary: (machine: MachineSummary) => string,
): MachineSummary[] {
    if (!query.trim()) {
        return [...machines];
    }
    return machines.filter((machine) => machineMatches(machine, query, secondary(machine)));
}
