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
pub fn sanitize_email_html(raw_html: &str) -> String {
    let mut current = raw_html.to_string();
    // Strip <script ...>...</script> case-insensitively
    while let Some(start) = current.to_ascii_lowercase().find("<script") {
        if let Some(end_rel) = current
            .get(start..)
            .and_then(|s| s.to_ascii_lowercase().find("</script>"))
        {
            let remove_until = start.saturating_add(end_rel).saturating_add(9);
            current.replace_range(start..remove_until, "");
        } else {
            current.truncate(start);
            break;
        }
    }
    // Neutralize inline event handlers (`onload=`, `onerror=`, `onclick=`)
    for handler in [
        "onload=",
        "onerror=",
        "onclick=",
        "onmouseover=",
        "onfocus=",
        "javascript:",
    ] {
        current = current.replace(handler, "data-blocked=");
    }
    current
}
