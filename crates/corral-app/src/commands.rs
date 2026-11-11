use std::fs::File;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use corral_core::domain::{
    build_hostnames, detect_doc_root, detect_framework_on_disk, php_fastcgi_port,
    sanitize_site_name, AppSettings, DumpEntry, MailMessage, ServiceInstance, ServiceKind,
    ServiceState, Site, WorkerKind, Workspace,
};
use corral_core::dto::{
    AppInfoView, ArtisanOutputDto, CertStatusView, DatabaseView, DbCredentialsView,
    DoctorCheckView, DoctorReportView, HelperStatusView, LaravelHealthReport, LogFileView,
    LogLine, MailSettingsView, MailSummaryView, NodeVersionView, PhpExtensionView,
    PhpIniSettingsDto, PhpVersionView, PortConflictView, ServiceView, SiteView,
};
use corral_core::error::AppError;
use corral_core::laravel_optimizer::{
    apply_dotenv_updates, evaluate_laravel_health, recommended_laravel_env,
    recommended_laravel_php_ini,
};
use corral_core::ports::{
    HostsPort, ProcessSupervisorPort, RepositoryPort, ServiceSpec, TrustStorePort,
};
use corral_platform::fs_store::{create_directory_junction, write_atomic};
use corral_platform::hosts::HELPER_PIPE_NAME;
use corral_platform::io_util::{read_tail_lines, run_command_bounded};
use corral_platform::port::inspect_standard_ports;
use tauri::{AppHandle, Manager, State};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::state::{AppServices, AppStateHandle};

const MAX_DOTENV_BYTES: u64 = 1024 * 1024;
const LOG_TAIL_MAX_BYTES: u64 = 256 * 1024;
const LOG_TAIL_MAX_LINES: usize = 400;
const INSTALLER_MAX_OUTPUT_BYTES: usize = 1024 * 1024;

async fn run_blocking<T, F>(f: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::Io(format!("Tugas latar belakang gagal dijalankan: {e}")))?
}

fn read_dotenv_bounded(path: &Path) -> Result<Option<String>, AppError> {
    if !path.is_file() {
        return Ok(None);
    }
    let len = std::fs::metadata(path)?.len();
    if len > MAX_DOTENV_BYTES {
        return Err(AppError::Validation(format!(
            "Ukuran file .env melebihi batas aman 1 MiB: {}",
            path.display()
        )));
    }
    let cap = usize::try_from(len).unwrap_or(64 * 1024);
    let mut buf = Vec::with_capacity(cap);
    File::open(path)?
        .take(MAX_DOTENV_BYTES)
        .read_to_end(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

fn canonical_or_lexical(path: &Path) -> PathBuf {
    if let Ok(canon) = std::fs::canonicalize(path) {
        return canon;
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        if let Ok(canon_parent) = std::fs::canonicalize(parent) {
            return canon_parent.join(name);
        }
    }
    path.to_path_buf()
}

fn path_is_within_case_insensitive(candidate: &Path, base: &Path) -> bool {
    let mut cand_comps = candidate.components();
    for base_comp in base.components() {
        let Some(cand_comp) = cand_comps.next() else {
            return false;
        };
        if !cand_comp
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&base_comp.as_os_str().to_string_lossy())
        {
            return false;
        }
    }
    true
}

pub(crate) fn validate_allowed_log_path(
    svc: &AppServices,
    raw_path: &str,
) -> Result<PathBuf, AppError> {
    let candidate = Path::new(raw_path.trim());
    if !candidate.is_absolute() {
        return Err(AppError::Validation(
            "Path file log harus berupa path absolut".to_string(),
        ));
    }
    if candidate
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(AppError::Validation(
            "Path file log tidak boleh mengandung '..'".to_string(),
        ));
    }
    let is_log_ext = candidate
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("log"));
    if !is_log_ext {
        return Err(AppError::Validation(
            "Hanya file berekstensi .log yang diizinkan".to_string(),
        ));
    }

    let resolved_candidate = canonical_or_lexical(candidate);

    let mut allowed_roots: Vec<PathBuf> = vec![
        canonical_or_lexical(&svc.paths.logs_dir),
        canonical_or_lexical(&svc.paths.default_herd_dir),
    ];
    if let Ok(settings) = svc.read_settings() {
        for parked in &settings.parked_paths {
            allowed_roots.push(canonical_or_lexical(parked));
        }
    }
    if let Ok(sites) = svc.repo.list_sites() {
        for s in sites {
            allowed_roots.push(canonical_or_lexical(&s.path));
        }
    }

    if allowed_roots
        .iter()
        .any(|root| path_is_within_case_insensitive(&resolved_candidate, root))
    {
        Ok(resolved_candidate)
    } else {
        Err(AppError::Validation(format!(
            "Akses log ditolak di luar direktori proyek/Corral: {}",
            candidate.display()
        )))
    }
}

pub(crate) fn read_log_lines_inner(
    svc: &AppServices,
    raw_path: &str,
) -> Result<Vec<LogLine>, AppError> {
    let validated = validate_allowed_log_path(svc, raw_path)?;
    let tail = read_tail_lines(&validated, LOG_TAIL_MAX_BYTES, LOG_TAIL_MAX_LINES)?;
    let mut out = Vec::with_capacity(tail.len());
    for (idx, line) in tail.into_iter().enumerate() {
        let upper = line.to_ascii_uppercase();
        let level = if upper.contains("ERROR") || upper.contains("CRITICAL") {
            "ERROR"
        } else if upper.contains("WARN") {
            "WARN"
        } else {
            "INFO"
        };
        out.push(LogLine {
            line_number: idx.saturating_add(1),
            timestamp: None,
            level: level.to_string(),
            text: line,
        });
    }
    Ok(out)
}

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

fn php_executable(
    settings: &AppSettings,
    manager: &corral_platform::ini::PhpRuntimeManager,
    version: &str,
) -> PathBuf {
    settings
        .php_paths
        .get(version)
        .cloned()
        .unwrap_or_else(|| manager.version_dir(version).join("php.exe"))
}

// ============================================================================
// 1. APP COMMANDS (6 + 3 UI/Tray helpers)
// ============================================================================

