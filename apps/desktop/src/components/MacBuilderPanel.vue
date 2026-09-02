<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import {
    Apple,
    Archive,
    Box,
    CheckCircle2,
    CircleAlert,
    Clock3,
    Copy,
    Cpu,
    Fingerprint,
    FolderOpen,
    HardDrive,
    Hammer,
    KeyRound,
    LoaderCircle,
    LockKeyhole,
    Play,
    RefreshCw,
    Server,
    ShieldCheck,
    Square,
    Terminal,
    Trash2,
    UploadCloud,
    Wifi,
} from '@lucide/vue';
import { computed, onMounted, onUnmounted, reactive, ref } from 'vue';

type MacOsRelease = 'tahoe' | 'sequoia' | 'sonoma' | 'ventura';
type ContainerState =
    | 'missing'
    | 'created'
    | 'running'
    | 'paused'
    | 'restarting'
    | 'exited'
    | 'dead'
    | 'unavailable'
    | 'unknown';

type MacBuilderProfile = {
    name: string;
    macosRelease: MacOsRelease;
    memoryGib: number;
    cpuCores: number;
    sshPort: number;
};

type HostPrerequisites = {
    supportedHost: boolean;
    dockerCli: boolean;
    dockerDaemon: boolean;
    dockerVersion: string | null;
    kvmAccess: boolean;
    displayAccess: boolean;
    display: string | null;
    ready: boolean;
    issues: string[];
};

type MacBuilderSecretSummary = {
    appStoreConnectConfigured: boolean;
    appStoreConnectKeyId: string | null;
    signingCertificateConfigured: boolean;
    signingCertificateName: string | null;
    provisioningProfileCount: number;
    guestKeychainConfigured: boolean;
};

type AppleTeamVerification = {
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
};

type AppleProvisioningProfile = {
    id: string;
    name: string;
    platform: string;
    profileType: string;
    profileState: string;
    uuid: string;
    createdDate: string;
    expirationDate: string;
};

type AppleCertificate = {
    id: string;
    name: string;
    displayName: string;
    certificateType: string;
    serialNumber: string;
    platform: string;
    expirationDate: string;
};

type CreateAppleProfileResult = {
    profile: AppleProvisioningProfile;
    certificate: AppleCertificate;
    savedPath: string;
    secrets: MacBuilderSecretSummary;
};

type GuestTrustState = 'unavailable' | 'untrusted' | 'trusted' | 'mismatch';

type MacGuestAccess = {
    username: string | null;
    publicKey: string | null;
    ssh: {
        portOpen: boolean;
        reachable: boolean;
        trust: GuestTrustState;
        fingerprint: string | null;
        pinnedFingerprint: string | null;
        issue: string | null;
    };
    diagnostics: {
        authenticated: boolean;
        macosVersion: string | null;
        xcodeVersion: string | null;
        xcodePath: string | null;
        xcodeSelected: boolean;
        issue: string | null;
    };
};

type XcodeImportPhase =
    | 'preparing'
    | 'transferring'
    | 'expanding'
    | 'awaiting_activation'
    | 'awaiting_authorization'
    | 'activation_failed'
    | 'failed';

type XcodeImportProgress = {
    phase: XcodeImportPhase;
    transferredBytes: number;
    totalBytes: number;
    elapsedSeconds: number;
    detail: string;
};

type ImportMacXcodeResult = {
    view: MacBuilderView;
    installedPath: string;
    activationCommands: string[];
};

type AppleWorkspace = {
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
};

type ProvisioningProfileSummary = {
    uuid: string;
    teamIdentifier: string;
    applicationIdentifier: string;
    expiresAt: string;
    developerCertificateSha256: string[];
};

type SigningProvisioningResult = {
    keychainPath: string;
    identityName: string;
    identitySha1: string;
    certificateSha256: string;
    certificateExpiresAt: string;
    developmentTeam: string;
    bundleIdentifier: string;
    profiles: ProvisioningProfileSummary[];
};

type SigningProvisioningPhase =
    | 'preparing'
    | 'transferring'
    | 'importing_certificate'
    | 'inspecting_profiles'
    | 'installing_profiles'
    | 'verifying'
    | 'completed'
    | 'failed';

type SigningProvisioningProgress = {
    phase: SigningProvisioningPhase;
    completedBytes: number;
    totalBytes: number;
    elapsedSeconds: number;
    detail: string;
};

type AppleProjectPhase =
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
    | 'completed'
    | 'failed';

type AppleProjectProgress = {
    phase: AppleProjectPhase;
    completedBytes: number;
    totalBytes: number;
    elapsedSeconds: number;
    detail: string;
    logLine: string | null;
};

type SyncAppleWorkspaceResult = {
    view: MacBuilderView;
    sync: {
        guestPath: string;
        snapshotSha256: string;
        sourceFileCount: number;
        sourceBytes: number;
        archiveBytes: number;
    };
};

type RunAppleSmokeBuildResult = {
    view: MacBuilderView;
    build: {
        xcodeVersion: string;
        nativeLockfileUpdated: boolean;
        outputTail: string[];
    };
};

type AppleArchivePhase =
    | 'preparing'
    | 'archiving'
    | 'exporting'
    | 'verifying'
    | 'packaging_archive'
    | 'transferring'
    | 'completed'
    | 'failed';

type AppleArchiveProgress = {
    phase: AppleArchivePhase;
    completedBytes: number;
    totalBytes: number;
    elapsedSeconds: number;
    detail: string;
    logLine: string | null;
};

type AppleArchiveArtifact = {
    path: string;
    bytes: number;
    sha256: string;
};

type AppleArchiveResult = {
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
};

type RunAppleArchiveResult = {
    view: MacBuilderView;
    archive: AppleArchiveResult;
};

type MacBuilderView = {
    profile: MacBuilderProfile | null;
    runtime: {
        prerequisites: HostPrerequisites;
        state: ContainerState;
        containerId: string | null;
        startedAt: string | null;
    };
    secrets: MacBuilderSecretSummary;
    guest: MacGuestAccess;
    appleWorkspace: AppleWorkspace | null;
    signing: SigningProvisioningResult | null;
    archive: AppleArchiveResult | null;
    archiveError: string | null;
    logs: string[];
};

type Operation =
    | 'refresh'
    | 'save'
    | 'launch'
    | 'stop'
    | 'secrets'
    | 'clear-secrets'
    | 'apple-team-verify'
    | 'apple-profile-create'
    | 'signing-provision'
    | 'signing-clear'
    | 'guest-config'
    | 'guest-trust'
    | 'guest-forget'
    | 'xcode-import'
    | 'xcode-activate'
    | 'workspace-approve'
    | 'workspace-clear'
    | 'workspace-sync'
    | 'workspace-build'
    | 'archive-build'
    | 'archive-reveal'
    | 'archive-clear';

const profile = reactive<MacBuilderProfile>({
    name: 'Local macOS builder',
    macosRelease: 'sequoia',
    memoryGib: 8,
    cpuCores: 4,
    sshPort: 50922,
});
const signing = reactive({
    appStoreConnectKeyId: '',
    appStoreConnectIssuerId: '',
    appStoreConnectPrivateKeyPath: '',
    signingCertificatePath: '',
    signingCertificatePassword: '',
    provisioningProfilePaths: '',
    guestKeychainPassword: '',
});
const signingSetupMethod = ref<'xcode' | 'files'>('files');
const signingSetupMethodSelected = ref(false);
const view = ref<MacBuilderView | null>(null);
const operation = ref<Operation | null>('refresh');
const error = ref<string | null>(null);
const notice = ref<string | null>(null);
const signingSecretError = ref<string | null>(null);
const appleTeamVerification = ref<AppleTeamVerification | null>(null);
const confirmClearSecrets = ref(false);
const confirmCreateAppleProfile = ref(false);
const confirmClearGuestSigning = ref(false);
const confirmForgetGuest = ref(false);
const guestUsername = ref('');
const xcodePackagePath = ref('');
const xcodeImportProgress = ref<XcodeImportProgress | null>(null);
const xcodeImportResult = ref<ImportMacXcodeResult | null>(null);
const xcodeDropActive = ref(false);
const appleWorkspacePath = ref('');
const appleProjectProgress = ref<AppleProjectProgress | null>(null);
const appleArchiveProgress = ref<AppleArchiveProgress | null>(null);
const signingProgress = ref<SigningProvisioningProgress | null>(null);
const signingStartedAt = ref<number | null>(null);
const appleBuildOutput = ref<string[]>([]);
const appleArchiveOutput = ref<string[]>([]);
const appleProjectStartedAt = ref<number | null>(null);
const appleArchiveStartedAt = ref<number | null>(null);
const clockNow = ref(Date.now());
const selectedAppleCertificateId = ref('');
const appleProfileCreationError = ref<string | null>(null);
const createdAppleProfilePath = ref<string | null>(null);
const confirmClearAppleArchive = ref(false);
const signingCertificateInput = ref<HTMLInputElement | null>(null);
let unlistenMacBuilder: UnlistenFn | null = null;
let unlistenXcodeImport: UnlistenFn | null = null;
let unlistenXcodeDrop: UnlistenFn | null = null;
let unlistenAppleProject: UnlistenFn | null = null;
let unlistenAppleArchive: UnlistenFn | null = null;
let unlistenSigning: UnlistenFn | null = null;
let installerProbeTimer: ReturnType<typeof setTimeout> | null = null;
let clockTimer: ReturnType<typeof setInterval> | null = null;

const INSTALLER_PROBE_INTERVAL_MS = 10_000;
const APPLE_BUILD_OUTPUT_LIMIT = 240;
const APPLE_DEVELOPER_CERTIFICATES_URL =
    'https://developer.apple.com/account/resources/certificates/list';
const APPLE_DEVELOPER_PROFILES_URL = 'https://developer.apple.com/account/resources/profiles/list';
const APP_STORE_CONNECT_KEYS_URL = 'https://appstoreconnect.apple.com/access/integrations/api';

const isRunning = computed(() => view.value?.runtime.state === 'running');
const launchActionLabel = computed(() =>
    view.value?.runtime.state === 'missing' ? 'Create & launch' : 'Resume macOS builder',
);
const prerequisites = computed(() => view.value?.runtime.prerequisites ?? null);
const canLaunch = computed(() => prerequisites.value?.ready === true && operation.value === null);
const hasSigningInput = computed(() =>
    Object.values(signing).some((value) => value.trim().length > 0),
);
const canVerifyAppleTeam = computed(
    () =>
        view.value?.secrets.appStoreConnectConfigured === true &&
        Boolean(view.value?.appleWorkspace?.developmentTeam) &&
        Boolean(view.value?.appleWorkspace?.bundleIdentifier) &&
        operation.value === null,
);
const activeAppStoreProfiles = computed(
    () =>
        appleTeamVerification.value?.profiles.filter(
            (profile) => profile.profileType === 'IOS_APP_STORE' && !appleProfileIsExpired(profile),
        ) ?? [],
);
const visibleAppleProfiles = computed(
    () => appleTeamVerification.value?.profiles.slice(0, 8) ?? [],
);
const visibleAppleCertificates = computed(
    () => appleTeamVerification.value?.certificates.slice(0, 8) ?? [],
);
const usableDistributionCertificates = computed(
    () => appleTeamVerification.value?.certificates.filter(appleCertificateIsUsable) ?? [],
);
const selectedAppleCertificate = computed(
    () =>
        usableDistributionCertificates.value.find(
            (certificate) => certificate.id === selectedAppleCertificateId.value,
        ) ?? null,
);
const canCreateAppleProfile = computed(
    () =>
        appleTeamVerification.value?.bundleIdFound === true &&
        appleTeamVerification.value.profilesAccessible &&
        activeAppStoreProfiles.value.length === 0 &&
        selectedAppleCertificate.value !== null &&
        operation.value === null,
);
const guestTrustLabel = computed(() => {
    const trust = view.value?.guest.ssh.trust ?? 'unavailable';

    return {
        unavailable: 'Waiting for SSH',
        untrusted: 'Verification required',
        trusted: 'Identity trusted',
        mismatch: 'Identity changed',
    }[trust];
});
const installerPhase = computed(() => {
    if (!isRunning.value) {
        return {
            title: 'macOS builder is stopped',
            detail: 'Resume it from BuildBridge or the system tray to continue setup.',
            ready: false,
        };
    }

    const guest = view.value?.guest;
    if (guest?.ssh.reachable !== true) {
        return {
            title: 'macOS installation or setup is in progress',
            detail:
                guest?.ssh.portOpen === true
                    ? 'QEMU is running, but macOS Remote Login is not ready yet.'
                    : 'QEMU is starting. Complete the installer in the console window.',
            ready: false,
        };
    }
    if (guest.ssh.trust !== 'trusted') {
        return {
            title: 'macOS is ready for identity verification',
            detail: 'Remote Login responded. Verify and trust the displayed fingerprint.',
            ready: true,
        };
    }
    if (!guest.diagnostics.authenticated) {
        return {
            title: 'Waiting for BuildBridge key access',
            detail: 'Add the generated public key to the macOS user’s authorized_keys file.',
            ready: true,
        };
    }

    if (!guest.diagnostics.xcodeSelected) {
        return {
            title: guest.diagnostics.xcodeVersion
                ? 'Xcode requires activation'
                : 'macOS is connected',
            detail: guest.diagnostics.xcodeVersion
                ? `${guest.diagnostics.xcodeVersion} is installed but not selected.`
                : 'SSH works; import a compatible Xcode .xip package.',
            ready: true,
        };
    }

    return {
        title: 'macOS and Xcode are ready',
        detail: guest.diagnostics.xcodeVersion ?? 'The selected Xcode toolchain is ready.',
        ready: true,
    };
});
const builderUptime = computed(() => {
    const startedAt = view.value?.runtime.startedAt;
    if (!startedAt || !isRunning.value) return null;

    const normalized = startedAt.replace(/\.(\d{3})\d+Z$/, '.$1Z');
    const startedAtMs = Date.parse(normalized);
    if (Number.isNaN(startedAtMs)) return null;

    const elapsedSeconds = Math.max(0, Math.floor((clockNow.value - startedAtMs) / 1000));
    const hours = Math.floor(elapsedSeconds / 3600);
    const minutes = Math.floor((elapsedSeconds % 3600) / 60);
    if (hours > 0) return `${hours}h ${minutes}m elapsed`;
    if (minutes > 0) return `${minutes}m elapsed`;

    return `${elapsedSeconds}s elapsed`;
});
const stateLabel = computed(() => {
    const state = view.value?.runtime.state ?? 'unavailable';

    return {
        missing: 'Not created',
        created: 'Created',
        running: 'Running',
        paused: 'Paused',
        restarting: 'Restarting',
        exited: 'Stopped',
        dead: 'Needs attention',
        unavailable: 'Host unavailable',
        unknown: 'Unknown',
    }[state];
});
const stateTone = computed(() => {
    const state = view.value?.runtime.state;
    if (state === 'running') return 'success';
    if (state === 'dead' || state === 'unavailable') return 'danger';
    if (state === 'restarting' || state === 'created') return 'warning';
    return 'neutral';
});
const canImportXcode = computed(
    () =>
        view.value?.guest.diagnostics.authenticated === true &&
        view.value?.guest.diagnostics.xcodeSelected !== true &&
        xcodePackagePath.value.trim().toLowerCase().endsWith('.xip') &&
        operation.value === null,
);
const xcodeImportPercent = computed(() => {
    const progress = xcodeImportProgress.value;
    if (!progress || progress.totalBytes <= 0) return 0;

    return Math.min(100, Math.round((progress.transferredBytes / progress.totalBytes) * 100));
});
const xcodeImportPhaseLabel = computed(() => {
    const phase = xcodeImportProgress.value?.phase;

    return {
        preparing: 'Preparing guest',
        transferring: 'Transferring Xcode',
        expanding: 'Verifying and expanding',
        awaiting_activation: 'Ready to activate',
        awaiting_authorization: 'Waiting for macOS Terminal',
        activation_failed: 'Activation failed',
        failed: 'Import failed',
    }[phase ?? 'preparing'];
});
const xcodeActivationCommands = computed(() => {
    if (xcodeImportResult.value) return xcodeImportResult.value.activationCommands;

    const diagnostics = view.value?.guest.diagnostics;
    if (!diagnostics?.xcodeVersion || diagnostics.xcodeSelected || !diagnostics.xcodePath)
        return [];

    return [
        `sudo xcode-select --switch '${diagnostics.xcodePath}'`,
        'sudo xcodebuild -license accept',
        'sudo xcodebuild -runFirstLaunch',
        'xcodebuild -version',
    ];
});
const appleProjectPhaseLabel = computed(() => {
    const phase = appleProjectProgress.value?.phase;

    return {
        snapshotting: 'Creating safe snapshot',
        transferring: 'Synchronizing source',
        extracting: 'Preparing guest workspace',
        preparing_tools: 'Preparing build tools',
        preparing_platform: 'Preparing iOS platform',
        installing_dependencies: 'Installing dependencies',
        building_web_assets: 'Building web assets',
        syncing_ios: 'Synchronizing iOS',
        resolving_pods: 'Resolving CocoaPods',
        building: 'Compiling iOS app',
        completed: 'Test build complete',
        failed: 'Project workflow failed',
    }[phase ?? 'snapshotting'];
});
const appleProjectPercent = computed(() => {
    const progress = appleProjectProgress.value;
    if (!progress || progress.totalBytes <= 0) return 0;

    return Math.min(100, Math.round((progress.completedBytes / progress.totalBytes) * 100));
});
const appleProjectElapsedSeconds = computed(() => {
    const reported = appleProjectProgress.value?.elapsedSeconds ?? 0;
    if (appleProjectStartedAt.value === null) return reported;

    return Math.max(reported, Math.floor((clockNow.value - appleProjectStartedAt.value) / 1_000));
});
const canApproveAppleWorkspace = computed(
    () => appleWorkspacePath.value.trim().length > 0 && operation.value === null,
);
const canSyncAppleWorkspace = computed(
    () =>
        view.value?.appleWorkspace != null &&
        view.value?.guest.diagnostics.xcodeSelected === true &&
        operation.value === null,
);
const canRunAppleSmokeBuild = computed(
    () =>
        Boolean(view.value?.appleWorkspace?.lastSnapshotSha256) &&
        view.value?.guest.diagnostics.xcodeSelected === true &&
        operation.value === null,
);
const canProvisionSigning = computed(
    () =>
        view.value?.secrets.signingCertificateConfigured === true &&
        (view.value?.secrets.provisioningProfileCount ?? 0) > 0 &&
        view.value?.secrets.guestKeychainConfigured === true &&
        view.value?.guest.diagnostics.xcodeSelected === true &&
        view.value?.appleWorkspace?.lastBuildSucceeded === true &&
        Boolean(view.value?.appleWorkspace?.developmentTeam) &&
        Boolean(view.value?.appleWorkspace?.bundleIdentifier) &&
        operation.value === null,
);
const signingPhaseLabel = computed(() => {
    const phase = signingProgress.value?.phase;

    return {
        preparing: 'Preparing signing kit',
        transferring: 'Transferring securely',
        importing_certificate: 'Importing identity',
        inspecting_profiles: 'Inspecting profiles',
        installing_profiles: 'Installing profiles',
        verifying: 'Verifying signing state',
        completed: 'Signing is provisioned',
        failed: 'Signing setup failed',
    }[phase ?? 'preparing'];
});
const signingPercent = computed(() => {
    const progress = signingProgress.value;
    if (!progress || progress.totalBytes <= 0) return 0;

    return Math.min(100, Math.round((progress.completedBytes / progress.totalBytes) * 100));
});
const signingElapsedSeconds = computed(() => {
    const reported = signingProgress.value?.elapsedSeconds ?? 0;
    if (signingStartedAt.value === null) return reported;

    return Math.max(reported, Math.floor((clockNow.value - signingStartedAt.value) / 1_000));
});
const canRunAppleArchive = computed(
    () =>
        view.value?.signing !== null &&
        view.value?.signing !== undefined &&
        view.value?.appleWorkspace?.lastBuildSucceeded === true &&
        view.value.appleWorkspace.lastNativeLockUpdated === false &&
        Boolean(view.value.appleWorkspace.lastSnapshotSha256) &&
        view.value.guest.diagnostics.xcodeSelected === true &&
        operation.value === null,
);
const appleArchivePhaseLabel = computed(() => {
    const phase = appleArchiveProgress.value?.phase;

    return {
        preparing: 'Preparing signed release',
        archiving: 'Compiling signed archive',
        exporting: 'Exporting App Store IPA',
        verifying: 'Verifying app signature',
        packaging_archive: 'Packaging Xcode archive',
        transferring: 'Retaining local artifacts',
        completed: 'Signed artifacts complete',
        failed: 'Signed archive failed',
    }[phase ?? 'preparing'];
});
const appleArchivePercent = computed(() => {
    const progress = appleArchiveProgress.value;
    if (!progress || progress.totalBytes <= 0) return 0;

    return Math.min(100, Math.round((progress.completedBytes / progress.totalBytes) * 100));
});
const appleArchiveElapsedSeconds = computed(() => {
    const reported = appleArchiveProgress.value?.elapsedSeconds ?? 0;
    if (appleArchiveStartedAt.value === null) return reported;

    return Math.max(reported, Math.floor((clockNow.value - appleArchiveStartedAt.value) / 1_000));
});
const expiredSigningCertificateRecovery = computed(
    () =>
        signingProgress.value?.phase === 'failed' &&
        signingProgress.value.detail.includes('the .p12 certificate expired at'),
);
const prerequisiteChecks = computed(() => {
    const checks = prerequisites.value;
    if (checks === null) return [];

    return [
        {
            label: 'Linux x86_64 host',
            detail: 'Docker-OSX host support',
            ready: checks.supportedHost,
            icon: Server,
        },
        {
            label: 'Docker engine',
            detail: checks.dockerVersion ?? 'CLI and daemon access',
            ready: checks.dockerCli && checks.dockerDaemon,
            icon: Box,
        },
        {
            label: 'KVM acceleration',
            detail: 'Read/write access to /dev/kvm',
            ready: checks.kvmAccess,
            icon: Cpu,
        },
        {
            label: 'First-boot display',
            detail: checks.display ?? 'X11 display unavailable',
            ready: checks.displayAccess,
            icon: Terminal,
        },
    ];
});

