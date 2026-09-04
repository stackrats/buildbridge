//! Machine templates: a prepared machine saved once on this host, new machines cloned from it
//! in seconds, and a clone made reachable through the template's key on its first boot.
//!
//! A template keeps three things beside the disk files the provider writes: the source
//! machine's guest access key (owner-only, because it opens every clone until the clone's
//! own key replaces it), the guest's pinned host key, and the guest username. With those a
//! clone needs no console, no fingerprint comparison and no password: BuildBridge pins the
//! identity it already knows, installs the clone's own key through the template's, retires
//! the template's, and the journey resumes at the first project step.

use super::*;

const TEMPLATES_DIRECTORY: &str = "templates";
const TEMPLATE_RECORD_FILE: &str = "template.json";
const MAX_TEMPLATE_NAME_CHARS: usize = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredMachineTemplate {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) created_at_epoch_seconds: u64,
    pub(crate) source_machine_name: String,
    pub(crate) macos_version: Option<String>,
    pub(crate) xcode_version: Option<String>,
    pub(crate) guest_username: String,
    /// The guest's SSH host key as pinned on the source machine. A clone boots with the same
    /// key, which is how it is recognised.
    pub(crate) known_hosts_line: String,
    pub(crate) size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MachineTemplateSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) created_at_epoch_seconds: u64,
    pub(crate) source_machine_name: String,
    pub(crate) macos_version: Option<String>,
    pub(crate) xcode_version: Option<String>,
    pub(crate) size_bytes: u64,
    /// Machines cloned from it. While any exists the template cannot be deleted, since their
    /// disks read through it.
    pub(crate) machine_names: Vec<String>,
    /// The files are all present; a save that stopped halfway leaves this false.
    pub(crate) ready: bool,
}

/// The template a machine was cloned from, as the machine view names it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MachineTemplateRef {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveTemplateInput {
    name: String,
    confirmed: bool,
}

/// Where one template lives: its record and key material under the config directory, its
/// disk files under the data directory, both named by the template id.
pub(crate) struct TemplatePaths {
    config_dir: PathBuf,
    data_dir: PathBuf,
}

impl TemplatePaths {
    pub(crate) fn resolve(app: &AppHandle, template_id: &str) -> Result<Self, String> {
        if !buildbridge_docker_osx::valid_machine_id(template_id) {
            return Err("The template identifier is invalid.".to_string());
        }
        Ok(Self {
            config_dir: templates_config_root(app)?.join(template_id),
            data_dir: app
                .path()
                .app_local_data_dir()
                .map_err(|error| error.to_string())?
                .join(TEMPLATES_DIRECTORY)
                .join(template_id),
        })
    }

    pub(crate) fn record(&self) -> PathBuf {
        self.config_dir.join(TEMPLATE_RECORD_FILE)
    }

    pub(crate) fn guest_identity(&self) -> PathBuf {
        self.config_dir.join("guest_ed25519")
    }

    pub(crate) fn guest_public_key(&self) -> PathBuf {
        self.config_dir.join("guest_ed25519.pub")
    }

    /// The directory the provider fills and every clone's container binds read-only.
    pub(crate) fn files_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    fn remove(&self) -> Result<(), String> {
        for dir in [&self.data_dir, &self.config_dir] {
            match fs::remove_dir_all(dir) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("Could not remove {}: {error}", dir.display())),
            }
        }
        Ok(())
    }
}

fn templates_config_root(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join(TEMPLATES_DIRECTORY))
        .map_err(|error| error.to_string())
}

pub(crate) fn load_template(
    app: &AppHandle,
    template_id: &str,
) -> Result<Option<StoredMachineTemplate>, String> {
    let paths = TemplatePaths::resolve(app, template_id)?;
    match fs::read(paths.record()) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("The template record is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Could not read the template record: {error}")),
    }
}

fn save_template_record(app: &AppHandle, template: &StoredMachineTemplate) -> Result<(), String> {
    let paths = TemplatePaths::resolve(app, &template.id)?;
    let encoded = serde_json::to_vec_pretty(template).map_err(|error| error.to_string())?;
    write_restricted_file(&paths.record(), &encoded)
}

/// Every stored template, newest first.
pub(crate) fn list_templates(app: &AppHandle) -> Result<Vec<StoredMachineTemplate>, String> {
    let root = templates_config_root(app)?;
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("Could not read the templates directory: {error}")),
    };
    let mut templates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let Some(id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !buildbridge_docker_osx::valid_machine_id(&id) {
            continue;
        }
        if let Some(template) = load_template(app, &id)? {
            templates.push(template);
        }
    }
    templates.sort_by_key(|template| std::cmp::Reverse(template.created_at_epoch_seconds));
    Ok(templates)
}

