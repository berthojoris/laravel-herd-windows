use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use corral_core::dto::{PhpExtensionView, PhpIniSettingsDto};
use corral_core::error::AppError;
use corral_core::laravel_optimizer::recommended_laravel_php_ini;

use crate::fs_store::write_atomic;

pub struct PhpRuntimeManager {
    php_runtimes_dir: PathBuf,
}

impl PhpRuntimeManager {
    pub fn new(php_runtimes_dir: PathBuf) -> Self {
        Self { php_runtimes_dir }
    }

    pub fn version_dir(&self, version: &str) -> PathBuf {
        self.php_runtimes_dir.join(version)
    }

    pub fn ini_path(&self, version: &str) -> PathBuf {
        self.version_dir(version).join("php.ini")
    }

    pub fn ensure_version_provisioned(&self, version: &str) -> Result<PathBuf, AppError> {
        let dir = self.version_dir(version);
        std::fs::create_dir_all(&dir)?;
        std::fs::create_dir_all(dir.join("ext"))?;
        let ini_file = dir.join("php.ini");
        if !ini_file.exists() {
            let defaults = recommended_laravel_php_ini(version, &ini_file.to_string_lossy());
            self.write_ini(&defaults)?;
        }
        Ok(dir)
    }

    pub fn read_ini(&self, version: &str) -> Result<PhpIniSettingsDto, AppError> {
        let _ = self.ensure_version_provisioned(version)?;
        let path = self.ini_path(version);
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        let map = parse_ini_key_values(&raw);
        let mut dto = recommended_laravel_php_ini(version, &path.to_string_lossy());

        if let Some(v) = map.get("memory_limit") {
            dto.memory_limit = v.clone();
        }
        if let Some(v) = map.get("upload_max_filesize") {
            dto.upload_max_filesize = v.clone();
        }
        if let Some(v) = map.get("post_max_size") {
            dto.post_max_size = v.clone();
        }
        if let Some(v) = map.get("max_execution_time") {
            dto.max_execution_time = v.clone();
        }
        if let Some(v) = map.get("max_input_vars") {
            dto.max_input_vars = v.clone();
        }
        if let Some(v) = map.get("realpath_cache_size") {
            dto.realpath_cache_size = v.clone();
        }
        if let Some(v) = map.get("realpath_cache_ttl") {
            dto.realpath_cache_ttl = v.clone();
        }
        if let Some(v) = map.get("opcache.enable") {
            dto.opcache_enable = v == "1" || v.eq_ignore_ascii_case("On");
        }
        if let Some(v) = map.get("opcache.memory_consumption") {
            dto.opcache_memory_consumption = v.clone();
        }
        if let Some(v) = map.get("opcache.max_accelerated_files") {
            dto.opcache_max_accelerated_files = v.clone();
        }
        if let Some(v) = map.get("opcache.jit") {
            dto.opcache_jit = v.clone();
        }
        if let Some(v) = map.get("opcache.jit_buffer_size") {
            dto.opcache_jit_buffer_size = v.clone();
        }
        if let Some(v) = map.get("xdebug.mode") {
            dto.xdebug_mode = v.clone();
        }
        if let Some(v) = map.get("xdebug.client_port") {
            if let Ok(p) = v.parse::<u16>() {
                dto.xdebug_client_port = p;
            }
        }

        Ok(dto)
    }

