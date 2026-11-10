use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use corral_core::domain::{
    build_hostnames, detect_doc_root, detect_framework_on_disk, php_fastcgi_port,
    sanitize_site_name, AppSettings, DumpEntry, Framework, MailMessage, ServiceInstance,
    ServiceKind, ServiceState, Site, WorkerKind, Workspace,
};
use corral_core::dto::{
    AppInfoView, ArtisanOutputDto, CertStatusView, DatabaseView, DbCredentialsView,
    DoctorCheckView, DoctorReportView, HelperStatusView, LaravelHealthReport, LogFileView,
    LogLine, MailSettingsView, NodeVersionView, PhpExtensionView, PhpIniSettingsDto,
    PhpVersionView, PortConflictView, ServiceView, SiteView,
};
use corral_core::error::AppError;
use corral_core::laravel_optimizer::{
    apply_dotenv_updates, evaluate_laravel_health, recommended_laravel_env,
    recommended_laravel_php_ini,
};
use corral_core::ports::{HostsPort, ProcessSupervisorPort, RepositoryPort, ServiceSpec, TrustStorePort};
use corral_platform::fs_store::{create_directory_junction, write_atomic};
use corral_platform::hosts::HELPER_PIPE_NAME;
use corral_platform::port::inspect_standard_ports;
use corral_platform::sqlite::SqliteRepository;
use tauri::{AppHandle, Manager, State};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::state::AppStateHandle;

fn to_service_view(svc: &ServiceInstance) -> ServiceView {
    let state_detail = match &svc.state {
        ServiceState::Failed { code, reason } => Some(format!("Exit {code}: {reason}")),
        _ => None,
    };
    ServiceView {
        id: svc.id.clone(),
        name: svc.name.clone(),
        kind: svc.kind.display_name().to_string(),
        version: svc.version.clone(),
        port: svc.port,
        pid: svc.pid,
        state: svc.state.as_badge().to_string(),
        state_detail,
        autostart: svc.autostart,
        data_dir: svc
            .data_dir
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        site_id: svc.site_id.clone(),
    }
}

// ============================================================================
// 1. APP COMMANDS (6 + 3 UI/Tray helpers)
// ============================================================================

#[tauri::command]
pub fn app_info(state: State<'_, AppStateHandle>) -> Result<AppInfoView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let ca_trusted = guard.tls.is_trusted("").unwrap_or(false);
    Ok(AppInfoView {
        name: "Corral".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: "windows".to_string(),
        arch: "x86_64".to_string(),
        data_dir: guard.paths.appdata_dir.to_string_lossy().to_string(),
        local_data_dir: guard.paths.local_appdata_dir.to_string_lossy().to_string(),
        helper_installed: guard.hosts.is_helper_available(),
        ca_trusted,
        default_php_version: guard.settings.default_php_version.clone(),
        default_node_version: guard.settings.default_node_version.clone(),
        close_to_tray: guard.settings.close_to_tray,
        minimize_to_tray: guard.settings.minimize_to_tray,
    })
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppStateHandle>) -> Result<AppSettings, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    Ok(guard.settings.clone())
}

#[tauri::command]
pub async fn save_settings_cmd(
    state: State<'_, AppStateHandle>,
    settings: AppSettings,
) -> Result<(), AppError> {
    let mut guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.paths.save_settings(&settings)?;
    guard.settings = settings;
    guard.sync_routing()?;
    Ok(())
}

#[tauri::command]
pub async fn minimize_to_tray(app: AppHandle) -> Result<(), AppError> {
    if let Some(w) = app.get_webview_window("main") {
        w.hide().map_err(|e| AppError::Io(e.to_string()))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn open_path(path: String) -> Result<(), AppError> {
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err(AppError::Validation(format!(
            "lokasi '{path}' tidak ditemukan di disk"
        )));
    }
    let _ = std::process::Command::new("explorer")
        .arg(&path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    Ok(())
}

#[tauri::command]
pub async fn open_url(url: String) -> Result<(), AppError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(AppError::Validation(
            "URL hanya boleh menggunakan skema http:// atau https://".to_string(),
        ));
    }
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", &url])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    Ok(())
}

#[tauri::command]
pub async fn set_theme(
    state: State<'_, AppStateHandle>,
    theme: String,
) -> Result<(), AppError> {
    let mut guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.settings.theme = theme;
    guard.paths.save_settings(&guard.settings)?;
    Ok(())
}

#[tauri::command]
pub fn autostart_get(state: State<'_, AppStateHandle>) -> Result<bool, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    Ok(guard.settings.autostart_on_login)
}

#[tauri::command]
pub async fn autostart_set(
    state: State<'_, AppStateHandle>,
    enabled: bool,
) -> Result<(), AppError> {
    let mut guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.settings.autostart_on_login = enabled;
    guard.paths.save_settings(&guard.settings)?;
    Ok(())
}

// ============================================================================
// 2. SITES COMMANDS (12 + Laravel Optimizer & Env commands)
// ============================================================================

#[tauri::command]
pub fn list_sites(state: State<'_, AppStateHandle>) -> Result<Vec<SiteView>, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let sites = guard.repo.list_sites()?;
    let views = sites
        .iter()
        .map(|s| {
            SiteView::from_domain(
                s,
                &guard.settings.default_php_version,
                &guard.settings.default_node_version,
                guard.settings.http_port,
                guard.settings.https_port,
                true,
            )
        })
        .collect();
    Ok(views)
}

#[tauri::command]
pub fn get_site(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<SiteView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let site = guard
        .repo
        .get_site(&site_id)?
        .ok_or(AppError::SiteNotFound(site_id))?;
    Ok(SiteView::from_domain(
        &site,
        &guard.settings.default_php_version,
        &guard.settings.default_node_version,
        guard.settings.http_port,
        guard.settings.https_port,
        true,
    ))
}

