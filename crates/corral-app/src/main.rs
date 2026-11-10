#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

fn main() {
    tracing_subscriber::fmt::init();
    install_panic_hook();
    if let Err(e) = corral_app_lib::run() {
        show_fatal_dialog(&e.to_string());
    }
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("Corral mengalami kesalahan fatal (panic): {info}");
        tracing::error!(target: "panic", "{msg}");
        persist_crash_report(&msg);
        show_fatal_dialog(&msg);
        default_hook(info);
    }));
}

fn persist_crash_report(report: &str) {
    let appdata = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("C:\\Users\\Public"));
    let crash_dir = appdata.join("Corral").join("crash");
    if std::fs::create_dir_all(&crash_dir).is_ok() {
        let ts = time::OffsetDateTime::now_utc().unix_timestamp();
        let file = crash_dir.join(format!("crash-{ts}.txt"));
        let _ = std::fs::write(file, report);
    }
}

fn show_fatal_dialog(message: &str) {
    let mut wide_title: Vec<u16> = "Corral — Fatal Error".encode_utf16().collect();
    wide_title.push(0);
    let mut wide_msg: Vec<u16> = message.encode_utf16().collect();
    wide_msg.push(0);
    // SAFETY: Passing null-terminated UTF-16 strings to Win32 `MessageBoxW`.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide_msg.as_ptr(),
            wide_title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
