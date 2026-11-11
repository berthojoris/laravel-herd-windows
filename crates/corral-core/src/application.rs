use crate::domain::{HostEntry, ServiceState, Site, Workspace};
use crate::error::AppError;

pub const HOSTS_START_MARKER: &str = "# Corral generated Hosts. Do not change.";
pub const HOSTS_END_MARKER: &str = "# End Corral generated Hosts";

/// Restart policy for site workers (SVC-7: max 3 restarts within a 60-second window, then Degraded).
#[derive(Debug, Clone)]
pub struct WorkerRestartPolicy {
    pub max_restarts: usize,
    pub window_secs: i64,
    pub restart_timestamps: Vec<i64>,
}

impl Default for WorkerRestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 3,
            window_secs: 60,
            restart_timestamps: Vec::new(),
        }
    }
}

impl WorkerRestartPolicy {
    pub fn record_failure_and_decide(&mut self, now_unix: i64) -> (bool, ServiceState) {
        let cutoff = now_unix.saturating_sub(self.window_secs);
        self.restart_timestamps.retain(|&ts| ts >= cutoff);
        if self.restart_timestamps.len() >= self.max_restarts {
            (false, ServiceState::Degraded)
        } else {
            self.restart_timestamps.push(now_unix);
            (true, ServiceState::Starting)
        }
    }

    pub fn reset(&mut self) {
        self.restart_timestamps.clear();
    }
}

/// Collect all host entries across sites and workspaces for the Windows `hosts` file block.
pub fn collect_host_entries(sites: &[Site], workspaces: &[Workspace]) -> Vec<HostEntry> {
    let mut entries = Vec::new();
    for site in sites {
        for host in &site.hostnames {
            if host.starts_with("*.") {
                continue;
            }
            entries.push(HostEntry {
                ip: "127.0.0.1".to_string(),
                hostname: host.clone(),
                comment: Some(format!("site:{}", site.name)),
            });
        }
        // Add reverb.<site>.test entry if Reverb worker is configured
        let primary = site.primary_host();
        entries.push(HostEntry {
            ip: "127.0.0.1".to_string(),
            hostname: format!("reverb.{primary}"),
            comment: Some(format!("reverb:{}", site.name)),
        });
    }
    for ws in workspaces {
        entries.push(HostEntry {
            ip: "127.0.0.1".to_string(),
            hostname: ws.host.clone(),
            comment: Some(format!("workspace:{}:{}", ws.site_name, ws.branch)),
        });
    }
    entries
}

/// Replace or append the `# Corral generated Hosts` section inside a Windows `hosts` file content (DNS-3).
pub fn render_hosts_file_with_block(
    existing_hosts: &str,
    entries: &[HostEntry],
) -> Result<String, AppError> {
    for entry in entries {
        crate::domain::validate_hostname(&entry.hostname)?;
    }

    let mut clean_lines: Vec<&str> = Vec::new();
    let mut inside_block = false;
    for line in existing_hosts.lines() {
        let trimmed = line.trim();
        if trimmed == HOSTS_START_MARKER {
            inside_block = true;
            continue;
        }
        if trimmed == HOSTS_END_MARKER {
            inside_block = false;
            continue;
        }
        if !inside_block {
            clean_lines.push(line);
        }
    }

    while clean_lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        clean_lines.pop();
    }

    let mut out = clean_lines.join("\r\n");
    if !out.is_empty() {
        out.push_str("\r\n\r\n");
    }

    if !entries.is_empty() {
        out.push_str(HOSTS_START_MARKER);
        out.push_str("\r\n");
        for entry in entries {
            out.push_str(&format!("127.0.0.1 {}\r\n", entry.hostname));
            out.push_str(&format!("::1 {}\r\n", entry.hostname));
        }
        out.push_str(HOSTS_END_MARKER);
        out.push_str("\r\n");
    }

    Ok(out)
}

