use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::AppError;

pub type SiteId = String;
pub type ServiceId = String;

pub const ALLOWED_TLDS: &[&str] = &["test", "local", "wip", "corral"];
pub const DEFAULT_BASE_PHP_PORT: u16 = 9000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkerKind {
    Queue,
    Scheduler,
    Reverb,
}

impl WorkerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Scheduler => "scheduler",
            Self::Reverb => "reverb",
        }
    }

    pub fn artisan_args(&self) -> Vec<String> {
        match self {
            Self::Queue => vec![
                "artisan".to_string(),
                "queue:work".to_string(),
                "--tries=3".to_string(),
                "--backoff=3".to_string(),
            ],
            Self::Scheduler => vec!["artisan".to_string(), "schedule:work".to_string()],
            Self::Reverb => vec!["artisan".to_string(), "reverb:start".to_string()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Framework {
    Laravel(String),
    Symfony,
    WordPress,
    PlainPhp,
    Static,
}

impl Framework {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Laravel(_) => "Laravel",
            Self::Symfony => "Symfony",
            Self::WordPress => "WordPress",
            Self::PlainPhp => "Plain PHP",
            Self::Static => "Static",
        }
    }

    pub fn version_badge(&self) -> String {
        match self {
            Self::Laravel(v) if !v.is_empty() => format!("Laravel {v}"),
            Self::Laravel(_) => "Laravel".to_string(),
            Self::Symfony => "Symfony".to_string(),
            Self::WordPress => "WordPress".to_string(),
            Self::PlainPhp => "PHP".to_string(),
            Self::Static => "Static".to_string(),
        }
    }

    pub fn is_laravel(&self) -> bool {
        matches!(self, Self::Laravel(_))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub id: SiteId,
    pub name: String,
    pub path: PathBuf,
    pub document_root: Option<PathBuf>,
    pub hostnames: Vec<String>,
    pub php_version: Option<String>,
    pub node_version: Option<String>,
    pub https: bool,
    pub isolated_services: Vec<ServiceId>,
    pub workers: Vec<WorkerKind>,
    pub framework: Framework,
    pub db_name: Option<String>,
    pub proxy_target: Option<String>,
    pub opcache_enabled: bool,
    pub xdebug_enabled: bool,
    pub created_at: i64,
}

impl Site {
    pub fn primary_host(&self) -> String {
        self.hostnames
            .iter()
            .find(|h| !h.starts_with("*.") && !h.starts_with("www."))
            .or_else(|| self.hostnames.iter().find(|h| !h.starts_with("*.")))
            .cloned()
            .unwrap_or_else(|| format!("{}.test", self.name))
    }

    pub fn effective_doc_root(&self) -> PathBuf {
        if let Some(ref doc_root) = self.document_root {
            if doc_root.is_absolute() {
                return doc_root.clone();
            }
            return self.path.join(doc_root);
        }
        detect_doc_root(&self.path)
    }

    pub fn effective_php_version<'a>(&'a self, global_default: &'a str) -> &'a str {
        self.php_version.as_deref().unwrap_or(global_default)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceKind {
    Nginx,
    PhpCgi,
    MariaDb,
    Postgres,
    Redis,
    Mongo,
    Mail,
    Dumps,
    Worker,
}

impl ServiceKind {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Nginx => "nginx",
            Self::PhpCgi => "PHP FastCGI",
            Self::MariaDb => "MariaDB",
            Self::Postgres => "PostgreSQL",
            Self::Redis => "Redis",
            Self::Mongo => "MongoDB",
            Self::Mail => "Mail Catcher (SMTP)",
            Self::Dumps => "VarDumper Receiver",
            Self::Worker => "Laravel Worker",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceState {
    Stopped,
    Starting,
    Running,
    Degraded,
    Failed { code: i32, reason: String },
}

impl ServiceState {
    pub fn as_badge(&self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Degraded => "degraded",
            Self::Failed { .. } => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInstance {
    pub id: ServiceId,
    pub name: String,
    pub kind: ServiceKind,
    pub version: String,
    pub port: u16,
    pub data_dir: Option<PathBuf>,
    pub pid: Option<u32>,
    pub state: ServiceState,
    pub autostart: bool,
    pub site_id: Option<SiteId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Certificate {
    pub host: String,
    pub ca_thumbprint: String,
    pub leaf_path: PathBuf,
    pub key_path: PathBuf,
    pub not_after: i64,
    pub trusted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostEntry {
    pub ip: String,
    pub hostname: String,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailAttachment {
    pub filename: String,
    pub content_type: String,
    pub size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailMessage {
    pub id: String,
    pub received_at: i64,
    pub envelope_from: String,
    pub recipients: Vec<String>,
    pub subject: String,
    pub preview: String,
    pub html_body: String,
    pub text_body: String,
    pub raw_headers: String,
    pub attachments: Vec<MailAttachment>,
    pub read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DumpEntry {
    pub id: String,
    pub timestamp: i64,
    pub site_name: Option<String>,
    pub file_line: Option<String>,
    pub kind: String,
    pub payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub site_id: SiteId,
    pub site_name: String,
    pub branch: String,
    pub path: PathBuf,
    pub host: String,
    pub db_name: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorralProjectConfig {
    pub php: Option<String>,
    pub node: Option<String>,
    pub https: Option<bool>,
    pub document_root: Option<String>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub workers: Vec<WorkerKind>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl CorralProjectConfig {
    pub fn parse_yaml(content: &str) -> Result<Self, AppError> {
        let cfg: Self = serde_yaml::from_str(content)?;
        if let Some(ref php) = cfg.php {
            let _ = php_fastcgi_port(DEFAULT_BASE_PHP_PORT, php)?;
        }
        Ok(cfg)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub theme: String,
    pub default_php_version: String,
    #[serde(default)]
    pub php_paths: std::collections::BTreeMap<String, PathBuf>,
    pub default_node_version: String,
    pub default_tld: String,
    pub http_port: u16,
    pub https_port: u16,
    pub base_php_port: u16,
    pub smtp_port: u16,
    pub dumps_port: u16,
    pub parked_paths: Vec<PathBuf>,
    pub close_to_tray: bool,
    pub minimize_to_tray: bool,
    pub start_minimized: bool,
    pub autostart_on_login: bool,
    pub mica_effect: bool,
    pub auto_start_services: bool,
    pub auto_configure_laravel_env: bool,
    pub laravel_opcache_turbo: bool,
    pub code_editor: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        let home = std::env::var("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("C:\\Users\\Public"));
        let default_herd_dir = home.join("Herd");
        Self {
            theme: "system".to_string(),
            default_php_version: "8.4".to_string(),
            php_paths: std::collections::BTreeMap::new(),
            default_node_version: "22".to_string(),
            default_tld: "test".to_string(),
            http_port: 80,
            https_port: 443,
            base_php_port: DEFAULT_BASE_PHP_PORT,
            smtp_port: 1025,
            dumps_port: 9912,
            parked_paths: vec![default_herd_dir],
            close_to_tray: false,
            minimize_to_tray: false,
            start_minimized: false,
            autostart_on_login: false,
            mica_effect: false,
            auto_start_services: true,
            auto_configure_laravel_env: true,
            laravel_opcache_turbo: true,
            code_editor: "code".to_string(),
        }
    }
}

/// Sanitize a folder name into a valid RFC-1123 hostname label.
pub fn sanitize_site_name(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Err(AppError::Validation(
            "nama situs tidak boleh kosong".to_string(),
        ));
    }
    let mut out = String::with_capacity(trimmed.len());
    let mut last_dash = false;
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if (ch == '-' || ch == '_' || ch == ' ' || ch == '.') && !out.is_empty() && !last_dash
        {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() || out.len() > 63 {
        return Err(AppError::Validation(format!(
            "nama situs '{raw}' tidak valid setelah normalisasi"
        )));
    }
    Ok(out)
}

/// Validate a hostname against Corral's strict allowlist (DNS-2).
pub fn validate_hostname(hostname: &str) -> Result<(), AppError> {
    let host = hostname.trim().to_ascii_lowercase();
    if host.is_empty() || host.len() > 253 {
        return Err(AppError::Validation(
            "hostname kosong atau terlalu panjang".to_string(),
        ));
    }
    let normalized = host.strip_prefix("*.").unwrap_or(&host);
    // Reject IP literals
    if normalized.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err(AppError::HelperRejected(
            "hostname tidak boleh berupa alamat IP literal".to_string(),
        ));
    }
    if !normalized
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
    {
        return Err(AppError::HelperRejected(format!(
            "karakter tidak diizinkan pada hostname '{hostname}'"
        )));
    }
    let parts: Vec<&str> = normalized.split('.').collect();
    if parts.len() < 2 {
        return Err(AppError::HelperRejected(
            "hostname wajib memiliki TLD (.test, .local, .wip, .corral)".to_string(),
        ));
    }
    let tld = parts.last().copied().unwrap_or_default();
    if !ALLOWED_TLDS.contains(&tld) {
        return Err(AppError::HelperRejected(format!(
            "TLD '.{tld}' tidak berada dalam allowlist Corral (test, local, wip, corral)"
        )));
    }
    for part in parts {
        if part.is_empty() || part.starts_with('-') || part.ends_with('-') {
            return Err(AppError::HelperRejected(format!(
                "segmen hostname tidak valid pada '{hostname}'"
            )));
        }
    }
    Ok(())
}

/// Build the 3 standard hostnames for a site (`<name>.<tld>`, `www.<name>.<tld>`, `*.<name>.<tld>`).
pub fn build_hostnames(site_name: &str, tld: &str) -> Result<Vec<String>, AppError> {
    let clean_name = sanitize_site_name(site_name)?;
    let clean_tld = tld.trim().trim_start_matches('.').to_ascii_lowercase();
    if !ALLOWED_TLDS.contains(&clean_tld.as_str()) {
        return Err(AppError::Validation(format!(
            "TLD '.{clean_tld}' tidak didukung"
        )));
    }
    let primary = format!("{clean_name}.{clean_tld}");
    validate_hostname(&primary)?;
    Ok(vec![
        primary.clone(),
        format!("www.{primary}"),
        format!("*.{primary}"),
    ])
}

/// Compute the FastCGI TCP port for a given PHP version (`base_port + version_int`, e.g., 9000 + 84 = 9084).
pub fn php_fastcgi_port(base_port: u16, php_version: &str) -> Result<u16, AppError> {
    let digits: String = php_version
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(2)
        .collect();
    if digits.len() < 2 {
        return Err(AppError::Validation(format!(
            "versi PHP '{php_version}' tidak valid (contoh: 8.4)"
        )));
    }
    let ver_num: u16 = digits.parse().map_err(|_| {
        AppError::Validation(format!("gagal membaca angka versi PHP '{php_version}'"))
    })?;
    base_port
        .checked_add(ver_num)
        .ok_or_else(|| AppError::Validation("overflow port PHP FastCGI".to_string()))
}

/// Detect the document root of a project directory (WEB-3).
/// If `public/index.php` or `artisan` exists, returns `<path>/public`, otherwise `<path>`.
pub fn detect_doc_root(project_path: &Path) -> PathBuf {
    let public_dir = project_path.join("public");
    if public_dir.join("index.php").is_file() || project_path.join("artisan").is_file() {
        return public_dir;
    }
    project_path.to_path_buf()
}

/// Pure function to detect framework from project indicators.
pub fn detect_framework_from_indicators(
    has_artisan: bool,
    composer_json: Option<&str>,
    has_wp_config: bool,
    has_index_php: bool,
) -> Framework {
    if let Some(raw) = composer_json {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
            if let Some(req) = val.get("require").and_then(|v| v.as_object()) {
                if let Some(laravel_ver) = req.get("laravel/framework").and_then(|v| v.as_str()) {
                    let cleaned = laravel_ver
                        .trim_start_matches('^')
                        .trim_start_matches('~')
                        .to_string();
                    return Framework::Laravel(cleaned);
                }
                if req.contains_key("symfony/framework-bundle") {
                    return Framework::Symfony;
                }
            }
        }
    }
    if has_artisan {
        return Framework::Laravel(String::new());
    }
    if has_wp_config {
        return Framework::WordPress;
    }
    if has_index_php {
        return Framework::PlainPhp;
    }
    Framework::Static
}

/// Detect framework directly from disk path.
pub fn detect_framework_on_disk(project_path: &Path) -> Framework {
    let has_artisan = project_path.join("artisan").is_file();
    let composer_content = std::fs::read_to_string(project_path.join("composer.json")).ok();
    let has_wp = project_path.join("wp-config.php").is_file()
        || project_path.join("wp-login.php").is_file();
    let has_index_php = project_path.join("public").join("index.php").is_file()
        || project_path.join("index.php").is_file();
    detect_framework_from_indicators(
        has_artisan,
        composer_content.as_deref(),
        has_wp,
        has_index_php,
    )
}

/// Render an Nginx server block optimized for Laravel on Windows x64.
pub fn render_nginx_site_conf(
    site: &Site,
    http_port: u16,
    https_port: u16,
    base_php_port: u16,
    global_php: &str,
    cert: Option<&Certificate>,
    reverb_port: Option<u16>,
) -> Result<String, AppError> {
    let php_ver = site.effective_php_version(global_php);
    let fcgi_port = php_fastcgi_port(base_php_port, php_ver)?;
    let doc_root = site
        .effective_doc_root()
        .to_string_lossy()
        .replace('\\', "/");
    let server_names = if site.hostnames.is_empty() {
        format!("{}.test www.{}.test", site.name, site.name)
    } else {
        site.hostnames.join(" ")
    };

    let mut conf = String::with_capacity(2048);
    conf.push_str(&format!(
        "# Auto-generated by Corral for site: {} ({})\n",
        site.name,
        site.framework.version_badge()
    ));

    // If HTTPS is enabled and a certificate exists, redirect HTTP -> HTTPS and serve SSL block
    if site.https {
        if let Some(cert_info) = cert {
            let cert_path = cert_info.leaf_path.to_string_lossy().replace('\\', "/");
            let key_path = cert_info.key_path.to_string_lossy().replace('\\', "/");
            conf.push_str(&format!(
                "server {{\n\
                 \x20   listen 127.0.0.1:{http_port};\n\
                 \x20   server_name {server_names};\n\
                 \x20   return 302 https://$host$request_uri;\n\
                 }}\n\n"
            ));
            conf.push_str(&format!(
                "server {{\n\
                 \x20   listen 127.0.0.1:{https_port} ssl;\n\
                 \x20   http2 on;\n\
                 \x20   server_name {server_names};\n\
                 \x20   ssl_certificate \"{cert_path}\";\n\
                 \x20   ssl_certificate_key \"{key_path}\";\n\
                 \x20   ssl_protocols TLSv1.2 TLSv1.3;\n\
                 \x20   ssl_ciphers HIGH:!aNULL:!MD5;\n"
            ));
        } else {
            conf.push_str(&format!(
                "server {{\n\
                 \x20   listen 127.0.0.1:{http_port};\n\
                 \x20   server_name {server_names};\n"
            ));
        }
    } else {
        conf.push_str(&format!(
            "server {{\n\
             \x20   listen 127.0.0.1:{http_port};\n\
             \x20   server_name {server_names};\n"
        ));
    }

    conf.push_str(&format!(
        "    root \"{doc_root}\";\n\
         \x20   index index.php index.html index.htm;\n\
         \x20   charset utf-8;\n\
         \x20   client_max_body_size 512M;\n\n\
         \x20   # Security & Laravel optimization headers\n\
         \x20   add_header X-Frame-Options \"SAMEORIGIN\";\n\
         \x20   add_header X-Content-Type-Options \"nosniff\";\n\
         \x20   add_header X-Powered-By \"Corral/{php_ver}\";\n\n"
    ));

    if let Some(ref proxy_url) = site.proxy_target {
        conf.push_str(&format!(
            "    location / {{\n\
             \x20       proxy_pass {proxy_url};\n\
             \x20       proxy_http_version 1.1;\n\
             \x20       proxy_set_header Upgrade $http_upgrade;\n\
             \x20       proxy_set_header Connection \"upgrade\";\n\
             \x20       proxy_set_header Host $host;\n\
             \x20       proxy_set_header X-Real-IP $remote_addr;\n\
             \x20       proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n\
             \x20       proxy_set_header X-Forwarded-Proto $scheme;\n\
             \x20   }}\n"
        ));
    } else {
        conf.push_str(
            "    location / {\n\
             \x20       try_files $uri $uri/ /index.php?$query_string;\n\
             \x20   }\n\n\
             \x20   location = /favicon.ico { access_log off; log_not_found off; }\n\
             \x20   location = /robots.txt  { access_log off; log_not_found off; }\n\
             \x20   error_page 404 /index.php;\n\n",
        );

        // Laravel Reverb WebSocket route support (/app and /apps)
        if let Some(rev_port) = reverb_port {
            conf.push_str(&format!(
                "    location /app {{\n\
                 \x20       proxy_http_version 1.1;\n\
                 \x20       proxy_set_header Host $http_host;\n\
                 \x20       proxy_set_header Scheme $scheme;\n\
                 \x20       proxy_set_header SERVER_PORT $server_port;\n\
                 \x20       proxy_set_header REMOTE_ADDR $remote_addr;\n\
                 \x20       proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n\
                 \x20       proxy_set_header Upgrade $http_upgrade;\n\
                 \x20       proxy_set_header Connection \"Upgrade\";\n\
                 \x20       proxy_pass http://127.0.0.1:{rev_port};\n\
                 \x20   }}\n\n"
            ));
        }

        conf.push_str(&format!(
            "    location ~ \\.php$ {{\n\
             \x20       fastcgi_split_path_info ^(.+\\.php)(/.+)$;\n\
             \x20       fastcgi_pass 127.0.0.1:{fcgi_port};\n\
             \x20       fastcgi_index index.php;\n\
             \x20       include fastcgi_params;\n\
             \x20       fastcgi_param SCRIPT_FILENAME $realpath_root$fastcgi_script_name;\n\
             \x20       fastcgi_param DOCUMENT_ROOT $realpath_root;\n\
             \x20       fastcgi_buffers 16 32k;\n\
             \x20       fastcgi_buffer_size 64k;\n\
             \x20       fastcgi_busy_buffers_size 64k;\n\
             \x20       fastcgi_read_timeout 300s;\n\
             \x20       fastcgi_keep_conn on;\n\
             \x20   }}\n\n\
             \x20   location ~ /\\.(?!well-known).* {{\n\
             \x20       deny all;\n\
             \x20   }}\n"
        ));
    }

    conf.push_str("}\n");
    Ok(conf)
}
