use corral_core::ports::{ProcessSupervisorPort, RepositoryPort};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::state::AppStateHandle;

pub fn setup_system_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let show_item = MenuItem::with_id(app, "show", "Open Corral Dashboard", true, None::<&str>)?;
    let hide_item = MenuItem::with_id(app, "hide", "Minimize to System Tray", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let start_all_item = MenuItem::with_id(
        app,
        "start_all",
        "● Start All Services (Nginx + PHP + DB)",
        true,
        None::<&str>,
    )?;
    let restart_all_item = MenuItem::with_id(
        app,
        "restart_all",
        "⟳ Restart All Services",
        true,
        None::<&str>,
    )?;
    let stop_all_item = MenuItem::with_id(
        app,
        "stop_all",
        "○ Stop All Services",
        true,
        None::<&str>,
    )?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(
        app,
        "quit",
        "Quit Corral (Stop All Daemons)",
        true,
        None::<&str>,
    )?;

    let menu = Menu::with_items(
        app,
        &[
            &show_item,
            &hide_item,
            &sep1,
            &start_all_item,
            &restart_all_item,
            &stop_all_item,
            &sep2,
            &quit_item,
        ],
    )?;

    let mut tray_builder = TrayIconBuilder::with_id("corral-tray")
        .tooltip("Corral — Laravel & PHP Dev Server (Windows x64)")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app_handle, event| {
            let id = event.id().as_ref();
            match id {
                "show" => {
                    restore_and_focus_main_window(app_handle);
                }
                "hide" => {
                    if let Some(w) = app_handle.get_webview_window("main") {
                        let _ = w.hide();
                    }
                }
                "start_all" => {
                    if let Some(state) = app_handle.try_state::<AppStateHandle>() {
                        if let Ok(guard) = state.lock() {
                            if let Ok(list) = guard.repo.list_services() {
                                for s in list {
                                    let _ = guard.supervisor.restart(&s.id);
                                }
                            }
                        }
                    }
                }
                "restart_all" => {
                    if let Some(state) = app_handle.try_state::<AppStateHandle>() {
                        if let Ok(guard) = state.lock() {
                            if let Ok(list) = guard.repo.list_services() {
                                for s in list {
                                    let _ = guard.supervisor.restart(&s.id);
                                }
                            }
                        }
                    }
                }
                "stop_all" => {
                    if let Some(state) = app_handle.try_state::<AppStateHandle>() {
                        if let Ok(guard) = state.lock() {
                            if let Ok(list) = guard.repo.list_services() {
                                for s in list {
                                    let _ = guard.supervisor.stop(&s.id);
                                }
                            }
                        }
                    }
                }
                "quit" => {
                    if let Some(state) = app_handle.try_state::<AppStateHandle>() {
                        if let Ok(guard) = state.lock() {
                            guard.cancel_token.cancel();
                            guard.supervisor.shutdown_all();
                        }
                    }
                    app_handle.exit(0);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                restore_and_focus_main_window(tray.app_handle());
            }
        });

    if let Some(default_icon) = app.default_window_icon() {
        tray_builder = tray_builder.icon(default_icon.clone());
    }

    let _ = tray_builder.build(app)?;
    Ok(())
}

pub fn restore_and_focus_main_window(app_handle: &AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
