// Typed mirror of the Tauri command and event contract in src-tauri/src/lib.rs and the
// buildbridge-docker-osx crate. Field names are camelCase because every Rust DTO is
// serialized with `rename_all = "camelCase"`; enum values are snake_case.

export interface DesktopStatus {
    paired: boolean;
    /** A runner is configured on this host but its token is gone from the vault. */
    credentialsMissing: boolean;
    serverUrl: string | null;
    runnerId: string | null;
    runnerName: string | null;
    platform: string;
    architecture: string;
    version: string;
}

/** The part of a heartbeat reply the interface acts on. */
export interface HeartbeatSummary {
    /** Work waiting for this runner. Non-zero means a queue event was missed; claim now. */
    queuedBuilds: number;
}

export interface PairInput {
    serverUrl: string;
    code: string;
    runnerName: string;
}

export interface RealtimeConfiguration {
    key: string;
    host: string;
    port: number;
    scheme: 'http' | 'https';
    channel: string;
}

export interface RealtimeAuthorization {
    auth: string;
    channel_data?: string;
    shared_secret?: string;
}

export type RunState = 'idle' | 'completed' | 'failed' | 'busy';

export type OptimizationTier = 'recommended' | 'at_your_own_risk' | 'extremely_insecure';

/** One tweak from the osx-optimizer catalogue and whether this machine's guest has it. */
export interface GuestOptimization {
    id: string;
    title: string;
    summary: string;
    tier: OptimizationTier;
    /** The caveat, in the source's words where it gives one. */
    warning: string | null;
    /** Runs in the guest's own Terminal, where sudo reads the password from its TTY. */
    needsAdmin: boolean;
    /** Null when the guest cannot be asked right now, or the check could not tell. */
    applied: boolean | null;
}

export interface GuestOptimizationsView {
    available: boolean;
    reason: string | null;
    items: GuestOptimization[];
}

export interface RunOnceResult {
    state: RunState;
    buildId: string | null;
    message: string;
}

export type MacOsRelease = 'tahoe' | 'sequoia' | 'sonoma' | 'ventura';

export interface MacBuilderConfig {
    name: string;
    macosRelease: MacOsRelease;
    memoryGib: number;
    cpuCores: number;
    sshPort: number;
}

export interface HostPrerequisites {
    supportedHost: boolean;
    dockerCli: boolean;
    dockerDaemon: boolean;
    dockerVersion: string | null;
    kvmAccess: boolean;
    displayAccess: boolean;
    display: string | null;
    ready: boolean;
    issues: string[];
}

export type ContainerState =
    | 'missing'
    | 'created'
    | 'running'
    | 'paused'
    | 'restarting'
    | 'exited'
    | 'dead'
    | 'unavailable'
    | 'unknown';

export interface RuntimeStatus {
    prerequisites: HostPrerequisites;
    state: ContainerState;
    containerId: string | null;
    startedAt: string | null;
}

export type GuestTrustState = 'unavailable' | 'untrusted' | 'trusted' | 'mismatch';

export interface GuestSshStatus {
    portOpen: boolean;
    reachable: boolean;
    trust: GuestTrustState;
    fingerprint: string | null;
    pinnedFingerprint: string | null;
    issue: string | null;
}

export interface GuestDiagnostics {
    authenticated: boolean;
    macosVersion: string | null;
    xcodeVersion: string | null;
    xcodePath: string | null;
    xcodeSelected: boolean;
    issue: string | null;
}

export interface MacGuestAccessView {
    username: string | null;
    publicKey: string | null;
    ssh: GuestSshStatus;
    diagnostics: GuestDiagnostics;
    /** The phones the guest saw at its last listing; empty until one is requested. */
    devices: GuestDevice[];
}

export interface HostUsbDevice {
    bus: number;
    /** The sysfs port path, which is also what QEMU is handed. */
    port: string;
    vendorId: string;
    productId: string;
    product: string | null;
    serial: string | null;
    manufacturer: string | null;
    deviceNode: string;
    /** This user can open the node, which is what the container's QEMU needs. */
    nodeReady: boolean;
    heldBy: 'usbmuxd' | 'this_machine' | 'other' | null;
}

export type UdevRuleState = 'missing' | 'installed' | 'modified';

export interface HostUsbStatus {
    supported: boolean;
    rule: UdevRuleState;
    rulePath: string;
    usbmuxdActive: boolean;
    plugdevGid: number | null;
    devices: HostUsbDevice[];
    issues: string[];
}

export interface AttachedUsbDevice {
    bus: number;
    port: string;
    /** The guest has enumerated the phone; until then QEMU holds only the port. */
    enumerated: boolean;
    issue: string | null;
}

