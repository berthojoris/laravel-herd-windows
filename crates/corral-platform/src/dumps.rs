use std::sync::Arc;
use std::time::Duration;

use corral_core::domain::DumpEntry;
use corral_core::ports::RepositoryPort;
use tokio::io::BufReader;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::io_util::{read_bounded_line, BoundedLine};

const MAX_CONCURRENT_DUMP_CLIENTS: usize = 32;
const MAX_DUMP_LINE_BYTES: usize = 256 * 1024;
const DUMP_LINE_TIMEOUT: Duration = Duration::from_secs(30);

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
                let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_DUMP_CLIENTS));

                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            break;
                        }
                        accept_res = listener.accept() => {
                            if let Ok((stream, _peer)) = accept_res {
                                let Ok(permit) = semaphore.clone().try_acquire_owned() else {
                                    drop(stream);
                                    continue;
                                };
                                let repo_clone = repo.clone();
                                let client_cancel = cancel.clone();
                                tokio::spawn(async move {
                                    let _permit = permit;
                                    tokio::select! {
                                        _ = client_cancel.cancelled() => {}
                                        _ = handle_dump_client(stream, repo_clone) => {}
                                    }
                                });
                            }
                        }
                    }
                }
            });
        });
}

async fn handle_dump_client(
    stream: TcpStream,
    repo: Arc<dyn RepositoryPort>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream);
    loop {
        let (line, truncated) =
            match read_bounded_line(&mut reader, MAX_DUMP_LINE_BYTES, DUMP_LINE_TIMEOUT).await? {
                BoundedLine::Eof => break,
                BoundedLine::Line(line) => (line, false),
                BoundedLine::Oversized(prefix) => (prefix, true),
            };
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
            kind: if truncated {
                "VarDumper (Truncated)".to_string()
            } else {
                "VarDumper".to_string()
            },
            payload: if truncated {
                format!("{trimmed}\n...[dipotong oleh Corral karena melebihi batas 256 KB]")
            } else {
                trimmed.to_string()
            },
        };
        let repo_for_insert = repo.clone();
        let _ = tokio::task::spawn_blocking(move || repo_for_insert.insert_dump(&entry)).await;
    }
    Ok(())
}
