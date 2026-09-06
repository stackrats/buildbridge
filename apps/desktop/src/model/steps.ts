// The step model is the heart of the interface: every screen that shows "what is done, what is
// next, and who does it" derives from these pure functions. They take the backend view and the
// client's in-flight operation and return an ordered list of steps with one status each.
//
// Keeping this pure keeps it testable: the guided flows in the desktop are covered by unit
// tests rather than by clicking through a macOS installation.

import type {
    HostPrerequisites,
    MachineView,
    MachineSummary,
    UnsignedBuildTarget,
} from '../types/backend';
import { formatBytes, formatElapsed, relativeTime, secondsSince } from '../lib/format';
import { isLive } from '../lib/status';
import { deviceNextSummary, deviceReadiness, deviceWorkingSummary } from './device';
import { androidDeviceApks } from './android-device';
import type { AndroidDeviceRun } from './android-device';
import { isAndroid, providerHostIssues, providerHostReady } from './providers';
import { formatVersion } from './version';
import {
    androidKitShortfall,
    kitIsProvisionable,
    kitReadiness,
    kitShortfall,
    kitSignsAndroid,
} from './signing';

export type StepStatus = 'done' | 'active' | 'running' | 'pending' | 'failed';

/**
 * Who performs a step.
 *
 * - `automatic`: BuildBridge does it end to end.
 * - `manual`: the user does it, usually inside the macOS console.
 * - `assisted`: BuildBridge starts it and the user confirms inside macOS.
 */
export type StepKind = 'automatic' | 'manual' | 'assisted';

/** The two halves of the journey: prepare a machine once, then build a project on it. */
// A device run is its own section rather than the tail of the build: it is optional, it needs
// hardware on the desk, and counting it with the required steps made the journey look unfinished
// on a machine that is doing exactly what was asked of it.
export type StepPhase = 'setup' | 'build' | 'device' | 'publish';

export interface Step<Id extends string> {
    id: Id;
    phase: StepPhase;
    title: string;
    kind: StepKind;
    status: StepStatus;
    /**
     * One line of fact for the rail: the result when done, what is happening when running,
     * what unlocks it when pending, what went wrong when failed.
     */
    summary: string;
    /** The usual duration of a long step, shown beside "next" so nobody is surprised. */
    expected?: string;
    /** Off the golden path: never the focus while merely available, only when it needs someone. */
    optional?: boolean;
    /** Self-hosted and experimental; the timeline says so beside the kind. */
    experimental?: boolean;
    /**
     * Running and arrived: the operation has reached what it set out to do and stays up until
     * it is stopped — an app on the phone streaming its console — so nothing is waiting.
     */
    live?: boolean;
}

export type SetupStepId =
    | 'host'
    | 'launch'
    | 'install'
    | 'trust'
    | 'access'
    | 'xcode-import'
    | 'xcode-activate';

export type BuildStepId =
    | 'approve'
    | 'sync'
    | 'test-build'
    | 'signing-kit'
    | 'provision'
    | 'archive'
    | 'run-device';

/**
 * An Android machine's journey is the same shape with fewer steps: no macOS to install, no
 * guest to trust, no Xcode; the debug build stands where the unsigned test build does, the
 * signed release where the archive does, and nothing is provisioned because the upload key is
 * streamed in per release.
 */
export type AndroidStepId =
    | 'host'
    | 'launch'
    | 'approve'
    | 'sync'
    | 'test-build'
    | 'signing-kit'
    | 'release';

export type JourneyStepId = SetupStepId | BuildStepId | AndroidStepId | 'publish';

export type SetupStep = Step<SetupStepId>;
export type BuildStep = Step<BuildStepId>;
export type AndroidStep = Step<AndroidStepId>;
export type JourneyStep = Step<JourneyStepId>;

export const phaseLabel: Record<StepPhase, string> = {
    setup: 'Machine setup',
    build: 'Project build',
    device: 'On a real device',
    publish: 'Publishing',
};

export const stepKindLabel: Record<StepKind, string> = {
    automatic: 'Automatic',
    manual: 'You do this',
    assisted: 'Assisted',
};

export const stepKindDescription: Record<StepKind, string> = {
    automatic: 'BuildBridge performs this step end to end.',
    manual: 'BuildBridge cannot do this for you; follow the instructions and it verifies the result.',
    assisted:
        'BuildBridge prepares this step; you finish it in the relevant app, service or device.',
};

/** Short, lowercase names for places with room for a few words, such as the sidebar. */
export const stepShortTitle: Record<JourneyStepId, string> = {
    host: 'check the host',
    launch: 'start the machine',
    install: 'install macOS',
    trust: 'pin the guest identity',
    access: 'authorize the key',
    'xcode-import': 'import Xcode',
    'xcode-activate': 'activate Xcode',
    approve: 'approve a project',
    sync: 'synchronize source',
    'test-build': 'test build',
    'signing-kit': 'attach signing credentials',
    provision: 'provision signing',
    archive: 'signed archive',
    release: 'signed release',
    'run-device': 'run on the device',
    publish: 'publish the release',
};

/** What a pending step is waiting for. A pending row never just says "waiting". */
const unlockedBy: Record<JourneyStepId, string> = {
    host: '',
    launch: 'after the host check passes',
    install: 'after the machine starts',
    trust: 'after macOS is reachable',
    access: 'after the identity is pinned',
    'xcode-import': 'after the access key is authorized',
    'xcode-activate': 'after Xcode is imported',
    approve: 'after the machine setup is complete',
    sync: 'after a project is approved',
    'test-build': 'after the source is synchronized',
    'signing-kit': 'after the test build passes',
    provision: 'after complete credentials are attached',
    archive: 'after signing is provisioned',
    release: 'after credentials with an upload key are attached',
    'run-device': 'after signing is provisioned',
    publish: 'after a signed release is retained',
};