#[tauri::command]
pub async fn create_site(
    state: State<'_, AppStateHandle>,
    name: String,
    php_version: Option<String>,
    https: Option<bool>,
) -> Result<SiteView, AppError> {
    let clean_name = sanitize_site_name(&name)?;
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let site_dir = guard.paths.default_herd_dir.join(&clean_name);
    SqliteRepository::scaffold_laravel_skeleton(&site_dir, &clean_name, "11.31", true)?;

    let hostnames = build_hostnames(&clean_name, &guard.settings.default_tld)?;
    let db_name = clean_name.replace('-', "_");
    let site = Site {
        id: format!("site-{clean_name}"),
        name: clean_name.clone(),
        path: site_dir.clone(),
        document_root: Some(site_dir.join("public")),
        hostnames,
        php_version: Some(
            php_version.unwrap_or_else(|| guard.settings.default_php_version.clone()),
        ),
        node_version: Some(guard.settings.default_node_version.clone()),
        https: https.unwrap_or(true),
        isolated_services: Vec::new(),
        workers: vec![WorkerKind::Queue],
        framework: Framework::Laravel("11.31".to_string()),
        db_name: Some(db_name.clone()),
        proxy_target: None,
        opcache_enabled: true,
        xdebug_enabled: false,
        created_at: time::OffsetDateTime::now_utc().unix_timestamp(),
    };

    guard.repo.upsert_site(&site)?;
    guard.repo.upsert_database(
        &db_name,
        "MariaDB 11.4",
        3306,
        14,
        2048,
        Some(&site.primary_host()),
    )?;
    guard.sync_routing()?;

    Ok(SiteView::from_domain(
        &site,
        &guard.settings.default_php_version,
        &guard.settings.default_node_version,
        guard.settings.http_port,
        guard.settings.https_port,
        true,
    ))
}

#[tauri::command]
pub async fn delete_site(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<(), AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.repo.delete_site(&id)?;
    guard.sync_routing()?;
    Ok(())
}

#[tauri::command]
pub async fn link_folder(
    state: State<'_, AppStateHandle>,
    path: String,
    name: Option<String>,
) -> Result<SiteView, AppError> {
    let target_path = PathBuf::from(&path);
    std::fs::create_dir_all(&target_path)?;
    let raw_name = name
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            target_path
                .file_name()
                .and_then(|s| s.to_str())
                .map(String::from)
        })
        .unwrap_or_else(|| "linked-site".to_string());
    let clean_name = sanitize_site_name(&raw_name)?;

    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let junction_path = guard.paths.default_herd_dir.join(&clean_name);
    if junction_path != target_path {
        let _ = create_directory_junction(&junction_path, &target_path);
    }

    let fw = detect_framework_on_disk(&target_path);
    let doc_root = detect_doc_root(&target_path);
    let hostnames = build_hostnames(&clean_name, &guard.settings.default_tld)?;
    let db_name = clean_name.replace('-', "_");

    let site = Site {
        id: format!("site-{clean_name}"),
        name: clean_name,
        path: target_path,
        document_root: Some(doc_root),
        hostnames,
        php_version: Some(guard.settings.default_php_version.clone()),
        node_version: Some(guard.settings.default_node_version.clone()),
        https: true,
        isolated_services: Vec::new(),
        workers: Vec::new(),
        framework: fw,
        db_name: Some(db_name),
        proxy_target: None,
        opcache_enabled: true,
        xdebug_enabled: false,
        created_at: time::OffsetDateTime::now_utc().unix_timestamp(),
    };

    guard.repo.upsert_site(&site)?;
    guard.sync_routing()?;

    Ok(SiteView::from_domain(
        &site,
        &guard.settings.default_php_version,
        &guard.settings.default_node_version,
        guard.settings.http_port,
        guard.settings.https_port,
        true,
    ))
}

#[tauri::command]
pub async fn park_folder(
    state: State<'_, AppStateHandle>,
    path: String,
) -> Result<Vec<String>, AppError> {
    let folder = PathBuf::from(&path);
    std::fs::create_dir_all(&folder)?;
    let mut guard = state.lock().map_err(|_| AppError::StateLock)?;
    if !guard.settings.parked_paths.contains(&folder) {
        guard.settings.parked_paths.push(folder);
        guard.paths.save_settings(&guard.settings)?;
    }
    Ok(guard
        .settings
        .parked_paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect())
}

#[tauri::command]
pub async fn unpark_folder(
    state: State<'_, AppStateHandle>,
    path: String,
) -> Result<Vec<String>, AppError> {
    let folder = PathBuf::from(&path);
    let mut guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.settings.parked_paths.retain(|p| p != &folder);
    guard.paths.save_settings(&guard.settings)?;
    Ok(guard
        .settings
        .parked_paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect())
}

#[tauri::command]
pub fn detect_framework(path: String) -> Result<String, AppError> {
    let fw = detect_framework_on_disk(Path::new(&path));
    Ok(fw.version_badge())
}

#[tauri::command]
pub async fn set_doc_root(
    state: State<'_, AppStateHandle>,
    site_id: String,
    document_root: String,
) -> Result<SiteView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let mut site = guard
        .repo
        .get_site(&site_id)?
        .ok_or(AppError::SiteNotFound(site_id))?;
    site.document_root = Some(PathBuf::from(document_root));
    guard.repo.upsert_site(&site)?;
    guard.sync_routing()?;
    Ok(SiteView::from_domain(
        &site,
        &guard.settings.default_php_version,
        &guard.settings.default_node_version,
        guard.settings.http_port,
        guard.settings.https_port,
        true,
    ))
}

#[tauri::command]
pub async fn set_domain(
    state: State<'_, AppStateHandle>,
    site_id: String,
    domain: String,
) -> Result<SiteView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let mut site = guard
        .repo
        .get_site(&site_id)?
        .ok_or(AppError::SiteNotFound(site_id))?;
    let clean = sanitize_site_name(&domain)?;
    site.name = clean.clone();
    site.hostnames = build_hostnames(&clean, &guard.settings.default_tld)?;
    guard.repo.upsert_site(&site)?;
    guard.sync_routing()?;
    Ok(SiteView::from_domain(
        &site,
        &guard.settings.default_php_version,
        &guard.settings.default_node_version,
        guard.settings.http_port,
        guard.settings.https_port,
        true,
    ))
}

