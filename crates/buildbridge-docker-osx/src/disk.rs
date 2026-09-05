//! The macOS disk lives on the host, in the machine's data directory, and is bind-mounted into
//! the container. The container is then disposable: any change to how it is created — USB
//! access, a control socket, a different hardware profile — is a cheap recreate rather than a
//! reinstall.
//!
//! Docker-OSX reads the disk from `IMAGE_PATH`, the documented way to bring your own image.
//! Its NVRAM and install media sit at fixed paths in `Launch.sh`, so those are bound file by
//! file. Everything else in a container's writable layer is regenerated on every start from the
//! identity file, which is why a container created before this layout can be migrated by
//! copying three files.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::qmp::QmpClient;
use crate::qmp::QmpEndpoint;
use crate::{
    ContainerState, DOCKER_IMAGE, LaunchOptions, MacBuilderConfig, MachineProvider, ProviderError,
    RuntimeStatus, TrackedCommand, clean_output, create_container, inspect_container, probe_host,
    probe_host_for, run_docker, status, status_for, stop,
};
use ts_rs::TS;

pub const DISK_IMAGE_NAME: &str = "mac_hdd_ng.img";
pub const DISK_NVRAM_NAME: &str = "OVMF_VARS-1024x768.fd";
pub const DISK_BASESYSTEM_NAME: &str = "BaseSystem.img";
/// Docker-OSX's own default virtual size; the qcow2 grows as macOS uses it.
pub const DISK_VIRTUAL_SIZE: &str = "200G";
const CONTAINER_DISK_DIR: &str = "/image";
const CONTAINER_OSX_KVM_DIR: &str = "/home/arch/OSX-KVM";
pub(crate) const THROWAWAY_DISK_DIR: &str = "/buildbridge-disk";
const COPY_POLL: Duration = Duration::from_millis(500);
const GIB: u64 = 1024 * 1024 * 1024;
/// QEMU runs as `arch`, uid 1000, in the image; a control directory it can use has that owner.
const CONTAINER_QEMU_UID: u32 = 1000;

/// Where a provider keeps a machine's files inside its disk directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskLayout {
    /// Docker-OSX: the image, the NVRAM and the install media at the top of the directory.
    DockerOsx,
    /// dockur/macos: everything under the macOS version directory the image keeps them in.
    DockurMacos { version: &'static str },
}

impl DiskLayout {
    pub fn for_config(config: &MacBuilderConfig) -> Self {
        match config.provider {
            MachineProvider::DockerOsx => Self::DockerOsx,
            MachineProvider::DockurMacos => Self::DockurMacos {
                version: crate::dockur::version_env(config.macos_release),
            },
        }
    }

    pub fn provider(self) -> MachineProvider {
        match self {
            Self::DockerOsx => MachineProvider::DockerOsx,
            Self::DockurMacos { .. } => MachineProvider::DockurMacos,
        }
    }

    fn relative(self, docker_osx_name: &str, dockur_name: &str) -> PathBuf {
        match self {
            Self::DockerOsx => PathBuf::from(docker_osx_name),
            Self::DockurMacos { version } => Path::new(version).join(dockur_name),
        }
    }

    fn image(self) -> PathBuf {
        self.relative(DISK_IMAGE_NAME, crate::dockur::DATA_DISK_NAME)
    }

    fn nvram(self) -> PathBuf {
        self.relative(DISK_NVRAM_NAME, crate::dockur::NVRAM_NAME)
    }

    fn basesystem(self) -> PathBuf {
        self.relative(DISK_BASESYSTEM_NAME, crate::dockur::RECOVERY_NAME)
    }
}

/// The host directory holding one machine's disk files, and the template directory its disk
/// is an overlay of, when it was cloned from one.
#[derive(Debug, Clone)]
pub struct MachineDisk {
    dir: PathBuf,
    template_dir: Option<PathBuf>,
    layout: DiskLayout,
}

impl MachineDisk {
    /// A Docker-OSX disk; [`MachineDisk::for_machine`] is the provider-aware constructor.
    pub fn new(dir: &Path) -> Result<Self, ProviderError> {
        validate_bind_path(dir, "disk directory")?;

        Ok(Self {
            dir: dir.to_path_buf(),
            template_dir: None,
            layout: DiskLayout::DockerOsx,
        })
    }