/** USB passthrough for one machine: the host's phones and rule, the container, the attachment. */
export interface MachineUsbStatus {
    host: HostUsbStatus;
    diskOnHost: boolean;
    containerReady: boolean;
    containerIssue: string | null;
    qmpReachable: boolean;
    attached: AttachedUsbDevice | null;
}

export type DeveloperModeState = 'enabled' | 'disabled' | 'unknown';
export type DevicePairingState = 'paired' | 'unpaired' | 'unknown';
export type DeviceTunnelState = 'connected' | 'disconnected' | 'unavailable' | 'unknown';
export type DeviceTransportType = 'wired' | 'local_network' | 'unknown';

/** One row of `xcrun devicectl list devices` as the guest reports it. */
export interface GuestDevice {
    identifier: string;
    udid: string | null;
    name: string;
    osVersion: string | null;
    model: string | null;
    developerMode: DeveloperModeState;
    pairingState: DevicePairingState;
    tunnelState: DeviceTunnelState;
    transportType: DeviceTransportType;
    ready: boolean;
    issue: string | null;
}

/** What is safe to show about one stored signing kit: names and counts, never a secret. */
export interface SigningKitSummary {
    id: string;
    name: string;
    appStoreConnectConfigured: boolean;
    appStoreConnectKeyId: string | null;
    signingCertificateConfigured: boolean;
    signingCertificateName: string | null;
    signingCertificatePasswordStored: boolean;
    provisioningProfileNames: string[];
    guestKeychainConfigured: boolean;
    createdAtEpochSeconds: number;
    /** Display names of the machines attached to this kit. */
    attachedMachines: string[];
    /** The optional development identity, for Debug builds on registered phones. */
    developmentCertificateConfigured: boolean;
    developmentCertificateName: string | null;
    developmentCertificatePasswordStored: boolean;
}

/**
 * What a machine can do about signing.
 *
 * `kit_missing` is the state an operating-system keyring reset leaves behind: the guest still
 * holds a provisioned keychain, but the material that created it is gone.
 */
export type SigningHealth =
    | 'unconfigured'
    | 'ready'
    | 'incomplete'
    | 'kit_missing'
    | 'vault_unavailable';

export interface SigningKitInput {
    /** Null creates a kit; an id updates that kit in place. */
    kitId: string | null;
    name: string;
    appStoreConnectKeyId: string;
    appStoreConnectIssuerId: string;
    appStoreConnectPrivateKeyPath: string;
    signingCertificatePath: string;
    signingCertificatePassword: string;
    provisioningProfilePaths: string[];
    guestKeychainPassword: string;
}

/** One variable as the interface shows it, value included. */
export interface EnvVariableSummary {
    key: string;
    value: string;
}

/** An env set as the interface may show it: plain variables with their values, secrets by key. */
export interface EnvSetSummary {
    id: string;
    name: string;
    variables: EnvVariableSummary[];
    /** Their values are left out; the backend hands them to the editor on request. */
    secretKeys: string[];
    createdAtEpochSeconds: number;
    attachedMachines: string[];
}

export interface EnvVariableInput {
    key: string;
    /** Null keeps the value already stored under this key. */
    value: string | null;
    /** Masked in the interface and left out of every summary once stored. */
    secret: boolean;
}

export interface EnvSetInput {
    /** Null creates a set; an id updates that set in place. */
    setId: string | null;
    name: string;
    /** The complete list: a stored key that is not listed is removed. */
    variables: EnvVariableInput[];
}

export interface StoredAppleWorkspace {
    localPath: string;
    name: string;
    iosWorkspace: string;
    scheme: string;
    developmentTeam: string | null;
    bundleIdentifier: string | null;
    lastSnapshotSha256: string | null;
    lastSyncFileCount: number | null;
    lastSyncBytes: number | null;
    lastBuildSucceeded: boolean;
    lastXcodeVersion: string | null;
    lastNativeLockUpdated: boolean;
    /** What the last snapshot came from: the approved folder, or a fetched revision of it. */
    lastSource: WorkspaceSource | null;
}

export interface WorkspaceSource {
    kind: 'folder' | 'git';
    gitRef: string | null;
    commit: string | null;
}

/** What a profile is for, read from its entitlements; null on records made before kinds existed. */
export type ProfileKind = 'app_store' | 'development' | 'ad_hoc' | 'enterprise';

export interface ProvisioningProfileSummary {
    uuid: string;
    teamIdentifier: string;
    applicationIdentifier: string;
    expiresAt: string;
    developerCertificateSha256: string[];
    kind: ProfileKind | null;
    provisionedDeviceUdids: string[];
    getTaskAllow: boolean;
}

export interface ProvisionedIdentity {
    identityName: string;
    identitySha1: string;
    certificateSha256: string;
    certificateExpiresAt: string;
}