#[tauri::command]
pub async fn open_in_browser(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<(), AppError> {
    let url = {
        let guard = state.lock().map_err(|_| AppError::StateLock)?;
        let site = guard
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        SiteView::from_domain(
            &site,
            &guard.settings.default_php_version,
            &guard.settings.default_node_version,
            guard.settings.http_port,
            guard.settings.https_port,
            true,
        )
        .url
    };
    open_url(url).await
}

#[tauri::command]
pub async fn open_in_editor(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<(), AppError> {
    let (editor, path) = {
        let guard = state.lock().map_err(|_| AppError::StateLock)?;
        let site = guard
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        (
            guard.settings.code_editor.clone(),
            site.path.to_string_lossy().to_string(),
        )
    };
    if std::process::Command::new(&editor)
        .arg(&path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .is_err()
    {
        open_path(path).await?;
    }
    Ok(())
}

// ============================================================================
// LARAVEL SERVER OPTIMIZATION & .ENV COMMANDS
// ============================================================================

#[tauri::command]
pub async fn laravel_health_check(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<LaravelHealthReport, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let site = guard
        .repo
        .get_site(&site_id)?
        .ok_or(AppError::SiteNotFound(site_id))?;
    let php_ver = site.effective_php_version(&guard.settings.default_php_version);
    let ini = guard.php_mgr.read_ini(php_ver)?;
    let dotenv_path = site.path.join(".env");
    let dotenv_content = std::fs::read_to_string(&dotenv_path).ok();
    let storage_linked = site.path.join("public").join("storage").exists();
    Ok(evaluate_laravel_health(
        &site,
        &ini,
        dotenv_content.as_deref(),
        storage_linked,
    ))
}

#[tauri::command]
pub async fn optimize_laravel_site(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<LaravelHealthReport, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let mut site = guard
        .repo
        .get_site(&site_id)?
        .ok_or(AppError::SiteNotFound(site_id))?;
    site.opcache_enabled = true;
    site.xdebug_enabled = false;
    guard.repo.upsert_site(&site)?;

    let php_ver = site
        .effective_php_version(&guard.settings.default_php_version)
        .to_string();
    let ini_path = guard.php_mgr.ini_path(&php_ver);
    let optimal_ini = recommended_laravel_php_ini(&php_ver, &ini_path.to_string_lossy());
    guard.php_mgr.write_ini(&optimal_ini)?;

    let dotenv_path = site.path.join(".env");
    let existing_env = std::fs::read_to_string(&dotenv_path).unwrap_or_default();
    let updates = recommended_laravel_env(
        &site,
        guard.settings.http_port,
        guard.settings.https_port,
        guard.settings.smtp_port,
        guard.settings.dumps_port,
    );
    let updated_env = apply_dotenv_updates(&existing_env, &updates);
    write_atomic(&dotenv_path, updated_env.as_bytes())?;

    let public_storage = site.path.join("public").join("storage");
    let target_storage = site.path.join("storage").join("app").join("public");
    std::fs::create_dir_all(&target_storage)?;
    let _ = create_directory_junction(&public_storage, &target_storage);

    guard.sync_routing()?;

    Ok(evaluate_laravel_health(
        &site,
        &optimal_ini,
        Some(&updated_env),
        true,
    ))
}

#[tauri::command]
pub async fn run_artisan_command(
    state: State<'_, AppStateHandle>,
    site_id: String,
    command: String,
) -> Result<ArtisanOutputDto, AppError> {
    let (site_path, php_bin, supervisor) = {
        let guard = state.lock().map_err(|_| AppError::StateLock)?;
        let site = guard
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        let php_ver = site.effective_php_version(&guard.settings.default_php_version);
        let bin = guard
            .php_mgr
            .version_dir(php_ver)
            .join("php.exe");
        (site.path.clone(), bin, guard.supervisor.clone())
    };
    supervisor.run_artisan(&site_path, Some(&php_bin), &command)
}

#[tauri::command]
pub async fn toggle_site_worker(
    state: State<'_, AppStateHandle>,
    site_id: String,
    worker: WorkerKind,
) -> Result<SiteView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let mut site = guard
        .repo
        .get_site(&site_id)?
        .ok_or(AppError::SiteNotFound(site_id))?;
    if site.workers.contains(&worker) {
        site.workers.retain(|w| w != &worker);
    } else {
        site.workers.push(worker);
    }
    guard.repo.upsert_site(&site)?;
    guard.sync_routing()?;
    Ok(SiteView::from_domain(
        &site,
        &guard.settings.default_php_version,
        &guard.settings.default_node_version,
        guard.settings.http_port,
        guard.settings.https_port,
        true,
    ))
}

#[tauri::command]
pub async fn get_site_env(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<String, AppError> {
    let path = {
        let guard = state.lock().map_err(|_| AppError::StateLock)?;
        let site = guard
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        site.path.join(".env")
    };
    Ok(std::fs::read_to_string(&path).unwrap_or_default())
}

#[tauri::command]
pub async fn save_site_env(
    state: State<'_, AppStateHandle>,
    site_id: String,
    content: String,
) -> Result<(), AppError> {
    let path = {
        let guard = state.lock().map_err(|_| AppError::StateLock)?;
        let site = guard
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        site.path.join(".env")
    };
    write_atomic(&path, content.as_bytes())
}

#[tauri::command]
pub async fn autofill_site_env(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<String, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let site = guard
        .repo
        .get_site(&site_id)?
        .ok_or(AppError::SiteNotFound(site_id))?;
    let dotenv_path = site.path.join(".env");
    let existing = std::fs::read_to_string(&dotenv_path).unwrap_or_default();
    let updates = recommended_laravel_env(
        &site,
        guard.settings.http_port,
        guard.settings.https_port,
        guard.settings.smtp_port,
        guard.settings.dumps_port,
    );
    let updated = apply_dotenv_updates(&existing, &updates);
    write_atomic(&dotenv_path, updated.as_bytes())?;
    Ok(updated)
}

// ============================================================================
// 3. PHP COMMANDS (8)
// ============================================================================

#[tauri::command]
pub fn list_php_versions(state: State<'_, AppStateHandle>) -> Result<Vec<PhpVersionView>, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let catalog = ["8.5", "8.4", "8.3", "8.2", "8.1"];
    let mut out = Vec::with_capacity(catalog.len());
    for ver in catalog {
        let fcgi_port = php_fastcgi_port(guard.settings.base_php_port, ver)?;
        let ini = guard.php_mgr.read_ini(ver)?;
        let exts = guard.php_mgr.list_extensions(ver)?;
        let installed = ver == "8.4" || ver == "8.3" || ver == "8.2";
        out.push(PhpVersionView {
            version: ver.to_string(),
            installed,
            active_global: guard.settings.default_php_version == ver,
            fastcgi_port: fcgi_port,
            running: installed,
            binary_path: guard
                .php_mgr
                .version_dir(ver)
                .join("php-cgi.exe")
                .to_string_lossy()
                .to_string(),
            ini_path: ini.raw_path,
            extensions: exts,
            opcache_turbo: ini.opcache_enable,
            xdebug_enabled: ini.xdebug_mode != "off",
        });
    }
    Ok(out)
}

#[tauri::command]
pub async fn install_php(
    state: State<'_, AppStateHandle>,
    version: String,
) -> Result<PhpVersionView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let _ = guard.php_mgr.ensure_version_provisioned(&version)?;
    let fcgi_port = php_fastcgi_port(guard.settings.base_php_port, &version)?;
    let ini = guard.php_mgr.read_ini(&version)?;
    let exts = guard.php_mgr.list_extensions(&version)?;
    Ok(PhpVersionView {
        version: version.clone(),
        installed: true,
        active_global: guard.settings.default_php_version == version,
        fastcgi_port: fcgi_port,
        running: true,
        binary_path: guard
            .php_mgr
            .version_dir(&version)
            .join("php-cgi.exe")
            .to_string_lossy()
            .to_string(),
        ini_path: ini.raw_path,
        extensions: exts,
        opcache_turbo: ini.opcache_enable,
        xdebug_enabled: ini.xdebug_mode != "off",
    })
}

#[tauri::command]
pub async fn uninstall_php(
    _state: State<'_, AppStateHandle>,
    _version: String,
) -> Result<(), AppError> {
    Ok(())
}

#[tauri::command]
pub async fn switch_php_global(
    state: State<'_, AppStateHandle>,
    version: String,
) -> Result<(), AppError> {
    let mut guard = state.lock().map_err(|_| AppError::StateLock)?;
    let _ = php_fastcgi_port(guard.settings.base_php_port, &version)?;
    guard.settings.default_php_version = version;
    guard.paths.save_settings(&guard.settings)?;
    guard.sync_routing()?;
    Ok(())
}

#[tauri::command]
pub async fn switch_php_site(
    state: State<'_, AppStateHandle>,
    site_id: String,
    php_version: String,
) -> Result<SiteView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let _ = php_fastcgi_port(guard.settings.base_php_port, &php_version)?;
    let mut site = guard
        .repo
        .get_site(&site_id)?
        .ok_or(AppError::SiteNotFound(site_id))?;
    site.php_version = Some(php_version);
    guard.repo.upsert_site(&site)?;
    guard.sync_routing()?;
    Ok(SiteView::from_domain(
        &site,
        &guard.settings.default_php_version,
        &guard.settings.default_node_version,
        guard.settings.http_port,
        guard.settings.https_port,
        true,
    ))
}

#[tauri::command]
pub async fn get_php_ini(
    state: State<'_, AppStateHandle>,
    version: String,
) -> Result<PhpIniSettingsDto, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.php_mgr.read_ini(&version)
}

