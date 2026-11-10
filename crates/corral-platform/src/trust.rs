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
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::fs_store::write_atomic;

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

        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, &primary);
        dn.push(DnType::OrganizationName, "Corral Local TLS");
        params.distinguished_name = dn;

        let mut sans = Vec::new();
        for h in hostnames {
            if let Ok(ia5) = h.clone().try_into() {
                sans.push(SanType::DnsName(ia5));
            }
        }
        sans.push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        params.subject_alt_names = sans;

        // CA/B Forum max validity <= 396 days (TLS-1)
        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::days(1);
        params.not_after = now + Duration::days(395);

        let key_pair = KeyPair::generate()
            .map_err(|e| AppError::Io(format!("gagal membuat kunci leaf TLS: {e}")))?;
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| AppError::Io(format!("gagal membuat sertifikat situs {primary}: {e}")))?;

        write_atomic(&leaf_path, cert.pem().as_bytes())?;
        write_atomic(&key_path, key_pair.serialize_pem().as_bytes())?;

        Ok(Certificate {
            host: primary,
            ca_thumbprint: ca_info.ca_thumbprint,
            leaf_path,
            key_path,
            not_after: (now + Duration::days(395)).unix_timestamp(),
            trusted: ca_info.trusted,
        })
    }

    fn install_ca(&self, pem: &Path) -> Result<(), AppError> {
        if !pem.is_file() {
            let _ = self.ensure_ca()?;
        }
        // Best-effort install into CurrentUser Root Certificate Store via certutil
        let _ = std::process::Command::new("certutil")
            .args(["-user", "-f", "-addstore", "Root", &pem.to_string_lossy()])
            .creation_flags(CREATE_NO_WINDOW)
            .status();

        let marker = self.ca_dir.join(".trusted");
        let _ = std::fs::write(marker, b"1");
        if let Ok(mut guard) = self.trusted_state.lock() {
            *guard = true;
        }
        Ok(())
    }

    fn remove_ca(&self, _thumbprint: &str) -> Result<(), AppError> {
        let _ = std::process::Command::new("certutil")
            .args([
                "-user",
                "-delstore",
                "Root",
                "Corral Local Development CA",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .status();

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
