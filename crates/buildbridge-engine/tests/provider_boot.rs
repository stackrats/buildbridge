//! Live boot tests, one per provider. Each creates a throwaway machine in directories of its
//! own, starts it, waits until QEMU answers on the machine's control socket and the guest's
//! screen shows something (the OpenCore picker or Apple's installer), then stops it and removes
//! the container and its files. They pull the pinned image and download Apple's recovery image
//! into the test directory, so they are ignored by default and run on purpose:
//!
//!     cargo test -p buildbridge-engine --test provider_boot -- --ignored --nocapture
//!
//! They need Docker with KVM and the machine's 4 GiB free; the Docker-OSX one needs an X11
//! display for its console window, the dockur/macos one the tun device. Set
//! `BUILDBRIDGE_BOOT_TEST_DIR` to a disk-backed directory when the temp directory is tmpfs.
//! Together they are the check that an image upgrade must pass before its digest changes.
//!
//! The Android toolchain's test is the same shape without a guest: it creates the container,
//! starts it, asks the JDK inside for its version, and removes everything including the home
//! the container wrote as root. It pulls only the JDK image.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use buildbridge_engine::machines::MachinePaths;
use buildbridge_engine::{
    ConfirmInput, Engine, EngineDeps, MacOsRelease, MachineConfig, MachineProvider, NoEvents,
};
use buildbridge_machines::{capture_guest_screen, screen_has_content};
use serde_json::{Value, json};

const QEMU_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const SCREEN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const POLL: Duration = Duration::from_secs(10);

fn test_root(provider: MachineProvider) -> PathBuf {
    let base = std::env::var_os("BUILDBRIDGE_BOOT_TEST_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join(format!("buildbridge-boot-test-{provider:?}-{}", std::process::id()).to_lowercase())
}

fn as_json<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("engine results serialize")
}

/// Where QEMU is told to write the screen, and where the host reads it back: the one directory
/// each provider binds from the host that QEMU can write.
fn screen_paths(provider: MachineProvider, paths: &MachinePaths) -> (String, PathBuf) {
    match provider {
        MachineProvider::DockerOsx => (
            format!("{}/boot-test.ppm", buildbridge_machines::QMP_CONTAINER_DIR),
            paths.qmp_dir().join("boot-test.ppm"),
        ),
        MachineProvider::DockurMacos => (
            "/storage/boot-test.ppm".to_string(),
            paths.disk_dir().join("boot-test.ppm"),
        ),
        MachineProvider::AndroidToolchain => {
            unreachable!("the toolchain container has no screen")
        }
    }
}

/// The Android provider's boot: the container comes up, the JDK answers inside it, and the
/// machine's home is on this host until the machine is deleted.
async fn android_toolchain_boots() -> Result<(), String> {
    let provider = MachineProvider::AndroidToolchain;
    let root = test_root(provider);
    let engine = Engine::new(EngineDeps {
        config_dir: root.join("config"),
        data_dir: root.join("data"),
        events: Arc::new(NoEvents),
    });
    let profile = MachineConfig {
        name: "Boot test Android".to_string(),
        memory_gib: 4,
        cpu_cores: 2,
        provider,
        ..MachineConfig::default()
    };
    let list = as_json(buildbridge_engine::create_machine(&engine, profile, None).await?);
    let machine_id = list["machines"][0]["id"]
        .as_str()
        .ok_or("the created machine has no id")?
        .to_string();
    let paths = MachinePaths::resolve(&engine, &machine_id)?;
    assert!(paths.container_name.starts_with("buildbridge-android-"));

    let outcome = async {
        let started = Instant::now();
        let view = as_json(buildbridge_engine::launch_machine(&engine, machine_id.clone()).await?);
        eprintln!(
            "[Android toolchain] container {} after {:?}",
            view["runtime"]["state"],
            started.elapsed()
        );
        if view["runtime"]["state"] != "running" {
            return Err(format!("the container is {}", view["runtime"]["state"]));
        }
        if view["android"].is_null() || !view["guest"]["ssh"]["reachable"].is_boolean() {
            return Err("the view is not an Android machine's".to_string());
        }
        let java = std::process::Command::new("docker")
            .args(["exec", &paths.container_name, "java", "-version"])
            .output()
            .map_err(|error| error.to_string())?;
        let version = String::from_utf8_lossy(&java.stderr).to_string();
        eprintln!(
            "[Android toolchain] {}",
            version.lines().next().unwrap_or_default()
        );
        if !java.status.success() || !version.contains("openjdk version \"17.") {
            return Err(format!("the JDK did not answer: {version}"));
        }
        if !paths.android_home_dir().is_dir() {
            return Err("the machine's home was not created on this host".to_string());
        }
        Ok::<(), String>(())
    }
    .await;

    let confirmed: ConfirmInput = serde_json::from_value(json!({ "confirmed": true })).unwrap();
    let _ = buildbridge_engine::stop_machine(&engine, machine_id.clone()).await;
    let discard =
        buildbridge_engine::discard_machine_container(&engine, machine_id.clone(), confirmed)
            .await
            .map(|_| ());
    let confirmed: ConfirmInput = serde_json::from_value(json!({ "confirmed": true })).unwrap();
    let delete = buildbridge_engine::delete_machine(&engine, machine_id, confirmed)
        .await
        .map(|_| ());
    let home_gone = !paths.android_home_dir().exists();
    let _ = fs::remove_dir_all(&root);

    outcome?;
    discard.map_err(|error| format!("discarding the test machine failed: {error}"))?;
    delete.map_err(|error| format!("deleting the test machine failed: {error}"))?;
    if !home_gone {
        return Err("the machine's home survived its deletion".to_string());
    }

    Ok(())
}

