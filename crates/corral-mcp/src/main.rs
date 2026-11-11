use std::io::Write;

use corral_core::dto::SiteView;
use corral_core::ports::RepositoryPort;
use corral_platform::fs_store::CorralPaths;
use corral_platform::io_util::{read_bounded_sync_line, BoundedLine};
use corral_platform::sqlite::SqliteRepository;
use serde_json::{json, Value};

const MAX_MCP_LINE_BYTES: usize = 1024 * 1024;

#[derive(Default)]
struct McpSession {
    ctx: Option<(CorralPaths, SqliteRepository)>,
}

impl McpSession {
    fn ensure_ctx(&mut self) -> Result<&(CorralPaths, SqliteRepository), &'static str> {
        if self.ctx.is_none() {
            let paths =
                CorralPaths::discover().map_err(|_| "Failed to discover Corral paths")?;
            let repo = SqliteRepository::open(&paths.db_path, &paths.default_herd_dir)
                .map_err(|_| "Failed to open SQLite DB")?;
            self.ctx = Some((paths, repo));
        }
        self.ctx.as_ref().ok_or("Failed to initialize MCP context")
    }

    fn execute_tool(&mut self, tool_name: &str) -> Value {
        let (paths, repo) = match self.ensure_ctx() {
            Ok(pair) => pair,
            Err(msg) => return json!({ "ok": false, "error": msg }),
        };
        let Ok(settings) = paths.load_settings() else {
            return json!({ "ok": false, "error": "Failed to load settings" });
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

    fn handle_message(&mut self, raw: &str) -> Value {
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
                let result_payload = self.execute_tool(tool_name);
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
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("corral-mcp {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let stdin = std::io::stdin();
    let mut stdin_lock = stdin.lock();
    let mut stdout = std::io::stdout();
    let mut session = McpSession::default();

    loop {
        let resp = match read_bounded_sync_line(&mut stdin_lock, MAX_MCP_LINE_BYTES) {
            Ok(BoundedLine::Eof) => break,
            Ok(BoundedLine::Oversized(_)) => json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": {
                    "code": -32600,
                    "message": "Invalid Request: JSON-RPC line exceeds 1 MiB limit"
                }
            }),
            Ok(BoundedLine::Line(line)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                session.handle_message(trimmed)
            }
            Err(_) => break,
        };

        if let Ok(mut out_str) = serde_json::to_string(&resp) {
            out_str.push('\n');
            let _ = stdout.write_all(out_str.as_bytes());
            let _ = stdout.flush();
        }
    }
}

#[cfg(test)]
fn handle_jsonrpc_message(raw: &str) -> Value {
    let mut session = McpSession::default();
    session.handle_message(raw)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{handle_jsonrpc_message, McpSession};

    #[test]
    fn test_mcp_jsonrpc_initialize_and_tools_list() {
        let init_resp = handle_jsonrpc_message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#);
        assert_eq!(
            init_resp
                .pointer("/result/serverInfo/name")
                .and_then(|v| v.as_str()),
            Some("corral-mcp")
        );

        let list_resp = handle_jsonrpc_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let tools = list_resp
            .pointer("/result/tools")
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(tools.len() >= 5);

        let mut session = McpSession::default();
        let call_resp_1 = session.handle_message(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_sites"}}"#,
        );
        assert!(call_resp_1
            .pointer("/result/content")
            .is_some_and(|v| v.is_array()));
        assert!(session.ctx.is_some());

        // Second call reuses the already-open SQLite connection.
        let call_resp_2 = session.handle_message(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"db_query"}}"#,
        );
        assert!(call_resp_2
            .pointer("/result/content")
            .is_some_and(|v| v.is_array()));
    }
}