export interface SigningProvisioningResult {
    keychainPath: string;
    identityName: string;
    identitySha1: string;
    certificateSha256: string;
    certificateExpiresAt: string;
    developmentTeam: string;
    bundleIdentifier: string;
    profiles: ProvisioningProfileSummary[];
    /** The development identity in the same keychain, when the kit holds one. */
    developmentIdentity: ProvisionedIdentity | null;
}

export interface AppleArchiveArtifact {
    path: string;
    bytes: number;
    sha256: string;
}

export interface AppleArchiveResult {
    scheme: string;
    configuration: string;
    exportMethod: string;
    bundleIdentifier: string;
    developmentTeam: string;
    marketingVersion: string;
    buildNumber: string;
    provisioningProfileUuid: string;
    ipa: AppleArchiveArtifact;
    archive: AppleArchiveArtifact;
    outputTail: string[];
}

export interface MacBuilderView {
    machineId: string;
    profile: MacBuilderConfig;
    /** Label of the native operation currently holding this machine, if any. */
    busyOperation: string | null;
    runtime: RuntimeStatus;
    /** The kit this machine will provision, resolved through its attachment. */
    signingKit: SigningKitSummary | null;
    /** The env set written into the guest workspace at sync, if one is attached. */
    envSet: EnvSetSummary | null;
    signingHealth: SigningHealth;
    /** Why the credential vault could not be read, when that is the problem. */
    vaultIssue: string | null;
    guest: MacGuestAccessView;
    appleWorkspace: StoredAppleWorkspace | null;
    signing: SigningProvisioningResult | null;
    archive: AppleArchiveResult | null;
    /** The env set the retained archive was built with, if any. */
    archiveEnvSet: string | null;
    archiveError: string | null;
    logs: string[];
    usb: MachineUsbStatus;
    /** The last run on a phone, kept until cleared. */
    deviceRun: AppleDeviceRunResult | null;
    /** The last failed device run, retained like `archiveError` until cleared. */
    deviceRunError: string | null;
}

export interface MachineSummary {
    id: string;
    config: MacBuilderConfig;
    createdAtEpochSeconds: number;
    state: ContainerState;
    containerId: string | null;
    busyOperation: string | null;
    guestConfigured: boolean;
    trustPinned: boolean;
    workspaceName: string | null;
    signingKitName: string | null;
    signingProvisioned: boolean;
    signingIdentity: string | null;
    archiveRetained: boolean;
    envSetName: string | null;
    /** The container keeps its disk on the host and can be handed USB devices. */
    usbReady: boolean;
    deviceRunRetained: boolean;
}

export interface MachineListView {
    host: HostPrerequisites;
    machines: MachineSummary[];
}

export type LaunchPhase =
    | 'preparing'
    | 'pulling_image'
    | 'generating_identity'
    | 'creating_container'
    | 'starting'
    | 'completed';

export interface LaunchProgress {
    phase: LaunchPhase;
    elapsedSeconds: number;
    detail: string;
}

export type XcodeImportPhase =
    | 'preparing'
    | 'transferring'
    | 'expanding'
    | 'awaiting_activation'
    | 'awaiting_authorization'
    | 'activating';

export interface XcodeImportProgress {
    phase: XcodeImportPhase;
    transferredBytes: number;
    totalBytes: number;
    elapsedSeconds: number;
    detail: string;
}

export type SigningProvisioningPhase =
    | 'preparing'
    | 'transferring'
    | 'importing_certificate'
    | 'inspecting_profiles'
    | 'installing_profiles'
    | 'verifying'
    | 'completed';

export interface SigningProvisioningProgress {
    phase: SigningProvisioningPhase;
    completedBytes: number;
    totalBytes: number;
    elapsedSeconds: number;
    detail: string;
}

export type AppleProjectPhase =
    | 'snapshotting'
    | 'transferring'
    | 'extracting'
    | 'preparing_tools'
    | 'preparing_platform'
    | 'installing_dependencies'
    | 'building_web_assets'
    | 'syncing_ios'
    | 'resolving_pods'
    | 'building'
    | 'completed';

export interface AppleProjectProgress {
    phase: AppleProjectPhase;
    completedBytes: number;
    totalBytes: number;
    elapsedSeconds: number;
    detail: string;
    logLine: string | null;
}

export type AppleArchivePhase =
    | 'preparing'
    | 'building_web_assets'
    | 'archiving'
    | 'exporting'
    | 'verifying'
    | 'packaging_archive'
    | 'transferring'
    | 'completed';

export interface AppleArchiveProgress {
    phase: AppleArchivePhase;
    completedBytes: number;
    totalBytes: number;
    elapsedSeconds: number;
    detail: string;
    logLine: string | null;
}

/** Every progress event carries the machine it belongs to. */
export type MachineEvent<T> = T & { machineId: string };

export interface MachineChangedEvent {
    machineId: string | null;
}