export interface StepContext {
    androidDeviceRun?: AndroidDeviceRun | null;
    /** Step whose native operation this client started and is still awaiting. */
    runningStep: string | null;
    /**
     * The operation behind `runningStep`: this client's operation id or the native busy key.
     * A step that hosts more than one operation reads it, so a stop is never described as a
     * start.
     */
    runningOperation?: string | null;
    /**
     * The running operation has reached its live stage — the app is up on the phone with its
     * console streaming — rather than still working towards it.
     */
    runningLive?: boolean;
    /** Reference time for elapsed displays; injectable for tests. */
    now?: number;
}

/**
 * What a running step says when the operation on it is not the step's own: stopping,
 * discarding, deleting and saving a template run on the launch step, removing signing on the
 * provision step, adopting the lockfile on the test-build step. Keyed by client operation id
 * and native busy key alike.
 */
const runningSummaryByOperation: Record<string, string> = {
    stop: 'Stopping safely; the container and its macOS disk are kept',
    stopping: 'Stopping safely; the container and its macOS disk are kept',
    discard: 'Discarding the container and its macOS disk',
    discarding: 'Discarding the container and its macOS disk',
    delete: 'Deleting the machine',
    deleting: 'Deleting the machine',
    'save-template': 'Saving the machine as a template; macOS is shut down first',
    saving_template: 'Saving the machine as a template; macOS is shut down first',
    'clear-signing': 'Removing the guest keychain and installed profiles',
    clearing_signing: 'Removing the guest keychain and installed profiles',
    'adopt-lock': 'Adopting the guest’s Podfile.lock into the project',
    adopting_lock: 'Adopting the guest’s Podfile.lock into the project',
    'clear-release': 'Clearing the retained release',
};

function runningSummary(context: StepContext, own: string): string {
    const operation = context.runningOperation ?? null;
    return (operation && runningSummaryByOperation[operation]) || own;
}

/** The guest reports "Xcode 26.6" or bare "26.6"; name the tool exactly once either way. */
function xcodeLabel(version: string | null | undefined): string {
    const value = (version ?? '').trim();
    if (!value) {
        return 'Xcode';
    }
    return value.toLowerCase().startsWith('xcode') ? value : `Xcode ${value}`;
}

/** What the unsigned build compiled against, in the words the interface uses. */
export const unsignedBuildTargetLabel: Record<UnsignedBuildTarget, string> = {
    device_sdk: 'device SDK',
    simulator: 'Simulator',
};

/**
 * The two compile targets of the unsigned build, as the one list every picker draws from. The
 * Simulator entry names the installed runtime when the guest has one, and the download otherwise.
 */
export function unsignedBuildTargetOptions(
    simulatorRuntime: string | null,
): { value: UnsignedBuildTarget; label: string; description: string }[] {
    return [
        {
            value: 'device_sdk',
            label: 'iOS device SDK',
            description: 'Recommended · installs the iOS platform only if Xcode requires it',
        },
        {
            value: 'simulator',
            label: 'iOS Simulator',
            description: simulatorRuntime
                ? `Runtime ${simulatorRuntime} installed · compiles only`
                : 'About 8 GB download · compiles only',
        },
    ];
}

/** `Built with Xcode 26.6 · device SDK`; records from before the choice existed name no target. */
function builtWith(workspace: {
    lastXcodeVersion: string | null;
    lastBuildTarget: UnsignedBuildTarget | null;
}): string {
    const base = `Built with ${xcodeLabel(workspace.lastXcodeVersion)}`;
    return workspace.lastBuildTarget
        ? `${base} · ${unsignedBuildTargetLabel[workspace.lastBuildTarget]}`
        : base;
}