    /// The disk of a machine with this profile, an overlay of a template when it has one.
    pub fn for_machine(
        config: &MacBuilderConfig,
        dir: &Path,
        template_dir: Option<&Path>,
    ) -> Result<Self, ProviderError> {
        let mut disk = match template_dir {
            Some(template_dir) => Self::from_template(dir, template_dir)?,
            None => Self::new(dir)?,
        };
        disk.layout = DiskLayout::for_config(config);
        Ok(disk)
    }

    pub fn layout(&self) -> DiskLayout {
        self.layout
    }

    /// The image's path inside the disk directory, as a throwaway container sees it.
    pub fn image_relative(&self) -> String {
        self.layout.image().to_string_lossy().into_owned()
    }

    /// A disk cloned from a template: its image is a copy-on-write overlay whose backing file
    /// is the template's image at the path the container binds it to, so the template
    /// directory is bound read-only into every container that runs the clone.
    pub fn from_template(dir: &Path, template_dir: &Path) -> Result<Self, ProviderError> {
        validate_bind_path(dir, "disk directory")?;
        validate_bind_path(template_dir, "template directory")?;

        Ok(Self {
            dir: dir.to_path_buf(),
            template_dir: Some(template_dir.to_path_buf()),
            layout: DiskLayout::DockerOsx,
        })
    }

    /// The disk a launch describes: an overlay over its template when it has one.
    pub fn for_launch(
        options: &LaunchOptions<'_>,
        config: &MacBuilderConfig,
    ) -> Result<Self, ProviderError> {
        Self::for_machine(config, options.disk_dir, options.template_dir)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn template_dir(&self) -> Option<&Path> {
        self.template_dir.as_deref()
    }

    pub fn image(&self) -> PathBuf {
        self.dir.join(self.layout.image())
    }

    pub fn nvram(&self) -> PathBuf {
        self.dir.join(self.layout.nvram())
    }

    pub fn basesystem(&self) -> PathBuf {
        self.dir.join(self.layout.basesystem())
    }

    /// The disk exists and is not empty, and for Docker-OSX so does the NVRAM it is created
    /// with; the install media is optional. dockur/macos writes its own NVRAM on first boot.
    pub fn ready(&self) -> bool {
        non_empty_file(&self.image())
            && (self.layout != DiskLayout::DockerOsx || non_empty_file(&self.nvram()))
    }
}

pub(crate) fn non_empty_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

/// What a container was created with, read back from Docker rather than remembered, so it
/// cannot drift from the truth after a discard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ContainerLayout {
    pub disk_on_host: bool,
    pub usb_access: bool,
    pub control_socket: bool,
    /// The dedicated USB 2.0 controller phones are attached to is on QEMU's command line.
    pub phone_controller: bool,
    /// The container binds a template directory, so its disk is an overlay over that template.
    pub from_template: bool,
}