onMounted(async () => {
    unlistenMacBuilder = await listen('mac-builder-changed', () => {
        if (operation.value === null) void refresh();
    });
    unlistenXcodeImport = await listen<XcodeImportProgress>(
        'mac-builder-xcode-import-progress',
        ({ payload }) => {
            xcodeImportProgress.value = payload;
        },
    );
    unlistenAppleProject = await listen<AppleProjectProgress>(
        'mac-builder-apple-project-progress',
        ({ payload }) => {
            appleProjectProgress.value = payload;
            if (payload.logLine) {
                appleBuildOutput.value.push(payload.logLine);
                if (appleBuildOutput.value.length > APPLE_BUILD_OUTPUT_LIMIT) {
                    appleBuildOutput.value.shift();
                }
            }
        },
    );
    unlistenAppleArchive = await listen<AppleArchiveProgress>(
        'mac-builder-apple-archive-progress',
        ({ payload }) => {
            appleArchiveProgress.value = payload;
            if (payload.logLine) {
                appleArchiveOutput.value.push(payload.logLine);
                if (appleArchiveOutput.value.length > APPLE_BUILD_OUTPUT_LIMIT) {
                    appleArchiveOutput.value.shift();
                }
            }
        },
    );
    unlistenSigning = await listen<SigningProvisioningProgress>(
        'mac-builder-signing-progress',
        ({ payload }) => {
            signingProgress.value = payload;
        },
    );
    unlistenXcodeDrop = await getCurrentWebview().onDragDropEvent(({ payload }) => {
        if (payload.type === 'enter') {
            xcodeDropActive.value = payload.paths.some((path) =>
                path.toLowerCase().endsWith('.xip'),
            );
            return;
        }
        if (payload.type === 'drop') {
            const packagePath = payload.paths.find((path) => path.toLowerCase().endsWith('.xip'));
            if (packagePath) {
                xcodePackagePath.value = packagePath;
                error.value = null;
            } else if (payload.paths[0]) {
                appleWorkspacePath.value = payload.paths[0];
                error.value = null;
            }
        }
        if (payload.type === 'drop' || payload.type === 'leave') xcodeDropActive.value = false;
    });
    clockTimer = setInterval(() => {
        clockNow.value = Date.now();
    }, 1_000);
    await refresh();
});

onUnmounted(() => {
    unlistenMacBuilder?.();
    unlistenMacBuilder = null;
    unlistenXcodeImport?.();
    unlistenXcodeImport = null;
    unlistenAppleProject?.();
    unlistenAppleProject = null;
    unlistenAppleArchive?.();
    unlistenAppleArchive = null;
    unlistenSigning?.();
    unlistenSigning = null;
    unlistenXcodeDrop?.();
    unlistenXcodeDrop = null;
    if (installerProbeTimer !== null) clearTimeout(installerProbeTimer);
    if (clockTimer !== null) clearInterval(clockTimer);
    installerProbeTimer = null;
    clockTimer = null;
});

async function refresh(): Promise<void> {
    operation.value = 'refresh';

    try {
        setView(await invoke<MacBuilderView>('get_mac_builder_status'));
        error.value = null;
    } catch (caught) {
        const message = String(caught);
        const previous = xcodeImportProgress.value;
        error.value = message;
        xcodeImportProgress.value = {
            phase: 'failed',
            transferredBytes: previous?.transferredBytes ?? 0,
            totalBytes: previous?.totalBytes ?? 0,
            elapsedSeconds: previous?.elapsedSeconds ?? 0,
            detail: message,
        };
    } finally {
        operation.value = null;
    }
}

async function saveConfiguration(): Promise<void> {
    operation.value = 'save';
    notice.value = null;

    try {
        setView(
            await invoke<MacBuilderView>('configure_mac_builder', {
                profile: { ...profile },
            }),
        );
        notice.value = 'Builder profile saved locally.';
        error.value = null;
    } catch (caught) {
        error.value = String(caught);
    } finally {
        operation.value = null;
    }
}

async function launch(): Promise<void> {
    operation.value = 'launch';
    notice.value = null;

    try {
        if (hasSigningInput.value) await persistSecrets();
        setView(
            await invoke<MacBuilderView>('launch_mac_builder', {
                profile: { ...profile },
            }),
        );
        notice.value =
            'Docker-OSX launched. Complete macOS installation in the host display window.';
        error.value = null;
    } catch (caught) {
        error.value = String(caught);
    } finally {
        operation.value = null;
    }
}

async function stop(): Promise<void> {
    operation.value = 'stop';
    notice.value = null;

    try {
        setView(await invoke<MacBuilderView>('stop_mac_builder'));
        notice.value = 'Builder stopped. Its macOS disk remains in the managed container.';
        error.value = null;
    } catch (caught) {
        error.value = String(caught);
    } finally {
        operation.value = null;
    }
}

async function saveSecrets(): Promise<void> {
    operation.value = 'secrets';
    notice.value = null;
    error.value = null;
    signingSecretError.value = null;

    try {
        await persistSecrets();
        appleTeamVerification.value = null;
        notice.value = 'Signing kit stored in the operating system credential vault.';
        error.value = null;
    } catch (caught) {
        const message = String(caught);
        error.value = message;
        signingSecretError.value = message;
    } finally {
        operation.value = null;
    }
}

async function persistSecrets(): Promise<void> {
    const summary = await invoke<MacBuilderSecretSummary>('save_mac_builder_secrets', {
        input: {
            appStoreConnectKeyId: signing.appStoreConnectKeyId,
            appStoreConnectIssuerId: signing.appStoreConnectIssuerId,
            appStoreConnectPrivateKeyPath: signing.appStoreConnectPrivateKeyPath,
            signingCertificatePath: signing.signingCertificatePath,
            signingCertificatePassword: signing.signingCertificatePassword,
            provisioningProfilePaths: signing.provisioningProfilePaths
                .split('\n')
                .map((path) => path.trim())
                .filter(Boolean),
            guestKeychainPassword: signing.guestKeychainPassword,
        },
    });

    if (view.value !== null) view.value.secrets = summary;
    clearSensitiveForm();
}

async function clearSecrets(): Promise<void> {
    if (!confirmClearSecrets.value) {
        confirmClearSecrets.value = true;
        return;
    }

    operation.value = 'clear-secrets';
    notice.value = null;

    try {
        const summary = await invoke<MacBuilderSecretSummary>('clear_mac_builder_secrets');
        if (view.value !== null) view.value.secrets = summary;
        clearSensitiveForm();
        signingSecretError.value = null;
        appleTeamVerification.value = null;
        notice.value = 'Signing kit removed from the operating system credential vault.';
        error.value = null;
    } catch (caught) {
        error.value = String(caught);
    } finally {
        confirmClearSecrets.value = false;
        operation.value = null;
    }
}

function appleProfileIsExpired(profile: AppleProvisioningProfile): boolean {
    const expiration = Date.parse(profile.expirationDate);
    return (
        profile.profileState !== 'ACTIVE' ||
        Number.isNaN(expiration) ||
        expiration <= clockNow.value
    );
}

function appleCertificateIsUsable(certificate: AppleCertificate): boolean {
    const expiration = Date.parse(certificate.expirationDate);
    return (
        ['DISTRIBUTION', 'IOS_DISTRIBUTION'].includes(certificate.certificateType) &&
        !Number.isNaN(expiration) &&
        expiration > clockNow.value
    );
}

function appleCertificateIsExpired(certificate: AppleCertificate): boolean {
    const expiration = Date.parse(certificate.expirationDate);
    return Number.isNaN(expiration) || expiration <= clockNow.value;
}

function formatAppleCertificateType(certificateType: string): string {
    return (
        {
            DISTRIBUTION: 'Apple Distribution',
            IOS_DISTRIBUTION: 'iOS Distribution',
            DEVELOPMENT: 'Apple Development',
            IOS_DEVELOPMENT: 'iOS Development',
        }[certificateType] ?? certificateType.replace(/_/g, ' ').toLowerCase()
    );
}

function appleCertificateStatusLabel(certificate: AppleCertificate): string {
    if (!['DISTRIBUTION', 'IOS_DISTRIBUTION'].includes(certificate.certificateType)) {
        return 'Not for App Store';
    }
    if (appleCertificateIsExpired(certificate)) return 'Expired';

    return 'Ready';
}

function appleCertificateLabel(certificate: AppleCertificate): string {
    return certificate.displayName || certificate.name;
}

function formatAppleDate(value: string): string {
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return 'Unavailable';

    return new Intl.DateTimeFormat(undefined, {
        year: 'numeric',
        month: 'short',
        day: 'numeric',
    }).format(date);
}

function appleProfileStatusLabel(profile: AppleProvisioningProfile): string {
    const expiration = Date.parse(profile.expirationDate);
    if (profile.profileState === 'INVALID') return 'Invalid';
    if (!Number.isNaN(expiration) && expiration <= clockNow.value) return 'Expired';
    if (profile.profileState === 'ACTIVE' && !Number.isNaN(expiration)) return 'Active';
    return 'Status unavailable';
}

function formatAppleProfileType(value: string): string {
    return (
        {
            IOS_APP_STORE: 'iOS App Store',
            IOS_APP_DEVELOPMENT: 'iOS development',
            IOS_APP_ADHOC: 'iOS ad hoc',
            MAC_CATALYST_APP_STORE: 'Mac Catalyst App Store',
        }[value] ?? value.replace(/_/g, ' ').toLowerCase()
    );
}

async function verifyAppleDeveloperTeam(): Promise<void> {
    if (!canVerifyAppleTeam.value) return;

    operation.value = 'apple-team-verify';
    notice.value = null;
    error.value = null;
    appleTeamVerification.value = null;
    confirmCreateAppleProfile.value = false;
    appleProfileCreationError.value = null;
    createdAppleProfilePath.value = null;

    try {
        const result = await invoke<AppleTeamVerification>('verify_apple_developer_team');
        appleTeamVerification.value = result;
        selectedAppleCertificateId.value =
            result.certificates.find(appleCertificateIsUsable)?.id ?? '';
        if (result.appStoreRecordFound && result.bundleIdFound) {
            notice.value = `Apple Team API key verified; the App Store app and ${result.bundleIdentifier} both match.`;
        } else if (result.appStoreRecordFound) {
            notice.value = `Apple Team API key verified and the App Store app was found; its Developer provisioning identifier needs attention.`;
        } else {
            notice.value = `Apple Team API key verified, but no App Store app exactly matches ${result.bundleIdentifier}.`;
        }
    } catch (caught) {
        error.value = String(caught);
    } finally {
        operation.value = null;
    }
}

async function createAppleReplacementProfile(): Promise<void> {
    if (!canCreateAppleProfile.value) return;
    if (!confirmCreateAppleProfile.value) {
        confirmCreateAppleProfile.value = true;
        appleProfileCreationError.value = null;
        return;
    }

    operation.value = 'apple-profile-create';
    notice.value = null;
    error.value = null;
    appleProfileCreationError.value = null;
    createdAppleProfilePath.value = null;

    try {
        const result = await invoke<CreateAppleProfileResult>('create_apple_replacement_profile', {
            input: {
                certificateId: selectedAppleCertificateId.value,
                confirmed: true,
            },
        });
        if (view.value !== null) view.value.secrets = result.secrets;
        if (appleTeamVerification.value !== null) {
            appleTeamVerification.value.profiles = [
                result.profile,
                ...appleTeamVerification.value.profiles.filter(
                    (profile) => profile.id !== result.profile.id,
                ),
            ];
        }
        createdAppleProfilePath.value = result.savedPath;
        notice.value = `${result.profile.name} was created with Apple and retained by BuildBridge.`;
    } catch (caught) {
        const message = String(caught);
        error.value = message;
        appleProfileCreationError.value = message;
    } finally {
        confirmCreateAppleProfile.value = false;
        operation.value = null;
    }
}

async function provisionSigning(): Promise<void> {
    if (!canProvisionSigning.value) return;

    operation.value = 'signing-provision';
    notice.value = null;
    error.value = null;
    confirmClearGuestSigning.value = false;
    signingStartedAt.value = Date.now();
    signingProgress.value = {
        phase: 'preparing',
        completedBytes: 0,
        totalBytes: 0,
        elapsedSeconds: 0,
        detail: 'Validating the signing kit and preparing the macOS keychain.',
    };

    try {
        setView(await invoke<MacBuilderView>('provision_mac_signing'));
        signingProgress.value = null;
        notice.value =
            'Signing identity and provisioning profiles were verified and installed in macOS.';
    } catch (caught) {
        const message = String(caught);
        const previous = signingProgress.value;
        error.value = message;
        signingProgress.value = {
            phase: 'failed',
            completedBytes: previous?.completedBytes ?? 0,
            totalBytes: previous?.totalBytes ?? 0,
            elapsedSeconds: signingElapsedSeconds.value,
            detail: message,
        };
    } finally {
        signingStartedAt.value = null;
        operation.value = null;
    }
}

async function clearGuestSigning(): Promise<void> {
    if (!confirmClearGuestSigning.value) {
        confirmClearGuestSigning.value = true;
        return;
    }

    operation.value = 'signing-clear';
    notice.value = null;
    error.value = null;

    try {
        setView(await invoke<MacBuilderView>('clear_mac_guest_signing'));
        signingProgress.value = null;
        notice.value = 'The BuildBridge keychain and installed profiles were removed from macOS.';
    } catch (caught) {
        error.value = String(caught);
    } finally {
        confirmClearGuestSigning.value = false;
        operation.value = null;
    }
}

async function saveGuestAccess(): Promise<void> {
    operation.value = 'guest-config';
    notice.value = null;

    try {
        setView(
            await invoke<MacBuilderView>('configure_mac_guest_access', {
                input: { username: guestUsername.value },
            }),
        );
        notice.value = 'Guest access key is ready. Add the public key inside macOS.';
        error.value = null;
    } catch (caught) {
        error.value = String(caught);
    } finally {
        operation.value = null;
    }
}

async function trustGuest(): Promise<void> {
    const fingerprint = view.value?.guest.ssh.fingerprint;
    if (!fingerprint) return;

    operation.value = 'guest-trust';
    notice.value = null;

    try {
        setView(
            await invoke<MacBuilderView>('trust_mac_builder_guest', {
                input: { fingerprint },
            }),
        );
        notice.value = 'The verified macOS guest fingerprint is now pinned.';
        error.value = null;
    } catch (caught) {
        error.value = String(caught);
    } finally {
        operation.value = null;
    }
}

async function forgetGuestTrust(): Promise<void> {
    if (!confirmForgetGuest.value) {
        confirmForgetGuest.value = true;
        return;
    }

    operation.value = 'guest-forget';
    notice.value = null;

    try {
        setView(await invoke<MacBuilderView>('forget_mac_builder_guest_trust'));
        notice.value = 'The old guest identity pin was removed. Verify the new fingerprint.';
        error.value = null;
    } catch (caught) {
        error.value = String(caught);
    } finally {
        confirmForgetGuest.value = false;
        operation.value = null;
    }
}

async function copyGuestPublicKey(): Promise<void> {
    const publicKey = view.value?.guest.publicKey;
    if (!publicKey) return;

    try {
        await navigator.clipboard.writeText(publicKey);
        notice.value = 'Guest public key copied.';
        error.value = null;
    } catch (caught) {
        error.value = `Could not copy the public key: ${String(caught)}`;
    }
}

function selectSigningSetupMethod(method: 'xcode' | 'files'): void {
    signingSetupMethod.value = method;
    signingSetupMethodSelected.value = true;
    error.value = null;
}

function startReplacingExpiredSigningCertificate(): void {
    signingSetupMethod.value = 'files';
    signingSetupMethodSelected.value = true;
    error.value = null;
    requestAnimationFrame(() => {
        signingCertificateInput.value?.scrollIntoView({ behavior: 'smooth', block: 'center' });
        signingCertificateInput.value?.focus();
    });
}

async function copySigningResourceLink(label: string, url: string): Promise<void> {
    try {
        await navigator.clipboard.writeText(url);
        notice.value = `${label} link copied. Open it in your trusted host browser.`;
        error.value = null;
    } catch (caught) {
        error.value = `Could not copy the ${label.toLowerCase()} link: ${String(caught)}`;
    }
}

function inferAppStoreConnectKeyId(): void {
    if (signing.appStoreConnectKeyId) return;

    const pathParts = signing.appStoreConnectPrivateKeyPath.split(/[\\/]/);
    const fileName = pathParts[pathParts.length - 1] ?? '';
    const match = /^AuthKey_([A-Z0-9]{10})\.p8$/i.exec(fileName);
    if (match) signing.appStoreConnectKeyId = match[1].toUpperCase();
}

function setView(result: MacBuilderView): void {
    view.value = result;
    if (
        !signingSetupMethodSelected.value &&
        (result.signing !== null || result.secrets.signingCertificateConfigured)
    ) {
        signingSetupMethod.value = 'files';
    }
    if (result.profile !== null) Object.assign(profile, result.profile);
    if (result.guest.username !== null) guestUsername.value = result.guest.username;
    if (result.appleWorkspace !== null && !appleWorkspacePath.value) {
        appleWorkspacePath.value = result.appleWorkspace.localPath;
    }
    scheduleInstallerProbe();
}

function scheduleInstallerProbe(): void {
    if (installerProbeTimer !== null) clearTimeout(installerProbeTimer);
    installerProbeTimer = null;

    if (!isRunning.value || view.value?.guest.ssh.reachable === true) return;

    installerProbeTimer = setTimeout(
        () => void probeInstallerProgress(),
        INSTALLER_PROBE_INTERVAL_MS,
    );
}

async function probeInstallerProgress(): Promise<void> {
    installerProbeTimer = null;
    if (operation.value !== null) {
        scheduleInstallerProbe();
        return;
    }

    try {
        setView(await invoke<MacBuilderView>('get_mac_builder_status'));
        error.value = null;
    } catch (caught) {
        error.value = String(caught);
        scheduleInstallerProbe();
    }
}

async function importXcodePackage(): Promise<void> {
    if (!canImportXcode.value) return;

    operation.value = 'xcode-import';
    notice.value = null;
    error.value = null;
    xcodeImportResult.value = null;
    xcodeImportProgress.value = {
        phase: 'preparing',
        transferredBytes: 0,
        totalBytes: 0,
        elapsedSeconds: 0,
        detail: 'Validating the downloaded Apple XIP archive.',
    };

    try {
        const result = await invoke<ImportMacXcodeResult>('import_mac_xcode_package', {
            input: { path: xcodePackagePath.value.trim() },
        });
        xcodeImportResult.value = result;
        setView(result.view);
        notice.value = 'Xcode was transferred and expanded. Activate it from BuildBridge.';
    } catch (caught) {
        error.value = String(caught);
    } finally {
        operation.value = null;
    }
}