/// Sanitize HTML email body for safe `<iframe sandbox="allow-same-origin" srcdoc="...">` preview (§12).
/// Runs in linear O(N) time without full-document lowercasing clones or slice-indexing panics.
pub fn sanitize_email_html(raw_html: &str) -> String {
    use std::borrow::Cow;

    fn starts_with_ignore_ascii_case_at(haystack: &[u8], pos: usize, needle: &[u8]) -> bool {
        haystack
            .get(pos..pos.saturating_add(needle.len()))
            .is_some_and(|sub| sub.eq_ignore_ascii_case(needle))
    }

    // Pass 1: Strip `<script ...>...</script>` (or unclosed `<script...` to EOF) case-insensitively.
    let bytes = raw_html.as_bytes();
    let len = bytes.len();
    let mut without_scripts: Option<String> = None;
    let mut chunk_start: usize = 0;
    let mut i: usize = 0;

    while i < len {
        if bytes.get(i) == Some(&b'<') && starts_with_ignore_ascii_case_at(bytes, i, b"<script") {
            let buf = without_scripts.get_or_insert_with(|| String::with_capacity(len));
            if let Some(seg) = raw_html.get(chunk_start..i) {
                buf.push_str(seg);
            }

            let mut j = i.saturating_add(7);
            let mut closed_end: Option<usize> = None;
            while j < len {
                if bytes.get(j) == Some(&b'<')
                    && starts_with_ignore_ascii_case_at(bytes, j, b"</script")
                {
                    let mut k = j.saturating_add(8);
                    while k < len && bytes.get(k) != Some(&b'>') {
                        k = k.saturating_add(1);
                    }
                    closed_end = Some(if k < len { k.saturating_add(1) } else { len });
                    break;
                }
                j = j.saturating_add(1);
            }

            if let Some(next_pos) = closed_end {
                i = next_pos;
                chunk_start = next_pos;
                continue;
            } else {
                chunk_start = len;
                break;
            }
        }
        i = i.saturating_add(1);
    }

    let stage1: Cow<'_, str> = match without_scripts {
        Some(mut buf) => {
            if chunk_start < len {
                if let Some(seg) = raw_html.get(chunk_start..len) {
                    buf.push_str(seg);
                }
            }
            Cow::Owned(buf)
        }
        None => Cow::Borrowed(raw_html),
    };

    // Pass 2: Neutralize inline event handlers and script URI schemes case-insensitively in O(N).
    const BLOCKED_PATTERNS: &[&[u8]] = &[
        b"onload=",
        b"onerror=",
        b"onclick=",
        b"onmouseover=",
        b"onfocus=",
        b"onmouseenter=",
        b"onloadstart=",
        b"ontoggle=",
        b"onsubmit=",
        b"javascript:",
        b"vbscript:",
    ];

    let src = stage1.as_ref();
    let src_bytes = src.as_bytes();
    let src_len = src_bytes.len();
    let mut sanitized: Option<String> = None;
    let mut p2_chunk_start: usize = 0;
    let mut p2_i: usize = 0;

    while p2_i < src_len {
        let lower_first = src_bytes.get(p2_i).map(u8::to_ascii_lowercase);
        if matches!(lower_first, Some(b'o' | b'j' | b'v')) {
            let matched_len = BLOCKED_PATTERNS
                .iter()
                .find(|pat| starts_with_ignore_ascii_case_at(src_bytes, p2_i, pat))
                .map(|pat| pat.len());

            if let Some(pat_len) = matched_len {
                let buf = sanitized.get_or_insert_with(|| String::with_capacity(src_len));
                if let Some(seg) = src.get(p2_chunk_start..p2_i) {
                    buf.push_str(seg);
                }
                buf.push_str("data-blocked=");
                p2_i = p2_i.saturating_add(pat_len);
                p2_chunk_start = p2_i;
                continue;
            }
        }
        p2_i = p2_i.saturating_add(1);
    }

    match sanitized {
        Some(mut buf) => {
            if p2_chunk_start < src_len {
                if let Some(seg) = src.get(p2_chunk_start..src_len) {
                    buf.push_str(seg);
                }
            }
            buf
        }
        None => stage1.into_owned(),
    }
}
