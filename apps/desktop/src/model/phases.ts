// Human labels for every native progress phase, so a log-free progress row still reads as a
// sentence. The backend also sends a `detail` sentence; these labels are the short form.

import type {
    AppleArchivePhase,
    AppleProjectPhase,
    LaunchPhase,
    SigningProvisioningPhase,
    XcodeImportPhase,
} from '../types/backend';

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
    preparing_tools: 'Preparing Node, pnpm, and CocoaPods',
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