async function activateXcode(): Promise<void> {
    if (xcodeActivationCommands.value.length === 0 || operation.value !== null) return;

    operation.value = 'xcode-activate';
    notice.value = null;
    error.value = null;
    xcodeImportProgress.value = {
        phase: 'awaiting_authorization',
        transferredBytes: 0,
        totalBytes: 0,
        elapsedSeconds: 0,
        detail: 'Complete the guided activation in the macOS Terminal window.',
    };

    try {
        setView(await invoke<MacBuilderView>('activate_mac_xcode'));
        xcodeImportResult.value = null;
        xcodeImportProgress.value = null;
        notice.value = 'Xcode is activated and ready for BuildBridge builds.';
    } catch (caught) {
        const message = String(caught);
        const previous = xcodeImportProgress.value;
        error.value = message;
        xcodeImportProgress.value = {
            phase: 'activation_failed',
            transferredBytes: 0,
            totalBytes: 0,
            elapsedSeconds: previous?.elapsedSeconds ?? 0,
            detail: message,
        };
    } finally {
        operation.value = null;
    }
}

async function copyXcodeActivationCommands(): Promise<void> {
    if (xcodeActivationCommands.value.length === 0) return;

    try {
        await navigator.clipboard.writeText(xcodeActivationCommands.value.join('\n'));
        notice.value = 'Xcode activation commands copied.';
        error.value = null;
    } catch (caught) {
        error.value = `Could not copy the activation commands: ${String(caught)}`;
    }
}

async function approveAppleWorkspace(): Promise<void> {
    if (!canApproveAppleWorkspace.value) return;

    operation.value = 'workspace-approve';
    notice.value = null;
    error.value = null;

    try {
        appleTeamVerification.value = null;
        setView(
            await invoke<MacBuilderView>('approve_apple_workspace', {
                input: { path: appleWorkspacePath.value.trim() },
            }),
        );
        appleProjectProgress.value = null;
        appleBuildOutput.value = [];
        notice.value = 'Local Apple project approved. Source has not been copied yet.';
    } catch (caught) {
        error.value = String(caught);
    } finally {
        operation.value = null;
    }
}

async function clearAppleWorkspace(): Promise<void> {
    if (operation.value !== null) return;

    operation.value = 'workspace-clear';
    notice.value = null;
    error.value = null;

    try {
        setView(await invoke<MacBuilderView>('clear_apple_workspace'));
        appleTeamVerification.value = null;
        appleWorkspacePath.value = '';
        appleProjectProgress.value = null;
        appleBuildOutput.value = [];
        notice.value = 'Local project approval removed. Host source was not changed.';
    } catch (caught) {
        error.value = String(caught);
    } finally {
        operation.value = null;
    }
}

async function syncAppleWorkspace(): Promise<void> {
    if (!canSyncAppleWorkspace.value) return;

    operation.value = 'workspace-sync';
    notice.value = null;
    error.value = null;
    appleBuildOutput.value = [];
    appleProjectStartedAt.value = Date.now();
    appleProjectProgress.value = {
        phase: 'snapshotting',
        completedBytes: 0,
        totalBytes: 0,
        elapsedSeconds: 0,
        detail: 'Inspecting the approved project and filtering dependencies and secrets.',
        logLine: null,
    };

    try {
        const result = await invoke<SyncAppleWorkspaceResult>('sync_apple_workspace');
        setView(result.view);
        appleProjectProgress.value = null;
        notice.value = `Source synchronized securely (${result.sync.sourceFileCount} files, ${formatBytes(result.sync.sourceBytes)}).`;
    } catch (caught) {
        const message = String(caught);
        const previous = appleProjectProgress.value;
        const elapsedSeconds = appleProjectElapsedSeconds.value;
        error.value = message;
        appleProjectProgress.value = {
            phase: 'failed',
            completedBytes: previous?.completedBytes ?? 0,
            totalBytes: previous?.totalBytes ?? 0,
            elapsedSeconds,
            detail: message,
            logLine: null,
        };
    } finally {
        appleProjectStartedAt.value = null;
        operation.value = null;
    }
}

async function runAppleSmokeBuild(): Promise<void> {
    if (!canRunAppleSmokeBuild.value) return;

    operation.value = 'workspace-build';
    notice.value = null;
    error.value = null;
    appleBuildOutput.value = [];
    appleProjectStartedAt.value = Date.now();
    appleProjectProgress.value = {
        phase: 'preparing_tools',
        completedBytes: 0,
        totalBytes: 0,
        elapsedSeconds: 0,
        detail: 'Preparing pinned build tools in the macOS guest.',
        logLine: null,
    };

    try {
        const result = await invoke<RunAppleSmokeBuildResult>('run_apple_smoke_build');
        setView(result.view);
        if (appleBuildOutput.value.length === 0) {
            appleBuildOutput.value = result.build.outputTail;
        }
        notice.value = result.build.nativeLockfileUpdated
            ? 'Unsigned iOS build completed. The guest-only native lockfile was refreshed and needs review before a release build.'
            : 'The real project completed an unsigned iOS Simulator build.';
    } catch (caught) {
        const message = String(caught);
        const elapsedSeconds = appleProjectElapsedSeconds.value;
        const previous = appleProjectProgress.value;
        appendBuildFailureLines(message);
        error.value = message;
        appleProjectProgress.value = {
            phase: 'failed',
            completedBytes: previous?.completedBytes ?? 0,
            totalBytes: previous?.totalBytes ?? 0,
            elapsedSeconds,
            detail: message,
            logLine: null,
        };
    } finally {
        appleProjectStartedAt.value = null;
        operation.value = null;
    }
}

async function runAppleSignedArchive(): Promise<void> {
    if (!canRunAppleArchive.value) return;

    operation.value = 'archive-build';
    notice.value = null;
    error.value = null;
    confirmClearAppleArchive.value = false;
    appleArchiveOutput.value = [];
    appleArchiveStartedAt.value = Date.now();
    appleArchiveProgress.value = {
        phase: 'preparing',
        completedBytes: 0,
        totalBytes: 0,
        elapsedSeconds: 0,
        detail: 'Preparing the fixed Release and App Store Connect export recipe.',
        logLine: null,
    };

    try {
        const result = await invoke<RunAppleArchiveResult>('run_apple_signed_archive');
        setView(result.view);
        if (appleArchiveOutput.value.length === 0) {
            appleArchiveOutput.value = result.archive.outputTail;
        }
        notice.value = `Signed ${result.archive.bundleIdentifier} ${result.archive.marketingVersion} (${result.archive.buildNumber}) and retained its IPA and Xcode archive locally.`;
    } catch (caught) {
        const message = String(caught);
        const previous = appleArchiveProgress.value;
        for (const line of message.split('\n').slice(-12)) {
            if (line.trim()) appleArchiveOutput.value.push(line.trim());
        }
        error.value = message;
        appleArchiveProgress.value = {
            phase: 'failed',
            completedBytes: previous?.completedBytes ?? 0,
            totalBytes: previous?.totalBytes ?? 0,
            elapsedSeconds: appleArchiveElapsedSeconds.value,
            detail: message,
            logLine: null,
        };
    } finally {
        appleArchiveStartedAt.value = null;
        operation.value = null;
    }
}

async function copyAppleArtifactPath(path: string, label: string): Promise<void> {
    try {
        await navigator.clipboard.writeText(path);
        notice.value = `${label} path copied.`;
        error.value = null;
    } catch (caught) {
        error.value = `Could not copy the ${label.toLowerCase()} path: ${String(caught)}`;
    }
}

async function revealAppleArchive(): Promise<void> {
    operation.value = 'archive-reveal';
    notice.value = null;
    error.value = null;

    try {
        await invoke('reveal_apple_archive');
        notice.value = 'Opened the BuildBridge signed-artifact folder.';
    } catch (caught) {
        error.value = String(caught);
    } finally {
        operation.value = null;
    }
}

async function clearAppleArchive(): Promise<void> {
    if (!confirmClearAppleArchive.value) {
        confirmClearAppleArchive.value = true;
        return;
    }

    operation.value = 'archive-clear';
    notice.value = null;
    error.value = null;

    try {
        setView(await invoke<MacBuilderView>('clear_apple_archive'));
        appleArchiveProgress.value = null;
        appleArchiveOutput.value = [];
        notice.value = 'The retained IPA and Xcode archive were removed from this host.';
    } catch (caught) {
        error.value = String(caught);
    } finally {
        confirmClearAppleArchive.value = false;
        operation.value = null;
    }
}

function formatBytes(bytes: number): string {
    if (bytes <= 0) return '0 B';
    const units = ['B', 'KiB', 'MiB', 'GiB'];
    const unit = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);

    return `${(bytes / 1024 ** unit).toFixed(unit > 1 ? 1 : 0)} ${units[unit]}`;
}

function formatElapsed(seconds: number): string {
    const minutes = Math.floor(seconds / 60);
    const remainder = seconds % 60;

    return minutes > 0 ? `${minutes}m ${remainder}s` : `${remainder}s`;
}

function appendBuildFailureLines(message: string): void {
    const failureLines = message
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter((line) => line.length > 0)
        .slice(-20);

    for (const line of failureLines) {
        if (appleBuildOutput.value[appleBuildOutput.value.length - 1] !== line) {
            appleBuildOutput.value.push(line);
        }
    }
    if (appleBuildOutput.value.length > APPLE_BUILD_OUTPUT_LIMIT) {
        appleBuildOutput.value.splice(0, appleBuildOutput.value.length - APPLE_BUILD_OUTPUT_LIMIT);
    }
}

function clearSensitiveForm(): void {
    for (const key of Object.keys(signing) as Array<keyof typeof signing>) signing[key] = '';
}
</script>