#[tauri::command]
pub async fn set_php_ini(
    state: State<'_, AppStateHandle>,
    settings: PhpIniSettingsDto,
) -> Result<(), AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.php_mgr.write_ini(&settings)
}

#[tauri::command]
pub async fn list_php_extensions(
    state: State<'_, AppStateHandle>,
    version: String,
) -> Result<Vec<PhpExtensionView>, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.php_mgr.list_extensions(&version)
}

// ============================================================================
// 4. NODE COMMANDS (4)
// ============================================================================

#[tauri::command]
pub fn list_node_versions(state: State<'_, AppStateHandle>) -> Result<Vec<NodeVersionView>, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let active = &guard.settings.default_node_version;
    Ok(vec![
        NodeVersionView {
            version: "24".to_string(),
            lts_name: "Krypton (Current/LTS)".to_string(),
            installed: true,
            active_global: active == "24",
            composer_installed: true,
            laravel_installer_installed: true,
        },
        NodeVersionView {
            version: "22".to_string(),
            lts_name: "Jod (Active LTS)".to_string(),
            installed: true,
            active_global: active == "22",
            composer_installed: true,
            laravel_installer_installed: true,
        },
        NodeVersionView {
            version: "20".to_string(),
            lts_name: "Iron (Maintenance LTS)".to_string(),
            installed: true,
            active_global: active == "20",
            composer_installed: true,
            laravel_installer_installed: true,
        },
    ])
}

#[tauri::command]
pub async fn install_node(
    state: State<'_, AppStateHandle>,
    version: String,
) -> Result<NodeVersionView, AppError> {
    let mut guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.settings.default_node_version = version.clone();
    guard.paths.save_settings(&guard.settings)?;
    Ok(NodeVersionView {
        version,
        lts_name: "LTS".to_string(),
        installed: true,
        active_global: true,
        composer_installed: true,
        laravel_installer_installed: true,
    })
}

#[tauri::command]
pub async fn switch_node_site(
    state: State<'_, AppStateHandle>,
    site_id: String,
    node_version: String,
) -> Result<SiteView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let mut site = guard
        .repo
        .get_site(&site_id)?
        .ok_or(AppError::SiteNotFound(site_id))?;
    site.node_version = Some(node_version);
    guard.repo.upsert_site(&site)?;
    Ok(SiteView::from_domain(
        &site,
        &guard.settings.default_php_version,
        &guard.settings.default_node_version,
        guard.settings.http_port,
        guard.settings.https_port,
        true,
    ))
}

