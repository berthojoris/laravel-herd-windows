use serde::{Deserialize, Serialize};

use crate::domain::{Site, WorkerKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfoView {
    pub name: String,
    pub version: String,
    pub platform: String,
    pub arch: String,
    pub data_dir: String,
    pub local_data_dir: String,
    pub helper_installed: bool,
    pub ca_trusted: bool,
    pub default_php_version: String,
    pub default_node_version: String,
    pub close_to_tray: bool,
    pub minimize_to_tray: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteView {
    pub id: String,
    pub name: String,
    pub host: String,
    pub hostnames: Vec<String>,
    pub path: String,
    pub document_root: String,
    pub php_version: String,
    pub node_version: String,
    pub https: bool,
    pub framework: String,
    pub framework_badge: String,
    pub is_laravel: bool,
    pub healthy: bool,
    pub db_name: Option<String>,
    pub workers: Vec<WorkerKind>,
    pub opcache_enabled: bool,
    pub xdebug_enabled: bool,
    pub has_corral_yml: bool,
    pub url: String,
}

impl SiteView {
    pub fn from_domain(
        site: &Site,
        global_php: &str,
        global_node: &str,
        http_port: u16,
        https_port: u16,
        healthy: bool,
    ) -> Self {
        let host = site.primary_host();
        let php_ver = site.effective_php_version(global_php).to_string();
        let node_ver = site
            .node_version
            .clone()
            .unwrap_or_else(|| global_node.to_string());
        let doc_root = site.effective_doc_root().to_string_lossy().to_string();
        let has_corral_yml = site.path.join(".corral.yml").is_file()
            || site.path.join("herd.yml").is_file();
        let scheme = if site.https { "https" } else { "http" };
        let port_suffix = if site.https && https_port != 443 {
            format!(":{https_port}")
        } else if !site.https && http_port != 80 {
            format!(":{http_port}")
        } else {
            String::new()
        };
        let url = format!("{scheme}://{host}{port_suffix}");

        Self {
            id: site.id.clone(),
            name: site.name.clone(),
            host,
            hostnames: site.hostnames.clone(),
            path: site.path.to_string_lossy().to_string(),
            document_root: doc_root,
            php_version: php_ver,
            node_version: node_ver,
            https: site.https,
            framework: site.framework.label().to_string(),
            framework_badge: site.framework.version_badge(),
            is_laravel: site.framework.is_laravel(),
            healthy,
            db_name: site.db_name.clone(),
            workers: site.workers.clone(),
            opcache_enabled: site.opcache_enabled,
            xdebug_enabled: site.xdebug_enabled,
            has_corral_yml,
            url,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhpVersionView {
    pub version: String,
    pub installed: bool,
    pub active_global: bool,
    pub fastcgi_port: u16,
    pub running: bool,
    pub binary_path: String,
    pub ini_path: String,
    pub extensions: Vec<PhpExtensionView>,
    pub opcache_turbo: bool,
    pub xdebug_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhpExtensionView {
    pub name: String,
    pub enabled: bool,
    pub available: bool,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhpIniSettingsDto {
    pub version: String,
    pub memory_limit: String,
    pub upload_max_filesize: String,
    pub post_max_size: String,
    pub max_execution_time: String,
    pub max_input_vars: String,
    pub realpath_cache_size: String,
    pub realpath_cache_ttl: String,
    pub opcache_enable: bool,
    pub opcache_memory_consumption: String,
    pub opcache_max_accelerated_files: String,
    pub opcache_jit: String,
    pub opcache_jit_buffer_size: String,
    pub xdebug_mode: String,
    pub xdebug_client_port: u16,
    pub raw_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeVersionView {
    pub version: String,
    pub lts_name: String,
    pub installed: bool,
    pub active_global: bool,
    pub composer_installed: bool,
    pub laravel_installer_installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub version: String,
    pub port: u16,
    pub pid: Option<u32>,
    pub state: String,
    pub state_detail: Option<String>,
    pub autostart: bool,
    pub data_dir: Option<String>,
    pub site_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseView {
    pub name: String,
    pub engine: String,
    pub port: u16,
    pub tables_count: usize,
    pub size_kb: u64,
    pub linked_site: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbCredentialsView {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub default_database: String,
    pub connection_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertStatusView {
    pub ca_exists: bool,
    pub ca_trusted: bool,
    pub ca_subject: String,
    pub ca_thumbprint: String,
    pub ca_not_after: String,
    pub ca_path: String,
    pub active_site_certs: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFileView {
    pub id: String,
    pub label: String,
    pub category: String,
    pub path: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub line_number: usize,
    pub timestamp: Option<String>,
    pub level: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailSummaryView {
    pub id: String,
    pub received_at: i64,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub preview: String,
    pub has_attachments: bool,
    pub read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailSettingsView {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub running: bool,
    pub total_captured: usize,
    pub unread_count: usize,
    pub env_snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperStatusView {
    pub installed: bool,
    pub running: bool,
    pub pipe_name: String,
    pub mode: String,
    pub hosts_writable: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortConflictView {
    pub port: u16,
    pub service_label: String,
    pub in_use: bool,
    pub pid: Option<u32>,
    pub process_name: Option<String>,
    pub excluded_by_win_nat: bool,
    pub recommendation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorCheckView {
    pub id: String,
    pub title: String,
    pub status: String, // "ok" | "warning" | "error"
    pub detail: String,
    pub fix_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReportView {
    pub overall_healthy: bool,
    pub checks: Vec<DoctorCheckView>,
    pub port_conflicts: Vec<PortConflictView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaravelOptimizationItem {
    pub key: String,
    pub label: String,
    pub category: String,
    pub current_value: String,
    pub recommended_value: String,
    pub optimized: bool,
    pub impact: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaravelHealthReport {
    pub site_id: String,
    pub site_name: String,
    pub is_laravel: bool,
    pub laravel_version: String,
    pub score: u8,
    pub opcache_turbo: bool,
    pub realpath_cache_tuned: bool,
    pub fastcgi_pool_children: u8,
    pub env_configured: bool,
    pub storage_linked: bool,
    pub items: Vec<LaravelOptimizationItem>,
    pub artisan_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtisanOutputDto {
    pub command: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}
