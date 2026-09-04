// The only module that talks to the native layer. Every Tauri command and event is wrapped in
// a typed method so the rest of the app never sees a command name or an untyped payload.
//
// In a plain browser (`vp dev` without Tauri) the dev-only mock backend is loaded instead, which
// lets the interface be developed and screenshotted without Docker or a macOS guest.

import type * as T from '../types/backend';

export type Unlisten = () => void;

/** What a path field is asking the person to choose. */
export interface PathPickRequest {
    kind: 'file' | 'files' | 'directory';
    title: string;
    /** File-type filter, e.g. { name: 'Signing identity', extensions: ['p12', 'pfx'] }. */
    filter?: { name: string; extensions: string[] };
    /** Where to open the dialog, when a path is already chosen. */
    startFrom?: string | null;
}

export interface DragDropEvent {
    type: 'enter' | 'over' | 'drop' | 'leave';
    paths: string[];
}

export interface Backend {
    getRunnerStatus(): Promise<T.DesktopStatus>;
    pairRunner(input: T.PairInput): Promise<T.DesktopStatus>;
    unpairRunner(): Promise<void>;
    getRealtimeConfiguration(): Promise<T.RealtimeConfiguration>;
    authorizeRealtime(input: {
        socketId: string;
        channelName: string;
    }): Promise<T.RealtimeAuthorization>;
    heartbeatRunner(): Promise<T.HeartbeatSummary>;
    runOnce(): Promise<T.RunOnceResult>;

    listMachines(): Promise<T.MachineListView>;
    createMachine(profile: T.MacBuilderConfig): Promise<T.MachineListView>;
    deleteMachine(machineId: string): Promise<T.MachineListView>;
    discardMachineContainer(machineId: string): Promise<T.MacBuilderView>;
    /** Stops whatever is running on the machine; the operation returns as stopped. */
    cancelMachineOperation(machineId: string): Promise<void>;
    getMachine(machineId: string): Promise<T.MacBuilderView>;
    configureMachine(machineId: string, profile: T.MacBuilderConfig): Promise<T.MacBuilderView>;
    launchMachine(machineId: string): Promise<T.MacBuilderView>;
    stopMachine(machineId: string): Promise<T.MacBuilderView>;
    configureGuestAccess(machineId: string, username: string): Promise<T.MacBuilderView>;
    /**
     * Installs the access key over one password-authenticated SSH session to the pinned guest.
     * The password is used for that session only and is never stored.
     */
    authorizeGuestKey(
        machineId: string,
        username: string,
        password: string,
    ): Promise<T.MacBuilderView>;
    trustGuest(machineId: string, fingerprint: string): Promise<T.MacBuilderView>;
    forgetGuestTrust(machineId: string): Promise<T.MacBuilderView>;
    importXcode(machineId: string, path: string): Promise<T.ImportMacXcodeResult>;
    /**
     * With a password, activation runs over the bridge under sudo and the password is used for
     * that one session only. With null, the guest Terminal opens and the person types it there.
     */
    activateXcode(machineId: string, password: string | null): Promise<T.MacBuilderView>;
    provisionSigning(machineId: string): Promise<T.MacBuilderView>;
    clearGuestSigning(machineId: string): Promise<T.MacBuilderView>;
    approveWorkspace(machineId: string, path: string): Promise<T.MacBuilderView>;
    clearWorkspace(machineId: string): Promise<T.MacBuilderView>;
    syncWorkspace(machineId: string): Promise<T.SyncAppleWorkspaceResult>;
    runSmokeBuild(
        machineId: string,
        target: T.UnsignedBuildTarget,
    ): Promise<T.RunAppleSmokeBuildResult>;
    adoptGuestPodfileLock(machineId: string): Promise<T.AdoptPodfileLockResult>;
    /** The webview's inspector: console, network and elements of this desktop itself. */
    openDeveloperTools(): Promise<void>;
    /** `envSetId` null builds without an env set; the step defaults it to the attached one. */
    runSignedArchive(machineId: string, envSetId: string | null): Promise<T.RunAppleArchiveResult>;
    revealArchive(machineId: string): Promise<void>;
    clearArchive(machineId: string): Promise<T.MacBuilderView>;

