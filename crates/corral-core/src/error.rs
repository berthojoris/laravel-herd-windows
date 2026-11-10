use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("kesalahan I/O: {0}")]
    Io(String),
    #[error("input tidak valid: {0}")]
    Validation(String),
    #[error("situs tidak ditemukan: {0}")]
    SiteNotFound(String),
    #[error("port {port} dipakai proses {process} (PID {pid})")]
    PortInUse {
        port: u16,
        pid: u32,
        process: String,
    },
    #[error("konfigurasi nginx tidak valid: {0}")]
    NginxConfig(String),
    #[error("helper belum terpasang")]
    HelperMissing,
    #[error("helper menolak permintaan: {0}")]
    HelperRejected(String),
    #[error("komponen {component} {version} gagal diunduh: {reason}")]
    Download {
        component: String,
        version: String,
        reason: String,
    },
    #[error("sertifikat kedaluwarsa: {0}")]
    CertExpired(String),
    #[error("state terkunci")]
    StateLock,
    #[error("butuh hak administrator: {0}")]
    ElevationRequired(String),
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        Self::Validation(format!("JSON tidak valid: {err}"))
    }
}

impl From<serde_yaml::Error> for AppError {
    fn from(err: serde_yaml::Error) -> Self {
        Self::Validation(format!("YAML (.corral.yml) tidak valid: {err}"))
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "camelCase")]
pub enum ErrorKind {
    Io(String),
    Validation(String),
    SiteNotFound(String),
    PortInUse(String),
    NginxConfig(String),
    HelperMissing(String),
    HelperRejected(String),
    Download(String),
    CertExpired(String),
    StateLock(String),
    ElevationRequired(String),
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let m = self.to_string();
        let k = match self {
            Self::Io(_) => ErrorKind::Io(m),
            Self::Validation(_) => ErrorKind::Validation(m),
            Self::SiteNotFound(_) => ErrorKind::SiteNotFound(m),
            Self::PortInUse { .. } => ErrorKind::PortInUse(m),
            Self::NginxConfig(_) => ErrorKind::NginxConfig(m),
            Self::HelperMissing => ErrorKind::HelperMissing(m),
            Self::HelperRejected(_) => ErrorKind::HelperRejected(m),
            Self::Download { .. } => ErrorKind::Download(m),
            Self::CertExpired(_) => ErrorKind::CertExpired(m),
            Self::StateLock => ErrorKind::StateLock(m),
            Self::ElevationRequired(_) => ErrorKind::ElevationRequired(m),
        };
        k.serialize(s)
    }
}

pub type CoreError = AppError;
