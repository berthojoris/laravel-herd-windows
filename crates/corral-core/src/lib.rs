pub mod application;
pub mod domain;
pub mod dto;
pub mod error;
pub mod laravel_optimizer;
pub mod ports;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;

    use crate::application::{
        render_hosts_file_with_block, sanitize_email_html, WorkerRestartPolicy,
    };
    use crate::domain::{
        build_hostnames, detect_framework_from_indicators, php_fastcgi_port,
        render_nginx_site_conf, sanitize_site_name, validate_hostname, CorralProjectConfig,
        Framework, HostEntry, ServiceState, Site, WorkerKind,
    };
    use crate::laravel_optimizer::{
        apply_dotenv_updates, evaluate_laravel_health, parse_dotenv_map, recommended_laravel_env,
        recommended_laravel_php_ini,
    };

    #[test]
    fn test_sanitize_site_name_and_hostnames() {
        assert_eq!(sanitize_site_name("My_Laravel App!").unwrap(), "my-laravel-app");
        let hosts = build_hostnames("My_Laravel App", "test").unwrap();
        assert_eq!(
            hosts,
            vec![
                "my-laravel-app.test",
                "www.my-laravel-app.test",
                "*.my-laravel-app.test"
            ]
        );
    }

    #[test]
    fn test_validate_hostname_security_rules() {
        assert!(validate_hostname("my-app.test").is_ok());
        assert!(validate_hostname("*.my-app.test").is_ok());
        assert!(validate_hostname("api.shop.local").is_ok());
        // Reject raw IP literal
        assert!(validate_hostname("127.0.0.1").is_err());
        // Reject disallowed TLD
        assert!(validate_hostname("google.com").is_err());
        // Reject illegal chars
        assert!(validate_hostname("bad_host.test").is_err());
        assert!(validate_hostname("-leading.test").is_err());
    }

    #[test]
    fn test_php_fastcgi_port_computation() {
        assert_eq!(php_fastcgi_port(9000, "8.4").unwrap(), 9084);
        assert_eq!(php_fastcgi_port(9000, "8.3.12").unwrap(), 9083);
        assert_eq!(php_fastcgi_port(9000, "7.4").unwrap(), 9074);
        assert!(php_fastcgi_port(9000, "invalid").is_err());
    }

    #[test]
    fn test_detect_framework_laravel() {
        let composer = r#"{"require": {"php": "^8.2", "laravel/framework": "^11.9"}}"#;
        let fw = detect_framework_from_indicators(true, Some(composer), false, true);
        assert_eq!(fw, Framework::Laravel("11.9".to_string()));
    }

    #[test]
    fn test_worker_restart_policy_backoff() {
        let mut policy = WorkerRestartPolicy::default();
        assert_eq!(
            policy.record_failure_and_decide(100),
            (true, ServiceState::Starting)
        );
        assert_eq!(
            policy.record_failure_and_decide(110),
            (true, ServiceState::Starting)
        );
        assert_eq!(
            policy.record_failure_and_decide(120),
            (true, ServiceState::Starting)
        );
        // 4th failure within 60 seconds transitions to Degraded
        assert_eq!(
            policy.record_failure_and_decide(130),
            (false, ServiceState::Degraded)
        );
        // After 60s window expires, restarts are allowed again
        assert_eq!(
            policy.record_failure_and_decide(200),
            (true, ServiceState::Starting)
        );
    }

    #[test]
    fn test_hosts_file_atomic_block_replacement() {
        let original = "127.0.0.1 localhost\r\n# Corral generated Hosts. Do not change.\r\n127.0.0.1 old.test\r\n# End Corral generated Hosts\r\n";
        let entries = vec![HostEntry {
            ip: "127.0.0.1".to_string(),
            hostname: "laravel-shop.test".to_string(),
            comment: None,
        }];
        let rendered = render_hosts_file_with_block(original, &entries).unwrap();
        assert!(rendered.contains("127.0.0.1 localhost"));
        assert!(!rendered.contains("old.test"));
        assert!(rendered.contains("127.0.0.1 laravel-shop.test"));
        assert!(rendered.contains("::1 laravel-shop.test"));
    }

    #[test]
    fn test_laravel_dotenv_and_health_evaluation() {
        let site = Site {
            id: "site-1".to_string(),
            name: "my-app".to_string(),
            path: PathBuf::from("C:\\Users\\dev\\Herd\\my-app"),
            document_root: None,
            hostnames: vec!["my-app.test".to_string()],
            php_version: Some("8.4".to_string()),
            node_version: Some("22".to_string()),
            https: true,
            isolated_services: vec![],
            workers: vec![WorkerKind::Queue],
            framework: Framework::Laravel("11.31".to_string()),
            db_name: Some("my_app".to_string()),
            proxy_target: None,
            opcache_enabled: true,
            xdebug_enabled: false,
            created_at: 0,
        };
        let env_updates = recommended_laravel_env(&site, 80, 443, 1025, 9912);
        let initial_env = "APP_NAME=Laravel\n# Database comment\nDB_HOST=localhost\n";
        let updated_env = apply_dotenv_updates(initial_env, &env_updates);
        assert!(updated_env.contains("# Database comment"));
        let parsed = parse_dotenv_map(&updated_env);
        assert_eq!(
            parsed.get("APP_URL").map(String::as_str),
            Some("https://my-app.test")
        );
        assert_eq!(parsed.get("MAIL_PORT").map(String::as_str), Some("1025"));

        let ini = recommended_laravel_php_ini("8.4", "C:\\Corral\\php\\8.4\\php.ini");
        let report = evaluate_laravel_health(&site, &ini, Some(&updated_env), true);
        assert_eq!(report.score, 100);
        assert!(report.opcache_turbo);
    }

    #[test]
    fn test_nginx_site_conf_generation() {
        let site = Site {
            id: "site-1".to_string(),
            name: "my-app".to_string(),
            path: PathBuf::from("C:\\Users\\dev\\Herd\\my-app"),
            document_root: Some(PathBuf::from("C:\\Users\\dev\\Herd\\my-app\\public")),
            hostnames: vec!["my-app.test".to_string(), "www.my-app.test".to_string()],
            php_version: Some("8.4".to_string()),
            node_version: Some("22".to_string()),
            https: false,
            isolated_services: vec![],
            workers: vec![],
            framework: Framework::Laravel("11.0".to_string()),
            db_name: Some("my_app".to_string()),
            proxy_target: None,
            opcache_enabled: true,
            xdebug_enabled: false,
            created_at: 0,
        };
        let conf = render_nginx_site_conf(&site, 80, 443, 9000, "8.4", None, Some(8080)).unwrap();
        assert!(conf.contains("fastcgi_pass 127.0.0.1:9084;"));
        assert!(conf.contains("fastcgi_buffers 16 32k;"));
        assert!(conf.contains("proxy_pass http://127.0.0.1:8080;"));
    }

    #[test]
    fn test_corral_yml_parsing() {
        let yaml = r#"
php: "8.4"
node: "22"
https: true
workers:
  - queue
  - scheduler
"#;
        let cfg = CorralProjectConfig::parse_yaml(yaml).unwrap();
        assert_eq!(cfg.php.as_deref(), Some("8.4"));
        assert_eq!(cfg.https, Some(true));
        assert_eq!(cfg.workers.len(), 2);
    }

    #[test]
    fn test_sanitize_email_html() {
        let dirty = "<html><body><h1>Hi</h1><script>alert(1)</script><img src=x onerror=alert(2)></body></html>";
        let clean = sanitize_email_html(dirty);
        assert!(!clean.contains("<script>"));
        assert!(!clean.contains("onerror="));
    }
}
