//! dockur/macos as a machine's provider. Like Docker-OSX it runs macOS under QEMU with KVM in
//! a container the engine creates with a fixed argv, but everything a machine owns lives under
//! one storage directory bound at `/storage`: the recovery image it downloads from Apple, its
//! OpenCore boot image, the identity it generates for the machine, and a qcow2 data disk. Its
//! screen is a web page rather than a window on this host's display, so a headless host and
//! the command line can show it too. QEMU runs as root in the image, so its control socket is
//! reached through `docker exec` rather than a bound directory, and the image forwards every
//! published port into the guest except the ones it keeps, which is how the guest's SSH port
//! reaches this host while the screen stays with the container.

use super::*;

pub const DOCKUR_IMAGE: &str = "dockurr/macos:latest";
/// The volume the image keeps every file of the machine in. Inside it, everything sits under
/// the macOS version directory, which is what `version_env` names.
pub(crate) const STORAGE_CONTAINER_DIR: &str = "/storage";
pub(crate) const DATA_DISK_NAME: &str = "data.qcow2";
pub(crate) const NVRAM_NAME: &str = "macos.vars";
pub(crate) const RECOVERY_NAME: &str = "base.dmg";
/// Where QEMU creates its control socket inside the container.
pub(crate) const QMP_CONTAINER_SOCKET: &str = "/run/buildbridge-qmp.sock";
/// The `-qmp` value the image hands QEMU. Spelled out in full: a bare path is passed through
/// as it is by the image versions that do not normalize it, and QEMU rejects a bare path.
const QMP_CONTAINER_CHARDEV: &str = "unix:/run/buildbridge-qmp.sock,server=on,wait=off";
/// The image's own web viewer for the screen.
const WEB_VIEWER_PORT: u16 = 8006;
/// Remote Login inside the guest; the image forwards the published port to it.
const GUEST_SSH_PORT: u16 = 22;
/// The image shuts macOS down cleanly on stop when given the time.
const STOP_TIMEOUT_SECONDS: u16 = 120;
const GIB: u64 = 1024 * 1024 * 1024;
/// A throwaway container's view of a storage directory it is asked to empty.
const THROWAWAY_STORAGE_DIR: &str = "/buildbridge-storage";

/// The image's `VERSION` value for a release.
pub(crate) fn version_env(release: MacOsRelease) -> &'static str {
    match release {
        MacOsRelease::Tahoe => "26",
        MacOsRelease::Sequoia => "15",
        MacOsRelease::Sonoma => "14",
        MacOsRelease::Ventura => "13",
    }
}

/// QEMU's extra arguments: the dedicated USB 2.0 controller a phone is attached to, for the
/// same reason Docker-OSX gets one. Nothing else, because the image composes the rest.
pub(crate) fn qemu_arguments(usb: Option<&ContainerUsbOptions>) -> Option<String> {
    usb.map(|_| format!("-device usb-ehci,id={USB_PHONE_CONTROLLER}"))
}

/// The container's fixed argv. Both published ports bind to loopback; the tun device and the
/// network capability are what the image needs to forward them into the guest. USB access is
/// the same device rule and device tree as Docker-OSX gets, without a group: QEMU is root here.
pub(crate) fn create_args(
    container_name: &str,
    config: &MacBuilderConfig,
    disk: &MachineDisk,
    usb: Option<&ContainerUsbOptions>,
) -> Vec<String> {
    let mut args = vec![
        "create".to_string(),
        format!("--name={container_name}"),
        "--label=dev.buildbridge.managed=true".to_string(),
        "--label=dev.buildbridge.provider=dockur_macos".to_string(),
        "--restart=no".to_string(),
        format!("--stop-timeout={STOP_TIMEOUT_SECONDS}"),
        "--device=/dev/kvm".to_string(),
        "--device=/dev/net/tun".to_string(),
        "--cap-add=NET_ADMIN".to_string(),
        format!("--publish=127.0.0.1:{}:{GUEST_SSH_PORT}", config.ssh_port),
    ];
    if let Some(port) = config.display_port() {
        args.push(format!("--publish=127.0.0.1:{port}:{WEB_VIEWER_PORT}"));
    }
    args.push(format!(
        "--volume={}:{STORAGE_CONTAINER_DIR}:rw",
        disk.dir().display()
    ));
    if let Some(template_dir) = disk.template_dir() {
        args.push(format!(
            "--volume={}:{}:ro",
            template_dir.display(),
            crate::templates::CONTAINER_TEMPLATE_DIR
        ));
    }
    if usb.is_some() {
        args.push(format!(
            "--device-cgroup-rule=c {}:* rwm",
            usb::USB_BUS_MAJOR
        ));
        args.push("--volume=/dev/bus/usb:/dev/bus/usb".to_string());
    }
    args.extend([
        format!("--env=VERSION={}", version_env(config.macos_release)),
        format!("--env=RAM_SIZE={}G", config.memory_gib),
        format!("--env=CPU_CORES={}", config.cpu_cores),
        format!("--env=DISK_SIZE={}", disk::DISK_VIRTUAL_SIZE),
        "--env=DISK_FMT=qcow2".to_string(),
        format!("--env=QMP={QMP_CONTAINER_CHARDEV}"),
    ]);
    if let Some(arguments) = qemu_arguments(usb) {
        args.push(format!("--env=ARGUMENTS={arguments}"));
    }
    args.push(DOCKUR_IMAGE.to_string());

    args
}

