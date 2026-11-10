use std::net::TcpListener;
use std::os::windows::process::CommandExt;

use corral_core::dto::PortConflictView;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

pub fn check_port_status(port: u16, label: &str) -> PortConflictView {
    let excluded = is_port_in_excluded_range(port);
    let can_bind = TcpListener::bind(("127.0.0.1", port)).is_ok();

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

    let (pid, proc_name) = find_pid_using_port(port);
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
        format!("Port {port} sedang terpakai. Corral dapat mengalihkan ke port cadangan secara otomatis.")
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

pub fn inspect_standard_ports() -> Vec<PortConflictView> {
    let targets = [
        (80, "nginx HTTP"),
        (443, "nginx HTTPS (TLS)"),
        (9084, "PHP 8.4 FastCGI"),
        (9083, "PHP 8.3 FastCGI"),
        (3306, "MariaDB / MySQL"),
        (6379, "Redis"),
        (1025, "Corral Mail Catcher (SMTP)"),
        (9912, "Corral VarDumper Receiver"),
    ];
    targets
        .into_iter()
        .map(|(p, l)| check_port_status(p, l))
        .collect()
}

fn is_port_in_excluded_range(port: u16) -> bool {
    let Ok(out) = std::process::Command::new("netsh")
        .args(["interface", "ipv4", "show", "excludedportrange", "protocol=tcp"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return false;
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if let (Some(&start_s), Some(&end_s)) = (cols.first(), cols.get(1)) {
            if let (Ok(start), Ok(end)) = (start_s.parse::<u16>(), end_s.parse::<u16>()) {
                if port >= start && port <= end {
                    return true;
                }
            }
        }
    }
    false
}

fn find_pid_using_port(port: u16) -> (Option<u32>, Option<String>) {
    let Ok(out) = std::process::Command::new("netstat")
        .args(["-ano", "-p", "TCP"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return (None, None);
    };
    let needle = format!(":{port}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        if !line.contains("LISTENING") {
            continue;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        if let (Some(&local_addr), Some(&pid_str)) = (cols.get(1), cols.get(4)) {
            if local_addr.ends_with(&needle) {
                if let Ok(pid) = pid_str.parse::<u32>() {
                    if pid == std::process::id() {
                        return (Some(pid), Some("corral.exe (Internal)".to_string()));
                    }
                    let name = lookup_process_name(pid);
                    return (Some(pid), name);
                }
            }
        }
    }
    (None, None)
}

fn lookup_process_name(pid: u32) -> Option<String> {
    let filter = format!("PID eq {pid}");
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &filter, "/NH", "/FO", "CSV"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first_line = stdout.lines().next()?.trim();
    if first_line.starts_with('"') {
        let name = first_line
            .trim_start_matches('"')
            .split('"')
            .next()?
            .to_string();
        return Some(name);
    }
    None
}