/// Recreating the container from its current profile: the same disk, identity and options, so
/// a container created before an option existed picks it up. macOS restarts once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ContainerRebuildPhase {
    ShuttingDown,
    Removing,
    Creating,
    Starting,
    Completed,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ContainerRebuildProgress {
    pub phase: ContainerRebuildPhase,
    #[ts(type = "number")]
    pub elapsed_seconds: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DiskMigrationPhase {
    CheckingSpace,
    Stopping,
    CopyingDisk,
    Removing,
    Creating,
    Starting,
    Completed,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct DiskMigrationProgress {
    pub phase: DiskMigrationPhase,
    #[ts(type = "number")]
    pub completed_bytes: u64,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub elapsed_seconds: u64,
    pub detail: String,
}

/// A path Docker can bind: absolute, and free of the `:` that separates a bind's fields.
pub fn validate_bind_path(path: &Path, what: &str) -> Result<(), ProviderError> {
    if !path.is_absolute() || path.to_string_lossy().contains(':') {
        return Err(ProviderError::InvalidConfig(format!(
            "the {what} must be an absolute path without ':'"
        )));
    }

    Ok(())
}

/// Creates the disk with the image's own `qemu-img`, so the format matches what Docker-OSX
/// would have made itself.
pub(crate) fn disk_create_args(disk_dir: &Path) -> Vec<String> {
    vec![
        "run".to_string(),
        "--rm".to_string(),
        format!("--volume={}:{THROWAWAY_DISK_DIR}:rw", disk_dir.display()),
        DOCKER_IMAGE.to_string(),
        "qemu-img".to_string(),
        "create".to_string(),
        "-f".to_string(),
        "qcow2".to_string(),
        format!("{THROWAWAY_DISK_DIR}/{DISK_IMAGE_NAME}"),
        DISK_VIRTUAL_SIZE.to_string(),
    ]
}

/// Copies the pristine NVRAM out of the image so the file exists before the bind is created.
pub(crate) fn nvram_copy_args(disk_dir: &Path) -> Vec<String> {
    vec![
        "run".to_string(),
        "--rm".to_string(),
        format!("--volume={}:{THROWAWAY_DISK_DIR}:rw", disk_dir.display()),
        DOCKER_IMAGE.to_string(),
        "cp".to_string(),
        format!("{CONTAINER_OSX_KVM_DIR}/{DISK_NVRAM_NAME}"),
        format!("{THROWAWAY_DISK_DIR}/{DISK_NVRAM_NAME}"),
    ]
}

/// Makes sure a new machine's disk files exist before its container is created: an empty
/// disk for a fresh install, or an overlay over the template the machine was cloned from.
pub fn ensure_machine_disk(disk: &MachineDisk) -> Result<(), ProviderError> {
    fs::create_dir_all(disk.dir()).map_err(|error| ProviderError::Identity(error.to_string()))?;
    restrict_directory(disk.dir())?;
    if !non_empty_file(&disk.image()) {
        let _ = fs::remove_file(disk.image());
        match disk.template_dir() {
            Some(template_dir) => {
                crate::templates::clone_template_files(template_dir, disk)?;
            }
            None => run_docker("disk creation", &disk_create_args(disk.dir())).map(|_| ())?,
        }
    }
    if !non_empty_file(&disk.nvram()) {
        let _ = fs::remove_file(disk.nvram());
        run_docker("NVRAM preparation", &nvram_copy_args(disk.dir()))?;
    }
    if !disk.ready() {
        return Err(ProviderError::Identity(
            "the macOS disk files were not created on this host".to_string(),
        ));
    }

    Ok(())
}

/// The bind arguments for a container that keeps its disk on the host.
pub(crate) fn disk_bind_args(disk: &MachineDisk) -> Vec<String> {
    let mut args = vec![
        format!("--volume={}:{CONTAINER_DISK_DIR}:rw", disk.dir().display()),
        format!("--env=IMAGE_PATH={CONTAINER_DISK_DIR}/{DISK_IMAGE_NAME}"),
        format!(
            "--volume={}:{CONTAINER_OSX_KVM_DIR}/{DISK_NVRAM_NAME}:rw",
            disk.nvram().display()
        ),
    ];
    if non_empty_file(&disk.basesystem()) {
        args.push(format!(
            "--volume={}:{CONTAINER_OSX_KVM_DIR}/{DISK_BASESYSTEM_NAME}:rw",
            disk.basesystem().display()
        ));
    }
    if let Some(template_dir) = disk.template_dir() {
        args.push(format!(
            "--volume={}:{}:ro",
            template_dir.display(),
            crate::templates::CONTAINER_TEMPLATE_DIR
        ));
    }

    args
}

/// The control directory must exist before Docker binds it: a missing source is created by
/// the daemon as root, QEMU (uid 1000) then cannot bind its socket there, and a failed `-qmp`
/// bind is fatal to QEMU. Owned by the container's uid it is private; otherwise it is made
/// writable to everyone so the machine still boots, and the status reports the socket as
/// unreachable.
pub(crate) fn ensure_control_dir(qmp_dir: &Path) -> Result<(), ProviderError> {
    validate_bind_path(qmp_dir, "control socket directory")?;
    fs::create_dir_all(qmp_dir).map_err(|error| ProviderError::Identity(error.to_string()))?;
    let metadata =
        fs::metadata(qmp_dir).map_err(|error| ProviderError::Identity(error.to_string()))?;
    let mode = if metadata.uid() == CONTAINER_QEMU_UID {
        0o700
    } else {
        0o1777
    };
    fs::set_permissions(qmp_dir, fs::Permissions::from_mode(mode))
        .map_err(|error| ProviderError::Identity(error.to_string()))?;

    Ok(())
}

pub(crate) fn restrict_directory(path: &Path) -> Result<(), ProviderError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| ProviderError::Identity(error.to_string()))
}

