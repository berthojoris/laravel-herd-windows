use std::os::windows::process::CommandExt;
use std::path::PathBuf;

use corral_core::domain::{render_nginx_site_conf, AppSettings, Certificate, Site};
use corral_core::error::AppError;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::fs_store::write_atomic;

pub struct NginxController {
    etc_dir: PathBuf,
    sites_dir: PathBuf,
    snippets_dir: PathBuf,
    logs_dir: PathBuf,
    nginx_bin: PathBuf,
}

impl NginxController {
    pub fn new(
        etc_dir: PathBuf,
        sites_dir: PathBuf,
        snippets_dir: PathBuf,
        logs_dir: PathBuf,
        runtimes_dir: PathBuf,
    ) -> Self {
        let nginx_bin = runtimes_dir.join("nginx").join("nginx.exe");
        Self {
            etc_dir,
            sites_dir,
            snippets_dir,
            logs_dir,
            nginx_bin,
        }
    }

    pub fn ensure_master_config(&self, settings: &AppSettings) -> Result<PathBuf, AppError> {
        std::fs::create_dir_all(&self.etc_dir)?;
        std::fs::create_dir_all(&self.sites_dir)?;
        std::fs::create_dir_all(&self.snippets_dir)?;

        let error_log = self
            .logs_dir
            .join("nginx-error.log")
            .to_string_lossy()
            .replace('\\', "/");
        let sites_glob = self
            .sites_dir
            .join("*.conf")
            .to_string_lossy()
            .replace('\\', "/");
        let snippets_glob = self
            .snippets_dir
            .join("*.conf")
            .to_string_lossy()
            .replace('\\', "/");

        let master = format!(
            "# Corral Managed Master nginx.conf (Windows x64)\n\
             worker_processes 1;\n\
             error_log \"{error_log}\" warn;\n\n\
             events {{\n\
             \x20   worker_connections 1024;\n\
             }}\n\n\
             http {{\n\
             \x20   include       mime.types;\n\
             \x20   default_type  application/octet-stream;\n\
             \x20   sendfile        on;\n\
             \x20   tcp_nopush      on;\n\
             \x20   tcp_nodelay     on;\n\
             \x20   keepalive_timeout  65;\n\
             \x20   types_hash_max_size 2048;\n\
             \x20   server_names_hash_bucket_size 128;\n\
             \x20   client_max_body_size 512M;\n\
             \x20   gzip on;\n\
             \x20   gzip_types text/plain text/css application/json application/javascript text/xml application/xml;\n\n\
             \x20   # Default fallback server on port {http_port}\n\
             \x20   server {{\n\
             \x20       listen 127.0.0.1:{http_port} default_server;\n\
             \x20       server_name _;\n\
             \x20       return 404;\n\
             \x20   }}\n\n\
             \x20   include \"{snippets_glob}\";\n\
             \x20   include \"{sites_glob}\";\n\
             }}\n",
            http_port = settings.http_port
        );

        let master_path = self.etc_dir.join("nginx.conf");
        write_atomic(&master_path, master.as_bytes())?;
        Ok(master_path)
    }

    pub fn sync_all_sites(
        &self,
        sites: &[Site],
        settings: &AppSettings,
        certs: &std::collections::HashMap<String, Certificate>,
    ) -> Result<(), AppError> {
        let _ = self.ensure_master_config(settings)?;

        // Write each site configuration atomically
        for site in sites {
            let cert = certs.get(&site.primary_host());
            let conf = render_nginx_site_conf(
                site,
                settings.http_port,
                settings.https_port,
                settings.base_php_port,
                &settings.default_php_version,
                cert,
                Some(8080),
            )?;
            let target = self.sites_dir.join(format!("{}.conf", site.name));
            write_atomic(&target, conf.as_bytes())?;
        }

        // Clean up removed site conf files
        if let Ok(entries) = std::fs::read_dir(&self.sites_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("conf") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if !sites.iter().any(|s| s.name == stem) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
            }
        }

        self.validate_and_reload()
    }

    pub fn validate_and_reload(&self) -> Result<(), AppError> {
        if !self.nginx_bin.is_file() {
            // Runtime binary is on-demand; config files are already written & validated syntactically.
            return Ok(());
        }

        let master_conf = self.etc_dir.join("nginx.conf");
        let test_out = std::process::Command::new(&self.nginx_bin)
            .args(["-t", "-c", &master_conf.to_string_lossy()])
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;

        if !test_out.status.success() {
            let stderr = String::from_utf8_lossy(&test_out.stderr).to_string();
            return Err(AppError::NginxConfig(stderr));
        }

        let _ = std::process::Command::new(&self.nginx_bin)
            .args(["-s", "reload", "-c", &master_conf.to_string_lossy()])
            .creation_flags(CREATE_NO_WINDOW)
            .status();

        Ok(())
    }
}
