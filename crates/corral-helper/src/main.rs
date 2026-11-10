mod pipe_server;
mod policy;

use std::path::PathBuf;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let appdata = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("C:\\ProgramData"))
        .join("Corral");

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("corral-helper {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--self-test") {
        println!("{{\"ok\":true,\"service\":\"CorralHelper\",\"pipe\":\"\\\\\\\\.\\\\pipe\\\\corral-helper-v1\"}}");
        return;
    }

    tracing::info!("Memulai CorralHelper Named Pipe Service...");
    if let Err(e) = pipe_server::run_helper_pipe_server(appdata).await {
        tracing::error!("CorralHelper berhenti dengan error: {e}");
    }
}