/// Reads the layout back from `docker inspect`'s `HostConfig`.
/// Whether the container's QEMU arguments (`EXTRA` for Docker-OSX, `ARGUMENTS` for dockur/macos)
/// carry the phone controller, read back so the interface knows which containers predate it.
pub(crate) fn phone_controller_from_env(env: &[String]) -> bool {
    env.iter()
        .find_map(|entry| {
            entry
                .strip_prefix("EXTRA=")
                .or_else(|| entry.strip_prefix("ARGUMENTS="))
        })
        .is_some_and(|extra| {
            extra
                .split_whitespace()
                .any(|word| word == format!("usb-ehci,id={}", crate::qmp::USB_PHONE_CONTROLLER))
        })
}

pub(crate) fn container_layout(
    host_config_json: &str,
    env_json: &str,
) -> Result<ContainerLayout, ProviderError> {
    let value: Value =
        serde_json::from_str(host_config_json).map_err(|error| ProviderError::DockerCommand {
            operation: "inspect",
            message: format!("unreadable host configuration: {error}"),
        })?;
    let strings = |key: &str| -> Vec<String> {
        value
            .get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let binds = strings("Binds");
    let destination = |bind: &str| bind.split(':').nth(1).unwrap_or_default().to_string();
    let bound = |target: &str| binds.iter().any(|bind| destination(bind) == target);
    let rules = strings("DeviceCgroupRules");

    let env: Vec<String> = serde_json::from_str(env_json).unwrap_or_default();

    Ok(ContainerLayout {
        phone_controller: phone_controller_from_env(&env),
        from_template: bound(crate::templates::CONTAINER_TEMPLATE_DIR),
        disk_on_host: bound(CONTAINER_DISK_DIR) || bound(crate::dockur::STORAGE_CONTAINER_DIR),
        usb_access: rules
            .iter()
            .any(|rule| rule == &format!("c {}:* rwm", crate::usb::USB_BUS_MAJOR))
            && bound("/dev/bus/usb"),
        control_socket: bound(crate::qmp::QMP_CONTAINER_DIR)
            || env.iter().any(|entry| entry.starts_with("QMP=")),
    })
}

/// `None` when the container does not exist.
pub fn inspect_container_layout(
    container_name: &str,
) -> Result<Option<ContainerLayout>, ProviderError> {
    let output = Command::new("docker")
        .args([
            "inspect",
            "--format",
            // One call, two documents: the mounts and rules, then the environment that
            // carries QEMU's command line.
            "{{json .HostConfig}}{{\"\\n\"}}{{json .Config.Env}}",
            container_name,
        ])
        .output()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?;
    if !output.status.success() {
        let message = clean_output(&output.stderr);
        if crate::is_missing_container_error(&message) {
            return Ok(None);
        }
        return Err(ProviderError::DockerCommand {
            operation: "inspect",
            message,
        });
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let (host_config, env) = text.split_once('\n').unwrap_or((text.trim(), "[]"));
    container_layout(host_config, env).map(Some)
}

/// The writable layer plus a tenth and a gibibyte: the copy lands next to a growing qcow2.
pub fn required_free_bytes(size_rw: u64) -> u64 {
    size_rw.saturating_add(size_rw / 10).saturating_add(GIB)
}

pub(crate) fn parse_df_available(output: &str) -> Option<u64> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .nth(1)
        .and_then(|line| line.parse().ok())
}

pub(crate) fn available_bytes(path: &Path) -> Result<u64, ProviderError> {
    let output = Command::new("df")
        .args(["-B1", "--output=avail"])
        .arg(path)
        .output()
        .map_err(|error| ProviderError::Identity(format!("could not check free space: {error}")))?;
    parse_df_available(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
        ProviderError::Identity("could not read the free space of the disk directory".to_string())
    })
}

