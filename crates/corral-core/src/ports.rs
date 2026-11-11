use std::path::Path;

use crate::domain::{
    Certificate, DumpEntry, HostEntry, MailMessage, ServiceId, ServiceInstance, ServiceKind,
    ServiceState, Site, SiteId, Workspace,
};
use crate::dto::MailSummaryView;
use crate::error::CoreError;

#[derive(Debug, Clone)]
pub struct ServiceSpec {
    pub id: ServiceId,
    pub name: String,
    pub kind: ServiceKind,
    pub version: String,
    pub port: u16,
    pub program: Option<std::path::PathBuf>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub working_dir: Option<std::path::PathBuf>,
    pub site_id: Option<SiteId>,
}

pub trait ProcessSupervisorPort: Send + Sync {
    fn start(&self, spec: ServiceSpec) -> Result<ServiceId, CoreError>;
    fn stop(&self, id: &str) -> Result<(), CoreError>;
    fn restart(&self, id: &str) -> Result<(), CoreError>;
    fn status(&self, id: &str) -> ServiceState;
    fn list(&self) -> Result<Vec<ServiceInstance>, CoreError>;
}

pub trait HostsPort: Send + Sync {
    fn replace_block(&self, entries: &[HostEntry]) -> Result<(), CoreError>;
    fn is_helper_available(&self) -> bool;
}

pub trait TrustStorePort: Send + Sync {
    fn ensure_ca(&self) -> Result<Certificate, CoreError>;
    fn issue_site_cert(&self, hostnames: &[String]) -> Result<Certificate, CoreError>;
    fn install_ca(&self, pem: &Path) -> Result<(), CoreError>;
    fn remove_ca(&self, thumbprint: &str) -> Result<(), CoreError>;
    fn is_trusted(&self, thumbprint: &str) -> Result<bool, CoreError>;
}

pub trait RepositoryPort: Send + Sync {
    fn list_sites(&self) -> Result<Vec<Site>, CoreError>;
    fn get_site(&self, id: &str) -> Result<Option<Site>, CoreError>;
    fn upsert_site(&self, site: &Site) -> Result<(), CoreError>;
    fn delete_site(&self, id: &str) -> Result<(), CoreError>;

    fn list_services(&self) -> Result<Vec<ServiceInstance>, CoreError>;
    fn upsert_service(&self, svc: &ServiceInstance) -> Result<(), CoreError>;
    fn delete_service(&self, id: &str) -> Result<(), CoreError>;

    fn list_mails(&self) -> Result<Vec<MailMessage>, CoreError>;
    fn list_mail_summaries(&self) -> Result<Vec<MailSummaryView>, CoreError>;
    fn count_mails(&self) -> Result<(usize, usize), CoreError>;
    fn get_mail(&self, id: &str) -> Result<Option<MailMessage>, CoreError>;
    fn insert_mail(&self, mail: &MailMessage) -> Result<(), CoreError>;
    fn mark_mail_read(&self, id: &str) -> Result<(), CoreError>;
    fn delete_mail(&self, id: &str) -> Result<(), CoreError>;
    fn clear_mails(&self) -> Result<(), CoreError>;

    fn list_dumps(&self) -> Result<Vec<DumpEntry>, CoreError>;
    fn insert_dump(&self, dump: &DumpEntry) -> Result<(), CoreError>;
    fn clear_dumps(&self) -> Result<(), CoreError>;

    fn list_workspaces(&self, site_id: Option<&str>) -> Result<Vec<Workspace>, CoreError>;
    fn upsert_workspace(&self, ws: &Workspace) -> Result<(), CoreError>;
    fn delete_workspace(&self, id: &str) -> Result<(), CoreError>;
}
