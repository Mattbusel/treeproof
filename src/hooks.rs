//! Hook handlers for Claude Code and Codex.
//!
//! Both hosts send one JSON object on stdin with `hook_event_name`, `session_id`
//! and `cwd`, and both accept `{"decision":"block","reason":...}` from Stop and
//! the `permissionDecision: "deny"` shape from PreToolUse, so one handler serves
//! both. Codex adds `turn_id`, which is how the host is detected.
//!
//! Hooks fail open: an internal error is reported to the user as a warning and
//! never blocks the agent, so a broken install cannot wedge a session.

use crate::config::Config;
use crate::fileguard;
use crate::otlp::{self, Evaluation, Label};
use crate::store::{write_atomic, Store};
use crate::{gate, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What a hook run produces: JSON for stdout (or nothing) and an exit code.
#[derive(Debug, Clone, PartialEq)]
pub struct HookOutput {
    pub stdout: Option<Value>,
    pub exit_code: i32,
}

impl HookOutput {
    fn allow_json() -> HookOutput {
        HookOutput { stdout: Some(json!({})), exit_code: 0 }
    }
    fn silent() -> HookOutput {
        HookOutput { stdout: None, exit_code: 0 }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct SessionState {
    start_tree: Option<String>,
    started_at: Option<String>,
    #[serde(default)]
    blocks: BTreeMap<String, u32>,
}

const GATE_KIND: &str = "gate";

fn load_state(store: &Store, session: &str) -> Option<SessionState> {
    let t = std::fs::read_to_string(store.session_path(GATE_KIND, session)).ok()?;
    serde_json::from_str(&t).ok()
}

fn save_state(store: &Store, session: &str, st: &SessionState) -> Result<()> {
    write_atomic(&store.session_path(GATE_KIND, session), serde_json::to_string(st)?.as_bytes())
}

fn str_field<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

/// `claude` or `codex`.
pub fn detect_host(input: &Value) -> &'static str {
    if input.get("turn_id").is_some() {
        "codex"
    } else {
        "claude"
    }
}

fn gate_mode(cfg: &Config) -> String {
    std::env::var("TREEPROOF_GATE").ok().filter(|m| matches!(m.as_str(), "block" | "warn" | "off")).unwrap_or_else(|| cfg.gate.mode.clone())
}

/// Handle one hook invocation. `event` overrides `hook_event_name` when given.
pub fn handle(input: &Value, event: Option<&str>) -> HookOutput {
    let ev = event.map(String::from).or_else(|| str_field(input, "hook_event_name").map(String::from)).unwrap_or_default();
    let res = match ev.as_str() {
        "SessionStart" => session_start(input),
        "Stop" | "SubagentStop" | "TaskCompleted" => stop(input, &ev),
        "PostToolUse" => post_tool_use(input),
        "PreToolUse" => pre_tool_use(input),
        _ => Ok(HookOutput::silent()),
    };
    match res {
        Ok(o) => o,
        Err(e) => HookOutput { stdout: Some(json!({"systemMessage": format!("treeproof hook error ({ev}): {e}")})), exit_code: 0 },
    }
}

fn cwd_of(input: &Value) -> PathBuf {
    str_field(input, "cwd").map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn session_of(input: &Value) -> String {
    str_field(input, "session_id").unwrap_or("default").to_string()
}

fn session_start(input: &Value) -> Result<HookOutput> {
    let cwd = cwd_of(input);
    let Ok(root) = crate::git::repo_root(&cwd) else { return Ok(HookOutput::silent()) };
    let store = Store::open()?;
    let sid = session_of(input);
    if load_state(&store, &sid).and_then(|s| s.start_tree).is_none() {
        let tree = crate::git::tree_digest(&root)?;
        let st = SessionState {
            start_tree: Some(tree),
            started_at: Some(crate::util::rfc3339_ms(crate::util::now_ms())),
            blocks: BTreeMap::new(),
        };
        save_state(&store, &sid, &st)?;
    }
    Ok(HookOutput::silent())
}

fn stop(input: &Value, ev: &str) -> Result<HookOutput> {
    let cwd = cwd_of(input);
    let Ok(root) = crate::git::repo_root(&cwd) else { return Ok(HookOutput::allow_json()) };
    let cfg = Config::load(&root)?;
    let mode = gate_mode(&cfg);
    if mode == "off" {
        return Ok(HookOutput::allow_json());
    }
    if ev == "SubagentStop" && std::env::var("TREEPROOF_GATE_SUBAGENTS").map(|v| v != "1").unwrap_or(true) {
        return Ok(HookOutput::allow_json());
    }
    let store = Store::open()?;
    let sid = session_of(input);
    let tree = crate::git::tree_digest(&root)?;
    let mut state = load_state(&store, &sid).unwrap_or_default();
    let baseline = state.start_tree.clone().or_else(|| crate::git::head_tree(&root));
    if baseline.as_deref() == Some(tree.as_str()) {
        return Ok(HookOutput::allow_json());
    }

    let report = gate::evaluate(&root, &cfg, Some(tree.clone()))?;
    emit_otel(&report, &tree);
    if report.ok {
        state.blocks.remove(&tree);
        let _ = save_state(&store, &sid, &state);
        return Ok(HookOutput::allow_json());
    }
    let n = state.blocks.entry(tree.clone()).or_insert(0);
    *n += 1;
    let count = *n;
    save_state(&store, &sid, &state)?;
    let msg = format!("treeproof: {}", report.reason);
    if mode == "warn" || count > cfg.gate.max_blocks {
        let why = if mode == "warn" { "gate is in warn mode".to_string() } else { format!("gave up after {} blocks", cfg.gate.max_blocks) };
        return Ok(HookOutput { stdout: Some(json!({"systemMessage": format!("{msg} ({why}; the tree is unverified)")})), exit_code: 0 });
    }
    Ok(HookOutput { stdout: Some(json!({"decision": "block", "reason": msg})), exit_code: 0 })
}

fn emit_otel(report: &gate::GateReport, tree: &str) {
    let chosen = report.receipt.as_ref();
    let any_failed = report.considered.iter().any(|b| b.result == "FAILED");
    let exit = match (chosen, any_failed) {
        (Some(_), _) => Evaluation {
            name: otlp::CHECK_EXIT_CODE,
            label: Label::Verified,
            explanation: "a signed receipt shows the test command passed on this tree".into(),
        },
        (None, true) => Evaluation {
            name: otlp::CHECK_EXIT_CODE,
            label: Label::Contradicted,
            explanation: "the latest receipt for this tree is a failure".into(),
        },
        (None, false) => {
            Evaluation { name: otlp::CHECK_EXIT_CODE, label: Label::Unknown, explanation: "no test run was recorded for this tree".into() }
        }
    };
    let fs = match chosen {
        Some(_) => Evaluation {
            name: otlp::CHECK_FS_DIFF,
            label: Label::Verified,
            explanation: "the receipt's tree digest equals the current working tree".into(),
        },
        None => Evaluation {
            name: otlp::CHECK_FS_DIFF,
            label: Label::Unknown,
            explanation: "no receipt matches the current working tree".into(),
        },
    };
    let rerun = Evaluation {
        name: otlp::CHECK_TEST_RERUN,
        label: Label::Unknown,
        explanation: "not re-run at stop time; see `treeproof verify --rerun`".into(),
    };
    let extra = vec![("vcs.ref.head.revision".to_string(), tree.to_string())];
    let _ = otlp::export_if_configured(&[exit, fs, rerun], &extra);
}

/// Text output of a Bash tool response, for either host.
pub fn tool_output_text(resp: &Value) -> String {
    match resp {
        Value::String(s) => s.clone(),
        Value::Object(m) => {
            let mut parts = Vec::new();
            for k in ["stdout", "stderr", "output", "aggregated_output", "formatted_output"] {
                if let Some(Value::String(s)) = m.get(k) {
                    parts.push(s.clone());
                }
            }
            if parts.is_empty() {
                if let Some(Value::Array(items)) = m.get("content") {
                    for it in items {
                        if let Some(t) = it.get("text").and_then(|t| t.as_str()) {
                            parts.push(t.to_string());
                        }
                    }
                }
            }
            parts.join("\n")
        }
        Value::Array(items) => items.iter().map(tool_output_text).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn tool_exit_code(resp: &Value) -> Option<i32> {
    for k in ["exit_code", "exitCode", "returnCode", "return_code"] {
        if let Some(c) = resp.get(k).and_then(|c| c.as_i64()) {
            return Some(c as i32);
        }
    }
    None
}

fn bash_command(input: &Value) -> Option<String> {
    match input.get("tool_input")?.get("command")? {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => Some(a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")),
        _ => None,
    }
}

/// Does a shell command look like it runs tests?
pub fn looks_like_test_command(cmd: &str) -> bool {
    let c = cmd.to_ascii_lowercase();
    if c.contains("treeproof") {
        return false;
    }
    [
        "cargo test",
        "cargo nextest",
        "pytest",
        "py.test",
        "jest",
        "vitest",
        "go test",
        "npm test",
        "npm run test",
        "pnpm test",
        "yarn test",
        "bun test",
        "tox",
        "nox",
    ]
    .iter()
    .any(|k| c.contains(k))
}

fn file_paths(input: &Value, cwd: &Path) -> Vec<PathBuf> {
    let tool = str_field(input, "tool_name").unwrap_or("");
    let ti = input.get("tool_input").cloned().unwrap_or(Value::Null);
    match tool {
        "apply_patch" => ti.get("command").and_then(|c| c.as_str()).map(|p| fileguard::patch_paths(p, cwd)).unwrap_or_default(),
        _ => ["file_path", "notebook_path", "path"]
            .iter()
            .filter_map(|k| ti.get(*k).and_then(|v| v.as_str()))
            .map(|p| {
                let p = Path::new(p);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    cwd.join(p)
                }
            })
            .collect(),
    }
}

fn fileguard_enabled(cwd: &Path) -> bool {
    if std::env::var("TREEPROOF_FILEGUARD").map(|v| v == "0" || v.eq_ignore_ascii_case("off")).unwrap_or(false) {
        return false;
    }
    match crate::git::repo_root(cwd) {
        Ok(root) => Config::load(&root).map(|c| c.fileguard.enabled).unwrap_or(true),
        Err(_) => true,
    }
}

fn post_tool_use(input: &Value) -> Result<HookOutput> {
    let tool = str_field(input, "tool_name").unwrap_or("");
    let cwd = cwd_of(input);
    let sid = session_of(input);
    match tool {
        "Bash" | "exec_command" | "shell" => {
            let Some(cmd) = bash_command(input) else { return Ok(HookOutput::silent()) };
            if !looks_like_test_command(&cmd) || crate::git::repo_root(&cwd).is_err() {
                return Ok(HookOutput::silent());
            }
            let resp = input.get("tool_response").cloned().unwrap_or(Value::Null);
            if resp.get("interrupted").and_then(|i| i.as_bool()) == Some(true) {
                return Ok(HookOutput::silent());
            }
            let text = tool_output_text(&resp);
            let host = detect_host(input);
            if let Some(o) = crate::run::record_observed(&cwd, &cmd, &text, tool_exit_code(&resp), host)? {
                let note = format!(
                    "treeproof recorded this run: {} on tree {} ({} passed, {} failed){}",
                    o.result,
                    &o.tree[..12],
                    o.counts.passed,
                    o.counts.failed,
                    o.reason.map(|r| format!(": {r}")).unwrap_or_default()
                );
                return Ok(HookOutput {
                    stdout: Some(json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": note}})),
                    exit_code: 0,
                });
            }
            Ok(HookOutput::silent())
        }
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "apply_patch" => {
            if fileguard_enabled(&cwd) {
                let store = Store::open()?;
                fileguard::record(&store, &sid, &file_paths(input, &cwd))?;
            }
            Ok(HookOutput::silent())
        }
        _ => Ok(HookOutput::silent()),
    }
}

fn deny(reason: String) -> HookOutput {
    HookOutput {
        stdout: Some(
            json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason}}),
        ),
        exit_code: 0,
    }
}

/// Shell commands an agent must not run: feeding the hook handler by hand (which would let it
/// record an "observed" pass it never ran) or touching the signing key.
pub fn forbidden_shell_command(cmd: &str) -> Option<&'static str> {
    let c = cmd.to_ascii_lowercase();
    if c.contains("treeproof hook") || c.contains("treeproof.exe hook") {
        return Some("treeproof hooks are run by the host, not by the agent");
    }
    if c.contains("signing.key") || c.contains("treeproof_key_file") || c.contains("treeproof-ed25519-seed") {
        return Some("the treeproof signing key is not for agents to read or replace");
    }
    if c.contains("treeproof_cache_dir") || c.contains("treeproof/v1/") || c.contains("treeproof\\v1\\") {
        return Some("the treeproof receipt store is written only by treeproof");
    }
    None
}

fn pre_tool_use(input: &Value) -> Result<HookOutput> {
    let tool = str_field(input, "tool_name").unwrap_or("");
    if matches!(tool, "Bash" | "exec_command" | "shell") {
        if let Some(why) = bash_command(input).as_deref().and_then(forbidden_shell_command) {
            return Ok(deny(format!("treeproof: {why}. Run tests with `treeproof run -- <command>` instead.")));
        }
        return Ok(HookOutput::silent());
    }
    if !matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "apply_patch") {
        return Ok(HookOutput::silent());
    }
    let cwd = cwd_of(input);
    let paths = file_paths(input, &cwd);
    let store = Store::open()?;
    let protected: Vec<PathBuf> = [Some(store.root().to_path_buf()), crate::keys::key_path().ok()].into_iter().flatten().collect();
    if paths.iter().any(|p| protected.iter().any(|q| under(p, q))) {
        return Ok(deny("treeproof: the signing key and receipt store are written only by treeproof.".to_string()));
    }
    if !fileguard_enabled(&cwd) {
        return Ok(HookOutput::silent());
    }
    let stale = fileguard::stale(&store, &session_of(input), &paths);
    if stale.is_empty() {
        return Ok(HookOutput::silent());
    }
    let list: Vec<String> = stale.iter().map(|p| p.display().to_string()).collect();
    Ok(deny(format!(
        "treeproof FileGuard: {} changed on disk since you last read it (another agent or process wrote it). Read it again and redo the edit against the current content.",
        list.join(", ")
    )))
}

fn norm_path(p: &Path) -> String {
    let abs = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").unwrap_or(&s).trim_end_matches('/').to_string();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// Is `p` equal to or inside `dir`?
fn under(p: &Path, dir: &Path) -> bool {
    let (a, b) = (norm_path(p), norm_path(dir));
    a == b || a.starts_with(&format!("{b}/"))
}

/// Read stdin, handle, print. Returns the process exit code.
pub fn main(event: Option<&str>) -> i32 {
    let mut buf = String::new();
    if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf) {
        eprintln!("treeproof hook: could not read stdin: {e}");
        return 0;
    }
    let input: Value = match serde_json::from_str(buf.trim()) {
        Ok(v) => v,
        Err(e) => {
            let err = Error::new(format!("hook input is not JSON: {e}"));
            println!("{}", json!({"systemMessage": format!("treeproof hook error: {err}")}));
            return 0;
        }
    };
    let out = handle(&input, event);
    if let Some(v) = out.stdout {
        println!("{v}");
    }
    out.exit_code
}
