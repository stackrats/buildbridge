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
use crate::usb::BootUsbDevice;
use crate::{
    ContainerState, DOCKER_IMAGE, LaunchOptions, MacBuilderConfig, ProviderError, RuntimeStatus,
    TrackedCommand, clean_output, create_container, inspect_container, probe_host, run_docker,
    status, stop,
};

pub const DISK_IMAGE_NAME: &str = "mac_hdd_ng.img";
pub const DISK_NVRAM_NAME: &str = "OVMF_VARS-1024x768.fd";
pub const DISK_BASESYSTEM_NAME: &str = "BaseSystem.img";
/// Docker-OSX's own default virtual size; the qcow2 grows as macOS uses it.
pub const DISK_VIRTUAL_SIZE: &str = "200G";
const CONTAINER_DISK_DIR: &str = "/image";
const CONTAINER_OSX_KVM_DIR: &str = "/home/arch/OSX-KVM";
const THROWAWAY_DISK_DIR: &str = "/buildbridge-disk";
const COPY_POLL: Duration = Duration::from_millis(500);
const GIB: u64 = 1024 * 1024 * 1024;
/// QEMU runs as `arch`, uid 1000, in the image; a control directory it can use has that owner.
const CONTAINER_QEMU_UID: u32 = 1000;

/// The host directory holding one machine's disk files.
#[derive(Debug, Clone)]
pub struct MachineDisk {
    dir: PathBuf,
}

impl MachineDisk {
    pub fn new(dir: &Path) -> Result<Self, ProviderError> {
        validate_bind_path(dir, "disk directory")?;

        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn image(&self) -> PathBuf {
        self.dir.join(DISK_IMAGE_NAME)
    }

    pub fn nvram(&self) -> PathBuf {
        self.dir.join(DISK_NVRAM_NAME)
    }

    pub fn basesystem(&self) -> PathBuf {
        self.dir.join(DISK_BASESYSTEM_NAME)
    }

    /// The disk and NVRAM exist and are not empty; the install media is optional.
    pub fn ready(&self) -> bool {
        non_empty_file(&self.image()) && non_empty_file(&self.nvram())
    }
}

fn non_empty_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

/// What a container was created with, read back from Docker rather than remembered, so it
/// cannot drift from the truth after a discard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerLayout {
    pub disk_on_host: bool,
    pub usb_access: bool,
    pub control_socket: bool,
    /// The phone on QEMU's command line, when this container was created with one. Letting it
    /// go means recreating the container, so the interface has to know it is there.
    pub boot_usb: Option<BootUsbDevice>,
}

