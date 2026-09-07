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

export interface SavePathRequest {
    title: string;
    defaultPath: string;
    filter?: { name: string; extensions: string[] };
}

export interface Backend {
    /** This host's preferences, shared with the command line. */
    getHostSettings(): Promise<T.HostSettings>;
    saveHostSettings(input: T.HostSettings): Promise<T.HostSettings>;
    getStorageLocations(): Promise<T.StorageLocations>;
    revealStorageDirectory(directory: T.StorageDirectory): Promise<void>;
    /** Whether the engine keeps measuring what the machines cost; on while the window shows. */
    setUsageSampling(enabled: boolean): Promise<void>;
    onHostUsage(handler: (sample: T.UsageSample) => void): Promise<Unlisten>;
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
    getSharing(): Promise<T.SharingOverview>;
    createSharingInvitation(input: T.ShareMachineInput): Promise<T.SharingInvitation>;
    approveSharingGrant(grantId: string): Promise<void>;
    revokeSharingGrant(grantId: string): Promise<void>;
    setSharingPaused(paused: boolean): Promise<void>;
    /** Explicit Recheck bypasses the short toolchain cache used by background status. */
    nativeMacStatus(forceRefresh?: boolean): Promise<T.NativeMacStatus>;
    approveNativeMacProject(input: T.NativeMacProjectInput): Promise<T.NativeMacConfig>;
    configureNativeMacSigning(input: T.NativeMacSigningInput): Promise<T.NativeMacConfig>;
    runNativeMacBuild(input: T.NativeMacBuildInput): Promise<T.NativeMacBuildResult>;
    cancelNativeMacBuild(): Promise<void>;
    setNativeMacLogin(enabled: boolean): Promise<void>;
    revealNativeMacArtifacts(path: string): Promise<void>;
    onNativeMacProgress(handler: (progress: T.NativeMacProgress) => void): Promise<Unlisten>;
    onRunnerActivity(handler: (result: T.RunOnceResult) => void): Promise<Unlisten>;