fn container_size_rw(container_name: &str) -> Result<u64, ProviderError> {
    let output = run_docker(
        "size inspection",
        &[
            "inspect".to_string(),
            "--size".to_string(),
            "--format".to_string(),
            "{{.SizeRw}}".to_string(),
            container_name.to_string(),
        ],
    )?;

    clean_output(&output.stdout)
        .parse()
        .map_err(|_| ProviderError::DockerCommand {
            operation: "size inspection",
            message: "Docker did not report the container's writable size".to_string(),
        })
}

pub(crate) fn format_gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / GIB as f64)
}

/// Streams one file out of a stopped container, reporting the bytes landed so far.
fn copy_out<F>(
    container_name: &str,
    source: &str,
    destination: &Path,
    on_copied: &mut F,
) -> Result<u64, ProviderError>
where
    F: FnMut(u64),
{
    let partial = PathBuf::from(format!("{}.part", destination.display()));
    let _ = fs::remove_file(&partial);
    let mut child = Command::new("docker")
        .args(["cp", &format!("{container_name}:{source}")])
        .arg(&partial)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                on_copied(fs::metadata(&partial).map(|m| m.len()).unwrap_or(0));
                thread::sleep(COPY_POLL);
            }
            Err(error) => {
                let _ = fs::remove_file(&partial);
                return Err(ProviderError::DockerUnavailable(error.to_string()));
            }
        }
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
    }
    if !status.success() {
        let _ = fs::remove_file(&partial);
        return Err(ProviderError::DockerCommand {
            operation: "disk copy",
            message: clean_output(stderr.as_bytes()),
        });
    }
    let copied = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
    if copied == 0 {
        let _ = fs::remove_file(&partial);
        return Err(ProviderError::DockerCommand {
            operation: "disk copy",
            message: format!("{source} came back empty"),
        });
    }
    fs::rename(&partial, destination)
        .map_err(|error| ProviderError::Identity(error.to_string()))?;
    on_copied(copied);

    Ok(copied)
}

