use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use corral_core::domain::{
    DumpEntry, Framework, MailAttachment, MailMessage, ServiceInstance, ServiceKind, ServiceState,
    Site, WorkerKind, Workspace,
};
use corral_core::dto::{DatabaseView, MailSummaryView};
use corral_core::error::AppError;
use corral_core::ports::RepositoryPort;
use rusqlite::{params, Connection, OptionalExtension};

const MAX_STORED_MAILS: i64 = 500;
const MAX_STORED_DUMPS: i64 = 500;

pub struct SqliteRepository {
    conn: Mutex<Connection>,
}

impl SqliteRepository {
    pub fn open(db_path: &Path, default_herd_dir: &Path) -> Result<Self, AppError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)
            .map_err(|e| AppError::Io(format!("gagal membuka SQLite {}: {e}", db_path.display())))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| AppError::Io(format!("gagal mengatur SQLite busy_timeout: {e}")))?;

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;

             CREATE TABLE IF NOT EXISTS sites (
                 id TEXT PRIMARY KEY,
                 name TEXT UNIQUE NOT NULL,
                 path TEXT NOT NULL,
                 document_root TEXT,
                 php_version TEXT,
                 node_version TEXT,
                 https INTEGER NOT NULL DEFAULT 0,
                 framework_json TEXT NOT NULL,
                 db_name TEXT,
                 proxy_target TEXT,
                 workers_json TEXT NOT NULL DEFAULT '[]',
                 opcache_enabled INTEGER NOT NULL DEFAULT 1,
                 xdebug_enabled INTEGER NOT NULL DEFAULT 0,
                 created_at INTEGER NOT NULL
             );

             CREATE TABLE IF NOT EXISTS hosts (
                 site_id TEXT NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
                 host TEXT NOT NULL,
                 PRIMARY KEY(site_id, host)
             );

             CREATE TABLE IF NOT EXISTS services (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 kind_json TEXT NOT NULL,
                 version TEXT NOT NULL,
                 port INTEGER NOT NULL,
                 data_dir TEXT,
                 autostart INTEGER NOT NULL DEFAULT 1,
                 site_id TEXT
             );

             CREATE TABLE IF NOT EXISTS mails (
                 id TEXT PRIMARY KEY,
                 received_at INTEGER NOT NULL,
                 envelope_from TEXT NOT NULL,
                 recipients_json TEXT NOT NULL,
                 subject TEXT NOT NULL,
                 preview TEXT NOT NULL,
                 html_body TEXT NOT NULL,
                 text_body TEXT NOT NULL,
                 raw_headers TEXT NOT NULL,
                 attachments_json TEXT NOT NULL DEFAULT '[]',
                 read_flag INTEGER NOT NULL DEFAULT 0
             );

             CREATE TABLE IF NOT EXISTS dumps (
                 id TEXT PRIMARY KEY,
                 timestamp INTEGER NOT NULL,
                 site_name TEXT,
                 file_line TEXT,
                 kind TEXT NOT NULL,
                 payload TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS workspaces (
                 id TEXT PRIMARY KEY,
                 site_id TEXT NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
                 site_name TEXT NOT NULL,
                 branch TEXT NOT NULL,
                 path TEXT NOT NULL,
                 host TEXT NOT NULL,
                 db_name TEXT,
                 created_at INTEGER NOT NULL
             );

             CREATE TABLE IF NOT EXISTS download_ledger (
                 component TEXT NOT NULL,
                 version TEXT NOT NULL,
                 sha256 TEXT NOT NULL,
                 installed_at INTEGER NOT NULL,
                 PRIMARY KEY(component, version)
             );

             CREATE TABLE IF NOT EXISTS databases_meta (
                 name TEXT PRIMARY KEY,
                 engine TEXT NOT NULL,
                 port INTEGER NOT NULL,
                 tables_count INTEGER NOT NULL DEFAULT 12,
                 size_kb INTEGER NOT NULL DEFAULT 2048,
                 linked_site TEXT
             );

             CREATE INDEX IF NOT EXISTS idx_hosts_site_id ON hosts(site_id);
             CREATE INDEX IF NOT EXISTS idx_mails_received_at ON mails(received_at DESC);
             CREATE INDEX IF NOT EXISTS idx_dumps_timestamp ON dumps(timestamp DESC);",
        )
        .map_err(|e| AppError::Io(format!("gagal migrasi skema SQLite: {e}")))?;

        let repo = Self {
            conn: Mutex::new(conn),
        };
        repo.seed_if_empty(default_herd_dir)?;
        Ok(repo)
    }

    fn has_rows_query(&self, exists_sql: &'static str) -> Result<bool, AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let exists: i32 = guard
            .query_row(exists_sql, [], |row| row.get(0))
            .map_err(|e| AppError::Io(e.to_string()))?;
        Ok(exists != 0)
    }

    fn seed_if_empty(&self, default_herd_dir: &Path) -> Result<(), AppError> {
        if !self.has_rows_query("SELECT EXISTS(SELECT 1 FROM services LIMIT 1)")? {
            let defaults = vec![
                ServiceInstance {
                    id: "svc-nginx".to_string(),
                    name: "nginx Web Server".to_string(),
                    kind: ServiceKind::Nginx,
                    version: "1.26.2".to_string(),
                    port: 80,
                    data_dir: None,
                    pid: None,
                    state: ServiceState::Running,
                    autostart: true,
                    site_id: None,
                },
                ServiceInstance {
                    id: "svc-php-84".to_string(),
                    name: "PHP 8.4 FastCGI Pool".to_string(),
                    kind: ServiceKind::PhpCgi,
                    version: "8.4.2".to_string(),
                    port: 9084,
                    data_dir: None,
                    pid: None,
                    state: ServiceState::Running,
                    autostart: true,
                    site_id: None,
                },
                ServiceInstance {
                    id: "svc-php-83".to_string(),
                    name: "PHP 8.3 FastCGI Pool".to_string(),
                    kind: ServiceKind::PhpCgi,
                    version: "8.3.15".to_string(),
                    port: 9083,
                    data_dir: None,
                    pid: None,
                    state: ServiceState::Running,
                    autostart: true,
                    site_id: None,
                },
                ServiceInstance {
                    id: "svc-mariadb".to_string(),
                    name: "MariaDB Server".to_string(),
                    kind: ServiceKind::MariaDb,
                    version: "11.4.4".to_string(),
                    port: 3306,
                    data_dir: None,
                    pid: None,
                    state: ServiceState::Running,
                    autostart: true,
                    site_id: None,
                },
                ServiceInstance {
                    id: "svc-redis".to_string(),
                    name: "Redis In-Memory Store".to_string(),
                    kind: ServiceKind::Redis,
                    version: "7.4.1".to_string(),
                    port: 6379,
                    data_dir: None,
                    pid: None,
                    state: ServiceState::Running,
                    autostart: true,
                    site_id: None,
                },
                ServiceInstance {
                    id: "svc-postgres".to_string(),
                    name: "PostgreSQL Server".to_string(),
                    kind: ServiceKind::Postgres,
                    version: "17.2".to_string(),
                    port: 5432,
                    data_dir: None,
                    pid: None,
                    state: ServiceState::Stopped,
                    autostart: false,
                    site_id: None,
                },
                ServiceInstance {
                    id: "svc-mail".to_string(),
                    name: "Corral Mail Catcher (SMTP)".to_string(),
                    kind: ServiceKind::Mail,
                    version: "1.0.0".to_string(),
                    port: 1025,
                    data_dir: None,
                    pid: None,
                    state: ServiceState::Running,
                    autostart: true,
                    site_id: None,
                },
                ServiceInstance {
                    id: "svc-dumps".to_string(),
                    name: "Symfony/Laravel VarDumper".to_string(),
                    kind: ServiceKind::Dumps,
                    version: "1.0.0".to_string(),
                    port: 9912,
                    data_dir: None,
                    pid: None,
                    state: ServiceState::Running,
                    autostart: true,
                    site_id: None,
                },
            ];
            for svc in defaults {
                self.upsert_service(&svc)?;
            }
        }

        if !self.has_rows_query("SELECT EXISTS(SELECT 1 FROM sites LIMIT 1)")? {
            // Create two realistic Laravel project skeletons inside default_herd_dir so files exist on disk
            let app1_path = default_herd_dir.join("laravel-commerce");
            let app2_path = default_herd_dir.join("filament-crm");
            Self::scaffold_laravel_skeleton(&app1_path, "laravel-commerce", "11.31", true)?;
            Self::scaffold_laravel_skeleton(&app2_path, "filament-crm", "11.28", false)?;

            let site1 = Site {
                id: "site-laravel-commerce".to_string(),
                name: "laravel-commerce".to_string(),
                path: app1_path.clone(),
                document_root: Some(app1_path.join("public")),
                hostnames: vec![
                    "laravel-commerce.test".to_string(),
                    "www.laravel-commerce.test".to_string(),
                    "*.laravel-commerce.test".to_string(),
                ],
                php_version: Some("8.4".to_string()),
                node_version: Some("22".to_string()),
                https: true,
                isolated_services: vec![],
                workers: vec![WorkerKind::Queue, WorkerKind::Scheduler],
                framework: Framework::Laravel("11.31".to_string()),
                db_name: Some("laravel_commerce".to_string()),
                proxy_target: None,
                opcache_enabled: true,
                xdebug_enabled: false,
                created_at: 1_762_740_000,
            };

            let site2 = Site {
                id: "site-filament-crm".to_string(),
                name: "filament-crm".to_string(),
                path: app2_path.clone(),
                document_root: Some(app2_path.join("public")),
                hostnames: vec![
                    "filament-crm.test".to_string(),
                    "www.filament-crm.test".to_string(),
                    "*.filament-crm.test".to_string(),
                ],
                php_version: Some("8.3".to_string()),
                node_version: Some("22".to_string()),
                https: false,
                isolated_services: vec![],
                workers: vec![WorkerKind::Queue],
                framework: Framework::Laravel("11.28".to_string()),
                db_name: Some("filament_crm".to_string()),
                proxy_target: None,
                opcache_enabled: true,
                xdebug_enabled: false,
                created_at: 1_762_741_000,
            };

            self.upsert_site(&site1)?;
            self.upsert_site(&site2)?;

            let ws = Workspace {
                id: "ws-feature-checkout".to_string(),
                site_id: site1.id.clone(),
                site_name: site1.name.clone(),
                branch: "feature-checkout-v2".to_string(),
                path: default_herd_dir.join("laravel-commerce-checkout"),
                host: "feature-checkout-v2.laravel-commerce.test".to_string(),
                db_name: Some("laravel_commerce_checkout".to_string()),
                created_at: 1_762_742_000,
            };
            self.upsert_workspace(&ws)?;

            self.upsert_database("laravel_commerce", "MariaDB 11.4", 3306, 28, 8420, Some("laravel-commerce.test"))?;
            self.upsert_database("filament_crm", "MariaDB 11.4", 3306, 19, 4120, Some("filament-crm.test"))?;
            self.upsert_database("laravel_commerce_checkout", "MariaDB 11.4", 3306, 28, 3150, Some("feature-checkout-v2.laravel-commerce.test"))?;
        }

        if !self.has_rows_query("SELECT EXISTS(SELECT 1 FROM mails LIMIT 1)")? {
            let demo_mails = vec![
                MailMessage {
                    id: "mail-1".to_string(),
                    received_at: 1_762_749_600,
                    envelope_from: "no-reply@laravel-commerce.test".to_string(),
                    recipients: vec!["bertho@developer.test".to_string()],
                    subject: "Verify Email Address — Laravel Commerce".to_string(),
                    preview: "Please click the button below to verify your email address for Laravel Commerce.".to_string(),
                    html_body: "<div style=\"font-family:'Segoe UI',sans-serif;padding:24px;color:#18181b;background:#ffffff;border:1px solid #e4e4e7;border-radius:8px;\"><h2 style=\"margin-top:0;color:#ef4444;\">Laravel Commerce</h2><p>Hello <strong>Bertho</strong>,</p><p>Please click the button below to verify your email address and activate your developer sandbox account.</p><p style=\"margin:24px 0;\"><a href=\"https://laravel-commerce.test/verify/98f12a\" style=\"background:#ef4444;color:#fff;padding:10px 18px;border-radius:6px;text-decoration:none;font-weight:600;\">Verify Email Address</a></p><p style=\"font-size:12px;color:#71717a;\">Captured locally by Corral SMTP Server (127.0.0.1:1025)</p></div>".to_string(),
                    text_body: "Hello Bertho,\nPlease verify your email address: https://laravel-commerce.test/verify/98f12a".to_string(),
                    raw_headers: "From: Laravel Commerce <no-reply@laravel-commerce.test>\r\nTo: bertho@developer.test\r\nSubject: Verify Email Address — Laravel Commerce\r\nX-Mailer: Laravel/11.31 (Corral SMTP)".to_string(),
                    attachments: vec![],
                    read: false,
                },
                MailMessage {
                    id: "mail-2".to_string(),
                    received_at: 1_762_749_950,
                    envelope_from: "billing@laravel-commerce.test".to_string(),
                    recipients: vec!["finance@customer.test".to_string()],
                    subject: "Invoice #INV-2026-0891 Paid Successfully".to_string(),
                    preview: "Thank you for your order! Your PDF tax invoice is attached to this email.".to_string(),
                    html_body: "<div style=\"font-family:'Segoe UI',sans-serif;padding:24px;color:#18181b;background:#ffffff;border:1px solid #e4e4e7;border-radius:8px;\"><h2 style=\"margin-top:0;color:#10b981;\">Payment Confirmed (#INV-2026-0891)</h2><p>Your Laravel Queue worker processed payment webhook <code>evt_3Q9xLm</code> in <strong>18 ms</strong>.</p><table style=\"width:100%;border-collapse:collapse;margin:16px 0;font-size:13px;\"><tr style=\"border-bottom:1px solid #e4e4e7;\"><th style=\"text-align:left;padding:8px 0;\">Item</th><th style=\"text-align:right;padding:8px 0;\">Amount</th></tr><tr><td style=\"padding:8px 0;\">Corral Enterprise License (Annual)</td><td style=\"text-align:right;padding:8px 0;\">Rp 2.450.000</td></tr></table></div>".to_string(),
                    text_body: "Payment Confirmed (#INV-2026-0891) - Rp 2.450.000".to_string(),
                    raw_headers: "From: Billing <billing@laravel-commerce.test>\r\nTo: finance@customer.test\r\nSubject: Invoice #INV-2026-0891 Paid Successfully".to_string(),
                    attachments: vec![MailAttachment {
                        filename: "INV-2026-0891.pdf".to_string(),
                        content_type: "application/pdf".to_string(),
                        size_bytes: 84_120,
                    }],
                    read: false,
                },
            ];
            for m in demo_mails {
                self.insert_mail(&m)?;
            }
        }

        if !self.has_rows_query("SELECT EXISTS(SELECT 1 FROM dumps LIMIT 1)")? {
            let demo_dumps = vec![
                DumpEntry {
                    id: "dump-1".to_string(),
                    timestamp: 1_762_750_100,
                    site_name: Some("laravel-commerce.test".to_string()),
                    file_line: Some("app/Http/Controllers/CheckoutController.php:48".to_string()),
                    kind: "Eloquent\\Model".to_string(),
                    payload: "App\\Models\\Order {#1842\n  +id: \"0193158a-7c40-7120-9a11-08b22e41c901\"\n  +status: \"paid\"\n  +total_idr: 2450000\n  +customer_email: \"bertho@developer.test\"\n  +opcache_hit: true\n}".to_string(),
                },
                DumpEntry {
                    id: "dump-2".to_string(),
                    timestamp: 1_762_750_210,
                    site_name: Some("filament-crm.test".to_string()),
                    file_line: Some("app/Filament/Resources/LeadResource.php:92".to_string()),
                    kind: "Support\\Collection".to_string(),
                    payload: "Illuminate\\Support\\Collection {#2104\n  #items: array:3 [\n    0 => \"Lead#101 (Enterprise - Qualified)\"\n    1 => \"Lead#102 (Startup - Demo Scheduled)\"\n    2 => \"Lead#103 (Studio - Closed Won)\"\n  ]\n}".to_string(),
                },
            ];
            for d in demo_dumps {
                self.insert_dump(&d)?;
            }
        }

        Ok(())
    }

    pub fn scaffold_laravel_skeleton(
        dir: &Path,
        name: &str,
        laravel_ver: &str,
        with_corral_yml: bool,
    ) -> Result<(), AppError> {
        let public_dir = dir.join("public");
        let storage_logs = dir.join("storage").join("logs");
        let storage_public = dir.join("storage").join("app").join("public");
        std::fs::create_dir_all(&public_dir)?;
        std::fs::create_dir_all(&storage_logs)?;
        std::fs::create_dir_all(&storage_public)?;

        let artisan_file = dir.join("artisan");
        if !artisan_file.exists() {
            std::fs::write(
                &artisan_file,
                "#!/usr/bin/env php\n<?php\n// Laravel Artisan CLI Bootstrap managed by Corral\necho \"Laravel Framework "
                    .to_string()
                    + laravel_ver
                    + " (Corral Optimized Runtime)\\n\";\n",
            )?;
        }

        let composer_file = dir.join("composer.json");
        if !composer_file.exists() {
            let composer_json = format!(
                "{{\n  \"name\": \"corral/{name}\",\n  \"type\": \"project\",\n  \"require\": {{\n    \"php\": \"^8.2\",\n    \"laravel/framework\": \"^{laravel_ver}\"\n  }}\n}}\n"
            );
            std::fs::write(&composer_file, composer_json)?;
        }

        let index_file = public_dir.join("index.php");
        if !index_file.exists() {
            let index_php = format!(
                "<?php\n// Corral Laravel Front Controller ({name})\nheader('X-Corral-Site: {name}');\necho '<h1>{name}.test — Running on Corral (Laravel {laravel_ver})</h1>';\n"
            );
            std::fs::write(&index_file, index_php)?;
        }

        let dotenv_file = dir.join(".env");
        if !dotenv_file.exists() {
            let db_name = name.replace('-', "_");
            let env_content = format!(
                "APP_NAME=\"{name}\"\nAPP_ENV=local\nAPP_KEY=base64:CorralGeneratedKeyForLocalDevOnly1234567890=\nAPP_DEBUG=true\nAPP_URL=https://{name}.test\n\nDB_CONNECTION=mysql\nDB_HOST=127.0.0.1\nDB_PORT=3306\nDB_DATABASE={db_name}\nDB_USERNAME=root\nDB_PASSWORD=\n\nCACHE_STORE=redis\nQUEUE_CONNECTION=redis\nREDIS_HOST=127.0.0.1\nREDIS_PORT=6379\n\nMAIL_MAILER=smtp\nMAIL_HOST=127.0.0.1\nMAIL_PORT=1025\nMAIL_USERNAME=null\nMAIL_PASSWORD=null\nMAIL_ENCRYPTION=null\nMAIL_FROM_ADDRESS=\"hello@{name}.test\"\n\nVAR_DUMPER_FORMAT=server\nVAR_DUMPER_SERVER=127.0.0.1:9912\n"
            );
            std::fs::write(&dotenv_file, env_content)?;
        }

        if with_corral_yml {
            let yml_file = dir.join(".corral.yml");
            if !yml_file.exists() {
                std::fs::write(
                    &yml_file,
                    "php: \"8.4\"\nnode: \"22\"\nhttps: true\ndocumentRoot: \"public\"\nservices:\n  - mariadb\n  - redis\nworkers:\n  - queue\n  - scheduler\n",
                )?;
            }
        }

        let site_log = storage_logs.join("laravel.log");
        if !site_log.exists() {
            std::fs::write(
                &site_log,
                format!(
                    "[2026-11-10 11:40:12] local.INFO: {name} booted with OPcache JIT (tracing, 128M) and NTFS realpath cache (4096K)\n[2026-11-10 11:41:05] local.INFO: Database connection established to 127.0.0.1:3306\n"
                ),
            )?;
        }

        Ok(())
    }

    pub fn list_databases_meta(&self) -> Result<Vec<DatabaseView>, AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let mut stmt = guard
            .prepare(
                "SELECT name, engine, port, tables_count, size_kb, linked_site FROM databases_meta ORDER BY name ASC",
            )
            .map_err(|e| AppError::Io(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(DatabaseView {
                    name: row.get::<_, String>(0)?,
                    engine: row.get::<_, String>(1)?,
                    port: row.get::<_, u16>(2)?,
                    tables_count: row.get::<_, usize>(3)?,
                    size_kb: row.get::<_, u64>(4)?,
                    linked_site: row.get::<_, Option<String>>(5)?,
                })
            })
            .map_err(|e| AppError::Io(e.to_string()))?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r.map_err(|e| AppError::Io(e.to_string()))?);
        }
        Ok(list)
    }

    pub fn upsert_database(
        &self,
        name: &str,
        engine: &str,
        port: u16,
        tables_count: usize,
        size_kb: u64,
        linked_site: Option<&str>,
    ) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        guard
            .execute(
                "INSERT INTO databases_meta (name, engine, port, tables_count, size_kb, linked_site)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(name) DO UPDATE SET
                   engine = excluded.engine,
                   port = excluded.port,
                   tables_count = excluded.tables_count,
                   size_kb = excluded.size_kb,
                   linked_site = excluded.linked_site",
                params![name, engine, port, tables_count, size_kb, linked_site],
            )
            .map_err(|e| AppError::Io(e.to_string()))?;
        Ok(())
    }

    pub fn drop_database_meta(&self, name: &str) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        guard
            .execute("DELETE FROM databases_meta WHERE name = ?1", params![name])
            .map_err(|e| AppError::Io(e.to_string()))?;
        Ok(())
    }
}

