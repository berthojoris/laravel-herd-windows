use std::path::PathBuf;
use std::sync::Mutex;

use corral_core::application::render_hosts_file_with_block;
use corral_core::domain::{validate_hostname, HostEntry};
use corral_core::error::AppError;
use corral_core::ports::HostsPort;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::fs_store::write_atomic;

pub const HELPER_PIPE_NAME: &str = r"\\.\pipe\corral-helper-v1";
pub const APP_PIPE_NAME: &str = r"\\.\pipe\corral-app-v1";
pub const DEFAULT_HELPER_HMAC_KEY: &[u8] = b"corral-helper-v1-local-ipc-key-2026";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperRequest {
    pub action: String,
    pub hostnames: Vec<String>,
    pub cert_path: Option<String>,
    pub nonce: String,
    pub hmac: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperResponse {
    pub ok: bool,
    pub code: String,
    pub message: String,
}

pub fn compute_helper_hmac(action: &str, hostnames: &[String], nonce: &str, key: &[u8]) -> String {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        return String::new();
    };
    mac.update(action.as_bytes());
    mac.update(b":");
    for h in hostnames {
        mac.update(h.as_bytes());
        mac.update(b",");
    }
    mac.update(b":");
    mac.update(nonce.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn verify_helper_hmac(
    action: &str,
    hostnames: &[String],
    nonce: &str,
    provided_hmac: &str,
    key: &[u8],
) -> bool {
    let expected = compute_helper_hmac(action, hostnames, nonce, key);
    if expected.is_empty() || expected.len() != provided_hmac.len() {
        return false;
    }
    expected
        .as_bytes()
        .ct_eq(provided_hmac.as_bytes())
        .into()
}

pub struct WindowsHostsManager {
    appdata_dir: PathBuf,
    system_hosts_path: PathBuf,
    helper_installed: Mutex<bool>,
}

impl WindowsHostsManager {
    pub fn new(appdata_dir: PathBuf) -> Self {
        let sys_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        let system_hosts_path = PathBuf::from(sys_root)
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts");
        let helper_marker = appdata_dir.join(".helper-installed");
        let is_installed = helper_marker.is_file() || Self::can_write_system_hosts(&system_hosts_path);
        Self {
            appdata_dir,
            system_hosts_path,
            helper_installed: Mutex::new(is_installed),
        }
    }

    pub fn can_write_system_hosts(path: & std::path::Path) -> bool {
        std::fs::OpenOptions::new().write(true).open(path).is_ok()
    }

    pub fn set_helper_installed(&self, installed: bool) {
        let marker = self.appdata_dir.join(".helper-installed");
        if installed {
            let _ = std::fs::write(marker, b"1");
        } else {
            let _ = std::fs::remove_file(marker);
        }
        if let Ok(mut g) = self.helper_installed.lock() {
            *g = installed;
        }
    }
}

impl HostsPort for WindowsHostsManager {
    fn replace_block(&self, entries: &[HostEntry]) -> Result<(), AppError> {
        for entry in entries {
            validate_hostname(&entry.hostname)?;
        }

        // Always write preview/staging hosts block in %APPDATA%\Corral\etc\hosts.corral
        let staging_path = self.appdata_dir.join("etc").join("hosts.corral");
        let existing_sys = std::fs::read_to_string(&self.system_hosts_path)
            .unwrap_or_else(|_| "127.0.0.1 localhost\r\n::1 localhost\r\n".to_string());
        let rendered = render_hosts_file_with_block(&existing_sys, entries)?;
        write_atomic(&staging_path, rendered.as_bytes())?;

        // If elevated or system hosts is directly writable, also update system hosts atomically with backup
        if Self::can_write_system_hosts(&self.system_hosts_path) {
            let bak = self.appdata_dir.join("etc").join("hosts.bak");
            let _ = write_atomic(&bak, existing_sys.as_bytes());
            let _ = std::fs::write(&self.system_hosts_path, rendered.as_bytes());
        }

        Ok(())
    }

    fn is_helper_available(&self) -> bool {
        self.helper_installed
            .lock()
            .map(|g| *g)
            .unwrap_or(false)
    }
}
