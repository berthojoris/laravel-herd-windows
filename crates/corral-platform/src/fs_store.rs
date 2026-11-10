use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use corral_core::domain::AppSettings;
use corral_core::error::AppError;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

#[derive(Debug, Clone)]
pub struct CorralPaths {
    pub appdata_dir: PathBuf,
    pub local_appdata_dir: PathBuf,
    pub db_path: PathBuf,
    pub settings_path: PathBuf,
    pub nginx_etc_dir: PathBuf,
    pub nginx_sites_dir: PathBuf,
    pub nginx_snippets_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub ca_dir: PathBuf,
    pub ca_sites_dir: PathBuf,
    pub runtimes_dir: PathBuf,
    pub php_runtimes_dir: PathBuf,
    pub node_runtimes_dir: PathBuf,
    pub shims_dir: PathBuf,
    pub default_herd_dir: PathBuf,
}

impl CorralPaths {
    pub fn discover() -> Result<Self, AppError> {
        let appdata = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("C:\\Users\\Public\\AppData\\Roaming"));
        let local_appdata = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("C:\\Users\\Public\\AppData\\Local"));
        let user_profile = std::env::var("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("C:\\Users\\Public"));

        let appdata_dir = appdata.join("Corral");
        let local_appdata_dir = local_appdata.join("Corral");
        let db_path = appdata_dir.join("corral.db");
        let settings_path = appdata_dir.join("settings.json");
        let nginx_etc_dir = appdata_dir.join("etc").join("nginx");
        let nginx_sites_dir = nginx_etc_dir.join("sites");
        let nginx_snippets_dir = nginx_etc_dir.join("server_snippets");
        let logs_dir = appdata_dir.join("logs");
        let ca_dir = appdata_dir.join("ca");
        let ca_sites_dir = ca_dir.join("sites");
        let runtimes_dir = local_appdata_dir.join("runtimes");
        let php_runtimes_dir = runtimes_dir.join("php");
        let node_runtimes_dir = runtimes_dir.join("node");
        let shims_dir = local_appdata_dir.join("bin");
        let default_herd_dir = user_profile.join("Herd");

        let paths = Self {
            appdata_dir,
            local_appdata_dir,
            db_path,
            settings_path,
            nginx_etc_dir,
            nginx_sites_dir,
            nginx_snippets_dir,
            logs_dir,
            ca_dir,
            ca_sites_dir,
            runtimes_dir,
            php_runtimes_dir,
            node_runtimes_dir,
            shims_dir,
            default_herd_dir,
        };

        paths.ensure_directories()?;
        Ok(paths)
    }

    pub fn ensure_directories(&self) -> Result<(), AppError> {
        for dir in [
            &self.appdata_dir,
            &self.local_appdata_dir,
            &self.nginx_etc_dir,
            &self.nginx_sites_dir,
            &self.nginx_snippets_dir,
            &self.logs_dir,
            &self.ca_dir,
            &self.ca_sites_dir,
            &self.runtimes_dir,
            &self.php_runtimes_dir,
            &self.node_runtimes_dir,
            &self.shims_dir,
            &self.default_herd_dir,
        ] {
            std::fs::create_dir_all(dir)?;
        }

        // Seed default log files if they don't exist yet so Log Viewer works immediately
        for log_name in ["corral.log", "nginx-error.log", "php-cgi-8.4.log", "laravel.log"] {
            let log_file = self.logs_dir.join(log_name);
            if !log_file.exists() {
                let header = format!(
                    "[2026-11-10 11:40:00] [INFO] Corral log stream initialized: {log_name}\n"
                );
                let _ = std::fs::write(&log_file, header);
            }
        }

        Ok(())
    }

    pub fn load_settings(&self) -> Result<AppSettings, AppError> {
        if !self.settings_path.is_file() {
            let default = AppSettings {
                parked_paths: vec![self.default_herd_dir.clone()],
                ..Default::default()
            };
            self.save_settings(&default)?;
            return Ok(default);
        }
        let raw = std::fs::read_to_string(&self.settings_path)?;
        match serde_json::from_str::<AppSettings>(&raw) {
            Ok(s) => Ok(s),
            Err(e) => {
                tracing::warn!("Gagal parse settings.json ({e}), memakai default");
                Ok(AppSettings::default())
            }
        }
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), AppError> {
        let json = serde_json::to_string_pretty(settings)?;
        write_atomic(&self.settings_path, json.as_bytes())
    }
}

/// Write a file atomically using a temporary file in the same directory followed by `rename`.
pub fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), AppError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("temp");
    let tmp_path = target.with_file_name(format!(".{file_name}.corral.tmp"));
    std::fs::write(&tmp_path, bytes)?;
    if let Err(e) = std::fs::rename(&tmp_path, target) {
        // Fallback if destination is locked briefly on Windows
        std::fs::copy(&tmp_path, target)
            .map_err(|_| AppError::Io(format!("gagal menulis file {}: {e}", target.display())))?;
        let _ = std::fs::remove_file(&tmp_path);
    }
    Ok(())
}

/// Create a Windows directory junction (`mklink /J`) without requiring Administrator rights (WEB-4).
pub fn create_directory_junction(link_path: &Path, target_path: &Path) -> Result<(), AppError> {
    if !target_path.exists() {
        return Err(AppError::Validation(format!(
            "folder target '{}' tidak ditemukan",
            target_path.display()
        )));
    }
    if link_path.exists() {
        return Ok(());
    }
    if let Some(parent) = link_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Try std symlink_dir first, fallback to `cmd /c mklink /J` which works for non-admin users
    if std::os::windows::fs::symlink_dir(target_path, link_path).is_ok() {
        return Ok(());
    }

    let status = std::process::Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            &link_path.to_string_lossy(),
            &target_path.to_string_lossy(),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;

    if !status.success() {
        return Err(AppError::Io(format!(
            "gagal membuat junction dari {} ke {}",
            link_path.display(),
            target_path.display()
        )));
    }
    Ok(())
}
