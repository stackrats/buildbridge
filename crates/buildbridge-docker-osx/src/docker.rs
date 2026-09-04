//! The container's lifecycle through the docker CLI, with fixed argv.

use super::*;

pub fn probe_host() -> HostPrerequisites {
    probe_host_for(MachineProvider::DockerOsx)
}

/// The host checks one provider needs: Docker-OSX shows its screen in a window on this host's
/// X display, dockur/macos serves it as a web page but needs the tun device for its network.
pub fn probe_host_for(provider: MachineProvider) -> HostPrerequisites {
    let supported_host = cfg!(target_os = "linux") && std::env::consts::ARCH == "x86_64";
    let docker_cli_output = Command::new("docker").arg("--version").output();
    let docker_cli = docker_cli_output.is_ok();
    let docker_version = docker_cli_output
        .ok()
        .filter(|output| output.status.success())
        .map(|output| clean_output(&output.stdout));
    let docker_daemon = docker_cli
        && Command::new("docker")
            .args(["info", "--format", "{{.ServerVersion}}"])
            .output()
            .is_ok_and(|output| output.status.success());
    let kvm_access = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok();
    let display = std::env::var("DISPLAY")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let display_access = display.is_some() && std::path::Path::new("/tmp/.X11-unix").is_dir();
    let tun_access = std::path::Path::new("/dev/net/tun").exists();
    let mut issues = Vec::new();

    if !supported_host {
        issues.push(format!(
            "{} currently requires an x86_64 Linux host with KVM.",
            provider.label()
        ));
    }
    if !docker_cli {
        issues.push("Install the Docker CLI before creating a macOS builder.".to_string());
    } else if !docker_daemon {
        issues.push("Start Docker and ensure this user can access the daemon.".to_string());
    }
    if !kvm_access {
        issues.push("Grant this user read/write access to /dev/kvm.".to_string());
    }
    if provider == MachineProvider::DockerOsx && !display_access {
        issues.push("An X11 display is required for the first-boot macOS console.".to_string());
    }
    if provider == MachineProvider::DockurMacos && !tun_access {
        issues.push(
            "dockur/macos needs /dev/net/tun for its network; load the tun module on this host."
                .to_string(),
        );
    }

    HostPrerequisites {
        supported_host,
        docker_cli,
        docker_daemon,
        docker_version,
        kvm_access,
        tun_access,
        display_access,
        display,
        ready: issues.is_empty(),
        issues,
    }
}

/// Reports the host prerequisites and the state of one managed container.
pub fn status(container_name: &str) -> Result<RuntimeStatus, ProviderError> {
    status_for(container_name, MachineProvider::DockerOsx)
}

/// The same, with the host checks of the provider that runs this machine.
pub fn status_for(
    container_name: &str,
    provider: MachineProvider,
) -> Result<RuntimeStatus, ProviderError> {
    let prerequisites = probe_host_for(provider);

    if !prerequisites.docker_daemon {
        return Ok(RuntimeStatus {
            prerequisites,
            state: ContainerState::Unavailable,
            container_id: None,
            started_at: None,
        });
    }

    let (state, container_id, started_at) = inspect_container(container_name)?;

    Ok(RuntimeStatus {
        prerequisites,
        state,
        container_id,
        started_at,
    })
}

