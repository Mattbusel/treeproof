//! A minimal MCP server over stdio (newline-delimited JSON-RPC 2.0).
//!
//! Tools: `receipt_run`, `receipt_query`, `receipt_verify`. The server never
//! writes anything but JSON-RPC messages to stdout; command output is captured.

use crate::config::Config;
use crate::run::{self, RunOptions};
use crate::{gate, keys, verify, Error, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub fn tools() -> Value {
    json!([
        {
            "name": "receipt_run",
            "title": "Run tests and sign a receipt",
            "description": "Run a test command under treeproof. Returns PASSED/WARNED/FAILED with test counts and a signed in-toto test-result receipt bound to the current git tree. Identical runs (same tree, command, toolchain, lockfiles) in any worktree are served from the shared cache. A run with 0 tests, an unrecognised runner, or a failing test is FAILED.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command line, e.g. \"cargo test --workspace\". Use either command or argv."},
                    "argv": {"type": "array", "items": {"type": "string"}, "description": "Program and arguments, run without a shell."},
                    "cwd": {"type": "string", "description": "Directory inside the repository (default: the project directory)."},
                    "no_cache": {"type": "boolean", "description": "Always execute, ignoring cached passes."},
                    "timeout_secs": {"type": "integer", "minimum": 1, "description": "Kill the command after this many seconds (default 1800)."}
                }
            }
        },
        {
            "name": "receipt_query",
            "title": "Is the current tree verified?",
            "description": "Report the current git tree digest, every receipt recorded for it, and whether the done-gate is satisfied (a valid passing receipt that meets the repository policy).",
            "inputSchema": {"type": "object", "properties": {"cwd": {"type": "string"}}}
        },
        {
            "name": "receipt_verify",
            "title": "Verify a receipt",
            "description": "Check a receipt's DSSE signature against trusted keys and its in-toto test-result statement, optionally require it to match the current tree, and optionally re-run its command to confirm the result.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "receipt": {"type": "string", "description": "Path to a receipt file, or the receipt JSON itself."},
                    "cwd": {"type": "string"},
                    "current_tree": {"type": "boolean", "description": "Require the receipt to be for the current working tree."},
                    "rerun": {"type": "boolean", "description": "Re-run the receipt's command and compare."}
                },
                "required": ["receipt"]
            }
        }
    ])
}

