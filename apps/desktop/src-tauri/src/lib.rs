//! The desktop: a window over the engine. Every command here hands its arguments to the same
//! engine function a command line or a daemon would call; the only things the desktop owns are
//! the window, the tray, and forwarding the engine's events to the webview.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use buildbridge_engine::*;
use serde::Serialize;
use serde_json::Value;
use tauri::webview::DownloadEvent;
use tauri::{AppHandle, Emitter, Manager, State};

mod tray;

/// What the window holds: one engine for the process.
pub(crate) struct Desktop {
    pub(crate) engine: Engine,
}

/// Forwards every engine event to the webview under the same name, and refreshes the tray
/// when the machine list changed shape.
struct WindowSink {
    app: AppHandle,
}

impl EventSink for WindowSink {
    fn emit(&self, event: &str, payload: Value) {
        let machines_changed =
            event == MACHINE_CHANGED_EVENT && payload.get("machineId").is_none_or(Value::is_null);
        let _ = self.app.emit(event, payload);
        if machines_changed {
            tray::refresh(&self.app);
        }
    }
}

/// Opens the webview's own inspector — console, network, elements — which a development build
/// of Tauri carries. A release build does not, and says so rather than doing nothing.
/// Opens a machine's screen, which some providers serve as a web page on this host's
/// loopback, in a window of its own named by the machine; a second click focuses it, and a
/// closed one is created again. Only loopback addresses are accepted, because the page gets
/// a window of this app.
#[tauri::command]
fn open_machine_screen(
    app: AppHandle,
    machine_id: String,
    url: String,
    title: String,
) -> Result<(), String> {
    let valid_label = !machine_id.is_empty()
        && machine_id.len() <= 64
        && machine_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid_label {
        return Err("The machine identifier is invalid.".to_string());
    }
    let parsed = tauri::Url::parse(&url)
        .map_err(|error| format!("The screen address is invalid: {error}"))?;
    let loopback = parsed.scheme() == "http"
        && matches!(parsed.host_str(), Some("127.0.0.1") | Some("localhost"));
    if !loopback {
        return Err("The screen is only opened from this host's loopback address.".to_string());
    }
    let label = format!("screen-{machine_id}");
    if let Some(existing) = app.get_webview_window(&label) {
        existing.show().map_err(|error| error.to_string())?;
        existing.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::External(parsed))
        .title(format!("{title} · screen"))
        .inner_size(1280.0, 800.0)
        .build()
        .map_err(|error| format!("The screen window could not be opened: {error}"))?;

    Ok(())
}

/// Where the Xcode archives downloaded through the app land, under the desktop's data.
const XCODE_DOWNLOADS_DIRECTORY: &str = "xcode";
const XCODE_DOWNLOAD_PROGRESS_EVENT: &str = "xcode-download-progress";
const APPLE_DOWNLOADS_URL: &str = "https://developer.apple.com/download/all/";

/// One Xcode archive on its way from Apple into buildbridge's folder. `total_bytes` is unknown
/// while the download runs: the webview reports a request and an end, and the file's size in
/// between is what there is to show.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct XcodeDownloadProgress {
    machine_id: String,
    path: String,
    file_name: String,
    bytes: u64,
    /// `downloading`, `finished` or `failed`.
    state: &'static str,
}

/// The downloads this process is watching, by destination, each with the flag that stops its
/// size poll once the webview says the download ended.
#[derive(Default)]
struct XcodeDownloads(Arc<Mutex<HashMap<PathBuf, Arc<AtomicBool>>>>);

fn emit_xcode_download(
    app: &AppHandle,
    machine_id: &str,
    path: &std::path::Path,
    state: &'static str,
) {
    let bytes = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let _ = app.emit(
        XCODE_DOWNLOAD_PROGRESS_EVENT,
        XcodeDownloadProgress {
            machine_id: machine_id.to_string(),
            path: path.to_string_lossy().to_string(),
            file_name: path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            bytes,
            state,
        },
    );
}