<template>
    <main class="builder-content">
        <section class="builder-hero">
            <div>
                <p class="builder-eyebrow">Managed executor</p>
                <div class="builder-title-row">
                    <h1>macOS builder</h1>
                    <span class="state-badge" :class="stateTone"><i />{{ stateLabel }}</span>
                </div>
                <p>
                    Configure and retain a Docker-OSX machine here. BuildBridge owns its lifecycle;
                    connect Xcode interactively when Apple permits it, or import a portable signing
                    kit without giving BuildBridge your Apple password.
                </p>
            </div>
            <button
                class="icon-button"
                type="button"
                title="Refresh builder status"
                :disabled="operation !== null"
                @click="refresh()"
            >
                <LoaderCircle v-if="operation === 'refresh'" :size="17" class="spin" />
                <RefreshCw v-else :size="17" />
            </button>
        </section>

        <div v-if="error" class="builder-banner error">
            <CircleAlert :size="17" />
            <span>{{ error }}</span>
        </div>
        <div v-if="notice" class="builder-banner notice">
            <CheckCircle2 :size="17" />
            <span>{{ notice }}</span>
        </div>

        <div class="builder-grid">
            <section class="builder-card configuration-card">
                <header class="card-header">
                    <span class="step-number">01</span>
                    <div>
                        <h2>Host and machine</h2>
                        <p>Validated settings become fixed Docker arguments.</p>
                    </div>
                </header>

                <div class="check-grid">
                    <article
                        v-for="check in prerequisiteChecks"
                        :key="check.label"
                        class="check-item"
                        :class="{ ready: check.ready }"
                    >
                        <component :is="check.icon" :size="16" />
                        <div>
                            <strong>{{ check.label }}</strong>
                            <small>{{ check.detail }}</small>
                        </div>
                        <CheckCircle2 v-if="check.ready" :size="15" class="check-result" />
                        <CircleAlert v-else :size="15" class="check-result" />
                    </article>
                </div>

                <ul v-if="prerequisites?.issues.length" class="issue-list">
                    <li v-for="issue in prerequisites.issues" :key="issue">{{ issue }}</li>
                </ul>

                <form class="profile-form" @submit.prevent="launch">
                    <label class="wide-field">
                        <span>Builder name</span>
                        <input v-model.trim="profile.name" required maxlength="80" />
                    </label>
                    <label>
                        <span>macOS release</span>
                        <select v-model="profile.macosRelease">
                            <option value="tahoe">Tahoe 26</option>
                            <option value="sequoia">Sequoia 15</option>
                            <option value="sonoma">Sonoma 14</option>
                            <option value="ventura">Ventura 13</option>
                        </select>
                    </label>
                    <label>
                        <span>Memory</span>
                        <div class="input-suffix">
                            <input
                                v-model.number="profile.memoryGib"
                                type="number"
                                min="4"
                                max="64"
                                required
                            />
                            <b>GiB</b>
                        </div>
                    </label>
                    <label>
                        <span>CPU cores</span>
                        <input
                            v-model.number="profile.cpuCores"
                            type="number"
                            min="2"
                            max="32"
                            required
                        />
                    </label>
                    <label>
                        <span>Guest SSH port</span>
                        <input
                            v-model.number="profile.sshPort"
                            type="number"
                            min="1024"
                            max="65535"
                            required
                        />
                    </label>

                    <div class="profile-actions wide-field">
                        <button
                            class="secondary-button"
                            type="button"
                            :disabled="operation !== null"
                            @click="saveConfiguration"
                        >
                            <HardDrive :size="15" />
                            {{ operation === 'save' ? 'Saving…' : 'Save profile' }}
                        </button>
                        <button
                            v-if="!isRunning"
                            class="launch-button"
                            type="submit"
                            :disabled="!canLaunch"
                        >
                            <LoaderCircle v-if="operation === 'launch'" :size="16" class="spin" />
                            <Play v-else :size="15" />
                            {{
                                operation === 'launch' ? 'Preparing Docker-OSX…' : launchActionLabel
                            }}
                        </button>
                        <button
                            v-else
                            class="stop-button"
                            type="button"
                            :disabled="operation !== null"
                            @click="stop"
                        >
                            <LoaderCircle v-if="operation === 'stop'" :size="16" class="spin" />
                            <Square v-else :size="14" /> Stop safely
                        </button>
                    </div>
                </form>

                <p class="persistence-note">
                    <HardDrive :size="14" /> Disk erase and macOS installation are first-time setup
                    only. Stop preserves the managed container and its disk; BuildBridge never
                    removes it implicitly.
                </p>
            </section>

            <section class="builder-card signing-card">
                <header class="card-header">
                    <span class="step-number blue">02</span>
                    <div>
                        <h2>Apple signing</h2>
                        <p>Choose one setup route. The App Store upload key is optional.</p>
                    </div>
                </header>

                <div class="signing-methods" role="group" aria-label="Apple signing setup route">
                    <button
                        type="button"
                        :class="{ selected: signingSetupMethod === 'xcode' }"
                        :aria-pressed="signingSetupMethod === 'xcode'"
                        @click="selectSigningSetupMethod('xcode')"
                    >
                        <Apple :size="18" />
                        <span>
                            <strong>Sign in through Xcode</strong>
                            <small>One-time account setup in the persistent macOS guest.</small>
                        </span>
                        <b>Best effort</b>
                    </button>
                    <button
                        type="button"
                        :class="{ selected: signingSetupMethod === 'files' }"
                        :aria-pressed="signingSetupMethod === 'files'"
                        @click="selectSigningSetupMethod('files')"
                    >
                        <Archive :size="18" />
                        <span>
                            <strong>Import signing files</strong>
                            <small
                                >Portable setup that does not depend on guest account login.</small
                            >
                        </span>
                        <b>Recommended now</b>
                    </button>
                </div>

                <section v-if="signingSetupMethod === 'xcode'" class="signing-method-guide">
                    <header>
                        <div>
                            <strong>No signing files are required for this route</strong>
                            <small>
                                Apple credentials and 2FA stay entirely inside Xcode and the macOS
                                keychain.
                            </small>
                        </div>
                        <span class="state-badge warning"><i />May be blocked</span>
                    </header>
                    <ol class="signing-steps">
                        <li>
                            <span>1</span>
                            <p>
                                In the macOS guest, open <b>Xcode → Settings → Accounts</b> and add
                                the Apple Account that belongs to the required developer team.
                            </p>
                        </li>
                        <li>
                            <span>2</span>
                            <p>
                                Complete Apple’s sign-in and 2FA in Xcode. Select the team, choose
                                <b>Manage Certificates</b>, and create an
                                <b>Apple Distribution</b> certificate if one is unavailable.
                            </p>
                        </li>
                        <li>
                            <span>3</span>
                            <p>
                                In the project’s <b>Signing &amp; Capabilities</b>, enable
                                <b>Automatically manage signing</b> and select the same team.
                            </p>
                        </li>
                    </ol>
                    <p class="signing-route-note">
                        Apple may reject account sign-in in a virtualized macOS guest. If Xcode
                        reports an unknown verification error, cancel instead of repeatedly retrying
                        the password and use <b>Import signing files</b>. BuildBridge does not apply
                        VM-hiding kernel patches.
                    </p>
                </section>

                <template v-else>
                    <section class="signing-method-guide">
                        <header>
                            <div>
                                <strong>Get a matching certificate and profile</strong>
                                <small>
                                    Apple’s downloaded .cer is not enough; the .p12 must contain its
                                    private key.
                                </small>
                            </div>
                            <span class="state-badge success"><i />Ready to use</span>
                        </header>

                        <div class="signing-target">
                            <span>
                                Team
                                <code>{{
                                    view?.appleWorkspace?.developmentTeam ??
                                    'detected after project approval'
                                }}</code>
                            </span>
                            <span>
                                Bundle
                                <code>{{
                                    view?.appleWorkspace?.bundleIdentifier ??
                                    'detected after project approval'
                                }}</code>
                            </span>
                        </div>

                        <ol class="signing-steps">
                            <li>
                                <span>1</span>
                                <p>
                                    On a trusted Mac, use
                                    <b>Xcode → Settings → Accounts → Manage Certificates</b> to
                                    create an <b>Apple Distribution</b> identity. Control-click it,
                                    export it as a password-protected <b>.p12</b>, and keep that
                                    password. If another Mac created the identity, export it from
                                    that Mac—the private key cannot be recovered from an Apple .cer
                                    file.
                                </p>
                            </li>
                            <li>
                                <span>2</span>
                                <p>
                                    Store and verify an Admin Team API key below. BuildBridge can
                                    create and retain a replacement <b>App Store</b> profile using
                                    the active certificate selected in the UI. Alternatively, create
                                    it in Apple Developer
                                    <b>Certificates, Identifiers &amp; Profiles → Profiles</b> and
                                    supply the downloaded <b>.mobileprovision</b> file here.
                                </p>
                            </li>
                            <li>
                                <span>3</span>
                                <p>
                                    Choose a new BuildBridge guest-keychain password. This is local
                                    protection for the imported identity; it is not your Apple
                                    Account password.
                                </p>
                            </li>
                        </ol>

                        <p class="signing-route-note">
                            Already have a <b>.certSigningRequest</b>? That file is only the public
                            request; its private key remains on the Mac that created it. If Apple
                            Developer already shows the new unexpired Distribution certificate, open
                            that record and download its <b>.cer</b>—do not create a duplicate.
                            Otherwise choose <b>Certificates + → Software → Apple Distribution</b>,
                            upload the request, and download the issued <b>.cer</b>. Open the
                            <b>.cer on the same Mac that created the request</b>; Keychain Access
                            can then pair it with the retained private key and export the complete
                            identity as a <b>.p12</b>.
                        </p>
                        <p class="signing-route-note">
                            Certificate-name note: <b>iPhone Distribution</b> or
                            <b>iOS Distribution</b> is Apple’s older iOS-specific label;
                            <b>Apple Distribution</b> is the newer unified label. BuildBridge
                            accepts either when it is unexpired and verifies that the exact
                            certificate is included in this app’s provisioning profile.
                        </p>

                        <div class="signing-resource-actions">
                            <button
                                class="secondary-button"
                                type="button"
                                @click="
                                    copySigningResourceLink(
                                        'Apple Developer certificates',
                                        APPLE_DEVELOPER_CERTIFICATES_URL,
                                    )
                                "
                            >
                                <Copy :size="14" /> Copy Certificates link
                            </button>
                            <button
                                class="secondary-button"
                                type="button"
                                @click="
                                    copySigningResourceLink(
                                        'Apple Developer profiles',
                                        APPLE_DEVELOPER_PROFILES_URL,
                                    )
                                "
                            >
                                <Copy :size="14" /> Copy Profiles link
                            </button>
                        </div>
                    </section>

                    <div class="vault-summary">
                        <div>
                            <ShieldCheck :size="16" />
                            <span>
                                <strong>Signing identity</strong>
                                <small>{{
                                    view?.secrets.signingCertificateName ?? 'Not configured'
                                }}</small>
                            </span>
                        </div>
                        <div>
                            <KeyRound :size="16" />
                            <span>
                                <strong>Profiles / keychain</strong>
                                <small>
                                    {{ view?.secrets.provisioningProfileCount ?? 0 }} profiles ·
                                    {{
                                        view?.secrets.guestKeychainConfigured
                                            ? 'vault ready'
                                            : 'no vault'
                                    }}
                                </small>
                            </span>
                        </div>
                    </div>

                    <details class="secret-details" open>
                        <summary>
                            <ShieldCheck :size="15" /> Signing certificate and profile
                        </summary>
                        <div class="secret-fields">
                            <label>
                                <span>Absolute .p12/.pfx path</span>
                                <input
                                    ref="signingCertificateInput"
                                    v-model.trim="signing.signingCertificatePath"
                                    placeholder="/secure/apple-distribution.p12"
                                />
                            </label>
                            <label>
                                <span>Password chosen when the .p12 was exported</span>
                                <input
                                    v-model="signing.signingCertificatePassword"
                                    type="password"
                                    autocomplete="new-password"
                                />
                            </label>
                            <label>
                                <span>Downloaded .mobileprovision paths, one per line</span>
                                <textarea
                                    v-model="signing.provisioningProfilePaths"
                                    rows="3"
                                    spellcheck="false"
                                    placeholder="/secure/app-store.mobileprovision"
                                />
                            </label>
                            <label>
                                <span>New dedicated BuildBridge guest-keychain password</span>
                                <input
                                    v-model="signing.guestKeychainPassword"
                                    type="password"
                                    maxlength="512"
                                    autocomplete="new-password"
                                />
                            </label>
                        </div>
                    </details>
                </template>

                <details
                    class="secret-details optional-key-details"
                    :open="!view?.secrets.appStoreConnectConfigured"
                >
                    <summary>
                        <UploadCloud :size="15" /> App Store Connect Team API key
                        <span class="optional-label">
                            {{
                                view?.secrets.appStoreConnectConfigured
                                    ? 'Stored · key ' + view.secrets.appStoreConnectKeyId
                                    : 'Needed for managed provisioning'
                            }}
                        </span>
                    </summary>
                    <div class="optional-key-intro">
                        <p>
                            This will link BuildBridge to the developer team without an Apple
                            password in the guest. In App Store Connect, open
                            <b>Users and Access → Integrations → App Store Connect API</b>, create a
                            Team Key with provisioning access, and download its .p8 file. Apple
                            allows the private key to be downloaded only once. After storing it,
                            BuildBridge verifies this project's Apple records, inventories
                            distribution certificates and profiles, and can create a replacement App
                            Store profile only after an explicit confirmation.
                        </p>
                        <button
                            class="secondary-button"
                            type="button"
                            @click="
                                copySigningResourceLink(
                                    'App Store Connect API keys',
                                    APP_STORE_CONNECT_KEYS_URL,
                                )
                            "
                        >
                            <Copy :size="14" /> Copy API keys link
                        </button>
                    </div>
                    <div class="secret-fields two-column">
                        <label>
                            <span>Key ID</span>
                            <input v-model.trim="signing.appStoreConnectKeyId" maxlength="512" />
                        </label>
                        <label>
                            <span>Issuer ID</span>
                            <input v-model.trim="signing.appStoreConnectIssuerId" maxlength="512" />
                        </label>
                        <label class="wide-field">
                            <span>Absolute private .p8 path on this host</span>
                            <input
                                v-model.trim="signing.appStoreConnectPrivateKeyPath"
                                spellcheck="false"
                                placeholder="/home/user/Downloads/AuthKey_ABC123DEFG.p8"
                                @change="inferAppStoreConnectKeyId"
                            />
                        </label>
                    </div>
                </details>

                <section
                    v-if="view?.secrets.appStoreConnectConfigured"
                    class="team-api-verification"
                    :class="{
                        ready:
                            appleTeamVerification?.appStoreRecordFound &&
                            appleTeamVerification?.bundleIdFound,
                        warning:
                            appleTeamVerification !== null &&
                            (!appleTeamVerification.appStoreRecordFound ||
                                !appleTeamVerification.bundleIdFound),
                    }"
                >
                    <header>
                        <span><ShieldCheck :size="16" /></span>
                        <div>
                            <strong>Developer team connection</strong>
                            <small v-if="appleTeamVerification">
                                Key {{ appleTeamVerification.keyId }} accepted by Apple · checked
                                this session
                            </small>
                            <small v-else>
                                Run a GET-only check; no certificate, profile, or bundle ID is
                                changed.
                            </small>
                        </div>
                        <span
                            v-if="appleTeamVerification"
                            class="state-badge"
                            :class="
                                appleTeamVerification.appStoreRecordFound &&
                                appleTeamVerification.bundleIdFound
                                    ? 'success'
                                    : 'warning'
                            "
                        >
                            <i />
                            {{
                                appleTeamVerification.appStoreRecordFound &&
                                appleTeamVerification.bundleIdFound
                                    ? 'Records match'
                                    : appleTeamVerification.appStoreRecordFound
                                      ? 'App found'
                                      : 'App mismatch'
                            }}
                        </span>
                    </header>

                    <div v-if="appleTeamVerification" class="team-api-result-grid">
                        <div>
                            <span>Project team</span>
                            <strong>{{ appleTeamVerification.projectDevelopmentTeam }}</strong>
                            <small v-if="appleTeamVerification.appIdPrefix">
                                App ID prefix {{ appleTeamVerification.appIdPrefix }}
                            </small>
                        </div>
                        <div>
                            <span>App Store Connect app</span>
                            <strong>
                                {{
                                    appleTeamVerification.appStoreRecordFound
                                        ? appleTeamVerification.appStoreAppName
                                        : 'No exact app match'
                                }}
                            </strong>
                            <small>
                                {{
                                    appleTeamVerification.appStoreAppId
                                        ? `Apple app ID ${appleTeamVerification.appStoreAppId}`
                                        : appleTeamVerification.bundleIdentifier
                                }}
                            </small>
                        </div>
                        <div>
                            <span>Developer provisioning ID</span>
                            <strong>{{ appleTeamVerification.bundleIdentifier }}</strong>
                            <small>
                                {{
                                    !appleTeamVerification.developerResourcesAccessible
                                        ? 'Provisioning resources are not accessible'
                                        : appleTeamVerification.bundleIdFound
                                          ? `${appleTeamVerification.bundleIdName} · ${appleTeamVerification.bundleIdPlatform}`
                                          : 'No exact provisioning identifier returned'
                                }}
                            </small>
                            <small v-if="appleTeamVerification.bundleLookupFallbackUsed">
                                Account inventory fallback used
                            </small>
                        </div>
                    </div>

                    <p v-if="!view.appleWorkspace?.bundleIdentifier" class="team-api-guidance">
                        Approve the Apple project first so BuildBridge can verify its exact bundle
                        identifier.
                    </p>
                    <p v-else-if="appleTeamVerification" class="team-api-guidance">
                        <template v-if="appleTeamVerification.developerResourcesIssue">
                            {{ appleTeamVerification.developerResourcesIssue }}
                        </template>
                        <template v-else-if="!appleTeamVerification.appStoreRecordFound">
                            The key works, but the approved project's bundle identifier does not
                            exactly match an app visible in this App Store Connect account. Check
                            the app's Bundle ID under My Apps → App Information before creating
                            anything.
                        </template>
                        <template v-else-if="!appleTeamVerification.bundleIdFound">
                            The App Store app exists, but Apple did not return its Developer
                            provisioning identifier. Confirm the Team key and account have access to
                            Certificates, Identifiers &amp; Profiles; do not create a duplicate ID.
                        </template>
                        <template v-else>
                            Both read-only checks pass. Apple does not expose this key's assigned
                            role in these responses. Before managed distribution-certificate
                            creation, confirm the Team key shows <b>Admin</b> access. A key showing
                            <b>Developer</b> should remain read-only here.
                        </template>
                    </p>

                    <section
                        v-if="appleTeamVerification?.bundleIdFound"
                        class="apple-profile-inventory"
                    >
                        <header>
                            <div>
                                <strong>Provisioning profiles</strong>
                                <small>Read-only inventory for this exact Bundle ID</small>
                            </div>
                            <span
                                class="state-badge"
                                :class="activeAppStoreProfiles.length > 0 ? 'success' : 'warning'"
                            >
                                <i />
                                {{
                                    activeAppStoreProfiles.length > 0
                                        ? `${activeAppStoreProfiles.length} active`
                                        : 'Replacement needed'
                                }}
                            </span>
                        </header>

                        <p v-if="appleTeamVerification.profilesIssue" class="team-api-guidance">
                            {{ appleTeamVerification.profilesIssue }}
                        </p>
                        <div v-else-if="visibleAppleProfiles.length > 0" class="apple-profile-list">
                            <article
                                v-for="profileItem in visibleAppleProfiles"
                                :key="profileItem.id"
                                :class="{ expired: appleProfileIsExpired(profileItem) }"
                            >
                                <div>
                                    <strong>{{ profileItem.name }}</strong>
                                    <small>
                                        {{ formatAppleProfileType(profileItem.profileType) }} ·
                                        {{ profileItem.platform }}
                                    </small>
                                </div>
                                <span>
                                    <b>{{ appleProfileStatusLabel(profileItem) }}</b>
                                    <small>
                                        {{
                                            profileItem.expirationDate
                                                ? `Expires ${formatAppleDate(profileItem.expirationDate)}`
                                                : 'Expiry unavailable'
                                        }}
                                    </small>
                                </span>
                            </article>
                            <small v-if="appleTeamVerification.profiles.length > 8">
                                Showing 8 of {{ appleTeamVerification.profiles.length }} profiles.
                            </small>
                        </div>
                        <p v-else class="team-api-guidance">
                            No profiles exist for this Bundle ID. Managed provisioning can create a
                            replacement after certificate inventory and explicit confirmation.
                        </p>

                        <section class="apple-certificate-inventory">
                            <header>
                                <div>
                                    <strong>Signing certificates</strong>
                                    <small>
                                        Read-only Apple inventory; importing a .p12 does not change
                                        these records
                                    </small>
                                </div>
                                <span
                                    class="state-badge"
                                    :class="
                                        usableDistributionCertificates.length > 0
                                            ? 'success'
                                            : 'warning'
                                    "
                                >
                                    <i />
                                    {{
                                        usableDistributionCertificates.length > 0
                                            ? `${usableDistributionCertificates.length} ready`
                                            : 'None usable'
                                    }}
                                </span>
                            </header>

                            <p
                                v-if="appleTeamVerification.certificatesIssue"
                                class="team-api-guidance"
                            >
                                {{ appleTeamVerification.certificatesIssue }}
                            </p>
                            <div
                                v-else-if="visibleAppleCertificates.length > 0"
                                class="apple-certificate-list"
                            >
                                <article
                                    v-for="certificate in visibleAppleCertificates"
                                    :key="certificate.id"
                                    :class="{ usable: appleCertificateIsUsable(certificate) }"
                                >
                                    <div>
                                        <strong>{{ appleCertificateLabel(certificate) }}</strong>
                                        <small>
                                            {{
                                                formatAppleCertificateType(
                                                    certificate.certificateType,
                                                )
                                            }}
                                            · {{ certificate.platform }} · serial
                                            {{ certificate.serialNumber || 'unavailable' }}
                                        </small>
                                    </div>
                                    <span>
                                        <b>{{ appleCertificateStatusLabel(certificate) }}</b>
                                        <small>
                                            {{
                                                certificate.expirationDate
                                                    ? `Expires ${formatAppleDate(certificate.expirationDate)}`
                                                    : 'Expiry unavailable'
                                            }}
                                        </small>
                                    </span>
                                </article>
                                <small v-if="appleTeamVerification.certificates.length > 8">
                                    Showing 8 of
                                    {{ appleTeamVerification.certificates.length }} certificates.
                                </small>
                            </div>
                            <p v-else class="team-api-guidance">
                                Apple returned no certificates for this developer team. Confirm the
                                distribution identity is registered under team
                                {{ appleTeamVerification.projectDevelopmentTeam }}.
                            </p>
                        </section>

                        <p
                            v-if="
                                appleTeamVerification.profilesAccessible &&
                                appleTeamVerification.profiles.length > 0 &&
                                activeAppStoreProfiles.length === 0
                            "
                            class="profile-replacement-guidance"
                        >
                            No active iOS App Store profile remains. Existing records will be kept;
                            BuildBridge should create a replacement rather than revoke them.
                        </p>

                        <section
                            v-if="
                                appleTeamVerification.profilesAccessible &&
                                activeAppStoreProfiles.length === 0
                            "
                            class="apple-profile-replacement"
                        >
                            <header>
                                <div>
                                    <strong>Create replacement profile</strong>
                                    <small>
                                        One explicit Apple mutation; existing profiles remain
                                        untouched
                                    </small>
                                </div>
                                <span class="state-badge warning"><i /> Confirmation required</span>
                            </header>

                            <p
                                v-if="appleTeamVerification.certificatesIssue"
                                class="team-api-guidance"
                            >
                                {{ appleTeamVerification.certificatesIssue }}
                            </p>
                            <template v-else-if="usableDistributionCertificates.length > 0">
                                <label class="apple-certificate-select">
                                    <span>Apple Distribution certificate</span>
                                    <select
                                        v-model="selectedAppleCertificateId"
                                        @change="confirmCreateAppleProfile = false"
                                    >
                                        <option
                                            v-for="certificate in usableDistributionCertificates"
                                            :key="certificate.id"
                                            :value="certificate.id"
                                        >
                                            {{ appleCertificateLabel(certificate) }} · expires
                                            {{ formatAppleDate(certificate.expirationDate) }}
                                        </option>
                                    </select>
                                </label>

                                <div
                                    v-if="selectedAppleCertificate"
                                    class="apple-certificate-summary"
                                >
                                    <div>
                                        <span>Selected identity</span>
                                        <strong>{{
                                            appleCertificateLabel(selectedAppleCertificate)
                                        }}</strong>
                                    </div>
                                    <div>
                                        <span>Serial number</span>
                                        <strong>{{
                                            selectedAppleCertificate.serialNumber || 'Unavailable'
                                        }}</strong>
                                    </div>
                                    <div>
                                        <span>Expires</span>
                                        <strong>{{
                                            formatAppleDate(selectedAppleCertificate.expirationDate)
                                        }}</strong>
                                    </div>
                                </div>

                                <p class="apple-certificate-warning">
                                    The `.p12` used for signing must contain this certificate and
                                    its private key. BuildBridge will verify that match before an
                                    archive; a profile alone cannot sign the app.
                                </p>
                                <p
                                    v-if="confirmCreateAppleProfile"
                                    class="apple-profile-confirmation"
                                >
                                    Confirm creating one new <b>iOS App Store</b> profile for
                                    <b>{{ appleTeamVerification.bundleIdentifier }}</b> using the
                                    selected certificate. I have confirmed the stored Team key has
                                    <b>Admin</b> access. No existing Apple resource will be deleted
                                    or revoked.
                                </p>
                                <button
                                    class="primary-button apple-profile-create-button"
                                    type="button"
                                    :disabled="!canCreateAppleProfile"
                                    @click="createAppleReplacementProfile"
                                >
                                    <LoaderCircle
                                        v-if="operation === 'apple-profile-create'"
                                        :size="15"
                                        class="spin"
                                    />
                                    <ShieldCheck v-else :size="15" />
                                    {{
                                        operation === 'apple-profile-create'
                                            ? 'Creating and retaining profile…'
                                            : confirmCreateAppleProfile
                                              ? 'Confirm profile creation'
                                              : 'Create replacement profile'
                                    }}
                                </button>
                            </template>
                            <div v-else class="apple-certificate-missing">
                                <p>
                                    Apple returned no unexpired Apple Distribution certificate.
                                    Review the certificate inventory above for its exact type and
                                    expiry. The imported .p12 supplies the private key locally but
                                    cannot alter Apple's certificate record.
                                </p>
                                <button
                                    class="secondary-button"
                                    type="button"
                                    @click="
                                        copySigningResourceLink(
                                            'Apple Developer certificates',
                                            APPLE_DEVELOPER_CERTIFICATES_URL,
                                        )
                                    "
                                >
                                    <Copy :size="14" /> Copy Certificates link
                                </button>
                            </div>

                            <p v-if="appleProfileCreationError" class="secret-action-error">
                                <CircleAlert :size="14" />
                                <span>{{ appleProfileCreationError }}</span>
                            </p>
                        </section>

                        <p v-if="createdAppleProfilePath" class="apple-profile-created">
                            <CheckCircle2 :size="14" />
                            <span>
                                Replacement retained at <code>{{ createdAppleProfilePath }}</code>
                                and added to the local signing kit.
                            </span>
                        </p>
                    </section>

                    <button
                        class="secondary-button team-api-verify-button"
                        type="button"
                        :disabled="!canVerifyAppleTeam"
                        @click="verifyAppleDeveloperTeam"
                    >
                        <LoaderCircle
                            v-if="operation === 'apple-team-verify'"
                            :size="15"
                            class="spin"
                        />
                        <ShieldCheck v-else :size="15" />
                        {{
                            operation === 'apple-team-verify'
                                ? 'Checking with Apple…'
                                : appleTeamVerification
                                  ? 'Verify again'
                                  : 'Verify developer team'
                        }}
                    </button>
                </section>

                <div class="secret-actions">
                    <button
                        class="secondary-button"
                        type="button"
                        :disabled="operation !== null || !hasSigningInput"
                        @click="saveSecrets"
                    >
                        <LoaderCircle v-if="operation === 'secrets'" :size="15" class="spin" />
                        <LockKeyhole v-else :size="15" /> Store in OS vault
                    </button>
                    <button
                        class="danger-link"
                        type="button"
                        :disabled="operation !== null"
                        @click="clearSecrets"
                    >
                        <Trash2 :size="14" />
                        {{ confirmClearSecrets ? 'Confirm removal' : 'Clear signing kit' }}
                    </button>
                </div>
                <p v-if="signingSecretError" class="secret-action-error">
                    <CircleAlert :size="14" />
                    <span>{{ signingSecretError }}</span>
                </p>

                <div
                    v-if="signingSetupMethod === 'files' || view?.signing"
                    class="signing-provision-panel"
                    :class="{ ready: view?.signing }"
                >
                    <header>
                        <span class="signing-provision-icon"><Fingerprint :size="16" /></span>
                        <div>
                            <strong>Guest signing state</strong>
                            <small v-if="view?.signing">
                                Identity, certificate, and profile match verified
                            </small>
                            <small v-else>
                                Available after the unsigned project build succeeds
                            </small>
                        </div>
                        <span class="state-badge" :class="view?.signing ? 'success' : 'neutral'">
                            <i />{{ view?.signing ? 'Provisioned' : 'Not provisioned' }}
                        </span>
                    </header>

                    <div v-if="view?.signing" class="signing-verification-grid">
                        <div>
                            <span>Identity</span>
                            <strong>{{ view.signing.identityName }}</strong>
                            <small>SHA-1 {{ view.signing.identitySha1.slice(0, 12) }}…</small>
                        </div>
                        <div>
                            <span>Project match</span>
                            <strong>{{ view.signing.bundleIdentifier }}</strong>
                            <small>Team {{ view.signing.developmentTeam }}</small>
                        </div>
                        <div>
                            <span>Certificate</span>
                            <strong>{{ view.signing.certificateExpiresAt }}</strong>
                            <small
                                >SHA-256 {{ view.signing.certificateSha256.slice(0, 12) }}…</small
                            >
                        </div>
                        <div>
                            <span>Profiles</span>
                            <strong>{{ view.signing.profiles.length }} installed</strong>
                            <small>{{ view.signing.profiles[0]?.applicationIdentifier }}</small>
                        </div>
                    </div>

                    <div v-else class="signing-readiness">
                        <span :class="{ complete: view?.secrets.signingCertificateConfigured }">
                            Certificate
                        </span>
                        <span
                            :class="{ complete: (view?.secrets.provisioningProfileCount ?? 0) > 0 }"
                        >
                            Profile
                        </span>
                        <span :class="{ complete: view?.appleWorkspace?.lastBuildSucceeded }">
                            Test build
                        </span>
                        <span
                            :class="{
                                complete:
                                    view?.appleWorkspace?.developmentTeam &&
                                    view?.appleWorkspace?.bundleIdentifier,
                            }"
                        >
                            Project IDs
                        </span>
                    </div>

                    <div v-if="signingProgress" class="signing-progress">
                        <div>
                            <strong>{{ signingPhaseLabel }}</strong>
                            <span>{{ formatElapsed(signingElapsedSeconds) }}</span>
                        </div>
                        <div
                            class="xcode-progress-track"
                            :class="{
                                indeterminate: signingProgress.totalBytes <= 0,
                                failed: signingProgress.phase === 'failed',
                            }"
                        >
                            <i :style="{ width: `${signingPercent}%` }" />
                        </div>
                        <p>{{ signingProgress.detail }}</p>
                    </div>

                    <section v-if="expiredSigningCertificateRecovery" class="signing-recovery">
                        <header>
                            <CircleAlert :size="15" />
                            <div>
                                <strong>Replace the expired identity file</strong>
                                <small>The new Apple profile is valid and remains stored.</small>
                            </div>
                        </header>
                        <ol>
                            <li>
                                A <b>.certSigningRequest</b> is not the certificate. In Apple
                                Developer, download the unexpired Distribution certificate already
                                issued from that request. If it has not been issued, choose
                                <b>Certificates + → Software → Apple Distribution</b>, upload the
                                request, and download the resulting <b>.cer</b>.
                            </li>
                            <li>
                                Open the downloaded
                                <b>.cer on the same trusted Mac that created the request</b>. Its
                                matching private key never left that Mac.
                            </li>
                            <li>
                                Open <b>Keychain Access → login → My Certificates</b>. Find the
                                unexpired <b>Apple Distribution</b> or
                                <b>iOS Distribution</b> identity matching the record marked
                                <b>Ready</b> above. Expand it and confirm a private key is directly
                                underneath.
                            </li>
                            <li>
                                Export only that identity as a password-protected <b>.p12</b>. Enter
                                its new path and export password above, then select
                                <b>Store in OS vault</b>. Existing profiles and API-key details are
                                preserved.
                            </li>
                        </ol>
                        <div class="signing-recovery-actions">
                            <button
                                class="secondary-button"
                                type="button"
                                @click="
                                    copySigningResourceLink(
                                        'Apple Developer certificates',
                                        APPLE_DEVELOPER_CERTIFICATES_URL,
                                    )
                                "
                            >
                                <Copy :size="14" /> Copy Certificates link
                            </button>
                            <button
                                class="secondary-button"
                                type="button"
                                @click="startReplacingExpiredSigningCertificate"
                            >
                                <KeyRound :size="14" /> Enter replacement .p12
                            </button>
                        </div>
                    </section>

                    <button
                        v-if="!view?.signing"
                        class="launch-button signing-provision-button"
                        type="button"
                        :disabled="!canProvisionSigning"
                        @click="provisionSigning"
                    >
                        <LoaderCircle
                            v-if="operation === 'signing-provision'"
                            :size="15"
                            class="spin"
                        />
                        <ShieldCheck v-else :size="15" />
                        {{
                            operation === 'signing-provision'
                                ? 'Provisioning signing…'
                                : 'Provision signing into macOS'
                        }}
                    </button>
                    <button
                        v-else
                        class="danger-link signing-clear-button"
                        type="button"
                        :disabled="operation !== null"
                        @click="clearGuestSigning"
                    >
                        <LoaderCircle
                            v-if="operation === 'signing-clear'"
                            :size="14"
                            class="spin"
                        />
                        <Trash2 v-else :size="14" />
                        {{
                            confirmClearGuestSigning
                                ? 'Confirm removal from macOS'
                                : 'Remove provisioned signing'
                        }}
                    </button>

                    <p class="signing-security-note">
                        Passwords move from the OS vault through protected SSH input to a fixed
                        native macOS helper. They never appear in process arguments or build logs.
                        Provisioning replaces BuildBridge’s dedicated guest keychain.
                    </p>
                </div>

                <section
                    v-if="view?.appleWorkspace?.lastBuildSucceeded || view?.archive"
                    class="signed-archive-panel"
                    :class="{ ready: view?.signing, complete: view?.archive }"
                >
                    <header>
                        <span class="signed-archive-icon"><Archive :size="17" /></span>
                        <div>
                            <strong>Signed App Store archive</strong>
                            <small
                                >Build a verified local Release archive and exportable IPA.</small
                            >
                        </div>
                        <span
                            class="state-badge"
                            :class="
                                view?.archive ? 'success' : view?.signing ? 'warning' : 'neutral'
                            "
                        >
                            <i />{{
                                view?.archive
                                    ? 'Artifacts ready'
                                    : view?.signing
                                      ? 'Ready to build'
                                      : 'Needs signing'
                            }}
                        </span>
                    </header>

                    <div class="signed-archive-recipe">
                        <div>
                            <span>Configuration</span>
                            <b>Release</b>
                        </div>
                        <div>
                            <span>Export</span>
                            <b>App Store Connect</b>
                        </div>
                        <div>
                            <span>Scheme</span>
                            <b>{{ view?.appleWorkspace?.scheme ?? view?.archive?.scheme }}</b>
                        </div>
                        <div>
                            <span>Bundle</span>
                            <b>{{
                                view?.signing?.bundleIdentifier ?? view?.archive?.bundleIdentifier
                            }}</b>
                        </div>
                        <div>
                            <span>Signing</span>
                            <b>App target only · locked export</b>
                        </div>
                        <div>
                            <span>Profile</span>
                            <b>{{
                                view?.signing?.profiles[0]?.uuid ??
                                view?.archive?.provisioningProfileUuid
                            }}</b>
                        </div>
                    </div>

                    <p v-if="!view?.signing" class="signed-archive-guidance">
                        Provision the verified certificate and App Store profile above to enable
                        this build. No Apple Account sign-in is needed.
                    </p>
                    <p
                        v-else-if="view.appleWorkspace?.lastNativeLockUpdated"
                        class="apple-lock-warning"
                    >
                        <CircleAlert :size="15" /> Apply the guest Podfile.lock update to the host
                        project and synchronize again before producing a signed Release.
                    </p>
                    <p v-else class="signed-archive-guidance">
                        BuildBridge lets the app target select the installed matching profile while
                        CocoaPods remain profile-free, then locks the verified identity and profile
                        during export. It verifies the archived app, exports one IPA, checksums both
                        artifacts, and retains them in its private host data directory. It will
                        <b>not upload anything to Apple</b>.
                    </p>

                    <button
                        class="launch-button signed-archive-build"
                        type="button"
                        :disabled="!canRunAppleArchive"
                        @click="runAppleSignedArchive"
                    >
                        <LoaderCircle
                            v-if="operation === 'archive-build'"
                            :size="15"
                            class="spin"
                        />
                        <Archive v-else :size="15" />
                        {{
                            operation === 'archive-build'
                                ? appleArchivePhaseLabel
                                : view?.archive
                                  ? 'Rebuild signed archive & IPA'
                                  : 'Build signed archive & IPA'
                        }}
                    </button>

                    <p
                        v-if="view?.archiveError && appleArchiveProgress?.phase !== 'failed'"
                        class="signed-archive-error"
                    >
                        <CircleAlert :size="14" />
                        <span>{{ view.archiveError }}</span>
                    </p>

                    <div v-if="appleArchiveProgress" class="apple-project-progress">
                        <div>
                            <strong>{{ appleArchivePhaseLabel }}</strong>
                            <span>
                                <template v-if="appleArchiveProgress.totalBytes > 0">
                                    {{ appleArchivePercent }}% ·
                                    {{ formatBytes(appleArchiveProgress.completedBytes) }} /
                                    {{ formatBytes(appleArchiveProgress.totalBytes) }} ·
                                </template>
                                {{ formatElapsed(appleArchiveElapsedSeconds) }}
                            </span>
                        </div>
                        <div
                            class="xcode-progress-track"
                            :class="{
                                indeterminate:
                                    appleArchiveProgress.totalBytes === 0 &&
                                    appleArchiveProgress.phase !== 'completed' &&
                                    appleArchiveProgress.phase !== 'failed',
                                failed: appleArchiveProgress.phase === 'failed',
                            }"
                        >
                            <i :style="{ width: appleArchivePercent + '%' }" />
                        </div>
                        <p>{{ appleArchiveProgress.detail }}</p>
                    </div>

                    <div v-if="appleArchiveOutput.length" class="apple-build-output">
                        <div><Terminal :size="14" /> Live signed archive output</div>
                        <pre>{{ appleArchiveOutput.join('\n') }}</pre>
                    </div>

                    <template v-if="view?.archive">
                        <div class="signed-archive-result">
                            <div class="signed-archive-version">
                                <span>Verified release</span>
                                <strong
                                    >{{ view.archive.bundleIdentifier }}
                                    {{ view.archive.marketingVersion }} ({{
                                        view.archive.buildNumber
                                    }})</strong
                                >
                                <small
                                    >{{ view.archive.configuration }} ·
                                    {{ view.archive.exportMethod }} · no upload performed</small
                                >
                            </div>
                            <article>
                                <Archive :size="15" />
                                <div>
                                    <strong>Signed IPA</strong>
                                    <small :title="view.archive.ipa.path">{{
                                        view.archive.ipa.path
                                    }}</small>
                                    <span :title="view.archive.ipa.sha256"
                                        >{{ formatBytes(view.archive.ipa.bytes) }} · SHA-256
                                        {{ view.archive.ipa.sha256.slice(0, 16) }}…</span
                                    >
                                </div>
                            </article>
                            <article>
                                <FolderOpen :size="15" />
                                <div>
                                    <strong>Portable Xcode archive</strong>
                                    <small :title="view.archive.archive.path">{{
                                        view.archive.archive.path
                                    }}</small>
                                    <span :title="view.archive.archive.sha256"
                                        >{{ formatBytes(view.archive.archive.bytes) }} · SHA-256
                                        {{ view.archive.archive.sha256.slice(0, 16) }}…</span
                                    >
                                </div>
                            </article>
                        </div>
                        <div class="signed-archive-actions">
                            <button
                                class="secondary-button"
                                type="button"
                                :disabled="operation !== null"
                                @click="revealAppleArchive"
                            >
                                <LoaderCircle
                                    v-if="operation === 'archive-reveal'"
                                    :size="14"
                                    class="spin"
                                />
                                <FolderOpen v-else :size="14" /> Reveal folder
                            </button>
                            <button
                                class="secondary-button"
                                type="button"
                                :disabled="operation !== null"
                                @click="copyAppleArtifactPath(view.archive.ipa.path, 'IPA')"
                            >
                                <Copy :size="14" /> Copy IPA path
                            </button>
                            <button
                                class="secondary-button"
                                type="button"
                                :disabled="operation !== null"
                                @click="copyAppleArtifactPath(view.archive.archive.path, 'archive')"
                            >
                                <Copy :size="14" /> Copy archive path
                            </button>
                            <button
                                class="danger-link"
                                type="button"
                                :disabled="operation !== null"
                                @click="clearAppleArchive"
                            >
                                <LoaderCircle
                                    v-if="operation === 'archive-clear'"
                                    :size="14"
                                    class="spin"
                                />
                                <Trash2 v-else :size="14" />
                                {{
                                    confirmClearAppleArchive
                                        ? 'Confirm artifact removal'
                                        : 'Clear artifacts'
                                }}
                            </button>
                        </div>
                    </template>
                </section>

                <p class="apple-note">
                    Apple ID passwords and 2FA are never collected here. Docker-OSX builds use
                    imported Xcode packages, certificates, profiles, and App Store Connect keys.
                </p>
            </section>

            <section class="builder-card runtime-card">
                <header class="card-header runtime-header">
                    <span class="step-number green">03</span>
                    <div>
                        <h2>Guest bootstrap</h2>
                        <p>Pin the macOS identity, authenticate, then verify Xcode.</p>
                    </div>
                    <span v-if="view?.runtime.containerId" class="container-id">
                        {{ view.runtime.containerId.slice(0, 12) }}
                    </span>
                </header>

                <div class="bootstrap-flow">
                    <article :class="{ complete: isRunning }">
                        <span>1</span>
                        <div>
                            <strong>Docker-OSX running</strong><small>Managed host container</small>
                        </div>
                    </article>
                    <i />
                    <article :class="{ complete: view?.guest.ssh.reachable }">
                        <span>2</span>
                        <div>
                            <strong>Remote Login reachable</strong
                            ><small>Forwarded guest SSH port</small>
                        </div>
                    </article>
                    <i />
                    <article :class="{ complete: view?.guest.diagnostics.authenticated }">
                        <span>3</span>
                        <div>
                            <strong>Guest bridge verified</strong
                            ><small>Key authentication and Xcode probe</small>
                        </div>
                    </article>
                </div>

                <div
                    v-if="isRunning"
                    class="installer-monitor"
                    :class="{ ready: installerPhase.ready }"
                >
                    <LoaderCircle
                        v-if="!installerPhase.ready"
                        :size="17"
                        class="spin installer-monitor-icon"
                    />
                    <CheckCircle2 v-else :size="17" class="installer-monitor-icon" />
                    <div>
                        <strong>{{ installerPhase.title }}</strong>
                        <small>{{ installerPhase.detail }}</small>
                    </div>
                    <span v-if="builderUptime" class="installer-elapsed">
                        <Clock3 :size="13" /> {{ builderUptime }}
                    </span>
                    <div v-if="!installerPhase.ready" class="installer-monitor-track">
                        <i />
                    </div>
                    <p v-if="!installerPhase.ready">
                        The exact installer percentage remains in the macOS console. BuildBridge
                        checks for a real SSH host identity every 10 seconds and advances
                        automatically when Remote Login is ready.
                    </p>
                </div>

                <details class="installer-guide" :open="isRunning && !view?.guest.ssh.reachable">
                    <summary>
                        <CircleAlert :size="16" />
                        <span>
                            <strong>First-time macOS console setup</strong>
                            <small>Required before BuildBridge can connect to the guest</small>
                        </span>
                        <b>{{ view?.guest.ssh.reachable ? 'Completed' : 'Interactive step' }}</b>
                    </summary>
                    <div class="installer-steps">
                        <article>
                            <span>1</span>
                            <div>
                                <strong>Boot macOS Base System</strong>
                                <p>
                                    In the OpenCore picker, use the arrow keys to select
                                    <b>macOS Base System</b> and press Enter. Do not choose EFI,
                                    UEFI Shell, or Reset NVRAM.
                                </p>
                            </div>
                        </article>
                        <article>
                            <span>2</span>
                            <div>
                                <strong>Prepare the virtual disk</strong>
                                <p>
                                    Open Disk Utility and select the largest top-level
                                    <b>QEMU HARDDISK Media</b>. It should be uninitialized and show
                                    device <b>disk0</b>—about 274.88 GB with the current image.
                                    Erase it as <b>Macintosh HD</b>, APFS, with a GUID Partition
                                    Map. Do not erase the smaller QEMU disk or <b>disk2s1</b>.
                                </p>
                            </div>
                        </article>
                        <article>
                            <span>3</span>
                            <div>
                                <strong>Install and finish macOS</strong>
                                <p>
                                    Close Disk Utility, choose Reinstall macOS, and allow its
                                    reboots. Continue with the installer or installed macOS volume
                                    when it appears, then finish the macOS account setup.
                                </p>
                            </div>
                        </article>
                        <article>
                            <span>4</span>
                            <div>
                                <strong>Enable the BuildBridge connection</strong>
                                <p>
                                    Enable General → Sharing → Remote Login. Return here to
                                    configure the username, access key, and guest fingerprint, then
                                    import Xcode from the host.
                                </p>
                            </div>
                        </article>
                    </div>
                    <p class="console-warning">
                        <CircleAlert :size="14" /> The initial installation is interactive. Use
                        BuildBridge’s tray menu or <b>Stop safely</b> to stop the machine; closing
                        the QEMU console is not the lifecycle control. If the console is too large,
                        use QEMU’s View → Zoom Out or press <b>Ctrl+Alt+-</b>.
                    </p>
                </details>

                <div class="guest-bridge-grid">
                    <section class="guest-access-panel">
                        <div class="guest-panel-title">
                            <KeyRound :size="15" />
                            <div>
                                <strong>BuildBridge access key</strong>
                                <small>Dedicated to this macOS guest; no password is stored.</small>
                            </div>
                        </div>

                        <form class="guest-user-form" @submit.prevent="saveGuestAccess">
                            <label>
                                <span>macOS short username</span>
                                <input
                                    v-model.trim="guestUsername"
                                    required
                                    maxlength="32"
                                    placeholder="builder"
                                />
                            </label>
                            <button
                                class="secondary-button"
                                type="submit"
                                :disabled="operation !== null"
                            >
                                <LoaderCircle
                                    v-if="operation === 'guest-config'"
                                    :size="15"
                                    class="spin"
                                />
                                <KeyRound v-else :size="15" />
                                {{
                                    view?.guest.publicKey ? 'Save username' : 'Generate access key'
                                }}
                            </button>
                        </form>

                        <div v-if="view?.guest.publicKey" class="public-key-box">
                            <code>{{ view.guest.publicKey }}</code>
                            <button
                                type="button"
                                title="Copy public key"
                                @click="copyGuestPublicKey"
                            >
                                <Copy :size="14" />
                            </button>
                        </div>
                        <ol class="guest-instructions">
                            <li>In macOS, enable General → Sharing → Remote Login.</li>
                            <li>
                                Add this public key to the guest user’s
                                <code>~/.ssh/authorized_keys</code>.
                            </li>
                            <li>Refresh, verify the displayed fingerprint, then trust it.</li>
                        </ol>
                    </section>

                    <section class="guest-status-panel">
                        <div class="guest-panel-title">
                            <Fingerprint :size="15" />
                            <div>
                                <strong>Guest identity</strong>
                                <small>{{ guestTrustLabel }}</small>
                            </div>
                            <span
                                class="state-badge"
                                :class="{
                                    success: view?.guest.ssh.trust === 'trusted',
                                    danger: view?.guest.ssh.trust === 'mismatch',
                                    warning: view?.guest.ssh.trust === 'untrusted',
                                }"
                            >
                                <i />{{ view?.guest.ssh.trust ?? 'unavailable' }}
                            </span>
                        </div>

                        <div class="guest-status-list">
                            <div :class="{ ready: view?.guest.ssh.reachable }">
                                <Wifi :size="14" />
                                <span>
                                    <strong>SSH transport</strong>
                                    <small>
                                        {{
                                            view?.guest.ssh.reachable
                                                ? 'SSH identity detected'
                                                : view?.guest.ssh.portOpen
                                                  ? 'Forward open · guest not ready'
                                                  : `127.0.0.1:${profile.sshPort}`
                                        }}
                                    </small>
                                </span>
                            </div>
                            <div :class="{ ready: view?.guest.ssh.trust === 'trusted' }">
                                <Fingerprint :size="14" />
                                <span>
                                    <strong>Host fingerprint</strong>
                                    <small>{{
                                        view?.guest.ssh.fingerprint ?? 'Not available'
                                    }}</small>
                                </span>
                            </div>
                            <div :class="{ ready: view?.guest.diagnostics.authenticated }">
                                <ShieldCheck :size="14" />
                                <span>
                                    <strong>Key authentication</strong>
                                    <small>
                                        {{
                                            view?.guest.diagnostics.authenticated
                                                ? 'macOS ' + view.guest.diagnostics.macosVersion
                                                : 'Waiting for authorized_keys'
                                        }}
                                    </small>
                                </span>
                            </div>
                            <div :class="{ ready: view?.guest.diagnostics.xcodeSelected }">
                                <Apple :size="14" />
                                <span>
                                    <strong>Xcode toolchain</strong>
                                    <small>
                                        {{
                                            view?.guest.diagnostics.xcodeVersion
                                                ? view.guest.diagnostics.xcodeSelected
                                                    ? view.guest.diagnostics.xcodeVersion
                                                    : 'Installed · activation required'
                                                : 'Not detected'
                                        }}
                                    </small>
                                </span>
                            </div>
                        </div>

                        <p v-if="view?.guest.ssh.issue" class="guest-issue">
                            <CircleAlert :size="14" /> {{ view.guest.ssh.issue }}
                        </p>
                        <p
                            v-else-if="view?.guest.diagnostics.issue"
                            class="guest-issue"
                            :class="{ warning: view.guest.diagnostics.authenticated }"
                        >
                            <CircleAlert :size="14" /> {{ view.guest.diagnostics.issue }}
                        </p>

                        <div class="guest-trust-actions">
                            <button
                                v-if="view?.guest.ssh.trust === 'untrusted'"
                                class="launch-button"
                                type="button"
                                :disabled="operation !== null"
                                @click="trustGuest"
                            >
                                <LoaderCircle
                                    v-if="operation === 'guest-trust'"
                                    :size="15"
                                    class="spin"
                                />
                                <Fingerprint v-else :size="15" /> Trust this fingerprint
                            </button>
                            <button
                                v-if="
                                    view?.guest.ssh.trust === 'trusted' ||
                                    view?.guest.ssh.trust === 'mismatch'
                                "
                                class="danger-link"
                                type="button"
                                :disabled="operation !== null"
                                @click="forgetGuestTrust"
                            >
                                <Trash2 :size="14" />
                                {{
                                    confirmForgetGuest
                                        ? 'Confirm forgotten identity'
                                        : 'Forget identity pin'
                                }}
                            </button>
                        </div>
                    </section>
                </div>

                <section
                    class="xcode-import-panel"
                    :class="{
                        ready: view?.guest.diagnostics.xcodeSelected,
                        dragging: xcodeDropActive,
                    }"
                >
                    <header>
                        <span class="xcode-import-icon"><Archive :size="17" /></span>
                        <div>
                            <strong>Host-managed Xcode</strong>
                            <small>
                                {{
                                    view?.guest.diagnostics.xcodeSelected
                                        ? 'Selected and ready for typed builds'
                                        : 'Import Apple’s signed Universal .xip without App Store login'
                                }}
                            </small>
                        </div>
                        <span
                            class="state-badge"
                            :class="{ success: view?.guest.diagnostics.xcodeSelected }"
                        >
                            <i />
                            {{ view?.guest.diagnostics.xcodeSelected ? 'ready' : 'setup required' }}
                        </span>
                    </header>

                    <div v-if="view?.guest.diagnostics.xcodeSelected" class="xcode-ready-summary">
                        <CheckCircle2 :size="16" />
                        <span>
                            <strong>{{ view.guest.diagnostics.xcodeVersion }}</strong>
                            <small>{{ view.guest.diagnostics.xcodePath }}</small>
                        </span>
                    </div>

                    <div v-if="view?.guest.diagnostics.xcodeSelected" class="xcode-first-open-note">
                        <CircleAlert :size="15" />
                        <div>
                            <strong
                                >First Xcode window may still show Apple’s component chooser</strong
                            >
                            <p>
                                BuildBridge installs required first-launch tools during activation
                                and automatically installs iOS when the first project build needs
                                it. If Xcode shows the chooser once, keep iOS selected, leave
                                unneeded platforms unchecked, and choose
                                <b>Download &amp; Install</b>. The completed state persists with
                                this builder and its future local template.
                            </p>
                        </div>
                    </div>

                    <div v-else-if="xcodeActivationCommands.length" class="xcode-activation">
                        <div>
                            <CheckCircle2 :size="16" />
                            <span>
                                <strong>Xcode is expanded</strong>
                                <small>Activate it here, then finish in macOS Terminal.</small>
                            </span>
                        </div>
                        <p class="xcode-authorization-note">
                            <LockKeyhole :size="14" /> The administrator password stays inside macOS
                            Terminal and is never sent to BuildBridge. This selects Xcode, accepts
                            its license, and installs required components.
                        </p>
                        <button
                            class="launch-button xcode-activate-button"
                            type="button"
                            :disabled="operation !== null"
                            @click="activateXcode"
                        >
                            <LoaderCircle
                                v-if="operation === 'xcode-activate'"
                                :size="15"
                                class="spin"
                            />
                            <ShieldCheck v-else :size="15" />
                            {{
                                operation === 'xcode-activate'
                                    ? 'Waiting for macOS…'
                                    : 'Activate Xcode'
                            }}
                        </button>
                        <details class="xcode-manual-fallback">
                            <summary>Manual recovery commands</summary>
                            <pre>{{ xcodeActivationCommands.join('\n') }}</pre>
                            <button
                                class="secondary-button"
                                type="button"
                                :disabled="operation !== null"
                                @click="copyXcodeActivationCommands"
                            >
                                <Copy :size="14" /> Copy commands
                            </button>
                        </details>
                    </div>

                    <template v-else>
                        <div class="xcode-package-input">
                            <label>
                                <span>Downloaded Universal .xip path</span>
                                <input
                                    v-model.trim="xcodePackagePath"
                                    placeholder="/home/matt/Downloads/Xcode_26.6_Universal.xip"
                                    spellcheck="false"
                                />
                            </label>
                            <button
                                class="launch-button"
                                type="button"
                                :disabled="!canImportXcode"
                                @click="importXcodePackage"
                            >
                                <LoaderCircle
                                    v-if="operation === 'xcode-import'"
                                    :size="15"
                                    class="spin"
                                />
                                <UploadCloud v-else :size="15" />
                                {{
                                    operation === 'xcode-import'
                                        ? xcodeImportPhaseLabel
                                        : 'Import and expand'
                                }}
                            </button>
                        </div>
                        <p class="xcode-drop-hint">
                            <UploadCloud :size="14" /> Drop a completed <b>.xip</b> anywhere on this
                            window, or paste its absolute host path. The archive is validated before
                            transfer.
                        </p>
                    </template>

                    <div v-if="xcodeImportProgress" class="xcode-import-progress">
                        <div>
                            <strong>{{ xcodeImportPhaseLabel }}</strong>
                            <span>
                                <template v-if="xcodeImportProgress.totalBytes > 0">
                                    {{ formatBytes(xcodeImportProgress.transferredBytes) }} /
                                    {{ formatBytes(xcodeImportProgress.totalBytes) }} ·
                                </template>
                                {{ formatElapsed(xcodeImportProgress.elapsedSeconds) }}
                            </span>
                        </div>
                        <div
                            class="xcode-progress-track"
                            :class="{
                                indeterminate:
                                    xcodeImportProgress.phase === 'expanding' ||
                                    xcodeImportProgress.phase === 'awaiting_authorization',
                                failed:
                                    xcodeImportProgress.phase === 'failed' ||
                                    xcodeImportProgress.phase === 'activation_failed',
                            }"
                        >
                            <i :style="{ width: `${xcodeImportPercent}%` }" />
                        </div>
                        <p>{{ xcodeImportProgress.detail }}</p>
                    </div>
                </section>

                <section
                    class="apple-project-panel"
                    :class="{ ready: view?.appleWorkspace?.lastBuildSucceeded }"
                >
                    <header>
                        <span class="apple-project-icon"><FolderOpen :size="17" /></span>
                        <div>
                            <strong>First Apple build</strong>
                            <small>Approve, synchronize, and compile a real local project.</small>
                        </div>
                        <span
                            class="state-badge"
                            :class="{ success: view?.appleWorkspace?.lastBuildSucceeded }"
                        >
                            <i />
                            {{
                                view?.appleWorkspace?.lastBuildSucceeded
                                    ? 'build ready'
                                    : 'guided setup'
                            }}
                        </span>
                    </header>

                    <div class="apple-workflow-steps">
                        <article :class="{ ready: view?.appleWorkspace }">
                            <b>01</b>
                            <span>
                                <strong>Approve project</strong>
                                <small>Choose the one host folder BuildBridge may read.</small>
                            </span>
                        </article>
                        <i />
                        <article :class="{ ready: view?.appleWorkspace?.lastSnapshotSha256 }">
                            <b>02</b>
                            <span>
                                <strong>Synchronize source</strong>
                                <small>Secrets and local dependencies are excluded.</small>
                            </span>
                        </article>
                        <i />
                        <article :class="{ ready: view?.appleWorkspace?.lastBuildSucceeded }">
                            <b>03</b>
                            <span>
                                <strong>Run test build</strong>
                                <small>Prepare tools and compile without signing.</small>
                            </span>
                        </article>
                    </div>

                    <div class="apple-project-input">
                        <label>
                            <span>Local project directory</span>
                            <input
                                v-model.trim="appleWorkspacePath"
                                placeholder="/absolute/path/to/capacitor-project"
                                spellcheck="false"
                            />
                        </label>
                        <button
                            class="secondary-button"
                            type="button"
                            :disabled="!canApproveAppleWorkspace"
                            @click="approveAppleWorkspace"
                        >
                            <LoaderCircle
                                v-if="operation === 'workspace-approve'"
                                :size="15"
                                class="spin"
                            />
                            <FolderOpen v-else :size="15" />
                            {{ view?.appleWorkspace ? 'Approve path' : 'Approve project' }}
                        </button>
                    </div>
                    <p class="apple-project-hint">
                        Drop a project folder anywhere on this window or paste its path. Approval
                        never modifies the host project.
                    </p>

                    <div v-if="view?.appleWorkspace" class="apple-project-summary">
                        <div>
                            <strong>{{ view.appleWorkspace.name }}</strong>
                            <small>{{ view.appleWorkspace.localPath }}</small>
                        </div>
                        <div>
                            <span>Workspace</span>
                            <b>{{ view.appleWorkspace.iosWorkspace }}</b>
                        </div>
                        <div>
                            <span>Scheme</span>
                            <b>{{ view.appleWorkspace.scheme }}</b>
                        </div>
                        <div>
                            <span>Team</span>
                            <b>{{ view.appleWorkspace.developmentTeam ?? 'Not detected' }}</b>
                        </div>
                        <div>
                            <span>Release bundle</span>
                            <b>{{ view.appleWorkspace.bundleIdentifier ?? 'Not detected' }}</b>
                        </div>
                        <div v-if="view.appleWorkspace.lastSnapshotSha256">
                            <span>Snapshot</span>
                            <b>{{ view.appleWorkspace.lastSnapshotSha256.slice(0, 12) }}…</b>
                        </div>
                    </div>

                    <div v-if="view?.appleWorkspace" class="apple-project-actions">
                        <button
                            class="secondary-button"
                            type="button"
                            :disabled="!canSyncAppleWorkspace"
                            @click="syncAppleWorkspace"
                        >
                            <LoaderCircle
                                v-if="operation === 'workspace-sync'"
                                :size="15"
                                class="spin"
                            />
                            <UploadCloud v-else :size="15" />
                            {{
                                operation === 'workspace-sync'
                                    ? appleProjectPhaseLabel
                                    : 'Sync source'
                            }}
                        </button>
                        <button
                            class="launch-button"
                            type="button"
                            :disabled="!canRunAppleSmokeBuild"
                            @click="runAppleSmokeBuild"
                        >
                            <LoaderCircle
                                v-if="operation === 'workspace-build'"
                                :size="15"
                                class="spin"
                            />
                            <Hammer v-else :size="15" />
                            {{
                                operation === 'workspace-build'
                                    ? appleProjectPhaseLabel
                                    : view.appleWorkspace.lastBuildSucceeded
                                      ? 'Re-run test build'
                                      : 'Run unsigned test build'
                            }}
                        </button>
                        <button
                            class="danger-link"
                            type="button"
                            :disabled="operation !== null"
                            @click="clearAppleWorkspace"
                        >
                            <Trash2 :size="14" /> Remove approval
                        </button>
                    </div>

                    <p
                        v-if="view?.appleWorkspace && !view.appleWorkspace.lastSnapshotSha256"
                        class="apple-next-step"
                    >
                        Next: select <b>Sync source</b>. BuildBridge creates a bounded snapshot and
                        excludes <code>node_modules</code>, Git data, environment files, and signing
                        credentials.
                    </p>
                    <p
                        v-else-if="view?.appleWorkspace && !view.appleWorkspace.lastBuildSucceeded"
                        class="apple-next-step"
                    >
                        Next: run the unsigned test build. The first run downloads pinned Node,
                        pnpm, CocoaPods, locked project dependencies, and—when missing—Apple’s iOS
                        Simulator platform inside macOS. The platform is several GiB but persists
                        across later builds.
                    </p>
                    <p
                        v-else-if="view?.appleWorkspace?.lastBuildSucceeded"
                        class="apple-build-ready"
                    >
                        <CheckCircle2 :size="15" /> Real project build verified with
                        {{ view.appleWorkspace.lastXcodeVersion }}.
                        {{
                            view?.signing
                                ? 'The signed Release archive is ready above.'
                                : 'Signing provisioning is the next stage.'
                        }}
                    </p>
                    <p
                        v-if="
                            view?.appleWorkspace?.lastBuildSucceeded &&
                            view.appleWorkspace.lastNativeLockUpdated
                        "
                        class="apple-lock-warning"
                    >
                        <CircleAlert :size="15" /> The native dependency lock changed only inside
                        the guest snapshot. The host project is untouched. Review and apply that
                        lock through a future BuildBridge action before enabling strict signed
                        release builds.
                    </p>

                    <div v-if="appleProjectProgress" class="apple-project-progress">
                        <div>
                            <strong>{{ appleProjectPhaseLabel }}</strong>
                            <span>
                                <template v-if="appleProjectProgress.totalBytes > 0">
                                    {{ appleProjectPercent }}% ·
                                    {{ formatBytes(appleProjectProgress.completedBytes) }} /
                                    {{ formatBytes(appleProjectProgress.totalBytes) }} ·
                                </template>
                                {{ formatElapsed(appleProjectElapsedSeconds) }}
                            </span>
                        </div>
                        <div
                            class="xcode-progress-track"
                            :class="{
                                indeterminate:
                                    appleProjectProgress.totalBytes === 0 &&
                                    appleProjectProgress.phase !== 'completed' &&
                                    appleProjectProgress.phase !== 'failed',
                                failed: appleProjectProgress.phase === 'failed',
                            }"
                        >
                            <i :style="{ width: `${appleProjectPercent}%` }" />
                        </div>
                        <p>{{ appleProjectProgress.detail }}</p>
                    </div>

                    <div v-if="appleBuildOutput.length" class="apple-build-output">
                        <div><Terminal :size="14" /> Live project build output</div>
                        <pre>{{ appleBuildOutput.join('\n') }}</pre>
                    </div>
                </section>

                <div class="log-panel">
                    <div><Terminal :size="14" /> Recent Docker-OSX output</div>
                    <pre v-if="view?.logs.length">{{ view.logs.join('\n') }}</pre>
                    <p v-else>Container output will appear here after the first launch.</p>
                </div>
            </section>
        </div>
    </main>