    pub fn write_ini(&self, dto: &PhpIniSettingsDto) -> Result<(), AppError> {
        let dir = self.version_dir(&dto.version);
        std::fs::create_dir_all(&dir)?;
        let path = self.ini_path(&dto.version);
        let opcache_flag = if dto.opcache_enable { "1" } else { "0" };

        let content = format!(
            "; =====================================================================\n\
             ; Corral Managed php.ini — PHP {ver} (Windows x64 FastCGI)\n\
             ; Optimized for Laravel Application Server\n\
             ; =====================================================================\n\
             [PHP]\n\
             engine = On\n\
             short_open_tag = Off\n\
             memory_limit = {mem}\n\
             upload_max_filesize = {upload}\n\
             post_max_size = {post}\n\
             max_execution_time = {exec}\n\
             max_input_vars = {vars}\n\
             realpath_cache_size = {rp_size}\n\
             realpath_cache_ttl = {rp_ttl}\n\
             cgi.fix_pathinfo = 1\n\
             extension_dir = \"ext\"\n\n\
             ; Core Laravel Extensions\n\
             extension=curl\n\
             extension=fileinfo\n\
             extension=gd\n\
             extension=intl\n\
             extension=mbstring\n\
             extension=openssl\n\
             extension=pdo_mysql\n\
             extension=pdo_pgsql\n\
             extension=pdo_sqlite\n\
             extension=redis\n\
             extension=zip\n\
             zend_extension=opcache\n\n\
             [opcache]\n\
             opcache.enable = {opcache_flag}\n\
             opcache.enable_cli = 1\n\
             opcache.memory_consumption = {op_mem}\n\
             opcache.interned_strings_buffer = 32\n\
             opcache.max_accelerated_files = {op_files}\n\
             opcache.validate_timestamps = 1\n\
             opcache.revalidate_freq = 0\n\
             opcache.jit = {jit}\n\
             opcache.jit_buffer_size = {jit_buf}\n\n\
             [xdebug]\n\
             xdebug.mode = {xdebug_mode}\n\
             xdebug.client_host = 127.0.0.1\n\
             xdebug.client_port = {xdebug_port}\n\
             xdebug.start_with_request = trigger\n",
            ver = dto.version,
            mem = dto.memory_limit,
            upload = dto.upload_max_filesize,
            post = dto.post_max_size,
            exec = dto.max_execution_time,
            vars = dto.max_input_vars,
            rp_size = dto.realpath_cache_size,
            rp_ttl = dto.realpath_cache_ttl,
            op_mem = dto.opcache_memory_consumption,
            op_files = dto.opcache_max_accelerated_files,
            jit = dto.opcache_jit,
            jit_buf = dto.opcache_jit_buffer_size,
            xdebug_mode = dto.xdebug_mode,
            xdebug_port = dto.xdebug_client_port,
        );

        write_atomic(&path, content.as_bytes())
    }

    pub fn list_extensions(&self, version: &str) -> Result<Vec<PhpExtensionView>, AppError> {
        let ini = self.read_ini(version)?;
        let raw = std::fs::read_to_string(Path::new(&ini.raw_path)).unwrap_or_default();

        let catalog = [
            ("opcache", "Zend OPcache + Tracing JIT Compiler (Laravel Accelerator)"),
            ("redis", "PhpRedis C-extension untuk Laravel Cache, Session & Queue"),
            ("pdo_mysql", "PDO Driver untuk MySQL / MariaDB"),
            ("pdo_pgsql", "PDO Driver untuk PostgreSQL"),
            ("mbstring", "Multibyte String Functions (Wajib untuk Laravel)"),
            ("openssl", "OpenSSL Cryptography & HTTPS Client"),
            ("intl", "Internationalization & Localization (Filament/Number helper)"),
            ("gd", "GD Image Processing Library"),
            ("imagick", "ImageMagick Image Processing Extension"),
            ("mongodb", "MongoDB Driver untuk PHP"),
            ("xdebug", "Xdebug Step Debugger & Profiler (Port 9003)"),
        ];

        let mut out = Vec::with_capacity(catalog.len());
        for (name, desc) in catalog {
            let enabled = if name == "opcache" {
                ini.opcache_enable
            } else if name == "xdebug" {
                ini.xdebug_mode != "off"
            } else {
                raw.lines().any(|l| {
                    let t = l.trim();
                    !t.starts_with(';') && t.contains(&format!("extension={name}"))
                })
            };
            out.push(PhpExtensionView {
                name: name.to_string(),
                enabled,
                available: true,
                description: desc.to_string(),
            });
        }
        Ok(out)
    }
}

pub fn parse_ini_key_values(raw: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with('[') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            let key = k.trim().to_string();
            let val = v.trim().trim_matches('"').to_string();
            map.insert(key, val);
        }
    }
    map
}