    /** Installs the host udev rule that keeps usbmuxd off iPhones; one authorization prompt. */
    installUsbReleaseRule(): Promise<T.HostUsbStatus>;
    removeUsbReleaseRule(): Promise<T.HostUsbStatus>;
    /** Moves the container's disk to the host and recreates it with USB access. */
    migrateMachineForUsb(machineId: string): Promise<T.MacBuilderView>;
    attachUsbDevice(machineId: string, bus: number, port: string): Promise<T.MacBuilderView>;
    detachUsbDevice(machineId: string): Promise<T.MacBuilderView>;
    /** Recreates the container from its profile; macOS restarts once, the disk is kept. */
    rebuildMachineContainer(machineId: string): Promise<T.MacBuilderView>;
    /** Asks the guest which phones it sees; the returned view carries the fresh list. */
    listGuestDevices(machineId: string): Promise<T.MacBuilderView>;
    /** Pairs the guest with the phone; raises Trust on the phone when needed and waits for it. */
    pairGuestDevice(machineId: string, udid: string): Promise<T.MacBuilderView>;
    prepareAppleDeviceSigning(
        machineId: string,
        udid: string,
        deviceName: string,
    ): Promise<T.PrepareDeviceSigningResult>;
    /** Returns when the console session ends; a Stop while running is the normal end. */
    runAppleDeviceBuild(machineId: string, udid: string): Promise<T.RunAppleDeviceResult>;
    clearAppleDeviceRun(machineId: string): Promise<T.MacBuilderView>;

    listSigningKits(): Promise<T.SigningKitSummary[]>;
    saveSigningKit(input: T.SigningKitInput): Promise<T.SigningKitSummary[]>;
    deleteSigningKit(kitId: string): Promise<T.SigningKitSummary[]>;
    attachSigningKit(machineId: string, kitId: string | null): Promise<T.MacBuilderView>;
    listEnvSets(): Promise<T.EnvSetSummary[]>;
    saveEnvSet(input: T.EnvSetInput): Promise<T.EnvSetSummary[]>;
    deleteEnvSet(setId: string): Promise<T.EnvSetSummary[]>;
    attachEnvSet(machineId: string, setId: string | null): Promise<T.MacBuilderView>;
    /** Every secret in one set with its value, for the editor. Plain values are in the summary. */
    revealEnvSecrets(setId: string): Promise<T.EnvVariableSummary[]>;
    verifyAppleTeam(machineId: string): Promise<T.AppleTeamVerification>;
    createAppleProfile(
        machineId: string,
        certificateId: string,
    ): Promise<T.CreateAppleProfileResult>;
    /** Creates an Apple Distribution identity for the kit with no Mac involved. */
    createAppleCertificate(kitId: string): Promise<T.CreateAppleCertificateResult>;
    /** Creates the Apple Development identity a Debug build on a registered phone signs with. */
    createAppleDevelopmentCertificate(kitId: string): Promise<T.CreateAppleCertificateResult>;
    /** Provisioning profiles already downloaded to this host, newest first. */
    listManagedAppleProfiles(): Promise<T.ManagedAppleProfile[]>;
    /** The optimizer catalogue with each item's state on this machine's guest. */
    listGuestOptimizations(machineId: string): Promise<T.GuestOptimizationsView>;
    /** Applies one catalogue item; admin items open the guest Terminal for the password. */
    applyGuestOptimization(
        machineId: string,
        optimizationId: string,
    ): Promise<T.GuestOptimizationsView>;
    /** Takes an existing Apple profile back into the attached kit. */
    downloadAppleProfile(
        machineId: string,
        profileId: string,
    ): Promise<T.DownloadAppleProfileResult>;

    onMachineChanged(handler: (event: T.MachineChangedEvent) => void): Promise<Unlisten>;
    onLaunchProgress(handler: (event: T.MachineEvent<T.LaunchProgress>) => void): Promise<Unlisten>;
    onXcodeProgress(
        handler: (event: T.MachineEvent<T.XcodeImportProgress>) => void,
    ): Promise<Unlisten>;
    onSigningProgress(
        handler: (event: T.MachineEvent<T.SigningProvisioningProgress>) => void,
    ): Promise<Unlisten>;
    onProjectProgress(
        handler: (event: T.MachineEvent<T.AppleProjectProgress>) => void,
    ): Promise<Unlisten>;
    onArchiveProgress(
        handler: (event: T.MachineEvent<T.AppleArchiveProgress>) => void,
    ): Promise<Unlisten>;
    onUsbMigrationProgress(
        handler: (event: T.MachineEvent<T.DiskMigrationProgress>) => void,
    ): Promise<Unlisten>;
    onContainerRebuildProgress(
        handler: (event: T.MachineEvent<T.ContainerRebuildProgress>) => void,
    ): Promise<Unlisten>;
    onUsbAttachProgress(
        handler: (event: T.MachineEvent<T.UsbAttachProgress>) => void,
    ): Promise<Unlisten>;
    onDeviceSigningProgress(
        handler: (event: T.MachineEvent<T.DeviceSigningProgress>) => void,
    ): Promise<Unlisten>;
    onDeviceProgress(
        handler: (event: T.MachineEvent<T.AppleDeviceRunProgress>) => void,
    ): Promise<Unlisten>;
    onDragDrop(handler: (event: DragDropEvent) => void): Promise<Unlisten>;

