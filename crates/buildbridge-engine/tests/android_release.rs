//! The Android provider's live acceptance: a throwaway Android machine in directories of its
//! own approves a real Capacitor project, synchronizes it, runs the debug build (which prepares
//! the toolchain), then signs a release with an upload key created for the run, and verifies
//! both artifacts landed with the checksums the container reported. It pulls the JDK image and
//! downloads Node, Google's command-line tools, the SDK packages the project needs and the
//! project's dependencies, so it is ignored by default:
//!
//!     BUILDBRIDGE_ANDROID_TEST_PROJECT=/path/to/capacitor-project \
//!     cargo test -p buildbridge-engine --test android_release -- --ignored --nocapture
//!
//! The kit and the vault stay out of it: the keystore is created and used directly, so the
//! host's own signing kits are untouched.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use buildbridge_engine::machines::MachinePaths;
use buildbridge_engine::{
    ApproveAndroidWorkspaceInput, ConfirmInput, Engine, EngineDeps, EventSink, MachineConfig,
    MachineProvider,
};
use buildbridge_machines::{
    AndroidSigningMaterial, create_android_keystore, run_signed_android_release,
};
use serde_json::{Value, json};

struct Progress;

impl EventSink for Progress {
    fn emit(&self, event: &str, payload: Value) {
        if event == buildbridge_engine::MACHINE_CHANGED_EVENT {
            return;
        }
        if let Some(line) = payload.get("logLine").and_then(Value::as_str) {
            eprintln!("    {line}");
        } else if let Some(phase) = payload.get("phase").and_then(Value::as_str) {
            eprintln!(
                "[{}] {phase} · {}",
                event,
                payload
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
        }
    }
}

fn as_json<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("engine results serialize")
}