export interface ImportMacXcodeResult {
    view: MacBuilderView;
    installedPath: string;
    activationCommands: string[];
}

export interface AppleWorkspaceSyncResult {
    guestPath: string;
    snapshotSha256: string;
    sourceFileCount: number;
    sourceBytes: number;
    archiveBytes: number;
}

export interface SyncAppleWorkspaceResult {
    view: MacBuilderView;
    sync: AppleWorkspaceSyncResult;
}

export interface AppleSmokeBuildResult {
    xcodeVersion: string;
    nativeLockfileUpdated: boolean;
    outputTail: string[];
}

export interface RunAppleSmokeBuildResult {
    view: MacBuilderView;
    build: AppleSmokeBuildResult;
}

export interface RunAppleArchiveResult {
    view: MacBuilderView;
    archive: AppleArchiveResult;
}

export type DiskMigrationPhase =
    | 'checking_space'
    | 'stopping'
    | 'copying_disk'
    | 'removing'
    | 'creating'
    | 'starting'
    | 'completed';

export interface DiskMigrationProgress {
    phase: DiskMigrationPhase;
    completedBytes: number;
    totalBytes: number;
    elapsedSeconds: number;
    detail: string;
}

export type DeviceSigningPhase =
    | 'checking_kit'
    | 'creating_certificate'
    | 'registering_device'
    | 'checking_profiles'
    | 'creating_profile'
    | 'downloading_profile'
    | 'provisioning'
    | 'completed';

export interface DeviceSigningProgress {
    phase: DeviceSigningPhase;
    elapsedSeconds: number;
    detail: string;
}

export interface PrepareDeviceSigningResult {
    view: MacBuilderView;
    certificateCreated: boolean;
    deviceAlreadyRegistered: boolean;
    profileCreated: boolean;
}

export type AppleDeviceRunPhase =
    | 'preparing'
    | 'building_web_assets'
    | 'resolving_target'
    | 'building'
    | 'verifying'
    | 'installing'
    | 'launching'
    | 'running'
    | 'completed';

/** Console lines arrive in batches: an app console must not drop lines between ticks. */
export interface AppleDeviceRunProgress {
    phase: AppleDeviceRunPhase;
    elapsedSeconds: number;
    detail: string;
    logLines: string[];
}

/** How the console session ended; any of these is a run that happened, not a failure. */
export type ConsoleEnd = 'stopped' | 'exited' | 'disconnected';

export interface AppleDeviceRunResult {
    device: GuestDevice;
    bundleIdentifier: string;
    appPath: string;
    marketingVersion: string;
    buildNumber: string;
    provisioningProfileUuid: string;
    installedAtEpochSeconds: number;
    consoleEnd: ConsoleEnd;
    exitStatus: number | null;
    reattached: boolean;
    buildTail: string[];
    consoleTail: string[];
}

export interface RunAppleDeviceResult {
    view: MacBuilderView;
    run: AppleDeviceRunResult;
}

export interface AppleProvisioningProfile {
    id: string;
    name: string;
    platform: string;
    profileType: string;
    profileState: string;
    uuid: string;
    createdDate: string;
    expirationDate: string;
}

export interface AppleCertificate {
    id: string;
    name: string;
    displayName: string;
    certificateType: string;
    serialNumber: string;
    platform: string;
    expirationDate: string;
}

export interface AppleTeamVerification {
    keyId: string;
    projectDevelopmentTeam: string;
    bundleIdentifier: string;
    appStoreRecordFound: boolean;
    appStoreAppName: string | null;
    appStoreAppId: string | null;
    bundleIdFound: boolean;
    bundleIdName: string | null;
    bundleIdPlatform: string | null;
    appIdPrefix: string | null;
    bundleLookupFallbackUsed: boolean;
    developerResourcesAccessible: boolean;
    developerResourcesIssue: string | null;
    profilesAccessible: boolean;
    profilesIssue: string | null;
    profiles: AppleProvisioningProfile[];
    certificatesAccessible: boolean;
    certificatesIssue: string | null;
    certificates: AppleCertificate[];
    verifiedAtEpochSeconds: number;
}

/** A provisioning profile this host already downloaded and still holds on disk. */
export interface ManagedAppleProfile {
    fileName: string;
    path: string;
    savedAtEpochSeconds: number;
}

export interface DownloadAppleProfileResult {
    profile: AppleProvisioningProfile;
    savedPath: string;
    kit: SigningKitSummary;
}

/** An identity created at Apple for a key generated on this host, packaged into the kit. */
export interface CreateAppleCertificateResult {
    certificate: AppleCertificate;
    savedPath: string;
    kit: SigningKitSummary;
}

export interface CreateAppleProfileResult {
    profile: AppleProvisioningProfile;
    certificate: AppleCertificate;
    savedPath: string;
    kit: SigningKitSummary;
}
