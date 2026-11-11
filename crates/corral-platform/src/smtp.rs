use std::sync::Arc;
use std::time::Duration;

use corral_core::application::sanitize_email_html;
use corral_core::domain::MailMessage;
use corral_core::ports::RepositoryPort;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::io_util::{read_bounded_line, BoundedLine};

const MAX_CONCURRENT_SMTP_CLIENTS: usize = 32;
const MAX_SMTP_LINE_BYTES: usize = 64 * 1024;
const MAX_SMTP_DATA_BYTES: usize = 10 * 1024 * 1024;
const MAX_RCPT_TO: usize = 100;
const SMTP_LINE_TIMEOUT: Duration = Duration::from_secs(30);

/// Spawn an embedded async SMTP server on `127.0.0.1:<port>` (default 1025) that captures outgoing
/// Laravel `Mail::to(...)->send()` messages directly into SQLite (MAIL-1).
/// Uses a dedicated background thread with its own Tokio reactor so it is safe to call from any thread
/// (including `fn main()` before Tauri starts its runtime).
pub fn spawn_smtp_server(
    port: u16,
    repo: Arc<dyn RepositoryPort>,
    cancel: CancellationToken,
) {
    let _ = std::thread::Builder::new()
        .name("corral-smtp-catcher".to_string())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                tracing::error!("Gagal membuat runtime Tokio untuk SMTP Mail Catcher");
                return;
            };

            rt.block_on(async move {
                let addr = format!("127.0.0.1:{port}");
                let Ok(listener) = TcpListener::bind(&addr).await else {
                    tracing::warn!(
                        "SMTP Mail Catcher tidak dapat bind ke {addr} (mungkin sudah aktif)"
                    );
                    return;
                };
                tracing::info!("Corral SMTP Mail Catcher aktif di {addr}");
                let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_SMTP_CLIENTS));

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
                                        _ = handle_smtp_client(stream, repo_clone) => {}
                                    }
                                });
                            }
                        }
                    }
                }
            });
        });
}