    listMachines(): Promise<T.MachineListView>;
    /** `templateId` clones the machine's disk from a saved template instead of a fresh install. */
    createMachine(profile: T.MachineConfig, templateId: string | null): Promise<T.MachineListView>;
    listMachineTemplates(): Promise<T.MachineTemplateSummary[]>;
    /** Shuts macOS down and saves the machine's disk as a template; the machine stays stopped. */
    saveMachineTemplate(machineId: string, name: string): Promise<T.MachineTemplateSummary>;
    deleteMachineTemplate(templateId: string): Promise<T.MachineTemplateSummary[]>;
    /** Pins a clone's identity and installs its key through the template it came from. */
    adoptTemplateGuest(machineId: string): Promise<T.MachineView>;
    deleteMachine(machineId: string): Promise<T.MachineListView>;
    discardMachineContainer(machineId: string): Promise<T.MachineView>;
    /** Stops whatever is running on the machine; the operation returns as stopped. */
    cancelMachineOperation(machineId: string): Promise<void>;
    getMachine(machineId: string): Promise<T.MachineView>;
    configureMachine(machineId: string, profile: T.MachineConfig): Promise<T.MachineView>;
    launchMachine(machineId: string): Promise<T.MachineView>;
    stopMachine(machineId: string): Promise<T.MachineView>;
    configureGuestAccess(machineId: string, username: string): Promise<T.MachineView>;
    /**
     * Installs the access key over one password-authenticated SSH session to the pinned guest.
     * The password is used for that session only and is never stored.
     */
    authorizeGuestKey(
        machineId: string,
        username: string,
        password: string,
    ): Promise<T.MachineView>;
    trustGuest(machineId: string, fingerprint: string): Promise<T.MachineView>;
    forgetGuestTrust(machineId: string): Promise<T.MachineView>;
    importXcode(machineId: string, path: string): Promise<T.ImportMacXcodeResult>;
    /**
     * With a password, activation runs over the bridge under sudo and the password is used for
     * that one session only. With null, the guest Terminal opens and the person types it there.
     */
    activateXcode(machineId: string, password: string | null): Promise<T.MachineView>;
    provisionSigning(machineId: string): Promise<T.MachineView>;
    clearGuestSigning(machineId: string): Promise<T.MachineView>;
    approveWorkspace(machineId: string, path: string): Promise<T.MachineView>;
    clearWorkspace(machineId: string): Promise<T.MachineView>;
    syncWorkspace(machineId: string): Promise<T.SyncAppleWorkspaceResult>;
    runSmokeBuild(
        machineId: string,
        target: T.UnsignedBuildTarget,
        version?: T.ProjectVersionInput | null,
    ): Promise<T.RunAppleSmokeBuildResult>;
    adoptGuestPodfileLock(machineId: string): Promise<T.AdoptPodfileLockResult>;
    /** The webview's inspector: console, network and elements of this desktop itself. */
    openDeveloperTools(): Promise<void>;
    /** Opens a machine's screen, which its provider serves as a web page, in its own window. */
    openMachineScreen(machineId: string, url: string, title: string): Promise<void>;
    /** Opens an https address in this host's browser, outside the app. */
    openUrl(url: string): Promise<void>;
    /** Opens the host browser's Android device inspector and returns its internal URL. */
    openAndroidInspector(): Promise<string>;
    /**
     * Opens Apple's downloads page, searched for `query`, in a window of this app. A `.xip`
     * downloaded there lands in buildbridge's folder and is reported as it grows; the desktop
     * only (the browser preview simulates it).
     */
    downloadXcode(machineId: string, query: string): Promise<void>;
    /**
     * `envSetId` null builds without an env set; the step defaults it to the attached one.
     * `version` sets the marketing version and build number in the project and in this
     * archive; null archives the project as synced.
     */
    runSignedArchive(
        machineId: string,
        envSetId: string | null,
        version?: T.ProjectVersionInput | null,
    ): Promise<T.RunAppleArchiveResult>;
    /** Delivers exactly the retained IPA the person reviewed to App Store Connect. */
    uploadAppleArchive(machineId: string, expectedSha256: string): Promise<void>;
    revealArchive(machineId: string): Promise<void>;
    clearArchive(machineId: string): Promise<T.MachineView>;

    /** The Android machine's project: the same verbs, into the toolchain container. */
    approveAndroidWorkspace(machineId: string, path: string): Promise<T.MachineView>;
    clearAndroidWorkspace(machineId: string): Promise<T.MachineView>;
    syncAndroidWorkspace(machineId: string): Promise<T.SyncAndroidWorkspaceResult>;
    /** The debug APK; the first run also prepares the toolchain inside the container. */
    runAndroidDebugBuild(
        machineId: string,
        allowHttp?: boolean,
        version?: T.ProjectVersionInput | null,
    ): Promise<T.RunAndroidBuildResult>;
    /**
     * Selected signed release files, both by default, with the attached kit's key. `version`
     * sets the version name and code in the project and in this release.
     */
    runAndroidRelease(
        machineId: string,
        envSetId: string | null,
        outputs?: T.AndroidReleaseOutputs,
        version?: T.ProjectVersionInput | null,
    ): Promise<T.RunAndroidReleaseResult>;
    revealAndroidRelease(machineId: string): Promise<void>;
    clearAndroidRelease(machineId: string): Promise<T.MachineView>;
    /** Opens the folder holding the last debug APK, the one a phone takes over adb. */
    revealAndroidDebugApk(machineId: string): Promise<void>;
    /** Host ADB devices; the Android build container can be stopped. */
    listAndroidDevices(machineId: string): Promise<T.AndroidDevices>;
    /**
     * Installs and launches the retained APK, then streams the app's log until the session
     * ends; a stop, the app exiting, and the device going away are all ends, not failures.
     */
    runAndroidDevice(
        machineId: string,
        input: T.AndroidDeviceRunInput,
    ): Promise<T.RunAndroidDeviceResult>;
    clearAndroidDeviceRun(machineId: string): Promise<T.MachineView>;
    googlePlayConnection(machineId: string): Promise<T.GooglePlayConnection>;
    /** The native layer reads the JSON file and stores the credentials in the OS vault. */
    configureGooglePlay(machineId: string, path: string): Promise<T.GooglePlayConnection>;
    exportGooglePlayCredential(machineId: string, path: string): Promise<void>;
    disconnectGooglePlay(machineId: string): Promise<void>;
    uploadGooglePlay(machineId: string, expectedSha256: string): Promise<T.GooglePlayUploadResult>;
    /**
     * Asks the machine's store which builds it already holds for the app, for `version` or the
     * project's own: App Store Connect through the kit's Team key, Google Play through the
     * connected service account. A read; nothing is locked or written.
     */
    checkStoreBuilds(machineId: string, version: string | null): Promise<T.StoreBuildsCheck>;

