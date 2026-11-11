use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use corral_core::error::AppError;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

use crate::job::JobHandle;

/// Outcome of reading a single line with a strict byte cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundedLine {
    /// Complete line within `max_bytes` (trailing `\r\n` or `\n` stripped).
    Line(String),
    /// Line exceeded `max_bytes`; returns the truncated prefix up to `max_bytes` after draining to `\n`.
    Oversized(String),
    /// Clean EOF before any bytes were read.
    Eof,
}

fn bytes_to_string_lossless_or_lossy(mut bytes: Vec<u8>) -> String {
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
    }
}

/// Async bounded line reader with idle/total line timeout.
/// Never allocates more than `max_bytes` for the line buffer, preventing single-line OOM attacks.
pub async fn read_bounded_line<R>(
    reader: &mut R,
    max_bytes: usize,
    timeout_dur: Duration,
) -> std::io::Result<BoundedLine>
where
    R: AsyncBufRead + Unpin,
{
    let max_drain_bytes = max_bytes.saturating_add(1024 * 1024);

    tokio::time::timeout(timeout_dur, async {
        let mut buf: Vec<u8> = Vec::with_capacity(max_bytes.min(4096));
        let mut exceeded = false;
        let mut total_seen: usize = 0;

        loop {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                if buf.is_empty() && !exceeded {
                    return Ok(BoundedLine::Eof);
                }
                break;
            }

            if let Some(nl_pos) = available.iter().position(|&b| b == b'\n') {
                let line_part = available.get(..nl_pos).unwrap_or_default();
                if !exceeded {
                    let remaining = max_bytes.saturating_sub(buf.len());
                    if line_part.len() <= remaining {
                        buf.extend_from_slice(line_part);
                    } else {
                        buf.extend_from_slice(line_part.get(..remaining).unwrap_or_default());
                        exceeded = true;
                    }
                }
                reader.consume(nl_pos.saturating_add(1));
                break;
            }

            let chunk_len = available.len();
            total_seen = total_seen.saturating_add(chunk_len);
            if !exceeded {
                let remaining = max_bytes.saturating_sub(buf.len());
                if chunk_len <= remaining {
                    buf.extend_from_slice(available);
                } else {
                    buf.extend_from_slice(available.get(..remaining).unwrap_or_default());
                    exceeded = true;
                }
            }
            reader.consume(chunk_len);

            if total_seen > max_drain_bytes {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unterminated oversized stream exceeded drain limit",
                ));
            }
        }

        let text = bytes_to_string_lossless_or_lossy(buf);
        if exceeded {
            Ok(BoundedLine::Oversized(text))
        } else {
            Ok(BoundedLine::Line(text))
        }
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "bounded line read timed out"))?
}

/// Synchronous bounded line reader (e.g. for stdio JSON-RPC in `corral-mcp`).
pub fn read_bounded_sync_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> std::io::Result<BoundedLine> {
    let max_drain_bytes = max_bytes.saturating_add(1024 * 1024);
    let mut buf: Vec<u8> = Vec::with_capacity(max_bytes.min(4096));
    let mut exceeded = false;
    let mut total_seen: usize = 0;

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if buf.is_empty() && !exceeded {
                return Ok(BoundedLine::Eof);
            }
            break;
        }

        if let Some(nl_pos) = available.iter().position(|&b| b == b'\n') {
            let line_part = available.get(..nl_pos).unwrap_or_default();
            if !exceeded {
                let remaining = max_bytes.saturating_sub(buf.len());
                if line_part.len() <= remaining {
                    buf.extend_from_slice(line_part);
                } else {
                    buf.extend_from_slice(line_part.get(..remaining).unwrap_or_default());
                    exceeded = true;
                }
            }
            reader.consume(nl_pos.saturating_add(1));
            break;
        }

        let chunk_len = available.len();
        total_seen = total_seen.saturating_add(chunk_len);
        if !exceeded {
            let remaining = max_bytes.saturating_sub(buf.len());
            if chunk_len <= remaining {
                buf.extend_from_slice(available);
            } else {
                buf.extend_from_slice(available.get(..remaining).unwrap_or_default());
                exceeded = true;
            }
        }
        reader.consume(chunk_len);

        if total_seen > max_drain_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unterminated oversized stdin line exceeded drain limit",
            ));
        }
    }

    let text = bytes_to_string_lossless_or_lossy(buf);
    if exceeded {
        Ok(BoundedLine::Oversized(text))
    } else {
        Ok(BoundedLine::Line(text))
    }
}