</template>

<style scoped>
.builder-content {
    width: min(100%, 1180px);
    margin: 0 auto;
    padding: 30px 28px 36px;
}

.builder-hero,
.builder-title-row,
.card-header,
.profile-actions,
.secret-actions,
.runtime-header,
.builder-banner,
.persistence-note,
.secret-details summary,
.log-panel > div,
.guest-panel-title,
.guest-issue,
.installer-guide summary,
.console-warning {
    display: flex;
    align-items: center;
}

.builder-hero {
    justify-content: space-between;
    gap: 24px;
    margin-bottom: 20px;
}

.builder-eyebrow {
    margin: 0 0 7px;
    color: #5eead4;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.18em;
    text-transform: uppercase;
}

.builder-title-row {
    gap: 12px;
}

.builder-hero h1 {
    margin: 0;
    color: white;
    font-size: clamp(28px, 4vw, 40px);
    letter-spacing: -0.035em;
}

.builder-hero > div > p:last-child {
    max-width: 720px;
    margin: 9px 0 0;
    color: #94a3b8;
    font-size: 13px;
    line-height: 1.65;
}

.state-badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 9px;
    color: #94a3b8;
    font-size: 9px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    border: 1px solid rgb(255 255 255 / 0.08);
    border-radius: 999px;
    background: rgb(255 255 255 / 0.03);
}