/// Opens Apple's downloads page in a window of this app, so the person signs in with Apple
/// directly and the Xcode `.xip` they download lands in buildbridge's folder with its progress
/// shown in the step, ready to import the moment it is complete. Only `.xip` downloads are
/// captured; the page gets no access to this app. buildbridge sees no Apple credential: the
/// sign-in happens on Apple's page, as it would in any browser.
#[tauri::command]
fn download_xcode(
    app: AppHandle,
    downloads: State<'_, XcodeDownloads>,
    machine_id: String,
    query: String,
) -> Result<(), String> {
    let valid_label = !machine_id.is_empty()
        && machine_id.len() <= 64
        && machine_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid_label {
        return Err("The machine identifier is invalid.".to_string());
    }
    let query = query.trim();
    if query.is_empty()
        || query.len() > 40
        || !query
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.'))
    {
        return Err("The Xcode search is invalid.".to_string());
    }
    let directory = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?
        .join(XCODE_DOWNLOADS_DIRECTORY);
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;

    let label = format!("xcode-download-{machine_id}");
    if let Some(existing) = app.get_webview_window(&label) {
        existing.show().map_err(|error| error.to_string())?;
        existing.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }
    let mut url = tauri::Url::parse(APPLE_DOWNLOADS_URL).map_err(|error| error.to_string())?;
    url.query_pairs_mut().append_pair("q", query);

    let watched = Arc::clone(&downloads.0);
    let handler_app = app.clone();
    let handler_machine = machine_id.clone();
    tauri::WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::External(url))
        .title(format!("Download {query} from Apple"))
        .inner_size(1180.0, 820.0)
        .on_download(move |webview, event| {
            match event {
                DownloadEvent::Requested {
                    url: _,
                    destination,
                } => {
                    let file_name = destination
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let is_xip = file_name.to_ascii_lowercase().ends_with(".xip")
                        && !file_name.contains('/')
                        && !file_name.starts_with('.');
                    if !is_xip {
                        // Anything else the page hands out goes where the webview would put it.
                        return true;
                    }
                    let target = directory.join(&file_name);
                    let _ = std::fs::remove_file(&target);
                    *destination = target.clone();
                    let stop = Arc::new(AtomicBool::new(false));
                    if let Ok(mut map) = watched.lock() {
                        map.insert(target.clone(), Arc::clone(&stop));
                    }
                    emit_xcode_download(&handler_app, &handler_machine, &target, "downloading");
                    // The webview reports no progress of its own; the file growing is it.
                    let poll_app = handler_app.clone();
                    let poll_machine = handler_machine.clone();
                    std::thread::spawn(move || {
                        while !stop.load(Ordering::Acquire) {
                            std::thread::sleep(std::time::Duration::from_millis(500));
                            if !stop.load(Ordering::Acquire) {
                                emit_xcode_download(
                                    &poll_app,
                                    &poll_machine,
                                    &target,
                                    "downloading",
                                );
                            }
                        }
                    });
                    true
                }
                DownloadEvent::Finished {
                    url: _,
                    path,
                    success,
                } => {
                    let Some(path) = path else {
                        return true;
                    };
                    let stop = watched.lock().ok().and_then(|mut map| map.remove(&path));
                    let Some(stop) = stop else {
                        return true;
                    };
                    stop.store(true, Ordering::Release);
                    if success {
                        emit_xcode_download(&handler_app, &handler_machine, &path, "finished");
                        // The archive is here; the window has done its job.
                        let _ = webview.window().close();
                    } else {
                        let _ = std::fs::remove_file(&path);
                        emit_xcode_download(&handler_app, &handler_machine, &path, "failed");
                    }
                    true
                }
                _ => true,
            }
        })
        .build()
        .map_err(|error| format!("The download window could not be opened: {error}"))?;

    Ok(())
}

/// Opens a page in this host's browser, the one chosen in Settings; the engine validates the
/// address and starts the browser detached.
#[tauri::command]
async fn open_url(desktop: State<'_, Desktop>, url: String) -> Result<(), String> {
    buildbridge_engine::open_url(&desktop.engine, url).await
}

#[tauri::command]
async fn open_android_web_inspector(desktop: State<'_, Desktop>) -> Result<String, String> {
    buildbridge_engine::open_android_inspector(&desktop.engine).await
}

#[tauri::command]
async fn get_host_settings(desktop: State<'_, Desktop>) -> Result<HostSettings, String> {
    buildbridge_engine::get_host_settings(&desktop.engine).await
}

#[tauri::command]
async fn save_host_settings(
    desktop: State<'_, Desktop>,
    input: HostSettings,
) -> Result<HostSettings, String> {
    buildbridge_engine::save_host_settings(&desktop.engine, input).await
}

#[tauri::command]
async fn get_storage_locations(desktop: State<'_, Desktop>) -> Result<StorageLocations, String> {
    buildbridge_engine::get_storage_locations(&desktop.engine).await
}