/// Creates the managed container when it is missing, then starts it.
///
/// Progress is reported per phase so the desktop can explain a multi-gigabyte image pull
/// instead of showing an unexplained wait.
pub fn launch<F>(
    container_name: &str,
    config: &MacBuilderConfig,
    options: &LaunchOptions<'_>,
    mut on_progress: F,
) -> Result<RuntimeStatus, ProviderError>
where
    F: FnMut(LaunchProgress),
{
    config.validate()?;
    if config.provider == MachineProvider::DockurMacos {
        return dockur::launch(container_name, config, options, on_progress);
    }
    let disk = MachineDisk::for_launch(options)?;
    validate_bind_path(options.qmp_dir, "control socket directory")?;
    let started = Instant::now();
    let mut report = |phase: LaunchPhase, detail: &str| {
        on_progress(LaunchProgress {
            phase,
            elapsed_seconds: started.elapsed().as_secs(),
            detail: detail.to_string(),
        });
    };
    report(LaunchPhase::Preparing, "Checking the host and Docker");
    let prerequisites = probe_host();

    if !prerequisites.ready {
        return Err(ProviderError::Prerequisites(prerequisites.issues.join(" ")));
    }

    let (state, _, _) = inspect_container(container_name)?;

    if state == ContainerState::Missing {
        report(
            LaunchPhase::PullingImage,
            "Pulling the Docker-OSX image; the first pull downloads several gigabytes",
        );
        ensure_image(DOCKER_IMAGE)?;
        report(
            LaunchPhase::GeneratingIdentity,
            "Generating a stable machine identity",
        );
        ensure_identity(options.identity_path)?;
        report(
            LaunchPhase::PreparingDisk,
            "Preparing the macOS disk on this host",
        );
        ensure_machine_disk(&disk)?;
        disk::ensure_control_dir(options.qmp_dir)?;
        report(
            LaunchPhase::CreatingContainer,
            "Creating the managed container",
        );
        create_container(
            container_name,
            config,
            prerequisites.display.as_deref().unwrap_or(":0"),
            options,
            &disk,
        )?;
    } else {
        ensure_manual_restart_policy(container_name)?;
    }

    let (state, _, _) = inspect_container(container_name)?;
    if state != ContainerState::Running {
        // A control directory the daemon would have to create is created as root, and QEMU
        // then cannot bind its socket there; make it before every start, not only the first.
        disk::ensure_control_dir(options.qmp_dir)?;
        report(LaunchPhase::Starting, "Starting the macOS machine");
        run_docker("start", &["start".to_string(), container_name.to_string()])?;
    }

    report(LaunchPhase::Completed, "The macOS machine is running");
    status_for(container_name, config.provider)
}

/// Stops the container gracefully while preserving it and its macOS disk.
pub fn stop(container_name: &str) -> Result<RuntimeStatus, ProviderError> {
    let current = status(container_name)?;

    if current.state == ContainerState::Paused {
        run_docker(
            "unpause",
            &["unpause".to_string(), container_name.to_string()],
        )?;
    }

    if matches!(
        current.state,
        ContainerState::Running | ContainerState::Paused | ContainerState::Restarting
    ) {
        run_docker(
            "stop",
            &[
                "stop".to_string(),
                "--time=30".to_string(),
                container_name.to_string(),
            ],
        )?;
    }

    status(container_name)
}

/// Removes a stopped container together with the macOS disk stored inside it.
///
/// This is deliberately separate from [`stop`]: callers must confirm the data loss and
/// stop the machine first. A live container is never removed implicitly.
pub fn remove(container_name: &str) -> Result<RuntimeStatus, ProviderError> {
    let current = status(container_name)?;

    match current.state {
        ContainerState::Missing | ContainerState::Unavailable => return Ok(current),
        ContainerState::Running | ContainerState::Paused | ContainerState::Restarting => {
            return Err(ProviderError::DockerCommand {
                operation: "container removal",
                message: "stop the machine before discarding its container".to_string(),
            });
        }
        _ => {}
    }

    run_docker(
        "container removal",
        &["rm".to_string(), container_name.to_string()],
    )?;

    status(container_name)
}

pub fn recent_logs(container_name: &str) -> Result<Vec<String>, ProviderError> {
    let current = status(container_name)?;

    if matches!(
        current.state,
        ContainerState::Missing | ContainerState::Unavailable
    ) {
        return Ok(Vec::new());
    }

    let output = run_docker(
        "logs",
        &[
            "logs".to_string(),
            "--tail=80".to_string(),
            container_name.to_string(),
        ],
    )?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(combined
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect())
}

pub(crate) fn inspect_container(
    container_name: &str,
) -> Result<(ContainerState, Option<String>, Option<String>), ProviderError> {
    let output = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{.State.Status}}|{{.Id}}|{{.State.StartedAt}}",
            container_name,
        ])
        .output()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?;

    if !output.status.success() {
        let message = clean_output(&output.stderr);

        if is_missing_container_error(&message) {
            return Ok((ContainerState::Missing, None, None));
        }

        return Err(ProviderError::DockerCommand {
            operation: "inspect",
            message,
        });
    }

    let value = clean_output(&output.stdout);
    let mut fields = value.splitn(3, '|');
    let state = fields.next().unwrap_or_default();
    let container_id = fields.next();
    let started_at = fields.next().and_then(normalize_started_at);

    if container_id.is_none() {
        return Ok((ContainerState::Unknown, None, None));
    }

    Ok((
        ContainerState::from_docker(state),
        container_id.map(str::to_string),
        started_at,
    ))
}

