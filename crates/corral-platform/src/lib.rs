pub mod dumps;
pub mod fs_store;
pub mod hosts;
pub mod ini;
pub mod job;
pub mod nginx;
pub mod port;
pub mod process;
pub mod smtp;
pub mod sqlite;
pub mod trust;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use corral_core::ports::{RepositoryPort, TrustStorePort};

    use crate::hosts::{compute_helper_hmac, verify_helper_hmac, DEFAULT_HELPER_HMAC_KEY};
    use crate::ini::PhpRuntimeManager;
    use crate::job::JobHandle;
    use crate::smtp::parse_raw_smtp_payload;
    use crate::sqlite::SqliteRepository;
    use crate::trust::TlsCertificateManager;

    #[test]
    fn test_windows_job_object_creation() {
        let job = JobHandle::kill_on_close().unwrap();
        job.terminate_all();
    }

    #[test]
    fn test_helper_hmac_sign_and_verify() {
        let hosts = vec!["my-app.test".to_string(), "www.my-app.test".to_string()];
        let nonce = "nonce-12345";
        let sig = compute_helper_hmac("updateHosts", &hosts, nonce, DEFAULT_HELPER_HMAC_KEY);
        assert!(verify_helper_hmac(
            "updateHosts",
            &hosts,
            nonce,
            &sig,
            DEFAULT_HELPER_HMAC_KEY
        ));
        assert!(!verify_helper_hmac(
            "updateHosts",
            &hosts,
            "wrong-nonce",
            &sig,
            DEFAULT_HELPER_HMAC_KEY
        ));
    }

    #[test]
    fn test_sqlite_repository_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("corral-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let db_file = tmp.join("corral.db");
        let herd_dir = tmp.join("Herd");
        std::fs::create_dir_all(&herd_dir).unwrap();

        let repo = SqliteRepository::open(&db_file, &herd_dir).unwrap();
        let sites = repo.list_sites().unwrap();
        assert!(sites.len() >= 2);
        let services = repo.list_services().unwrap();
        assert!(services.len() >= 6);
        let mails = repo.list_mails().unwrap();
        assert!(!mails.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_php_ini_manager_and_tls_ca_generation() {
        let tmp = std::env::temp_dir().join(format!("corral-ini-tls-{}", uuid::Uuid::new_v4()));
        let php_mgr = PhpRuntimeManager::new(tmp.join("php"));
        let ini = php_mgr.read_ini("8.4").unwrap();
        assert_eq!(ini.realpath_cache_size, "4096K");
        assert!(ini.opcache_enable);

        let tls_mgr = TlsCertificateManager::new(tmp.join("ca"), tmp.join("ca/sites"));
        let ca = tls_mgr.ensure_ca().unwrap();
        assert!(!ca.ca_thumbprint.is_empty());
        let leaf = tls_mgr
            .issue_site_cert(&["shop.test".to_string(), "www.shop.test".to_string()])
            .unwrap();
        assert!(leaf.leaf_path.is_file());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_smtp_payload_parser() {
        let raw = "From: app@shop.test\r\nSubject: Order #100\r\n\r\n<h1>Thank you</h1><script>evil()</script>";
        let msg = parse_raw_smtp_payload("app@shop.test", &["user@shop.test".to_string()], raw);
        assert_eq!(msg.subject, "Order #100");
        assert!(!msg.html_body.contains("<script>"));
    }
}