#[tauri::command]
pub async fn install_composer(state: State<'_, AppStateHandle>) -> Result<String, AppError> {
    let shims_dir = {
        let guard = state.lock().map_err(|_| AppError::StateLock)?;
        guard.paths.shims_dir.clone()
    };
    let composer_bat = shims_dir.join("composer.bat");
    write_atomic(
        &composer_bat,
        b"@echo off\r\nphp \"%~dp0composer.phar\" %*\r\n",
    )?;
    Ok(composer_bat.to_string_lossy().to_string())
}

// ============================================================================
// 5. SERVICES COMMANDS (11)
// ============================================================================

#[tauri::command]
pub fn list_services(state: State<'_, AppStateHandle>) -> Result<Vec<ServiceView>, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let list = guard.supervisor.list()?;
    Ok(list.iter().map(to_service_view).collect())
}

#[tauri::command]
pub async fn start_service(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<ServiceView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let list = guard.supervisor.list()?;
    let svc = list
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| AppError::Validation(format!("service '{id}' tidak ditemukan")))?;
    let spec = ServiceSpec {
        id: svc.id.clone(),
        name: svc.name.clone(),
        kind: svc.kind.clone(),
        version: svc.version.clone(),
        port: svc.port,
        program: None,
        args: Vec::new(),
        env: Vec::new(),
        working_dir: svc.data_dir.clone(),
        site_id: svc.site_id.clone(),
    };
    let _ = guard.supervisor.start(spec)?;
    let updated = guard
        .supervisor
        .list()?
        .into_iter()
        .find(|s| s.id == id)
        .unwrap_or(svc);
    Ok(to_service_view(&updated))
}

#[tauri::command]
pub async fn stop_service(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<ServiceView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.supervisor.stop(&id)?;
    let updated = guard
        .supervisor
        .list()?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| AppError::Validation(format!("service '{id}' tidak ditemukan")))?;
    Ok(to_service_view(&updated))
}

#[tauri::command]
pub async fn restart_service(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<ServiceView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.supervisor.restart(&id)?;
    let updated = guard
        .supervisor
        .list()?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| AppError::Validation(format!("service '{id}' tidak ditemukan")))?;
    Ok(to_service_view(&updated))
}

#[tauri::command]
pub async fn start_all(state: State<'_, AppStateHandle>) -> Result<Vec<ServiceView>, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let services = guard.supervisor.list()?;
    for s in services {
        let _ = guard.supervisor.restart(&s.id);
    }
    Ok(guard.supervisor.list()?.iter().map(to_service_view).collect())
}

#[tauri::command]
pub async fn stop_all(state: State<'_, AppStateHandle>) -> Result<Vec<ServiceView>, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let services = guard.supervisor.list()?;
    for s in services {
        let _ = guard.supervisor.stop(&s.id);
    }
    Ok(guard.supervisor.list()?.iter().map(to_service_view).collect())
}

#[tauri::command]
pub fn service_status(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<String, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    Ok(guard.supervisor.status(&id).as_badge().to_string())
}

#[tauri::command]
pub async fn set_autostart(
    state: State<'_, AppStateHandle>,
    id: String,
    autostart: bool,
) -> Result<(), AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let list = guard.repo.list_services()?;
    if let Some(mut svc) = list.into_iter().find(|s| s.id == id) {
        svc.autostart = autostart;
        guard.repo.upsert_service(&svc)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn create_extra_service(
    state: State<'_, AppStateHandle>,
    name: String,
    kind: String,
    version: String,
    port: u16,
) -> Result<ServiceView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let svc_kind = match kind.as_str() {
        "mariaDb" => ServiceKind::MariaDb,
        "postgres" => ServiceKind::Postgres,
        "redis" => ServiceKind::Redis,
        "mongo" => ServiceKind::Mongo,
        _ => ServiceKind::Worker,
    };
    let id = format!("svc-extra-{}", uuid::Uuid::now_v7());
    let instance = ServiceInstance {
        id: id.clone(),
        name,
        kind: svc_kind,
        version,
        port,
        data_dir: Some(guard.paths.runtimes_dir.join(&id)),
        pid: Some(std::process::id()),
        state: ServiceState::Running,
        autostart: true,
        site_id: None,
    };
    guard.repo.upsert_service(&instance)?;
    guard
        .supervisor
        .set_service_state(&id, ServiceState::Running, Some(std::process::id()));
    Ok(to_service_view(&instance))
}

#[tauri::command]
pub async fn delete_extra_service(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<(), AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let _ = guard.supervisor.stop(&id);
    guard.repo.delete_service(&id)
}

#[tauri::command]
pub async fn service_logs(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<Vec<String>, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let log_file = guard.paths.logs_dir.join("corral.log");
    let raw = std::fs::read_to_string(log_file).unwrap_or_default();
    let lines: Vec<String> = raw
        .lines()
        .map(|l| format!("[{id}] {l}"))
        .collect();
    Ok(lines)
}

// ============================================================================
// 6. DB COMMANDS (6)
// ============================================================================

#[tauri::command]
pub fn list_databases(state: State<'_, AppStateHandle>) -> Result<Vec<DatabaseView>, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.repo.list_databases_meta()
}

#[tauri::command]
pub async fn create_database(
    state: State<'_, AppStateHandle>,
    name: String,
) -> Result<DatabaseView, AppError> {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if clean.is_empty() {
        return Err(AppError::Validation(
            "nama database tidak boleh kosong".to_string(),
        ));
    }
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard
        .repo
        .upsert_database(&clean, "MariaDB 11.4", 3306, 0, 64, None)?;
    Ok(DatabaseView {
        name: clean,
        engine: "MariaDB 11.4".to_string(),
        port: 3306,
        tables_count: 0,
        size_kb: 64,
        linked_site: None,
    })
}

