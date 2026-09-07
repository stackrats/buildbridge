// The saved snapshot in one line, wherever a build offers to reuse it instead of copying the
// folder again: which copy it is, how old, how big, and the environment written with it.
import { formatBytes, relativeTime } from '../lib/format';
import type { MachineView } from '../types/backend';
import { projectWorkspace } from './build-flow';

export function describeSnapshot(view: MachineView, now = Date.now()): string | null {
    const workspace = projectWorkspace(view);
    if (!workspace?.lastSnapshotSha256) {
        return null;
    }
    const source = workspace.lastSource;
    return [
        `snapshot ${workspace.lastSnapshotSha256.slice(0, 12)}`,
        workspace.lastSyncedAtEpochSeconds
            ? `copied ${relativeTime(new Date(workspace.lastSyncedAtEpochSeconds * 1000).toISOString(), now)}`
            : null,
        source?.kind === 'git' && source.commit
            ? `${source.gitRef ?? 'git'} at ${source.commit.slice(0, 12)}`
            : null,
        workspace.lastSyncFileCount === null ? null : `${workspace.lastSyncFileCount} files`,
        workspace.lastSyncBytes === null ? null : formatBytes(workspace.lastSyncBytes),
        view.envSet ? `with ${view.envSet.name}` : 'project configuration only',
    ]
        .filter(Boolean)
        .join(' · ');
}
