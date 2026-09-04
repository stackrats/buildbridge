// The readiness ladder for running a build on a phone. Each rung is a fact the view already
// carries, so the step's row summary and the panel's primary action both read this and never
// disagree about what comes next.

import type {
    GuestDevice,
    HostUsbDevice,
    MacBuilderView,
    SigningProvisioningResult,
} from '../types/backend';

export type DeviceSubstate =
    | 'host-rule'
    | 'container'
    | 'plug-in'
    | 'attach'
    | 'unplugged'
    | 'replug'
    | 'trust'
    | 'signing'
    | 'developer-mode'
    | 'ready';

export interface DeviceReadiness {
    substate: DeviceSubstate;
    /** The phone as the guest reports it, once attached and listed. */
    device: GuestDevice | null;
    /** The phone as the host reports it, while it is plugged in. */
    hostDevice: HostUsbDevice | null;
    signingReady: boolean;
    /** The attached kit holds a Team key, so BuildBridge can register the phone at Apple. */
    canPrepareSigning: boolean;
    /** What to call the phone in copy, before and after the guest names it. */
    name: string;
}

/** A development identity is provisioned and one of its profiles lists this phone. */
export function deviceSigningReady(
    signing: SigningProvisioningResult | null,
    udid: string,
): boolean {
    if (!signing?.developmentIdentity) {
        return false;
    }
    const wanted = udid.toUpperCase();
    return signing.profiles.some(
        (profile) =>
            profile.kind === 'development' &&
            (profile.provisionedDeviceUdids ?? []).some(
                (listed) => listed.toUpperCase() === wanted,
            ),
    );
}

export function deviceReadiness(view: MacBuilderView): DeviceReadiness {
    const { usb, guest, signing, signingKit } = view;
    const attached = usb.attached;
    const hostDevice = attached
        ? (usb.host.devices.find(
              (device) => device.bus === attached.bus && device.port === attached.port,
          ) ?? null)
        : null;
    const device = attached
        ? (guest.devices.find((candidate) => candidate.transportType === 'wired') ??
          guest.devices[0] ??
          null)
        : null;
    const signingReady =
        device !== null && device.udid !== null && deviceSigningReady(signing, device.udid);
    const canPrepareSigning = signingKit?.appStoreConnectConfigured ?? false;
    const name = device?.name ?? hostDevice?.product ?? 'the iPhone';
    const substate: DeviceSubstate =
        !usb.host.supported || usb.host.rule !== 'installed'
            ? 'host-rule'
            : !usb.containerReady
              ? 'container'
              : attached === null
                ? usb.host.devices.length
                    ? 'attach'
                    : 'plug-in'
                : hostDevice === null
                  ? 'unplugged'
                  : !attached.enumerated
                    ? 'replug'
                    : device === null || device.pairingState !== 'paired'
                      ? 'trust'
                      : !signingReady
                        ? 'signing'
                        : device.developerMode !== 'enabled'
                          ? 'developer-mode'
                          : 'ready';

    return { substate, device, hostDevice, signingReady, canPrepareSigning, name };
}

/** The row's fact line while the step is next: what the facts say to do. */
export function deviceNextSummary(readiness: DeviceReadiness, view: MacBuilderView): string {
    switch (readiness.substate) {
        case 'host-rule':
            return 'Prepare the host: a udev rule lets usbmuxd release the iPhone to the guest';
        case 'container':
            return view.runtime.state === 'missing'
                ? 'Start the machine; a new container is created with USB access'
                : view.usb.diskOnHost && !view.usb.phoneController
                  ? 'Rebuild the container once to add the phone controller; the disk is kept'
                  : 'Recreate the container with USB access; the disk and identity are kept';
        case 'plug-in':
            return 'Plug an iPhone into this host by cable, then attach it';
        case 'attach': {
            const count = view.usb.host.devices.length;
            return `${count} Apple device${count === 1 ? '' : 's'} on this host; attach one to the guest`;
        }
        case 'unplugged':
            return `${readiness.name} is no longer plugged into this host; reconnect it or detach`;
        case 'replug':
            return `QEMU holds ${readiness.name} but could not read it; detach, unplug it, plug it in again, and attach once`;
        case 'trust':
            return `Unlock ${readiness.name} and tap Trust when it asks about this computer`;
        case 'signing':
            return readiness.canPrepareSigning
                ? `Register ${readiness.name} at Apple and provision a development identity`
                : 'The attached kit has no Team key to register the iPhone; add one to the kit';
        case 'developer-mode':
            return `Turn on Developer Mode on ${readiness.name}, then refresh`;
        case 'ready':
            return `Debug build · install and launch on ${readiness.name} · console streamed`;
    }
}

/** The row's fact line while an operation of this step runs; the facts say which one. */
export function deviceWorkingSummary(readiness: DeviceReadiness): string {
    switch (readiness.substate) {
        case 'host-rule':
            return 'Installing the udev rule; a system prompt asks for authorization';
        case 'container':
            return 'Copying the disk to this host and recreating the container with USB access';
        case 'plug-in':
        case 'attach':
        case 'unplugged':
        case 'replug':
            return 'Passing the iPhone into the guest';
        case 'trust':
        case 'developer-mode':
            return 'Reading the iPhone’s state from the guest';
        case 'signing':
            return 'Registering the iPhone at Apple and provisioning the development identity';
        case 'ready':
            return 'Building, installing, and launching on the iPhone; console streaming';
    }
}
