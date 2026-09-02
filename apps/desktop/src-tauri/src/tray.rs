//! System-tray lifecycle controls for the paired runner and managed macOS builder.

use buildbridge_docker_osx::ContainerState;
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

const TRAY_ID: &str = "main";

pub(crate) fn reveal_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

pub(crate) fn setup(app: &tauri::App) -> tauri::Result<()> {
    use tauri::tray::TrayIconBuilder;

    let icon = app.default_window_icon().map(|icon| {
        tauri::image::Image::new_owned(icon.rgba().to_vec(), icon.width(), icon.height())
    });
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu(app.handle())?)
        .show_menu_on_left_click(true)
        .tooltip(tooltip());
    if let Some(icon) = icon {
        builder = builder.icon(icon);
    }

    builder
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => reveal_window(app),
            "builder-start" => start_builder(app.clone()),
            "builder-stop" => stop_builder(app.clone(), false),
            "builder-refresh" => {
                let _ = app.emit("mac-builder-changed", ());
                refresh(app);
            }
            "stop-and-quit" => stop_builder(app.clone(), true),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    Ok(())
}

pub(crate) fn refresh(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    if let Ok(menu) = menu(app) {
        let _ = tray.set_menu(Some(menu));
    }
    let _ = tray.set_tooltip(Some(tooltip()));
}

fn menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};

    let profile_exists = crate::load_mac_builder_config(app).ok().flatten().is_some();
    let runtime = buildbridge_docker_osx::status().ok();
    let state = runtime
        .as_ref()
        .map(|runtime| runtime.state)
        .unwrap_or(ContainerState::Unavailable);
    let host_ready = runtime
        .as_ref()
        .is_some_and(|runtime| runtime.prerequisites.ready);
    let live = matches!(
        state,
        ContainerState::Running | ContainerState::Paused | ContainerState::Restarting
    );
    let status = MenuItemBuilder::with_id(
        "builder-status",
        format!("macOS builder — {}", state_label(state)),
    )
    .enabled(false)
    .build(app)?;
    let start = MenuItemBuilder::with_id("builder-start", start_label(state))
        .enabled(profile_exists && host_ready && !live)
        .build(app)?;
    let stop = MenuItemBuilder::with_id("builder-stop", "Stop macOS builder safely")
        .enabled(live)
        .build(app)?;
    let refresh =
        MenuItemBuilder::with_id("builder-refresh", "Refresh machine status").build(app)?;
    let open = MenuItemBuilder::with_id("open", "Open BuildBridge").build(app)?;
    let stop_and_quit =
        MenuItemBuilder::with_id("stop-and-quit", "Stop builder and quit BuildBridge")
            .enabled(live)
            .build(app)?;
    let quit =
        MenuItemBuilder::with_id("quit", "Quit BuildBridge (keep builder running)").build(app)?;

    MenuBuilder::new(app)
        .items(&[&status, &start, &stop, &refresh])
        .separator()
        .item(&open)
        .separator()
        .items(&[&stop_and_quit, &quit])
        .build()
}

fn state_label(state: ContainerState) -> &'static str {
    match state {
        ContainerState::Missing => "not created",
        ContainerState::Created => "created",
        ContainerState::Running => "running",
        ContainerState::Paused => "paused",
        ContainerState::Restarting => "restarting",
        ContainerState::Exited => "stopped",
        ContainerState::Dead => "needs attention",
        ContainerState::Unavailable => "host unavailable",
        ContainerState::Unknown => "unknown",
    }
}

fn start_label(state: ContainerState) -> &'static str {
    match state {
        ContainerState::Created | ContainerState::Exited => "Resume macOS builder",
        _ => "Start macOS builder",
    }
}

fn tooltip() -> String {
    let state = buildbridge_docker_osx::status()
        .map(|runtime| state_label(runtime.state))
        .unwrap_or("host unavailable");

    format!("BuildBridge — macOS builder {state}")
}

fn start_builder(app: AppHandle) {
    let state = app.state::<AppState>();
    if state
        .builder_running
        .swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return;
    }

    tauri::async_runtime::spawn(async move {
        let result = async {
            let profile = crate::load_mac_builder_config(&app)?
                .ok_or_else(|| "Save a macOS builder profile first.".to_string())?;
            let identity_path = crate::mac_builder_identity_path(&app)?;
            tauri::async_runtime::spawn_blocking(move || {
                buildbridge_docker_osx::launch(&profile, &identity_path)
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())??;

            Ok::<_, String>(())
        }
        .await;
        app.state::<AppState>()
            .builder_running
            .store(false, std::sync::atomic::Ordering::Release);
        let _ = app.emit("mac-builder-changed", ());
        refresh(&app);
        if result.is_err() {
            reveal_window(&app);
        }
    });
}

fn stop_builder(app: AppHandle, quit_after_stop: bool) {
    let state = app.state::<AppState>();
    if state
        .builder_running
        .swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return;
    }

    tauri::async_runtime::spawn(async move {
        let result = tauri::async_runtime::spawn_blocking(|| {
            buildbridge_docker_osx::stop().map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result);
        app.state::<AppState>()
            .builder_running
            .store(false, std::sync::atomic::Ordering::Release);
        let _ = app.emit("mac-builder-changed", ());
        refresh(&app);

        if result.is_ok() && quit_after_stop {
            app.exit(0);
        } else if result.is_err() {
            reveal_window(&app);
        }
    });
}
