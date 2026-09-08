import { describe, expect, it } from 'vite-plus/test';

import type { MachineSummary } from '../types/backend';
import { filterMachines, machineMatches } from './machine-filter';

function machine(
    id: string,
    name: string,
    provider: MachineSummary['config']['provider'],
): MachineSummary {
    return {
        id,
        config: {
            name,
            macosRelease: 'tahoe',
            memoryGib: 8,
            cpuCores: 4,
            sshPort: 50922,
            provider,
        },
        platform: provider === 'android_toolchain' ? 'android' : 'ios',
        createdAtEpochSeconds: 1_788_400_000,
        state: 'running',
        containerId: null,
        busyOperation: null,
        guestConfigured: true,
        trustPinned: true,
        workspaceName: null,
        signingKitName: null,
        signingProvisioned: false,
        signingIdentity: null,
        archiveRetained: false,
        envSetName: null,
        usbReady: false,
        deviceRunRetained: false,
        templateName: null,
    };
}

const list = [
    machine('local-mac', 'Local macOS builder', 'docker_osx'),
    machine('team-mac', 'Team Mac', 'dockur_macos'),
    machine('pixel', 'Android builder', 'android_toolchain'),
];
const secondary = (target: MachineSummary) =>
    target.id === 'pixel' ? 'signed release retained' : 'next: start the machine';

describe('filtering the sidebar machines', () => {
    it('keeps every machine, in order, until something is typed', () => {
        for (const query of ['', '   ']) {
            expect(filterMachines(list, query, secondary).map((found) => found.id)).toEqual([
                'local-mac',
                'team-mac',
                'pixel',
            ]);
        }
    });

    it('finds a machine by its name, whatever the case, and anywhere in it', () => {
        expect(filterMachines(list, 'TEAM', secondary).map((found) => found.id)).toEqual([
            'team-mac',
        ]);
        expect(filterMachines(list, 'builder', secondary).map((found) => found.id)).toEqual([
            'local-mac',
            'pixel',
        ]);
        expect(filterMachines(list, '  mac  ', secondary).map((found) => found.id)).toEqual([
            'local-mac',
            'team-mac',
        ]);
    });

    it('finds a machine by the platform it builds for, and by its identifier', () => {
        expect(filterMachines(list, 'android', secondary).map((found) => found.id)).toEqual([
            'pixel',
        ]);
        expect(filterMachines(list, 'ios', secondary).map((found) => found.id)).toEqual([
            'local-mac',
            'team-mac',
        ]);
        expect(filterMachines(list, 'pixel', secondary).map((found) => found.id)).toEqual([
            'pixel',
        ]);
    });

    it('finds a machine by what its second line currently says', () => {
        expect(filterMachines(list, 'signed', secondary).map((found) => found.id)).toEqual([
            'pixel',
        ]);
        expect(filterMachines(list, 'next:', secondary).map((found) => found.id)).toEqual([
            'local-mac',
            'team-mac',
        ]);
    });

    it('leaves nothing when a query matches nothing', () => {
        expect(filterMachines(list, 'zzz', secondary)).toEqual([]);
        expect(machineMatches(list[0]!, 'zzz', 'idle')).toBe(false);
    });
});