pub(crate) fn normalize_started_at(value: &str) -> Option<String> {
    let value = value.trim();

    (!value.is_empty() && !value.starts_with("0001-")).then(|| value.to_string())
}

pub(crate) fn is_missing_container_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();

    normalized.contains("no such object") || normalized.contains("no such container")
}

pub(crate) fn ensure_image(image: &str) -> Result<(), ProviderError> {
    let exists = Command::new("docker")
        .args(["image", "inspect", image])
        .output()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?
        .status
        .success();

    if !exists {
        run_docker(
            "image pull",
            &["pull".to_string(), image.to_string()],
        )?;
    }

    Ok(())
}

pub(crate) fn ensure_identity(identity_path: &Path) -> Result<(), ProviderError> {
    validate_identity_path(identity_path)?;

    if identity_path.is_file() {
        return validate_identity_file(identity_path);
    }

    let parent = identity_path
        .parent()
        .ok_or_else(|| ProviderError::Identity("identity directory is unavailable".to_string()))?;
    fs::create_dir_all(parent).map_err(|error| ProviderError::Identity(error.to_string()))?;
    let mount = format!("--volume={}:/buildbridge-identity:rw", parent.display());
    let args = vec![
        "run".to_string(),
        "--rm".to_string(),
        mount,
        "--workdir=/home/arch/OSX-KVM/Docker-OSX/osx-serial-generator".to_string(),
        "--entrypoint=/home/arch/OSX-KVM/Docker-OSX/osx-serial-generator/generate-unique-machine-values.sh".to_string(),
        DOCKER_IMAGE.to_string(),
        "--count=1".to_string(),
        "--output-env=/buildbridge-identity/identity.env".to_string(),
    ];
    run_docker("machine identity generation", &args)?;
    validate_identity_file(identity_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(identity_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| ProviderError::Identity(error.to_string()))?;
    }

    Ok(())
}

pub(crate) fn validate_identity_path(identity_path: &Path) -> Result<(), ProviderError> {
    if !identity_path.is_absolute()
        || identity_path.file_name().and_then(|name| name.to_str()) != Some("identity.env")
        || identity_path.to_string_lossy().contains(':')
    {
        return Err(ProviderError::Identity(
            "identity path must be an absolute identity.env path without ':'".to_string(),
        ));
    }

    Ok(())
}

pub(crate) fn validate_identity_file(identity_path: &Path) -> Result<(), ProviderError> {
    let identity = fs::read_to_string(identity_path)
        .map_err(|error| ProviderError::Identity(error.to_string()))?;
    let required_values = [
        "export DEVICE_MODEL=",
        "export SERIAL=",
        "export BOARD_SERIAL=",
        "export UUID=",
        "export MAC_ADDRESS=",
    ];

    if identity.len() > 8_192
        || required_values
            .iter()
            .any(|required| !identity.contains(required))
    {
        return Err(ProviderError::Identity(
            "the generated identity file is incomplete".to_string(),
        ));
    }

    Ok(())
}

pub(crate) fn create_container(
    container_name: &str,
    config: &MacBuilderConfig,
    display: &str,
    options: &LaunchOptions<'_>,
    disk: &MachineDisk,
) -> Result<(), ProviderError> {
    if config.provider == MachineProvider::DockurMacos {
        return dockur::create_container(container_name, config, options);
    }
    validate_identity_path(options.identity_path)?;
    let args = create_args(
        container_name,
        config,
        display,
        options.identity_path,
        disk,
        options.qmp_dir,
        options.usb.as_ref(),
    );
    run_docker("container creation", &args)?;

    Ok(())
}

pub(crate) fn ensure_manual_restart_policy(container_name: &str) -> Result<(), ProviderError> {
    run_docker(
        "restart policy update",
        &[
            "update".to_string(),
            "--restart=no".to_string(),
            container_name.to_string(),
        ],
    )?;

    Ok(())
}