impl RepositoryPort for SqliteRepository {
    fn list_sites(&self) -> Result<Vec<Site>, AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let mut stmt = guard
            .prepare_cached(
                "SELECT id, name, path, document_root, php_version, node_version, https,
                        framework_json, db_name, proxy_target, workers_json,
                        opcache_enabled, xdebug_enabled, created_at
                 FROM sites ORDER BY name ASC",
            )
            .map_err(|e| AppError::Io(e.to_string()))?;

        let site_rows = stmt
            .query_map([], map_raw_site_tuple)
            .map_err(|e| AppError::Io(e.to_string()))?;

        let mut sites = Vec::new();
        for row_res in site_rows {
            let raw = row_res.map_err(|e| AppError::Io(e.to_string()))?;
            let hostnames = load_site_hostnames(&guard, &raw.0, &raw.1)?;
            sites.push(build_site_from_raw(raw, hostnames));
        }
        Ok(sites)
    }

    fn get_site(&self, id: &str) -> Result<Option<Site>, AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let mut stmt = guard
            .prepare_cached(
                "SELECT id, name, path, document_root, php_version, node_version, https,
                        framework_json, db_name, proxy_target, workers_json,
                        opcache_enabled, xdebug_enabled, created_at
                 FROM sites
                 WHERE id = ?1 OR name = ?1
                 LIMIT 1",
            )
            .map_err(|e| AppError::Io(e.to_string()))?;

        let maybe_raw = stmt
            .query_row(params![id], map_raw_site_tuple)
            .optional()
            .map_err(|e| AppError::Io(e.to_string()))?;

        let Some(raw) = maybe_raw else {
            return Ok(None);
        };
        let hostnames = load_site_hostnames(&guard, &raw.0, &raw.1)?;
        Ok(Some(build_site_from_raw(raw, hostnames)))
    }

    fn upsert_site(&self, site: &Site) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let fw_json = serde_json::to_string(&site.framework)?;
        let workers_json = serde_json::to_string(&site.workers)?;
        let path_str = site.path.to_string_lossy().to_string();
        let doc_root_str = site
            .document_root
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());

        guard
            .execute(
                "INSERT INTO sites (
                    id, name, path, document_root, php_version, node_version, https,
                    framework_json, db_name, proxy_target, workers_json,
                    opcache_enabled, xdebug_enabled, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    path = excluded.path,
                    document_root = excluded.document_root,
                    php_version = excluded.php_version,
                    node_version = excluded.node_version,
                    https = excluded.https,
                    framework_json = excluded.framework_json,
                    db_name = excluded.db_name,
                    proxy_target = excluded.proxy_target,
                    workers_json = excluded.workers_json,
                    opcache_enabled = excluded.opcache_enabled,
                    xdebug_enabled = excluded.xdebug_enabled",
                params![
                    site.id,
                    site.name,
                    path_str,
                    doc_root_str,
                    site.php_version,
                    site.node_version,
                    i32::from(site.https),
                    fw_json,
                    site.db_name,
                    site.proxy_target,
                    workers_json,
                    i32::from(site.opcache_enabled),
                    i32::from(site.xdebug_enabled),
                    site.created_at,
                ],
            )
            .map_err(|e| AppError::Io(e.to_string()))?;

        guard
            .execute("DELETE FROM hosts WHERE site_id = ?1", params![site.id])
            .map_err(|e| AppError::Io(e.to_string()))?;
        for host in &site.hostnames {
            guard
                .execute(
                    "INSERT OR IGNORE INTO hosts (site_id, host) VALUES (?1, ?2)",
                    params![site.id, host],
                )
                .map_err(|e| AppError::Io(e.to_string()))?;
        }
        Ok(())
    }

    fn delete_site(&self, id: &str) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        guard
            .execute(
                "DELETE FROM sites WHERE id = ?1 OR name = ?1",
                params![id],
            )
            .map_err(|e| AppError::Io(e.to_string()))?;
        Ok(())
    }

    fn list_services(&self) -> Result<Vec<ServiceInstance>, AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let mut stmt = guard
            .prepare_cached(
                "SELECT id, name, kind_json, version, port, data_dir, autostart, site_id
                 FROM services ORDER BY port ASC",
            )
            .map_err(|e| AppError::Io(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let kind_json: String = row.get(2)?;
                let version: String = row.get(3)?;
                let port: u16 = row.get(4)?;
                let data_dir: Option<String> = row.get(5)?;
                let autostart: i32 = row.get(6)?;
                let site_id: Option<String> = row.get(7)?;
                Ok((id, name, kind_json, version, port, data_dir, autostart != 0, site_id))
            })
            .map_err(|e| AppError::Io(e.to_string()))?;

        let mut list = Vec::new();
        for r in rows {
            let (id, name, kind_json, version, port, data_dir, autostart, site_id) =
                r.map_err(|e| AppError::Io(e.to_string()))?;
            let kind: ServiceKind = serde_json::from_str(&kind_json).unwrap_or(ServiceKind::Worker);
            list.push(ServiceInstance {
                id,
                name,
                kind,
                version,
                port,
                data_dir: data_dir.map(PathBuf::from),
                pid: None,
                state: if autostart {
                    ServiceState::Running
                } else {
                    ServiceState::Stopped
                },
                autostart,
                site_id,
            });
        }
        Ok(list)
    }

    fn upsert_service(&self, svc: &ServiceInstance) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let kind_json = serde_json::to_string(&svc.kind)?;
        let data_dir_str = svc
            .data_dir
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());
        guard
            .execute(
                "INSERT INTO services (id, name, kind_json, version, port, data_dir, autostart, site_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                   name = excluded.name,
                   kind_json = excluded.kind_json,
                   version = excluded.version,
                   port = excluded.port,
                   data_dir = excluded.data_dir,
                   autostart = excluded.autostart,
                   site_id = excluded.site_id",
                params![
                    svc.id,
                    svc.name,
                    kind_json,
                    svc.version,
                    svc.port,
                    data_dir_str,
                    i32::from(svc.autostart),
                    svc.site_id,
                ],
            )
            .map_err(|e| AppError::Io(e.to_string()))?;
        Ok(())
    }

    fn delete_service(&self, id: &str) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        guard
            .execute("DELETE FROM services WHERE id = ?1", params![id])
            .map_err(|e| AppError::Io(e.to_string()))?;
        Ok(())
    }

    fn list_mails(&self) -> Result<Vec<MailMessage>, AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let mut stmt = guard
            .prepare_cached(
                "SELECT id, received_at, envelope_from, recipients_json, subject, preview,
                        html_body, text_body, raw_headers, attachments_json, read_flag
                 FROM mails ORDER BY received_at DESC LIMIT 200",
            )
            .map_err(|e| AppError::Io(e.to_string()))?;
        let rows = stmt
            .query_map([], map_mail_row)
            .map_err(|e| AppError::Io(e.to_string()))?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r.map_err(|e| AppError::Io(e.to_string()))?);
        }
        Ok(list)
    }

    fn list_mail_summaries(&self) -> Result<Vec<MailSummaryView>, AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let mut stmt = guard
            .prepare_cached(
                "SELECT id, received_at, envelope_from, recipients_json, subject, preview,
                        attachments_json, read_flag
                 FROM mails ORDER BY received_at DESC LIMIT 200",
            )
            .map_err(|e| AppError::Io(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let received_at: i64 = row.get(1)?;
                let envelope_from: String = row.get(2)?;
                let recipients_json: String = row.get(3)?;
                let subject: String = row.get(4)?;
                let preview: String = row.get(5)?;
                let attachments_json: String = row.get(6)?;
                let read_int: i32 = row.get(7)?;
                let recipients: Vec<String> =
                    serde_json::from_str(&recipients_json).unwrap_or_default();
                let attachments: Vec<MailAttachment> =
                    serde_json::from_str(&attachments_json).unwrap_or_default();
                let has_attachments = !attachments.is_empty();
                Ok(MailSummaryView {
                    id,
                    received_at,
                    envelope_from,
                    recipients,
                    subject,
                    preview,
                    attachments,
                    has_attachments,
                    read: read_int != 0,
                })
            })
            .map_err(|e| AppError::Io(e.to_string()))?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r.map_err(|e| AppError::Io(e.to_string()))?);
        }
        Ok(list)
    }

    fn count_mails(&self) -> Result<(usize, usize), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let mut stmt = guard
            .prepare_cached(
                "SELECT COUNT(*), COALESCE(SUM(CASE WHEN read_flag = 0 THEN 1 ELSE 0 END), 0)
                 FROM mails",
            )
            .map_err(|e| AppError::Io(e.to_string()))?;
        let (total_i64, unread_i64): (i64, i64) = stmt
            .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| AppError::Io(e.to_string()))?;
        let total = usize::try_from(total_i64.max(0)).unwrap_or(0);
        let unread = usize::try_from(unread_i64.max(0)).unwrap_or(0);
        Ok((total, unread))
    }

    fn get_mail(&self, id: &str) -> Result<Option<MailMessage>, AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let mut stmt = guard
            .prepare_cached(
                "SELECT id, received_at, envelope_from, recipients_json, subject, preview,
                        html_body, text_body, raw_headers, attachments_json, read_flag
                 FROM mails
                 WHERE id = ?1
                 LIMIT 1",
            )
            .map_err(|e| AppError::Io(e.to_string()))?;
        stmt.query_row(params![id], map_mail_row)
            .optional()
            .map_err(|e| AppError::Io(e.to_string()))
    }

    fn insert_mail(&self, mail: &MailMessage) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let recipients_json = serde_json::to_string(&mail.recipients)?;
        let attachments_json = serde_json::to_string(&mail.attachments)?;
        guard
            .execute(
                "INSERT OR REPLACE INTO mails (
                    id, received_at, envelope_from, recipients_json, subject, preview,
                    html_body, text_body, raw_headers, attachments_json, read_flag
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    mail.id,
                    mail.received_at,
                    mail.envelope_from,
                    recipients_json,
                    mail.subject,
                    mail.preview,
                    mail.html_body,
                    mail.text_body,
                    mail.raw_headers,
                    attachments_json,
                    i32::from(mail.read),
                ],
            )
            .map_err(|e| AppError::Io(e.to_string()))?;

        // Ring-buffer pruning: keep at most MAX_STORED_MAILS latest emails so SQLite stays bounded.
        let _ = guard.execute(
            "DELETE FROM mails WHERE id NOT IN (
                 SELECT id FROM mails ORDER BY received_at DESC, id DESC LIMIT ?1
             )",
            params![MAX_STORED_MAILS],
        );
        Ok(())
    }

    fn mark_mail_read(&self, id: &str) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        guard
            .execute(
                "UPDATE mails SET read_flag = 1 WHERE id = ?1",
                params![id],
            )
            .map_err(|e| AppError::Io(e.to_string()))?;
        Ok(())
    }

    fn delete_mail(&self, id: &str) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        guard
            .execute("DELETE FROM mails WHERE id = ?1", params![id])
            .map_err(|e| AppError::Io(e.to_string()))?;
        Ok(())
    }

    fn clear_mails(&self) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        guard
            .execute("DELETE FROM mails", [])
            .map_err(|e| AppError::Io(e.to_string()))?;
        Ok(())
    }

    fn list_dumps(&self) -> Result<Vec<DumpEntry>, AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let mut stmt = guard
            .prepare_cached(
                "SELECT id, timestamp, site_name, file_line, kind, payload
                 FROM dumps ORDER BY timestamp DESC LIMIT 200",
            )
            .map_err(|e| AppError::Io(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(DumpEntry {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    site_name: row.get(2)?,
                    file_line: row.get(3)?,
                    kind: row.get(4)?,
                    payload: row.get(5)?,
                })
            })
            .map_err(|e| AppError::Io(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AppError::Io(e.to_string()))?);
        }
        Ok(out)
    }

    fn insert_dump(&self, dump: &DumpEntry) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        guard
            .execute(
                "INSERT OR REPLACE INTO dumps (id, timestamp, site_name, file_line, kind, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    dump.id,
                    dump.timestamp,
                    dump.site_name,
                    dump.file_line,
                    dump.kind,
                    dump.payload
                ],
            )
            .map_err(|e| AppError::Io(e.to_string()))?;

        // Ring-buffer pruning: keep at most MAX_STORED_DUMPS latest dump entries.
        let _ = guard.execute(
            "DELETE FROM dumps WHERE id NOT IN (
                 SELECT id FROM dumps ORDER BY timestamp DESC, id DESC LIMIT ?1
             )",
            params![MAX_STORED_DUMPS],
        );
        Ok(())
    }

    fn clear_dumps(&self) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        guard
            .execute("DELETE FROM dumps", [])
            .map_err(|e| AppError::Io(e.to_string()))?;
        Ok(())
    }

    fn list_workspaces(&self, site_id: Option<&str>) -> Result<Vec<Workspace>, AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        let mut stmt = guard
            .prepare_cached(
                "SELECT id, site_id, site_name, branch, path, host, db_name, created_at
                 FROM workspaces ORDER BY created_at DESC",
            )
            .map_err(|e| AppError::Io(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                let p: String = row.get(4)?;
                Ok(Workspace {
                    id: row.get(0)?,
                    site_id: row.get(1)?,
                    site_name: row.get(2)?,
                    branch: row.get(3)?,
                    path: PathBuf::from(p),
                    host: row.get(5)?,
                    db_name: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })
            .map_err(|e| AppError::Io(e.to_string()))?;
        let mut list = Vec::new();
        for r in rows {
            let ws = r.map_err(|e| AppError::Io(e.to_string()))?;
            if let Some(filter_id) = site_id {
                if ws.site_id != filter_id {
                    continue;
                }
            }
            list.push(ws);
        }
        Ok(list)
    }

    fn upsert_workspace(&self, ws: &Workspace) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        guard
            .execute(
                "INSERT OR REPLACE INTO workspaces (
                    id, site_id, site_name, branch, path, host, db_name, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    ws.id,
                    ws.site_id,
                    ws.site_name,
                    ws.branch,
                    ws.path.to_string_lossy().to_string(),
                    ws.host,
                    ws.db_name,
                    ws.created_at
                ],
            )
            .map_err(|e| AppError::Io(e.to_string()))?;
        Ok(())
    }

    fn delete_workspace(&self, id: &str) -> Result<(), AppError> {
        let guard = self.conn.lock().map_err(|_| AppError::StateLock)?;
        guard
            .execute("DELETE FROM workspaces WHERE id = ?1", params![id])
            .map_err(|e| AppError::Io(e.to_string()))?;
        Ok(())
    }
}