/// Moves an existing container's disk onto the host and recreates the container around it
/// with the current create-time options. The macOS installation, NVRAM, pinned SSH identity
/// and everything on the disk survive; only the container is new.
pub fn migrate_disk_to_host<F>(
    container_name: &str,
    config: &MacBuilderConfig,
    options: &LaunchOptions<'_>,
    mut on_progress: F,
) -> Result<RuntimeStatus, ProviderError>
where
    F: FnMut(DiskMigrationProgress),
{
    config.validate()?;
    if config.provider == MachineProvider::DockurMacos {
        return Err(ProviderError::UsbPassthrough(
            "a dockur/macos machine keeps its disk on this host from its first start; there is nothing to migrate".to_string(),
        ));
    }
    let disk = MachineDisk::for_launch(options, config)?;
    validate_bind_path(options.qmp_dir, "control socket directory")?;
    let started = Instant::now();
    let mut report = |phase: DiskMigrationPhase, completed: u64, total: u64, detail: &str| {
        on_progress(DiskMigrationProgress {
            phase,
            completed_bytes: completed,
            total_bytes: total,
            elapsed_seconds: started.elapsed().as_secs(),
            detail: detail.to_string(),
        });
    };
    let prerequisites = probe_host();
    if !prerequisites.ready {
        return Err(ProviderError::Prerequisites(prerequisites.issues.join(" ")));
    }
    let (state, _, _) = inspect_container(container_name)?;
    if matches!(state, ContainerState::Missing | ContainerState::Unavailable) {
        return Err(ProviderError::UsbPassthrough(
            "there is no container to migrate; starting the machine creates one with the disk on this host".to_string(),
        ));
    }
    if inspect_container_layout(container_name)?.is_some_and(|layout| layout.disk_on_host) {
        return Err(ProviderError::UsbPassthrough(
            "this machine's disk is already on the host".to_string(),
        ));
    }
    if non_empty_file(&disk.image()) {
        return Err(ProviderError::UsbPassthrough(format!(
            "a disk image already exists at {}; move it away before migrating this container",
            disk.image().display()
        )));
    }

    report(
        DiskMigrationPhase::CheckingSpace,
        0,
        0,
        "Measuring the container and the free space on this host",
    );
    fs::create_dir_all(disk.dir()).map_err(|error| ProviderError::Identity(error.to_string()))?;
    restrict_directory(disk.dir())?;
    let size_rw = container_size_rw(container_name)?;
    let available = available_bytes(disk.dir())?;
    let required = required_free_bytes(size_rw);
    if available < required {
        return Err(ProviderError::UsbPassthrough(format!(
            "the disk directory has {} free but the copy needs {} ({} in the container plus margin)",
            format_gib(available),
            format_gib(required),
            format_gib(size_rw)
        )));
    }

    report(
        DiskMigrationPhase::Stopping,
        0,
        size_rw,
        "Stopping the machine so its disk is consistent",
    );
    stop(container_name)?;

    let image_source = format!("{CONTAINER_OSX_KVM_DIR}/{DISK_IMAGE_NAME}");
    let mut on_copied = |copied: u64| {
        report(
            DiskMigrationPhase::CopyingDisk,
            copied.min(size_rw),
            size_rw,
            "Copying the macOS disk to this host",
        );
    };
    copy_out(container_name, &image_source, &disk.image(), &mut on_copied)?;
    let mut quiet = |_: u64| {};
    copy_out(
        container_name,
        &format!("{CONTAINER_OSX_KVM_DIR}/{DISK_NVRAM_NAME}"),
        &disk.nvram(),
        &mut quiet,
    )?;
    // The install media is worth keeping to avoid a download, but the machine boots without
    // it: Docker-OSX fetches it again on the first start of the new container.
    let _ = copy_out(
        container_name,
        &format!("{CONTAINER_OSX_KVM_DIR}/{DISK_BASESYSTEM_NAME}"),
        &disk.basesystem(),
        &mut quiet,
    );

    report(
        DiskMigrationPhase::Removing,
        size_rw,
        size_rw,
        "Removing the old container; its disk is now on this host",
    );
    run_docker(
        "container removal",
        &["rm".to_string(), container_name.to_string()],
    )?;

    report(
        DiskMigrationPhase::Creating,
        size_rw,
        size_rw,
        "Creating the container with the host disk, the control socket, and USB access",
    );
    ensure_control_dir(options.qmp_dir)?;
    create_container(
        container_name,
        config,
        prerequisites.display.as_deref().unwrap_or(":0"),
        options,
        &disk,
    )?;

    report(
        DiskMigrationPhase::Starting,
        size_rw,
        size_rw,
        "Starting the machine",
    );
    run_docker("start", &["start".to_string(), container_name.to_string()])?;
    report(
        DiskMigrationPhase::Completed,
        size_rw,
        size_rw,
        "The machine is running from the host disk",
    );

    status(container_name)
}

/// How long to let macOS finish shutting itself down before the container is stopped anyway.
const GUEST_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(120);
const GUEST_SHUTDOWN_POLL: Duration = Duration::from_secs(2);

