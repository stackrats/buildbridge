import { describe, expect, it } from 'vite-plus/test';

import type { HostPrerequisites } from '../types/backend';
import {
    availableSshPort,
    defaultMachineName,
    machinePorts,
    providerHostIssues,
    providerHostReady,
} from './providers';

const readyHost: HostPrerequisites = {
    supportedHost: true,
    dockerCli: true,
    dockerDaemon: true,
    dockerVersion: '28.3.2',
    kvmAccess: true,
    tunAccess: true,
    displayAccess: true,
    display: ':1',
    ready: true,
    issues: [],
};

describe('provider host requirements', () => {
    it('does not make a headless dockur machine depend on Docker-OSX display readiness', () => {
        const host = { ...readyHost, displayAccess: false, ready: false };
        expect(providerHostReady(host, 'dockur_macos')).toBe(true);
        expect(providerHostIssues(host, 'docker_osx')).toEqual([
            'Docker-OSX needs an X11 display for the macOS screen.',
        ]);
    });

    it('requires the tunnel for dockur without adding that requirement to Docker-OSX', () => {
        const host = { ...readyHost, tunAccess: false };
        expect(providerHostReady(host, 'docker_osx')).toBe(true);
        expect(providerHostReady(host, 'dockur_macos')).toBe(false);
    });

    it('allows Android with Docker on a Unix host even when macOS checks fail', () => {
        const host = {
            ...readyHost,
            supportedHost: false,
            kvmAccess: false,
            displayAccess: false,
            tunAccess: false,
            ready: false,
        };
        expect(providerHostReady(host, 'android_toolchain', 'macos')).toBe(true);
        expect(providerHostReady(host, 'android_toolchain', 'windows')).toBe(false);
        expect(providerHostReady(host, 'docker_osx', 'macos')).toBe(false);
    });

    it('requires Docker for every provider and does not mark an unchecked host ready', () => {
        const host = { ...readyHost, dockerDaemon: false };
        for (const provider of ['docker_osx', 'dockur_macos', 'android_toolchain'] as const) {
            expect(providerHostReady(host, provider)).toBe(false);
            expect(providerHostReady(null, provider)).toBe(false);
        }
    });
});

describe('machine defaults', () => {
    it('reserves both dockur ports and ignores Android placeholder ports', () => {
        expect(machinePorts({ provider: 'dockur_macos', sshPort: 50922 })).toEqual([50922, 50923]);
        expect(machinePorts({ provider: 'android_toolchain', sshPort: 50922 })).toEqual([]);
        expect(
            availableSshPort('docker_osx', [
                { provider: 'dockur_macos', sshPort: 50922 },
                { provider: 'android_toolchain', sshPort: 50924 },
            ]),
        ).toBe(50924);
    });

    it('chooses a dockur port whose following screen port is also free', () => {
        expect(availableSshPort('dockur_macos', [{ provider: 'docker_osx', sshPort: 50923 }])).toBe(
            50924,
        );
    });

    it('keeps the screen port within range and searches lower ports when necessary', () => {
        expect(availableSshPort('dockur_macos', [], 65535)).toBe(65534);
        expect(
            availableSshPort('dockur_macos', [{ provider: 'docker_osx', sshPort: 65535 }], 65535),
        ).toBe(1024);
    });

    it('suggests recognizable platform names without duplicating existing defaults', () => {
        expect(defaultMachineName('android_toolchain', [])).toBe('Android builder');
        expect(defaultMachineName('dockur_macos', ['iOS builder', 'iOS builder 2'])).toBe(
            'iOS builder 3',
        );
    });
});