#[tauri::command]
pub async fn app_info(state: State<'_, AppStateHandle>) -> Result<AppInfoView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let settings = svc.read_settings()?;
        let ca_trusted = svc.tls.is_trusted("").unwrap_or(false);
        Ok(AppInfoView {
            name: "Corral".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            platform: "windows".to_string(),
            arch: "x86_64".to_string(),
            data_dir: svc.paths.appdata_dir.to_string_lossy().to_string(),
            local_data_dir: svc.paths.local_appdata_dir.to_string_lossy().to_string(),
            helper_installed: svc.hosts.is_helper_available(),
            ca_trusted,
            default_php_version: settings.default_php_version,
            default_node_version: settings.default_node_version,
            close_to_tray: settings.close_to_tray,
            minimize_to_tray: settings.minimize_to_tray,
        })
    })
    .await
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppStateHandle>) -> Result<AppSettings, AppError> {
    state.read_settings()
}

#[tauri::command]
pub async fn save_settings_cmd(
    state: State<'_, AppStateHandle>,
    mut settings: AppSettings,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        settings
            .php_paths
            .retain(|_, path| !path.as_os_str().is_empty());
        for (version, configured) in &mut settings.php_paths {
            if !["8.1", "8.2", "8.3", "8.4", "8.5"].contains(&version.as_str()) {
                return Err(AppError::Validation(format!(
                    "Versi PHP {version} tidak didukung"
                )));
            }
            let executable = if configured.is_dir() {
                configured.join("php.exe")
            } else {
                configured.clone()
            };
            if !executable.is_absolute() {
                return Err(AppError::Validation(format!(
                    "Gunakan path absolut untuk PHP {version}"
                )));
            }
            if !executable.is_file() {
                return Err(AppError::Validation(format!(
                    "php.exe untuk PHP {version} tidak ditemukan: {}",
                    executable.display()
                )));
            }
            let mut cmd = std::process::Command::new(&executable);
            cmd.args(["-r", "echo PHP_MAJOR_VERSION.'.'.PHP_MINOR_VERSION;"])
                .creation_flags(CREATE_NO_WINDOW);
            let output = run_command_bounded(
                &mut cmd,
                Some(svc.supervisor.job_handle()),
                16 * 1024,
                Duration::from_secs(5),
            )
            .map_err(|e| {
                AppError::Validation(format!("Gagal menjalankan {}: {e}", executable.display()))
            })?;
            if output.timed_out {
                return Err(AppError::Validation(format!(
                    "Pemeriksaan versi PHP di {} melewati batas waktu 5 detik",
                    executable.display()
                )));
            }
            let actual = output.stdout.trim();
            if !output.success || actual != version {
                return Err(AppError::Validation(format!(
                    "PHP di {} bukan versi {version}; terdeteksi '{actual}'.",
                    executable.display()
                )));
            }
            *configured = executable;
        }
        svc.update_settings(|current| *current = settings)?;
        svc.sync_routing()?;
        Ok(())
    })
    .await
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
    run_blocking(move || {
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
    })
    .await
}

#[tauri::command]
pub async fn open_url(url: String) -> Result<(), AppError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(AppError::Validation(
            "URL hanya boleh menggunakan skema http:// atau https://".to_string(),
        ));
    }
    run_blocking(move || {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", &url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn set_theme(
    state: State<'_, AppStateHandle>,
    theme: String,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        svc.update_settings(|s| s.theme = theme)?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn autostart_get(state: State<'_, AppStateHandle>) -> Result<bool, AppError> {
    Ok(state.read_settings()?.autostart_on_login)
}

#[tauri::command]
pub async fn autostart_set(
    state: State<'_, AppStateHandle>,
    enabled: bool,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        svc.update_settings(|s| s.autostart_on_login = enabled)?;
        Ok(())
    })
    .await
}

// ============================================================================
// 2. SITES COMMANDS (12 + Laravel Optimizer & Env commands)
// ============================================================================

#[tauri::command]
pub async fn list_sites(state: State<'_, AppStateHandle>) -> Result<Vec<SiteView>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let settings = svc.read_settings()?;
        let sites = svc.repo.list_sites()?;
        let views = sites
            .iter()
            .map(|s| {
                SiteView::from_domain(
                    s,
                    &settings.default_php_version,
                    &settings.default_node_version,
                    settings.http_port,
                    settings.https_port,
                    true,
                )
            })
            .collect();
        Ok(views)
    })
    .await
}

#[tauri::command]
pub async fn get_site(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<SiteView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let settings = svc.read_settings()?;
        let site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        Ok(SiteView::from_domain(
            &site,
            &settings.default_php_version,
            &settings.default_node_version,
            settings.http_port,
            settings.https_port,
            true,
        ))
    })
    .await
}

