use std::net::{IpAddr, Ipv4Addr};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use corral_core::domain::Certificate;
use corral_core::error::AppError;
use corral_core::ports::TrustStorePort;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, KeyUsagePurpose,
    SanType,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::fs_store::write_atomic;
use crate::io_util::run_command_bounded;

const CERT_RENEW_SAFETY_SECS: i64 = 7 * 86_400;

#[derive(Debug, Serialize, Deserialize)]
struct CachedSiteCertMeta {
    hostnames: Vec<String>,
    ca_thumbprint: String,
    not_after: i64,
}

pub struct TlsCertificateManager {
    ca_dir: PathBuf,
    sites_dir: PathBuf,
    trusted_state: Mutex<bool>,
}

impl TlsCertificateManager {
    pub fn new(ca_dir: PathBuf, sites_dir: PathBuf) -> Self {
        let marker = ca_dir.join(".trusted");
        let is_trusted = marker.is_file();
        Self {
            ca_dir,
            sites_dir,
            trusted_state: Mutex::new(is_trusted),
        }
    }

    pub fn ca_pem_path(&self) -> PathBuf {
        self.ca_dir.join("corral-local-ca.pem")
    }

    pub fn ca_key_path(&self) -> PathBuf {
        self.ca_dir.join("corral-local-ca.key")
    }

    pub fn compute_thumbprint(pem: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(pem.as_bytes());
        let digest = hasher.finalize();
        hex::encode_upper(digest)
    }
}

impl TrustStorePort for TlsCertificateManager {
    fn ensure_ca(&self) -> Result<Certificate, AppError> {
        std::fs::create_dir_all(&self.ca_dir)?;
        std::fs::create_dir_all(&self.sites_dir)?;

        let pem_path = self.ca_pem_path();
        let key_path = self.ca_key_path();

        if pem_path.is_file() && key_path.is_file() {
            let pem = std::fs::read_to_string(&pem_path)?;
            let thumbprint = Self::compute_thumbprint(&pem);
            let trusted = self
                .trusted_state
                .lock()
                .map(|g| *g)
                .unwrap_or(false);
            return Ok(Certificate {
                host: "Corral Local CA".to_string(),
                ca_thumbprint: thumbprint,
                leaf_path: pem_path,
                key_path,
                not_after: OffsetDateTime::now_utc().unix_timestamp() + (365 * 10 * 86400),
                trusted,
            });
        }

        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "Corral Local Development CA");
        dn.push(DnType::OrganizationName, "Corral Local Dev Server");
        params.distinguished_name = dn;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::days(1);
        params.not_after = now + Duration::days(3650);