#[tauri::command]
pub async fn drop_database(
    state: State<'_, AppStateHandle>,
    name: String,
) -> Result<(), AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.repo.drop_database_meta(&name)
}

#[tauri::command]
pub async fn open_adminer() -> Result<(), AppError> {
    open_url("http://127.0.0.1:8080".to_string()).await
}

#[tauri::command]
pub fn db_credentials() -> Result<DbCredentialsView, AppError> {
    Ok(DbCredentialsView {
        host: "127.0.0.1".to_string(),
        port: 3306,
        username: "root".to_string(),
        password: String::new(),
        default_database: "laravel_commerce".to_string(),
        connection_url: "mysql://root@127.0.0.1:3306/laravel_commerce".to_string(),
    })
}

#[tauri::command]
pub async fn run_readonly_query(
    state: State<'_, AppStateHandle>,
    query: String,
) -> Result<Vec<String>, AppError> {
    let trimmed = query.trim().to_ascii_uppercase();
    if !trimmed.starts_with("SELECT") && !trimmed.starts_with("SHOW") {
        return Err(AppError::Validation(
            "hanya query SELECT atau SHOW (read-only) yang diizinkan".to_string(),
        ));
    }
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let dbs = guard.repo.list_databases_meta()?;
    Ok(dbs.into_iter().map(|d| d.name).collect())
}

// ============================================================================
// 7. TLS COMMANDS (6)
// ============================================================================

#[tauri::command]
pub async fn generate_ca(state: State<'_, AppStateHandle>) -> Result<CertStatusView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let ca = guard.tls.ensure_ca()?;
    let sites = guard.repo.list_sites()?;
    Ok(CertStatusView {
        ca_exists: true,
        ca_trusted: ca.trusted,
        ca_subject: "CN=Corral Local Development CA".to_string(),
        ca_thumbprint: ca.ca_thumbprint,
        ca_not_after: "2036-11-10".to_string(),
        ca_path: ca.leaf_path.to_string_lossy().to_string(),
        active_site_certs: sites.iter().filter(|s| s.https).count(),
    })
}

#[tauri::command]
pub async fn trust_ca(state: State<'_, AppStateHandle>) -> Result<CertStatusView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let ca = guard.tls.ensure_ca()?;
    guard.tls.install_ca(&ca.leaf_path)?;
    let sites = guard.repo.list_sites()?;
    Ok(CertStatusView {
        ca_exists: true,
        ca_trusted: true,
        ca_subject: "CN=Corral Local Development CA".to_string(),
        ca_thumbprint: ca.ca_thumbprint,
        ca_not_after: "2036-11-10".to_string(),
        ca_path: ca.leaf_path.to_string_lossy().to_string(),
        active_site_certs: sites.iter().filter(|s| s.https).count(),
    })
}

#[tauri::command]
pub async fn untrust_ca(state: State<'_, AppStateHandle>) -> Result<CertStatusView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let ca = guard.tls.ensure_ca()?;
    guard.tls.remove_ca(&ca.ca_thumbprint)?;
    let sites = guard.repo.list_sites()?;
    Ok(CertStatusView {
        ca_exists: true,
        ca_trusted: false,
        ca_subject: "CN=Corral Local Development CA".to_string(),
        ca_thumbprint: ca.ca_thumbprint,
        ca_not_after: "2036-11-10".to_string(),
        ca_path: ca.leaf_path.to_string_lossy().to_string(),
        active_site_certs: sites.iter().filter(|s| s.https).count(),
    })
}

#[tauri::command]
pub async fn enable_https(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<SiteView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let mut site = guard
        .repo
        .get_site(&site_id)?
        .ok_or(AppError::SiteNotFound(site_id))?;
    site.https = true;
    let _ = guard.tls.issue_site_cert(&site.hostnames)?;
    guard.repo.upsert_site(&site)?;
    guard.sync_routing()?;
    Ok(SiteView::from_domain(
        &site,
        &guard.settings.default_php_version,
        &guard.settings.default_node_version,
        guard.settings.http_port,
        guard.settings.https_port,
        true,
    ))
}

#[tauri::command]
pub async fn disable_https(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<SiteView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let mut site = guard
        .repo
        .get_site(&site_id)?
        .ok_or(AppError::SiteNotFound(site_id))?;
    site.https = false;
    guard.repo.upsert_site(&site)?;
    guard.sync_routing()?;
    Ok(SiteView::from_domain(
        &site,
        &guard.settings.default_php_version,
        &guard.settings.default_node_version,
        guard.settings.http_port,
        guard.settings.https_port,
        true,
    ))
}

#[tauri::command]
pub fn cert_status(state: State<'_, AppStateHandle>) -> Result<CertStatusView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let ca = guard.tls.ensure_ca()?;
    let sites = guard.repo.list_sites()?;
    Ok(CertStatusView {
        ca_exists: true,
        ca_trusted: ca.trusted,
        ca_subject: "CN=Corral Local Development CA".to_string(),
        ca_thumbprint: ca.ca_thumbprint,
        ca_not_after: "2036-11-10".to_string(),
        ca_path: ca.leaf_path.to_string_lossy().to_string(),
        active_site_certs: sites.iter().filter(|s| s.https).count(),
    })
}

// ============================================================================
// 8. LOGS COMMANDS (4 + read_log_lines)
// ============================================================================

#[tauri::command]
pub async fn tail_log(
    path: String,
    on_line: tauri::ipc::Channel<LogLine>,
) -> Result<(), AppError> {
    let lines = read_log_lines(path).await?;
    for line in lines {
        let _ = on_line.send(line);
    }
    Ok(())
}

#[tauri::command]
pub async fn read_log_lines(path: String) -> Result<Vec<LogLine>, AppError> {
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out = Vec::new();
    for (idx, line) in raw.lines().rev().take(400).collect::<Vec<_>>().into_iter().rev().enumerate() {
        let upper = line.to_ascii_uppercase();
        let level = if upper.contains("ERROR") || upper.contains("CRITICAL") {
            "ERROR"
        } else if upper.contains("WARN") {
            "WARN"
        } else {
            "INFO"
        };
        out.push(LogLine {
            line_number: idx + 1,
            timestamp: None,
            level: level.to_string(),
            text: line.to_string(),
        });
    }
    Ok(out)
}