/// `MemAvailable` from `/proc/meminfo`, in bytes.
pub(crate) fn parse_mem_available(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))
        .and_then(|rest| rest.trim().strip_suffix("kB"))
        .and_then(|kib| kib.trim().parse::<u64>().ok())
        .map(|kib| kib * 1024)
}

/// The image refuses to start a machine whose memory is not actually free on the host, so the
/// refusal is made here first, with the numbers.
pub(crate) fn ensure_memory_available(config: &MacBuilderConfig) -> Result<(), ProviderError> {
    let Some(available) = fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| parse_mem_available(&text))
    else {
        return Ok(());
    };
    let wanted = u64::from(config.memory_gib) * GIB;
    if available < wanted {
        return Err(ProviderError::Prerequisites(format!(
            "dockur/macos refuses to start unless the machine's {} GiB of memory are free, and this host has {:.1} GiB available; stop other machines or give this one less memory",
            config.memory_gib,
            available as f64 / GIB as f64
        )));
    }

    Ok(())
}

pub(crate) fn ensure_storage_dir(storage_dir: &Path) -> Result<(), ProviderError> {
    validate_bind_path(storage_dir, "storage directory")?;
    fs::create_dir_all(storage_dir).map_err(|error| ProviderError::Identity(error.to_string()))?;
    disk::restrict_directory(storage_dir)
}

pub(crate) fn create_container(
    container_name: &str,
    config: &MacBuilderConfig,
    options: &LaunchOptions<'_>,
) -> Result<(), ProviderError> {
    let disk = MachineDisk::for_launch(options, config)?;
    run_docker(
        "container creation",
        &create_args(container_name, config, &disk, options.usb.as_ref()),
    )?;

    Ok(())
}

/// Creates the container when it is missing, then starts it. There is no identity to generate
/// and, from scratch, no disk to prepare: the image does both on its first start, into the
/// storage directory. A clone gets its overlay first, and the image then generates a fresh
/// identity and boot image for it, since neither is in the storage directory.
pub(crate) fn launch<F>(
    container_name: &str,
    config: &MacBuilderConfig,
    options: &LaunchOptions<'_>,
    mut on_progress: F,
) -> Result<RuntimeStatus, ProviderError>
where
    F: FnMut(LaunchProgress),
{
    let disk = MachineDisk::for_launch(options, config)?;
    let started = Instant::now();
    let mut report = |phase: LaunchPhase, detail: &str| {
        on_progress(LaunchProgress {
            phase,
            elapsed_seconds: started.elapsed().as_secs(),
            detail: detail.to_string(),
        });
    };
    report(LaunchPhase::Preparing, "Checking the host and Docker");
    let prerequisites = probe_host_for(MachineProvider::DockurMacos);
    if !prerequisites.ready {
        return Err(ProviderError::Prerequisites(prerequisites.issues.join(" ")));
    }

    let (state, _, _) = inspect_container(container_name)?;
    if state == ContainerState::Missing {
        report(LaunchPhase::PullingImage, "Pulling the dockur/macos image");
        ensure_image(DOCKUR_IMAGE)?;
        report(
            LaunchPhase::PreparingDisk,
            "Preparing the machine's storage directory on this host",
        );
        ensure_storage_dir(options.disk_dir)?;
        if let Some(template_dir) = disk.template_dir()
            && !disk.ready()
        {
            let _ = fs::remove_file(disk.image());
            crate::templates::clone_template_files(template_dir, &disk)?;
        }
        report(
            LaunchPhase::CreatingContainer,
            "Creating the managed container",
        );
        create_container(container_name, config, options)?;
    } else {
        ensure_manual_restart_policy(container_name)?;
    }

    let (state, _, _) = inspect_container(container_name)?;
    if state != ContainerState::Running {
        ensure_memory_available(config)?;
        report(
            LaunchPhase::Starting,
            "Starting the macOS machine; the first start downloads macOS from Apple",
        );
        run_docker("start", &["start".to_string(), container_name.to_string()])?;
    }

    report(LaunchPhase::Completed, "The macOS machine is running");
    status_for(container_name, MachineProvider::DockurMacos)
}

