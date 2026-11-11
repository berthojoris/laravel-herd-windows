use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use corral_core::domain::HostEntry;
use corral_core::ports::HostsPort;
use corral_platform::hosts::{HelperResponse, WindowsHostsManager, HELPER_PIPE_NAME};
use corral_platform::io_util::{read_bounded_line, BoundedLine};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::sync::Semaphore;

use crate::policy::{validate_helper_request, MAX_PAYLOAD_BYTES};

const MAX_CONCURRENT_PIPE_CLIENTS: usize = 8;
const PIPE_READ_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn run_helper_pipe_server(appdata_dir: PathBuf) -> std::io::Result<()> {
    let ca_dir = appdata_dir.join("ca");
    let hosts_mgr = Arc::new(WindowsHostsManager::new(appdata_dir));
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_PIPE_CLIENTS));

    loop {
        let server = ServerOptions::new()
            .first_pipe_instance(false)
            .reject_remote_clients(true)
            .max_instances(MAX_CONCURRENT_PIPE_CLIENTS)
            .create(HELPER_PIPE_NAME)?;

        if server.connect().await.is_err() {
            continue;
        }

        let Ok(permit) = semaphore.clone().try_acquire_owned() else {
            drop(server);
            continue;
        };

        let ca_dir_clone = ca_dir.clone();
        let hosts_mgr_clone = hosts_mgr.clone();

        tokio::spawn(async move {
            let _permit = permit;
            let (reader, mut writer) = tokio::io::split(server);
            let mut buf_reader = BufReader::new(reader);

            let response = match read_bounded_line(
                &mut buf_reader,
                MAX_PAYLOAD_BYTES.saturating_add(1024),
                PIPE_READ_TIMEOUT,
            )
            .await
            {
                Ok(BoundedLine::Eof) => return,
                Ok(BoundedLine::Oversized(_)) => HelperResponse {
                    ok: false,
                    code: "PAYLOAD_SIZE_INVALID".to_string(),
                    message: "Ukuran payload melebihi batas 64 KB".to_string(),
                },
                Err(err) if err.kind() == std::io::ErrorKind::TimedOut => HelperResponse {
                    ok: false,
                    code: "REQUEST_TIMEOUT".to_string(),
                    message: "Pembacaan request named pipe melampaui batas waktu 5 detik"
                        .to_string(),
                },
                Err(err) => HelperResponse {
                    ok: false,
                    code: "IO_ERROR".to_string(),
                    message: format!("Gagal membaca request pipe: {err}"),
                },
                Ok(BoundedLine::Line(line)) => match validate_helper_request(&line, &ca_dir_clone)
                {
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
                            let write_res = tokio::task::spawn_blocking(move || {
                                hosts_mgr_clone.replace_block(&entries)
                            })
                            .await;
                            match write_res {
                                Ok(Ok(())) => HelperResponse {
                                    ok: true,
                                    code: "OK".to_string(),
                                    message: "Blok hosts Corral berhasil diperbarui".to_string(),
                                },
                                Ok(Err(e)) => HelperResponse {
                                    ok: false,
                                    code: "HOSTS_WRITE_FAILED".to_string(),
                                    message: e.to_string(),
                                },
                                Err(join_err) => HelperResponse {
                                    ok: false,
                                    code: "HOSTS_WRITE_FAILED".to_string(),
                                    message: format!("Task penulisan hosts gagal: {join_err}"),
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
                },
            };

            if let Ok(mut json) = serde_json::to_string(&response) {
                json.push('\n');
                let _ = writer.write_all(json.as_bytes()).await;
            }
        });
    }
}