async fn handle_smtp_client(
    stream: TcpStream,
    repo: Arc<dyn RepositoryPort>,
) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    writer
        .write_all(b"220 corral.local ESMTP Corral Mail Catcher Ready\r\n")
        .await?;

    let mut mail_from = String::from("app@corral.test");
    let mut rcpt_to: Vec<String> = Vec::new();
    let mut in_data = false;
    let mut data_overflow = false;
    let mut raw_data = String::new();

    loop {
        let line = match read_bounded_line(&mut reader, MAX_SMTP_LINE_BYTES, SMTP_LINE_TIMEOUT)
            .await?
        {
            BoundedLine::Eof => break,
            BoundedLine::Oversized(prefix) => {
                if in_data {
                    data_overflow = true;
                    if raw_data.len() < MAX_SMTP_DATA_BYTES {
                        let remaining = MAX_SMTP_DATA_BYTES.saturating_sub(raw_data.len());
                        if let Some(chunk) = prefix.get(..remaining.min(prefix.len())) {
                            raw_data.push_str(chunk);
                        }
                    }
                    continue;
                } else {
                    writer.write_all(b"500 5.5.2 Line too long\r\n").await?;
                    continue;
                }
            }
            BoundedLine::Line(line) => line,
        };

        if in_data {
            if line == "." {
                in_data = false;
                if data_overflow {
                    writer
                        .write_all(b"552 5.3.4 Message size exceeds fixed maximum message size\r\n")
                        .await?;
                } else {
                    let msg = parse_raw_smtp_payload(&mail_from, &rcpt_to, &raw_data);
                    let repo_for_insert = repo.clone();
                    let _ = tokio::task::spawn_blocking(move || repo_for_insert.insert_mail(&msg))
                        .await;
                    writer.write_all(b"250 2.0.0 Ok: queued by Corral\r\n").await?;
                }
                raw_data.clear();
                raw_data.shrink_to(4096);
                data_overflow = false;
            } else if !data_overflow {
                let unstuffed = line.strip_prefix('.').unwrap_or(&line);
                if raw_data
                    .len()
                    .saturating_add(unstuffed.len())
                    .saturating_add(2)
                    > MAX_SMTP_DATA_BYTES
                {
                    data_overflow = true;
                } else {
                    raw_data.push_str(unstuffed);
                    raw_data.push_str("\r\n");
                }
            }
            continue;
        }

        let upper = line.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            writer
                .write_all(b"250-corral.local\r\n250-SIZE 10485760\r\n250 8BITMIME\r\n")
                .await?;
        } else if upper.starts_with("MAIL FROM:") {
            mail_from = extract_angle_addr(&line);
            rcpt_to.clear();
            raw_data.clear();
            data_overflow = false;
            writer.write_all(b"250 2.1.0 Ok\r\n").await?;
        } else if upper.starts_with("RCPT TO:") {
            if rcpt_to.len() >= MAX_RCPT_TO {
                writer
                    .write_all(b"452 4.5.3 Too many recipients\r\n")
                    .await?;
            } else {
                rcpt_to.push(extract_angle_addr(&line));
                writer.write_all(b"250 2.1.5 Ok\r\n").await?;
            }
        } else if upper == "DATA" {
            in_data = true;
            data_overflow = false;
            raw_data.clear();
            writer
                .write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                .await?;
        } else if upper.starts_with("RSET") {
            mail_from.clear();
            rcpt_to.clear();
            raw_data.clear();
            data_overflow = false;
            writer.write_all(b"250 2.0.0 Ok\r\n").await?;
        } else if upper.starts_with("NOOP") {
            writer.write_all(b"250 2.0.0 Ok\r\n").await?;
        } else if upper.starts_with("QUIT") {
            writer.write_all(b"221 2.0.0 Bye\r\n").await?;
            break;
        } else {
            writer.write_all(b"250 2.0.0 Ok\r\n").await?;
        }
    }

    Ok(())
}

fn extract_angle_addr(line: &str) -> String {
    if let (Some(s), Some(e)) = (line.find('<'), line.find('>')) {
        if e > s + 1 {
            return line.get(s + 1..e).unwrap_or_default().trim().to_string();
        }
    }
    line.split_once(':')
        .map(|(_, r)| r.trim().to_string())
        .unwrap_or_else(|| "unknown@corral.test".to_string())
}

pub fn parse_raw_smtp_payload(from: &str, to: &[String], raw: &str) -> MailMessage {
    let (headers_part, body_part) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let mut subject = String::from("(Tanpa Subjek)");
    for line in headers_part.lines() {
        if let Some(rest) = line.strip_prefix("Subject:").or_else(|| line.strip_prefix("subject:")) {
            subject = rest.trim().to_string();
        }
    }
    let clean_body = body_part.trim();
    let preview: String = clean_body
        .chars()
        .filter(|&c| c != '\r' && c != '\n')
        .take(140)
        .collect();
    let html_body = if clean_body.contains('<') && clean_body.contains('>') {
        sanitize_email_html(clean_body)
    } else {
        format!(
            "<pre style=\"font-family:'Segoe UI',sans-serif;white-space:pre-wrap;\">{}</pre>",
            clean_body
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        )
    };
    let now = time::OffsetDateTime::now_utc().unix_timestamp();

    MailMessage {
        id: format!("mail-{}", uuid::Uuid::now_v7()),
        received_at: now,
        envelope_from: from.to_string(),
        recipients: if to.is_empty() {
            vec!["dev@corral.test".to_string()]
        } else {
            to.to_vec()
        },
        subject,
        preview,
        html_body,
        text_body: clean_body.to_string(),
        raw_headers: headers_part.to_string(),
        attachments: Vec::new(),
        read: false,
    }
}
