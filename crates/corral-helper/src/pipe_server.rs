use std::path::PathBuf;

use corral_core::domain::HostEntry;
use corral_core::ports::HostsPort;
use corral_platform::hosts::{HelperResponse, WindowsHostsManager, HELPER_PIPE_NAME};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::ServerOptions;

use crate::policy::validate_helper_request;

pub async fn run_helper_pipe_server(appdata_dir: PathBuf) -> std::io::Result<()> {
    let ca_dir = appdata_dir.join("ca");
    let hosts_mgr = std::sync::Arc::new(WindowsHostsManager::new(appdata_dir));

    loop {
        let server = ServerOptions::new()
            .first_pipe_instance(false)
            .reject_remote_clients(true)
            .create(HELPER_PIPE_NAME)?;

        if server.connect().await.is_err() {
            continue;
        }

        let ca_dir_clone = ca_dir.clone();
        let hosts_mgr_clone = hosts_mgr.clone();

        tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(server);
            let mut lines = BufReader::new(reader).lines();
            if let Ok(Some(line)) = lines.next_line().await {
                let response = match validate_helper_request(&line, &ca_dir_clone) {
                    Ok(req) => match req.action.as_str() {
                        "status" => HelperResponse {
                            ok: true,
                            code: "OK".to_string(),
                            message: "CorralHelper aktif".to_string(),
                        },
                        "updateHosts" => {
                            let entries: Vec<HostEntry> = req
                                .hostnames
                                .into_iter()
                                .map(|h| HostEntry {
                                    ip: "127.0.0.1".to_string(),
                                    hostname: h,
                                    comment: None,
                                })
                                .collect();
                            match hosts_mgr_clone.replace_block(&entries) {
                                Ok(()) => HelperResponse {
                                    ok: true,
                                    code: "OK".to_string(),
                                    message: "Blok hosts Corral berhasil diperbarui".to_string(),
                                },
                                Err(e) => HelperResponse {
                                    ok: false,
                                    code: "HOSTS_WRITE_FAILED".to_string(),
                                    message: e.to_string(),
                                },
                            }
                        }
                        _ => HelperResponse {
                            ok: true,
                            code: "OK".to_string(),
                            message: "Operasi helper selesai".to_string(),
                        },
                    },
                    Err(err_resp) => {
                        tracing::warn!(
                            code = %err_resp.code,
                            "CorralHelper menolak request: {}",
                            err_resp.message
                        );
                        err_resp
                    }
                };

                if let Ok(mut json) = serde_json::to_string(&response) {
                    json.push('\n');
                    let _ = writer.write_all(json.as_bytes()).await;
                }
            }
        });
    }
}
