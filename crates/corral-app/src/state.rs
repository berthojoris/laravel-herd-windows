use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use corral_core::application::collect_host_entries;
use corral_core::domain::{AppSettings, Certificate};
use corral_core::error::AppError;
use corral_core::ports::{HostsPort, RepositoryPort, TrustStorePort};
use corral_platform::dumps::spawn_dumps_server;
use corral_platform::fs_store::CorralPaths;
use corral_platform::hosts::WindowsHostsManager;
use corral_platform::ini::PhpRuntimeManager;
use corral_platform::nginx::NginxController;
use corral_platform::process::ServiceSupervisor;
use corral_platform::smtp::spawn_smtp_server;
use corral_platform::sqlite::SqliteRepository;
use corral_platform::trust::TlsCertificateManager;
use tokio_util::sync::CancellationToken;

pub struct AppServices {
    pub paths: CorralPaths,
    pub settings: AppSettings,
    pub repo: Arc<SqliteRepository>,
    pub supervisor: Arc<ServiceSupervisor>,
    pub php_mgr: Arc<PhpRuntimeManager>,
    pub nginx: Arc<NginxController>,
    pub tls: Arc<TlsCertificateManager>,
    pub hosts: Arc<WindowsHostsManager>,
    pub cancel_token: CancellationToken,
}

pub type AppStateHandle = Mutex<AppServices>;

impl AppServices {
    pub fn bootstrap() -> Result<Self, AppError> {
        let paths = CorralPaths::discover()?;
        let settings = paths.load_settings()?;
        let repo = Arc::new(SqliteRepository::open(
            &paths.db_path,
            &paths.default_herd_dir,
        )?);
        let supervisor = Arc::new(ServiceSupervisor::new(repo.clone())?);
        let php_mgr = Arc::new(PhpRuntimeManager::new(paths.php_runtimes_dir.clone()));
        let nginx = Arc::new(NginxController::new(
            paths.nginx_etc_dir.clone(),
            paths.nginx_sites_dir.clone(),
            paths.nginx_snippets_dir.clone(),
            paths.logs_dir.clone(),
            paths.runtimes_dir.clone(),
        ));
        let tls = Arc::new(TlsCertificateManager::new(
            paths.ca_dir.clone(),
            paths.ca_sites_dir.clone(),
        ));
        let hosts = Arc::new(WindowsHostsManager::new(paths.appdata_dir.clone()));
        let cancel_token = CancellationToken::new();

        // Ensure default PHP versions have optimized php.ini files ready
        for ver in ["8.4", "8.3", "8.2"] {
            let _ = php_mgr.ensure_version_provisioned(ver);
        }

        // Ensure local CA and initial site certificates exist & sync nginx + hosts block
        let _ = tls.ensure_ca();
        let sites = repo.list_sites().unwrap_or_default();
        let workspaces = repo.list_workspaces(None).unwrap_or_default();
        let mut cert_map: HashMap<String, Certificate> = HashMap::new();
        for site in &sites {
            if site.https {
                if let Ok(c) = tls.issue_site_cert(&site.hostnames) {
                    cert_map.insert(site.primary_host(), c);
                }
            }
        }
        let _ = nginx.sync_all_sites(&sites, &settings, &cert_map);
        let host_entries = collect_host_entries(&sites, &workspaces);
        let _ = hosts.replace_block(&host_entries);

        // Start embedded SMTP Mail Catcher (:1025) & Symfony VarDumper (:9912)
        spawn_smtp_server(settings.smtp_port, repo.clone(), cancel_token.clone());
        spawn_dumps_server(settings.dumps_port, repo.clone(), cancel_token.clone());

        Ok(Self {
            paths,
            settings,
            repo,
            supervisor,
            php_mgr,
            nginx,
            tls,
            hosts,
            cancel_token,
        })
    }

    pub fn sync_routing(&self) -> Result<(), AppError> {
        let sites = self.repo.list_sites()?;
        let workspaces = self.repo.list_workspaces(None)?;
        let mut cert_map: HashMap<String, Certificate> = HashMap::new();
        for site in &sites {
            if site.https {
                if let Ok(c) = self.tls.issue_site_cert(&site.hostnames) {
                    cert_map.insert(site.primary_host(), c);
                }
            }
        }
        self.nginx.sync_all_sites(&sites, &self.settings, &cert_map)?;
        let entries = collect_host_entries(&sites, &workspaces);
        self.hosts.replace_block(&entries)?;
        Ok(())
    }
}
