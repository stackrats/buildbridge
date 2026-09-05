// What each provider is called, and the release it does best with.
import type { MacOsRelease, MachineProvider } from '../types/backend';

export const providerLabel: Record<MachineProvider, string> = {
    docker_osx: 'Docker-OSX',
    dockur_macos: 'dockur/macos',
};

/**
 * Applied when a provider is chosen. dockur/macos's own authors do not recommend Tahoe on it
 * yet, and the first Tahoe install here hung in its second stage.
 */
export const recommendedRelease: Record<MachineProvider, MacOsRelease> = {
    docker_osx: 'tahoe',
    dockur_macos: 'sequoia',
};