async fn boot_to_the_installer(
    provider: MachineProvider,
    macos_release: MacOsRelease,
    ssh_port: u16,
) -> Result<(), String> {
    let root = test_root(provider);
    let engine = Engine::new(EngineDeps {
        config_dir: root.join("config"),
        data_dir: root.join("data"),
        events: Arc::new(NoEvents),
    });
    let profile = MachineConfig {
        name: format!("Boot test {}", provider.label()),
        macos_release,
        memory_gib: 4,
        cpu_cores: 2,
        ssh_port,
        provider,
    };
    let list = as_json(buildbridge_engine::create_machine(&engine, profile, None).await?);
    let machine_id = list["machines"][0]["id"]
        .as_str()
        .ok_or("the created machine has no id")?
        .to_string();

    let outcome = watch_boot(&engine, provider, &machine_id).await;

    // Whatever happened, leave nothing behind: the container, its disk, and the directories.
    let confirmed: ConfirmInput = serde_json::from_value(json!({ "confirmed": true })).unwrap();
    let _ = buildbridge_engine::stop_machine(&engine, machine_id.clone()).await;
    let discard =
        buildbridge_engine::discard_machine_container(&engine, machine_id.clone(), confirmed)
            .await
            .map(|_| ());
    let confirmed: ConfirmInput = serde_json::from_value(json!({ "confirmed": true })).unwrap();
    let delete = buildbridge_engine::delete_machine(&engine, machine_id, confirmed)
        .await
        .map(|_| ());
    let _ = fs::remove_dir_all(&root);

    outcome?;
    discard.map_err(|error| format!("discarding the test machine failed: {error}"))?;
    delete.map_err(|error| format!("deleting the test machine failed: {error}"))
}

async fn watch_boot(
    engine: &Engine,
    provider: MachineProvider,
    machine_id: &str,
) -> Result<(), String> {
    let started = Instant::now();
    let view = as_json(buildbridge_engine::launch_machine(engine, machine_id.to_string()).await?);
    eprintln!(
        "[{}] container {} after {:?}",
        provider.label(),
        view["runtime"]["state"],
        started.elapsed()
    );

    // QEMU answering on the control socket is the machine being up at all.
    loop {
        let view = as_json(buildbridge_engine::get_machine(engine, machine_id.to_string()).await?);
        let state = view["runtime"]["state"].as_str().unwrap_or_default();
        if state != "running" && state != "created" {
            return Err(format!(
                "the container is {state} after {:?}",
                started.elapsed()
            ));
        }
        if view["usb"]["qmpReachable"].as_bool() == Some(true) {
            eprintln!(
                "[{}] QEMU answers after {:?}",
                provider.label(),
                started.elapsed()
            );
            break;
        }
        if started.elapsed() > QEMU_TIMEOUT {
            return Err(format!(
                "QEMU did not answer on the control socket within {QEMU_TIMEOUT:?}; last logs: {}",
                view["logs"]
            ));
        }
        tokio::time::sleep(POLL).await;
    }

    // The screen lighting up is the guest reaching the OpenCore picker or the installer.
    let paths = MachinePaths::resolve(engine, machine_id)?;
    let endpoint = paths.qmp_endpoint(provider);
    let (in_qemu, on_host) = screen_paths(provider, &paths);
    let screen_started = Instant::now();
    loop {
        let captured = tokio::task::spawn_blocking({
            let endpoint = endpoint.clone();
            let in_qemu = in_qemu.clone();
            move || capture_guest_screen(&endpoint, &in_qemu)
        })
        .await
        .map_err(|error| error.to_string())?;
        if let Err(error) = captured {
            eprintln!("[{}] screen capture: {error}", provider.label());
        } else if fs::read(&on_host)
            .map(|ppm| screen_has_content(&ppm))
            .unwrap_or(false)
        {
            eprintln!(
                "[{}] the screen is lit after {:?}",
                provider.label(),
                started.elapsed()
            );
            let _ = fs::remove_file(&on_host);
            return Ok(());
        }
        if screen_started.elapsed() > SCREEN_TIMEOUT {
            return Err(format!(
                "the guest's screen stayed dark for {SCREEN_TIMEOUT:?} after QEMU answered"
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

fn run(test: impl std::future::Future<Output = Result<(), String>>) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(test)
        .unwrap_or_else(|error| panic!("{error}"));
}

#[test]
#[ignore = "boots a real macOS machine; run on purpose with --ignored"]
fn docker_osx_boots_to_the_installer() {
    run(boot_to_the_installer(
        MachineProvider::DockerOsx,
        MacOsRelease::Sequoia,
        50_970,
    ));
}

#[test]
#[ignore = "boots a real macOS machine; run on purpose with --ignored"]
fn dockur_macos_boots_to_the_installer() {
    run(boot_to_the_installer(
        MachineProvider::DockurMacos,
        MacOsRelease::Sequoia,
        50_980,
    ));
}

#[test]
#[ignore = "pulls the JDK image and starts a container; run on purpose with --ignored"]
fn android_toolchain_boots_and_answers() {
    run(android_toolchain_boots());
}