/// The container's fixed argv. The disk, NVRAM and control directory are bound from the host;
/// USB access is a device cgroup rule plus the device tree plus the plugdev group — never
/// `--privileged`. `EXTRA` is word-split by Docker-OSX's `Launch.sh`, so the display and the
/// QMP socket are two flags in one value.
pub(crate) fn create_args(
    container_name: &str,
    config: &MacBuilderConfig,
    display: &str,
    identity_path: &Path,
    disk: &MachineDisk,
    qmp_dir: &Path,
    usb: Option<&ContainerUsbOptions>,
) -> Vec<String> {
    let mut args = vec![
        "create".to_string(),
        format!("--name={container_name}"),
        "--label=dev.buildbridge.managed=true".to_string(),
        "--restart=no".to_string(),
        "--interactive".to_string(),
        "--tty".to_string(),
        "--device=/dev/kvm".to_string(),
        format!("--publish={}:10022", config.ssh_port),
        "--volume=/tmp/.X11-unix:/tmp/.X11-unix:rw".to_string(),
        format!("--volume={}:/env:ro", identity_path.display()),
    ];
    args.extend(disk::disk_bind_args(disk));
    args.push(format!(
        "--volume={}:{QMP_CONTAINER_DIR}:rw",
        qmp_dir.display()
    ));
    if let Some(usb) = usb {
        args.push(format!(
            "--device-cgroup-rule=c {}:* rwm",
            usb::USB_BUS_MAJOR
        ));
        args.push("--volume=/dev/bus/usb:/dev/bus/usb".to_string());
        args.push(format!("--group-add={}", usb.plugdev_gid));
    }
    args.extend([
        format!("--env=DISPLAY={display}"),
        format!("--env=RAM={}", config.memory_gib),
        format!("--env=SMP={}", config.cpu_cores),
        format!("--env=CORES={}", config.cpu_cores),
        "--env=WIDTH=1280".to_string(),
        "--env=HEIGHT=720".to_string(),
        format!("--env=EXTRA={}", qemu_extra_args(usb)),
        format!("--env=SHORTNAME={}", config.macos_release.short_name()),
        "--env=CPU=Haswell-noTSX".to_string(),
        "--env=CPUID_FLAGS=kvm=on,vendor=GenuineIntel,+invtsc,vmware-cpuid-freq=on".to_string(),
        format!(
            "--env=MASTER_PLIST_URL={}",
            config.macos_release.master_plist_url()
        ),
        "--env=GENERATE_SPECIFIC=true".to_string(),
        "--env=GENERATE_UNIQUE=false".to_string(),
        "--env=NOPICKER=false".to_string(),
        DOCKER_IMAGE.to_string(),
    ]);

    args
}

/// QEMU's extra arguments: the console, the control socket, and — whenever the host can pass
/// USB through at all — a dedicated USB 2.0 controller for a phone. The controller is there from
/// the start so that attaching a phone later is a hot-plug over QMP and never a rebuild: on the
/// machine's own emulated xHCI macOS never assigns an iPhone an address, and adding a controller
/// to a running guest is PCI hot-plug, which is not worth asking of macOS. It costs nothing when
/// no phone is attached. The image's launch script word-splits this, so every value is fixed.
pub(crate) fn qemu_extra_args(usb: Option<&ContainerUsbOptions>) -> String {
    let mut extra = format!(
        "-display gtk,zoom-to-fit=on -qmp unix:{QMP_CONTAINER_DIR}/{QMP_SOCKET_NAME},server,nowait"
    );
    if usb.is_some() {
        extra.push_str(&format!(" -device usb-ehci,id={USB_PHONE_CONTROLLER}"));
    }

    extra
}

pub(crate) fn run_docker(
    operation: &'static str,
    args: &[String],
) -> Result<Output, ProviderError> {
    let output = Command::new("docker")
        .args(args)
        .tracked_output()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?;

    if output.status.success() {
        Ok(output)
    } else {
        Err(ProviderError::DockerCommand {
            operation,
            message: clean_output(&output.stderr),
        })
    }
}

/// Reduces `xcodebuild -version` output ("Xcode 26.6\nBuild version 17F113") to the bare
/// version the desktop contract carries ("26.6"); every caller renders its own "Xcode" label.
pub(crate) fn parse_xcode_version(raw: &str) -> String {
    let line = raw.lines().next().unwrap_or_default().trim();
    let version = line.strip_prefix("Xcode").unwrap_or(line).trim();

    if version.is_empty() {
        "version unknown".to_string()
    } else {
        version.to_string()
    }
}

pub(crate) fn clean_output(output: &[u8]) -> String {
    String::from_utf8_lossy(output)
        .trim()
        .chars()
        .take(4_000)
        .collect()
}
