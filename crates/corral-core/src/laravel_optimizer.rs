use std::collections::BTreeMap;

use crate::domain::{Framework, Site};
use crate::dto::{LaravelHealthReport, LaravelOptimizationItem, PhpIniSettingsDto};

/// Parse a `.env` file into key-value pairs for inspection.
pub fn parse_dotenv_map(content: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim().to_string();
            let val = v
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !key.is_empty() {
                map.insert(key, val);
            }
        }
    }
    map
}

/// Non-destructively update or insert keys in a `.env` file while preserving comments and blank lines (MAIL-2 / WS-2).
pub fn apply_dotenv_updates(existing: &str, updates: &BTreeMap<String, String>) -> String {
    let mut seen = BTreeMap::new();
    let mut out_lines: Vec<String> = Vec::new();

    for raw_line in existing.lines() {
        let trimmed = raw_line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            if let Some((k, _)) = trimmed.split_once('=') {
                let key = k.trim();
                if let Some(new_val) = updates.get(key) {
                    let formatted = format_env_value(new_val);
                    out_lines.push(format!("{key}={formatted}"));
                    seen.insert(key.to_string(), true);
                    continue;
                }
            }
        }
        out_lines.push(raw_line.to_string());
    }

    let missing: Vec<(&String, &String)> = updates
        .iter()
        .filter(|(k, _)| !seen.contains_key(k.as_str()))
        .collect();

    if !missing.is_empty() {
        if !out_lines.is_empty() && !out_lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            out_lines.push(String::new());
        }
        out_lines.push("# Managed by Corral Laravel App Server".to_string());
        for (k, v) in missing {
            out_lines.push(format!("{k}={}", format_env_value(v)));
        }
    }

    let mut joined = out_lines.join("\n");
    if !joined.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

fn format_env_value(val: &str) -> String {
    if val.contains(' ') || val.contains('#') {
        format!("\"{val}\"")
    } else {
        val.to_string()
    }
}

/// Build the recommended `.env` key-value pairs for a Laravel site managed by Corral.
pub fn recommended_laravel_env(
    site: &Site,
    http_port: u16,
    https_port: u16,
    smtp_port: u16,
    dumps_port: u16,
) -> BTreeMap<String, String> {
    let mut updates = BTreeMap::new();
    let scheme = if site.https { "https" } else { "http" };
    let port_part = if site.https && https_port != 443 {
        format!(":{https_port}")
    } else if !site.https && http_port != 80 {
        format!(":{http_port}")
    } else {
        String::new()
    };
    let app_url = format!("{scheme}://{}{port_part}", site.primary_host());
    let db_name = site
        .db_name
        .clone()
        .unwrap_or_else(|| site.name.replace('-', "_"));

    updates.insert("APP_URL".to_string(), app_url);
    updates.insert("DB_CONNECTION".to_string(), "mysql".to_string());
    updates.insert("DB_HOST".to_string(), "127.0.0.1".to_string());
    updates.insert("DB_PORT".to_string(), "3306".to_string());
    updates.insert("DB_DATABASE".to_string(), db_name);
    updates.insert("DB_USERNAME".to_string(), "root".to_string());
    updates.insert("DB_PASSWORD".to_string(), String::new());
    updates.insert("MAIL_MAILER".to_string(), "smtp".to_string());
    updates.insert("MAIL_HOST".to_string(), "127.0.0.1".to_string());
    updates.insert("MAIL_PORT".to_string(), smtp_port.to_string());
    updates.insert("MAIL_USERNAME".to_string(), "null".to_string());
    updates.insert("MAIL_PASSWORD".to_string(), "null".to_string());
    updates.insert("MAIL_ENCRYPTION".to_string(), "null".to_string());
    updates.insert(
        "MAIL_FROM_ADDRESS".to_string(),
        format!("hello@{}", site.primary_host()),
    );
    updates.insert("REDIS_HOST".to_string(), "127.0.0.1".to_string());
    updates.insert("REDIS_PORT".to_string(), "6379".to_string());
    updates.insert("VAR_DUMPER_FORMAT".to_string(), "server".to_string());
    updates.insert(
        "VAR_DUMPER_SERVER".to_string(),
        format!("127.0.0.1:{dumps_port}"),
    );
    updates
}

/// Generate optimal `php.ini` settings tailored for high-speed Laravel execution on Windows x64.
pub fn recommended_laravel_php_ini(version: &str, ini_path: &str) -> PhpIniSettingsDto {
    PhpIniSettingsDto {
        version: version.to_string(),
        memory_limit: "512M".to_string(),
        upload_max_filesize: "128M".to_string(),
        post_max_size: "128M".to_string(),
        max_execution_time: "120".to_string(),
        max_input_vars: "5000".to_string(),
        realpath_cache_size: "4096K".to_string(),
        realpath_cache_ttl: "600".to_string(),
        opcache_enable: true,
        opcache_memory_consumption: "256".to_string(),
        opcache_max_accelerated_files: "20000".to_string(),
        opcache_jit: "tracing".to_string(),
        opcache_jit_buffer_size: "128M".to_string(),
        xdebug_mode: "off".to_string(),
        xdebug_client_port: 9003,
        raw_path: ini_path.to_string(),
    }
}

