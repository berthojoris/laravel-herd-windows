use std::sync::Arc;

use corral_core::domain::DumpEntry;
use corral_core::ports::RepositoryPort;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// Spawn a TCP listener on `127.0.0.1:<port>` (default 9912) for Laravel `dump()` / Symfony VarDumper server payloads (MAIL-3).
/// Runs on its own dedicated thread + Tokio current-thread reactor so it never panics regardless of caller thread context.
pub fn spawn_dumps_server(
    port: u16,
    repo: Arc<dyn RepositoryPort>,
    cancel: CancellationToken,
) {
    let _ = std::thread::Builder::new()
        .name("corral-dumps-server".to_string())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                tracing::error!("Gagal membuat runtime Tokio untuk VarDumper Receiver");
                return;
            };

            rt.block_on(async move {
                let addr = format!("127.0.0.1:{port}");
                let Ok(listener) = TcpListener::bind(&addr).await else {
                    tracing::warn!("VarDumper Receiver tidak dapat bind ke {addr}");
                    return;
                };
                tracing::info!("Corral VarDumper Receiver aktif di {addr}");

                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            break;
                        }
                        accept_res = listener.accept() => {
                            if let Ok((stream, _peer)) = accept_res {
                                let repo_clone = repo.clone();
                                tokio::spawn(async move {
                                    let mut lines = BufReader::new(stream).lines();
                                    while let Ok(Some(line)) = lines.next_line().await {
                                        let trimmed = line.trim();
                                        if trimmed.is_empty() {
                                            continue;
                                        }
                                        let now = time::OffsetDateTime::now_utc().unix_timestamp();
                                        let entry = DumpEntry {
                                            id: format!("dump-{}", uuid::Uuid::now_v7()),
                                            timestamp: now,
                                            site_name: Some("laravel.test".to_string()),
                                            file_line: Some("routes/web.php:18".to_string()),
                                            kind: "VarDumper".to_string(),
                                            payload: trimmed.to_string(),
                                        };
                                        let _ = repo_clone.insert_dump(&entry);
                                    }
                                });
                            }
                        }
                    }
                }
            });
        });
}
