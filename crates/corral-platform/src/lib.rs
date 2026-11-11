pub mod dumps;
pub mod fs_store;
pub mod hosts;
pub mod ini;
pub mod io_util;
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
    use std::io::Cursor;
    use std::sync::Arc;

    use corral_core::ports::{RepositoryPort, TrustStorePort};

    use crate::hosts::{compute_helper_hmac, verify_helper_hmac, DEFAULT_HELPER_HMAC_KEY};
    use crate::ini::PhpRuntimeManager;
    use crate::io_util::{read_bounded_sync_line, read_tail_lines, BoundedLine};
    use crate::job::JobHandle;
    use crate::process::ServiceSupervisor;
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
    fn test_sqlite_repository_roundtrip_and_summaries() {
        let tmp = std::env::temp_dir().join(format!("corral-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let db_file = tmp.join("corral.db");
        let herd_dir = tmp.join("Herd");
        std::fs::create_dir_all(&herd_dir).unwrap();

        let repo = SqliteRepository::open(&db_file, &herd_dir).unwrap();
        let sites = repo.list_sites().unwrap();
        assert!(sites.len() >= 2);
        assert!(repo.get_site("laravel-commerce").unwrap().is_some());
        let services = repo.list_services().unwrap();
        assert!(services.len() >= 6);
        let summaries = repo.list_mail_summaries().unwrap();
        assert!(!summaries.is_empty());
        let (total, unread) = repo.count_mails().unwrap();
        assert_eq!(total, summaries.len());
        assert_eq!(unread, summaries.len());
        let first_id = summaries.first().unwrap().id.clone();
        assert!(repo.get_mail(&first_id).unwrap().is_some());

        // Verify blocking Artisan daemon subcommands are rejected cleanly.
        let repo_arc: Arc<dyn RepositoryPort> = Arc::new(repo);
        let supervisor = ServiceSupervisor::new(repo_arc).unwrap();
        let err = supervisor
            .run_artisan(&herd_dir, None, "queue:work --tries=3")
            .unwrap_err();
        assert!(err.to_string().contains("daemon"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_php_ini_manager_and_tls_ca_generation_and_cert_cache() {
        let tmp = std::env::temp_dir().join(format!("corral-ini-tls-{}", uuid::Uuid::new_v4()));
        let php_mgr = PhpRuntimeManager::new(tmp.join("php"));
        let ini = php_mgr.read_ini("8.4").unwrap();
        assert_eq!(ini.realpath_cache_size, "4096K");
        assert!(ini.opcache_enable);

        let tls_mgr = TlsCertificateManager::new(tmp.join("ca"), tmp.join("ca/sites"));
        let ca = tls_mgr.ensure_ca().unwrap();
        assert!(!ca.ca_thumbprint.is_empty());
        let hosts = vec!["shop.test".to_string(), "www.shop.test".to_string()];
        let leaf = tls_mgr.issue_site_cert(&hosts).unwrap();
        assert!(leaf.leaf_path.is_file());
        let first_pem = std::fs::read_to_string(&leaf.leaf_path).unwrap();

        // Second call with identical SANs reuses the existing certificate without regenerating keys.
        let leaf_cached = tls_mgr.issue_site_cert(&hosts).unwrap();
        let second_pem = std::fs::read_to_string(&leaf_cached.leaf_path).unwrap();
        assert_eq!(first_pem, second_pem);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_bounded_io_utilities() {
        let tmp = std::env::temp_dir().join(format!("corral-io-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let log_file = tmp.join("big.log");

        let mut content = String::new();
        for i in 1..=1000 {
            content.push_str(&format!("line-{i:04}-payload-0123456789\n"));
        }
        std::fs::write(&log_file, content).unwrap();

        // Seek only the last 4 KB and extract at most 10 lines.
        let tail = read_tail_lines(&log_file, 4096, 10).unwrap();
        assert_eq!(tail.len(), 10);
        assert_eq!(tail.last().unwrap(), "line-1000-payload-0123456789");

        // Sync bounded line reader handles normal, oversized, and EOF lines safely.
        let raw_stream = b"short-line\r\n0123456789ABCDEF_TOO_LONG\nnext-ok\n";
        let mut cursor = Cursor::new(&raw_stream[..]);
        assert_eq!(
            read_bounded_sync_line(&mut cursor, 16).unwrap(),
            BoundedLine::Line("short-line".to_string())
        );
        assert_eq!(
            read_bounded_sync_line(&mut cursor, 16).unwrap(),
            BoundedLine::Oversized("0123456789ABCDEF".to_string())
        );
        assert_eq!(
            read_bounded_sync_line(&mut cursor, 16).unwrap(),
            BoundedLine::Line("next-ok".to_string())
        );
        assert_eq!(
            read_bounded_sync_line(&mut cursor, 16).unwrap(),
            BoundedLine::Eof
        );

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