    /** Installs the host udev rule that keeps usbmuxd off iPhones; one authorization prompt. */
    installUsbReleaseRule(): Promise<T.HostUsbStatus>;
    removeUsbReleaseRule(): Promise<T.HostUsbStatus>;
    /** Moves the container's disk to the host and recreates it with USB access. */
    migrateMachineForUsb(machineId: string): Promise<T.MachineView>;
    attachUsbDevice(machineId: string, bus: number, port: string): Promise<T.MachineView>;
    detachUsbDevice(machineId: string): Promise<T.MachineView>;
    /** Recreates the container from its profile; macOS restarts once, the disk is kept. */
    rebuildMachineContainer(machineId: string): Promise<T.MachineView>;
    /** Asks the guest which phones it sees; the returned view carries the fresh list. */
    listGuestDevices(machineId: string): Promise<T.MachineView>;
    /** Pairs the guest with the phone; raises Trust on the phone when needed and waits for it. */
    pairGuestDevice(machineId: string, udid: string): Promise<T.MachineView>;
    /** Opens Safari in the guest, Develop menu on, for Web Inspector on the app on the phone. */
    openSafariWebInspector(machineId: string): Promise<T.OpenSafariInspectorResult>;
    prepareAppleDeviceSigning(
        machineId: string,
        udid: string,
        deviceName: string,
    ): Promise<T.PrepareDeviceSigningResult>;
    /** Returns when the console session ends; a Stop while running is the normal end. */
    /**
     * `envSetId` rebuilds the web assets with that environment for this run alone; `version`
     * sets the version in the project and in this Debug build for the phone.
     */
    runAppleDeviceBuild(
        machineId: string,
        udid: string,
        envSetId?: string | null,
        version?: T.ProjectVersionInput | null,
    ): Promise<T.RunAppleDeviceResult>;
    clearAppleDeviceRun(machineId: string): Promise<T.MachineView>;