/// The template directory a machine's container binds, when the machine was cloned from one.
/// A machine whose template is gone cannot start: its disk reads through the template.
pub(crate) fn template_dir_for(
    app: &AppHandle,
    machine_id: &str,
) -> Result<Option<PathBuf>, String> {
    let machine = machines::load_registry(app)?.find(machine_id)?.clone();
    let Some(template_id) = machine.template_id else {
        return Ok(None);
    };
    if load_template(app, &template_id)?.is_none() {
        return Err(format!(
            "This machine was cloned from a template that is no longer stored ({template_id}); its disk cannot be read without it."
        ));
    }
    Ok(Some(TemplatePaths::resolve(app, &template_id)?.files_dir()))
}

pub(crate) fn template_name(app: &AppHandle, template_id: &str) -> Option<String> {
    load_template(app, template_id)
        .ok()
        .flatten()
        .map(|template| template.name)
}

pub(crate) fn template_ref_for(app: &AppHandle, machine_id: &str) -> Option<MachineTemplateRef> {
    let machine = machines::load_registry(app)
        .ok()?
        .find(machine_id)
        .ok()?
        .clone();
    let id = machine.template_id?;
    let name = template_name(app, &id)?;
    Some(MachineTemplateRef { id, name })
}

fn summarize_template(
    app: &AppHandle,
    registry: &machines::MachineRegistry,
    template: &StoredMachineTemplate,
) -> Result<MachineTemplateSummary, String> {
    let paths = TemplatePaths::resolve(app, &template.id)?;
    let files = buildbridge_docker_osx::MachineTemplateFiles::new(&paths.files_dir())
        .map_err(|error| error.to_string())?;
    Ok(MachineTemplateSummary {
        id: template.id.clone(),
        name: template.name.clone(),
        created_at_epoch_seconds: template.created_at_epoch_seconds,
        source_machine_name: template.source_machine_name.clone(),
        macos_version: template.macos_version.clone(),
        xcode_version: template.xcode_version.clone(),
        size_bytes: template.size_bytes,
        machine_names: registry
            .machines
            .iter()
            .filter(|machine| machine.template_id.as_deref() == Some(&template.id))
            .map(|machine| machine.config.name.clone())
            .collect(),
        ready: files.ready(),
    })
}

fn summarize_templates(app: &AppHandle) -> Result<Vec<MachineTemplateSummary>, String> {
    let registry = machines::load_registry(app)?;
    list_templates(app)?
        .iter()
        .map(|template| summarize_template(app, &registry, template))
        .collect()
}

fn validate_template_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the template a name.".to_string());
    }
    if name.chars().count() > MAX_TEMPLATE_NAME_CHARS {
        return Err(format!(
            "Keep the template name to {MAX_TEMPLATE_NAME_CHARS} characters."
        ));
    }
    if name.chars().any(char::is_control) {
        return Err("The template name cannot contain control characters.".to_string());
    }
    Ok(name.to_string())
}

#[tauri::command]
pub(crate) async fn list_machine_templates(
    app: AppHandle,
) -> Result<Vec<MachineTemplateSummary>, String> {
    summarize_templates(&app)
}