async fn release_a_real_project() -> Result<(), String> {
    let project = std::env::var("BUILDBRIDGE_ANDROID_TEST_PROJECT").map_err(|_| {
        "set BUILDBRIDGE_ANDROID_TEST_PROJECT to a Capacitor project with an android directory"
            .to_string()
    })?;
    let base = std::env::var_os("BUILDBRIDGE_BOOT_TEST_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!("buildbridge-android-test-{}", std::process::id()));
    let engine = Engine::new(EngineDeps {
        config_dir: root.join("config"),
        data_dir: root.join("data"),
        events: Arc::new(Progress),
    });
    let profile = MachineConfig {
        name: "Android release test".to_string(),
        memory_gib: 6,
        cpu_cores: 4,
        provider: MachineProvider::AndroidToolchain,
        ..MachineConfig::default()
    };
    let list = as_json(buildbridge_engine::create_machine(&engine, profile, None).await?);
    let machine_id = list["machines"][0]["id"]
        .as_str()
        .ok_or("the created machine has no id")?
        .to_string();
    let paths = MachinePaths::resolve(&engine, &machine_id)?;

    let outcome = async {
        let started = Instant::now();
        let view = as_json(buildbridge_engine::launch_machine(&engine, machine_id.clone()).await?);
        eprintln!(
            "== container {} after {:?}",
            view["runtime"]["state"],
            started.elapsed()
        );
        if view["runtime"]["state"] != "running" {
            return Err(format!("the container is {}", view["runtime"]["state"]));
        }

        let input: ApproveAndroidWorkspaceInput =
            serde_json::from_value(json!({ "path": project })).unwrap();
        let view = as_json(
            buildbridge_engine::approve_android_workspace(&engine, machine_id.clone(), input)
                .await?,
        );
        eprintln!(
            "== approved {} ({})",
            view["android"]["workspace"]["name"], view["android"]["workspace"]["applicationId"]
        );

        let synced =
            as_json(buildbridge_engine::sync_android_workspace(&engine, machine_id.clone()).await?);
        eprintln!(
            "== synchronized {} files, {} bytes, snapshot {} after {:?}",
            synced["sync"]["sourceFileCount"],
            synced["sync"]["sourceBytes"],
            synced["sync"]["snapshotSha256"],
            started.elapsed()
        );

        let built = as_json(
            buildbridge_engine::run_android_debug_build(&engine, machine_id.clone(), false, None)
                .await?,
        );
        eprintln!(
            "== debug build: {} {} ({}) with {} / build tools {} after {:?}",
            built["build"]["applicationId"],
            built["build"]["versionName"],
            built["build"]["versionCode"],
            built["build"]["toolchain"]["jdkVersion"],
            built["build"]["toolchain"]["buildToolsVersion"],
            started.elapsed()
        );
        if built["view"]["android"]["workspace"]["lastBuildSucceeded"] != true {
            return Err("the workspace does not record the successful build".to_string());
        }
        let debug_apk = built["build"]["apk"]["path"]
            .as_str()
            .ok_or("the debug build kept no APK")?
            .to_string();
        let debug_bytes = fs::metadata(&debug_apk)
            .map_err(|error| format!("the debug APK is not on this host: {error}"))?
            .len();
        eprintln!("   debug APK {debug_apk} ({debug_bytes} bytes)");
        if Some(debug_bytes) != built["build"]["apk"]["bytes"].as_u64() {
            return Err("the debug APK's size does not match its record".to_string());
        }

        let keystore_path = root.join("upload.keystore");
        let keystore = create_android_keystore(
            &keystore_path,
            "test-secret-1",
            "upload",
            "buildbridge Test",
        )
        .map_err(|error| error.to_string())?;
        eprintln!(
            "== keystore {} certificate {}",
            keystore.path, keystore.certificate_sha256
        );

        let output = root.join("artifacts");
        fs::create_dir_all(&output).map_err(|error| error.to_string())?;
        let container = paths.container_name.clone();
        let material = AndroidSigningMaterial {
            keystore_path,
            keystore_password: "test-secret-1".to_string(),
            key_alias: "upload".to_string(),
            key_password: "test-secret-1".to_string(),
        };
        let release = tokio::task::spawn_blocking(move || {
            run_signed_android_release(
                &container,
                &material,
                None,
                buildbridge_machines::AndroidReleaseOutputs::Both,
                None,
                &output,
                |progress| {
                    if let Some(line) = &progress.log_line {
                        eprintln!("    {line}");
                    } else {
                        eprintln!("[release] {:?} · {}", progress.phase, progress.detail);
                    }
                },
            )
            .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())??;
        eprintln!(
            "== release: {} {} ({}) key {} certificate {} after {:?}",
            release.application_id,
            release.version_name,
            release.version_code,
            release.key_alias,
            release.certificate_sha256,
            started.elapsed()
        );
        let aab = release
            .aab
            .as_ref()
            .expect("both outputs includes the app bundle");
        let apk = release.apk.as_ref().expect("both outputs includes the APK");
        eprintln!(
            "   bundle {} ({} bytes, {})",
            aab.path, aab.bytes, aab.sha256
        );
        eprintln!(
            "   apk    {} ({} bytes, {})",
            apk.path, apk.bytes, apk.sha256
        );
        if release.certificate_sha256 != keystore.certificate_sha256 {
            return Err(
                "the release was signed with a different certificate than the keystore holds"
                    .to_string(),
            );
        }
        for artifact in release.artifacts() {
            let bytes = fs::metadata(&artifact.path)
                .map_err(|error| error.to_string())?
                .len();
            if bytes != artifact.bytes {
                return Err(format!(
                    "{} is {bytes} bytes, not {}",
                    artifact.path, artifact.bytes
                ));
            }
            let sum = std::process::Command::new("sha256sum")
                .arg(&artifact.path)
                .output()
                .map_err(|error| error.to_string())?;
            let sum = String::from_utf8_lossy(&sum.stdout);
            if !sum.starts_with(&artifact.sha256) {
                return Err(format!("{} does not match its checksum", artifact.path));
            }
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

#[test]
#[ignore = "builds and signs a real project in a container; run on purpose with --ignored"]
fn an_android_machine_releases_a_real_project() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(release_a_real_project())
        .unwrap_or_else(|error| panic!("{error}"));
}