/// Read the last `max_lines` lines from a potentially multi-gigabyte log file by seeking
/// at most `max_bytes` from the end of the file.
pub fn read_tail_lines(
    path: &Path,
    max_bytes: u64,
    max_lines: usize,
) -> Result<Vec<String>, AppError> {
    if max_lines == 0 || max_bytes == 0 || !path.is_file() {
        return Ok(Vec::new());
    }

    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(AppError::Io(format!("gagal membuka log {}: {e}", path.display()))),
    };

    let file_len = file.metadata()?.len();
    if file_len == 0 {
        return Ok(Vec::new());
    }

    let seek_pos = file_len.saturating_sub(max_bytes);
    if seek_pos > 0 {
        file.seek(SeekFrom::Start(seek_pos))?;
    }

    let to_read = file_len.saturating_sub(seek_pos).min(max_bytes);
    let cap = usize::try_from(to_read).unwrap_or(256 * 1024);
    let mut buf = Vec::with_capacity(cap);
    file.take(to_read).read_to_end(&mut buf)?;

    let text = String::from_utf8_lossy(&buf);
    let mut lines_iter = text.lines();
    if seek_pos > 0 {
        // Discard partial first line when starting mid-file.
        let _ = lines_iter.next();
    }

    let mut ring: VecDeque<String> = VecDeque::with_capacity(max_lines.min(512));
    for line in lines_iter {
        if ring.len() == max_lines {
            let _ = ring.pop_front();
        }
        ring.push_back(line.to_string());
    }

    Ok(ring.into_iter().collect())
}

/// Kill a child process and poll `try_wait` up to `max_wait` so shutdown never blocks indefinitely.
pub fn kill_and_reap_bounded(child: &mut Child, max_wait: Duration) {
    let _ = child.kill();
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) => {
                if started.elapsed() >= max_wait {
                    break;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
        }
    }
}

fn spawn_pipe_reader<R: Read + Send + 'static>(
    pipe: Option<R>,
    max_bytes: usize,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let Some(mut reader) = pipe else {
            return String::new();
        };
        let mut captured: Vec<u8> = Vec::with_capacity(max_bytes.min(8192));
        let mut truncated = false;
        let mut chunk = [0_u8; 4096];

        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if !truncated {
                        let slice = chunk.get(..n).unwrap_or_default();
                        let remaining = max_bytes.saturating_sub(captured.len());
                        if slice.len() <= remaining {
                            captured.extend_from_slice(slice);
                        } else {
                            captured.extend_from_slice(slice.get(..remaining).unwrap_or_default());
                            truncated = true;
                        }
                    }
                    // Continue draining pipe until EOF so child never blocks on full stdout/stderr pipe.
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }

        let mut out = String::from_utf8_lossy(&captured).into_owned();
        if truncated {
            out.push_str("\n...[output dipotong oleh Corral demi batas memori]...");
        }
        out
    })
}

#[derive(Debug, Clone)]
pub struct BoundedCommandOutput {
    pub exit_code: i32,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// Spawn a subprocess with piped stdout/stderr, optional Windows Job Object assignment,
/// strict output byte cap, and wall-clock execution timeout.
pub fn run_command_bounded(
    cmd: &mut Command,
    job: Option<&JobHandle>,
    max_output_bytes: usize,
    timeout: Duration,
) -> Result<BoundedCommandOutput, AppError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    if let Some(j) = job {
        let _ = j.assign_child(&child);
    }

    let stdout_handle = spawn_pipe_reader(child.stdout.take(), max_output_bytes);
    let stderr_handle = spawn_pipe_reader(child.stderr.take(), max_output_bytes);

    let started = Instant::now();
    let mut timed_out = false;
    let mut exit_code: i32 = -1;
    let mut success = false;

    loop {
        match child.try_wait()? {
            Some(status) => {
                success = status.success();
                exit_code = status.code().unwrap_or(if success { 0 } else { 1 });
                break;
            }
            None => {
                if started.elapsed() >= timeout {
                    timed_out = true;
                    kill_and_reap_bounded(&mut child, Duration::from_millis(500));
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();

    Ok(BoundedCommandOutput {
        exit_code,
        success,
        stdout,
        stderr,
        timed_out,
    })
}
