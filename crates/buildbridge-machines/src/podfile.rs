//! Podfile.lock: reading the guest's copy and diffing it against the project's.

use super::*;
use ts_rs::TS;

/// A Podfile.lock is small; anything past this is not one.
pub const PODFILE_LOCK_MAX_BYTES: usize = 1024 * 1024;

/// One pod whose pinned version differs between two lockfiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PodfileLockChange {
    pub name: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PodfileLockChanges {
    pub pods: Vec<PodfileLockChange>,
    #[ts(type = "number")]
    pub lines_added: usize,
    #[ts(type = "number")]
    pub lines_removed: usize,
    pub identical: bool,
}

/// The Podfile.lock CocoaPods wrote in the guest workspace, bounded and checked for the shape
/// of a lockfile before the host adopts it.
pub fn read_guest_podfile_lock(
    ssh_port: u16,
    username: &str,
    identity_path: &Path,
    known_hosts_path: &Path,
) -> Result<String, ProviderError> {
    validate_guest_operation(ssh_port, username, identity_path, known_hosts_path)?;
    let lock = format!("/Users/{username}/BuildBridge/workspaces/active/ios/App/Podfile.lock");
    let content = device_run::run_guest_command_capped(
        ssh_port,
        username,
        identity_path,
        known_hosts_path,
        &format!("/bin/cat {}", shell_single_quote(&lock)),
        PODFILE_LOCK_MAX_BYTES,
    )?;
    validate_podfile_lock(&content)?;
    Ok(content)
}

/// Non-empty, bounded, printable, and shaped like CocoaPods wrote it: a `PODS:` section first
/// and the `COCOAPODS:` version line somewhere after.
pub(crate) fn validate_podfile_lock(content: &str) -> Result<(), ProviderError> {
    let printable = content
        .chars()
        .all(|character| !character.is_control() || matches!(character, '\n' | '\t' | '\r'));
    if content.is_empty()
        || content.len() > PODFILE_LOCK_MAX_BYTES
        || !printable
        || !content.starts_with("PODS:")
        || !content.contains("\nCOCOAPODS: ")
    {
        return Err(ProviderError::GuestBridge(
            "the guest workspace holds no readable Podfile.lock".to_string(),
        ));
    }
    Ok(())
}

/// The top-level pods of a lockfile's `PODS:` section — `  - Name (version)` — by name.
pub(crate) fn podfile_lock_pods(content: &str) -> Vec<(String, String)> {
    let mut pods = Vec::new();
    let mut in_pods = false;
    for line in content.lines() {
        if line == "PODS:" {
            in_pods = true;
            continue;
        }
        if in_pods && !line.starts_with(' ') {
            break;
        }
        let Some(entry) = line.strip_prefix("  - ") else {
            continue;
        };
        if line.starts_with("    ") {
            continue;
        }
        let entry = entry.trim_end_matches(':');
        let Some((name, rest)) = entry.split_once(" (") else {
            continue;
        };
        let Some(version) = rest.strip_suffix(')') else {
            continue;
        };
        pods.push((name.to_string(), version.to_string()));
    }
    pods
}

/// What adopting `after` over `before` changes: each pod whose pin differs, and the raw line
/// counts either way, so the change can be judged before it is committed.
pub fn podfile_lock_changes(before: &str, after: &str) -> PodfileLockChanges {
    if before == after {
        return PodfileLockChanges {
            identical: true,
            ..PodfileLockChanges::default()
        };
    }
    let before_pods = podfile_lock_pods(before);
    let after_pods = podfile_lock_pods(after);
    let mut pods = Vec::new();
    for (name, version) in &before_pods {
        match after_pods.iter().find(|(other, _)| other == name) {
            Some((_, after_version)) if after_version == version => {}
            Some((_, after_version)) => pods.push(PodfileLockChange {
                name: name.clone(),
                before: Some(version.clone()),
                after: Some(after_version.clone()),
            }),
            None => pods.push(PodfileLockChange {
                name: name.clone(),
                before: Some(version.clone()),
                after: None,
            }),
        }
    }
    for (name, version) in &after_pods {
        if !before_pods.iter().any(|(other, _)| other == name) {
            pods.push(PodfileLockChange {
                name: name.clone(),
                before: None,
                after: Some(version.clone()),
            });
        }
    }

    let mut counts: HashMap<&str, i64> = HashMap::new();
    for line in before.lines() {
        *counts.entry(line).or_default() += 1;
    }
    for line in after.lines() {
        *counts.entry(line).or_default() -= 1;
    }
    let lines_removed = counts.values().filter(|count| **count > 0).sum::<i64>() as usize;
    let lines_added = counts
        .values()
        .filter(|count| **count < 0)
        .map(|count| -count)
        .sum::<i64>() as usize;

    PodfileLockChanges {
        pods,
        lines_added,
        lines_removed,
        identical: false,
    }
}
