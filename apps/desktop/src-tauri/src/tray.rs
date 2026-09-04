//! System-tray lifecycle controls for the paired runner and every managed macOS machine.

use buildbridge_docker_osx::ContainerState;
use buildbridge_engine::{Engine, MachineRuntime, machine_runtimes};
use tauri::{AppHandle, Manager};

use crate::Desktop;

fn engine(app: &AppHandle) -> Engine {
    app.state::<Desktop>().engine.clone()
}

const TRAY_ID: &str = "main";
const START_PREFIX: &str = "machine-start:";
const STOP_PREFIX: &str = "machine-stop:";

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
        .tooltip(tooltip(app.handle()));
    if let Some(icon) = icon {
        builder = builder.icon(icon);
    }

    builder
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if let Some(machine_id) = id.strip_prefix(START_PREFIX) {
                start_machine(app.clone(), machine_id.to_string());
            } else if let Some(machine_id) = id.strip_prefix(STOP_PREFIX) {
                stop_machine(app.clone(), machine_id.to_string(), false);
            } else {
                match id {
                    "open" => reveal_window(app),
                    "refresh" => engine(app).notify_machines_changed(),
                    "stop-all-and-quit" => stop_all_and_quit(app.clone()),
                    "quit" => app.exit(0),
                    _ => {}
                }
            }
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
    let _ = tray.set_tooltip(Some(tooltip(app)));
}

fn machine_states(app: &AppHandle) -> Vec<MachineRuntime> {
    machine_runtimes(&engine(app))
}

fn is_live(state: ContainerState) -> bool {
    matches!(
        state,
        ContainerState::Running | ContainerState::Paused | ContainerState::Restarting
    )
}

fn menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};

    let states = machine_states(app);
    let any_live = states.iter().any(|entry| is_live(entry.state));
    let mut builder = MenuBuilder::new(app);

    if states.is_empty() {
        let empty = MenuItemBuilder::with_id("no-machines", "No macOS machines yet")
            .enabled(false)
            .build(app)?;
        builder = builder.item(&empty);
    }

    for entry in &states {
        let live = is_live(entry.state);
        let status = MenuItemBuilder::with_id(
            format!("machine-status:{}", entry.id),
            format!("Status: {}", state_label(entry.state)),
        )
        .enabled(false)
        .build(app)?;
        let start = MenuItemBuilder::with_id(
            format!("{START_PREFIX}{}", entry.id),
            start_label(entry.state),
        )
        .enabled(entry.host_ready && !live)
        .build(app)?;
        let stop = MenuItemBuilder::with_id(format!("{STOP_PREFIX}{}", entry.id), "Stop safely")
            .enabled(live)
            .build(app)?;
        let submenu = SubmenuBuilder::new(
            app,
            format!("{} — {}", entry.name, state_label(entry.state)),
        )
        .items(&[&status, &start, &stop])
        .build()?;
        builder = builder.item(&submenu);
    }

    let refresh = MenuItemBuilder::with_id("refresh", "Refresh machine status").build(app)?;
    let open = MenuItemBuilder::with_id("open", "Open BuildBridge").build(app)?;
    let stop_all_and_quit = MenuItemBuilder::with_id(
        "stop-all-and-quit",
        "Stop all machines and quit BuildBridge",
    )
    .enabled(any_live)
    .build(app)?;
    let quit =
        MenuItemBuilder::with_id("quit", "Quit BuildBridge (keep machines running)").build(app)?;

    builder
        .item(&refresh)
        .separator()
        .item(&open)
        .separator()
        .items(&[&stop_all_and_quit, &quit])
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
        ContainerState::Created | ContainerState::Exited => "Resume",
        _ => "Start",
    }
}

fn tooltip(app: &AppHandle) -> String {
    let states = machine_states(app);
    let running = states.iter().filter(|entry| is_live(entry.state)).count();

    match states.len() {
        0 => "BuildBridge — no macOS machines".to_string(),
        total => format!("BuildBridge — {running} of {total} macOS machines running"),
    }
}

fn start_machine(app: AppHandle, machine_id: String) {
    tauri::async_runtime::spawn(async move {
        let result = buildbridge_engine::launch_mac_builder(&engine(&app), machine_id).await;
        refresh(&app);
        if result.is_err() {
            reveal_window(&app);
        }
    });
}

fn stop_machine(app: AppHandle, machine_id: String, quit_after_stop: bool) {
    tauri::async_runtime::spawn(async move {
        let result = buildbridge_engine::stop_mac_builder(&engine(&app), machine_id).await;
        refresh(&app);

        if result.is_ok() && quit_after_stop {
            app.exit(0);
        } else if result.is_err() {
            reveal_window(&app);
        }
    });
}

fn stop_all_and_quit(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut failed = false;
        for entry in machine_states(&app) {
            if is_live(entry.state)
                && buildbridge_engine::stop_mac_builder(&engine(&app), entry.id.clone())
                    .await
                    .is_err()
            {
                failed = true;
            }
        }
        refresh(&app);
        if failed {
            reveal_window(&app);
        } else {
            app.exit(0);
        }
    });
}