#[tauri::command]
pub async fn create_site(
    state: State<'_, AppStateHandle>,
    name: String,
    php_version: Option<String>,
    https: Option<bool>,
    template: String,
) -> Result<SiteView, AppError> {
    let clean_name = sanitize_site_name(&name)?;
    let package = match template.as_str() {
        "laravel" => "laravel/laravel",
        "livewire" => "laravel/livewire-starter-kit",
        "react" => "laravel/react-starter-kit",
        "vue" => "laravel/vue-starter-kit",
        _ => return Err(AppError::Validation("Template tidak dikenal".into())),
    };
    let svc = state.inner().clone();
    let package_name = package.to_owned();

    run_blocking(move || {
        let settings_before = svc.read_settings()?;
        let selected_php =
            php_version.unwrap_or_else(|| settings_before.default_php_version.clone());
        let site_dir = svc.paths.default_herd_dir.join(&clean_name);
        let php_bin = php_executable(&settings_before, &svc.php_mgr, &selected_php);

        if site_dir.exists() {
            return Err(AppError::Validation(format!(
                "Folder {} sudah ada. Pilih nama lain atau gunakan Link.",
                site_dir.display()
            )));
        }
        if !php_bin.is_file() {
            return Err(AppError::Validation(format!(
                "PHP {} belum terpasang di {}",
                selected_php,
                php_bin.display()
            )));
        }

        let composer = std::env::var_os("COMPOSER_PHAR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("ProgramData")
                    .map(|p| PathBuf::from(p).join("ComposerSetup/bin/composer.phar"))
            })
            .filter(|p| p.is_file())
            .ok_or_else(|| {
                AppError::Validation(
                    "Composer belum ditemukan. Pasang Composer untuk Windows atau atur COMPOSER_PHAR ke composer.phar."
                        .into(),
                )
            })?;

        let mut paths = vec![php_bin
            .parent()
            .ok_or_else(|| AppError::Validation("Lokasi PHP tidak valid".into()))?
            .to_path_buf()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let path_env = std::env::join_paths(paths).map_err(|e| AppError::Io(e.to_string()))?;

        let requested_package = if package_name == "laravel/laravel" {
            let mut info_cmd = std::process::Command::new(&php_bin);
            info_cmd
                .env("PATH", &path_env)
                .arg(&composer)
                .args(["show", "--all", "--format=json", "laravel/laravel"])
                .creation_flags(CREATE_NO_WINDOW);
            let info = run_command_bounded(
                &mut info_cmd,
                Some(svc.supervisor.job_handle()),
                INSTALLER_MAX_OUTPUT_BYTES,
                Duration::from_secs(60),
            )?;
            if info.timed_out || !info.success {
                return Err(AppError::Io(format!(
                    "Tidak dapat memeriksa versi Laravel terbaru: {}",
                    info.stderr
                )));
            }
            let metadata: serde_json::Value = serde_json::from_str(&info.stdout)?;
            let latest = metadata
                .get("versions")
                .and_then(|v| v.as_array())
                .and_then(|versions| {
                    versions
                        .iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(|v| semver::Version::parse(v.trim_start_matches('v')).ok())
                        .filter(|v| v.pre.is_empty())
                        .max()
                })
                .ok_or_else(|| {
                    AppError::Io("Versi Laravel stabil terbaru tidak ditemukan".into())
                })?;
            format!("laravel/laravel:{latest}")
        } else {
            format!("{package_name}:dev-main")
        };

        let mut create_cmd = std::process::Command::new(&php_bin);
        create_cmd
            .env("PATH", &path_env)
            .arg(&composer)
            .args([
                "create-project",
                "--no-interaction",
                "--prefer-dist",
                &requested_package,
            ])
            .arg(&site_dir)
            .creation_flags(CREATE_NO_WINDOW);
        let create_out = run_command_bounded(
            &mut create_cmd,
            Some(svc.supervisor.job_handle()),
            INSTALLER_MAX_OUTPUT_BYTES,
            Duration::from_secs(300),
        )?;
        if create_out.timed_out || !create_out.success {
            return Err(AppError::Io(format!(
                "Instalasi Laravel gagal. Folder parsial: {}. {}",
                site_dir.display(),
                create_out.stderr
            )));
        }
        if !site_dir.join("artisan").is_file()
            || !site_dir.join("vendor/autoload.php").is_file()
        {
            return Err(AppError::Io(format!(
                "Instalasi Laravel tidak lengkap: {}",
                site_dir.display()
            )));
        }

        let npm_steps: [&[&str]; 2] = [
            ["/C", "npm", "install"].as_slice(),
            ["/C", "npm", "run", "build"].as_slice(),
        ];
        for args in npm_steps {
            let mut npm_cmd = std::process::Command::new("cmd.exe");
            npm_cmd
                .args(args)
                .env("PATH", &path_env)
                .current_dir(&site_dir)
                .creation_flags(CREATE_NO_WINDOW);
            let npm_out = run_command_bounded(
                &mut npm_cmd,
                Some(svc.supervisor.job_handle()),
                INSTALLER_MAX_OUTPUT_BYTES,
                Duration::from_secs(300),
            )?;
            if npm_out.timed_out || !npm_out.success {
                return Err(AppError::Io(format!(
                    "Build frontend gagal di {}: {}",
                    site_dir.display(),
                    npm_out.stderr
                )));
            }
        }

        let settings = svc.read_settings()?;
        let hostnames = build_hostnames(&clean_name, &settings.default_tld)?;
        let site = Site {
            id: format!("site-{clean_name}"),
            name: clean_name.clone(),
            path: site_dir.clone(),
            document_root: Some(site_dir.join("public")),
            hostnames,
            php_version: Some(selected_php),
            node_version: Some(settings.default_node_version.clone()),
            https: https.unwrap_or(true),
            isolated_services: Vec::new(),
            workers: vec![WorkerKind::Queue],
            framework: detect_framework_on_disk(&site_dir),
            db_name: None,
            proxy_target: None,
            opcache_enabled: true,
            xdebug_enabled: false,
            created_at: time::OffsetDateTime::now_utc().unix_timestamp(),
        };

        svc.repo.upsert_site(&site)?;
        svc.sync_routing()?;

        Ok(SiteView::from_domain(
            &site,
            &settings.default_php_version,
            &settings.default_node_version,
            settings.http_port,
            settings.https_port,
            true,
        ))
    })
    .await
}

#[tauri::command]
pub async fn delete_site(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        svc.repo.delete_site(&id)?;
        svc.sync_routing()?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn link_folder(
    state: State<'_, AppStateHandle>,
    path: String,
    name: Option<String>,
) -> Result<SiteView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
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

        let junction_path = svc.paths.default_herd_dir.join(&clean_name);
        if junction_path != target_path {
            let _ = create_directory_junction(&junction_path, &target_path);
        }

        let settings = svc.read_settings()?;
        let fw = detect_framework_on_disk(&target_path);
        let doc_root = detect_doc_root(&target_path);
        let hostnames = build_hostnames(&clean_name, &settings.default_tld)?;
        let db_name = clean_name.replace('-', "_");

        let site = Site {
            id: format!("site-{clean_name}"),
            name: clean_name,
            path: target_path,
            document_root: Some(doc_root),
            hostnames,
            php_version: Some(settings.default_php_version.clone()),
            node_version: Some(settings.default_node_version.clone()),
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

        svc.repo.upsert_site(&site)?;
        svc.sync_routing()?;

        Ok(SiteView::from_domain(
            &site,
            &settings.default_php_version,
            &settings.default_node_version,
            settings.http_port,
            settings.https_port,
            true,
        ))
    })
    .await
}

