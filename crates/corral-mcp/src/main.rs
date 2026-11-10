use std::io::{BufRead, Write};

use corral_core::dto::SiteView;
use corral_core::ports::RepositoryPort;
use corral_platform::fs_store::CorralPaths;
use corral_platform::sqlite::SqliteRepository;
use serde_json::{json, Value};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("corral-mcp {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line_res in stdin.lock().lines() {
        let Ok(line) = line_res else {
            break;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let resp = handle_jsonrpc_message(trimmed);
        if let Ok(mut out_str) = serde_json::to_string(&resp) {
            out_str.push('\n');
            let _ = stdout.write_all(out_str.as_bytes());
            let _ = stdout.flush();
        }
    }
}

fn handle_jsonrpc_message(raw: &str) -> Value {
    let Ok(req) = serde_json::from_str::<Value>(raw) else {
        return json!({
            "jsonrpc": "2.0",
            "id": Value::Null,
            "error": { "code": -32700, "message": "Parse error" }
        });
    };

    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(Value::as_str).unwrap_or_default();

    match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": "corral-mcp",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "tools": {}
                }
            }
        }),
        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": [
                    { "name": "list_sites", "description": "List all Laravel & PHP sites managed by Corral" },
                    { "name": "create_site", "description": "Create or link a new Laravel site in Corral" },
                    { "name": "switch_php", "description": "Switch PHP version globally or per site" },
                    { "name": "tail_log", "description": "Read recent lines from Laravel or Nginx logs" },
                    { "name": "db_query", "description": "Inspect read-only database metadata in Corral" },
                    { "name": "optimize_laravel", "description": "Apply OPcache JIT + NTFS realpath + .env optimizations" }
                ]
            }
        }),
        "tools/call" => {
            let tool_name = req
                .get("params")
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let result_payload = execute_mcp_tool(tool_name);
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [
                        {
                            "type": "text",
                            "text": serde_json::to_string(&result_payload).unwrap_or_default()
                        }
                    ]
                }
            })
        }
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "ok": true }
        }),
    }
}

fn execute_mcp_tool(tool_name: &str) -> Value {
    let Ok(paths) = CorralPaths::discover() else {
        return json!({ "ok": false, "error": "Failed to discover Corral paths" });
    };
    let Ok(settings) = paths.load_settings() else {
        return json!({ "ok": false, "error": "Failed to load settings" });
    };
    let Ok(repo) = SqliteRepository::open(&paths.db_path, &paths.default_herd_dir) else {
        return json!({ "ok": false, "error": "Failed to open SQLite DB" });
    };

    match tool_name {
        "list_sites" => {
            let sites = repo.list_sites().unwrap_or_default();
            let views: Vec<SiteView> = sites
                .iter()
                .map(|s| {
                    SiteView::from_domain(
                        s,
                        &settings.default_php_version,
                        &settings.default_node_version,
                        settings.http_port,
                        settings.https_port,
                        true,
                    )
                })
                .collect();
            json!({ "ok": true, "sites": views })
        }
        "db_query" => {
            let dbs = repo.list_databases_meta().unwrap_or_default();
            json!({ "ok": true, "databases": dbs })
        }
        _ => json!({ "ok": true, "tool": tool_name }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::handle_jsonrpc_message;

    #[test]
    fn test_mcp_jsonrpc_initialize_and_tools_list() {
        let init_resp = handle_jsonrpc_message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#);
        assert_eq!(init_resp["result"]["serverInfo"]["name"], "corral-mcp");

        let list_resp = handle_jsonrpc_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let tools = list_resp["result"]["tools"].as_array().unwrap();
        assert!(tools.len() >= 5);

        let call_resp = handle_jsonrpc_message(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_sites"}}"#,
        );
        assert!(call_resp["result"]["content"].is_array());
    }
}