    listSigningKits(): Promise<T.SigningKitSummary[]>;
    listSigningCredentials(kitId: string): Promise<T.SigningCredential[]>;
    revealSigningCredential(kitId: string, credentialId: string): Promise<string>;
    exportSigningCredential(kitId: string, credentialId: string, path: string): Promise<void>;
    saveSigningKit(input: T.SigningKitInput): Promise<T.SigningKitSummary[]>;
    deleteSigningKit(kitId: string): Promise<T.SigningKitSummary[]>;
    attachSigningKit(machineId: string, kitId: string | null): Promise<T.MachineView>;
    listEnvSets(): Promise<T.EnvSetSummary[]>;
    saveEnvSet(input: T.EnvSetInput): Promise<T.EnvSetSummary[]>;
    deleteEnvSet(setId: string): Promise<T.EnvSetSummary[]>;
    attachEnvSet(machineId: string, setId: string | null): Promise<T.MachineView>;
    /** Every secret in one set with its value, for the editor. Plain values are in the summary. */
    revealEnvSecrets(setId: string): Promise<T.EnvVariableSummary[]>;
    verifyAppleTeam(machineId: string): Promise<T.AppleTeamVerification>;
    verifyAndroidSigningKit(kitId: string): Promise<T.AndroidSigningVerification>;
    createAppleProfile(
        machineId: string,
        certificateId: string,
    ): Promise<T.CreateAppleProfileResult>;
    /** Creates an Apple Distribution identity for the kit with no Mac involved. */
    createAppleCertificate(kitId: string): Promise<T.CreateAppleCertificateResult>;
    /** Creates the Apple Development identity a Debug build on a registered phone signs with. */
    createAppleDevelopmentCertificate(kitId: string): Promise<T.CreateAppleCertificateResult>;
    /** Creates an Android upload key for the kit; the password is the person's to keep. */
    createAndroidKeystore(
        kitId: string,
        input: { password: string; keyAlias: string; certificateName: string },
    ): Promise<T.CreateAndroidKeystoreResult>;
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
    onTemplateProgress(
        handler: (event: T.MachineEvent<T.TemplateSaveProgress>) => void,
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
    onAndroidBuildProgress(
        handler: (event: T.MachineEvent<T.AndroidBuildProgress>) => void,
    ): Promise<Unlisten>;
    onAndroidReleaseProgress(
        handler: (event: T.MachineEvent<T.AndroidReleaseProgress>) => void,
    ): Promise<Unlisten>;
    onAndroidDeviceProgress(
        handler: (event: T.MachineEvent<T.AndroidDeviceRunProgress>) => void,
    ): Promise<Unlisten>;
    onXcodeDownloadProgress(handler: (event: T.XcodeDownloadProgress) => void): Promise<Unlisten>;
    onDragDrop(handler: (event: DragDropEvent) => void): Promise<Unlisten>;

    /** Opens the operating system's file picker. Resolves to [] when cancelled. */
    pickPaths(request: PathPickRequest): Promise<string[]>;
    pickSavePath(request: SavePathRequest): Promise<string | null>;
}

export function isTauriRuntime(): boolean {
    return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

async function createTauriBackend(): Promise<Backend> {
    const [{ invoke }, { listen }, { getCurrentWebview }, { open, save }] = await Promise.all([
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
        getHostSettings: () => invoke('get_host_settings'),
        saveHostSettings: (input) => invoke('save_host_settings', { input }),
        getStorageLocations: () => invoke('get_storage_locations'),
        revealStorageDirectory: (directory) => invoke('reveal_storage_directory', { directory }),
        setUsageSampling: (enabled) => invoke('set_usage_sampling', { enabled }),
        onHostUsage: subscribe<T.UsageSample>('host-usage'),
        getRunnerStatus: () => invoke('get_runner_status'),
        pairRunner: (input) => invoke('pair_runner', { input }),
        unpairRunner: () => invoke('unpair_runner'),
        getRealtimeConfiguration: () => invoke('get_realtime_configuration'),
        authorizeRealtime: (input) => invoke('authorize_realtime', { input }),
        heartbeatRunner: () => invoke('heartbeat_runner'),
        runOnce: () => invoke('run_once'),
        getSharing: () => invoke('get_sharing'),
        createSharingInvitation: (input) => invoke('create_sharing_invitation', { input }),
        approveSharingGrant: (grantId) => invoke('approve_sharing_grant', { grantId }),
        revokeSharingGrant: (grantId) => invoke('revoke_sharing_grant', { grantId }),
        setSharingPaused: (paused) => invoke('set_sharing_paused', { paused }),
        nativeMacStatus: (forceRefresh = false) => invoke('native_mac_status', { forceRefresh }),
        approveNativeMacProject: (input) => invoke('approve_native_mac_project', { input }),
        configureNativeMacSigning: (input) => invoke('configure_native_mac_signing', { input }),
        runNativeMacBuild: (input) => invoke('run_native_mac_build', { input }),
        cancelNativeMacBuild: () => invoke('cancel_native_mac_build'),
        revealNativeMacArtifacts: (path) => invoke('reveal_native_mac_artifacts', { path }),
        setNativeMacLogin: (enabled) => invoke('set_native_mac_login', { enabled }),
        onNativeMacProgress: subscribe<T.NativeMacProgress>('native-mac-progress'),
        onRunnerActivity: subscribe<T.RunOnceResult>('runner-activity'),

        listMachines: () => invoke('list_machines'),
        createMachine: (profile, templateId) => invoke('create_machine', { profile, templateId }),
        listMachineTemplates: () => invoke('list_machine_templates'),
        saveMachineTemplate: (machineId, name) =>
            invoke('save_machine_template', { machineId, input: { name, confirmed: true } }),
        deleteMachineTemplate: (templateId) =>
            invoke('delete_machine_template', { templateId, input: { confirmed: true } }),
        adoptTemplateGuest: (machineId) => invoke('adopt_template_guest', { machineId }),
        deleteMachine: (machineId) =>
            invoke('delete_machine', { machineId, input: { confirmed: true } }),
        discardMachineContainer: (machineId) =>
            invoke('discard_machine_container', { machineId, input: { confirmed: true } }),
        cancelMachineOperation: (machineId) => invoke('cancel_machine_operation', { machineId }),
        getMachine: (machineId) => invoke('get_machine', { machineId }),
        configureMachine: (machineId, profile) =>
            invoke('configure_machine', { machineId, profile }),
        launchMachine: (machineId) => invoke('launch_machine', { machineId }),
        stopMachine: (machineId) => invoke('stop_machine', { machineId }),
        configureGuestAccess: (machineId, username) =>
            invoke('configure_mac_guest_access', { machineId, input: { username } }),
        authorizeGuestKey: (machineId, username, password) =>
            invoke('authorize_mac_guest_key', { machineId, input: { username, password } }),
        trustGuest: (machineId, fingerprint) =>
            invoke('trust_mac_guest', { machineId, input: { fingerprint } }),
        forgetGuestTrust: (machineId) => invoke('forget_mac_guest_trust', { machineId }),
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
        runSmokeBuild: (machineId, target, version = null) =>
            invoke('run_apple_smoke_build', { machineId, input: { target, version } }),
        adoptGuestPodfileLock: (machineId) => invoke('adopt_guest_podfile_lock', { machineId }),
        openDeveloperTools: () => invoke('open_developer_tools'),
        openMachineScreen: (machineId, url, title) =>
            invoke('open_machine_screen', { machineId, url, title }),
        openUrl: (url) => invoke('open_url', { url }),
        openAndroidInspector: () => invoke('open_android_web_inspector'),
        downloadXcode: (machineId, query) => invoke('download_xcode', { machineId, query }),
        runSignedArchive: (machineId, envSetId, version = null) =>
            invoke('run_apple_signed_archive', { machineId, envSetId, version }),
        uploadAppleArchive: (machineId, expectedSha256) =>
            invoke('upload_apple_archive', { machineId, expectedSha256 }),
        revealArchive: (machineId) => invoke('reveal_apple_archive', { machineId }),
        clearArchive: (machineId) => invoke('clear_apple_archive', { machineId }),
        approveAndroidWorkspace: (machineId, path) =>
            invoke('approve_android_workspace', { machineId, input: { path } }),
        clearAndroidWorkspace: (machineId) => invoke('clear_android_workspace', { machineId }),
        syncAndroidWorkspace: (machineId) => invoke('sync_android_workspace', { machineId }),
        runAndroidDebugBuild: (machineId, allowHttp = false, version = null) =>
            invoke('run_android_debug_build', { machineId, allowHttp, version }),
        runAndroidRelease: (machineId, envSetId, outputs = 'both', version = null) =>
            invoke('run_android_signed_release', { machineId, envSetId, outputs, version }),
        revealAndroidRelease: (machineId) => invoke('reveal_android_release', { machineId }),
        revealAndroidDebugApk: (machineId) => invoke('reveal_android_debug_apk', { machineId }),
        listAndroidDevices: (machineId) => invoke('list_android_devices', { machineId }),
        runAndroidDevice: (machineId, input) => invoke('run_android_device', { machineId, input }),
        clearAndroidDeviceRun: (machineId) => invoke('clear_android_device_run', { machineId }),
        googlePlayConnection: (machineId) => invoke('google_play_connection', { machineId }),
        exportGooglePlayCredential: (machineId, path) =>
            invoke('export_google_play_credential', { machineId, path }),
        configureGooglePlay: (machineId, path) =>
            invoke('configure_google_play', { machineId, path }),
        disconnectGooglePlay: (machineId) => invoke('disconnect_google_play', { machineId }),
        checkStoreBuilds: (machineId, version) =>
            invoke('check_store_builds', { machineId, input: { version } }),
        uploadGooglePlay: (machineId, expectedSha256) =>
            invoke('upload_google_play', { machineId, expectedSha256 }),
        clearAndroidRelease: (machineId) => invoke('clear_android_release', { machineId }),

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
        openSafariWebInspector: (machineId) => invoke('open_safari_web_inspector', { machineId }),
        pairGuestDevice: (machineId, udid) =>
            invoke('pair_guest_device', { machineId, input: { udid } }),
        prepareAppleDeviceSigning: (machineId, udid, deviceName) =>
            invoke('prepare_apple_device_signing', {
                machineId,
                input: { udid, deviceName, confirmed: true },
            }),
        runAppleDeviceBuild: (machineId, udid, envSetId = null, version = null) =>
            invoke('run_apple_device_build', { machineId, input: { udid, envSetId, version } }),
        clearAppleDeviceRun: (machineId) => invoke('clear_apple_device_run', { machineId }),

        listSigningKits: () => invoke('list_signing_kits'),
        listSigningCredentials: (kitId) => invoke('list_signing_credentials', { kitId }),
        revealSigningCredential: (kitId, credentialId) =>
            invoke('reveal_signing_credential', { kitId, credentialId }),
        exportSigningCredential: (kitId, credentialId, path) =>
            invoke('export_signing_credential', { kitId, credentialId, path }),
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
        verifyAndroidSigningKit: (kitId) => invoke('verify_android_signing_kit', { kitId }),
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
        createAndroidKeystore: (kitId, input) =>
            invoke('create_android_keystore', { kitId, input: { ...input, confirmed: true } }),
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
        onTemplateProgress: subscribe('machine-template-progress'),
        onUsbAttachProgress: subscribe('machine-usb-attach-progress'),
        onDeviceSigningProgress: subscribe('machine-device-signing-progress'),
        onDeviceProgress: subscribe('machine-device-progress'),
        onAndroidBuildProgress: subscribe('machine-android-build-progress'),
        onAndroidReleaseProgress: subscribe('machine-android-release-progress'),
        onAndroidDeviceProgress: subscribe('machine-android-device-progress'),
        onXcodeDownloadProgress: subscribe('xcode-download-progress'),
        pickSavePath: (request) =>
            save({
                title: request.title,
                defaultPath: request.defaultPath,
                filters: request.filter ? [request.filter] : undefined,
            }),
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
        throw new Error('buildbridge must run inside the desktop application.');
    }

    return backend;
}

export function useBackend(): Backend {
    if (!backend) {
        throw new Error('The backend has not been loaded yet.');
    }

    return backend;
}