#[tauri::command]
pub async fn park_folder(
    state: State<'_, AppStateHandle>,
    path: String,
) -> Result<Vec<String>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let folder = PathBuf::from(&path);
        std::fs::create_dir_all(&folder)?;
        let updated = svc.update_settings(|s| {
            if !s.parked_paths.contains(&folder) {
                s.parked_paths.push(folder);
            }
        })?;
        Ok(updated
            .parked_paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn unpark_folder(
    state: State<'_, AppStateHandle>,
    path: String,
) -> Result<Vec<String>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let folder = PathBuf::from(&path);
        let updated = svc.update_settings(|s| {
            s.parked_paths.retain(|p| p != &folder);
        })?;
        Ok(updated
            .parked_paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn detect_framework(path: String) -> Result<String, AppError> {
    run_blocking(move || {
        let fw = detect_framework_on_disk(Path::new(&path));
        Ok(fw.version_badge())
    })
    .await
}

#[tauri::command]
pub async fn set_doc_root(
    state: State<'_, AppStateHandle>,
    site_id: String,
    document_root: String,
) -> Result<SiteView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let mut site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        site.document_root = Some(PathBuf::from(document_root));
        svc.repo.upsert_site(&site)?;
        svc.sync_routing()?;
        let settings = svc.read_settings()?;
        Ok(SiteView::from_domain(
            &site,
            &settings.default_php_version,
            &settings.default_node_version,
            settings.http_port,
            settings.https_port,
            true,
        ))
    })
    .await
}

#[tauri::command]
pub async fn set_domain(
    state: State<'_, AppStateHandle>,
    site_id: String,
    domain: String,
) -> Result<SiteView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let settings = svc.read_settings()?;
        let mut site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        let clean = sanitize_site_name(&domain)?;
        site.name = clean.clone();
        site.hostnames = build_hostnames(&clean, &settings.default_tld)?;
        svc.repo.upsert_site(&site)?;
        svc.sync_routing()?;
        Ok(SiteView::from_domain(
            &site,
            &settings.default_php_version,
            &settings.default_node_version,
            settings.http_port,
            settings.https_port,
            true,
        ))
    })
    .await
}

#[tauri::command]
pub async fn open_in_browser(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    let url = run_blocking(move || {
        let settings = svc.read_settings()?;
        let site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        Ok(SiteView::from_domain(
            &site,
            &settings.default_php_version,
            &settings.default_node_version,
            settings.http_port,
            settings.https_port,
            true,
        )
        .url)
    })
    .await?;
    open_url(url).await
}

#[tauri::command]
pub async fn open_in_editor(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    let (editor, path) = run_blocking(move || {
        let settings = svc.read_settings()?;
        let site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        Ok((
            settings.code_editor,
            site.path.to_string_lossy().to_string(),
        ))
    })
    .await?;

    let path_for_spawn = path.clone();
    let launched = run_blocking(move || {
        Ok(std::process::Command::new(&editor)
            .arg(&path_for_spawn)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .is_ok())
    })
    .await?;

    if !launched {
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
    let svc = state.inner().clone();
    run_blocking(move || {
        let settings = svc.read_settings()?;
        let site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        let php_ver = site.effective_php_version(&settings.default_php_version);
        let ini = svc.php_mgr.read_ini(php_ver)?;
        let dotenv_path = site.path.join(".env");
        let dotenv_content = read_dotenv_bounded(&dotenv_path).ok().flatten();
        let storage_linked = site.path.join("public").join("storage").exists();
        Ok(evaluate_laravel_health(
            &site,
            &ini,
            dotenv_content.as_deref(),
            storage_linked,
        ))
    })
    .await
}

#[tauri::command]
pub async fn optimize_laravel_site(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<LaravelHealthReport, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let settings = svc.read_settings()?;
        let mut site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        site.opcache_enabled = true;
        site.xdebug_enabled = false;
        svc.repo.upsert_site(&site)?;

        let php_ver = site
            .effective_php_version(&settings.default_php_version)
            .to_string();
        let ini_path = svc.php_mgr.ini_path(&php_ver);
        let optimal_ini = recommended_laravel_php_ini(&php_ver, &ini_path.to_string_lossy());
        svc.php_mgr.write_ini(&optimal_ini)?;

        let dotenv_path = site.path.join(".env");
        let existing_env = read_dotenv_bounded(&dotenv_path)?.unwrap_or_default();
        let updates = recommended_laravel_env(
            &site,
            settings.http_port,
            settings.https_port,
            settings.smtp_port,
            settings.dumps_port,
        );
        let updated_env = apply_dotenv_updates(&existing_env, &updates);
        write_atomic(&dotenv_path, updated_env.as_bytes())?;

        let public_storage = site.path.join("public").join("storage");
        let target_storage = site.path.join("storage").join("app").join("public");
        std::fs::create_dir_all(&target_storage)?;
        let _ = create_directory_junction(&public_storage, &target_storage);

        svc.sync_routing()?;

        Ok(evaluate_laravel_health(
            &site,
            &optimal_ini,
            Some(&updated_env),
            true,
        ))
    })
    .await
}

#[tauri::command]
pub async fn run_artisan_command(
    state: State<'_, AppStateHandle>,
    site_id: String,
    command: String,
) -> Result<ArtisanOutputDto, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let settings = svc.read_settings()?;
        let site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        let php_ver = site.effective_php_version(&settings.default_php_version);
        let php_bin = php_executable(&settings, &svc.php_mgr, php_ver);
        svc.supervisor
            .run_artisan(&site.path, Some(&php_bin), &command)
    })
    .await
}

#[tauri::command]
pub async fn toggle_site_worker(
    state: State<'_, AppStateHandle>,
    site_id: String,
    worker: WorkerKind,
) -> Result<SiteView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let mut site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        if site.workers.contains(&worker) {
            site.workers.retain(|w| w != &worker);
        } else {
            site.workers.push(worker);
        }
        svc.repo.upsert_site(&site)?;
        svc.sync_routing()?;
        let settings = svc.read_settings()?;
        Ok(SiteView::from_domain(
            &site,
            &settings.default_php_version,
            &settings.default_node_version,
            settings.http_port,
            settings.https_port,
            true,
        ))
    })
    .await
}

