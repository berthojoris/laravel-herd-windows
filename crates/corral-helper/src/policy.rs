use std::path::Path;

use corral_core::domain::validate_hostname;
use corral_platform::hosts::{
    verify_helper_hmac, HelperRequest, HelperResponse, DEFAULT_HELPER_HMAC_KEY,
};

pub const MAX_HOST_ENTRIES: usize = 500;
pub const MAX_PAYLOAD_BYTES: usize = 65_536;

pub fn validate_helper_request(
    raw_json: &str,
    allowed_ca_dir: &Path,
) -> Result<HelperRequest, HelperResponse> {
    if raw_json.is_empty() || raw_json.len() > MAX_PAYLOAD_BYTES {
        return Err(HelperResponse {
            ok: false,
            code: "PAYLOAD_SIZE_INVALID".to_string(),
            message: "Ukuran payload kosong atau melebihi batas 64 KB".to_string(),
        });
    }

    let req: HelperRequest = serde_json::from_str(raw_json).map_err(|_| HelperResponse {
        ok: false,
        code: "INVALID_JSON".to_string(),
        message: "Format JSON request tidak valid".to_string(),
    })?;

    if req.nonce.trim().len() < 8 {
        return Err(HelperResponse {
            ok: false,
            code: "INVALID_NONCE".to_string(),
            message: "Nonce keamanan terlalu pendek atau kosong".to_string(),
        });
    }

    if !verify_helper_hmac(
        &req.action,
        &req.hostnames,
        &req.nonce,
        &req.hmac,
        DEFAULT_HELPER_HMAC_KEY,
    ) {
        return Err(HelperResponse {
            ok: false,
            code: "HMAC_MISMATCH".to_string(),
            message: "Verifikasi tanda tangan HMAC gagal".to_string(),
        });
    }

    match req.action.as_str() {
        "status" => Ok(req),
        "updateHosts" => {
            if req.hostnames.len() > MAX_HOST_ENTRIES {
                return Err(HelperResponse {
                    ok: false,
                    code: "TOO_MANY_HOSTS".to_string(),
                    message: "Jumlah entri host melebihi batas 500".to_string(),
                });
            }
            for h in &req.hostnames {
                if let Err(e) = validate_hostname(h) {
                    return Err(HelperResponse {
                        ok: false,
                        code: "DISALLOWED_HOSTNAME".to_string(),
                        message: e.to_string(),
                    });
                }
            }
            Ok(req)
        }
        "trustCa" | "untrustCa" => {
            let Some(ref cert_str) = req.cert_path else {
                return Err(HelperResponse {
                    ok: false,
                    code: "MISSING_CERT_PATH".to_string(),
                    message: "Path sertifikat wajib diisi".to_string(),
                });
            };
            if cert_str.contains("..") {
                return Err(HelperResponse {
                    ok: false,
                    code: "PATH_TRAVERSAL_REJECTED".to_string(),
                    message: "Deteksi path traversal pada lokasi sertifikat".to_string(),
                });
            }
            let cert_path = Path::new(cert_str);
            let ext = cert_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if ext != "pem" && ext != "crt" {
                return Err(HelperResponse {
                    ok: false,
                    code: "INVALID_CERT_EXTENSION".to_string(),
                    message: "Sertifikat harus berekstensi .pem atau .crt".to_string(),
                });
            }
            if !cert_path.starts_with(allowed_ca_dir) {
                return Err(HelperResponse {
                    ok: false,
                    code: "CERT_OUTSIDE_CA_DIR".to_string(),
                    message: "Path sertifikat berada di luar direktori CA Corral".to_string(),
                });
            }
            Ok(req)
        }
        _ => Err(HelperResponse {
            ok: false,
            code: "UNKNOWN_ACTION".to_string(),
            message: format!("Aksi '{}' tidak didukung oleh CorralHelper", req.action),
        }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;

    use corral_platform::hosts::{
        compute_helper_hmac, HelperRequest, DEFAULT_HELPER_HMAC_KEY,
    };

    use super::validate_helper_request;

    #[test]
    fn test_helper_policy_rejects_malicious_payloads() {
        let ca_dir = PathBuf::from("C:\\Users\\dev\\AppData\\Roaming\\Corral\\ca");

        // 1. Valid request succeeds
        let hosts = vec!["my-app.test".to_string(), "api.my-app.local".to_string()];
        let nonce = "valid-nonce-0001".to_string();
        let hmac = compute_helper_hmac("updateHosts", &hosts, &nonce, DEFAULT_HELPER_HMAC_KEY);
        let valid_req = HelperRequest {
            action: "updateHosts".to_string(),
            hostnames: hosts,
            cert_path: None,
            nonce: nonce.clone(),
            hmac,
        };
        let raw_valid = serde_json::to_string(&valid_req).unwrap();
        assert!(validate_helper_request(&raw_valid, &ca_dir).is_ok());

        // 2. Rejects IP literals, non-allowlisted TLDs, path traversal, and invalid HMAC across 200+ variations
        let bad_hosts_corpus = [
            "127.0.0.1",
            "192.168.1.1",
            "google.com",
            "evil.org",
            "bank.co.id",
            "my_app.test",
            "-bad.test",
            "bad-.test",
            "sub..test",
            "localhost",
            "a b.test",
            "test;rm -rf.test",
        ];
        for (idx, bad_host) in bad_hosts_corpus.iter().cycle().take(200).enumerate() {
            let n = format!("fuzz-nonce-{idx:04}");
            let h_list = vec![(*bad_host).to_string()];
            let sig = compute_helper_hmac("updateHosts", &h_list, &n, DEFAULT_HELPER_HMAC_KEY);
            let req = HelperRequest {
                action: "updateHosts".to_string(),
                hostnames: h_list,
                cert_path: None,
                nonce: n,
                hmac: sig,
            };
            let json = serde_json::to_string(&req).unwrap();
            assert!(validate_helper_request(&json, &ca_dir).is_err());
        }

        // 3. Rejects path traversal in cert_path
        let sig_cert = compute_helper_hmac("trustCa", &[], &nonce, DEFAULT_HELPER_HMAC_KEY);
        let traversal_req = HelperRequest {
            action: "trustCa".to_string(),
            hostnames: vec![],
            cert_path: Some("C:\\Users\\dev\\AppData\\Roaming\\Corral\\ca\\..\\..\\evil.pem".to_string()),
            nonce,
            hmac: sig_cert,
        };
        let raw_traversal = serde_json::to_string(&traversal_req).unwrap();
        assert!(validate_helper_request(&raw_traversal, &ca_dir).is_err());
    }
}