.state-badge i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentcolor;
}

.state-badge.success {
    color: #6ee7b7;
    border-color: rgb(52 211 153 / 0.2);
    background: rgb(52 211 153 / 0.07);
}

.state-badge.warning {
    color: #fcd34d;
}

.state-badge.danger {
    color: #fda4af;
}

.icon-button {
    display: grid;
    width: 40px;
    height: 40px;
    flex: 0 0 auto;
    place-items: center;
    color: #94a3b8;
    border: 1px solid rgb(255 255 255 / 0.08);
    border-radius: 11px;
    background: rgb(255 255 255 / 0.025);
}

.builder-banner {
    gap: 9px;
    margin-bottom: 14px;
    padding: 11px 13px;
    font-size: 12px;
    line-height: 1.5;
    border-radius: 10px;
}

.builder-banner.error {
    color: #fecdd3;
    border: 1px solid rgb(251 113 133 / 0.18);
    background: rgb(244 63 94 / 0.08);
}

.builder-banner.notice {
    color: #a7f3d0;
    border: 1px solid rgb(52 211 153 / 0.18);
    background: rgb(52 211 153 / 0.07);
}

.builder-grid {
    display: grid;
    grid-template-columns: minmax(0, 1.15fr) minmax(340px, 0.85fr);
    gap: 16px;
}

.configuration-card,
.signing-card {
    grid-column: 1 / -1;
}

.builder-card {
    padding: 21px;
    border: 1px solid rgb(255 255 255 / 0.075);
    border-radius: 17px;
    background: rgb(12 25 43 / 0.74);
    box-shadow: 0 18px 45px rgb(0 0 0 / 0.13);
    backdrop-filter: blur(14px);
}

.runtime-card {
    grid-column: 1 / -1;
}

.card-header {
    gap: 11px;
    margin-bottom: 18px;
}

.card-header > div {
    min-width: 0;
}

.card-header h2 {
    margin: 0 0 3px;
    color: white;
    font-size: 15px;
}

.card-header p {
    margin: 0;
    color: #64748b;
    font-size: 11px;
}

.step-number {
    display: grid;
    width: 36px;
    height: 36px;
    flex: 0 0 auto;
    place-items: center;
    color: #5eead4;
    font-family: ui-monospace, monospace;
    font-size: 10px;
    font-weight: 700;
    border-radius: 10px;
    background: rgb(45 212 191 / 0.09);
}

.step-number.blue {
    color: #7dd3fc;
    background: rgb(56 189 248 / 0.08);
}

.step-number.green {
    color: #86efac;
    background: rgb(34 197 94 / 0.08);
}

.check-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
}

.check-item {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 9px;
    min-width: 0;
    padding: 10px;
    color: #fb7185;
    border: 1px solid rgb(251 113 133 / 0.1);
    border-radius: 10px;
    background: rgb(244 63 94 / 0.035);
}

.check-item.ready {
    color: #5eead4;
    border-color: rgb(94 234 212 / 0.1);
    background: rgb(45 212 191 / 0.035);
}

.check-item strong,
.check-item small {
    display: block;
}

.check-item strong {
    color: #cbd5e1;
    font-size: 10px;
}

.check-item small {
    margin-top: 2px;
    overflow: hidden;
    color: #64748b;
    font-size: 9px;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.check-result {
    opacity: 0.8;
}

.issue-list {
    display: grid;
    gap: 4px;
    margin: 12px 0 0;
    padding: 10px 10px 10px 27px;
    color: #fda4af;
    font-size: 10px;
    line-height: 1.45;
    border-radius: 9px;
    background: rgb(244 63 94 / 0.05);
}

.profile-form,
.secret-fields {
    display: grid;
    gap: 12px;
}

.profile-form {
    grid-template-columns: 1fr 1fr;
    margin-top: 17px;
}

.secret-fields.two-column {
    grid-template-columns: 1fr 1fr;
}

.wide-field {
    grid-column: 1 / -1;
}

label {
    display: grid;
    gap: 6px;
}

label span {
    color: #94a3b8;
    font-size: 10px;
    font-weight: 600;
}

input,
select,
textarea {
    width: 100%;
    padding: 9px 11px;
    color: #e2e8f0;
    font: inherit;
    font-size: 12px;
    outline: none;
    border: 1px solid rgb(255 255 255 / 0.09);
    border-radius: 8px;
    background: rgb(2 8 20 / 0.55);
}

textarea {
    resize: vertical;
    font-family: ui-monospace, monospace;
    font-size: 10px;
    line-height: 1.5;
}

input:focus,
select:focus,
textarea:focus {
    border-color: rgb(94 234 212 / 0.45);
    box-shadow: 0 0 0 3px rgb(45 212 191 / 0.08);
}

.input-suffix {
    position: relative;
}

.input-suffix input {
    padding-right: 42px;
}

.input-suffix b {
    position: absolute;
    top: 50%;
    right: 11px;
    color: #64748b;
    font-size: 9px;
    transform: translateY(-50%);
}

.profile-actions,
.secret-actions {
    gap: 8px;
}

.secondary-button,
.launch-button,
.stop-button,
.danger-link {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    padding: 9px 12px;
    font-size: 11px;
    font-weight: 700;
    border-radius: 8px;
}

.secondary-button {
    color: #cbd5e1;
    border: 1px solid rgb(255 255 255 / 0.09);
    background: rgb(255 255 255 / 0.035);
}

.launch-button {
    flex: 1;
    color: #042f2e;
    border: 1px solid #5eead4;
    background: #5eead4;
}

.stop-button {
    flex: 1;
    color: #fecdd3;
    border: 1px solid rgb(251 113 133 / 0.2);
    background: rgb(244 63 94 / 0.08);
}

.persistence-note,
.apple-note {
    gap: 7px;
    margin: 14px 0 0;
    color: #64748b;
    font-size: 9px;
    line-height: 1.5;
}

.vault-summary {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 7px;
    margin-top: 11px;
}

.vault-summary > div {
    display: grid;
    grid-template-columns: auto 1fr;
    align-items: center;
    gap: 9px;
    padding: 9px 10px;
    color: #7dd3fc;
    border: 1px solid rgb(255 255 255 / 0.06);
    border-radius: 9px;
    background: rgb(255 255 255 / 0.02);
}

.vault-summary strong,
.vault-summary small {
    display: block;
}

.vault-summary strong {
    color: #cbd5e1;
    font-size: 10px;
}

.vault-summary small {
    margin-top: 2px;
    color: #64748b;
    font-size: 9px;
}

.secret-details {
    margin-top: 9px;
    overflow: hidden;
    border: 1px solid rgb(255 255 255 / 0.065);
    border-radius: 9px;
    background: rgb(2 8 20 / 0.24);
}

.signing-methods {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 9px;
}

.signing-methods > button {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 10px;
    padding: 12px;
    color: #7dd3fc;
    text-align: left;
    border: 1px solid rgb(255 255 255 / 0.07);
    border-radius: 10px;
    background: rgb(2 8 20 / 0.3);
}

.signing-methods > button.selected {
    color: #5eead4;
    border-color: rgb(94 234 212 / 0.3);
    background: rgb(45 212 191 / 0.06);
    box-shadow: 0 0 0 2px rgb(45 212 191 / 0.035);
}

.signing-methods strong,
.signing-methods small {
    display: block;
}

.signing-methods strong {
    color: #e2e8f0;
    font-size: 11px;
}

.signing-methods small {
    margin-top: 3px;
    color: #64748b;
    font-size: 9px;
    line-height: 1.4;
}

.signing-methods b {
    padding: 4px 7px;
    color: currentcolor;
    font-size: 8px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    border-radius: 999px;
    background: rgb(255 255 255 / 0.04);
}

.signing-method-guide {
    margin-top: 11px;
    padding: 12px;
    border: 1px solid rgb(255 255 255 / 0.065);
    border-radius: 10px;
    background: rgb(2 8 20 / 0.24);
}

.signing-method-guide > header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
}