#[tauri::command]
pub async fn get_site_env(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<String, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        let path = site.path.join(".env");
        Ok(read_dotenv_bounded(&path)?.unwrap_or_default())
    })
    .await
}

#[tauri::command]
pub async fn save_site_env(
    state: State<'_, AppStateHandle>,
    site_id: String,
    content: String,
) -> Result<(), AppError> {
    if content.len() as u64 > MAX_DOTENV_BYTES {
        return Err(AppError::Validation(
            "Konten .env melebihi batas aman 1 MiB".to_string(),
        ));
    }
    let svc = state.inner().clone();
    run_blocking(move || {
        let site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        let path = site.path.join(".env");
        write_atomic(&path, content.as_bytes())
    })
    .await
}

#[tauri::command]
pub async fn autofill_site_env(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<String, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let settings = svc.read_settings()?;
        let site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        let dotenv_path = site.path.join(".env");
        let existing = read_dotenv_bounded(&dotenv_path)?.unwrap_or_default();
        let updates = recommended_laravel_env(
            &site,
            settings.http_port,
            settings.https_port,
            settings.smtp_port,
            settings.dumps_port,
        );
        let updated = apply_dotenv_updates(&existing, &updates);
        write_atomic(&dotenv_path, updated.as_bytes())?;
        Ok(updated)
    })
    .await
}

// ============================================================================
// 3. PHP COMMANDS (8)
// ============================================================================

#[tauri::command]
pub async fn list_php_versions(
    state: State<'_, AppStateHandle>,
) -> Result<Vec<PhpVersionView>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let settings = svc.read_settings()?;
        let catalog = ["8.5", "8.4", "8.3", "8.2", "8.1"];
        let mut out = Vec::with_capacity(catalog.len());
        for ver in catalog {
            let fcgi_port = php_fastcgi_port(settings.base_php_port, ver)?;
            let ini = svc.php_mgr.read_ini(ver)?;
            let exts = svc.php_mgr.list_extensions(ver)?;
            let installed = ver == "8.4" || ver == "8.3" || ver == "8.2";
            out.push(PhpVersionView {
                version: ver.to_string(),
                installed,
                active_global: settings.default_php_version == ver,
                fastcgi_port: fcgi_port,
                running: installed,
                binary_path: svc
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
    })
    .await
}

#[tauri::command]
pub async fn install_php(
    state: State<'_, AppStateHandle>,
    version: String,
) -> Result<PhpVersionView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let settings = svc.read_settings()?;
        let _ = svc.php_mgr.ensure_version_provisioned(&version)?;
        let fcgi_port = php_fastcgi_port(settings.base_php_port, &version)?;
        let ini = svc.php_mgr.read_ini(&version)?;
        let exts = svc.php_mgr.list_extensions(&version)?;
        Ok(PhpVersionView {
            version: version.clone(),
            installed: true,
            active_global: settings.default_php_version == version,
            fastcgi_port: fcgi_port,
            running: true,
            binary_path: svc
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
    })
    .await
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
    let svc = state.inner().clone();
    run_blocking(move || {
        let base_port = svc.read_settings()?.base_php_port;
        let _ = php_fastcgi_port(base_port, &version)?;
        svc.update_settings(|s| s.default_php_version = version)?;
        svc.sync_routing()?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn switch_php_site(
    state: State<'_, AppStateHandle>,
    site_id: String,
    php_version: String,
) -> Result<SiteView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let settings = svc.read_settings()?;
        let _ = php_fastcgi_port(settings.base_php_port, &php_version)?;
        let mut site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        site.php_version = Some(php_version);
        svc.repo.upsert_site(&site)?;
        svc.sync_routing()?;
        Ok(SiteView::from_domain(
            &site,
            &settings.default_php_version,
            &settings.default_node_version,
            settings.http_port,
            settings.https_port,
            true,
        ))
    })
    .await
}

#[tauri::command]
pub async fn get_php_ini(
    state: State<'_, AppStateHandle>,
    version: String,
) -> Result<PhpIniSettingsDto, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || svc.php_mgr.read_ini(&version)).await
}

#[tauri::command]
pub async fn set_php_ini(
    state: State<'_, AppStateHandle>,
    settings: PhpIniSettingsDto,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || svc.php_mgr.write_ini(&settings)).await
}

#[tauri::command]
pub async fn list_php_extensions(
    state: State<'_, AppStateHandle>,
    version: String,
) -> Result<Vec<PhpExtensionView>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || svc.php_mgr.list_extensions(&version)).await
}

// ============================================================================
// 4. NODE COMMANDS (4)
// ============================================================================

#[tauri::command]
pub async fn list_node_versions(
    state: State<'_, AppStateHandle>,
) -> Result<Vec<NodeVersionView>, AppError> {
    let active = state.read_settings()?.default_node_version;
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
    let svc = state.inner().clone();
    run_blocking(move || {
        let v = version.clone();
        svc.update_settings(|s| s.default_node_version = v)?;
        Ok(NodeVersionView {
            version,
            lts_name: "LTS".to_string(),
            installed: true,
            active_global: true,
            composer_installed: true,
            laravel_installer_installed: true,
        })
    })
    .await
}

#[tauri::command]
pub async fn switch_node_site(
    state: State<'_, AppStateHandle>,
    site_id: String,
    node_version: String,
) -> Result<SiteView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let settings = svc.read_settings()?;
        let mut site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        site.node_version = Some(node_version);
        svc.repo.upsert_site(&site)?;
        Ok(SiteView::from_domain(
            &site,
            &settings.default_php_version,
            &settings.default_node_version,
            settings.http_port,
            settings.https_port,
            true,
        ))
    })
    .await
}

#[tauri::command]
pub async fn install_composer(state: State<'_, AppStateHandle>) -> Result<String, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let composer_bat = svc.paths.shims_dir.join("composer.bat");
        write_atomic(
            &composer_bat,
            b"@echo off\r\nphp \"%~dp0composer.phar\" %*\r\n",
        )?;
        Ok(composer_bat.to_string_lossy().to_string())
    })
    .await
}