/// Recreates the container around its disk, so that a container created before an option
/// existed — today the phone's USB controller — picks it up. macOS restarts once. This is
/// affordable only because the disk lives on this host, so it refuses outright when it does
/// not, since removing such a container would take the macOS installation with it. macOS is
/// asked to shut itself down first; a container that has been power-cut repeatedly is how a
/// disk gets corrupted.
pub fn rebuild_container<F>(
    container_name: &str,
    config: &MacBuilderConfig,
    options: &LaunchOptions<'_>,
    mut on_progress: F,
) -> Result<RuntimeStatus, ProviderError>
where
    F: FnMut(ContainerRebuildProgress),
{
    config.validate()?;
    let disk = MachineDisk::for_launch(options, config)?;
    validate_bind_path(options.qmp_dir, "control socket directory")?;
    let started = Instant::now();
    let mut report = |phase: ContainerRebuildPhase, detail: &str| {
        on_progress(ContainerRebuildProgress {
            phase,
            elapsed_seconds: started.elapsed().as_secs(),
            detail: detail.to_string(),
        });
    };
    let prerequisites = probe_host_for(config.provider);
    if !prerequisites.ready {
        return Err(ProviderError::Prerequisites(prerequisites.issues.join(" ")));
    }

    let (state, _, _) = inspect_container(container_name)?;
    let exists = !matches!(state, ContainerState::Missing | ContainerState::Unavailable);
    if exists && !crate::inspect_container_layout(container_name)?.is_some_and(|l| l.disk_on_host) {
        return Err(ProviderError::UsbPassthrough(
            "this machine still keeps its macOS disk inside the container, so it cannot be recreated; enable USB on this machine first, which moves the disk to this host".to_string(),
        ));
    }

    if exists {
        report(
            ContainerRebuildPhase::ShuttingDown,
            "Asking macOS to shut down before the machine is rebuilt",
        );
        shut_down_guest(
            container_name,
            &QmpEndpoint::for_machine(config.provider, options.qmp_dir, container_name),
            &mut |detail| report(ContainerRebuildPhase::ShuttingDown, detail),
        )?;

        report(
            ContainerRebuildPhase::Removing,
            "Removing the container; the macOS disk stays on this host",
        );
        run_docker(
            "container removal",
            &["rm".to_string(), container_name.to_string()],
        )?;
    }

    report(
        ContainerRebuildPhase::Creating,
        "Creating the container with the host disk, the control socket, USB access, and the phone controller",
    );
    ensure_control_dir(options.qmp_dir)?;
    create_container(
        container_name,
        config,
        prerequisites.display.as_deref().unwrap_or(":0"),
        options,
        &disk,
    )?;

    report(ContainerRebuildPhase::Starting, "Starting macOS");
    run_docker("start", &["start".to_string(), container_name.to_string()])?;
    report(ContainerRebuildPhase::Completed, "The machine is starting");

    status_for(container_name, config.provider)
}

/// Asks the guest to power down and waits for QEMU to exit, then stops the container whatever
/// happened: an unreachable socket or a guest that ignores the request must not block the
/// rebuild, and `stop` is a no-op once the container has already exited.
pub(crate) fn shut_down_guest(
    container_name: &str,
    qmp_endpoint: &QmpEndpoint,
    on_wait: &mut dyn FnMut(&str),
) -> Result<(), ProviderError> {
    if let Ok(mut client) = QmpClient::connect(qmp_endpoint)
        && client.power_down().is_ok()
    {
        let deadline = Instant::now() + GUEST_SHUTDOWN_TIMEOUT;
        while Instant::now() < deadline {
            let (state, _, _) = inspect_container(container_name)?;
            if !matches!(
                state,
                ContainerState::Running | ContainerState::Paused | ContainerState::Restarting
            ) {
                break;
            }
            // Shutting macOS down takes the better part of a minute, and a row that reports
            // nothing for that long reads as a hang rather than as waiting.
            on_wait("Waiting for macOS to finish shutting down");
            thread::sleep(GUEST_SHUTDOWN_POLL);
        }
        on_wait("macOS has shut down");
    }
    stop(container_name).map(|_| ())
}

