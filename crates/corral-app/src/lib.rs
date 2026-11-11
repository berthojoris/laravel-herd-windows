pub mod commands;
pub mod state;
pub mod tray;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use state::{AppServices, AppStateHandle};
use tauri::{Manager, WindowEvent};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let services = AppServices::bootstrap()?;
    let start_minimized = services
        .read_settings()
        .map(|s| s.start_minimized)
        .unwrap_or(false);
    let state_handle: AppStateHandle = Arc::new(services);

    tauri::Builder::default()
        .manage(state_handle)
        .setup(move |app| {
            tray::setup_system_tray(app.handle())?;
            if start_minimized {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                if let Some(state) = window.app_handle().try_state::<AppStateHandle>() {
                    if state.shutting_down.load(Ordering::SeqCst) {
                        return;
                    }
                    if state.close_to_tray.load(Ordering::Relaxed) {
                        api.prevent_close();
                        let _ = window.hide();
                        return;
                    }
                    api.prevent_close();
                    let _ = window.hide();
                    if !state.shutting_down.swap(true, Ordering::SeqCst) {
                        let svc = state.inner().clone();
                        let app_handle = window.app_handle().clone();
                        svc.cancel_token.cancel();
                        std::thread::spawn(move || {
                            svc.supervisor.shutdown_all();
                            app_handle.exit(0);
                        });
                    }
                } else {
                    window.app_handle().exit(0);
                }
            }
            WindowEvent::Resized(_) => {
                if window.is_minimized().unwrap_or(false) {
                    if let Some(state) = window.app_handle().try_state::<AppStateHandle>() {
                        if state.minimize_to_tray.load(Ordering::Relaxed) {
                            let _ = window.hide();
                        }
                    }
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            // app (6 + 3 helpers)
            commands::app_info,
            commands::get_settings,
            commands::save_settings_cmd,
            commands::minimize_to_tray,
            commands::open_path,
            commands::open_url,
            commands::set_theme,
            commands::autostart_get,
            commands::autostart_set,
            // sites (12 + 7 Laravel Optimizer / Env / Workers)
            commands::list_sites,
            commands::get_site,
            commands::create_site,
            commands::delete_site,
            commands::link_folder,
            commands::park_folder,
            commands::unpark_folder,
            commands::detect_framework,
            commands::set_doc_root,
            commands::set_domain,
            commands::open_in_browser,
            commands::open_in_editor,
            commands::laravel_health_check,
            commands::optimize_laravel_site,
            commands::run_artisan_command,
            commands::toggle_site_worker,
            commands::get_site_env,
            commands::save_site_env,
            commands::autofill_site_env,
            // php (8)
            commands::list_php_versions,
            commands::install_php,
            commands::uninstall_php,
            commands::switch_php_global,
            commands::switch_php_site,
            commands::get_php_ini,
            commands::set_php_ini,
            commands::list_php_extensions,
            // node (4)
            commands::list_node_versions,
            commands::install_node,
            commands::switch_node_site,
            commands::install_composer,
            // services (11)
            commands::list_services,
            commands::start_service,
            commands::stop_service,
            commands::restart_service,
            commands::start_all,
            commands::stop_all,
            commands::service_status,
            commands::set_autostart,
            commands::create_extra_service,
            commands::delete_extra_service,
            commands::service_logs,
            // db (6)
            commands::list_databases,
            commands::create_database,
            commands::drop_database,
            commands::open_adminer,
            commands::db_credentials,
            commands::run_readonly_query,
            // tls (6)
            commands::generate_ca,
            commands::trust_ca,
            commands::untrust_ca,
            commands::enable_https,
            commands::disable_https,
            commands::cert_status,
            // logs (4 + 1 reader)
            commands::tail_log,
            commands::read_log_lines,
            commands::list_log_files,
            commands::clear_log,
            commands::export_log,
            // mail (5 + 1 test)
            commands::list_mails,
            commands::get_mail,
            commands::delete_mail,
            commands::clear_mails,
            commands::mail_settings,
            commands::send_test_mail,
            // dumps (2 + 1 test)
            commands::list_dumps,
            commands::clear_dumps,
            commands::send_test_dump,
            // workspace (4)
            commands::list_workspaces,
            commands::create_workspace,
            commands::delete_workspace,
            commands::apply_workspace_config,
            // system (4)
            commands::doctor,
            commands::helper_status,
            commands::install_helper,
            commands::port_report,
        ])
        .run(tauri::generate_context!())?;

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::commands::{read_log_lines_inner, validate_allowed_log_path};
    use crate::state::AppServices;
    use corral_core::ports::RepositoryPort;

    #[test]
    fn test_app_services_bootstrap_without_tokio_runtime_on_main_thread() {
        let svc = AppServices::bootstrap().unwrap();
        let sites = svc.repo.list_sites().unwrap();
        assert!(!sites.is_empty());
        assert!(svc.sync_routing().is_ok());

        let corral_log = svc.paths.logs_dir.join("corral.log");
        let corral_log_str = corral_log.to_string_lossy().to_string();
        assert!(validate_allowed_log_path(&svc, &corral_log_str).is_ok());
        assert!(read_log_lines_inner(&svc, &corral_log_str).is_ok());

        assert!(validate_allowed_log_path(&svc, r"C:\Windows\WindowsUpdate.log").is_err());
        assert!(validate_allowed_log_path(&svc, r"C:\Windows\win.ini").is_err());
        let traversal = format!("{}\\..\\..\\Windows\\Temp\\evil.log", svc.paths.logs_dir.display());
        assert!(validate_allowed_log_path(&svc, &traversal).is_err());

        svc.cancel_token.cancel();
        svc.supervisor.shutdown_all();
    }
}

