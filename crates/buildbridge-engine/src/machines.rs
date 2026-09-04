//! Registry of the managed macOS machines this desktop owns, plus their per-machine storage.
//!
//! The first BuildBridge release managed exactly one builder whose files lived directly under
//! `macos-builder/` with a hard-coded container name. That machine is migrated into the
//! registry as [`DEFAULT_MACHINE_ID`] and keeps its original directory and container so an
//! existing installation keeps working. Every newer machine gets its own directory under
//! `machines/<id>/` and a container named after its identifier.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use buildbridge_docker_osx::{DEFAULT_MACHINE_ID, MacBuilderConfig, container_name};
use serde::{Deserialize, Serialize};

use crate::Engine;
use ts_rs::TS;

const REGISTRY_FILE: &str = "machines.json";
const LEGACY_CONFIG_FILE: &str = "mac-builder.json";
const LEGACY_DIRECTORY: &str = "macos-builder";
const MACHINES_DIRECTORY: &str = "machines";

/// Upper bound on registered machines; each one publishes a host port and owns a large disk.
pub(crate) const MAX_MACHINES: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StoredMachine {
    pub id: String,
    pub config: MacBuilderConfig,
    #[ts(type = "number")]
    pub created_at_epoch_seconds: u64,
    /// The signing kit this machine provisions. `None` falls back to the sole kit, if there is
    /// exactly one, so a single-kit host needs no attachment step.
    #[serde(default)]
    pub signing_kit_id: Option<String>,
    /// The env set written into this machine's guest workspace at every sync. Unlike a signing
    /// kit there is no implicit fallback: a build gets exactly the variables it was attached to.
    #[serde(default)]
    pub env_set_id: Option<String>,
    /// The template this machine's disk is an overlay of; its container binds the template
    /// directory read-only, and the template cannot be deleted while this machine exists.
    #[serde(default)]
    pub template_id: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct MachineRegistry {
    #[serde(default)]
    pub machines: Vec<StoredMachine>,
}

impl MachineRegistry {
    pub fn find(&self, machine_id: &str) -> Result<&StoredMachine, String> {
        self.machines
            .iter()
            .find(|machine| machine.id == machine_id)
            .ok_or_else(|| "This machine is no longer registered.".to_string())
    }

    pub fn position(&self, machine_id: &str) -> Result<usize, String> {
        self.machines
            .iter()
            .position(|machine| machine.id == machine_id)
            .ok_or_else(|| "This machine is no longer registered.".to_string())
    }

    /// Rejects a profile whose forwarded SSH port is already published by another machine.
    pub fn ensure_unique_ssh_port(
        &self,
        profile: &MacBuilderConfig,
        except_machine_id: Option<&str>,
    ) -> Result<(), String> {
        let conflict = self.machines.iter().find(|machine| {
            Some(machine.id.as_str()) != except_machine_id
                && machine.config.ssh_port == profile.ssh_port
        });

        match conflict {
            Some(machine) => Err(format!(
                "SSH port {} is already used by {}. Choose a different port.",
                profile.ssh_port, machine.config.name
            )),
            None => Ok(()),
        }
    }
}

/// Storage locations owned by one machine.
#[derive(Debug, Clone)]
pub struct MachinePaths {
    pub id: String,
    pub container_name: String,
    config_dir: PathBuf,
    data_dir: PathBuf,
}

impl MachinePaths {
    pub fn resolve(app: &Engine, machine_id: &str) -> Result<Self, String> {
        if !buildbridge_docker_osx::valid_machine_id(machine_id) {
            return Err("The machine identifier is invalid.".to_string());
        }

        let config_root = app.config_dir();
        let data_root = app.data_dir();
        let (config_dir, data_dir) = if machine_id == DEFAULT_MACHINE_ID {
            (
                config_root.join(LEGACY_DIRECTORY),
                data_root.join(LEGACY_DIRECTORY),
            )
        } else {
            (
                config_root.join(MACHINES_DIRECTORY).join(machine_id),
                data_root.join(MACHINES_DIRECTORY).join(machine_id),
            )
        };

        Ok(Self {
            id: machine_id.to_string(),
            container_name: container_name(machine_id),
            config_dir,
            data_dir,
        })
    }

    pub fn identity(&self) -> PathBuf {
        self.config_dir.join("identity.env")
    }

    /// Held by the process running an operation on this machine, so another BuildBridge
    /// process sees it busy.
    pub fn operation_lock(&self) -> PathBuf {
        self.data_dir.join("operation.lock")
    }

    pub fn guest_access(&self) -> PathBuf {
        self.config_dir.join("guest.json")
    }

    pub fn guest_identity(&self) -> PathBuf {
        self.config_dir.join("guest_ed25519")
    }

    pub fn guest_public_key(&self) -> PathBuf {
        self.config_dir.join("guest_ed25519.pub")
    }

    pub fn known_hosts(&self) -> PathBuf {
        self.config_dir.join("known_hosts")
    }

    pub fn apple_workspace(&self) -> PathBuf {
        self.config_dir.join("apple-workspace.json")
    }

    pub fn signing_provisioning(&self) -> PathBuf {
        self.config_dir.join("signing.json")
    }

    pub fn apple_archive_record(&self) -> PathBuf {
        self.config_dir.join("archive.json")
    }

    pub fn apple_archive_error(&self) -> PathBuf {
        self.config_dir.join("archive-error.txt")
    }

    pub fn artifacts_dir(&self) -> PathBuf {
        self.data_dir.join("artifacts")
    }

    /// The macOS disk, its NVRAM and, once migrated, its install media: what the container is
    /// created around, so the container itself can be recreated at will.
    pub fn disk_dir(&self) -> PathBuf {
        self.data_dir.join("disk")
    }

    /// Where QEMU creates the control socket the desktop uses to hand it USB devices.
    pub fn qmp_dir(&self) -> PathBuf {
        self.data_dir.join("qmp")
    }

    pub fn qmp_socket(&self) -> PathBuf {
        self.qmp_dir().join(buildbridge_docker_osx::QMP_SOCKET_NAME)
    }

    /// Scratch for files handed to a privileged host command, such as the USB udev rule.
    pub fn usb_staging_dir(&self) -> PathBuf {
        self.data_dir.join("usb")
    }

    pub fn apple_device_run_record(&self) -> PathBuf {
        self.config_dir.join("device-run.json")
    }

    pub fn apple_device_run_error(&self) -> PathBuf {
        self.config_dir.join("device-run-error.txt")
    }

    /// The host project's Podfile.lock as it was before the guest's copy was adopted.
    pub fn podfile_lock_backup(&self) -> PathBuf {
        self.config_dir.join("Podfile.lock.previous")
    }

    /// Removes the disk and the control directory: everything the container was built
    /// around. Discarding a container discards its macOS, so this goes with it.
    pub fn remove_container_storage(&self) -> Result<(), String> {
        remove_dir_all_if_present(&self.disk_dir())?;
        remove_dir_all_if_present(&self.qmp_dir())
    }

    /// Where a remote build checks out a revision of the approved project. One directory per
    /// machine, reused across builds so a fetch is incremental.
    pub fn checkout_dir(&self) -> PathBuf {
        self.data_dir.join("sources").join("checkout")
    }

    /// Removes every file this machine owns without touching host-level signing material.
    ///
    /// The legacy machine shares its directory with the managed Apple profile store, so files
    /// are removed individually and the directory is only pruned once it is empty.
    pub fn remove_machine_files(&self) -> Result<(), String> {
        for path in [
            self.identity(),
            self.guest_access(),
            self.guest_identity(),
            self.guest_public_key(),
            self.known_hosts(),
            self.apple_workspace(),
            self.signing_provisioning(),
            self.apple_archive_record(),
            self.apple_archive_error(),
            self.apple_device_run_record(),
            self.apple_device_run_error(),
            self.operation_lock(),
        ] {
            remove_file_if_present(&path)?;
        }
        self.remove_container_storage()?;
        remove_dir_all_if_present(&self.usb_staging_dir())?;
        remove_dir_all_if_present(&self.artifacts_dir())?;
        remove_dir_all_if_present(&self.data_dir.join("sources"))?;
        let _ = fs::remove_dir(&self.data_dir);
        let _ = fs::remove_dir(&self.config_dir);

        Ok(())
    }
}

fn remove_file_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not remove {}: {error}", path.display())),
    }
}

