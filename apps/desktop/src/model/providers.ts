// What each provider is called, what it builds for, and the release it does best with. The
// platform is the person's choice in the form; the provider is chosen under it, and only the
// provider is stored, because it decides the platform.
import type { MacOsRelease, MachinePlatform, MachineProvider } from '../types/backend';

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
        detail: 'A persistent macOS guest under QEMU and KVM, with macOS and Xcode installed once.',
    },
    {
        value: 'android',
        label: 'Android',
        detail: 'A toolchain container with the SDK, Gradle and Node prepared in it; no virtual machine.',
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
