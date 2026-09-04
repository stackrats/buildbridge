// Human labels for every native progress phase, so a log-free progress row still reads as a
// sentence. The backend also sends a `detail` sentence; these labels are the short form.

import type {
    ContainerRebuildPhase,
    UsbAttachPhase,
    AppleArchivePhase,
    AppleDeviceRunPhase,
    AppleProjectPhase,
    DeviceSigningPhase,
    DiskMigrationPhase,
    LaunchPhase,
    SigningProvisioningPhase,
    XcodeImportPhase,
} from '../types/backend';

export const usbMigrationPhaseLabel: Record<DiskMigrationPhase, string> = {
    checking_space: 'Measuring the disk and the free space',
    stopping: 'Stopping the machine',
    copying_disk: 'Copying the macOS disk to this host',
    removing: 'Removing the old container',
    creating: 'Creating the container with USB access',
    starting: 'Starting',
    completed: 'USB enabled',
};

export const containerRebuildPhaseLabel: Record<ContainerRebuildPhase, string> = {
    shutting_down: 'Shutting macOS down',
    removing: 'Removing the container',
    creating: 'Creating the container',
    starting: 'Starting macOS',
    completed: 'Rebuilt',
};

export const usbAttachPhaseLabel: Record<UsbAttachPhase, string> = {
    waiting_for_macos: 'macOS is finding the iPhone',
    pairing: 'Pairing; tap Trust on the phone',
    completed: 'Attached',
};

export const deviceSigningPhaseLabel: Record<DeviceSigningPhase, string> = {
    checking_kit: 'Checking the signing kit',
    creating_certificate: 'Creating the development identity at Apple',
    registering_device: 'Registering the iPhone with the team',
    checking_profiles: 'Checking development profiles',
    creating_profile: 'Creating the development profile',
    downloading_profile: 'Downloading the profile',
    provisioning: 'Provisioning into the guest keychain',
    completed: 'Ready to sign for this iPhone',
};

export const devicePhaseLabel: Record<AppleDeviceRunPhase, string> = {
    preparing: 'Preparing the recipe',
    building_web_assets: 'Rebuilding web assets with the env set',
    resolving_target: 'Reading the Debug build settings',
    building: 'Compiling for the iPhone',
    verifying: 'Verifying the signature',
    installing: 'Installing on the iPhone',
    launching: 'Launching',
    running: 'Running; console streaming',
    completed: 'Stopped',
};

export const launchPhaseLabel: Record<LaunchPhase, string> = {
    preparing: 'Checking the host',
    pulling_image: 'Pulling the Docker-OSX image',
    generating_identity: 'Generating the machine identity',
    creating_container: 'Creating the container',
    starting: 'Starting',
    completed: 'Running',
};

export const xcodePhaseLabel: Record<XcodeImportPhase, string> = {
    preparing: 'Preparing',
    transferring: 'Transferring the archive',
    expanding: 'Expanding in the guest',
    awaiting_activation: 'Installed; activation needed',
    awaiting_authorization: 'Waiting for the macOS password',
    activating: 'Activating over the bridge',
};

export const signingPhaseLabel: Record<SigningProvisioningPhase, string> = {
    creating_certificate: 'Creating the distribution certificate at Apple',
    creating_profile: 'Finding or creating the App Store profile at Apple',
    preparing: 'Preparing',
    transferring: 'Transferring signing files',
    importing_certificate: 'Importing the certificate',
    inspecting_profiles: 'Inspecting profiles',
    installing_profiles: 'Installing profiles',
    verifying: 'Verifying with a code-sign probe',
    completed: 'Provisioned',
};

export const projectPhaseLabel: Record<AppleProjectPhase, string> = {
    snapshotting: 'Creating the snapshot',
    transferring: 'Transferring source',
    extracting: 'Preparing the guest workspace',
    preparing_tools: 'Preparing Node, pnpm, Ruby, and CocoaPods',
    preparing_platform: 'Installing the iOS Simulator platform',
    installing_dependencies: 'Installing locked dependencies',
    building_web_assets: 'Building web assets',
    syncing_ios: 'Synchronizing Capacitor iOS',
    resolving_pods: 'Resolving CocoaPods',
    building: 'Compiling for the Simulator',
    completed: 'Completed',
};

export const archivePhaseLabel: Record<AppleArchivePhase, string> = {
    preparing: 'Preparing the recipe',
    building_web_assets: 'Rebuilding web assets with the env set',
    archiving: 'Archiving',
    exporting: 'Exporting the IPA',
    verifying: 'Verifying the signature',
    packaging_archive: 'Packaging the archive',
    transferring: 'Transferring artifacts',
    completed: 'Completed',
};