#[tauri::command]
async fn reveal_storage_directory(
    desktop: State<'_, Desktop>,
    directory: StorageDirectory,
) -> Result<(), String> {
    buildbridge_engine::reveal_storage_directory(&desktop.engine, directory).await
}

/// The window says whether it is visible; the engine samples only while it is.
#[tauri::command]
async fn set_usage_sampling(desktop: State<'_, Desktop>, enabled: bool) -> Result<(), String> {
    buildbridge_engine::set_usage_sampling(&desktop.engine, enabled).await
}

#[tauri::command]
fn open_developer_tools(window: tauri::WebviewWindow) -> Result<(), String> {
    #[cfg(debug_assertions)]
    {
        window.open_devtools();
        Ok(())
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = window;
        Err("The web inspector is only built into development builds of buildbridge.".to_string())
    }
}

#[tauri::command]
async fn get_runner_status(desktop: State<'_, Desktop>) -> Result<DesktopStatus, String> {
    buildbridge_engine::get_runner_status(&desktop.engine).await
}

#[tauri::command]
async fn pair_runner(
    desktop: State<'_, Desktop>,
    input: PairInput,
) -> Result<DesktopStatus, String> {
    buildbridge_engine::pair_runner(&desktop.engine, input).await
}

#[tauri::command]
async fn unpair_runner(desktop: State<'_, Desktop>) -> Result<(), String> {
    buildbridge_engine::unpair_runner(&desktop.engine).await
}

#[tauri::command]
async fn get_realtime_configuration(
    desktop: State<'_, Desktop>,
) -> Result<RealtimeConfiguration, String> {
    buildbridge_engine::get_realtime_configuration(&desktop.engine).await
}

#[tauri::command]
async fn authorize_realtime(
    desktop: State<'_, Desktop>,
    input: AuthorizeRealtimeInput,
) -> Result<serde_json::Value, String> {
    buildbridge_engine::authorize_realtime(&desktop.engine, input).await
}

#[tauri::command]
async fn heartbeat_runner(desktop: State<'_, Desktop>) -> Result<HeartbeatSummary, String> {
    buildbridge_engine::heartbeat_runner(&desktop.engine).await
}

#[tauri::command]
async fn run_once(desktop: State<'_, Desktop>) -> Result<RunOnceResult, String> {
    buildbridge_engine::run_once(&desktop.engine).await
}

#[tauri::command]
async fn get_sharing(desktop: State<'_, Desktop>) -> Result<SharingOverview, String> {
    buildbridge_engine::get_sharing(&desktop.engine).await
}

#[tauri::command]
async fn create_sharing_invitation(
    desktop: State<'_, Desktop>,
    input: ShareMachineInput,
) -> Result<SharingInvitation, String> {
    buildbridge_engine::create_sharing_invitation(&desktop.engine, input).await
}

#[tauri::command]
async fn approve_sharing_grant(
    desktop: State<'_, Desktop>,
    grant_id: String,
) -> Result<(), String> {
    buildbridge_engine::approve_sharing_grant(&desktop.engine, grant_id).await
}

#[tauri::command]
async fn revoke_sharing_grant(desktop: State<'_, Desktop>, grant_id: String) -> Result<(), String> {
    buildbridge_engine::revoke_sharing_grant(&desktop.engine, grant_id).await
}

#[tauri::command]
async fn set_sharing_paused(desktop: State<'_, Desktop>, paused: bool) -> Result<(), String> {
    buildbridge_engine::set_sharing_paused(&desktop.engine, paused).await
}

#[tauri::command]
async fn native_mac_status(
    desktop: State<'_, Desktop>,
    force_refresh: Option<bool>,
) -> Result<NativeMacStatus, String> {
    if force_refresh.unwrap_or(false) {
        buildbridge_engine::refresh_native_mac_status(&desktop.engine).await
    } else {
        buildbridge_engine::native_mac_status(&desktop.engine).await
    }
}

#[tauri::command]
async fn approve_native_mac_project(
    desktop: State<'_, Desktop>,
    input: NativeMacProjectInput,
) -> Result<NativeMacConfig, String> {
    buildbridge_engine::approve_native_mac_project(&desktop.engine, input).await
}

#[tauri::command]
async fn configure_native_mac_signing(
    desktop: State<'_, Desktop>,
    input: NativeMacSigningInput,
) -> Result<NativeMacConfig, String> {
    buildbridge_engine::configure_native_mac_signing(&desktop.engine, input).await
}