fn remove_dir_all_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not remove {}: {error}", path.display())),
    }
}

fn registry_path(app: &Engine) -> Result<PathBuf, String> {
    Ok(app.config_dir().join(REGISTRY_FILE))
}

/// Loads the registry, migrating the pre-registry single builder on first use.
pub(crate) fn load_registry(app: &Engine) -> Result<MachineRegistry, String> {
    let path = registry_path(app)?;

    match fs::read(&path) {
        Ok(bytes) => {
            let registry: MachineRegistry = serde_json::from_slice(&bytes)
                .map_err(|error| format!("The machine registry is invalid: {error}"))?;
            for machine in &registry.machines {
                if !buildbridge_docker_osx::valid_machine_id(&machine.id) {
                    return Err(format!(
                        "The machine registry contains an invalid identifier: {}",
                        machine.id
                    ));
                }
            }

            Ok(registry)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => migrate_legacy_builder(app),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn save_registry(app: &Engine, registry: &MachineRegistry) -> Result<(), String> {
    let path = registry_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "The machine registry directory is unavailable.".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    fs::write(
        path,
        serde_json::to_vec_pretty(registry).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn migrate_legacy_builder(app: &Engine) -> Result<MachineRegistry, String> {
    let legacy_path = app.config_dir().join(LEGACY_CONFIG_FILE);
    let config: MacBuilderConfig = match fs::read(&legacy_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            format!("The legacy macOS builder configuration is invalid: {error}")
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MachineRegistry::default());
        }
        Err(error) => return Err(error.to_string()),
    };
    let created_at_epoch_seconds = fs::metadata(&legacy_path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_else(now_epoch_seconds);
    let registry = MachineRegistry {
        machines: vec![StoredMachine {
            id: DEFAULT_MACHINE_ID.to_string(),
            config,
            created_at_epoch_seconds,
            signing_kit_id: None,
            env_set_id: None,
            template_id: None,
        }],
    };
    save_registry(app, &registry)?;
    remove_file_if_present(&legacy_path)?;

    Ok(registry)
}

pub(crate) fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// Derives a stable, unique identifier from a display name.
pub(crate) fn machine_id_from_name(name: &str, existing: &[&str]) -> String {
    let mut slug = String::new();
    let mut previous_dash = true;
    for character in name.trim().chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
        if slug.len() >= 32 {
            break;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    let base = if slug.is_empty() || slug == DEFAULT_MACHINE_ID {
        "machine".to_string()
    } else {
        slug
    };

    if !existing.contains(&base.as_str()) {
        return base;
    }
    (2..)
        .map(|suffix| format!("{base}-{suffix}"))
        .find(|candidate| !existing.contains(&candidate.as_str()))
        .expect("an unused machine identifier always exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_identifiers_are_derived_from_names_and_stay_unique() {
        assert_eq!(machine_id_from_name("Team Mac", &[]), "team-mac");
        assert_eq!(
            machine_id_from_name("  Xcode 26 -- CI  ", &[]),
            "xcode-26-ci"
        );
        assert_eq!(
            machine_id_from_name("Team Mac", &["team-mac"]),
            "team-mac-2"
        );
        assert_eq!(
            machine_id_from_name("Team Mac", &["team-mac", "team-mac-2"]),
            "team-mac-3"
        );
        assert_eq!(machine_id_from_name("默认", &[]), "machine");
        assert_eq!(machine_id_from_name("default", &[]), "machine");
        assert!(buildbridge_docker_osx::valid_machine_id(
            &machine_id_from_name(
                &"very long machine name that keeps going and going".repeat(2),
                &[]
            )
        ));
    }

    #[test]
    fn a_registry_written_before_signing_kits_loads_with_no_attachment() {
        // Registries written by the previous release have no `signingKitId`; they must load
        // rather than fail, leaving the machine to fall back to the host's only kit.
        let registry: MachineRegistry = serde_json::from_value(serde_json::json!({
            "machines": [{
                "id": "default",
                "config": {
                    "name": "macOS builder",
                    "macosRelease": "sequoia",
                    "memoryGib": 8,
                    "cpuCores": 4,
                    "sshPort": 50922
                },
                "createdAtEpochSeconds": 1
            }]
        }))
        .expect("an older registry should remain readable");

        assert_eq!(registry.machines.len(), 1);
        assert_eq!(registry.machines[0].signing_kit_id, None);
    }

    #[test]
    fn duplicate_ssh_ports_are_rejected_except_for_the_machine_being_edited() {
        let registry = MachineRegistry {
            machines: vec![StoredMachine {
                id: "default".to_string(),
                config: MacBuilderConfig::default(),
                created_at_epoch_seconds: 0,
                signing_kit_id: None,
                env_set_id: None,
                template_id: None,
            }],
        };

        assert!(
            registry
                .ensure_unique_ssh_port(&MacBuilderConfig::default(), None)
                .is_err()
        );
        assert!(
            registry
                .ensure_unique_ssh_port(&MacBuilderConfig::default(), Some("default"))
                .is_ok()
        );
        assert!(
            registry
                .ensure_unique_ssh_port(
                    &MacBuilderConfig {
                        ssh_port: 50923,
                        ..MacBuilderConfig::default()
                    },
                    None
                )
                .is_ok()
        );
    }
}
