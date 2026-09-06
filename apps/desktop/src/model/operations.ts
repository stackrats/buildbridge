// The vocabulary of what runs on a machine: this client's operation ids, the native busy keys
// the backend reports, and the one label and timeline step each of them maps to. Pure data, so
// the header, the sidebar, the drawer bar and the rows all say the same thing.

export type OperationId =
    | 'refresh'
    | 'launch'
    | 'stop'
    | 'configure'
    | 'guest-access'
    | 'guest-authorize'
    | 'trust'
    | 'forget-trust'
    | 'xcode-import'
    | 'xcode-activate'
    | 'approve'
    | 'clear-workspace'
    | 'sync'
    | 'test-build'
    | 'provision'
    | 'attach-kit'
    | 'attach-env'
    | 'clear-signing'
    | 'archive'
    | 'upload-archive'
    | 'upload-google-play'
    | 'android-run-device'
    | 'release'
    | 'reveal'
    | 'clear-archive'
    | 'clear-release'
    | 'discard'
    | 'delete'
    | 'optimize'
    | 'usb-rule'
    | 'usb-migrate'
    | 'usb-attach'
    | 'usb-detach'
    | 'usb-rebuild'
    | 'device-pair'
    | 'device-signing'
    | 'run-device'
    | 'clear-device-run'
    | 'adopt-lock'
    | 'save-template'
    | 'template-adopt';

/**
 * Maps the native busy key (see src-tauri/src/ops.rs) to the step it blocks. `optimizing` is
 * absent on purpose: guest optimizations sit outside the timeline.
 */
export const busyKeyStep: Record<string, string> = {
    starting: 'launch',
    stopping: 'launch',
    importing_xcode: 'xcode-import',
    activating_xcode: 'xcode-activate',
    provisioning_signing: 'provision',
    clearing_signing: 'provision',
    synchronizing: 'sync',
    test_building: 'test-build',
    adopting_lock: 'test-build',
    archiving: 'archive',
    uploading_archive: 'publish',
    uploading_google_play: 'publish',
    uploading_artifacts: 'publish',
    configuring_google_play: 'publish',
    approving_android_workspace: 'approve',
    clearing_android_workspace: 'approve',
    clearing_android_release: 'release',
    running_android_device: 'run-device',
    listing_android_devices: 'run-device',
    clearing_archive: 'archive',
    releasing: 'release',
    deleting: 'launch',
    discarding: 'launch',
    migrating_usb: 'run-device',
    rebuilding_container: 'run-device',
    settling_phone: 'run-device',
    attaching_usb: 'run-device',
    detaching_usb: 'run-device',
    listing_devices: 'run-device',
    pairing_device: 'run-device',
    preparing_device_signing: 'run-device',
    running_on_device: 'run-device',
    saving_template: 'launch',
    adopting_template: 'trust',
};

export const busyKeyLabel: Record<string, string> = {
    starting: 'Starting the machine',
    stopping: 'Stopping the machine',
    importing_xcode: 'Importing Xcode',
    activating_xcode: 'Activating Xcode',
    provisioning_signing: 'Provisioning signing',
    clearing_signing: 'Removing guest signing',
    synchronizing: 'Synchronizing source',
    test_building: 'Running the test build',
    adopting_lock: 'Adopting the guest Podfile.lock',
    archiving: 'Building the signed archive',
    uploading_archive: 'Uploading with Transporter',
    uploading_google_play: 'Uploading to Google Play',
    uploading_artifacts: 'Uploading artifacts to the dashboard',
    configuring_google_play: 'Saving Google Play credentials',
    approving_android_workspace: 'Approving the Android project',
    clearing_android_workspace: 'Removing the Android project approval',
    clearing_android_release: 'Clearing retained Android artifacts',
    running_android_device: 'Installing and opening on Android',
    listing_android_devices: 'Finding Android devices',
    clearing_archive: 'Clearing retained artifacts',
    releasing: 'Building the signed release',
    deleting: 'Deleting the machine',
    discarding: 'Discarding the container',
    optimizing: 'Applying an optimization',
    migrating_usb: 'Enabling USB on the machine',
    rebuilding_container: 'Rebuilding the container',
    settling_phone: 'Waiting for macOS to find the iPhone',
    attaching_usb: 'Attaching the iPhone',
    detaching_usb: 'Detaching the iPhone',
    listing_devices: 'Reading the phones the guest sees',
    pairing_device: 'Pairing with the phone',
    preparing_device_signing: 'Preparing device signing',
    running_on_device: 'Running on the device',
    saving_template: 'Saving the machine as a template',
    adopting_template: 'Adopting the template',
};

