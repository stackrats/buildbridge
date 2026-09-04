//! The desktop: a window over the engine. Every command here hands its arguments to the same
//! engine function a command line or a daemon would call; the only things the desktop owns are
//! the window, the tray, and forwarding the engine's events to the webview.

use std::sync::Arc;

use buildbridge_engine::*;
use serde_json::Value;
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
        Err("The web inspector is only built into development builds of BuildBridge.".to_string())
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
async fn list_machines(desktop: State<'_, Desktop>) -> Result<MachineListView, String> {
    buildbridge_engine::list_machines(&desktop.engine).await
}

#[tauri::command]
async fn create_machine(
    desktop: State<'_, Desktop>,
    profile: MacBuilderConfig,
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
) -> Result<MacBuilderView, String> {
    buildbridge_engine::discard_machine_container(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn get_mac_builder_status(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::get_mac_builder_status(&desktop.engine, machine_id).await
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
) -> Result<MacBuilderView, String> {
    buildbridge_engine::adopt_template_guest(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn configure_mac_builder(
    desktop: State<'_, Desktop>,
    machine_id: String,
    profile: MacBuilderConfig,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::configure_mac_builder(&desktop.engine, machine_id, profile).await
}

#[tauri::command]
async fn launch_mac_builder(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::launch_mac_builder(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn stop_mac_builder(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::stop_mac_builder(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn install_usb_release_rule(
    desktop: State<'_, Desktop>,
) -> Result<buildbridge_docker_osx::HostUsbStatus, String> {
    buildbridge_engine::install_usb_release_rule(&desktop.engine).await
}

#[tauri::command]
async fn remove_usb_release_rule(
    desktop: State<'_, Desktop>,
) -> Result<buildbridge_docker_osx::HostUsbStatus, String> {
    buildbridge_engine::remove_usb_release_rule(&desktop.engine).await
}

#[tauri::command]
async fn migrate_machine_for_usb(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::migrate_machine_for_usb(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn attach_usb_device(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: AttachUsbDeviceInput,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::attach_usb_device(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn rebuild_machine_container(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: ConfirmInput,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::rebuild_machine_container(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn detach_usb_device(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::detach_usb_device(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn list_guest_devices(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::list_guest_devices(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn pair_guest_device(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: PairGuestDeviceInput,
) -> Result<MacBuilderView, String> {
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
) -> Result<MacBuilderView, String> {
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
) -> Result<MacBuilderView, String> {
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
) -> Result<MacBuilderView, String> {
    buildbridge_engine::attach_env_set(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn reveal_env_secrets(set_id: String) -> Result<Vec<EnvVariableSummary>, String> {
    buildbridge_engine::reveal_env_secrets(set_id).await
}

#[tauri::command]
async fn configure_mac_guest_access(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: MacGuestAccessInput,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::configure_mac_guest_access(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn authorize_mac_guest_key(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: AuthorizeMacGuestKeyInput,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::authorize_mac_guest_key(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn trust_mac_builder_guest(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: TrustMacGuestInput,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::trust_mac_builder_guest(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn forget_mac_builder_guest_trust(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::forget_mac_builder_guest_trust(&desktop.engine, machine_id).await
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
) -> Result<MacBuilderView, String> {
    buildbridge_engine::activate_mac_xcode(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn provision_mac_signing(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::provision_mac_signing(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn clear_mac_guest_signing(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::clear_mac_guest_signing(&desktop.engine, machine_id).await
}

#[tauri::command]
async fn approve_apple_workspace(
    desktop: State<'_, Desktop>,
    machine_id: String,
    input: ApproveAppleWorkspaceInput,
) -> Result<MacBuilderView, String> {
    buildbridge_engine::approve_apple_workspace(&desktop.engine, machine_id, input).await
}

#[tauri::command]
async fn clear_apple_workspace(
    desktop: State<'_, Desktop>,
    machine_id: String,
) -> Result<MacBuilderView, String> {
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
) -> Result<RunAppleArchiveResult, String> {
    buildbridge_engine::run_apple_signed_archive(&desktop.engine, machine_id, env_set_id).await
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
) -> Result<MacBuilderView, String> {
    buildbridge_engine::clear_apple_archive(&desktop.engine, machine_id).await
}

pub fn run() {
    tauri::Builder::default()
        // Native file pickers for the paths BuildBridge asks for: signing files, a project
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
            list_machines,
            create_machine,
            delete_machine,
            discard_machine_container,
            get_mac_builder_status,
            open_developer_tools,
            open_safari_web_inspector,
            list_machine_templates,
            save_machine_template,
            delete_machine_template,
            adopt_template_guest,
            configure_mac_builder,
            launch_mac_builder,
            stop_mac_builder,
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
            trust_mac_builder_guest,
            forget_mac_builder_guest_trust,
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
            reveal_apple_archive,
            clear_apple_archive
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
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
            app.manage(Desktop {
                engine: Engine::new(deps),
            });
            let _ = tray::setup(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