fn base_dir(args: &Value) -> PathBuf {
    if let Some(c) = args.get("cwd").and_then(|c| c.as_str()) {
        return PathBuf::from(c);
    }
    std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn call_tool(name: &str, args: &Value) -> Result<Value> {
    match name {
        "receipt_run" => {
            let (argv, shell) = if let Some(a) = args.get("argv").and_then(|a| a.as_array()) {
                (a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>(), false)
            } else if let Some(c) = args.get("command").and_then(|c| c.as_str()) {
                (vec![c.to_string()], true)
            } else {
                return Err(Error::new("give either command or argv"));
            };
            if argv.is_empty() || argv[0].trim().is_empty() {
                return Err(Error::new("empty command"));
            }
            let opts = RunOptions {
                argv,
                shell,
                cwd: base_dir(args),
                no_cache: args.get("no_cache").and_then(|v| v.as_bool()).unwrap_or(false),
                echo: false,
                allow_unrecognized: None,
                timeout: Some(Duration::from_secs(args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(1800).max(1))),
            };
            let o = run::run(&opts)?;
            let mut v = serde_json::to_value(&o)?;
            v["output_tail"] = json!(crate::util::tail(&o.output, 8000));
            v["receipt"] = serde_json::from_str(&o.receipt_json)?;
            Ok(v)
        }
        "receipt_query" => {
            let root = crate::git::repo_root(&base_dir(args))?;
            let cfg = Config::load(&root)?;
            Ok(serde_json::to_value(gate::evaluate(&root, &cfg, None)?)?)
        }
        "receipt_verify" => {
            let src = args.get("receipt").and_then(|r| r.as_str()).ok_or_else(|| Error::new("receipt is required"))?;
            let text = if src.trim_start().starts_with('{') {
                src.to_string()
            } else {
                std::fs::read_to_string(src).map_err(|e| Error::new(format!("read {src}: {e}")))?
            };
            let dir = base_dir(args);
            let root = crate::git::repo_root(&dir).ok();
            let trusted = keys::trusted_keys(root.as_deref(), true, &[])?;
            let want = if args.get("current_tree").and_then(|v| v.as_bool()).unwrap_or(false) {
                let r = root.as_ref().ok_or_else(|| Error::new("current_tree needs a git repository"))?;
                Some(crate::git::tree_digest(r)?)
            } else {
                None
            };
            let rerun_root = if args.get("rerun").and_then(|v| v.as_bool()).unwrap_or(false) {
                Some(root.clone().ok_or_else(|| Error::new("rerun needs a git repository"))?)
            } else {
                None
            };
            let rep = verify::verify_text(
                if src.len() > 200 { "inline" } else { src },
                &text,
                &trusted,
                want.as_deref(),
                rerun_root.as_deref(),
                false,
            );
            Ok(serde_json::to_value(rep)?)
        }
        _ => Err(Error::new(format!("unknown tool {name}"))),
    }
}

/// Handle one JSON-RPC message; returns the response to send, if any.
pub fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = id?; // notifications get no response
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let ok = |result: Value| json!({"jsonrpc": "2.0", "id": id.clone(), "result": result});
    let err = |code: i64, message: String| json!({"jsonrpc": "2.0", "id": id.clone(), "error": {"code": code, "message": message}});
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
            let version = if PROTOCOL_VERSIONS.contains(&asked) { asked } else { PROTOCOL_VERSIONS[0] };
            ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "treeproof", "title": "treeproof", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Use receipt_run to run the test suite instead of running tests directly; the result is a signed receipt for the current tree. Use receipt_query before claiming work is done."
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if !matches!(name, "receipt_run" | "receipt_query" | "receipt_verify") {
                return Some(err(-32602, format!("unknown tool: {name}")));
            }
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            match call_tool(name, &args) {
                Ok(v) => {
                    let failed =
                        v.get("ok").and_then(|o| o.as_bool()) == Some(false) || v.get("result").and_then(|r| r.as_str()) == Some("FAILED");
                    ok(json!({
                        "content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}],
                        "structuredContent": v,
                        "isError": false,
                        "_meta": {"treeproof/failed": failed}
                    }))
                }
                Err(e) => ok(json!({"content": [{"type": "text", "text": e.to_string()}], "isError": true})),
            }
        }
        "resources/list" => ok(json!({"resources": []})),
        "prompts/list" => ok(json!({"prompts": []})),
        _ => err(-32601, format!("method not found: {method}")),
    })
}

/// Serve on stdin/stdout until EOF.
pub fn serve() -> i32 {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let rs: Vec<Value> = batch.iter().filter_map(handle).collect();
                if rs.is_empty() {
                    None
                } else {
                    Some(Value::Array(rs))
                }
            }
            Ok(v) => handle(&v),
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}})),
        };
        if let Some(r) = resp {
            let _ = writeln!(out, "{r}");
            let _ = out.flush();
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_and_list() {
        let r = handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        let r = handle(&json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        assert!(handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        let r = handle(&json!({"jsonrpc":"2.0","id":3,"method":"tools/list"})).unwrap();
        let names: Vec<&str> = r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["receipt_run", "receipt_query", "receipt_verify"]);
        let r = handle(&json!({"jsonrpc":"2.0","id":4,"method":"nope"})).unwrap();
        assert_eq!(r["error"]["code"], -32601);
        let r = handle(&json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"receipt_run","arguments":{}}})).unwrap();
        assert_eq!(r["result"]["isError"], true);
    }
}