/** What each operation this client starts is doing, for the header, the drawer bar and rows. */
export const operationLabel: Record<OperationId, string> = {
    refresh: 'Refreshing the machine',
    launch: 'Starting the machine',
    stop: 'Stopping the machine',
    configure: 'Saving the machine profile',
    'guest-access': 'Generating the access key',
    'guest-authorize': 'Installing the access key',
    trust: 'Pinning the guest identity',
    'forget-trust': 'Forgetting the pinned identity',
    'xcode-import': 'Importing Xcode',
    'xcode-activate': 'Activating Xcode',
    approve: 'Approving the project',
    'clear-workspace': 'Removing the project approval',
    sync: 'Synchronizing source',
    'test-build': 'Running the test build',
    provision: 'Provisioning signing',
    'attach-kit': 'Changing the signing credentials',
    'attach-env': 'Changing the environment',
    'clear-signing': 'Removing guest signing',
    archive: 'Building the signed archive',
    'upload-archive': 'Uploading with Transporter',
    'upload-google-play': 'Uploading to Google Play',
    'android-run-device': 'Installing and opening on Android',
    release: 'Building the signed release',
    reveal: 'Revealing the artifacts',
    'clear-archive': 'Clearing retained artifacts',
    'clear-release': 'Clearing retained artifacts',
    discard: 'Discarding the container',
    delete: 'Deleting the machine',
    optimize: 'Applying an optimization',
    'usb-rule': 'Updating the host USB rule',
    'usb-migrate': 'Enabling USB on the machine',
    'usb-attach': 'Attaching the iPhone',
    'usb-detach': 'Detaching the iPhone',
    'usb-rebuild': 'Rebuilding the container',
    'device-pair': 'Pairing with the phone',
    'device-signing': 'Preparing device signing',
    'run-device': 'Running on the device',
    'clear-device-run': 'Clearing the last device run',
    'adopt-lock': 'Adopting the guest Podfile.lock',
    'save-template': 'Saving the machine as a template',
    'template-adopt': 'Adopting the template',
};

/** The label for whatever is running: a native busy key or this client's operation id. */
export function activityLabel(operation: string | null | undefined): string | null {
    if (!operation) {
        return null;
    }
    // Own keys only: a backend-supplied name like "constructor" must not find Object.prototype.
    if (Object.hasOwn(busyKeyLabel, operation)) {
        return busyKeyLabel[operation] ?? null;
    }
    if (Object.hasOwn(operationLabel, operation)) {
        return operationLabel[operation as OperationId] ?? null;
    }
    return null;
}

export const operationStep: Partial<Record<OperationId, string>> = {
    launch: 'launch',
    stop: 'launch',
    discard: 'launch',
    'xcode-import': 'xcode-import',
    'xcode-activate': 'xcode-activate',
    sync: 'sync',
    'test-build': 'test-build',
    'adopt-lock': 'test-build',
    provision: 'provision',
    'clear-signing': 'provision',
    archive: 'archive',
    'upload-archive': 'publish',
    'upload-google-play': 'publish',
    'android-run-device': 'run-device',
    release: 'release',
    'clear-release': 'release',
    'usb-rule': 'run-device',
    'usb-migrate': 'run-device',
    'usb-attach': 'run-device',
    'usb-detach': 'run-device',
    'usb-rebuild': 'run-device',
    'device-pair': 'run-device',
    'device-signing': 'run-device',
    'run-device': 'run-device',
    'save-template': 'launch',
    'template-adopt': 'trust',
};