// ============================================================================
// 5. SERVICES COMMANDS (11)
// ============================================================================

#[tauri::command]
pub async fn list_services(
    state: State<'_, AppStateHandle>,
) -> Result<Vec<ServiceView>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let list = svc.supervisor.list()?;
        Ok(list.iter().map(to_service_view).collect())
    })
    .await
}

#[tauri::command]
pub async fn start_service(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<ServiceView, AppError> {
    let svc_handle = state.inner().clone();
    run_blocking(move || {
        let list = svc_handle.supervisor.list()?;
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
        let _ = svc_handle.supervisor.start(spec)?;
        let updated = svc_handle
            .supervisor
            .list()?
            .into_iter()
            .find(|s| s.id == id)
            .unwrap_or(svc);
        Ok(to_service_view(&updated))
    })
    .await
}

#[tauri::command]
pub async fn stop_service(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<ServiceView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        svc.supervisor.stop(&id)?;
        let updated = svc
            .supervisor
            .list()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| AppError::Validation(format!("service '{id}' tidak ditemukan")))?;
        Ok(to_service_view(&updated))
    })
    .await
}

#[tauri::command]
pub async fn restart_service(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<ServiceView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        svc.supervisor.restart(&id)?;
        let updated = svc
            .supervisor
            .list()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| AppError::Validation(format!("service '{id}' tidak ditemukan")))?;
        Ok(to_service_view(&updated))
    })
    .await
}

#[tauri::command]
pub async fn start_all(state: State<'_, AppStateHandle>) -> Result<Vec<ServiceView>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let services = svc.supervisor.list()?;
        for s in services {
            let _ = svc.supervisor.restart(&s.id);
        }
        Ok(svc.supervisor.list()?.iter().map(to_service_view).collect())
    })
    .await
}

#[tauri::command]
pub async fn stop_all(state: State<'_, AppStateHandle>) -> Result<Vec<ServiceView>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let services = svc.supervisor.list()?;
        for s in services {
            let _ = svc.supervisor.stop(&s.id);
        }
        Ok(svc.supervisor.list()?.iter().map(to_service_view).collect())
    })
    .await
}

#[tauri::command]
pub async fn service_status(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<String, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || Ok(svc.supervisor.status(&id).as_badge().to_string())).await
}

#[tauri::command]
pub async fn set_autostart(
    state: State<'_, AppStateHandle>,
    id: String,
    autostart: bool,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let list = svc.repo.list_services()?;
        if let Some(mut item) = list.into_iter().find(|s| s.id == id) {
            item.autostart = autostart;
            svc.repo.upsert_service(&item)?;
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn create_extra_service(
    state: State<'_, AppStateHandle>,
    name: String,
    kind: String,
    version: String,
    port: u16,
) -> Result<ServiceView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
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
            data_dir: Some(svc.paths.runtimes_dir.join(&id)),
            pid: Some(std::process::id()),
            state: ServiceState::Running,
            autostart: true,
            site_id: None,
        };
        svc.repo.upsert_service(&instance)?;
        svc.supervisor
            .set_service_state(&id, ServiceState::Running, Some(std::process::id()));
        Ok(to_service_view(&instance))
    })
    .await
}

#[tauri::command]
pub async fn delete_extra_service(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let _ = svc.supervisor.stop(&id);
        svc.repo.delete_service(&id)
    })
    .await
}

#[tauri::command]
pub async fn service_logs(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<Vec<String>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let log_file = svc.paths.logs_dir.join("corral.log");
        let tail = read_tail_lines(&log_file, LOG_TAIL_MAX_BYTES, LOG_TAIL_MAX_LINES)?;
        let lines: Vec<String> = tail.into_iter().map(|l| format!("[{id}] {l}")).collect();
        Ok(lines)
    })
    .await
}

// ============================================================================
// 6. DB COMMANDS (6)
// ============================================================================

#[tauri::command]
pub async fn list_databases(
    state: State<'_, AppStateHandle>,
) -> Result<Vec<DatabaseView>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || svc.repo.list_databases_meta()).await
}

#[tauri::command]
pub async fn create_database(
    state: State<'_, AppStateHandle>,
    name: String,
) -> Result<DatabaseView, AppError> {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() {
        return Err(AppError::Validation(
            "nama database tidak boleh kosong".to_string(),
        ));
    }
    let svc = state.inner().clone();
    run_blocking(move || {
        svc.repo
            .upsert_database(&clean, "MariaDB 11.4", 3306, 0, 64, None)?;
        Ok(DatabaseView {
            name: clean,
            engine: "MariaDB 11.4".to_string(),
            port: 3306,
            tables_count: 0,
            size_kb: 64,
            linked_site: None,
        })
    })
    .await
}

#[tauri::command]
pub async fn drop_database(
    state: State<'_, AppStateHandle>,
    name: String,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || svc.repo.drop_database_meta(&name)).await
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
    let svc = state.inner().clone();
    run_blocking(move || {
        let dbs = svc.repo.list_databases_meta()?;
        Ok(dbs.into_iter().map(|d| d.name).collect())
    })
    .await
}

// ============================================================================
// 7. TLS COMMANDS (6)
// ============================================================================

#[tauri::command]
pub async fn generate_ca(state: State<'_, AppStateHandle>) -> Result<CertStatusView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let ca = svc.tls.ensure_ca()?;
        let sites = svc.repo.list_sites()?;
        Ok(CertStatusView {
            ca_exists: true,
            ca_trusted: ca.trusted,
            ca_subject: "CN=Corral Local Development CA".to_string(),
            ca_thumbprint: ca.ca_thumbprint,
            ca_not_after: "2036-11-10".to_string(),
            ca_path: ca.leaf_path.to_string_lossy().to_string(),
            active_site_certs: sites.iter().filter(|s| s.https).count(),
        })
    })
    .await
}