/// Empties a storage directory whose files the image created as root, which this user cannot
/// unlink from outside the container: a throwaway container with the directory bound deletes
/// them with a fixed argv, and the directory itself is then this user's to remove.
pub(crate) fn empty_storage_with_container(storage_dir: &Path) -> Result<(), ProviderError> {
    validate_bind_path(storage_dir, "storage directory")?;
    let args = vec![
        "run".to_string(),
        "--rm".to_string(),
        format!("--volume={}:{THROWAWAY_STORAGE_DIR}:rw", storage_dir.display()),
        "--entrypoint=/usr/bin/find".to_string(),
        DOCKUR_IMAGE.to_string(),
        THROWAWAY_STORAGE_DIR.to_string(),
        "-mindepth".to_string(),
        "1".to_string(),
        "-delete".to_string(),
    ];
    run_docker("storage cleanup", &args).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> MacBuilderConfig {
        MacBuilderConfig {
            provider: MachineProvider::DockurMacos,
            ..MacBuilderConfig::default()
        }
    }

    #[test]
    fn docker_create_uses_fixed_argv_with_loopback_ports_and_no_privileged_mode() {
        let disk = MachineDisk::for_machine(&profile(), Path::new("/tmp/buildbridge/x/disk"), None)
            .unwrap();
        let args = create_args(
            "buildbridge-machine-x",
            &profile(),
            &disk,
            Some(&ContainerUsbOptions { plugdev_gid: 46 }),
        );

        assert_eq!(args[0], "create");
        assert!(args.contains(&"--publish=127.0.0.1:50922:22".to_string()));
        assert!(args.contains(&"--publish=127.0.0.1:50923:8006".to_string()));
        assert!(args.contains(&"--volume=/tmp/buildbridge/x/disk:/storage:rw".to_string()));
        assert!(args.contains(&"--device=/dev/net/tun".to_string()));
        assert!(args.contains(&"--cap-add=NET_ADMIN".to_string()));
        assert!(args.contains(&"--device-cgroup-rule=c 189:* rwm".to_string()));
        assert!(args.contains(&"--env=VERSION=15".to_string()));
        assert!(args.contains(&"--env=RAM_SIZE=8G".to_string()));
        assert!(args.contains(&"--env=CPU_CORES=4".to_string()));
        assert!(args.contains(&"--env=DISK_FMT=qcow2".to_string()));
        assert!(args.contains(
            &"--env=QMP=unix:/run/buildbridge-qmp.sock,server=on,wait=off".to_string()
        ));
        assert!(args.contains(&format!(
            "--env=ARGUMENTS=-device usb-ehci,id={USB_PHONE_CONTROLLER}"
        )));
        assert!(!args.iter().any(|arg| arg.contains("--privileged")));
        assert!(!args.iter().any(|arg| arg.starts_with("--group-add")));
        assert!(!args.iter().any(|arg| arg.starts_with("--publish=0.0.0.0")));
        assert_eq!(args.last().map(String::as_str), Some(DOCKUR_IMAGE));
    }

    #[test]
    fn without_usb_no_qemu_arguments_and_no_device_rule_are_passed() {
        let disk = MachineDisk::for_machine(&profile(), Path::new("/tmp/d"), None).unwrap();
        let args = create_args("c", &profile(), &disk, None);

        assert!(!args.iter().any(|arg| arg.starts_with("--env=ARGUMENTS=")));
        assert!(!args.iter().any(|arg| arg.starts_with("--device-cgroup-rule")));
        assert!(!args.iter().any(|arg| arg.contains("/buildbridge-template")));
    }

    #[test]
    fn a_clone_binds_its_template_read_only_where_the_overlay_expects_it() {
        let disk = MachineDisk::for_machine(
            &profile(),
            Path::new("/tmp/d"),
            Some(Path::new("/tmp/templates/base")),
        )
        .unwrap();
        let args = create_args("c", &profile(), &disk, None);
        assert!(args.contains(&"--volume=/tmp/templates/base:/buildbridge-template:ro".to_string()));
    }

    #[test]
    fn every_release_maps_to_the_image_version() {
        assert_eq!(version_env(MacOsRelease::Tahoe), "26");
        assert_eq!(version_env(MacOsRelease::Sequoia), "15");
        assert_eq!(version_env(MacOsRelease::Sonoma), "14");
        assert_eq!(version_env(MacOsRelease::Ventura), "13");
    }

    #[test]
    fn available_memory_is_read_from_meminfo() {
        let meminfo = "MemTotal:       32000000 kB\nMemFree:          500000 kB\nMemAvailable:    2300000 kB\n";
        assert_eq!(parse_mem_available(meminfo), Some(2_300_000 * 1024));
        assert_eq!(parse_mem_available("MemTotal: 1 kB\n"), None);
    }

    #[test]
    fn the_screen_is_served_on_the_port_after_ssh_only_for_this_provider() {
        assert_eq!(profile().display_port(), Some(50923));
        assert_eq!(
            profile().display_url().as_deref(),
            Some("http://127.0.0.1:50923/")
        );
        assert_eq!(MacBuilderConfig::default().display_port(), None);
        assert_eq!(profile().published_ports(), vec![50922, 50923]);
    }
}