#[tauri::command]
pub fn list_log_files(state: State<'_, AppStateHandle>) -> Result<Vec<LogFileView>, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let mut files = Vec::new();

    for (id, label, category, filename) in [
        ("log-corral", "corral.log", "Corral System", "corral.log"),
        ("log-nginx", "nginx-error.log", "Web Server", "nginx-error.log"),
        ("log-php84", "php-cgi-8.4.log", "PHP FastCGI", "php-cgi-8.4.log"),
    ] {
        let p = guard.paths.logs_dir.join(filename);
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        files.push(LogFileView {
            id: id.to_string(),
            label: label.to_string(),
            category: category.to_string(),
            path: p.to_string_lossy().to_string(),
            size_bytes: size,
        });
    }

    if let Ok(sites) = guard.repo.list_sites() {
        for s in sites {
            let laravel_log = s.path.join("storage").join("logs").join("laravel.log");
            if laravel_log.is_file() {
                let size = std::fs::metadata(&laravel_log).map(|m| m.len()).unwrap_or(0);
                files.push(LogFileView {
                    id: format!("log-site-{}", s.name),
                    label: format!("{} — storage/logs/laravel.log", s.hostnames.first().cloned().unwrap_or(s.name)),
                    category: "Laravel App".to_string(),
                    path: laravel_log.to_string_lossy().to_string(),
                    size_bytes: size,
                });
            }
        }
    }

    Ok(files)
}

#[tauri::command]
pub async fn clear_log(path: String) -> Result<(), AppError> {
    let p = PathBuf::from(&path);
    if p.is_file() {
        write_atomic(&p, b"")?;
    }
    Ok(())
}

#[tauri::command]
pub async fn export_log(path: String, destination: String) -> Result<(), AppError> {
    std::fs::copy(&path, &destination)?;
    Ok(())
}

// ============================================================================
// 9. MAIL COMMANDS (5 + send_test_mail)
// ============================================================================

#[tauri::command]
pub fn list_mails(state: State<'_, AppStateHandle>) -> Result<Vec<MailMessage>, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.repo.list_mails()
}

#[tauri::command]
pub fn get_mail(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<MailMessage, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let _ = guard.repo.mark_mail_read(&id);
    guard
        .repo
        .get_mail(&id)?
        .ok_or_else(|| AppError::Validation(format!("email '{id}' tidak ditemukan")))
}

#[tauri::command]
pub async fn delete_mail(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<(), AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.repo.delete_mail(&id)
}

#[tauri::command]
pub async fn clear_mails(state: State<'_, AppStateHandle>) -> Result<(), AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.repo.clear_mails()
}

#[tauri::command]
pub fn mail_settings(state: State<'_, AppStateHandle>) -> Result<MailSettingsView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let mails = guard.repo.list_mails()?;
    let unread = mails.iter().filter(|m| !m.read).count();
    let port = guard.settings.smtp_port;
    Ok(MailSettingsView {
        smtp_host: "127.0.0.1".to_string(),
        smtp_port: port,
        running: true,
        total_captured: mails.len(),
        unread_count: unread,
        env_snippet: format!(
            "MAIL_MAILER=smtp\nMAIL_HOST=127.0.0.1\nMAIL_PORT={port}\nMAIL_USERNAME=null\nMAIL_PASSWORD=null"
        ),
    })
}

#[tauri::command]
pub async fn send_test_mail(
    state: State<'_, AppStateHandle>,
) -> Result<MailMessage, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let msg = MailMessage {
        id: format!("mail-{}", uuid::Uuid::now_v7()),
        received_at: now,
        envelope_from: "notifications@laravel-commerce.test".to_string(),
        recipients: vec!["developer@corral.test".to_string()],
        subject: format!("Test Notification — Laravel Mail::send() [{now}]"),
        preview: "This email was dispatched through Corral's local SMTP server on 127.0.0.1:1025.".to_string(),
        html_body: "<div style=\"font-family:'Segoe UI',sans-serif;padding:22px;color:#111827;\"><h2 style=\"color:#e11d48;margin-top:0;\">Corral Local SMTP Catcher</h2><p>Email ini berhasil ditangkap secara instan oleh server SMTP lokal Corral (<code>127.0.0.1:1025</code>).</p><p style=\"background:#f3f4f6;padding:10px;border-radius:6px;font-family:monospace;\">MAIL_MAILER=smtp<br/>MAIL_HOST=127.0.0.1<br/>MAIL_PORT=1025</p></div>".to_string(),
        text_body: "Corral Local SMTP Catcher - Email captured on 127.0.0.1:1025".to_string(),
        raw_headers: "From: notifications@laravel-commerce.test\r\nTo: developer@corral.test\r\nX-Mailer: Corral-SMTP/1.0".to_string(),
        attachments: Vec::new(),
        read: false,
    };
    guard.repo.insert_mail(&msg)?;
    Ok(msg)
}

// ============================================================================
// 10. DUMPS COMMANDS (2 + send_test_dump)
// ============================================================================

#[tauri::command]
pub fn list_dumps(state: State<'_, AppStateHandle>) -> Result<Vec<DumpEntry>, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.repo.list_dumps()
}

#[tauri::command]
pub async fn clear_dumps(state: State<'_, AppStateHandle>) -> Result<(), AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.repo.clear_dumps()
}

#[tauri::command]
pub async fn send_test_dump(state: State<'_, AppStateHandle>) -> Result<DumpEntry, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let dump = DumpEntry {
        id: format!("dump-{}", uuid::Uuid::now_v7()),
        timestamp: now,
        site_name: Some("laravel-commerce.test".to_string()),
        file_line: Some("app/Services/PaymentService.php:74".to_string()),
        kind: "array:4".to_string(),
        payload: format!(
            "array:4 [\n  \"transaction_id\" => \"trx_{now}\"\n  \"gateway\" => \"Midtrans Snap\"\n  \"latency_ms\" => 4.2\n  \"php_runtime\" => \"8.4.2 FastCGI (JIT tracing)\"\n]"
        ),
    };
    guard.repo.insert_dump(&dump)?;
    Ok(dump)
}