#[tauri::command]
async fn run_native_mac_build(
    desktop: State<'_, Desktop>,
    input: NativeMacBuildInput,
) -> Result<NativeMacBuildResult, String> {
    buildbridge_engine::run_native_mac_build(&desktop.engine, input).await
}

#[tauri::command]
async fn cancel_native_mac_build(desktop: State<'_, Desktop>) -> Result<(), String> {
    buildbridge_engine::cancel_native_mac_build(&desktop.engine).await
}

#[tauri::command]
async fn set_native_mac_login(desktop: State<'_, Desktop>, enabled: bool) -> Result<(), String> {
    buildbridge_engine::set_native_mac_login(&desktop.engine, enabled).await
}

#[tauri::command]
async fn reveal_native_mac_artifacts(
    desktop: State<'_, Desktop>,
    path: String,
) -> Result<(), String> {
    buildbridge_engine::reveal_native_mac_artifacts(&desktop.engine, path).await
}

#[tauri::command]
async fn list_machines(desktop: State<'_, Desktop>) -> Result<MachineListView, String> {
    buildbridge_engine::list_machines(&desktop.engine).await
}

#[tauri::command]
async fn create_machine(
    desktop: State<'_, Desktop>,
    profile: MachineConfig,
    template_id: Option<String>,
) -> Result<MachineListView, String> {
    buildbridge_engine::create_machine(&desktop.engine, profile, template_id).await
}

