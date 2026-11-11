use std::collections::HashMap;
use std::net::TcpListener;
use std::os::windows::process::CommandExt;
use std::time::Duration;

use corral_core::dto::PortConflictView;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::io_util::run_command_bounded;

const PORT_CMD_MAX_OUTPUT: usize = 512 * 1024;
const PORT_CMD_TIMEOUT: Duration = Duration::from_secs(5);

pub fn check_port_status(port: u16, label: &str) -> PortConflictView {
    let excluded_ranges = collect_excluded_tcp_port_ranges();
    let excluded = is_port_in_ranges(port, &excluded_ranges);
    let can_bind = TcpListener::bind(("127.0.0.1", port)).is_ok();
    let self_pid = std::process::id();

    let pid_by_port = if can_bind {
        HashMap::new()
    } else {
        collect_listening_tcp_pids()
    };
    let process_names = match pid_by_port.get(&port) {
        Some(&pid) if pid != self_pid => collect_process_names(),
        _ => HashMap::new(),
    };

    build_port_conflict_view(
        port,
        label,
        can_bind,
        excluded,
        self_pid,
        &pid_by_port,
        &process_names,
    )
}

pub fn inspect_standard_ports() -> Vec<PortConflictView> {
    let targets = [
        (80_u16, "nginx HTTP"),
        (443_u16, "nginx HTTPS (TLS)"),
        (9084_u16, "PHP 8.4 FastCGI"),
        (9083_u16, "PHP 8.3 FastCGI"),
        (3306_u16, "MariaDB / MySQL"),
        (6379_u16, "Redis"),
        (1025_u16, "Corral Mail Catcher (SMTP)"),
        (9912_u16, "Corral VarDumper Receiver"),
    ];

    let excluded_ranges = collect_excluded_tcp_port_ranges();
    let self_pid = std::process::id();

    let mut bind_results: Vec<(u16, &'static str, bool, bool)> = Vec::with_capacity(targets.len());
    let mut any_in_use = false;
    for (port, label) in targets {
        let can_bind = TcpListener::bind(("127.0.0.1", port)).is_ok();
        let excluded = is_port_in_ranges(port, &excluded_ranges);
        if !can_bind {
            any_in_use = true;
        }
        bind_results.push((port, label, can_bind, excluded));
    }

    let pid_by_port = if any_in_use {
        collect_listening_tcp_pids()
    } else {
        HashMap::new()
    };

    let needs_process_names = pid_by_port.values().any(|&pid| pid != self_pid);
    let process_names = if needs_process_names {
        collect_process_names()
    } else {
        HashMap::new()
    };

    bind_results
        .into_iter()
        .map(|(port, label, can_bind, excluded)| {
            build_port_conflict_view(
                port,
                label,
                can_bind,
                excluded,
                self_pid,
                &pid_by_port,
                &process_names,
            )
        })
        .collect()
}

fn build_port_conflict_view(
    port: u16,
    label: &str,
    can_bind: bool,
    excluded: bool,
    self_pid: u32,
    pid_by_port: &HashMap<u16, u32>,
    process_names: &HashMap<u32, String>,
) -> PortConflictView {
    if can_bind && !excluded {
        return PortConflictView {
            port,
            service_label: label.to_string(),
            in_use: false,
            pid: None,
            process_name: None,
            excluded_by_win_nat: false,
            recommendation: "Port bebas dan siap digunakan oleh Corral".to_string(),
        };
    }

    let pid = pid_by_port.get(&port).copied();
    let proc_name = pid.and_then(|p| {
        if p == self_pid {
            Some("corral.exe (Internal)".to_string())
        } else {
            process_names.get(&p).cloned()
        }
    });

    let recommendation = if excluded {
        format!(
            "Port {port} masuk rentang excludedportrange Windows (Hyper-V/WSL). Jalankan `net stop winnat && net start winnat` atau ganti port di Settings."
        )
    } else if let Some(ref p_name) = proc_name {
        format!(
            "Port {port} sedang dipakai oleh {p_name} (PID {}). Matikan service tersebut atau gunakan fitur ganti port otomatis.",
            pid.unwrap_or(0)
        )
    } else {
        format!(
            "Port {port} sedang terpakai. Corral dapat mengalihkan ke port cadangan secara otomatis."
        )
    };

    PortConflictView {
        port,
        service_label: label.to_string(),
        in_use: !can_bind,
        pid,
        process_name: proc_name,
        excluded_by_win_nat: excluded,
        recommendation,
    }
}

fn is_port_in_ranges(port: u16, ranges: &[(u16, u16)]) -> bool {
    ranges
        .iter()
        .any(|&(start, end)| port >= start && port <= end)
}

fn collect_excluded_tcp_port_ranges() -> Vec<(u16, u16)> {
    let mut cmd = std::process::Command::new("netsh");
    cmd.args(["interface", "ipv4", "show", "excludedportrange", "protocol=tcp"])
        .creation_flags(CREATE_NO_WINDOW);
    let Ok(out) = run_command_bounded(&mut cmd, None, PORT_CMD_MAX_OUTPUT, PORT_CMD_TIMEOUT)
    else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    for line in out.stdout.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if let (Some(&start_s), Some(&end_s)) = (cols.first(), cols.get(1)) {
            if let (Ok(start), Ok(end)) = (start_s.parse::<u16>(), end_s.parse::<u16>()) {
                ranges.push((start, end));
            }
        }
    }
    ranges
}

fn collect_listening_tcp_pids() -> HashMap<u16, u32> {
    let mut cmd = std::process::Command::new("netstat");
    cmd.args(["-ano", "-p", "TCP"])
        .creation_flags(CREATE_NO_WINDOW);
    let Ok(out) = run_command_bounded(&mut cmd, None, PORT_CMD_MAX_OUTPUT, PORT_CMD_TIMEOUT)
    else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    for line in out.stdout.lines() {
        if !line.contains("LISTENING") {
            continue;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        if let (Some(&local_addr), Some(&pid_str)) = (cols.get(1), cols.last()) {
            let Some(port_str) = local_addr.rsplit(':').next() else {
                continue;
            };
            if let (Ok(port), Ok(pid)) = (port_str.parse::<u16>(), pid_str.parse::<u32>()) {
                map.entry(port).or_insert(pid);
            }
        }
    }
    map
}

fn collect_process_names() -> HashMap<u32, String> {
    let mut cmd = std::process::Command::new("tasklist");
    cmd.args(["/NH", "/FO", "CSV"])
        .creation_flags(CREATE_NO_WINDOW);
    let Ok(out) = run_command_bounded(&mut cmd, None, PORT_CMD_MAX_OUTPUT, PORT_CMD_TIMEOUT)
    else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    for line in out.stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('"') {
            continue;
        }
        let mut parts = trimmed.split("\",\"");
        let Some(raw_name) = parts.next() else {
            continue;
        };
        let Some(raw_pid) = parts.next() else {
            continue;
        };
        let name = raw_name.trim_start_matches('"').trim();
        let pid_str = raw_pid.trim_matches('"').trim();
        if !name.is_empty() {
            if let Ok(pid) = pid_str.parse::<u32>() {
                map.insert(pid, name.to_string());
            }
        }
    }
    map
}
