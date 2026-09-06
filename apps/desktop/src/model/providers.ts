// What each provider is called, what it builds for, and the release it does best with. The
// platform is the person's choice in the form; the provider is chosen under it, and only the
// provider is stored, because it decides the platform.
import type {
    HostPrerequisites,
    MachineConfig,
    MacOsRelease,
    MachinePlatform,
    MachineProvider,
} from '../types/backend';

export const providerLabel: Record<MachineProvider, string> = {
    docker_osx: 'Docker-OSX',
    dockur_macos: 'dockur/macos',
    android_toolchain: 'Android toolchain',
};

/**
 * The repository behind each provider's image, for reading before trusting it: the two macOS
 * images are projects of their own, and the toolchain starts from Adoptium's Temurin JDK image.
 */
export const providerSource: Record<MachineProvider, string> = {
    docker_osx: 'https://github.com/sickcodes/Docker-OSX',
    dockur_macos: 'https://github.com/dockur/macos',
    android_toolchain: 'https://github.com/adoptium/containers',
};

/** The provider decides the platform: a macOS machine builds iOS apps, the toolchain Android. */
export const providerPlatform: Record<MachineProvider, MachinePlatform> = {
    docker_osx: 'ios',
    dockur_macos: 'ios',
    android_toolchain: 'android',
};

export const platformLabel: Record<MachinePlatform, string> = {
    ios: 'iOS',
    android: 'Android',
};

/** Every platform the form offers, in order, with one sentence on what a machine for it is. */
export const platforms: { value: MachinePlatform; label: string; detail: string }[] = [
    {
        value: 'ios',
        label: 'iOS',
        detail: 'Set up macOS and Xcode once. Build and preview iOS apps.',
    },
    {
        value: 'android',
        label: 'Android',
        detail: 'Build APKs and app bundles. Android tools prepare themselves.',
    },
];

/** The providers that build for a platform, the recommended one first. */
export const providersFor: Record<MachinePlatform, MachineProvider[]> = {
    ios: ['docker_osx', 'dockur_macos'],
    android: ['android_toolchain'],
};

export function defaultProviderFor(platform: MachinePlatform): MachineProvider {
    return providersFor[platform][0]!;
}

export function isAndroid(provider: MachineProvider): boolean {
    return providerPlatform[provider] === 'android';
}

/**
 * Applied when a provider is chosen. dockur/macos's own authors do not recommend Tahoe on it
 * yet, and the first Tahoe install here hung in its second stage. The toolchain container has
 * no macOS in it; the value is carried but never read.
 */
export const recommendedRelease: Record<MachineProvider, MacOsRelease> = {
    docker_osx: 'tahoe',
    dockur_macos: 'sequoia',
    android_toolchain: 'sequoia',
};

/**
 * The overview's host probe describes Docker-OSX. Reuse its individual checks for the chosen
 * provider instead of treating a missing display or KVM as an Android failure. Its
 * supportedHost flag describes Linux x86_64, so only an explicit Windows platform rules
 * out Android here; a machine's own runtime prerequisites remain authoritative.
 */
export function providerHostIssues(
    host: HostPrerequisites | null,
    provider: MachineProvider,
    hostPlatform?: string,
): string[] {
    if (!host) {
        return [];
    }
    const issues: string[] = [];
    if (isAndroid(provider)) {
        if (hostPlatform === 'windows' || hostPlatform === 'win32') {
            issues.push('The Android toolchain currently needs a Unix host with Docker.');
        }
    } else if (!host.supportedHost) {
        issues.push(`${providerLabel[provider]} needs an x86_64 Linux host with KVM.`);
    }
    if (!host.dockerCli) {
        issues.push('Install Docker to start this machine.');
    } else if (!host.dockerDaemon) {
        issues.push('Start Docker and allow this user to access it.');
    }
    if (!isAndroid(provider) && !host.kvmAccess) {
        issues.push('Allow this user to read and write /dev/kvm.');
    }
    if (provider === 'docker_osx' && !host.displayAccess) {
        issues.push('Docker-OSX needs an X11 display for the macOS screen.');
    }
    if (provider === 'dockur_macos' && !host.tunAccess) {
        issues.push('dockur/macos needs /dev/net/tun; load the tun module on this host.');
    }
    return issues;
}

export function providerHostReady(
    host: HostPrerequisites | null,
    provider: MachineProvider,
    hostPlatform?: string,
): boolean {
    return host !== null && providerHostIssues(host, provider, hostPlatform).length === 0;
}

/** The ports the engine reserves: Android publishes none; dockur also reserves its screen. */
export function machinePorts(config: Pick<MachineConfig, 'provider' | 'sshPort'>): number[] {
    if (isAndroid(config.provider)) {
        return [];
    }
    return config.provider === 'dockur_macos'
        ? [config.sshPort, config.sshPort + 1]
        : [config.sshPort];
}

export function availableSshPort(
    provider: MachineProvider,
    machines: Pick<MachineConfig, 'provider' | 'sshPort'>[],
    preferred = 50922,
): number | null {
    const used = new Set(machines.flatMap(machinePorts));
    const maximum = provider === 'dockur_macos' ? 65534 : 65535;
    const start = Math.min(maximum, Math.max(1024, preferred));
    for (let offset = 0; offset <= maximum - 1024; offset += 1) {
        const port = 1024 + ((start - 1024 + offset) % (maximum - 1024 + 1));
        if (machinePorts({ provider, sshPort: port }).every((value) => !used.has(value))) {
            return port;
        }
    }
    return null;
}

export function defaultMachineName(provider: MachineProvider, names: string[]): string {
    const base = `${platformLabel[providerPlatform[provider]]} builder`;
    if (!names.includes(base)) {
        return base;
    }
    let suffix = 2;
    while (names.includes(`${base} ${suffix}`)) {
        suffix += 1;
    }
    return `${base} ${suffix}`;
}