/// Deletes the disk directory. Callers confirm first; this is the macOS installation. Files a
/// dockur/macos container created as root are not this user's to unlink, so a refusal is
/// retried once after a throwaway container has emptied the directory.
pub fn remove_machine_disk(disk: &MachineDisk) -> Result<(), ProviderError> {
    match fs::remove_dir_all(disk.dir()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            crate::dockur::empty_storage_with_container(disk.dir())?;
            fs::remove_dir_all(disk.dir()).map_err(|error| ProviderError::Identity(error.to_string()))
        }
        Err(error) => Err(ProviderError::Identity(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_files_are_created_with_the_images_own_tools_in_throwaway_containers() {
        let dir = Path::new("/tmp/buildbridge/disk");
        let create = disk_create_args(dir);
        assert_eq!(create[0], "run");
        assert!(create.contains(&"--rm".to_string()));
        assert!(
            create.contains(&"--volume=/tmp/buildbridge/disk:/buildbridge-disk:rw".to_string())
        );
        assert!(create.contains(&DOCKER_IMAGE.to_string()));
        assert!(create.ends_with(&[
            "qemu-img".to_string(),
            "create".to_string(),
            "-f".to_string(),
            "qcow2".to_string(),
            "/buildbridge-disk/mac_hdd_ng.img".to_string(),
            "200G".to_string(),
        ]));
        let nvram = nvram_copy_args(dir);
        assert!(nvram.ends_with(&[
            "cp".to_string(),
            "/home/arch/OSX-KVM/OVMF_VARS-1024x768.fd".to_string(),
            "/buildbridge-disk/OVMF_VARS-1024x768.fd".to_string(),
        ]));
        for arg in create.iter().chain(nvram.iter()) {
            assert!(!arg.contains("--privileged"));
        }
    }

    #[test]
    fn disk_binds_use_image_path_and_the_fixed_nvram_path() {
        let disk = MachineDisk::new(Path::new("/tmp/buildbridge/disk")).expect("valid");
        let args = disk_bind_args(&disk);
        assert!(args.contains(&"--volume=/tmp/buildbridge/disk:/image:rw".to_string()));
        assert!(args.contains(&"--env=IMAGE_PATH=/image/mac_hdd_ng.img".to_string()));
        assert!(args.contains(
            &"--volume=/tmp/buildbridge/disk/OVMF_VARS-1024x768.fd:/home/arch/OSX-KVM/OVMF_VARS-1024x768.fd:rw".to_string()
        ));
        assert!(
            !args.iter().any(|arg| arg.contains("BaseSystem.img")),
            "install media is bound only when the host holds a copy"
        );
    }

    #[test]
    fn bind_paths_are_absolute_and_free_of_colons() {
        assert!(MachineDisk::new(Path::new("/var/lib/buildbridge/disk")).is_ok());
        assert!(MachineDisk::new(Path::new("relative/disk")).is_err());
        assert!(MachineDisk::new(Path::new("/tmp/with:colon")).is_err());
    }

    #[test]
    fn the_phone_controller_is_read_back_out_of_the_container() {
        let with: Vec<String> = vec!["DISPLAY=:0".into(), "EXTRA=-display gtk,zoom-to-fit=on -qmp unix:/buildbridge-qmp/qmp.sock,server,nowait -device usb-ehci,id=buildbridge-phone-usb".into()];
        assert!(phone_controller_from_env(&with));
        let without: Vec<String> = vec![
            "EXTRA=-display gtk,zoom-to-fit=on -qmp unix:/buildbridge-qmp/qmp.sock,server,nowait"
                .into(),
        ];
        assert!(!phone_controller_from_env(&without));
        assert!(!phone_controller_from_env(&[]));
        assert!(!phone_controller_from_env(&[
            "EXTRA=-device usb-ehci,id=ehci".to_string()
        ]));
    }

    #[test]
    fn container_layout_is_read_from_the_host_configuration() {
        let legacy = r#"{"Binds":null,"DeviceCgroupRules":null}"#;
        assert_eq!(
            container_layout(legacy, "[]").expect("parses"),
            ContainerLayout {
                disk_on_host: false,
                usb_access: false,
                control_socket: false,
                phone_controller: false,
                from_template: false,
            }
        );
        let current = r#"{"Binds":["/tmp/.X11-unix:/tmp/.X11-unix:rw","/home/m/disk:/image:rw","/home/m/qmp:/buildbridge-qmp:rw","/dev/bus/usb:/dev/bus/usb"],"DeviceCgroupRules":["c 189:* rwm"]}"#;
        assert_eq!(
            container_layout(current, "[]").expect("parses"),
            ContainerLayout {
                disk_on_host: true,
                usb_access: true,
                control_socket: true,
                phone_controller: false,
                from_template: false,
            }
        );
        let rule_without_bind =
            r#"{"Binds":["/home/m/disk:/image:rw"],"DeviceCgroupRules":["c 189:* rwm"]}"#;
        assert!(
            !container_layout(rule_without_bind, "[]")
                .expect("parses")
                .usb_access
        );
        assert!(container_layout("not json", "[]").is_err());
    }

    #[test]
    fn migration_space_check_requires_the_writable_layer_plus_margin() {
        assert_eq!(required_free_bytes(0), GIB);
        assert_eq!(required_free_bytes(100 * GIB), 111 * GIB);
        assert_eq!(required_free_bytes(u64::MAX), u64::MAX);
        assert_eq!(
            parse_df_available("     Avail\n188386639872\n"),
            Some(188_386_639_872)
        );
        assert_eq!(parse_df_available("Avail\n"), None);
        assert_eq!(format_gib(34_526_003_200), "32.2 GiB");
    }
}