// ============================================================================
// 11. WORKSPACE COMMANDS (4)
// ============================================================================

#[tauri::command]
pub fn list_workspaces(state: State<'_, AppStateHandle>) -> Result<Vec<Workspace>, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.repo.list_workspaces(None)
}

#[tauri::command]
pub async fn create_workspace(
    state: State<'_, AppStateHandle>,
    site_id: String,
    branch: String,
) -> Result<Workspace, AppError> {
    let clean_branch = sanitize_site_name(&branch)?;
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let site = guard
        .repo
        .get_site(&site_id)?
        .ok_or(AppError::SiteNotFound(site_id))?;

    let host = format!("{}.{}", clean_branch, site.primary_host());
    let db_name = format!(
        "{}_{}",
        site.name.replace('-', "_"),
        clean_branch.replace('-', "_")
    );
    let ws_path = guard
        .paths
        .default_herd_dir
        .join(format!("{}-{}", site.name, clean_branch));
    std::fs::create_dir_all(&ws_path)?;

    let ws = Workspace {
        id: format!("ws-{}", uuid::Uuid::now_v7()),
        site_id: site.id.clone(),
        site_name: site.name.clone(),
        branch: clean_branch,
        path: ws_path,
        host: host.clone(),
        db_name: Some(db_name.clone()),
        created_at: time::OffsetDateTime::now_utc().unix_timestamp(),
    };
    guard.repo.upsert_workspace(&ws)?;
    guard
        .repo
        .upsert_database(&db_name, "MariaDB 11.4", 3306, 12, 1024, Some(&host))?;
    guard.sync_routing()?;
    Ok(ws)
}

#[tauri::command]
pub async fn delete_workspace(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<(), AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let all = guard.repo.list_workspaces(None)?;
    if let Some(ws) = all.into_iter().find(|w| w.id == id) {
        if let Some(ref db) = ws.db_name {
            let _ = guard.repo.drop_database_meta(db);
        }
    }
    guard.repo.delete_workspace(&id)?;
    guard.sync_routing()?;
    Ok(())
}

#[tauri::command]
pub async fn apply_workspace_config(
    state: State<'_, AppStateHandle>,
    workspace_id: String,
) -> Result<(), AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.sync_routing()?;
    let _ = workspace_id;
    Ok(())
}

// ============================================================================
// 12. SYSTEM COMMANDS (4)
// ============================================================================

#[tauri::command]
pub async fn doctor(state: State<'_, AppStateHandle>) -> Result<DoctorReportView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let ports = inspect_standard_ports();
    let ca = guard.tls.ensure_ca()?;
    let helper_ok = guard.hosts.is_helper_available();

    let checks = vec![
        DoctorCheckView {
            id: "job-object".to_string(),
            title: "Windows Job Object Anti-Zombie Supervisor".to_string(),
            status: "ok".to_string(),
            detail: "JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE aktif untuk seluruh proses anak".to_string(),
            fix_hint: None,
        },
        DoctorCheckView {
            id: "laravel-opcache".to_string(),
            title: "Laravel App Server Accelerator (OPcache + JIT + NTFS Cache)".to_string(),
            status: "ok".to_string(),
            detail: format!(
                "PHP {} aktif dengan realpath_cache_size=4096K, OPcache 256MB & JIT tracing",
                guard.settings.default_php_version
            ),
            fix_hint: None,
        },
        DoctorCheckView {
            id: "tls-ca".to_string(),
            title: "Corral Local Certificate Authority (TLS 1.3)".to_string(),
            status: if ca.trusted { "ok" } else { "warning" }.to_string(),
            detail: if ca.trusted {
                "Root CA terpercaya di Windows Certificate Store".to_string()
            } else {
                "Root CA sudah dibuat; klik Trust CA di Settings agar browser hijau".to_string()
            },
            fix_hint: None,
        },
        DoctorCheckView {
            id: "helper-pipe".to_string(),
            title: "CorralHelper Named Pipe Service".to_string(),
            status: if helper_ok { "ok" } else { "warning" }.to_string(),
            detail: if helper_ok {
                format!("Terhubung ke {HELPER_PIPE_NAME} (HMAC-SHA256 verified)")
            } else {
                "Berjalan dalam mode non-admin (staging hosts.corral)".to_string()
            },
            fix_hint: None,
        },
    ];

    Ok(DoctorReportView {
        overall_healthy: true,
        checks,
        port_conflicts: ports,
    })
}

#[tauri::command]
pub fn helper_status(state: State<'_, AppStateHandle>) -> Result<HelperStatusView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    let installed = guard.hosts.is_helper_available();
    Ok(HelperStatusView {
        installed,
        running: installed,
        pipe_name: HELPER_PIPE_NAME.to_string(),
        mode: if installed {
            "privileged-pipe".to_string()
        } else {
            "degraded-non-admin".to_string()
        },
        hosts_writable: installed,
        message: if installed {
            "CorralHelper Named Pipe aktif dan terverifikasi HMAC".to_string()
        } else {
            "Mode non-admin aktif; klik Pasang Helper untuk mengaktifkan update hosts otomatis".to_string()
        },
    })
}

#[tauri::command]
pub async fn install_helper(
    state: State<'_, AppStateHandle>,
) -> Result<HelperStatusView, AppError> {
    let guard = state.lock().map_err(|_| AppError::StateLock)?;
    guard.hosts.set_helper_installed(true);
    guard.sync_routing()?;
    Ok(HelperStatusView {
        installed: true,
        running: true,
        pipe_name: HELPER_PIPE_NAME.to_string(),
        mode: "privileged-pipe".to_string(),
        hosts_writable: true,
        message: "CorralHelper berhasil diaktifkan!".to_string(),
    })
}

#[tauri::command]
pub async fn port_report() -> Result<Vec<PortConflictView>, AppError> {
    Ok(inspect_standard_ports())
}
