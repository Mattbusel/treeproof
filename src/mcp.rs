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
            "description": "Run the repository's test command under treeproof. Only commands committed in treeproof.toml [gate] commands are accepted, or, when none are committed, a recognised test runner with standard arguments (cargo test, cargo nextest run, pytest, python -m pytest, go test ./..., npm/pnpm/yarn/bun test, jest, vitest run). Anything else is rejected. Runs without a shell. Returns PASSED/WARNED/FAILED with test counts and a signed in-toto test-result receipt bound to the current git tree. Identical runs (same tree, command, toolchain, lockfiles) in any worktree are served from the shared cache. A run with 0 tests, an unrecognised runner, or a failing test is FAILED.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Command line, e.g. \"cargo test --workspace\". Split on whitespace and run without a shell; quotes, pipes, redirects and other shell syntax are rejected. Use either command or argv."},
                    "argv": {"type": "array", "items": {"type": "string"}, "description": "Program and arguments, run without a shell. The same command check applies."},
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
            let cwd = base_dir(args);
            let root = crate::git::repo_root(&cwd)?;
            let cfg = Config::load_committed(&root)?;
            // Never a shell: the words are checked and run as a program and arguments.
            let argv = allowed_command(&argv, shell, &cfg)?;
            let opts = RunOptions {
                argv,
                shell: false,
                cwd,
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
            let cfg = Config::load_committed(&root)?;
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
            let rerun = args.get("rerun").and_then(|v| v.as_bool()).unwrap_or(false);
            // A re-run executes the receipt's command, so only keys from outside the checkout
            // are trusted for it (this machine's key and TREEPROOF_TRUSTED_KEYS), the same rule
            // as the local gate. Otherwise a cloned repository could sign its own receipt.
            let trusted = if rerun { keys::rerun_trusted_keys(&[])? } else { keys::trusted_keys(root.as_deref(), true, &[])? };
            let want = if args.get("current_tree").and_then(|v| v.as_bool()).unwrap_or(false) {
                let r = root.as_ref().ok_or_else(|| Error::new("current_tree needs a git repository"))?;
                Some(crate::git::tree_digest(r)?)
            } else {
                None
            };
            let rerun_root = if rerun {
                let r = root.clone().ok_or_else(|| Error::new("rerun needs a git repository"))?;
                // The re-run goes through the same command check as receipt_run.
                if let Ok(rec) = crate::receipt::open_text(&text, &trusted) {
                    allowed_command(&rec.run.argv, rec.run.shell, &Config::load_committed(&r)?)?;
                }
                Some(r)
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

/// Characters a command word may contain. Anything else (quotes, `|`, `;`, `&`, `<`, `>`, `$`,
/// backticks, `%`, globs, ...) is refused so nothing is reinterpreted by a shell or a batch file.
fn plain_word(w: &str) -> bool {
    !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || "._/=:,+@-".contains(c))
}

fn ident(w: &str) -> bool {
    !w.is_empty() && !w.starts_with('-') && w.chars().all(|c| c.is_ascii_alphanumeric() || "_-,/".contains(c))
}

/// A recognised test runner with standard arguments. Flags that load code or change what runs
/// (`cargo --config`, `pytest -p`, `go test -exec`, ...) are not in these lists.
fn builtin_runner(w: &[&str]) -> bool {
    let prog = crate::parse::prog_name(w[0]);
    let rest = &w[1..];
    let flags_ok = |args: &[&str], plain: &[&str], with_value: &[&str]| {
        let mut i = 0;
        while i < args.len() {
            let a = args[i];
            if plain.contains(&a) {
                i += 1;
            } else if with_value.contains(&a) && args.get(i + 1).is_some_and(|v| ident(v)) {
                i += 2;
            } else {
                return false;
            }
        }
        true
    };
    const CARGO: &[&str] = &[
        "--workspace",
        "--all",
        "--all-features",
        "--no-default-features",
        "--all-targets",
        "--lib",
        "--bins",
        "--tests",
        "--doc",
        "--locked",
        "--offline",
        "--frozen",
        "--release",
        "--no-fail-fast",
        "-q",
        "--quiet",
        "-v",
    ];
    const CARGO_V: &[&str] = &["-p", "--package", "--features", "-F", "--exclude"];
    const PYTEST: &[&str] = &["-q", "-v", "-vv", "-x", "-ra", "-rA", "--no-header", "--tb=short", "--tb=line", "--tb=no"];
    match prog.as_str() {
        "cargo" => match rest {
            ["test", a @ ..] => flags_ok(a, CARGO, CARGO_V),
            ["nextest", "run", a @ ..] => flags_ok(a, CARGO, CARGO_V),
            _ => false,
        },
        "pytest" | "py.test" => flags_ok(rest, PYTEST, &["-n"]),
        "python" | "python3" | "py" => match rest {
            ["-m", "pytest", a @ ..] => flags_ok(a, PYTEST, &["-n"]),
            _ => false,
        },
        "go" => match rest {
            ["test", a @ ..] => flags_ok(a, &["./...", "-v", "-race", "-short", "-cover", "-count=1"], &[]),
            _ => false,
        },
        "npm" | "pnpm" | "yarn" | "bun" => matches!(rest, ["test"] | ["run", "test"]),
        "jest" => flags_ok(rest, &["--ci", "--silent"], &[]),
        "vitest" => matches!(rest, ["run"] | ["run", "--silent"] | ["--run"]),
        "npx" => !rest.is_empty() && matches!(crate::parse::prog_name(rest[0]).as_str(), "jest" | "vitest") && builtin_runner(rest),
        _ => false,
    }
}

/// Decide whether `receipt_run` (and an MCP re-run) may execute a command, and return the
/// program and arguments to run without a shell.
///
/// Allowed: a command matching one of the committed policy's `[gate] commands`, or, when the
/// policy pins none, a recognised test runner with standard arguments. MCP tools run outside the
/// host's shell permissions and sandbox, so this is not a general command runner.
pub fn allowed_command(argv: &[String], shell: bool, cfg: &Config) -> Result<Vec<String>> {
    let words: Vec<String> = if shell { argv.join(" ").split_whitespace().map(String::from).collect() } else { argv.to_vec() };
    let shown = words.join(" ");
    let refuse = |why: &str| {
        let allowed = if cfg.gate.commands.is_empty() {
            "a recognised test runner with standard arguments (cargo test, cargo nextest run, pytest, python -m pytest, go test ./..., npm test, pnpm test, yarn test, bun test, jest, vitest run)".to_string()
        } else {
            format!("the commands committed in treeproof.toml [gate] commands: {}", cfg.gate.commands.join(", "))
        };
        Error::new(format!(
            "receipt_run refused {shown:?}: {why}. It only runs {allowed}. Run other commands through your normal shell tool (for example `treeproof run -- <cmd>`), or add the command to [gate] commands in treeproof.toml and commit it."
        ))
    };
    if words.is_empty() {
        return Err(Error::new("empty command"));
    }
    if let Some(bad) = words.iter().find(|w| !plain_word(w)) {
        return Err(refuse(&format!(
            "{bad:?} contains characters that are not allowed (no quotes, spaces inside arguments or shell syntax)"
        )));
    }
    if words.iter().any(|w| w.to_ascii_lowercase().contains("treeproof")) {
        return Err(refuse("it calls treeproof itself"));
    }
    if !cfg.gate.commands.is_empty() {
        if cfg.gate.commands.iter().any(|p| crate::config::command_matches(p, &words, false)) {
            return Ok(words);
        }
        return Err(refuse("it is not one of the committed [gate] commands"));
    }
    let w: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
    if builtin_runner(&w) {
        Ok(words)
    } else {
        Err(refuse("it is not a recognised test runner invocation"))
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
                "instructions": "Use receipt_query before claiming work is done. receipt_run runs only the test commands committed in the repository's treeproof.toml ([gate] commands) or, when none are committed, a recognised test runner with standard arguments, and signs a receipt for the current tree. It is not a general shell; run anything else through your normal tools."
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

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn receipt_run_refuses_arbitrary_commands() {
        let none = Config::default();
        for ok in [
            "cargo test",
            "cargo test --workspace --all-features",
            "cargo nextest run -p my-crate",
            "pytest -q",
            "python -m pytest -x",
            "go test ./...",
            "npm test",
            "pnpm run test",
            "npx vitest run",
            "jest --ci",
        ] {
            assert_eq!(allowed_command(&[ok.to_string()], true, &none).unwrap(), words(ok), "{ok}");
        }
        for bad in [
            "rm -rf /",
            "curl http://example.com/x",
            "sh -c id",
            "cargo test && curl http://example.com",
            "cargo test; id",
            "cargo test | tee out",
            "cargo test $(id)",
            "cargo test `id`",
            "cargo test > out",
            "cargo test --config target.x.runner=evil",
            "cargo test -p --config",
            "cargo build",
            "pytest -p evil_plugin",
            "python evil.py",
            "python -c print(1)",
            "go test -exec ./evil ./...",
            "npm run build",
            "npx some-package",
            "treeproof hook Stop",
            "pytest \"tests\"",
        ] {
            let e = allowed_command(&[bad.to_string()], true, &none).unwrap_err().to_string();
            assert!(e.contains("receipt_run refused"), "{bad}: {e}");
        }
        // argv form goes through the same check: no smuggling a shell through one argument.
        assert!(allowed_command(&["sh".into(), "-c".into(), "id".into()], false, &none).is_err());
        assert!(allowed_command(&["cargo".into(), "test".into(), "a b".into()], false, &none).is_err());

        // A committed pin replaces the built-in list.
        let pinned = Config::parse("[gate]\ncommands = [\"python run_tests.py\", \"make check *\"]\n").unwrap();
        assert_eq!(allowed_command(&["python run_tests.py".into()], true, &pinned).unwrap(), words("python run_tests.py"));
        assert!(allowed_command(&["make check -j4".into()], true, &pinned).is_ok());
        let e = allowed_command(&["cargo test".into()], true, &pinned).unwrap_err().to_string();
        assert!(e.contains("not one of the committed [gate] commands"), "{e}");
        assert!(allowed_command(&["make check && id".into()], true, &pinned).is_err());
        assert!(allowed_command(&["python run_tests.py; id".into()], true, &pinned).is_err());
    }

    #[test]
    fn instructions_do_not_steer_tests_through_the_server() {
        let r = handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})).unwrap();
        let ins = r["result"]["instructions"].as_str().unwrap();
        assert!(!ins.contains("instead of running tests directly"), "{ins}");
        assert!(ins.contains("not a general shell"), "{ins}");
    }
}