/// Evaluate a site's Laravel health & performance posture on Windows x64.
pub fn evaluate_laravel_health(
    site: &Site,
    ini: &PhpIniSettingsDto,
    dotenv_content: Option<&str>,
    storage_linked: bool,
) -> LaravelHealthReport {
    let laravel_ver = match &site.framework {
        Framework::Laravel(v) if !v.is_empty() => v.clone(),
        Framework::Laravel(_) => "11.x".to_string(),
        other => other.label().to_string(),
    };

    let env_map = dotenv_content.map(parse_dotenv_map).unwrap_or_default();
    let expected_scheme = if site.https { "https://" } else { "http://" };
    let app_url_ok = env_map
        .get("APP_URL")
        .map(|u| u.starts_with(expected_scheme) && u.contains(&site.primary_host()))
        .unwrap_or(false);
    let mail_ok = env_map
        .get("MAIL_PORT")
        .map(|p| p == "1025")
        .unwrap_or(false);

    let realpath_ok = ini.realpath_cache_size.eq_ignore_ascii_case("4096K")
        || ini.realpath_cache_size.eq_ignore_ascii_case("8192K");
    let opcache_ok = ini.opcache_enable && site.opcache_enabled;
    let jit_ok = !ini.opcache_jit.is_empty() && ini.opcache_jit != "off" && ini.opcache_jit != "0";
    let xdebug_off_for_speed = !site.xdebug_enabled && ini.xdebug_mode == "off";

    let items = vec![
        LaravelOptimizationItem {
            key: "opcache.enable".to_string(),
            label: "Zend OPcache Bytecode Accelerator".to_string(),
            category: "PHP Runtime".to_string(),
            current_value: if opcache_ok {
                format!(
                    "Enabled ({} MB, {} files)",
                    ini.opcache_memory_consumption, ini.opcache_max_accelerated_files
                )
            } else {
                "Disabled".to_string()
            },
            recommended_value: "Enabled (256 MB, 20000 files)".to_string(),
            optimized: opcache_ok,
            impact: "Memangkas waktu boot container & autoload Composer hingga 70% di Windows"
                .to_string(),
        },
        LaravelOptimizationItem {
            key: "opcache.jit".to_string(),
            label: "PHP 8 Tracing JIT Compiler".to_string(),
            category: "PHP Runtime".to_string(),
            current_value: format!("{} ({})", ini.opcache_jit, ini.opcache_jit_buffer_size),
            recommended_value: "tracing (128M)".to_string(),
            optimized: jit_ok,
            impact: "Akselerasi CPU untuk koleksi Eloquent, Blade rendering, dan serialisasi JSON"
                .to_string(),
        },
        LaravelOptimizationItem {
            key: "realpath_cache_size".to_string(),
            label: "Windows NTFS Realpath Stat Cache".to_string(),
            category: "Filesystem".to_string(),
            current_value: format!(
                "{} (TTL {}s)",
                ini.realpath_cache_size, ini.realpath_cache_ttl
            ),
            recommended_value: "4096K (TTL 600s)".to_string(),
            optimized: realpath_ok,
            impact:
                "Menghilangkan bottleneck I/O NTFS saat memuat ribuan class di folder vendor/"
                    .to_string(),
        },
        LaravelOptimizationItem {
            key: "xdebug.mode".to_string(),
            label: "Zero-Overhead Xdebug Mode".to_string(),
            category: "PHP Runtime".to_string(),
            current_value: if xdebug_off_for_speed {
                "off (On-Demand)".to_string()
            } else {
                "debug (Active)".to_string()
            },
            recommended_value: "off (Aktifkan hanya saat debugging)".to_string(),
            optimized: xdebug_off_for_speed,
            impact: "Xdebug aktif memperlambat request Laravel 3x-5x; matikan saat tidak dipakai"
                .to_string(),
        },
        LaravelOptimizationItem {
            key: "env.app_url".to_string(),
            label: "Sinkronisasi .env (APP_URL, DB & Mail Catcher)".to_string(),
            category: "Laravel Config".to_string(),
            current_value: if app_url_ok && mail_ok {
                "Tersinkronisasi dengan Corral".to_string()
            } else {
                "Perlu sinkronisasi .env".to_string()
            },
            recommended_value: format!("{expected_scheme}{} + SMTP :1025", site.primary_host()),
            optimized: app_url_ok && mail_ok,
            impact: "Memastikan asset Vite, Sanctum cookie, dan Mail::send() langsung bekerja"
                .to_string(),
        },
    ];

    let passed = items.iter().filter(|i| i.optimized).count();
    let total = items.len().max(1);
    let score = u8::try_from((passed * 100) / total).unwrap_or(100);

    LaravelHealthReport {
        site_id: site.id.clone(),
        site_name: site.name.clone(),
        is_laravel: site.framework.is_laravel(),
        laravel_version: laravel_ver,
        score,
        opcache_turbo: opcache_ok && jit_ok,
        realpath_cache_tuned: realpath_ok,
        fastcgi_pool_children: 8,
        env_configured: app_url_ok && mail_ok,
        storage_linked,
        items,
        artisan_commands: vec![
            "optimize:clear".to_string(),
            "migrate".to_string(),
            "migrate:fresh --seed".to_string(),
            "storage:link".to_string(),
            "route:list".to_string(),
            "about".to_string(),
            "queue:restart".to_string(),
        ],
    }
}