#[tauri::command]
async fn delete_machine(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MachineListView, String> {
    buildbridge_engine::delete_machine(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn discard_machine_container(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MachineView, String> {
    buildbridge_engine::discard_machine_container(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn get_machine(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::get_machine(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn open_safari_web_inspector(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<OpenSafariInspectorResult, String> {
    buildbridge_engine::open_safari_web_inspector(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn list_machine_templates(
    desktop: State<'_, Desktop>,
) -> Result<Vec<MachineTemplateSummary>, String> {
    buildbridge_engine::list_machine_templates(&desktop.engine).await
}

#[tauri::command]
async fn save_machine_template(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: SaveTemplateInput,
) -> Result<MachineTemplateSummary, String> {
    buildbridge_engine::save_machine_template(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn delete_machine_template(
    desktop: State<'_, Desktop>,
    template_id: String,
    input: ConfirmInput,
) -> Result<Vec<MachineTemplateSummary>, String> {
    buildbridge_engine::delete_machine_template(&desktop.engine, template_id, input).await
}

#[tauri::command]
async fn adopt_template_guest(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::adopt_template_guest(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn configure_machine(
    desktop: State<'_, Desktop>,
    machine_id: String,
    profile: MachineConfig,
) -> Result<MachineView, String> {
    buildbridge_engine::configure_machine(&desktop.engine, machine_id, profile).await
}

#[tauri::command]
async fn launch_machine(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::launch_machine(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn stop_machine(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::stop_machine(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn install_usb_release_rule(
    desktop: State<'_, Desktop>,
) -> Result<buildbridge_machines::HostUsbStatus, String> {
    buildbridge_engine::install_usb_release_rule(&desktop.engine).await
}

#[tauri::command]
async fn remove_usb_release_rule(
    desktop: State<'_, Desktop>,
) -> Result<buildbridge_machines::HostUsbStatus, String> {
    buildbridge_engine::remove_usb_release_rule(&desktop.engine).await
}

#[tauri::command]
async fn migrate_machine_for_usb(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MachineView, String> {
    buildbridge_engine::migrate_machine_for_usb(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn attach_usb_device(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: AttachUsbDeviceInput,
) -> Result<MachineView, String> {
    buildbridge_engine::attach_usb_device(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn rebuild_machine_container(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MachineView, String> {
    buildbridge_engine::rebuild_machine_container(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn detach_usb_device(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::detach_usb_device(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn list_guest_devices(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::list_guest_devices(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn pair_guest_device(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: PairGuestDeviceInput,
) -> Result<MachineView, String> {
    buildbridge_engine::pair_guest_device(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn prepare_apple_device_signing(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: PrepareDeviceSigningInput,
) -> Result<PrepareDeviceSigningResult, String> {
    buildbridge_engine::prepare_apple_device_signing(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn run_apple_device_build(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: RunAppleDeviceBuildInput,
) -> Result<RunAppleDeviceResult, String> {
    buildbridge_engine::run_apple_device_build(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn clear_apple_device_run(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::clear_apple_device_run(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn create_apple_development_certificate(
    desktop: State<'_, Desktop>,
    kit_id: String,
    input: CreateAppleCertificateInput,
) -> Result<CreateAppleCertificateResult, String> {
    buildbridge_engine::create_apple_development_certificate(&desktop.engine, kit_id, input).await
}

#[tauri::command]
async fn list_signing_kits(desktop: State<'_, Desktop>) -> Result<Vec<SigningKitSummary>, String> {
    buildbridge_engine::list_signing_kits(&desktop.engine).await
}

/// Credential recovery is available only to the local main UI, never the guest screen or
/// the external pages opened in additional webviews.
fn require_credential_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    let denied = || "Open saved credentials from the main buildbridge window.".to_string();
    let url = window.url().map_err(|_| denied())?;
    let config = window.app_handle().config();
    let dev_url = if cfg!(debug_assertions) {
        config.build.dev_url.as_ref()
    } else {
        None
    };
    if !credential_window_is_trusted(window.label(), &url, dev_url) {
        return Err(denied());
    }
    Ok(())
}

fn credential_window_is_trusted(
    label: &str,
    url: &tauri::Url,
    dev_url: Option<&tauri::Url>,
) -> bool {
    label == "main"
        && url.username().is_empty()
        && url.password().is_none()
        && ((url.port().is_none()
            && matches!(
                (url.scheme(), url.host_str()),
                ("tauri", Some("localhost")) | ("http" | "https", Some("tauri.localhost"))
            ))
            || dev_url.is_some_and(|expected| url.origin() == expected.origin()))
}

#[tauri::command]
async fn list_signing_credentials(
    window: tauri::WebviewWindow,
    kit_id: String,
) -> Result<Vec<SigningCredential>, String> {
    require_credential_window(&window)?;
    buildbridge_engine::list_signing_credentials(kit_id).await
}

#[tauri::command]
async fn reveal_signing_credential(
    window: tauri::WebviewWindow,
    kit_id: String,
    credential_id: String,
) -> Result<String, String> {
    require_credential_window(&window)?;
    buildbridge_engine::reveal_signing_credential(kit_id, credential_id).await
}

#[tauri::command]
async fn export_signing_credential(
    window: tauri::WebviewWindow,
    kit_id: String,
    credential_id: String,
    path: String,
) -> Result<(), String> {
    require_credential_window(&window)?;
    buildbridge_engine::export_signing_credential(kit_id, credential_id, path).await
}

#[tauri::command]
async fn save_signing_kit(
    desktop: State<'_, Desktop>,
    input: SigningKitInput,
) -> Result<Vec<SigningKitSummary>, String> {
    buildbridge_engine::save_signing_kit(&desktop.engine, input).await
}

#[tauri::command]
async fn delete_signing_kit(
    desktop: State<'_, Desktop>,
    kit_id: String,
    input: ConfirmInput,
) -> Result<Vec<SigningKitSummary>, String> {
    buildbridge_engine::delete_signing_kit(&desktop.engine, kit_id, input).await
}

#[tauri::command]
async fn attach_signing_kit(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: AttachSigningKitInput,
) -> Result<MachineView, String> {
    buildbridge_engine::attach_signing_kit(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn verify_apple_developer_team(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<apple_api::AppleTeamVerificationResult, String> {
    buildbridge_engine::verify_apple_developer_team(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn create_apple_replacement_profile(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: CreateAppleProfileInput,
) -> Result<CreateAppleProfileResult, String> {
    buildbridge_engine::create_apple_replacement_profile(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn download_apple_profile(
    desktop: State<'_, Desktop>,
    machine_id: String,
    profile_id: String,
) -> Result<DownloadAppleProfileResult, String> {
    buildbridge_engine::download_apple_profile(&desktop.engine, machine_id, profile_id).await
}

#[tauri::command]
async fn create_apple_distribution_certificate(
    desktop: State<'_, Desktop>,
    kit_id: String,
    input: CreateAppleCertificateInput,
) -> Result<CreateAppleCertificateResult, String> {
    buildbridge_engine::create_apple_distribution_certificate(&desktop.engine, kit_id, input).await
}

#[tauri::command]
async fn list_managed_apple_profiles(
    desktop: State<'_, Desktop>,
) -> Result<Vec<ManagedAppleProfile>, String> {
    buildbridge_engine::list_managed_apple_profiles(&desktop.engine)
}

#[tauri::command]
async fn cancel_machine_operation(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<(), String> {
    buildbridge_engine::cancel_machine_operation(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn list_guest_optimizations(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<GuestOptimizationsView, String> {
    buildbridge_engine::list_guest_optimizations(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn apply_guest_optimization(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: ApplyOptimizationInput,
) -> Result<GuestOptimizationsView, String> {
    buildbridge_engine::apply_guest_optimization(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn list_env_sets(desktop: State<'_, Desktop>) -> Result<Vec<EnvSetSummary>, String> {
    buildbridge_engine::list_env_sets(&desktop.engine).await
}

#[tauri::command]
async fn save_env_set(
    desktop: State<'_, Desktop>,
    input: EnvSetInput,
) -> Result<Vec<EnvSetSummary>, String> {
    buildbridge_engine::save_env_set(&desktop.engine, input).await
}

#[tauri::command]
async fn delete_env_set(
    desktop: State<'_, Desktop>,
    set_id: String,
    input: ConfirmInput,
) -> Result<Vec<EnvSetSummary>, String> {
    buildbridge_engine::delete_env_set(&desktop.engine, set_id, input).await
}

#[tauri::command]
async fn attach_env_set(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: AttachEnvSetInput,
) -> Result<MachineView, String> {
    buildbridge_engine::attach_env_set(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn reveal_env_secrets(
    window: tauri::WebviewWindow,
    set_id: String,
) -> Result<Vec<EnvVariableSummary>, String> {
    require_credential_window(&window)?;
    buildbridge_engine::reveal_env_secrets(set_id).await
}

#[tauri::command]
async fn configure_mac_guest_access(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: MacGuestAccessInput,
) -> Result<MachineView, String> {
    buildbridge_engine::configure_mac_guest_access(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn authorize_mac_guest_key(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: AuthorizeMacGuestKeyInput,
) -> Result<MachineView, String> {
    buildbridge_engine::authorize_mac_guest_key(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn trust_mac_guest(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: TrustMacGuestInput,
) -> Result<MachineView, String> {
    buildbridge_engine::trust_mac_guest(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn forget_mac_guest_trust(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::forget_mac_guest_trust(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn import_mac_xcode_package(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: ImportMacXcodeInput,
) -> Result<ImportMacXcodeResult, String> {
    buildbridge_engine::import_mac_xcode_package(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn activate_mac_xcode(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: ActivateMacXcodeInput,
) -> Result<MachineView, String> {
    buildbridge_engine::activate_mac_xcode(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn provision_mac_signing(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::provision_mac_signing(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn clear_mac_guest_signing(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::clear_mac_guest_signing(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn approve_apple_workspace(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: ApproveAppleWorkspaceInput,
) -> Result<MachineView, String> {
    buildbridge_engine::approve_apple_workspace(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn clear_apple_workspace(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::clear_apple_workspace(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn sync_apple_workspace(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<SyncAppleWorkspaceResult, String> {
    buildbridge_engine::sync_apple_workspace(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn run_apple_smoke_build(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: Option<RunAppleSmokeBuildInput>,
) -> Result<RunAppleSmokeBuildResult, String> {
    buildbridge_engine::run_apple_smoke_build(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn adopt_guest_podfile_lock(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<AdoptPodfileLockResult, String> {
    buildbridge_engine::adopt_guest_podfile_lock(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn run_apple_signed_archive(
    desktop: State<'_, Desktop>,
    machine_id: String,
    env_set_id: Option<String>,
    version: Option<ProjectVersionInput>,
) -> Result<RunAppleArchiveResult, String> {
    buildbridge_engine::run_apple_signed_archive(&desktop.engine, machine_id, env_set_id, version)
        .await
}

#[tauri::command]
async fn upload_apple_archive(
    desktop: State<'_, Desktop>,
    machine_id: String,
    expected_sha256: String,
) -> Result<(), String> {
    buildbridge_engine::upload_apple_archive(&desktop.engine, machine_id, expected_sha256).await
}

#[tauri::command]
async fn reveal_apple_archive(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<(), String> {
    buildbridge_engine::reveal_apple_archive(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn clear_apple_archive(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::clear_apple_archive(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn approve_android_workspace(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: ApproveAndroidWorkspaceInput,
) -> Result<MachineView, String> {
    buildbridge_engine::approve_android_workspace(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn list_android_devices(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<AndroidDevices, String> {
    buildbridge_engine::list_android_devices(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn run_android_device(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: AndroidDeviceRunInput,
) -> Result<RunAndroidDeviceResult, String> {
    buildbridge_engine::run_android_device(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn clear_android_device_run(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::clear_android_device_run(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn google_play_connection(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<GooglePlayConnection, String> {
    buildbridge_engine::google_play_connection(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn configure_google_play(
    desktop: State<'_, Desktop>,
    machine_id: String,
    path: String,
) -> Result<GooglePlayConnection, String> {
    buildbridge_engine::configure_google_play(&desktop.engine, machine_id, path).await
}

#[tauri::command]
async fn export_google_play_credential(
    desktop: State<'_, Desktop>,
    window: tauri::WebviewWindow,
    machine_id: String,
    path: String,
) -> Result<(), String> {
    require_credential_window(&window)?;
    buildbridge_engine::export_google_play_credential(&desktop.engine, machine_id, path).await
}

#[tauri::command]
async fn disconnect_google_play(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<(), String> {
    buildbridge_engine::disconnect_google_play(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn upload_google_play(
    desktop: State<'_, Desktop>,
    machine_id: String,
    expected_sha256: String,
) -> Result<GooglePlayUploadResult, String> {
    buildbridge_engine::upload_google_play(&desktop.engine, machine_id, expected_sha256).await
}

#[tauri::command]
async fn check_store_builds(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: Option<CheckStoreBuildsInput>,
) -> Result<StoreBuildsCheck, String> {
    buildbridge_engine::check_store_builds(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn clear_android_workspace(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::clear_android_workspace(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn sync_android_workspace(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<SyncAndroidWorkspaceResult, String> {
    buildbridge_engine::sync_android_workspace(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn run_android_debug_build(
    desktop: State<'_, Desktop>,
    machine_id: String,
    allow_http: Option<bool>,
    version: Option<ProjectVersionInput>,
) -> Result<RunAndroidBuildResult, String> {
    buildbridge_engine::run_android_debug_build(
        &desktop.engine,
        machine_id,
        allow_http.unwrap_or(false),
        version,
    )
    .await
}

#[tauri::command]
async fn run_android_signed_release(
    desktop: State<'_, Desktop>,
    machine_id: String,
    env_set_id: Option<String>,
    outputs: Option<AndroidReleaseOutputs>,
    version: Option<ProjectVersionInput>,
) -> Result<RunAndroidReleaseResult, String> {
    buildbridge_engine::run_android_signed_release(
        &desktop.engine,
        machine_id,
        env_set_id,
        outputs,
        version,
    )
    .await
}

#[tauri::command]
async fn reveal_android_release(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<(), String> {
    buildbridge_engine::reveal_android_release(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn clear_android_release(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MachineView, String> {
    buildbridge_engine::clear_android_release(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn reveal_android_debug_apk(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<(), String> {
    buildbridge_engine::reveal_android_debug_apk(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn create_android_keystore(
    desktop: State<'_, Desktop>,
    kit_id: String,
    input: CreateAndroidKeystoreInput,
) -> Result<CreateAndroidKeystoreResult, String> {
    buildbridge_engine::create_android_keystore(&desktop.engine, kit_id, input).await
}

#[tauri::command]
async fn verify_android_signing_kit(kit_id: String) -> Result<AndroidSigningVerification, String> {
    buildbridge_engine::verify_android_signing_kit(kit_id).await
}

pub fn run() {
    tauri::Builder::default()
        // Native file pickers for the paths buildbridge asks for: signing files, a project
        // folder, and the Xcode archive.
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_runner_status,
            pair_runner,
            unpair_runner,
            get_realtime_configuration,
            authorize_realtime,
            heartbeat_runner,
            run_once,
            get_sharing,
            create_sharing_invitation,
            approve_sharing_grant,
            revoke_sharing_grant,
            set_sharing_paused,
            native_mac_status,
            approve_native_mac_project,
            configure_native_mac_signing,
            run_native_mac_build,
            cancel_native_mac_build,
            set_native_mac_login,
            reveal_native_mac_artifacts,
            list_machines,
            create_machine,
            delete_machine,
            discard_machine_container,
            get_machine,
            open_developer_tools,
            open_machine_screen,
            open_url,
            open_android_web_inspector,
            get_host_settings,
            save_host_settings,
            get_storage_locations,
            reveal_storage_directory,
            set_usage_sampling,
            open_safari_web_inspector,
            list_machine_templates,
            save_machine_template,
            delete_machine_template,
            adopt_template_guest,
            configure_machine,
            launch_machine,
            stop_machine,
            install_usb_release_rule,
            remove_usb_release_rule,
            migrate_machine_for_usb,
            attach_usb_device,
            rebuild_machine_container,
            detach_usb_device,
            list_guest_devices,
            pair_guest_device,
            prepare_apple_device_signing,
            run_apple_device_build,
            clear_apple_device_run,
            create_apple_development_certificate,
            list_signing_kits,
            list_signing_credentials,
            reveal_signing_credential,
            export_signing_credential,
            save_signing_kit,
            delete_signing_kit,
            attach_signing_kit,
            verify_apple_developer_team,
            create_apple_replacement_profile,
            download_apple_profile,
            create_apple_distribution_certificate,
            list_managed_apple_profiles,
            cancel_machine_operation,
            list_guest_optimizations,
            apply_guest_optimization,
            list_env_sets,
            save_env_set,
            delete_env_set,
            attach_env_set,
            reveal_env_secrets,
            configure_mac_guest_access,
            authorize_mac_guest_key,
            trust_mac_guest,
            forget_mac_guest_trust,
            import_mac_xcode_package,
            activate_mac_xcode,
            provision_mac_signing,
            clear_mac_guest_signing,
            approve_apple_workspace,
            clear_apple_workspace,
            sync_apple_workspace,
            run_apple_smoke_build,
            adopt_guest_podfile_lock,
            run_apple_signed_archive,
            upload_apple_archive,
            reveal_apple_archive,
            clear_apple_archive,
            approve_android_workspace,
            list_android_devices,
            run_android_device,
            clear_android_device_run,
            google_play_connection,
            configure_google_play,
            export_google_play_credential,
            disconnect_google_play,
            upload_google_play,
            check_store_builds,
            clear_android_workspace,
            sync_android_workspace,
            run_android_debug_build,
            run_android_signed_release,
            reveal_android_release,
            clear_android_release,
            reveal_android_debug_apk,
            create_android_keystore,
            verify_android_signing_kit,
            download_xcode
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        // The main window is configured hidden: until the webview has a document to draw, the
        // window is an unpainted hole showing whatever is behind it, which reads as a freeze.
        // It appears once the page has loaded, so the first thing on screen is the splash.
        .on_page_load(|webview, payload| {
            if webview.label() == "main"
                && payload.event() == tauri::webview::PageLoadEvent::Finished
            {
                let _ = webview.window().show();
            }
        })
        .setup(|app| {
            // The engine's directories are Tauri's, so the desktop and the command line share
            // one registry, one vault namespace and one set of machines.
            let deps = EngineDeps {
                config_dir: app.path().app_config_dir()?,
                data_dir: app.path().app_local_data_dir()?,
                events: Arc::new(WindowSink {
                    app: app.handle().clone(),
                }),
            };
            let engine = Engine::new(deps);
            let service_engine = engine.clone();
            tauri::async_runtime::spawn(buildbridge_engine::run_runner_service(service_engine));
            app.manage(Desktop { engine });
            app.manage(XcodeDownloads::default());
            let _ = tray::setup(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod credential_window_tests {
    use super::credential_window_is_trusted;

    #[test]
    fn only_local_main_window_can_recover_credentials() {
        let dev_url = tauri::Url::parse("http://localhost:1420").unwrap();
        for address in ["tauri://localhost/index.html", "http://tauri.localhost/"] {
            let url = tauri::Url::parse(address).unwrap();
            assert!(credential_window_is_trusted("main", &url, None));
            assert!(!credential_window_is_trusted("screen-machine", &url, None));
        }
        assert!(credential_window_is_trusted(
            "main",
            &dev_url,
            Some(&dev_url)
        ));
        assert!(!credential_window_is_trusted("main", &dev_url, None));
        for address in [
            "https://developer.apple.com/download/all/",
            "http://127.0.0.1:8006/",
            "http://localhost:1421/",
            "http://tauri.localhost.attacker.example/",
            "http://username@tauri.localhost/",
        ] {
            let url = tauri::Url::parse(address).unwrap();
            assert!(!credential_window_is_trusted("main", &url, Some(&dev_url)));
        }
    }
}