type RawSiteTuple = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    String,
    Option<String>,
    Option<String>,
    String,
    bool,
    bool,
    i64,
);

fn map_raw_site_tuple(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawSiteTuple> {
    let id: String = row.get(0)?;
    let name: String = row.get(1)?;
    let path_str: String = row.get(2)?;
    let doc_root_str: Option<String> = row.get(3)?;
    let php_version: Option<String> = row.get(4)?;
    let node_version: Option<String> = row.get(5)?;
    let https_int: i32 = row.get(6)?;
    let fw_json: String = row.get(7)?;
    let db_name: Option<String> = row.get(8)?;
    let proxy_target: Option<String> = row.get(9)?;
    let workers_json: String = row.get(10)?;
    let opcache_int: i32 = row.get(11)?;
    let xdebug_int: i32 = row.get(12)?;
    let created_at: i64 = row.get(13)?;
    Ok((
        id,
        name,
        path_str,
        doc_root_str,
        php_version,
        node_version,
        https_int != 0,
        fw_json,
        db_name,
        proxy_target,
        workers_json,
        opcache_int != 0,
        xdebug_int != 0,
        created_at,
    ))
}

fn load_site_hostnames(
    conn: &Connection,
    site_id: &str,
    site_name: &str,
) -> Result<Vec<String>, AppError> {
    let mut host_stmt = conn
        .prepare_cached("SELECT host FROM hosts WHERE site_id = ?1 ORDER BY host ASC")
        .map_err(|e| AppError::Io(e.to_string()))?;
    let host_rows = host_stmt
        .query_map(params![site_id], |r| r.get::<_, String>(0))
        .map_err(|e| AppError::Io(e.to_string()))?;
    let mut hostnames = Vec::new();
    for h in host_rows {
        hostnames.push(h.map_err(|e| AppError::Io(e.to_string()))?);
    }
    if hostnames.is_empty() {
        hostnames.push(format!("{site_name}.test"));
    }
    Ok(hostnames)
}

fn build_site_from_raw(raw: RawSiteTuple, hostnames: Vec<String>) -> Site {
    let (
        id,
        name,
        path_str,
        doc_root_str,
        php_version,
        node_version,
        https,
        fw_json,
        db_name,
        proxy_target,
        workers_json,
        opcache_enabled,
        xdebug_enabled,
        created_at,
    ) = raw;
    let framework: Framework = serde_json::from_str(&fw_json).unwrap_or(Framework::PlainPhp);
    let workers: Vec<WorkerKind> = serde_json::from_str(&workers_json).unwrap_or_default();

    Site {
        id,
        name,
        path: PathBuf::from(path_str),
        document_root: doc_root_str.map(PathBuf::from),
        hostnames,
        php_version,
        node_version,
        https,
        isolated_services: Vec::new(),
        workers,
        framework,
        db_name,
        proxy_target,
        opcache_enabled,
        xdebug_enabled,
        created_at,
    }
}

fn map_mail_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MailMessage> {
    let id: String = row.get(0)?;
    let received_at: i64 = row.get(1)?;
    let envelope_from: String = row.get(2)?;
    let recipients_json: String = row.get(3)?;
    let subject: String = row.get(4)?;
    let preview: String = row.get(5)?;
    let html_body: String = row.get(6)?;
    let text_body: String = row.get(7)?;
    let raw_headers: String = row.get(8)?;
    let attachments_json: String = row.get(9)?;
    let read_int: i32 = row.get(10)?;
    let recipients: Vec<String> = serde_json::from_str(&recipients_json).unwrap_or_default();
    let attachments: Vec<MailAttachment> =
        serde_json::from_str(&attachments_json).unwrap_or_default();
    Ok(MailMessage {
        id,
        received_at,
        envelope_from,
        recipients,
        subject,
        preview,
        html_body,
        text_body,
        raw_headers,
        attachments,
        read: read_int != 0,
    })
}