.signing-method-guide > header strong,
.signing-method-guide > header small {
    display: block;
}

.signing-method-guide > header strong {
    color: #e2e8f0;
    font-size: 11px;
}

.signing-method-guide > header small {
    margin-top: 3px;
    color: #64748b;
    font-size: 9px;
    line-height: 1.45;
}

.signing-target {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 11px;
}

.signing-target span {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    color: #64748b;
    font-size: 8px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    border: 1px solid rgb(94 234 212 / 0.1);
    border-radius: 7px;
    background: rgb(45 212 191 / 0.025);
}

.signing-target code {
    color: #99f6e4;
    font-size: 9px;
    text-transform: none;
    letter-spacing: normal;
}

.signing-steps {
    display: grid;
    gap: 9px;
    margin: 12px 0 0;
    padding: 0;
    list-style: none;
}

.signing-steps li {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: start;
    gap: 9px;
}

.signing-steps li > span {
    display: grid;
    width: 21px;
    height: 21px;
    place-items: center;
    color: #052e2b;
    font-family: ui-monospace, monospace;
    font-size: 8px;
    font-weight: 700;
    border-radius: 50%;
    background: #5eead4;
}

.signing-steps p,
.signing-route-note,
.optional-key-intro p {
    margin: 0;
    color: #94a3b8;
    font-size: 9px;
    line-height: 1.55;
}

.signing-steps b,
.signing-route-note b,
.optional-key-intro b {
    color: #cbd5e1;
}

.signing-route-note {
    margin-top: 11px;
    padding: 9px 10px;
    color: #fcd34d;
    border-radius: 8px;
    background: rgb(251 191 36 / 0.045);
}

.signing-resource-actions,
.optional-key-intro {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 11px;
}

.optional-key-intro {
    align-items: flex-start;
    padding: 0 11px 11px;
}

.optional-key-intro p {
    flex: 1;
}

.optional-key-intro .secondary-button {
    flex: 0 0 auto;
}

.optional-key-details summary {
    flex-wrap: wrap;
}

.optional-label {
    margin-left: auto;
    color: #64748b;
    font-size: 8px;
    font-weight: 500;
}

.team-api-verification {
    margin-top: 11px;
    padding: 11px;
    border: 1px solid rgb(125 211 252 / 0.14);
    border-radius: 10px;
    background: rgb(14 116 144 / 0.035);
}

.team-api-verification.ready {
    border-color: rgb(94 234 212 / 0.2);
    background: rgb(45 212 191 / 0.035);
}

.team-api-verification.warning {
    border-color: rgb(251 191 36 / 0.2);
    background: rgb(251 191 36 / 0.035);
}

.team-api-verification > header {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 9px;
}

.team-api-verification > header > span:first-child {
    display: grid;
    width: 30px;
    height: 30px;
    place-items: center;
    color: #67e8f9;
    border-radius: 8px;
    background: rgb(6 182 212 / 0.1);
}

.team-api-verification > header strong,
.team-api-verification > header small,
.team-api-result-grid span,
.team-api-result-grid strong,
.team-api-result-grid small {
    display: block;
}

.team-api-verification > header strong {
    color: #e2e8f0;
    font-size: 10px;
}

.team-api-verification > header small,
.team-api-result-grid small,
.team-api-guidance {
    margin-top: 2px;
    color: #64748b;
    font-size: 9px;
    line-height: 1.45;
}

.team-api-result-grid {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 7px;
    margin-top: 10px;
}

.team-api-result-grid > div {
    min-width: 0;
    padding: 8px 9px;
    border: 1px solid rgb(255 255 255 / 0.055);
    border-radius: 8px;
    background: rgb(2 8 20 / 0.28);
}

.team-api-result-grid span {
    color: #64748b;
    font-size: 8px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
}

.team-api-result-grid strong {
    overflow: hidden;
    margin-top: 3px;
    color: #cbd5e1;
    font-size: 9px;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.team-api-guidance {
    margin-bottom: 0;
}

.apple-profile-inventory {
    margin-top: 10px;
    padding: 10px;
    border: 1px solid rgb(255 255 255 / 0.055);
    border-radius: 8px;
    background: rgb(2 8 20 / 0.22);
}

.apple-profile-inventory > header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
}

.apple-profile-inventory > header strong,
.apple-profile-inventory > header small,
.apple-profile-list article strong,
.apple-profile-list article small {
    display: block;
}

.apple-profile-inventory > header strong,
.apple-profile-list article strong {
    color: #cbd5e1;
    font-size: 9px;
}

.apple-profile-inventory > header small,
.apple-profile-list article small {
    margin-top: 2px;
    color: #64748b;
    font-size: 8px;
    line-height: 1.45;
}

.apple-profile-list {
    display: grid;
    gap: 6px;
    margin-top: 9px;
}

.apple-profile-list article {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 9px;
    border: 1px solid rgb(94 234 212 / 0.1);
    border-radius: 7px;
    background: rgb(45 212 191 / 0.025);
}

.apple-profile-list article > span {
    flex: 0 0 auto;
    text-align: right;
}

.apple-profile-list article > span b {
    display: block;
    color: #5eead4;
    font-size: 8px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
}

.apple-profile-list article.expired {
    border-color: rgb(251 191 36 / 0.12);
    background: rgb(251 191 36 / 0.025);
}

.apple-profile-list article.expired > span b {
    color: #fcd34d;
}

.apple-certificate-inventory {
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid rgb(255 255 255 / 0.055);
}

.apple-certificate-inventory > header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
}

.apple-certificate-inventory > header strong,
.apple-certificate-inventory > header small,
.apple-certificate-list article strong,
.apple-certificate-list article small {
    display: block;
}

.apple-certificate-inventory > header strong,
.apple-certificate-list article strong {
    color: #cbd5e1;
    font-size: 9px;
}

.apple-certificate-inventory > header small,
.apple-certificate-list article small {
    margin-top: 2px;
    color: #64748b;
    font-size: 8px;
    line-height: 1.45;
}

.apple-certificate-list {
    display: grid;
    gap: 6px;
    margin-top: 9px;
}

.apple-certificate-list article {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 9px;
    border: 1px solid rgb(251 191 36 / 0.12);
    border-radius: 7px;
    background: rgb(251 191 36 / 0.025);
}

.apple-certificate-list article > span {
    flex: 0 0 auto;
    text-align: right;
}

.apple-certificate-list article > span b {
    display: block;
    color: #fcd34d;
    font-size: 8px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
}

.apple-certificate-list article.usable {
    border-color: rgb(94 234 212 / 0.1);
    background: rgb(45 212 191 / 0.025);
}

.apple-certificate-list article.usable > span b {
    color: #5eead4;
}

.apple-profile-replacement {
    margin-top: 10px;
    padding: 10px;
    border: 1px solid rgb(125 211 252 / 0.14);
    border-radius: 8px;
    background: rgb(14 116 144 / 0.035);
}

.apple-profile-replacement > header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
}

.apple-profile-replacement > header strong,
.apple-profile-replacement > header small {
    display: block;
}

.apple-profile-replacement > header strong {
    color: #e2e8f0;
    font-size: 9px;
}

.apple-profile-replacement > header small {
    margin-top: 2px;
    color: #64748b;
    font-size: 8px;
}

.apple-certificate-select {
    margin-top: 10px;
}

.apple-certificate-summary {
    display: grid;
    grid-template-columns: minmax(0, 1.5fr) repeat(2, minmax(0, 1fr));
    gap: 6px;
    margin-top: 8px;
}

.apple-certificate-summary > div {
    min-width: 0;
    padding: 7px 8px;
    border: 1px solid rgb(255 255 255 / 0.055);
    border-radius: 7px;
    background: rgb(2 8 20 / 0.25);
}

.apple-certificate-summary span,
.apple-certificate-summary strong {
    display: block;
}

.apple-certificate-summary span {
    color: #64748b;
    font-size: 8px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
}

.apple-certificate-summary strong {
    overflow: hidden;
    margin-top: 3px;
    color: #cbd5e1;
    font-size: 9px;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.apple-certificate-warning,
.apple-profile-confirmation,
.apple-certificate-missing,
.apple-profile-created {
    margin: 9px 0 0;
    padding: 8px 9px;
    color: #94a3b8;
    font-size: 9px;
    line-height: 1.5;
    border-radius: 7px;
    background: rgb(255 255 255 / 0.025);
}

.apple-profile-confirmation {
    color: #fcd34d;
    border: 1px solid rgb(251 191 36 / 0.12);
    background: rgb(251 191 36 / 0.04);
}

.apple-profile-confirmation b {
    color: #fde68a;
}

.apple-certificate-missing p {
    margin: 0 0 8px;
}

.apple-profile-create-button {
    width: 100%;
    margin-top: 9px;
}

.apple-profile-created {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    color: #5eead4;
    border: 1px solid rgb(94 234 212 / 0.1);
    background: rgb(45 212 191 / 0.035);
}

.apple-profile-created svg {
    flex: 0 0 auto;
    margin-top: 1px;
}

.apple-profile-created code {
    color: #99f6e4;
    overflow-wrap: anywhere;
}

.profile-replacement-guidance,
.secret-action-error {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    margin: 9px 0 0;
    padding: 8px 9px;
    color: #fcd34d;
    font-size: 9px;
    line-height: 1.45;
    border-radius: 7px;
    background: rgb(251 191 36 / 0.045);
}

.secret-action-error {
    color: #fda4af;
    background: rgb(244 63 94 / 0.055);
}

.secret-action-error svg {
    flex: 0 0 auto;
    margin-top: 1px;
}

.team-api-verify-button {
    width: 100%;
    margin-top: 10px;
}

.secret-details summary {
    gap: 7px;
    padding: 10px 11px;
    color: #94a3b8;
    font-size: 10px;
    font-weight: 700;
    cursor: pointer;
    list-style: none;
}

.secret-details summary::-webkit-details-marker {
    display: none;
}

.secret-fields {
    padding: 0 11px 11px;
}

.secret-actions {
    margin-top: 11px;
}

.signing-provision-panel {
    margin-top: 12px;
    padding: 12px;
    border: 1px solid rgb(167 139 250 / 0.14);
    border-radius: 10px;
    background: rgb(139 92 246 / 0.035);
}

.signing-provision-panel.ready {
    border-color: rgb(94 234 212 / 0.15);
    background: rgb(45 212 191 / 0.03);
}

.signing-provision-panel > header {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 9px;
}

.signing-provision-panel > header strong,
.signing-provision-panel > header small {
    display: block;
}

.signing-provision-panel > header strong {
    color: #e2e8f0;
    font-size: 10px;
}

.signing-provision-panel > header small {
    margin-top: 2px;
    color: #64748b;
    font-size: 9px;
}

.signing-provision-icon {
    display: grid;
    width: 30px;
    height: 30px;
    place-items: center;
    color: #c4b5fd;
    border-radius: 8px;
    background: rgb(139 92 246 / 0.1);
}

.signing-verification-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 7px;
    margin-top: 11px;
}

.signing-verification-grid > div {
    min-width: 0;
    padding: 8px 9px;
    border: 1px solid rgb(255 255 255 / 0.055);
    border-radius: 8px;
    background: rgb(2 8 20 / 0.28);
}

.signing-verification-grid span,
.signing-verification-grid strong,
.signing-verification-grid small {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.signing-verification-grid span {
    color: #64748b;
    font-size: 8px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
}

.signing-verification-grid strong {
    margin-top: 3px;
    color: #cbd5e1;
    font-size: 9px;
}

.signing-verification-grid small {
    margin-top: 2px;
    color: #64748b;
    font-family: ui-monospace, monospace;
    font-size: 8px;
}

.signing-readiness {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 11px;
}

.signing-readiness span {
    padding: 5px 7px;
    color: #64748b;
    font-size: 8px;
    border: 1px solid rgb(255 255 255 / 0.06);
    border-radius: 999px;
}

.signing-readiness span.complete {
    color: #5eead4;
    border-color: rgb(94 234 212 / 0.14);
    background: rgb(45 212 191 / 0.035);
}

.signing-progress {
    margin-top: 11px;
    padding: 9px;
    border: 1px solid rgb(255 255 255 / 0.055);
    border-radius: 8px;
    background: rgb(2 8 20 / 0.28);
}

.signing-progress > div:first-child {
    display: flex;
    justify-content: space-between;
    gap: 10px;
}

.signing-progress strong {
    color: #cbd5e1;
    font-size: 9px;
}

.signing-progress span {
    color: #94a3b8;
    font-family: ui-monospace, monospace;
    font-size: 8px;
}

.signing-progress p,
.signing-security-note {
    margin: 7px 0 0;
    color: #64748b;
    font-size: 9px;
    line-height: 1.5;
}

.signing-recovery {
    margin-top: 10px;
    padding: 10px;
    color: #fcd34d;
    font-size: 9px;
    line-height: 1.5;
    border: 1px solid rgb(251 191 36 / 0.16);
    border-radius: 8px;
    background: rgb(251 191 36 / 0.045);
}

.signing-recovery header {
    display: flex;
    align-items: flex-start;
    gap: 8px;
}

.signing-recovery header svg {
    flex: 0 0 auto;
    margin-top: 1px;
}

.signing-recovery header div {
    display: grid;
    gap: 2px;
}

.signing-recovery strong,
.signing-recovery b {
    color: #fde68a;
}

.signing-recovery small {
    color: #94a3b8;
}

.signing-recovery ol {
    display: grid;
    gap: 5px;
    margin: 9px 0;
    padding-left: 18px;
    color: #94a3b8;
}

.signing-recovery-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 7px;
}

.signing-provision-button,
.signing-clear-button {
    width: 100%;
    margin-top: 11px;
}

.signing-clear-button {
    margin-left: 0;
}

.danger-link {
    margin-left: auto;
    color: #fda4af;
    border: 0;
    background: transparent;
}

.signed-archive-panel {
    margin-top: 12px;
    padding: 13px;
    border: 1px solid rgb(167 139 250 / 0.14);
    border-radius: 10px;
    background: rgb(139 92 246 / 0.03);
}

.signed-archive-panel.ready {
    border-color: rgb(94 234 212 / 0.17);
}

.signed-archive-panel.complete {
    background: rgb(45 212 191 / 0.035);
}

.signed-archive-panel > header {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 9px;
}

.signed-archive-panel > header strong,
.signed-archive-panel > header small {
    display: block;
}

.signed-archive-panel > header strong {
    color: #e2e8f0;
    font-size: 10px;
}

.signed-archive-panel > header small {
    margin-top: 2px;
    color: #64748b;
    font-size: 9px;
}

.signed-archive-icon {
    display: grid;
    width: 30px;
    height: 30px;
    color: #c4b5fd;
    border-radius: 8px;
    background: rgb(139 92 246 / 0.09);
    place-items: center;
}

.signed-archive-recipe {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 7px;
    margin-top: 11px;
}

.signed-archive-recipe > div,
.signed-archive-version,
.signed-archive-result article {
    min-width: 0;
    padding: 9px;
    border: 1px solid rgb(255 255 255 / 0.055);
    border-radius: 8px;
    background: rgb(2 8 20 / 0.26);
}

.signed-archive-recipe span,
.signed-archive-recipe b,
.signed-archive-version span,
.signed-archive-version strong,
.signed-archive-version small,
.signed-archive-result article strong,
.signed-archive-result article small,
.signed-archive-result article span {
    display: block;
}

.signed-archive-recipe span,
.signed-archive-version span {
    color: #64748b;
    font-size: 8px;
}