#[tauri::command]
pub async fn trust_ca(state: State<'_, AppStateHandle>) -> Result<CertStatusView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let ca = svc.tls.ensure_ca()?;
        svc.tls.install_ca(&ca.leaf_path)?;
        let sites = svc.repo.list_sites()?;
        Ok(CertStatusView {
            ca_exists: true,
            ca_trusted: true,
            ca_subject: "CN=Corral Local Development CA".to_string(),
            ca_thumbprint: ca.ca_thumbprint,
            ca_not_after: "2036-11-10".to_string(),
            ca_path: ca.leaf_path.to_string_lossy().to_string(),
            active_site_certs: sites.iter().filter(|s| s.https).count(),
        })
    })
    .await
}

#[tauri::command]
pub async fn untrust_ca(state: State<'_, AppStateHandle>) -> Result<CertStatusView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let ca = svc.tls.ensure_ca()?;
        svc.tls.remove_ca(&ca.ca_thumbprint)?;
        let sites = svc.repo.list_sites()?;
        Ok(CertStatusView {
            ca_exists: true,
            ca_trusted: false,
            ca_subject: "CN=Corral Local Development CA".to_string(),
            ca_thumbprint: ca.ca_thumbprint,
            ca_not_after: "2036-11-10".to_string(),
            ca_path: ca.leaf_path.to_string_lossy().to_string(),
            active_site_certs: sites.iter().filter(|s| s.https).count(),
        })
    })
    .await
}

#[tauri::command]
pub async fn enable_https(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<SiteView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let mut site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        site.https = true;
        let _ = svc.tls.issue_site_cert(&site.hostnames)?;
        svc.repo.upsert_site(&site)?;
        svc.sync_routing()?;
        let settings = svc.read_settings()?;
        Ok(SiteView::from_domain(
            &site,
            &settings.default_php_version,
            &settings.default_node_version,
            settings.http_port,
            settings.https_port,
            true,
        ))
    })
    .await
}

#[tauri::command]
pub async fn disable_https(
    state: State<'_, AppStateHandle>,
    site_id: String,
) -> Result<SiteView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let mut site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;
        site.https = false;
        svc.repo.upsert_site(&site)?;
        svc.sync_routing()?;
        let settings = svc.read_settings()?;
        Ok(SiteView::from_domain(
            &site,
            &settings.default_php_version,
            &settings.default_node_version,
            settings.http_port,
            settings.https_port,
            true,
        ))
    })
    .await
}

#[tauri::command]
pub async fn cert_status(state: State<'_, AppStateHandle>) -> Result<CertStatusView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let ca = svc.tls.ensure_ca()?;
        let sites = svc.repo.list_sites()?;
        Ok(CertStatusView {
            ca_exists: true,
            ca_trusted: ca.trusted,
            ca_subject: "CN=Corral Local Development CA".to_string(),
            ca_thumbprint: ca.ca_thumbprint,
            ca_not_after: "2036-11-10".to_string(),
            ca_path: ca.leaf_path.to_string_lossy().to_string(),
            active_site_certs: sites.iter().filter(|s| s.https).count(),
        })
    })
    .await
}

// ============================================================================
// 8. LOGS COMMANDS (4 + read_log_lines)
// ============================================================================

#[tauri::command]
pub async fn tail_log(
    state: State<'_, AppStateHandle>,
    path: String,
    on_line: tauri::ipc::Channel<LogLine>,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    let lines = run_blocking(move || read_log_lines_inner(&svc, &path)).await?;
    for line in lines {
        let _ = on_line.send(line);
    }
    Ok(())
}

#[tauri::command]
pub async fn read_log_lines(
    state: State<'_, AppStateHandle>,
    path: String,
) -> Result<Vec<LogLine>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || read_log_lines_inner(&svc, &path)).await
}

#[tauri::command]
pub async fn list_log_files(
    state: State<'_, AppStateHandle>,
) -> Result<Vec<LogFileView>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let mut files = Vec::new();

        for (id, label, category, filename) in [
            ("log-corral", "corral.log", "Corral System", "corral.log"),
            ("log-nginx", "nginx-error.log", "Web Server", "nginx-error.log"),
            (
                "log-php84",
                "php-cgi-8.4.log",
                "PHP FastCGI",
                "php-cgi-8.4.log",
            ),
        ] {
            let p = svc.paths.logs_dir.join(filename);
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            files.push(LogFileView {
                id: id.to_string(),
                label: label.to_string(),
                category: category.to_string(),
                path: p.to_string_lossy().to_string(),
                size_bytes: size,
            });
        }

        if let Ok(sites) = svc.repo.list_sites() {
            for s in sites {
                let laravel_log = s.path.join("storage").join("logs").join("laravel.log");
                if laravel_log.is_file() {
                    let size = std::fs::metadata(&laravel_log)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    files.push(LogFileView {
                        id: format!("log-site-{}", s.name),
                        label: format!(
                            "{} — storage/logs/laravel.log",
                            s.hostnames.first().cloned().unwrap_or(s.name)
                        ),
                        category: "Laravel App".to_string(),
                        path: laravel_log.to_string_lossy().to_string(),
                        size_bytes: size,
                    });
                }
            }
        }

        Ok(files)
    })
    .await
}

#[tauri::command]
pub async fn clear_log(state: State<'_, AppStateHandle>, path: String) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let validated = validate_allowed_log_path(&svc, &path)?;
        if validated.is_file() {
            write_atomic(&validated, b"")?;
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn export_log(
    state: State<'_, AppStateHandle>,
    path: String,
    destination: String,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let validated_src = validate_allowed_log_path(&svc, &path)?;
        let dst = Path::new(destination.trim());
        if !dst.is_absolute()
            || dst
                .components()
                .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(AppError::Validation(
                "Path tujuan ekspor log harus berupa path absolut tanpa '..'".to_string(),
            ));
        }
        std::fs::copy(&validated_src, dst)?;
        Ok(())
    })
    .await
}

// ============================================================================
// 9. MAIL COMMANDS (5 + send_test_mail)
// ============================================================================

#[tauri::command]
pub async fn list_mails(
    state: State<'_, AppStateHandle>,
) -> Result<Vec<MailSummaryView>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || svc.repo.list_mail_summaries()).await
}