/// Saves a machine as a template. macOS is shut down for a consistent copy and the machine
/// stays stopped; the compressed copy takes minutes and reports its progress.
#[tauri::command]
pub(crate) async fn save_machine_template(
    app: AppHandle,
    machine_id: String,
    input: SaveTemplateInput,
) -> Result<MachineTemplateSummary, String> {
    if !input.confirmed {
        return Err("Confirm saving the template before continuing.".to_string());
    }
    let name = validate_template_name(&input.name)?;
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let registry = machines::load_registry(&app)?;
    let machine = registry.find(&machine_id)?.clone();
    if !paths.known_hosts().is_file() {
        return Err(
            "Pin the guest identity first; a template carries it so clones need no comparison."
                .to_string(),
        );
    }
    let access = load_mac_guest_access(&paths)?.ok_or_else(|| {
        "Authorize the BuildBridge key first; a template carries it so clones need no password."
            .to_string()
    })?;
    if !paths.guest_identity().is_file() {
        return Err("This machine has no guest access key to carry into the template.".to_string());
    }
    let public_key = read_mac_guest_public_key(&paths.guest_public_key())?;
    let source_dir = paths.disk_dir();
    if !source_dir
        .join(buildbridge_docker_osx::DISK_IMAGE_NAME)
        .is_file()
    {
        return Err(
            "This machine keeps its macOS disk inside the container. Enable USB on this machine first: that moves the disk to this host, which a template needs."
                .to_string(),
        );
    }
    let source_template_dir = template_dir_for(&app, &machine_id)?;
    let known_hosts_line = fs::read_to_string(paths.known_hosts())
        .map_err(|error| format!("Could not read the pinned identity: {error}"))?
        .trim()
        .to_string();
    // Read while the guest may still be up; the copy needs it stopped.
    let current = build_mac_builder_view(&app, &paths).await?;
    let diagnostics = current.guest.diagnostics;

    let existing = list_templates(&app)?;
    let existing_ids = existing
        .iter()
        .map(|template| template.id.as_str())
        .collect::<Vec<_>>();
    let id = machines::machine_id_from_name(&name, &existing_ids);
    let template_paths = TemplatePaths::resolve(&app, &id)?;
    let private_key = fs::read(paths.guest_identity())
        .map_err(|error| format!("Could not read the guest access key: {error}"))?;
    write_restricted_file(&template_paths.guest_identity(), &private_key)?;
    write_restricted_file(
        &template_paths.guest_public_key(),
        format!("{public_key}\n").as_bytes(),
    )?;

    let guard = begin_machine_operation(&app, &machine_id, "saving_template")?;
    let container_name = paths.container_name.clone();
    let qmp_socket = paths.qmp_socket();
    let files_dir = template_paths.files_dir();
    let event_app = app.clone();
    let event_machine_id = machine_id.clone();
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        let source = match source_template_dir.as_deref() {
            Some(template_dir) => {
                buildbridge_docker_osx::MachineDisk::from_template(&source_dir, template_dir)
            }
            None => buildbridge_docker_osx::MachineDisk::new(&source_dir),
        }
        .map_err(|error| error.to_string())?;
        let files = buildbridge_docker_osx::MachineTemplateFiles::new(&files_dir)
            .map_err(|error| error.to_string())?;
        buildbridge_docker_osx::save_template(
            &container_name,
            &qmp_socket,
            &source,
            &files,
            |progress| {
                emit_machine_progress(
                    &event_app,
                    TEMPLATE_PROGRESS_EVENT,
                    &event_machine_id,
                    progress,
                );
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let size_bytes = match finish_operation(&cancel_probe, joined) {
        Ok(size) => size,
        Err(error) => {
            let _ = template_paths.remove();
            return Err(error);
        }
    };
    let template = StoredMachineTemplate {
        id,
        name,
        created_at_epoch_seconds: machines::now_epoch_seconds(),
        source_machine_name: machine.config.name.clone(),
        macos_version: diagnostics.macos_version,
        xcode_version: diagnostics.xcode_version,
        guest_username: access.username,
        known_hosts_line,
        size_bytes,
    };
    if let Err(error) = save_template_record(&app, &template) {
        let _ = template_paths.remove();
        return Err(error);
    }
    tray::refresh(&app);

    summarize_template(&app, &registry, &template)
}

#[tauri::command]
pub(crate) async fn delete_machine_template(
    app: AppHandle,
    template_id: String,
    input: ConfirmInput,
) -> Result<Vec<MachineTemplateSummary>, String> {
    if !input.confirmed {
        return Err("Confirm the template deletion before continuing.".to_string());
    }
    let registry = machines::load_registry(&app)?;
    let clones = registry
        .machines
        .iter()
        .filter(|machine| machine.template_id.as_deref() == Some(template_id.as_str()))
        .map(|machine| machine.config.name.clone())
        .collect::<Vec<_>>();
    if !clones.is_empty() {
        return Err(format!(
            "{} still read{} through this template: {}. Delete those machines first.",
            if clones.len() == 1 {
                "One machine"
            } else {
                "Machines"
            },
            if clones.len() == 1 { "s" } else { "" },
            clones.join(", ")
        ));
    }
    TemplatePaths::resolve(&app, &template_id)?.remove()?;

    summarize_templates(&app)
}

/// Makes a fresh clone reachable: pins the host key the template recorded, installs the
/// clone's own access key through the template's and retires the template's, and records the
/// guest username. Safe to run again; every step checks before it acts.
#[tauri::command]
pub(crate) async fn adopt_template_guest(
    app: AppHandle,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    let paths = MachinePaths::resolve(&app, &machine_id)?;
    let machine = machines::load_registry(&app)?.find(&machine_id)?.clone();
    let template_id = machine
        .template_id
        .clone()
        .ok_or_else(|| "This machine was not cloned from a template.".to_string())?;
    let template = load_template(&app, &template_id)?.ok_or_else(|| {
        format!("The template this machine was cloned from ({template_id}) is no longer stored.")
    })?;
    let template_paths = TemplatePaths::resolve(&app, &template_id)?;
    let guard = begin_machine_operation(&app, &machine_id, "adopting_template")?;
    let known_hosts_path = paths.known_hosts();
    let machine_identity = paths.guest_identity();
    let template_identity = template_paths.guest_identity();
    let template_public_key_path = template_paths.guest_public_key();
    let ssh_port = machine.config.ssh_port;
    let scope = guard.scope();
    let cancel_probe = Arc::clone(&scope);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _operation = buildbridge_docker_osx::enter_operation(scope);
        let expected = buildbridge_docker_osx::fingerprint_host_key_line(&template.known_hosts_line)
            .map_err(|error| error.to_string())?;
        let scanned = buildbridge_docker_osx::scan_guest_host_key(ssh_port)
            .map_err(|error| error.to_string())?;
        if scanned.fingerprint != expected {
            return Err(format!(
                "This machine's SSH identity ({}) is not the template's ({expected}). If you reinstalled macOS on it on purpose, pin the identity by hand.",
                scanned.fingerprint
            ));
        }
        match fs::read_to_string(&known_hosts_path) {
            Ok(pinned) => {
                let pinned = buildbridge_docker_osx::fingerprint_host_key_line(pinned.trim())
                    .map_err(|error| error.to_string())?;
                if pinned != expected {
                    return Err(
                        "A different identity is pinned on this machine. Forget it first."
                            .to_string(),
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                write_restricted_file(
                    &known_hosts_path,
                    format!("{}\n", scanned.known_hosts_line).as_bytes(),
                )?;
            }
            Err(error) => return Err(format!("Could not read the pinned identity: {error}")),
        }
        let public_key = ensure_mac_guest_keypair(&machine_identity)?;
        let template_public_key = read_mac_guest_public_key(&template_public_key_path)?;
        buildbridge_docker_osx::adopt_guest_key(
            ssh_port,
            &template.guest_username,
            &template_identity,
            &known_hosts_path,
            &public_key,
            &template_public_key,
        )
        .map_err(|error| error.to_string())?;
        let diagnostics = buildbridge_docker_osx::guest_diagnostics(
            ssh_port,
            &template.guest_username,
            &machine_identity,
            &known_hosts_path,
        );
        if !diagnostics.authenticated {
            return Err(format!(
                "The clone accepted the new key but the key does not sign in: {}",
                diagnostics
                    .issue
                    .unwrap_or_else(|| "no reason was reported".to_string())
            ));
        }
        Ok(template.guest_username)
    })
    .await
    .map_err(|error| error.to_string());
    drop(guard);
    let username = finish_operation(&cancel_probe, joined)?;
    save_mac_guest_access(&paths, &StoredMacGuestAccess { username })?;

    build_mac_builder_view(&app, &paths).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_names_are_trimmed_bounded_and_printable() {
        assert_eq!(
            validate_template_name("  Xcode 26 ready  ").unwrap(),
            "Xcode 26 ready"
        );
        assert!(validate_template_name("   ").is_err());
        assert!(validate_template_name(&"x".repeat(61)).is_err());
        assert!(validate_template_name("bad\u{7}name").is_err());
    }

    #[test]
    fn a_template_record_round_trips_with_its_pinned_identity() {
        let template = StoredMachineTemplate {
            id: "xcode-26-ready".to_string(),
            name: "Xcode 26 ready".to_string(),
            created_at_epoch_seconds: 1_757_000_000,
            source_machine_name: "Local macOS builder".to_string(),
            macos_version: Some("26.6.2".to_string()),
            xcode_version: Some("26.6".to_string()),
            guest_username: "builder".to_string(),
            known_hosts_line: "[127.0.0.1]:50922 ssh-ed25519 AAAA".to_string(),
            size_bytes: 21_000_000_000,
        };
        let encoded = serde_json::to_string(&template).unwrap();
        assert!(encoded.contains("\"knownHostsLine\""));
        let decoded: StoredMachineTemplate = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.guest_username, "builder");
        assert_eq!(decoded.size_bytes, 21_000_000_000);
    }
}