/** Guest setup: everything that prepares one machine to build any project. */
export function deriveSetupSteps(view: MachineView, context: StepContext): SetupStep[] {
    // A clone boots the template's macOS and is bootstrapped from the template's key, so
    // the three steps a person does on a fresh install are BuildBridge's here.
    const template = view.template;
    const { runtime, guest } = view;
    const { ssh, diagnostics } = guest;
    const running = runtime.state === 'running';
    const hostReady = runtime.prerequisites.ready;
    const dockur = view.profile.provider === 'dockur_macos';
    const reachable = running && ssh.reachable;
    const trusted = reachable && ssh.trust === 'trusted';
    const authenticated = trusted && diagnostics.authenticated;
    const xcodeInstalled =
        authenticated && (diagnostics.xcodeVersion !== null || diagnostics.xcodeSelected);
    const xcodeActive = authenticated && diagnostics.xcodeSelected;
    const isRunning = (id: SetupStepId) => context.runningStep === id;

    const steps: SetupStep[] = [];

    steps.push({
        id: 'host',
        phase: 'setup',
        title: 'Check the Linux host',
        kind: 'automatic',
        status: hostReady ? 'done' : 'failed',
        summary: hostReady
            ? [
                  runtime.prerequisites.dockerVersion ?? 'Docker',
                  'KVM',
                  dockur
                      ? runtime.prerequisites.tunAccess
                          ? 'tun'
                          : null
                      : runtime.prerequisites.display
                        ? `display ${runtime.prerequisites.display}`
                        : null,
              ]
                  .filter(Boolean)
                  .join(' · ')
            : runtime.prerequisites.issues.join(' '),
    });

    const uptime = secondsSince(runtime.startedAt, context.now);
    steps.push({
        id: 'launch',
        phase: 'setup',
        title: 'Start the macOS machine',
        kind: 'automatic',
        status: isRunning('launch')
            ? 'running'
            : running
              ? 'done'
              : runtime.state === 'dead' || runtime.state === 'unavailable'
                ? 'failed'
                : hostReady
                  ? 'active'
                  : 'pending',
        summary: isRunning('launch')
            ? runningSummary(context, 'Creating and starting the container')
            : running
              ? uptime === null
                  ? 'Running'
                  : `Running for ${formatElapsed(uptime)}`
              : runtime.state === 'missing'
                ? hostReady
                    ? `Not created yet; the first start pulls the ${dockur ? 'dockur/macos' : 'Docker-OSX'} image`
                    : unlockedBy.launch
                : runtime.state === 'exited' || runtime.state === 'created'
                  ? 'Stopped; the macOS disk is retained and resumes on start'
                  : runtime.state === 'unavailable'
                    ? 'Docker is unavailable on this host'
                    : `Container is ${runtime.state}`,
    });

    steps.push({
        id: 'install',
        phase: 'setup',
        title: template ? 'Boot macOS from the template' : 'Install macOS in the console',
        kind: template ? 'automatic' : 'manual',
        status: reachable ? 'done' : running ? 'active' : 'pending',
        expected: template ? undefined : '30 to 60 min',
        summary: reachable
            ? diagnostics.macosVersion
                ? `macOS ${diagnostics.macosVersion} · Remote Login enabled`
                : 'Remote Login is reachable'
            : running
              ? template
                  ? `Starting the macOS saved in ${template.name}; nothing to install`
                  : ssh.portOpen
                    ? `Port ${view.profile.sshPort} is open; waiting for the guest SSH service`
                    : 'One-time: erase the disk, install macOS, create the account, enable Remote Login'
              : unlockedBy.install,
    });

    steps.push({
        id: 'trust',
        phase: 'setup',
        title: 'Pin the guest identity',
        kind: template ? 'automatic' : 'manual',
        status:
            reachable && ssh.trust === 'mismatch'
                ? 'failed'
                : trusted
                  ? 'done'
                  : reachable
                    ? 'active'
                    : 'pending',
        summary:
            reachable && ssh.trust === 'mismatch'
                ? 'The guest host key changed; forget the pin only if you rebuilt the machine'
                : trusted
                  ? (ssh.pinnedFingerprint ?? 'Fingerprint pinned')
                  : reachable
                    ? template
                        ? `Pinning the identity ${template.name} recorded, as soon as macOS answers`
                        : 'Pin the fingerprint below; compare it with the one macOS reports first if you share this host'
                    : unlockedBy.trust,
    });

    steps.push({
        id: 'access',
        phase: 'setup',
        title: 'Authorize the BuildBridge key',
        kind: template ? 'automatic' : 'assisted',
        status: authenticated ? 'done' : trusted ? 'active' : 'pending',
        summary: authenticated
            ? `Signed in as ${guest.username ?? 'the macOS user'} with a dedicated Ed25519 key`
            : trusted
              ? template
                  ? (diagnostics.issue ??
                    `Installing this machine's own key through ${template.name}'s`)
                  : guest.username
                    ? (diagnostics.issue ??
                      'Enter the macOS password once to install the key, or add it from the guest Terminal')
                    : 'Enter the macOS short username and password to install the access key'
              : unlockedBy.access,
    });

    steps.push({
        id: 'xcode-import',
        phase: 'setup',
        title: 'Import Xcode',
        kind: 'assisted',
        status: isRunning('xcode-import')
            ? 'running'
            : xcodeInstalled
              ? 'done'
              : authenticated
                ? 'active'
                : 'pending',
        summary: isRunning('xcode-import')
            ? 'Transferring and expanding the Xcode archive'
            : xcodeInstalled
              ? xcodeLabel(diagnostics.xcodeVersion)
              : authenticated
                ? 'Download it from Apple in the window BuildBridge opens; the import follows by itself'
                : unlockedBy['xcode-import'],
    });

    steps.push({
        id: 'xcode-activate',
        phase: 'setup',
        title: 'Activate Xcode',
        kind: 'assisted',
        status: isRunning('xcode-activate')
            ? 'running'
            : xcodeActive
              ? 'done'
              : xcodeInstalled
                ? 'active'
                : 'pending',
        summary: isRunning('xcode-activate')
            ? 'Waiting for the administrator password in the macOS Terminal'
            : xcodeActive
              ? `Selected at ${diagnostics.xcodePath ?? 'the guest Applications folder'}`
              : xcodeInstalled
                ? 'Accept the license and run first-launch tasks with the macOS password'
                : unlockedBy['xcode-activate'],
    });

    return steps;
}