.signed-archive-recipe b {
    margin-top: 3px;
    overflow: hidden;
    color: #cbd5e1;
    font-family: ui-monospace, monospace;
    font-size: 8px;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.signed-archive-guidance {
    margin: 9px 0 0;
    padding: 9px 10px;
    color: #94a3b8;
    font-size: 9px;
    line-height: 1.5;
    border-radius: 8px;
    background: rgb(125 211 252 / 0.04);
}

.signed-archive-guidance b {
    color: #bae6fd;
}

.signed-archive-build {
    width: 100%;
    margin-top: 9px;
}

.signed-archive-error {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    margin: 9px 0 0;
    padding: 9px 10px;
    color: #fda4af;
    font-size: 9px;
    line-height: 1.5;
    border: 1px solid rgb(251 113 133 / 0.18);
    border-radius: 8px;
    background: rgb(244 63 94 / 0.055);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
}

.signed-archive-error svg {
    flex: 0 0 auto;
    margin-top: 1px;
}

.signed-archive-result {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    margin-top: 10px;
}

.signed-archive-version {
    grid-column: 1 / -1;
}

.signed-archive-version strong {
    margin-top: 3px;
    color: #5eead4;
    font-size: 10px;
}

.signed-archive-version small {
    margin-top: 3px;
    color: #64748b;
    font-size: 8px;
}

.signed-archive-result article {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: start;
    gap: 8px;
    color: #67e8f9;
}

.signed-archive-result article strong {
    color: #cbd5e1;
    font-size: 9px;
}

.signed-archive-result article small {
    margin-top: 3px;
    overflow: hidden;
    color: #94a3b8;
    font-family: ui-monospace, monospace;
    font-size: 8px;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.signed-archive-result article span {
    margin-top: 3px;
    color: #64748b;
    font-family: ui-monospace, monospace;
    font-size: 8px;
}

.signed-archive-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 7px;
    margin-top: 9px;
}

.apple-note {
    padding: 9px 10px;
    border-radius: 8px;
    background: rgb(251 191 36 / 0.04);
}

.runtime-header .container-id {
    margin-left: auto;
    color: #64748b;
    font-family: ui-monospace, monospace;
    font-size: 9px;
}

.bootstrap-flow {
    display: grid;
    grid-template-columns: 1fr 25px 1fr 25px 1fr;
    align-items: center;
}

.bootstrap-flow article {
    display: grid;
    grid-template-columns: auto 1fr;
    align-items: center;
    gap: 9px;
    padding: 10px;
    border: 1px solid rgb(255 255 255 / 0.06);
    border-radius: 9px;
    background: rgb(255 255 255 / 0.02);
}

.bootstrap-flow article > span {
    display: grid;
    width: 23px;
    height: 23px;
    place-items: center;
    color: #64748b;
    font-family: ui-monospace, monospace;
    font-size: 9px;
    border-radius: 50%;
    background: rgb(255 255 255 / 0.05);
}

.bootstrap-flow article.complete > span {
    color: #052e2b;
    background: #5eead4;
}

.bootstrap-flow strong,
.bootstrap-flow small {
    display: block;
}

.bootstrap-flow strong {
    color: #cbd5e1;
    font-size: 10px;
}

.bootstrap-flow small {
    margin-top: 2px;
    color: #64748b;
    font-size: 9px;
}

.bootstrap-flow > i {
    height: 1px;
    background: rgb(255 255 255 / 0.08);
}

.installer-monitor {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 9px;
    margin-top: 12px;
    padding: 11px 13px;
    color: #7dd3fc;
    border: 1px solid rgb(56 189 248 / 0.16);
    border-radius: 10px;
    background: rgb(56 189 248 / 0.045);
}

.installer-monitor.ready {
    color: #5eead4;
    border-color: rgb(94 234 212 / 0.14);
    background: rgb(45 212 191 / 0.04);
}

.installer-monitor > div:not(.installer-monitor-track) {
    min-width: 0;
}

.installer-monitor strong,
.installer-monitor small {
    display: block;
}

.installer-monitor strong {
    color: #e2e8f0;
    font-size: 11px;
}

.installer-monitor small {
    margin-top: 2px;
    color: #94a3b8;
    font-size: 9px;
}

.installer-monitor-icon {
    flex: 0 0 auto;
}

.installer-elapsed {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: #94a3b8;
    font-family: ui-monospace, monospace;
    font-size: 9px;
    white-space: nowrap;
}

.installer-monitor-track {
    position: relative;
    grid-column: 1 / -1;
    height: 3px;
    overflow: hidden;
    border-radius: 999px;
    background: rgb(125 211 252 / 0.1);
}

.installer-monitor-track i {
    position: absolute;
    width: 32%;
    height: 100%;
    border-radius: inherit;
    background: linear-gradient(90deg, transparent, #38bdf8, transparent);
    animation: installer-scan 1.8s ease-in-out infinite;
}

.installer-monitor p {
    grid-column: 1 / -1;
    margin: 0;
    color: #64748b;
    font-size: 9px;
    line-height: 1.5;
}

.installer-guide {
    margin-top: 12px;
    overflow: hidden;
    border: 1px solid rgb(251 191 36 / 0.16);
    border-radius: 10px;
    background: rgb(251 191 36 / 0.035);
}

.installer-guide summary {
    gap: 9px;
    padding: 11px 13px;
    color: #fcd34d;
    cursor: pointer;
    list-style: none;
}

.installer-guide summary::-webkit-details-marker {
    display: none;
}

.installer-guide summary > span {
    min-width: 0;
}

.installer-guide summary strong,
.installer-guide summary small {
    display: block;
}

.installer-guide summary strong {
    color: #e2e8f0;
    font-size: 11px;
}

.installer-guide summary small {
    margin-top: 2px;
    color: #94a3b8;
    font-size: 9px;
}

.installer-guide summary > b {
    margin-left: auto;
    color: #fcd34d;
    font-size: 9px;
    text-transform: uppercase;
}

.installer-steps {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    padding: 0 12px 12px;
}

.installer-steps article {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 9px;
    padding: 10px;
    border: 1px solid rgb(255 255 255 / 0.06);
    border-radius: 8px;
    background: rgb(2 8 20 / 0.28);
}

.installer-steps article > span {
    display: grid;
    width: 23px;
    height: 23px;
    place-items: center;
    color: #422006;
    font-family: ui-monospace, monospace;
    font-size: 9px;
    font-weight: 700;
    border-radius: 50%;
    background: #fcd34d;
}

.installer-steps strong {
    color: #e2e8f0;
    font-size: 10px;
}

.installer-steps p {
    margin: 4px 0 0;
    color: #94a3b8;
    font-size: 9px;
    line-height: 1.55;
}

.installer-steps p b {
    color: #cbd5e1;
}

.console-warning {
    gap: 7px;
    margin: 0 12px 12px;
    padding: 9px 10px;
    color: #fcd34d;
    font-size: 9px;
    line-height: 1.5;
    border-radius: 8px;
    background: rgb(251 191 36 / 0.055);
}

.guest-bridge-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
    margin-top: 12px;
}

.guest-access-panel,
.guest-status-panel {
    min-width: 0;
    padding: 14px;
    border: 1px solid rgb(255 255 255 / 0.06);
    border-radius: 10px;
    background: rgb(2 8 20 / 0.3);
}

.guest-panel-title {
    gap: 8px;
    color: #5eead4;
}

.guest-panel-title > div {
    min-width: 0;
}

.guest-panel-title > .state-badge {
    margin-left: auto;
}

.guest-panel-title strong,
.guest-panel-title small {
    display: block;
}

.guest-panel-title strong {
    color: #cbd5e1;
    font-size: 11px;
}

.guest-panel-title small {
    margin-top: 2px;
    color: #64748b;
    font-size: 9px;
}

.guest-user-form {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: end;
    gap: 8px;
    margin-top: 13px;
}

.public-key-box {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 7px;
    margin-top: 9px;
    padding: 8px;
    border: 1px solid rgb(94 234 212 / 0.12);
    border-radius: 8px;
    background: rgb(45 212 191 / 0.035);
}

.public-key-box code {
    overflow: hidden;
    color: #94a3b8;
    font-size: 8px;
    line-height: 1.45;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.public-key-box button {
    display: grid;
    width: 26px;
    height: 26px;
    place-items: center;
    color: #5eead4;
    border: 0;
    border-radius: 6px;
    background: rgb(45 212 191 / 0.08);
}

.guest-instructions {
    margin: 10px 0 0;
    padding-left: 20px;
    color: #64748b;
    font-size: 9px;
    line-height: 1.65;
}

.guest-instructions code {
    color: #94a3b8;
}

.guest-status-list {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 7px;
    margin-top: 13px;
}

.guest-status-list > div {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 8px;
    padding: 8px;
    color: #64748b;
    border: 1px solid rgb(255 255 255 / 0.05);
    border-radius: 8px;
}

.guest-status-list > div.ready {
    color: #5eead4;
    border-color: rgb(94 234 212 / 0.1);
    background: rgb(45 212 191 / 0.025);
}

.guest-status-list strong,
.guest-status-list small {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.guest-status-list strong {
    color: #cbd5e1;
    font-size: 9px;
}

.guest-status-list small {
    margin-top: 2px;
    color: #64748b;
    font-family: ui-monospace, monospace;
    font-size: 8px;
}

.guest-issue {
    gap: 7px;
    margin: 9px 0 0;
    padding: 8px;
    color: #fda4af;
    font-size: 9px;
    line-height: 1.45;
    border-radius: 8px;
    background: rgb(244 63 94 / 0.05);
}

.guest-issue.warning {
    color: #fcd34d;
    background: rgb(251 191 36 / 0.05);
}

.guest-trust-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 9px;
}

.xcode-import-panel {
    margin-top: 12px;
    padding: 14px;
    border: 1px solid rgb(125 211 252 / 0.12);
    border-radius: 10px;
    background: rgb(56 189 248 / 0.03);
    transition:
        border-color 160ms ease,
        background 160ms ease;
}

.xcode-import-panel.dragging {
    border-color: rgb(94 234 212 / 0.55);
    background: rgb(45 212 191 / 0.08);
}

.xcode-import-panel.ready {
    border-color: rgb(94 234 212 / 0.14);
    background: rgb(45 212 191 / 0.035);
}

.xcode-import-panel > header {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 9px;
}

.xcode-import-panel > header > div {
    min-width: 0;
}

.xcode-import-panel > header strong,
.xcode-import-panel > header small,
.xcode-ready-summary strong,
.xcode-ready-summary small,
.xcode-activation strong,
.xcode-activation small {
    display: block;
}

.xcode-import-panel > header strong,
.xcode-ready-summary strong,
.xcode-activation strong {
    color: #e2e8f0;
    font-size: 10px;
}

.xcode-import-panel > header small,
.xcode-ready-summary small,
.xcode-activation small {
    margin-top: 2px;
    color: #64748b;
    font-size: 9px;
}

.xcode-import-icon {
    display: grid;
    width: 30px;
    height: 30px;
    place-items: center;
    color: #7dd3fc;
    border-radius: 8px;
    background: rgb(56 189 248 / 0.08);
}

.xcode-package-input {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: end;
    gap: 9px;
    margin-top: 13px;
}

.xcode-package-input label {
    min-width: 0;
}

.xcode-drop-hint {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 8px 0 0;
    color: #64748b;
    font-size: 9px;
    line-height: 1.5;
}

.xcode-drop-hint b {
    color: #94a3b8;
}

.xcode-import-progress {
    margin-top: 12px;
    padding: 10px;
    border: 1px solid rgb(255 255 255 / 0.055);
    border-radius: 8px;
    background: rgb(2 8 20 / 0.28);
}

.xcode-import-progress > div:first-child {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
}

.xcode-import-progress strong {
    color: #cbd5e1;
    font-size: 9px;
}

.xcode-import-progress span {
    color: #94a3b8;
    font-family: ui-monospace, monospace;
    font-size: 8px;
}

.xcode-import-progress p {
    margin: 7px 0 0;
    color: #64748b;
    font-size: 9px;
}

.xcode-progress-track {
    position: relative;
    height: 4px;
    margin-top: 8px;
    overflow: hidden;
    border-radius: 999px;
    background: rgb(125 211 252 / 0.1);
}

.xcode-progress-track i {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: linear-gradient(90deg, #0ea5e9, #5eead4);
    transition: width 180ms ease;
}

.xcode-progress-track.indeterminate i {
    position: absolute;
    width: 32% !important;
    background: linear-gradient(90deg, transparent, #5eead4, transparent);
    animation: installer-scan 1.8s ease-in-out infinite;
}

.xcode-progress-track.failed i {
    background: #fb7185;
}

.xcode-activation,
.xcode-ready-summary {
    margin-top: 12px;
    padding: 10px;
    color: #5eead4;
    border: 1px solid rgb(94 234 212 / 0.1);
    border-radius: 8px;
    background: rgb(45 212 191 / 0.025);
}

.xcode-ready-summary,
.xcode-activation > div:first-child {
    display: flex;
    align-items: center;
    gap: 8px;
}

.xcode-first-open-note {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: start;
    gap: 8px;
    margin-top: 9px;
    padding: 10px;
    color: #fcd34d;
    border: 1px solid rgb(251 191 36 / 0.1);
    border-radius: 8px;
    background: rgb(251 191 36 / 0.035);
}

.xcode-first-open-note strong {
    display: block;
    color: #e2e8f0;
    font-size: 10px;
}

.xcode-first-open-note p {
    margin: 4px 0 0;
    color: #94a3b8;
    font-size: 9px;
    line-height: 1.55;
}

.xcode-first-open-note b {
    color: #cbd5e1;
}

.xcode-authorization-note {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 10px 0;
    color: #94a3b8;
    font-size: 9px;
    line-height: 1.5;
}

.xcode-authorization-note svg {
    flex: 0 0 auto;
    color: #7dd3fc;
}

.xcode-activate-button {
    width: 100%;
}

.xcode-manual-fallback {
    margin-top: 9px;
    color: #64748b;
    font-size: 9px;
}

.xcode-manual-fallback summary {
    padding: 3px 0;
    cursor: pointer;
}

.xcode-activation pre {
    margin: 9px 0;
    padding: 9px;
    overflow: auto;
    color: #cbd5e1;
    font-family: ui-monospace, monospace;
    font-size: 8px;
    line-height: 1.6;
    border-radius: 7px;
    background: rgb(2 8 20 / 0.5);
    white-space: pre-wrap;
}

.apple-project-panel {
    margin-top: 12px;
    padding: 14px;
    border: 1px solid rgb(167 139 250 / 0.14);
    border-radius: 10px;
    background: rgb(139 92 246 / 0.03);
}

.apple-project-panel.ready {
    border-color: rgb(94 234 212 / 0.18);
    background: rgb(45 212 191 / 0.035);
}

.apple-project-panel > header {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 9px;
}

.apple-project-panel > header strong,
.apple-project-panel > header small {
    display: block;
}

.apple-project-panel > header strong {
    color: #e2e8f0;
    font-size: 10px;
}

.apple-project-panel > header small {
    margin-top: 2px;
    color: #64748b;
    font-size: 9px;
}

.apple-project-icon {
    display: grid;
    width: 30px;
    height: 30px;
    color: #c4b5fd;
    border-radius: 8px;
    background: rgb(139 92 246 / 0.09);
    place-items: center;
}

.apple-workflow-steps {
    display: grid;
    grid-template-columns: 1fr auto 1fr auto 1fr;
    align-items: center;
    gap: 8px;
    margin-top: 13px;
}

.apple-workflow-steps > article {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 8px;
    min-height: 50px;
    padding: 9px;
    border: 1px solid rgb(255 255 255 / 0.055);
    border-radius: 8px;
    background: rgb(2 8 20 / 0.25);
}

.apple-workflow-steps > article.ready {
    border-color: rgb(94 234 212 / 0.16);
    background: rgb(45 212 191 / 0.04);
}

.apple-workflow-steps > article > b {
    display: grid;
    width: 23px;
    height: 23px;
    color: #94a3b8;
    font-family: ui-monospace, monospace;
    font-size: 8px;
    border-radius: 999px;
    background: rgb(148 163 184 / 0.1);
    place-items: center;
}

.apple-workflow-steps > article.ready > b {
    color: #5eead4;
    background: rgb(45 212 191 / 0.1);
}

.apple-workflow-steps strong,
.apple-workflow-steps small {
    display: block;
}

.apple-workflow-steps strong {
    color: #cbd5e1;
    font-size: 9px;
}

.apple-workflow-steps small {
    margin-top: 2px;
    color: #64748b;
    font-size: 8px;
    line-height: 1.4;
}

.apple-workflow-steps > i {
    width: 12px;
    height: 1px;
    background: rgb(255 255 255 / 0.08);
}

.apple-project-input {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: end;
    gap: 9px;
    margin-top: 12px;
}

.apple-project-hint {
    margin: 7px 0 0;
    color: #64748b;
    font-size: 8px;
}

.apple-project-summary {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto auto;
    gap: 8px;
    margin-top: 11px;
    padding: 9px;
    border: 1px solid rgb(255 255 255 / 0.055);
    border-radius: 8px;
    background: rgb(2 8 20 / 0.28);
}

.apple-project-summary strong,
.apple-project-summary small,
.apple-project-summary span,
.apple-project-summary b {
    display: block;
}

.apple-project-summary > div:first-child {
    min-width: 0;
}

.apple-project-summary strong,
.apple-project-summary b {
    color: #cbd5e1;
    font-size: 9px;
}

.apple-project-summary small {
    margin-top: 2px;
    overflow: hidden;
    color: #64748b;
    font-family: ui-monospace, monospace;
    font-size: 8px;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.apple-project-summary span {
    margin-bottom: 3px;
    color: #64748b;
    font-size: 8px;
}

.apple-project-actions {
    display: flex;
    gap: 8px;
    margin-top: 10px;
}

.apple-project-actions .launch-button {
    flex: 1;
}

.apple-next-step,
.apple-build-ready {
    margin: 10px 0 0;
    padding: 9px 10px;
    color: #94a3b8;
    font-size: 9px;
    line-height: 1.5;
    border-radius: 8px;
    background: rgb(125 211 252 / 0.045);
}

.apple-lock-warning {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    margin: 8px 0 0;
    padding: 9px 10px;
    color: #fcd34d;
    font-size: 9px;
    line-height: 1.5;
    border-radius: 8px;
    background: rgb(245 158 11 / 0.055);
}

.apple-lock-warning svg {
    flex: 0 0 auto;
    margin-top: 1px;
}

.apple-next-step b,
.apple-next-step code {
    color: #bae6fd;
}

.apple-build-ready {
    display: flex;
    align-items: center;
    gap: 7px;
    color: #5eead4;
    background: rgb(45 212 191 / 0.045);
}

.apple-project-progress {
    margin-top: 10px;
    padding: 10px;
    border: 1px solid rgb(255 255 255 / 0.055);
    border-radius: 8px;
    background: rgb(2 8 20 / 0.28);
}

.apple-project-progress > div:first-child {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
}

.apple-project-progress strong {
    color: #cbd5e1;
    font-size: 9px;
}

.apple-project-progress span {
    color: #94a3b8;
    font-family: ui-monospace, monospace;
    font-size: 8px;
}

.apple-project-progress p {
    margin: 7px 0 0;
    color: #64748b;
    font-size: 9px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
}

.apple-build-output {
    margin-top: 10px;
    overflow: hidden;
    border: 1px solid rgb(255 255 255 / 0.06);
    border-radius: 8px;
    background: #030914;
}

.apple-build-output > div {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 8px 10px;
    color: #64748b;
    font-size: 8px;
    border-bottom: 1px solid rgb(255 255 255 / 0.05);
}

.apple-build-output pre {
    max-height: 190px;
    margin: 0;
    padding: 10px;
    overflow: auto;
    color: #94a3b8;
    font-family: ui-monospace, monospace;
    font-size: 8px;
    line-height: 1.5;
    white-space: pre-wrap;
}

.log-panel {
    margin-top: 12px;
    overflow: hidden;
    border: 1px solid rgb(255 255 255 / 0.06);
    border-radius: 9px;
    background: #030914;
}

.log-panel > div {
    gap: 7px;
    padding: 9px 11px;
    color: #64748b;
    font-size: 9px;
    border-bottom: 1px solid rgb(255 255 255 / 0.05);
}

.log-panel pre,
.log-panel p {
    max-height: 180px;
    margin: 0;
    padding: 11px;
    overflow: auto;
    color: #94a3b8;
    font-family: ui-monospace, monospace;
    font-size: 9px;
    line-height: 1.55;
    white-space: pre-wrap;
}

.spin {
    animation: spin 900ms linear infinite;
}

@keyframes spin {
    to {
        transform: rotate(360deg);
    }
}

@keyframes installer-scan {
    from {
        transform: translateX(-110%);
    }

    to {
        transform: translateX(320%);
    }
}

@media (max-width: 860px) {
    .builder-grid {
        grid-template-columns: 1fr;
    }

    .runtime-card {
        grid-column: auto;
    }

    .bootstrap-flow {
        grid-template-columns: 1fr;
        gap: 7px;
    }

    .bootstrap-flow > i {
        display: none;
    }

    .guest-bridge-grid {
        grid-template-columns: 1fr;
    }

    .apple-certificate-summary {
        grid-template-columns: 1fr;
    }

    .installer-steps {
        grid-template-columns: 1fr;
    }

    .installer-monitor {
        grid-template-columns: auto minmax(0, 1fr);
    }

    .installer-elapsed {
        grid-column: 2;
    }

    .apple-workflow-steps {
        grid-template-columns: 1fr;
    }

    .apple-workflow-steps > i {
        width: 1px;
        height: 8px;
        margin: -3px auto;
    }

    .apple-project-summary {
        grid-template-columns: minmax(0, 1fr) repeat(2, auto);
    }

    .apple-project-summary > div:last-child {
        grid-column: 1 / -1;
    }
}

@media (max-width: 620px) {
    .builder-content {
        padding: 24px 16px 30px;
    }

    .check-grid,
    .profile-form,
    .secret-fields.two-column,
    .signing-methods,
    .vault-summary,
    .guest-status-list,
    .guest-user-form,
    .xcode-package-input,
    .apple-project-input,
    .apple-project-summary,
    .team-api-result-grid,
    .signing-verification-grid,
    .signed-archive-recipe,
    .signed-archive-result {
        grid-template-columns: 1fr;
    }

    .xcode-import-panel > header {
        grid-template-columns: auto minmax(0, 1fr);
    }

    .xcode-import-panel > header .state-badge {
        grid-column: 2;
        justify-self: start;
    }

    .wide-field {
        grid-column: auto;
    }

    .profile-actions,
    .secret-actions,
    .apple-project-actions,
    .signing-resource-actions,
    .optional-key-intro,
    .signed-archive-actions {
        align-items: stretch;
        flex-direction: column;
    }

    .signing-method-guide > header {
        align-items: flex-start;
    }

    .signing-methods > button {
        grid-template-columns: auto minmax(0, 1fr);
        align-items: flex-start;
    }

    .signing-methods b {
        grid-column: 2;
        justify-self: start;
    }

    .apple-project-panel > header {
        grid-template-columns: auto minmax(0, 1fr);
    }

    .signing-provision-panel > header {
        grid-template-columns: auto minmax(0, 1fr);
    }

    .signed-archive-panel > header {
        grid-template-columns: auto minmax(0, 1fr);
    }

    .apple-project-panel > header .state-badge,
    .signing-provision-panel > header .state-badge,
    .signed-archive-panel > header .state-badge,
    .apple-project-summary > div:last-child {
        grid-column: 2;
        justify-self: start;
    }

    .danger-link {
        margin-left: 0;
    }
}
</style>