/// Recreating the container so QEMU's command line matches the phone attached at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BootUsbPhase {
    ShuttingDown,
    Removing,
    Creating,
    Starting,
    Completed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootUsbProgress {
    pub phase: BootUsbPhase,
    pub elapsed_seconds: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskMigrationProgress {
    pub phase: DiskMigrationPhase,
    pub completed_bytes: u64,
    pub total_bytes: u64,
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

/// Makes sure a new machine's disk files exist before its container is created.
pub fn ensure_machine_disk(disk: &MachineDisk) -> Result<(), ProviderError> {
    fs::create_dir_all(disk.dir()).map_err(|error| ProviderError::Identity(error.to_string()))?;
    restrict_directory(disk.dir())?;
    if !non_empty_file(&disk.image()) {
        let _ = fs::remove_file(disk.image());
        run_docker("disk creation", &disk_create_args(disk.dir()))?;
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

fn restrict_directory(path: &Path) -> Result<(), ProviderError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| ProviderError::Identity(error.to_string()))
}

/// Reads the layout back from `docker inspect`'s `HostConfig`.
/// The phone QEMU was given on its command line, read back out of the container's `EXTRA`.
/// Anything that does not parse as a bus and a valid port is treated as no phone, because the
/// only thing this decides is whether letting one go needs a rebuild.
pub(crate) fn boot_usb_from_env(env: &[String]) -> Option<BootUsbDevice> {
    let extra = env.iter().find_map(|entry| entry.strip_prefix("EXTRA="))?;
    let device = extra
        .split_whitespace()
        .find(|word| word.starts_with("usb-host,"))?;
    let field = |name: &str| {
        device
            .split(',')
            .find_map(|part| part.strip_prefix(&format!("{name}=")))
    };
    let bus = field("hostbus")?.parse().ok()?;

    BootUsbDevice::new(bus, field("hostport")?).ok()
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
        boot_usb: boot_usb_from_env(&env),
        disk_on_host: bound(CONTAINER_DISK_DIR),
        usb_access: rules
            .iter()
            .any(|rule| rule == &format!("c {}:* rwm", crate::usb::USB_BUS_MAJOR))
            && bound("/dev/bus/usb"),
        control_socket: bound(crate::qmp::QMP_CONTAINER_DIR),
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

fn available_bytes(path: &Path) -> Result<u64, ProviderError> {
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
    let disk = MachineDisk::new(options.disk_dir)?;
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

/// Recreates the container so that QEMU's command line matches `options.usb.boot_device`:
/// with the phone to attach it, without it to let it go.
///
/// A phone hot-plugged into a running guest is at the mercy of QEMU's reset handling, which is
/// what makes it unreliable. On the command line the phone is simply there when macOS starts,
/// and macOS enumerates it during its own USB scan. The cost is that macOS restarts, which is
/// only affordable because the disk lives on this host — so this refuses outright when it does
/// not, since removing such a container would take the macOS installation with it.
///
/// macOS is asked to shut itself down first; a container that has been power-cut repeatedly is
/// how a disk gets corrupted.
pub fn set_boot_usb_device<F>(
    container_name: &str,
    config: &MacBuilderConfig,
    options: &LaunchOptions<'_>,
    mut on_progress: F,
) -> Result<RuntimeStatus, ProviderError>
where
    F: FnMut(BootUsbProgress),
{
    config.validate()?;
    let disk = MachineDisk::new(options.disk_dir)?;
    validate_bind_path(options.qmp_dir, "control socket directory")?;
    let started = Instant::now();
    let mut report = |phase: BootUsbPhase, detail: &str| {
        on_progress(BootUsbProgress {
            phase,
            elapsed_seconds: started.elapsed().as_secs(),
            detail: detail.to_string(),
        });
    };
    let prerequisites = probe_host();
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
            BootUsbPhase::ShuttingDown,
            "Asking macOS to shut down before the machine is rebuilt",
        );
        shut_down_guest(
            container_name,
            &options.qmp_dir.join(crate::QMP_SOCKET_NAME),
            &mut |detail| report(BootUsbPhase::ShuttingDown, detail),
        )?;

        report(
            BootUsbPhase::Removing,
            "Removing the container; the macOS disk stays on this host",
        );
        run_docker(
            "container removal",
            &["rm".to_string(), container_name.to_string()],
        )?;
    }

    report(
        BootUsbPhase::Creating,
        match options
            .usb
            .as_ref()
            .and_then(|usb| usb.boot_device.as_ref())
        {
            Some(_) => "Creating the container with the iPhone on QEMU's command line",
            None => "Creating the container without the iPhone",
        },
    );
    ensure_control_dir(options.qmp_dir)?;
    create_container(
        container_name,
        config,
        prerequisites.display.as_deref().unwrap_or(":0"),
        options,
        &disk,
    )?;

    report(BootUsbPhase::Starting, "Starting macOS");
    run_docker("start", &["start".to_string(), container_name.to_string()])?;
    report(
        BootUsbPhase::Completed,
        "The machine is starting; macOS enumerates the phone as it boots",
    );

    status(container_name)
}

/// Asks the guest to power down and waits for QEMU to exit, then stops the container whatever
/// happened: an unreachable socket or a guest that ignores the request must not block the
/// rebuild, and `stop` is a no-op once the container has already exited.
fn shut_down_guest(
    container_name: &str,
    qmp_socket: &Path,
    on_wait: &mut dyn FnMut(&str),
) -> Result<(), ProviderError> {
    if let Ok(mut client) = QmpClient::connect(qmp_socket)
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

/// Deletes the disk directory. Callers confirm first; this is the macOS installation.
pub fn remove_machine_disk(disk: &MachineDisk) -> Result<(), ProviderError> {
    match fs::remove_dir_all(disk.dir()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
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
    fn the_phone_on_qemus_command_line_is_read_back_out_of_the_container() {
        let env = r#"["DISPLAY=:0","EXTRA=-display gtk,zoom-to-fit=on -qmp unix:/buildbridge-qmp/qmp.sock,server,nowait -device usb-host,id=buildbridge-iphone,bus=xhci.0,hostbus=3,hostport=9"]"#;
        let parsed: Vec<String> = serde_json::from_str(env).expect("env");
        let device = boot_usb_from_env(&parsed).expect("a phone");
        assert_eq!(device.bus(), 3);
        assert_eq!(device.port(), "9");

        // No phone, no EXTRA at all, and a malformed one all read as no phone, because the
        // only thing this decides is whether letting one go needs a rebuild.
        let without: Vec<String> = serde_json::from_str(
            r#"["EXTRA=-display gtk,zoom-to-fit=on -qmp unix:/buildbridge-qmp/qmp.sock,server,nowait"]"#,
        )
        .expect("env");
        assert!(boot_usb_from_env(&without).is_none());
        assert!(boot_usb_from_env(&[]).is_none());
        assert!(
            boot_usb_from_env(&["EXTRA=-device usb-host,hostbus=x,hostport=9".to_string()])
                .is_none()
        );
        assert!(
            boot_usb_from_env(&["EXTRA=-device usb-host,hostbus=3,hostport=0".to_string()])
                .is_none()
        );
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
                boot_usb: None,
            }
        );
        let current = r#"{"Binds":["/tmp/.X11-unix:/tmp/.X11-unix:rw","/home/m/disk:/image:rw","/home/m/qmp:/buildbridge-qmp:rw","/dev/bus/usb:/dev/bus/usb"],"DeviceCgroupRules":["c 189:* rwm"]}"#;
        assert_eq!(
            container_layout(current, "[]").expect("parses"),
            ContainerLayout {
                disk_on_host: true,
                usb_access: true,
                control_socket: true,
                boot_usb: None,
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