/** Project build: everything that happens on an already prepared machine. */
export function deriveBuildSteps(view: MachineView, context: StepContext): BuildStep[] {
    const { appleWorkspace: workspace, signingKit, signing, archive } = view;
    const machineReady =
        view.runtime.state === 'running' &&
        view.guest.ssh.trust === 'trusted' &&
        view.guest.diagnostics.authenticated &&
        view.guest.diagnostics.xcodeSelected;
    const approved = workspace !== null;
    const synced = approved && workspace.lastSnapshotSha256 !== null;
    const built = synced && workspace.lastBuildSucceeded;
    const kitReady = signingKit !== null && kitIsProvisionable(signingKit);
    const provisioned = signing !== null;
    // The archive signs with the distribution identity; a development-only kit never unlocks it.
    const canArchive = signing?.distributionIdentity != null;
    const isRunning = (id: BuildStepId) => context.runningStep === id;

    const steps: BuildStep[] = [];

    steps.push({
        id: 'approve',
        phase: 'build',
        title: 'Approve the project folder',
        kind: 'manual',
        status: approved ? 'done' : machineReady ? 'active' : 'pending',
        summary: approved
            ? [workspace.name, workspace.developmentTeam, workspace.bundleIdentifier]
                  .filter(Boolean)
                  .join(' · ')
            : machineReady
              ? 'Choose the one host folder BuildBridge may read'
              : unlockedBy.approve,
    });

    steps.push({
        id: 'sync',
        phase: 'build',
        title: 'Synchronize source',
        kind: 'automatic',
        status: isRunning('sync') ? 'running' : synced ? 'done' : approved ? 'active' : 'pending',
        summary: isRunning('sync')
            ? 'Creating and transferring the bounded snapshot'
            : synced
              ? `${workspace.lastSyncFileCount ?? 0} files · ${formatBytes(workspace.lastSyncBytes)} · snapshot ${workspace.lastSnapshotSha256?.slice(0, 12) ?? ''}`
              : approved
                ? 'Copy a filtered, checksummed snapshot into the guest over the pinned bridge'
                : unlockedBy.sync,
    });

    steps.push({
        id: 'test-build',
        phase: 'build',
        title: 'Run the unsigned test build',
        kind: 'automatic',
        status: isRunning('test-build')
            ? 'running'
            : built
              ? 'done'
              : synced
                ? 'active'
                : 'pending',
        summary: isRunning('test-build')
            ? runningSummary(context, 'Preparing tools, dependencies, and the unsigned build')
            : built
              ? workspace.lastNativeLockUpdated
                  ? `${builtWith(workspace)} · the guest refreshed Podfile.lock`
                  : builtWith(workspace)
              : synced
                ? 'Compile the App scheme without signing, against the device SDK or the Simulator'
                : unlockedBy['test-build'],
    });

    const profileCount = signingKit?.provisioningProfileNames.length ?? 0;
    // A cleared keyring and a fresh install look identical unless the backend distinguishes
    // them, so these two states are failures with their own recovery wording.
    const credentialsLost =
        view.signingHealth === 'kit_missing' || view.signingHealth === 'vault_unavailable';
    steps.push({
        id: 'signing-kit',
        phase: 'build',
        title: 'Attach signing credentials',
        kind: 'manual',
        status: credentialsLost ? 'failed' : kitReady ? 'done' : built ? 'active' : 'pending',
        summary:
            view.signingHealth === 'vault_unavailable'
                ? `The credential vault could not be read: ${view.vaultIssue ?? 'unknown error'}`
                : view.signingHealth === 'kit_missing'
                  ? 'The credentials this machine was provisioned from are no longer stored. Store them again, then provision to rebuild the guest keychain.'
                  : kitReady
                    ? signingKit.signingCertificateConfigured
                        ? `${signingKit.name} · ${signingKit.signingCertificateName ?? 'certificate'} · ${profileCount} profile${profileCount === 1 ? '' : 's'}`
                        : kitReadiness(signingKit).archive === 'team_key'
                          ? `${signingKit.name} · Team key · distribution signing created when provisioned`
                          : `${signingKit.name} · development identity only · phone builds, no archive`
                    : signingKit === null
                      ? built
                          ? 'Attach signing credentials stored on this host, or store new ones'
                          : unlockedBy['signing-kit']
                      : `${signingKit.name} is incomplete: ${kitShortfall(signingKit).join(', ')} missing`,
    });

    steps.push({
        id: 'provision',
        phase: 'build',
        title: 'Provision signing into macOS',
        kind: 'automatic',
        status: isRunning('provision')
            ? 'running'
            : provisioned
              ? 'done'
              : built && kitReady
                ? 'active'
                : 'pending',
        summary: isRunning('provision')
            ? runningSummary(context, 'Importing the identity into a dedicated guest keychain')
            : provisioned
              ? signing.distributionIdentity
                  ? `${signing.distributionIdentity.identityName} · valid until ${signing.distributionIdentity.certificateExpiresAt.slice(0, 10)}`
                  : `${signing.developmentIdentity?.identityName ?? 'Development identity'} · development only · valid until ${signing.developmentIdentity?.certificateExpiresAt.slice(0, 10) ?? 'unknown'}`
              : built && kitReady
                ? 'Create a dedicated keychain and verify the identity with a real code-sign probe'
                : kitReady
                  ? 'after the test build passes'
                  : unlockedBy.provision,
    });

    steps.push({
        id: 'archive',
        phase: 'build',
        title: 'Build the signed archive and IPA',
        kind: 'automatic',
        status: isRunning('archive')
            ? 'running'
            : archive
              ? 'done'
              : view.archiveError
                ? 'failed'
                : canArchive && built && !credentialsLost
                  ? 'active'
                  : 'pending',
        summary: isRunning('archive')
            ? 'Archiving, exporting, verifying, and transferring artifacts'
            : archive
              ? `${archive.marketingVersion} (${archive.buildNumber}) · IPA ${formatBytes(archive.ipa.bytes)} · archive ${formatBytes(archive.archive.bytes)}`
              : view.archiveError
                ? 'The last signed build failed; the diagnostic is kept below'
                : credentialsLost
                  ? 'Blocked: the signing credentials are missing, and their keychain password is needed to sign'
                  : provisioned && !canArchive
                    ? 'Locked: the credentials hold only a development identity; add a distribution identity and an App Store profile, then provision again'
                    : provisioned && built
                      ? workspace?.lastNativeLockUpdated
                          ? 'Adopt the refreshed Podfile.lock into the project before signing, then commit it'
                          : [
                                formatVersion(view.projectVersion),
                                'Release configuration · App Store Connect export · app target only',
                            ]
                                .filter(Boolean)
                                .join(' · ')
                      : unlockedBy.archive,
    });

    // Off the golden path: a Debug build on a phone plugged into this host. It opens on the
    // archive's gate rather than on the archive, and a failed run outranks an earlier success
    // because the backend keeps both until the run is cleared.
    const readiness = deviceReadiness(view);
    const run = view.deviceRun;
    steps.push({
        id: 'run-device',
        phase: 'device',
        title: 'Run on the device',
        kind: 'assisted',
        optional: true,
        experimental: true,
        ...(isRunning('run-device') && context.runningLive ? { live: true } : {}),
        status: isRunning('run-device')
            ? 'running'
            : view.deviceRunError
              ? 'failed'
              : run
                ? 'done'
                : provisioned && built && !credentialsLost
                  ? 'active'
                  : 'pending',
        summary: isRunning('run-device')
            ? context.runningLive
                ? `Running on ${readiness.name}; console streaming`
                : deviceWorkingSummary(readiness)
            : view.deviceRunError
              ? 'The last device run failed; the diagnostic is kept below'
              : run
                ? `${run.device.name} · ${run.marketingVersion} (${run.buildNumber}) · installed ${relativeTime(new Date(run.installedAtEpochSeconds * 1000).toISOString(), context.now)}`
                : credentialsLost
                  ? 'Blocked: the signing credentials are missing, and their keychain password is needed to sign'
                  : provisioned && built
                    ? deviceNextSummary(readiness, view)
                    : unlockedBy['run-device'],
    });

    return steps;
}

