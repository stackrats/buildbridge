import { reactive } from 'vue';

import { loadMachineOrder, saveMachineOrder } from '../lib/prefs';
import type { MachineSummary } from '../types/backend';

export function createMachineOrderStore() {
    const preferences = reactive(loadMachineOrder());

    function sortSidebar(machines: readonly MachineSummary[]): MachineSummary[] {
        const positions = new Map(preferences.ids.map((id, index) => [id, index]));
        return [...machines].sort(
            (a, b) =>
                (positions.get(a.id) ?? positions.size) - (positions.get(b.id) ?? positions.size),
        );
    }

    function recent(machine: MachineSummary): number {
        const lastUsed = Object.hasOwn(preferences.lastUsed, machine.id)
            ? preferences.lastUsed[machine.id]!
            : 0;
        return Math.max(machine.createdAtEpochSeconds * 1000, lastUsed);
    }

    return {
        sortSidebar,
        sortDashboard(machines: readonly MachineSummary[]): MachineSummary[] {
            return sortSidebar(machines).sort(
                (a, b) =>
                    Number(b.state === 'running') - Number(a.state === 'running') ||
                    recent(b) - recent(a),
            );
        },
        reorder(ids: readonly string[]): void {
            preferences.ids = [...new Set(ids.filter((id) => !!id))];
            saveMachineOrder(preferences);
        },
        markUsed(id: string): void {
            if (!id) return;
            preferences.lastUsed = { ...preferences.lastUsed, [id]: Date.now() };
            saveMachineOrder(preferences);
        },
    };
}

const store = createMachineOrderStore();

export function useMachineOrder() {
    return store;
}