    /** Opens the operating system's file picker. Resolves to [] when cancelled. */
    pickPaths(request: PathPickRequest): Promise<string[]>;
}

export function isTauriRuntime(): boolean {
    return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

async function createTauriBackend(): Promise<Backend> {
    const [{ invoke }, { listen }, { getCurrentWebview }, { open }] = await Promise.all([
        import('@tauri-apps/api/core'),
        import('@tauri-apps/api/event'),
        import('@tauri-apps/api/webview'),
        import('@tauri-apps/plugin-dialog'),
    ]);

    const subscribe =
        <P>(event: string) =>
        async (handler: (payload: P) => void): Promise<Unlisten> =>
            listen<P>(event, (message) => handler(message.payload));

    return {
        getRunnerStatus: () => invoke('get_runner_status'),
        pairRunner: (input) => invoke('pair_runner', { input }),
        unpairRunner: () => invoke('unpair_runner'),
        getRealtimeConfiguration: () => invoke('get_realtime_configuration'),
        authorizeRealtime: (input) => invoke('authorize_realtime', { input }),
        heartbeatRunner: () => invoke('heartbeat_runner'),
        runOnce: () => invoke('run_once'),

        listMachines: () => invoke('list_machines'),
        createMachine: (profile) => invoke('create_machine', { profile }),
        deleteMachine: (machineId) =>
            invoke('delete_machine', { machineId, input: { confirmed: true } }),
        discardMachineContainer: (machineId) =>
            invoke('discard_machine_container', { machineId, input: { confirmed: true } }),
        cancelMachineOperation: (machineId) => invoke('cancel_machine_operation', { machineId }),
        getMachine: (machineId) => invoke('get_mac_builder_status', { machineId }),
        configureMachine: (machineId, profile) =>
            invoke('configure_mac_builder', { machineId, profile }),
        launchMachine: (machineId) => invoke('launch_mac_builder', { machineId }),
        stopMachine: (machineId) => invoke('stop_mac_builder', { machineId }),
        configureGuestAccess: (machineId, username) =>
            invoke('configure_mac_guest_access', { machineId, input: { username } }),
        authorizeGuestKey: (machineId, username, password) =>
            invoke('authorize_mac_guest_key', { machineId, input: { username, password } }),
        trustGuest: (machineId, fingerprint) =>
            invoke('trust_mac_builder_guest', { machineId, input: { fingerprint } }),
        forgetGuestTrust: (machineId) => invoke('forget_mac_builder_guest_trust', { machineId }),
        importXcode: (machineId, path) =>
            invoke('import_mac_xcode_package', { machineId, input: { path } }),
        activateXcode: (machineId, password) =>
            invoke('activate_mac_xcode', { machineId, input: { password } }),
        provisionSigning: (machineId) => invoke('provision_mac_signing', { machineId }),
        clearGuestSigning: (machineId) => invoke('clear_mac_guest_signing', { machineId }),
        approveWorkspace: (machineId, path) =>
            invoke('approve_apple_workspace', { machineId, input: { path } }),
        clearWorkspace: (machineId) => invoke('clear_apple_workspace', { machineId }),
        syncWorkspace: (machineId) => invoke('sync_apple_workspace', { machineId }),
        runSmokeBuild: (machineId, target) =>
            invoke('run_apple_smoke_build', { machineId, input: { target } }),
        adoptGuestPodfileLock: (machineId) => invoke('adopt_guest_podfile_lock', { machineId }),
        openDeveloperTools: () => invoke('open_developer_tools'),
        runSignedArchive: (machineId, envSetId) =>
            invoke('run_apple_signed_archive', { machineId, envSetId }),
        revealArchive: (machineId) => invoke('reveal_apple_archive', { machineId }),
        clearArchive: (machineId) => invoke('clear_apple_archive', { machineId }),

        installUsbReleaseRule: () => invoke('install_usb_release_rule'),
        removeUsbReleaseRule: () => invoke('remove_usb_release_rule'),
        migrateMachineForUsb: (machineId) =>
            invoke('migrate_machine_for_usb', { machineId, input: { confirmed: true } }),
        attachUsbDevice: (machineId, bus, port) =>
            invoke('attach_usb_device', { machineId, input: { bus, port } }),
        detachUsbDevice: (machineId) => invoke('detach_usb_device', { machineId }),
        rebuildMachineContainer: (machineId) =>
            invoke('rebuild_machine_container', { machineId, input: { confirmed: true } }),
        listGuestDevices: (machineId) => invoke('list_guest_devices', { machineId }),
        pairGuestDevice: (machineId, udid) =>
            invoke('pair_guest_device', { machineId, input: { udid } }),
        prepareAppleDeviceSigning: (machineId, udid, deviceName) =>
            invoke('prepare_apple_device_signing', {
                machineId,
                input: { udid, deviceName, confirmed: true },
            }),
        runAppleDeviceBuild: (machineId, udid) =>
            invoke('run_apple_device_build', { machineId, input: { udid } }),
        clearAppleDeviceRun: (machineId) => invoke('clear_apple_device_run', { machineId }),

        listSigningKits: () => invoke('list_signing_kits'),
        saveSigningKit: (input) => invoke('save_signing_kit', { input }),
        deleteSigningKit: (kitId) =>
            invoke('delete_signing_kit', { kitId, input: { confirmed: true } }),
        attachSigningKit: (machineId, kitId) =>
            invoke('attach_signing_kit', { machineId, input: { kitId } }),

        listEnvSets: () => invoke('list_env_sets'),
        saveEnvSet: (input) => invoke('save_env_set', { input }),
        deleteEnvSet: (setId) => invoke('delete_env_set', { setId, input: { confirmed: true } }),
        attachEnvSet: (machineId, setId) =>
            invoke('attach_env_set', { machineId, input: { setId } }),
        revealEnvSecrets: (setId) => invoke('reveal_env_secrets', { setId }),
        verifyAppleTeam: (machineId) => invoke('verify_apple_developer_team', { machineId }),
        createAppleProfile: (machineId, certificateId) =>
            invoke('create_apple_replacement_profile', {
                machineId,
                input: { certificateId, confirmed: true },
            }),

        createAppleCertificate: (kitId) =>
            invoke('create_apple_distribution_certificate', {
                kitId,
                input: { confirmed: true },
            }),
        createAppleDevelopmentCertificate: (kitId) =>
            invoke('create_apple_development_certificate', {
                kitId,
                input: { confirmed: true },
            }),
        listManagedAppleProfiles: () => invoke('list_managed_apple_profiles'),
        listGuestOptimizations: (machineId) => invoke('list_guest_optimizations', { machineId }),
        applyGuestOptimization: (machineId, optimizationId) =>
            invoke('apply_guest_optimization', {
                machineId,
                input: { optimizationId, confirmed: true },
            }),

        downloadAppleProfile: (machineId, profileId) =>
            invoke('download_apple_profile', { machineId, profileId }),

        onMachineChanged: subscribe('machine-changed'),
        onLaunchProgress: subscribe('machine-launch-progress'),
        onXcodeProgress: subscribe('machine-xcode-progress'),
        onSigningProgress: subscribe('machine-signing-progress'),
        onProjectProgress: subscribe('machine-project-progress'),
        onArchiveProgress: subscribe('machine-archive-progress'),
        onUsbMigrationProgress: subscribe('machine-usb-migration-progress'),
        onContainerRebuildProgress: subscribe('machine-container-rebuild-progress'),
        onUsbAttachProgress: subscribe('machine-usb-attach-progress'),
        onDeviceSigningProgress: subscribe('machine-device-signing-progress'),
        onDeviceProgress: subscribe('machine-device-progress'),
        pickPaths: async (request) => {
            const selection = await open({
                title: request.title,
                directory: request.kind === 'directory',
                multiple: request.kind === 'files',
                defaultPath: request.startFrom ?? undefined,
                filters: request.filter ? [request.filter] : undefined,
            });
            if (selection === null) {
                return [];
            }

            return Array.isArray(selection) ? selection : [selection];
        },

        onDragDrop: (handler) =>
            getCurrentWebview().onDragDropEvent((event) => {
                const payload = event.payload;
                handler({
                    type: payload.type,
                    paths: 'paths' in payload ? payload.paths : [],
                });
            }),
    };
}

let backend: Backend | null = null;

export async function loadBackend(): Promise<Backend> {
    if (backend) {
        return backend;
    }
    if (isTauriRuntime()) {
        backend = await createTauriBackend();
    } else if (import.meta.env.DEV) {
        const { createMockBackend } = await import('./backend-mock');
        backend = createMockBackend();
    } else {
        throw new Error('BuildBridge must run inside the desktop application.');
    }

    return backend;
}

export function useBackend(): Backend {
    if (!backend) {
        throw new Error('The backend has not been loaded yet.');
    }

    return backend;
}