        let key_pair = KeyPair::generate()
            .map_err(|e| AppError::Io(format!("gagal membuat kunci CA: {e}")))?;
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| AppError::Io(format!("gagal menandatangani CA lokal: {e}")))?;

        let pem = cert.pem();
        let key_pem = key_pair.serialize_pem();
        write_atomic(&pem_path, pem.as_bytes())?;
        write_atomic(&key_path, key_pem.as_bytes())?;

        let thumbprint = Self::compute_thumbprint(&pem);
        Ok(Certificate {
            host: "Corral Local CA".to_string(),
            ca_thumbprint: thumbprint,
            leaf_path: pem_path,
            key_path,
            not_after: (now + Duration::days(3650)).unix_timestamp(),
            trusted: false,
        })
    }

    fn issue_site_cert(&self, hostnames: &[String]) -> Result<Certificate, AppError> {
        let ca_info = self.ensure_ca()?;
        let primary = hostnames
            .iter()
            .find(|h| !h.starts_with("*.") && !h.starts_with("www."))
            .or_else(|| hostnames.iter().find(|h| !h.starts_with("*.")))
            .cloned()
            .unwrap_or_else(|| "localhost.test".to_string())
            .replace('*', "wildcard");

        let leaf_path = self.sites_dir.join(format!("{primary}.pem"));
        let key_path = self.sites_dir.join(format!("{primary}.key"));
        let meta_path = self.sites_dir.join(format!("{primary}.meta.json"));

        let mut sorted_hosts: Vec<String> = hostnames.to_vec();
        sorted_hosts.sort();
        sorted_hosts.dedup();

        let now = OffsetDateTime::now_utc();
        let now_ts = now.unix_timestamp();

        if leaf_path.is_file() && key_path.is_file() && meta_path.is_file() {
            if let Ok(meta_raw) = std::fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<CachedSiteCertMeta>(&meta_raw) {
                    if meta.ca_thumbprint == ca_info.ca_thumbprint
                        && meta.hostnames == sorted_hosts
                        && meta.not_after > now_ts.saturating_add(CERT_RENEW_SAFETY_SECS)
                    {
                        return Ok(Certificate {
                            host: primary,
                            ca_thumbprint: ca_info.ca_thumbprint,
                            leaf_path,
                            key_path,
                            not_after: meta.not_after,
                            trusted: ca_info.trusted,
                        });
                    }
                }
            }
        }

        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, &primary);
        dn.push(DnType::OrganizationName, "Corral Local TLS");
        params.distinguished_name = dn;

        let mut sans = Vec::new();
        for h in &sorted_hosts {
            if let Ok(ia5) = h.clone().try_into() {
                sans.push(SanType::DnsName(ia5));
            }
        }
        sans.push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        params.subject_alt_names = sans;

        // CA/B Forum max validity <= 396 days (TLS-1)
        let not_after_dt = now + Duration::days(395);
        let not_after_ts = not_after_dt.unix_timestamp();
        params.not_before = now - Duration::days(1);
        params.not_after = not_after_dt;

        let key_pair = KeyPair::generate()
            .map_err(|e| AppError::Io(format!("gagal membuat kunci leaf TLS: {e}")))?;
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| AppError::Io(format!("gagal membuat sertifikat situs {primary}: {e}")))?;

        write_atomic(&leaf_path, cert.pem().as_bytes())?;
        write_atomic(&key_path, key_pair.serialize_pem().as_bytes())?;

        let meta = CachedSiteCertMeta {
            hostnames: sorted_hosts,
            ca_thumbprint: ca_info.ca_thumbprint.clone(),
            not_after: not_after_ts,
        };
        if let Ok(meta_bytes) = serde_json::to_vec(&meta) {
            let _ = write_atomic(&meta_path, &meta_bytes);
        }

        Ok(Certificate {
            host: primary,
            ca_thumbprint: ca_info.ca_thumbprint,
            leaf_path,
            key_path,
            not_after: not_after_ts,
            trusted: ca_info.trusted,
        })
    }

    fn install_ca(&self, pem: &Path) -> Result<(), AppError> {
        if !pem.is_file() {
            let _ = self.ensure_ca()?;
        }
        let mut cmd = std::process::Command::new("certutil");
        cmd.args(["-user", "-f", "-addstore", "Root", &pem.to_string_lossy()])
            .creation_flags(CREATE_NO_WINDOW);
        let out = run_command_bounded(
            &mut cmd,
            None,
            64 * 1024,
            std::time::Duration::from_secs(60),
        )?;
        if out.timed_out {
            return Err(AppError::Io(
                "Batas waktu konfirmasi Windows Certificate Store habis (60 detik)".to_string(),
            ));
        }
        if !out.success {
            let detail = if out.stderr.trim().is_empty() {
                out.stdout.trim().to_string()
            } else {
                out.stderr.trim().to_string()
            };
            return Err(AppError::Io(format!(
                "Gagal memasang CA ke Windows Trust Store: {detail}"
            )));
        }

        let marker = self.ca_dir.join(".trusted");
        let _ = std::fs::write(marker, b"1");
        if let Ok(mut guard) = self.trusted_state.lock() {
            *guard = true;
        }
        Ok(())
    }

    fn remove_ca(&self, _thumbprint: &str) -> Result<(), AppError> {
        let mut cmd = std::process::Command::new("certutil");
        cmd.args([
            "-user",
            "-delstore",
            "Root",
            "Corral Local Development CA",
        ])
        .creation_flags(CREATE_NO_WINDOW);
        let _ = run_command_bounded(
            &mut cmd,
            None,
            64 * 1024,
            std::time::Duration::from_secs(15),
        );

        let marker = self.ca_dir.join(".trusted");
        let _ = std::fs::remove_file(marker);
        if let Ok(mut guard) = self.trusted_state.lock() {
            *guard = false;
        }
        Ok(())
    }

    fn is_trusted(&self, _thumbprint: &str) -> Result<bool, AppError> {
        let trusted = self
            .trusted_state
            .lock()
            .map(|g| *g)
            .unwrap_or(false);
        Ok(trusted)
    }
}