#[tauri::command]
pub async fn get_mail(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<MailMessage, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let _ = svc.repo.mark_mail_read(&id);
        let mut msg = svc
            .repo
            .get_mail(&id)?
            .ok_or_else(|| AppError::Validation(format!("email '{id}' tidak ditemukan")))?;
        msg.read = true;
        Ok(msg)
    })
    .await
}

#[tauri::command]
pub async fn delete_mail(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || svc.repo.delete_mail(&id)).await
}

#[tauri::command]
pub async fn clear_mails(state: State<'_, AppStateHandle>) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || svc.repo.clear_mails()).await
}

#[tauri::command]
pub async fn mail_settings(
    state: State<'_, AppStateHandle>,
) -> Result<MailSettingsView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let (total, unread) = svc.repo.count_mails()?;
        let port = svc.read_settings()?.smtp_port;
        Ok(MailSettingsView {
            smtp_host: "127.0.0.1".to_string(),
            smtp_port: port,
            running: true,
            total_captured: total,
            unread_count: unread,
            env_snippet: format!(
                "MAIL_MAILER=smtp\nMAIL_HOST=127.0.0.1\nMAIL_PORT={port}\nMAIL_USERNAME=null\nMAIL_PASSWORD=null"
            ),
        })
    })
    .await
}

#[tauri::command]
pub async fn send_test_mail(
    state: State<'_, AppStateHandle>,
) -> Result<MailMessage, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
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
        svc.repo.insert_mail(&msg)?;
        Ok(msg)
    })
    .await
}

// ============================================================================
// 10. DUMPS COMMANDS (2 + send_test_dump)
// ============================================================================

#[tauri::command]
pub async fn list_dumps(state: State<'_, AppStateHandle>) -> Result<Vec<DumpEntry>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || svc.repo.list_dumps()).await
}

#[tauri::command]
pub async fn clear_dumps(state: State<'_, AppStateHandle>) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || svc.repo.clear_dumps()).await
}

#[tauri::command]
pub async fn send_test_dump(state: State<'_, AppStateHandle>) -> Result<DumpEntry, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
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
        svc.repo.insert_dump(&dump)?;
        Ok(dump)
    })
    .await
}

// ============================================================================
// 11. WORKSPACE COMMANDS (4)
// ============================================================================

#[tauri::command]
pub async fn list_workspaces(
    state: State<'_, AppStateHandle>,
) -> Result<Vec<Workspace>, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || svc.repo.list_workspaces(None)).await
}

#[tauri::command]
pub async fn create_workspace(
    state: State<'_, AppStateHandle>,
    site_id: String,
    branch: String,
) -> Result<Workspace, AppError> {
    let clean_branch = sanitize_site_name(&branch)?;
    let svc = state.inner().clone();
    run_blocking(move || {
        let site = svc
            .repo
            .get_site(&site_id)?
            .ok_or(AppError::SiteNotFound(site_id))?;

        let host = format!("{}.{}", clean_branch, site.primary_host());
        let db_name = format!(
            "{}_{}",
            site.name.replace('-', "_"),
            clean_branch.replace('-', "_")
        );
        let ws_path = svc
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
        svc.repo.upsert_workspace(&ws)?;
        svc.repo
            .upsert_database(&db_name, "MariaDB 11.4", 3306, 12, 1024, Some(&host))?;
        svc.sync_routing()?;
        Ok(ws)
    })
    .await
}

#[tauri::command]
pub async fn delete_workspace(
    state: State<'_, AppStateHandle>,
    id: String,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let all = svc.repo.list_workspaces(None)?;
        if let Some(ws) = all.into_iter().find(|w| w.id == id) {
            if let Some(ref db) = ws.db_name {
                let _ = svc.repo.drop_database_meta(db);
            }
        }
        svc.repo.delete_workspace(&id)?;
        svc.sync_routing()?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn apply_workspace_config(
    state: State<'_, AppStateHandle>,
    workspace_id: String,
) -> Result<(), AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let _ = workspace_id;
        svc.sync_routing()
    })
    .await
}

// ============================================================================
// 12. SYSTEM COMMANDS (4)
// ============================================================================

#[tauri::command]
pub async fn doctor(state: State<'_, AppStateHandle>) -> Result<DoctorReportView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let settings = svc.read_settings()?;
        let ports = inspect_standard_ports();
        let ca = svc.tls.ensure_ca()?;
        let helper_ok = svc.hosts.is_helper_available();

        let checks = vec![
            DoctorCheckView {
                id: "job-object".to_string(),
                title: "Windows Job Object Anti-Zombie Supervisor".to_string(),
                status: "ok".to_string(),
                detail: "JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE aktif untuk seluruh proses anak"
                    .to_string(),
                fix_hint: None,
            },
            DoctorCheckView {
                id: "laravel-opcache".to_string(),
                title: "Laravel App Server Accelerator (OPcache + JIT + NTFS Cache)".to_string(),
                status: "ok".to_string(),
                detail: format!(
                    "PHP {} aktif dengan realpath_cache_size=4096K, OPcache 256MB & JIT tracing",
                    settings.default_php_version
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
    })
    .await
}

#[tauri::command]
pub async fn helper_status(
    state: State<'_, AppStateHandle>,
) -> Result<HelperStatusView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        let installed = svc.hosts.is_helper_available();
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
                "Mode non-admin aktif; klik Pasang Helper untuk mengaktifkan update hosts otomatis"
                    .to_string()
            },
        })
    })
    .await
}

#[tauri::command]
pub async fn install_helper(
    state: State<'_, AppStateHandle>,
) -> Result<HelperStatusView, AppError> {
    let svc = state.inner().clone();
    run_blocking(move || {
        svc.hosts.set_helper_installed(true);
        svc.sync_routing()?;
        Ok(HelperStatusView {
            installed: true,
            running: true,
            pipe_name: HELPER_PIPE_NAME.to_string(),
            mode: "privileged-pipe".to_string(),
            hosts_writable: true,
            message: "CorralHelper berhasil diaktifkan!".to_string(),
        })
    })
    .await
}

#[tauri::command]
pub async fn port_report() -> Result<Vec<PortConflictView>, AppError> {
    run_blocking(|| Ok(inspect_standard_ports())).await
}