/**
 * An Android machine's seven steps: the host and the container, then the project, the debug
 * build, the kit with the upload key, and the signed release.
 */
export function deriveAndroidSteps(view: MachineView, context: StepContext): AndroidStep[] {
    const { runtime, signingKit } = view;
    const android = view.android;
    const workspace = android?.workspace ?? null;
    const release = android?.release ?? null;
    const releaseError = android?.releaseError ?? null;
    const running = runtime.state === 'running';
    const hostReady = runtime.prerequisites.ready;
    const approved = workspace !== null;
    const synced = approved && workspace.lastSnapshotSha256 !== null;
    const built = synced && workspace.lastBuildSucceeded;
    const kitReady = signingKit !== null && kitSignsAndroid(signingKit);
    const credentialsLost =
        view.signingHealth === 'kit_missing' || view.signingHealth === 'vault_unavailable';
    const isRunning = (id: AndroidStepId) => context.runningStep === id;
    const lastBuild = workspace?.lastBuild ?? null;

    const steps: AndroidStep[] = [];

    steps.push({
        id: 'host',
        phase: 'setup',
        title: 'Check the host',
        kind: 'automatic',
        status: hostReady ? 'done' : 'failed',
        summary: hostReady
            ? `${runtime.prerequisites.dockerVersion ?? 'Docker'} · no virtual machine needed`
            : runtime.prerequisites.issues.join(' '),
    });

    const uptime = secondsSince(runtime.startedAt, context.now);
    steps.push({
        id: 'launch',
        phase: 'setup',
        title: 'Start the Android toolchain',
        kind: 'automatic',
        status: isRunning('launch')
            ? 'running'
            : running
              ? 'done'
              : runtime.state === 'dead' || runtime.state === 'unavailable'
                ? 'failed'
                : hostReady
                  ? 'active'
                  : 'pending',
        summary: isRunning('launch')
            ? runningSummary(context, 'Creating and starting the toolchain container')
            : running
              ? uptime === null
                  ? 'Running'
                  : `Running for ${formatElapsed(uptime)}`
              : runtime.state === 'missing'
                ? hostReady
                    ? 'Not created yet; the first start pulls the JDK image, a few hundred megabytes'
                    : unlockedBy.launch
                : runtime.state === 'exited' || runtime.state === 'created'
                  ? 'Stopped; the SDK and caches are kept on this host and resume on start'
                  : runtime.state === 'unavailable'
                    ? 'Docker is unavailable on this host'
                    : `Container is ${runtime.state}`,
    });

    steps.push({
        id: 'approve',
        phase: 'build',
        title: 'Approve the project folder',
        kind: 'manual',
        status: approved ? 'done' : running ? 'active' : 'pending',
        summary: approved
            ? [workspace.name, workspace.applicationId].filter(Boolean).join(' · ')
            : running
              ? 'Choose the one host folder BuildBridge may read: a Capacitor project with its Android platform'
              : 'after the toolchain starts',
    });

    steps.push({
        id: 'sync',
        phase: 'build',
        title: 'Synchronize source',
        kind: 'automatic',
        status: isRunning('sync') ? 'running' : synced ? 'done' : approved ? 'active' : 'pending',
        summary: isRunning('sync')
            ? 'Creating and transferring the bounded snapshot'
            : synced
              ? `${workspace.lastSyncFileCount ?? 0} files · ${formatBytes(workspace.lastSyncBytes)} · snapshot ${workspace.lastSnapshotSha256?.slice(0, 12) ?? ''}`
              : approved
                ? 'Copy a filtered, checksummed snapshot into the container'
                : unlockedBy.sync,
    });

    steps.push({
        id: 'test-build',
        phase: 'build',
        title: 'Run the debug build',
        kind: 'automatic',
        status: isRunning('test-build')
            ? 'running'
            : built
              ? 'done'
              : synced
                ? 'active'
                : 'pending',
        summary: isRunning('test-build')
            ? runningSummary(context, 'Preparing the toolchain, dependencies, and the debug APK')
            : built
              ? lastBuild
                  ? [
                        `${lastBuild.applicationId} ${lastBuild.versionName} (${lastBuild.versionCode})`,
                        lastBuild.apk ? `APK ${formatBytes(lastBuild.apk.bytes)}` : null,
                        lastBuild.toolchain.jdkVersion
                            .replace(/^openjdk version /, 'JDK ')
                            .replace(/"/g, ''),
                    ]
                        .filter(Boolean)
                        .join(' · ')
                  : 'Debug build passed'
              : synced
                ? 'Install the locked dependencies, build the web assets, and compile the debug APK with Gradle'
                : unlockedBy['test-build'],
    });

    steps.push({
        id: 'signing-kit',
        phase: 'build',
        title: 'Attach signing credentials',
        kind: 'manual',
        status: credentialsLost ? 'failed' : kitReady ? 'done' : built ? 'active' : 'pending',
        summary:
            view.signingHealth === 'vault_unavailable'
                ? `The credential vault could not be read: ${view.vaultIssue ?? 'unknown error'}`
                : view.signingHealth === 'kit_missing'
                  ? 'The credentials this machine used are no longer stored. Store them again and attach them here.'
                  : kitReady
                    ? `${signingKit.name} · ${signingKit.androidKeystoreName ?? 'keystore'} · key ${signingKit.androidKeyAlias ?? ''}`
                    : signingKit === null
                      ? built
                          ? 'Attach credentials holding an upload keystore, or create one on the Signing page'
                          : unlockedBy['signing-kit']
                      : `${signingKit.name} holds no upload key: ${androidKitShortfall(signingKit).join(', ')} missing`,
    });

    steps.push({
        id: 'release',
        phase: 'build',
        title: 'Build the signed release',
        kind: 'automatic',
        status: isRunning('release')
            ? 'running'
            : release
              ? 'done'
              : releaseError
                ? 'failed'
                : kitReady && built && !credentialsLost
                  ? 'active'
                  : 'pending',
        summary: isRunning('release')
            ? runningSummary(context, 'Bundling, signing, verifying, and transferring artifacts')
            : release
              ? `${release.versionName} (${release.versionCode}) · ${[release.aab ? `AAB ${formatBytes(release.aab.bytes)}` : null, release.apk ? `APK ${formatBytes(release.apk.bytes)}` : null].filter(Boolean).join(' · ')}`
              : releaseError
                ? 'The last signed release failed; the diagnostic is kept below'
                : credentialsLost
                  ? 'Blocked: the signing credentials are missing, and their upload key is needed to sign'
                  : kitReady && built
                    ? [
                          formatVersion(view.projectVersion),
                          'Choose an AAB for Google Play, an APK for direct install, or both',
                      ]
                          .filter(Boolean)
                          .join(' · ')
                    : unlockedBy.release,
    });

    return steps;
}

/**
 * Setup and build first, followed by optional device preview and publishing on both platforms.
 */
export function deriveJourney(view: MachineView, context: StepContext): JourneyStep[] {
    if (isAndroid(view.profile.provider)) {
        return [
            ...deriveAndroidSteps(view, context),
            deriveAndroidDeviceStep(androidDeviceApks(view.android).length > 0, context),
            derivePublishStep(!!view.android?.release),
        ];
    }
    return [
        ...deriveSetupSteps(view, context),
        ...deriveBuildSteps(view, context),
        derivePublishStep(!!view.archive),
    ];
}

/** Completion comes only from the retained APK's confirmed installation and launch. */
export function deriveAndroidDeviceStep(hasApk?: boolean, context?: StepContext): JourneyStep {
    const run = context?.androidDeviceRun;
    const running = context?.runningStep === 'run-device';
    return {
        id: 'run-device',
        phase: 'device',
        title: 'Run on an Android device',
        kind: 'automatic',
        optional: true,
        status: running
            ? 'running'
            : run?.status === 'complete'
              ? 'done'
              : run?.status === 'failed'
                ? 'failed'
                : hasApk
                  ? 'active'
                  : 'pending',
        summary: running
            ? 'Installing and opening the selected APK on Android'
            : run?.status === 'complete'
              ? `Installed and opened on ${run.serial}`
              : run?.status === 'failed'
                ? (run.error ?? 'Installation did not finish')
                : hasApk
                  ? 'Build and run fresh source, or install a retained APK on a connected device'
                  : hasApk === false
                    ? 'Connect a device, then build and run a debug APK'
                    : 'Open this machine to check for a retained APK and set up your device',
    };
}

/** Store upload is a separate optional handoff; a local file never proves publication. */
export function derivePublishStep(hasRelease: boolean): JourneyStep {
    return {
        id: 'publish',
        phase: 'publish',
        title: 'Publish the release',
        kind: 'assisted',
        status: hasRelease ? 'active' : 'pending',
        optional: true,
        summary: hasRelease
            ? 'Upload the release and finish in the store console; publication is not checked'
            : unlockedBy.publish,
    };
}

/** The coarse Android journey from the list row alone; the exact one replaces it once probed. */
function summarizeAndroidJourney(
    summary: MachineSummary,
    host: HostPrerequisites | null,
    runningStep: string | null,
): JourneyStep[] {
    const hostReady = providerHostReady(host, summary.config.provider);
    const live = isLive(summary.state);
    const dead = summary.state === 'dead' || summary.state === 'unavailable';
    const approved = summary.workspaceName !== null;
    const released = summary.archiveRetained;
    const kitReady = summary.signingProvisioned;
    const built = released || kitReady;
    const rows: Array<{
        id: AndroidStepId;
        phase: StepPhase;
        title: string;
        kind: StepKind;
        done: boolean;
        fact: string;
    }> = [
        {
            id: 'host',
            phase: 'setup',
            title: 'Check the host',
            kind: 'automatic',
            done: hostReady,
            fact: hostReady
                ? 'Docker ready'
                : host
                  ? providerHostIssues(host, summary.config.provider).join(' ')
                  : 'Checking host requirements',
        },
        {
            id: 'launch',
            phase: 'setup',
            title: 'Start the Android toolchain',
            kind: 'automatic',
            done: live,
            fact: live
                ? 'Running'
                : summary.state === 'missing'
                  ? 'Not created yet'
                  : dead
                    ? 'Docker is unavailable or the container died'
                    : 'Stopped; the SDK and caches are kept',
        },
        {
            id: 'approve',
            phase: 'build',
            title: 'Approve the project folder',
            kind: 'manual',
            done: approved,
            fact: summary.workspaceName ?? '',
        },
        {
            id: 'sync',
            phase: 'build',
            title: 'Synchronize source',
            kind: 'automatic',
            done: built,
            fact: 'Snapshot in the container',
        },
        {
            id: 'test-build',
            phase: 'build',
            title: 'Run the debug build',
            kind: 'automatic',
            done: built,
            fact: 'Debug build passed',
        },
        {
            id: 'signing-kit',
            phase: 'build',
            title: 'Attach signing credentials',
            kind: 'manual',
            done: built && kitReady,
            fact: summary.signingKitName ?? '',
        },
        {
            id: 'release',
            phase: 'build',
            title: 'Build the signed release',
            kind: 'automatic',
            done: released,
            fact: 'release retained',
        },
    ];

    let blocked = false;
    let activeSeen = false;
    return rows.map((row) => {
        let status: StepStatus;
        if (runningStep === row.id) {
            status = 'running';
        } else if (row.id === 'host' && !hostReady) {
            status = host ? 'failed' : 'pending';
            blocked = true;
        } else if (row.id === 'launch' && dead) {
            status = 'failed';
            blocked = true;
        } else if (row.done && !blocked) {
            status = 'done';
        } else if (!activeSeen && !blocked) {
            status = 'active';
            activeSeen = true;
        } else {
            status = 'pending';
        }
        return {
            id: row.id,
            phase: row.phase,
            title: row.title,
            kind: row.kind,
            status,
            summary:
                row.id === 'host' || status === 'done' || status === 'failed'
                    ? row.fact
                    : row.id === 'approve'
                      ? 'after the toolchain starts'
                      : unlockedBy[row.id],
        };
    });
}

/**
 * A coarse journey from the list summary alone, for the overview and the sidebar before a
 * machine has been probed. The flags only say what was reached, so steps between two reached
 * points are inferred; the exact model replaces it once the machine's view is loaded.
 */
function summarizeBuildJourney(
    summary: MachineSummary,
    host: HostPrerequisites | null,
    runningStep: string | null = null,
): JourneyStep[] {
    if (isAndroid(summary.config.provider)) {
        return summarizeAndroidJourney(summary, host, runningStep);
    }
    const hostReady = providerHostReady(host, summary.config.provider);
    const live = isLive(summary.state);
    const dead = summary.state === 'dead' || summary.state === 'unavailable';
    const approved = summary.workspaceName !== null;
    const trusted = approved || summary.trustPinned;
    const authenticated = approved || (trusted && summary.guestConfigured);
    const provisioned = summary.signingProvisioned;
    const archived = summary.archiveRetained;
    const built = provisioned || archived;
    const kitAttached = summary.signingKitName !== null;

    const rows: Array<{
        id: JourneyStepId;
        phase: StepPhase;
        title: string;
        kind: StepKind;
        done: boolean;
        fact: string;
        expected?: string;
        optional?: boolean;
        experimental?: boolean;
    }> = [
        {
            id: 'host',
            phase: 'setup',
            title: 'Check the Linux host',
            kind: 'automatic',
            done: hostReady,
            fact: hostReady
                ? summary.config.provider === 'dockur_macos'
                    ? 'Docker, KVM and tun ready'
                    : 'Docker, KVM and display ready'
                : host
                  ? providerHostIssues(host, summary.config.provider).join(' ')
                  : 'Checking host requirements',
        },
        {
            id: 'launch',
            phase: 'setup',
            title: 'Start the macOS machine',
            kind: 'automatic',
            done: live,
            fact: live
                ? 'Running'
                : summary.state === 'missing'
                  ? 'Not created yet'
                  : dead
                    ? 'Docker is unavailable or the container died'
                    : 'Stopped; the macOS disk is retained',
        },
        {
            id: 'install',
            phase: 'setup',
            title: 'Install macOS in the console',
            kind: 'manual',
            done: live && trusted,
            fact: 'macOS installed · Remote Login enabled',
            expected: '30 to 60 min',
        },
        {
            id: 'trust',
            phase: 'setup',
            title: 'Pin the guest identity',
            kind: 'manual',
            done: live && trusted,
            fact: 'Fingerprint pinned',
        },
        {
            id: 'access',
            phase: 'setup',
            title: 'Authorize the BuildBridge key',
            kind: 'assisted',
            done: live && authenticated,
            fact: 'Dedicated Ed25519 key authorized',
        },
        {
            id: 'xcode-import',
            phase: 'setup',
            title: 'Import Xcode',
            kind: 'assisted',
            done: live && approved,
            fact: 'Xcode installed',
        },
        {
            id: 'xcode-activate',
            phase: 'setup',
            title: 'Activate Xcode',
            kind: 'assisted',
            done: live && approved,
            fact: 'Xcode active',
        },
        {
            id: 'approve',
            phase: 'build',
            title: 'Approve the project folder',
            kind: 'manual',
            done: approved,
            fact: summary.workspaceName ?? '',
        },
        {
            id: 'sync',
            phase: 'build',
            title: 'Synchronize source',
            kind: 'automatic',
            done: built,
            fact: 'Snapshot in the guest',
        },
        {
            id: 'test-build',
            phase: 'build',
            title: 'Run the unsigned test build',
            kind: 'automatic',
            done: built,
            fact: 'Test build passed',
        },
        {
            id: 'signing-kit',
            phase: 'build',
            title: 'Attach signing credentials',
            kind: 'manual',
            done: built && kitAttached,
            fact: summary.signingKitName ?? '',
        },
        {
            id: 'provision',
            phase: 'build',
            title: 'Provision signing into macOS',
            kind: 'automatic',
            done: provisioned,
            fact: summary.signingIdentity ?? 'Signing provisioned',
        },
        {
            id: 'archive',
            phase: 'build',
            title: 'Build the signed archive and IPA',
            kind: 'automatic',
            done: archived,
            fact: 'IPA retained',
        },
        {
            id: 'run-device',
            phase: 'device',
            title: 'Run on the device',
            kind: 'assisted',
            done: summary.deviceRunRetained,
            fact: 'Ran on the iPhone',
            optional: true,
            experimental: true,
        },
    ];

    let blocked = false;
    let activeSeen = false;
    return rows.map((row) => {
        let status: StepStatus;
        if (runningStep === row.id) {
            status = 'running';
        } else if (row.id === 'host' && !hostReady) {
            status = host ? 'failed' : 'pending';
            blocked = true;
        } else if (row.id === 'launch' && dead) {
            status = 'failed';
            blocked = true;
        } else if (row.done && !blocked) {
            status = 'done';
        } else if (!activeSeen && !blocked) {
            status = 'active';
            activeSeen = true;
        } else {
            status = 'pending';
        }
        return {
            id: row.id,
            phase: row.phase,
            title: row.title,
            kind: row.kind,
            status,
            summary:
                row.id === 'host' || status === 'done' || status === 'failed'
                    ? row.fact
                    : unlockedBy[row.id],
            ...(row.expected ? { expected: row.expected } : {}),
            ...(row.optional ? { optional: true } : {}),
            ...(row.experimental ? { experimental: true } : {}),
        };
    });
}

export function summarizeJourney(
    summary: MachineSummary,
    host: HostPrerequisites | null,
    runningStep: string | null = null,
): JourneyStep[] {
    return [
        ...summarizeBuildJourney(summary, host, runningStep),
        ...(isAndroid(summary.config.provider) ? [deriveAndroidDeviceStep()] : []),
        derivePublishStep(summary.archiveRetained),
    ];
}

/**
 * The first step a person should look at: failed, then running, then active. An optional step
 * that is merely available is skipped, so the golden path keeps the headline; one that failed
 * or is running still needs someone.
 */
export function focusStep<Id extends string>(steps: Step<Id>[]): Step<Id> | null {
    return (
        steps.find((step) => step.status === 'failed') ??
        steps.find((step) => step.status === 'running') ??
        steps.find((step) => step.status === 'active' && !step.optional) ??
        null
    );
}

export function completedCount<Id extends string>(steps: Step<Id>[]): number {
    return steps.filter((step) => step.status === 'done').length;
}

/**
 * The steps the headline count is about: the ones a finished machine has to have done. An
 * optional step left undone is not an unfinished journey, so it is counted in its own section
 * and nowhere else.
 */
export function requiredSteps<Id extends string>(steps: Step<Id>[]): Step<Id>[] {
    return steps.filter((step) => !step.optional);
}

export interface PhaseGroup<Id extends string> {
    phase: StepPhase;
    label: string;
    steps: Step<Id>[];
    done: number;
    complete: boolean;
    focus: Step<Id> | null;
}

/** The journey split into its phases, in order, skipping any phase with no steps. */
export function groupByPhase<Id extends string>(steps: Step<Id>[]): PhaseGroup<Id>[] {
    const phases: StepPhase[] = ['setup', 'build', 'device', 'publish'];
    return phases
        .map((phase) => {
            const own = steps.filter((step) => step.phase === phase);
            const done = completedCount(own);
            return {
                phase,
                label: phaseLabel[phase],
                steps: own,
                done,
                complete: own.length > 0 && done === own.length,
                focus: focusStep(own),
            };
        })
        .filter((group) => group.steps.length > 0);
}

/**
 * One short line that says where a machine is in its journey, for the sidebar and the
 * overview: what needs attention, what is running, or what comes next.
 */
export function journeyHeadline(steps: JourneyStep[]): string {
    const focus = focusStep(steps);
    if (!focus) {
        const signed = steps.find((step) => step.id === 'archive' || step.id === 'release');
        if (signed?.status === 'done') {
            return `signed ${signed.summary.split(' · ')[0]}`;
        }
        const remaining = requiredSteps(steps).find((step) => step.status !== 'done');
        if (remaining) {
            const device = steps.find((step) => step.id === 'run-device');
            if (
                remaining.id === 'archive' &&
                (device?.status === 'active' || device?.status === 'done')
            ) {
                return 'device builds ready · release signing needed';
            }
            return `${stepShortTitle[remaining.id]} is not ready`;
        }
        return steps.length ? 'all steps done' : '';
    }
    const short = stepShortTitle[focus.id];
    switch (focus.status) {
        case 'failed':
            return `${short} needs attention`;
        case 'running':
            return `running: ${short}`;
        default:
            return `next: ${short}`;
    }
}
