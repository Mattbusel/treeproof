//! End-to-end tests against the real binary, real git, and isolated key/cache directories.
//!
//! These are the reference tests from the design: a fabricated "all passed" stop is
//! blocked, parallel worktrees share results, a 0-test run fails, a tampered receipt
//! fails verification, and a re-run that disagrees with the receipt fails.

use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_treeproof");

fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

/// A fake test runner that prints libtest-style output. Each execution appends a line to
/// $FAKE_COUNTER so tests can count real executions.
const RUNNER: &str = include_str!("fixtures/run_tests.py");

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    repo: PathBuf,
    counter: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let o = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

impl Env {
    fn new() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "t"]);
        git(&repo, &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.join("run_tests.py"), RUNNER).unwrap();
        std::fs::write(repo.join("lib.txt"), "v1\n").unwrap();
        std::fs::write(repo.join(".gitignore"), "ignored/\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        let counter = tmp.path().join("counter.txt");
        Env { _tmp: tmp, home, repo, counter }
    }

    fn cmd(&self, dir: &Path) -> Command {
        let mut c = Command::new(BIN);
        c.current_dir(dir);
        c.env("TREEPROOF_CACHE_DIR", self.home.join("cache"));
        c.env("TREEPROOF_KEY_FILE", self.home.join("key"));
        c.env("FAKE_COUNTER", &self.counter);
        c.env_remove("TREEPROOF_TRUSTED_KEYS");
        c.env_remove("TREEPROOF_GATE");
        c.env_remove("FAKE_FAIL");
        c.env_remove("OTEL_EXPORTER_OTLP_ENDPOINT");
        c.env_remove("TREEPROOF_OTLP_ENDPOINT");
        c
    }

    fn run_tests(&self, dir: &Path, extra: &[&str]) -> Output {
        let mut c = self.cmd(dir);
        c.args(["run", "--", python(), "run_tests.py"]).args(extra);
        c.output().unwrap()
    }

    fn executions(&self) -> usize {
        std::fs::read_to_string(&self.counter).map(|s| s.lines().count()).unwrap_or(0)
    }

    fn hook(&self, input: Value) -> Value {
        let mut c = self.cmd(&self.repo);
        c.arg("hook").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        let o = child.wait_with_output().unwrap();
        assert!(o.status.success(), "hook failed: {}", String::from_utf8_lossy(&o.stderr));
        let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if s.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&s).unwrap()
        }
    }
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

#[test]
fn pass_is_signed_and_cached_failures_are_not() {
    let e = Env::new();
    let o = e.run_tests(&e.repo, &[]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stderr(&o).contains("fresh PASSED"), "{}", stderr(&o));
    assert_eq!(e.executions(), 1);

    let o = e.run_tests(&e.repo, &[]);
    assert!(o.status.success());
    assert!(stderr(&o).contains("cache_hit PASSED"), "{}", stderr(&o));
    // The cached output is replayed.
    assert!(String::from_utf8_lossy(&o.stdout).contains("test result: ok. 3 passed"));
    assert_eq!(e.executions(), 1);

    // A failing run is never served from the cache: each one executes.
    let mut c = e.cmd(&e.repo);
    c.env("FAKE_FAIL", "1").args(["run", "--no-cache", "--", python(), "run_tests.py"]);
    let o = c.output().unwrap();
    assert_eq!(o.status.code(), Some(101), "{}", stderr(&o));
    assert!(stderr(&o).contains("FAILED"));
    let mut c = e.cmd(&e.repo);
    c.env("FAKE_FAIL", "1").args(["run", "--", python(), "run_tests.py", "again"]);
    let before = e.executions();
    let o = c.output().unwrap();
    assert!(!o.status.success());
    let mut c = e.cmd(&e.repo);
    c.env("FAKE_FAIL", "1").args(["run", "--", python(), "run_tests.py", "again"]);
    let o2 = c.output().unwrap();
    assert!(!o2.status.success());
    assert_eq!(e.executions(), before + 2, "failed runs must execute every time");
}

#[test]
fn zero_tests_and_collect_only_fail() {
    let e = Env::new();
    let o = e.run_tests(&e.repo, &["zero"]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("0 tests ran"), "{}", stderr(&o));

    let o = e.run_tests(&e.repo, &["--collect-only"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("not a test run"), "{}", stderr(&o));

    // Unrecognised output fails unless explicitly allowed.
    let mut c = e.cmd(&e.repo);
    c.args(["run", "--", python(), "-c", "print('all good')"]);
    let o = c.output().unwrap();
    assert!(!o.status.success());
    assert!(stderr(&o).contains("no test summary recognised"), "{}", stderr(&o));
}

#[test]
fn worktrees_share_results_and_untracked_files_miss() {
    let e = Env::new();
    let mut wts = vec![e.repo.clone()];
    for i in 1..4 {
        let p = e.repo.parent().unwrap().join(format!("wt{i}"));
        git(&e.repo, &["worktree", "add", "-q", p.to_str().unwrap(), "-b", &format!("b{i}")]);
        wts.push(p);
    }
    // Cold start: all four in parallel. Single-flight means one execution.
    let children: Vec<_> = wts
        .iter()
        .map(|w| {
            let mut c = e.cmd(w);
            c.env("FAKE_SLEEP", "1.5").args(["run", "--", python(), "run_tests.py"]);
            c.stdout(Stdio::piped()).stderr(Stdio::piped());
            c.spawn().unwrap()
        })
        .collect();
    let outs: Vec<Output> = children.into_iter().map(|c| c.wait_with_output().unwrap()).collect();
    for o in &outs {
        assert!(o.status.success(), "{}", stderr(o));
    }
    let fresh = outs.iter().filter(|o| stderr(o).contains("fresh PASSED")).count();
    let hits = outs.iter().filter(|o| stderr(o).contains("cache_hit PASSED")).count();
    assert_eq!((fresh, hits), (1, 3), "{:?}", outs.iter().map(stderr).collect::<Vec<_>>());
    assert_eq!(e.executions(), 1);

    // An untracked (non-ignored) file in one worktree changes its tree: a miss there only.
    std::fs::write(wts[2].join("scratch.txt"), "new").unwrap();
    let o = e.run_tests(&wts[2], &[]);
    assert!(stderr(&o).contains("fresh PASSED"), "{}", stderr(&o));
    assert_eq!(e.executions(), 2);
    // Ignored files do not change the tree.
    std::fs::create_dir_all(wts[3].join("ignored")).unwrap();
    std::fs::write(wts[3].join("ignored").join("x.log"), "noise").unwrap();
    let o = e.run_tests(&wts[3], &[]);
    assert!(stderr(&o).contains("cache_hit PASSED"), "{}", stderr(&o));
    assert_eq!(e.executions(), 2);
}

#[test]
fn tree_digest_matches_git_for_a_clean_checkout() {
    let e = Env::new();
    let o = e.cmd(&e.repo).arg("tree").output().unwrap();
    let ours = String::from_utf8_lossy(&o.stdout).trim().to_string();
    let head = Command::new("git").arg("-C").arg(&e.repo).args(["rev-parse", "HEAD^{tree}"]).output().unwrap();
    assert_eq!(ours, String::from_utf8_lossy(&head.stdout).trim());
    // A modification changes it; reverting restores it.
    std::fs::write(e.repo.join("lib.txt"), "v2\n").unwrap();
    let o2 = e.cmd(&e.repo).arg("tree").output().unwrap();
    assert_ne!(String::from_utf8_lossy(&o2.stdout).trim(), ours);
    std::fs::write(e.repo.join("lib.txt"), "v1\n").unwrap();
    let o3 = e.cmd(&e.repo).arg("tree").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o3.stdout).trim(), ours);
    // The real index was not touched.
    let st = Command::new("git").arg("-C").arg(&e.repo).args(["status", "--porcelain"]).output().unwrap();
    assert!(String::from_utf8_lossy(&st.stdout).trim().is_empty());
}

#[test]
fn same_size_edits_right_after_a_commit_change_the_tree() {
    // Git's racy-clean case: a same-size rewrite within the same second as the index write.
    for round in 0..5 {
        let e = Env::new();
        let before = e.cmd(&e.repo).arg("tree").output().unwrap();
        std::fs::write(
            e.repo.join("lib.txt"),
            "v2
",
        )
        .unwrap();
        let after = e.cmd(&e.repo).arg("tree").output().unwrap();
        assert_ne!(before.stdout, after.stdout, "round {round}: same-size edit not seen");
        std::fs::write(
            e.repo.join("lib.txt"),
            "v3
",
        )
        .unwrap();
        let again = e.cmd(&e.repo).arg("tree").output().unwrap();
        assert_ne!(after.stdout, again.stdout, "round {round}: second same-size edit not seen");
    }
}

#[test]
fn fabricated_claim_is_blocked_until_a_real_pass() {
    let e = Env::new();
    let sid = "session-fabricated";
    assert_eq!(e.hook(json!({"hook_event_name": "SessionStart", "session_id": sid, "cwd": e.repo, "source": "startup"})), Value::Null);

    // No changes yet: stopping is fine.
    let stop = json!({"hook_event_name": "Stop", "session_id": sid, "cwd": e.repo, "stop_hook_active": false,
        "last_assistant_message": "Done. All tests pass."});
    assert_eq!(e.hook(stop.clone()), json!({}));

    // The agent edits code and claims success without running anything.
    std::fs::write(e.repo.join("lib.txt"), "v2\n").unwrap();
    let out = e.hook(stop.clone());
    assert_eq!(out["decision"], "block", "{out}");
    assert!(out["reason"].as_str().unwrap().contains("treeproof run --"));

    // A failing run does not satisfy the gate.
    let mut c = e.cmd(&e.repo);
    c.env("FAKE_FAIL", "1").args(["run", "--", python(), "run_tests.py"]);
    assert!(!c.output().unwrap().status.success());
    let out = e.hook(stop.clone());
    assert_eq!(out["decision"], "block");
    assert!(out["reason"].as_str().unwrap().contains("FAILED"), "{out}");

    // A real pass does.
    assert!(e.run_tests(&e.repo, &[]).status.success());
    assert_eq!(e.hook(stop.clone()), json!({}));

    // TaskCompleted uses the same gate; after another edit it blocks again.
    std::fs::write(e.repo.join("lib.txt"), "v3\n").unwrap();
    let out = e.hook(json!({"hook_event_name": "TaskCompleted", "session_id": sid, "cwd": e.repo, "task_id": "t1", "task_subject": "x"}));
    assert_eq!(out["decision"], "block");

    // Loop protection: after max_blocks the stop is allowed with a warning.
    let mut last = Value::Null;
    for _ in 0..4 {
        last = e.hook(stop.clone());
    }
    assert!(last.get("decision").is_none(), "{last}");
    assert!(last["systemMessage"].as_str().unwrap().contains("unverified"));
}

#[test]
fn policy_requires_the_configured_command() {
    let e = Env::new();
    std::fs::write(e.repo.join("treeproof.toml"), format!("[gate]\ncommands = [\"{} run_tests.py full\"]\n", python())).unwrap();
    assert!(e.run_tests(&e.repo, &[]).status.success());
    let o = e.cmd(&e.repo).arg("gate").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stdout).contains("not one of the required commands"));
    assert!(e.run_tests(&e.repo, &["full"]).status.success());
    let o = e.cmd(&e.repo).arg("gate").output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
}

#[test]
fn observed_bash_run_is_recorded_by_post_tool_use() {
    let e = Env::new();
    std::fs::write(e.repo.join("lib.txt"), "v9\n").unwrap();
    let out = e.hook(json!({
        "hook_event_name": "PostToolUse", "session_id": "s-obs", "cwd": e.repo, "tool_name": "Bash",
        "tool_input": {"command": "cargo test"},
        "tool_response": {"stdout": "test a ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n", "stderr": "", "interrupted": false}
    }));
    assert!(out["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("PASSED"), "{out}");
    let o = e.cmd(&e.repo).args(["status", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["receipt"]["mode"], "observed");
    // Policy can refuse observed receipts.
    std::fs::write(e.repo.join("treeproof.toml"), "[gate]\naccept_observed = false\n").unwrap();
    // (the policy file itself changes the tree, so re-record an observation for the new tree)
    e.hook(json!({
        "hook_event_name": "PostToolUse", "session_id": "s-obs", "cwd": e.repo, "tool_name": "Bash",
        "tool_input": {"command": "cargo test"},
        "tool_response": {"stdout": "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n"}
    }));
    let o = e.cmd(&e.repo).arg("gate").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
}

fn export(e: &Env) -> PathBuf {
    assert!(e.run_tests(&e.repo, &[]).status.success());
    let o = e.cmd(&e.repo).arg("export").output().unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    PathBuf::from(String::from_utf8_lossy(&o.stdout).trim())
}

#[test]
fn export_verify_and_tamper() {
    let e = Env::new();
    let receipt = export(&e);
    // Exporting into .treeproof/ does not change the tree digest.
    let o = e.cmd(&e.repo).args(["verify", "--tree", "current", receipt.to_str().unwrap()]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));

    // Trust only the committed public key, not the local signing key.
    let o = e.cmd(&e.repo).args(["verify", "--no-local-key", "--tree", "current", receipt.to_str().unwrap()]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));

    // Tamper with the payload: flip a count and re-encode.
    use base64::Engine;
    let text = std::fs::read_to_string(&receipt).unwrap();
    let mut env: Value = serde_json::from_str(&text).unwrap();
    let payload = base64::engine::general_purpose::STANDARD.decode(env["payload"].as_str().unwrap()).unwrap();
    let forged = String::from_utf8(payload).unwrap().replace("\"passed\":3", "\"passed\":300");
    env["payload"] = json!(base64::engine::general_purpose::STANDARD.encode(forged));
    let bad = e.repo.join("tampered.json");
    std::fs::write(&bad, env.to_string()).unwrap();
    let o = e.cmd(&e.repo).args(["verify", bad.to_str().unwrap()]).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stdout).contains("signature does not verify"));

    // A receipt for an older tree is not accepted for the current one.
    std::fs::remove_file(&bad).unwrap();
    std::fs::write(e.repo.join("lib.txt"), "changed\n").unwrap();
    let o = e.cmd(&e.repo).args(["verify", "--tree", "current", receipt.to_str().unwrap()]).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stdout).contains("no receipt for tree"));
}

#[test]
fn rerun_mismatch_fails() {
    let e = Env::new();
    let receipt = export(&e);
    let o = e.cmd(&e.repo).args(["verify", "--rerun", "--tree", "current", receipt.to_str().unwrap()]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
    let mut c = e.cmd(&e.repo);
    c.env("FAKE_FAIL", "1").args(["verify", "--rerun", "--tree", "current", receipt.to_str().unwrap()]);
    let o = c.output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("receipt says PASSED but the re-run is FAILED"),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
}

#[test]
fn fileguard_denies_stale_edit() {
    let e = Env::new();
    let f = e.repo.join("lib.txt");
    let sid = "s-fg";
    e.hook(json!({"hook_event_name": "PostToolUse", "session_id": sid, "cwd": e.repo, "tool_name": "Read", "tool_input": {"file_path": f}, "tool_response": {}}));
    let pre = json!({"hook_event_name": "PreToolUse", "session_id": sid, "cwd": e.repo, "tool_name": "Edit", "tool_input": {"file_path": f, "old_string": "v1", "new_string": "v2"}});
    assert_eq!(e.hook(pre.clone()), Value::Null);
    std::fs::write(&f, "someone else wrote this\n").unwrap();
    let out = e.hook(pre.clone());
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny", "{out}");
    // Reading again clears it.
    e.hook(json!({"hook_event_name": "PostToolUse", "session_id": sid, "cwd": e.repo, "tool_name": "Read", "tool_input": {"file_path": f}, "tool_response": {}}));
    assert_eq!(e.hook(pre), Value::Null);
    // Codex apply_patch paths are checked too.
    e.hook(json!({"hook_event_name": "PostToolUse", "session_id": sid, "turn_id": "t", "cwd": e.repo, "tool_name": "apply_patch", "tool_input": {"command": "*** Begin Patch\n*** Update File: lib.txt\n*** End Patch\n"}, "tool_response": ""}));
    std::fs::write(&f, "changed again\n").unwrap();
    let out = e.hook(json!({"hook_event_name": "PreToolUse", "session_id": sid, "turn_id": "t", "cwd": e.repo, "tool_name": "apply_patch", "tool_input": {"command": "*** Begin Patch\n*** Update File: lib.txt\n@@\n-a\n+b\n*** End Patch\n"}}));
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny", "{out}");
}

#[test]
fn agents_cannot_feed_hooks_or_touch_the_key() {
    let e = Env::new();
    for cmd in ["echo '{}' | treeproof hook PostToolUse", "cat ~/.config/treeproof/signing.key"] {
        let out = e.hook(
            json!({"hook_event_name": "PreToolUse", "session_id": "s", "cwd": e.repo, "tool_name": "Bash", "tool_input": {"command": cmd}}),
        );
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny", "{cmd}: {out}");
    }
    let ok = e.hook(json!({"hook_event_name": "PreToolUse", "session_id": "s", "cwd": e.repo, "tool_name": "Bash", "tool_input": {"command": "cargo test"}}));
    assert_eq!(ok, Value::Null);
    let key = e.home.join("key");
    let out = e.hook(json!({"hook_event_name": "PreToolUse", "session_id": "s", "cwd": e.repo, "tool_name": "Write", "tool_input": {"file_path": key, "content": "x"}}));
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny", "{out}");
}

#[test]
fn mcp_server_runs_queries_and_verifies() {
    let e = Env::new();
    let mut c = e.cmd(&e.repo);
    c.arg("mcp").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let cmdline = format!("{} run_tests.py", python());
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"receipt_run","arguments":{"command": cmdline, "cwd": e.repo}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"receipt_query","arguments":{"cwd": e.repo}}}),
    ];
    for m in &msgs {
        writeln!(stdin, "{m}").unwrap();
    }
    drop(stdin);
    let o = child.wait_with_output().unwrap();
    let lines: Vec<Value> = String::from_utf8_lossy(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 3, "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "treeproof");
    let run = &lines[1]["result"]["structuredContent"];
    assert_eq!(run["result"], "PASSED", "{run}");
    assert_eq!(run["counts"]["passed"], 3);
    let q = &lines[2]["result"]["structuredContent"];
    assert_eq!(q["ok"], true, "{q}");
    // Verify the receipt JSON the run returned.
    let receipt = run["receipt"].to_string();
    let mut c = e.cmd(&e.repo);
    c.arg("mcp").stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"receipt_verify","arguments":{"receipt": receipt, "cwd": e.repo, "current_tree": true}}})).unwrap();
    drop(stdin);
    let o = child.wait_with_output().unwrap();
    let v: Value = serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().next().unwrap()).unwrap();
    assert_eq!(v["result"]["structuredContent"]["ok"], true, "{v}");
}

#[test]
fn otel_dry_run_shapes_events() {
    let e = Env::new();
    assert!(e.run_tests(&e.repo, &[]).status.success());
    let o = e.cmd(&e.repo).args(["otel", "--dry-run"]).output().unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    let recs = v["resourceLogs"][0]["scopeLogs"][0]["logRecords"].as_array().unwrap();
    let labels: Vec<(String, String)> = recs
        .iter()
        .map(|r| {
            let attrs = r["attributes"].as_array().unwrap();
            let get = |k: &str| attrs.iter().find(|a| a["key"] == k).unwrap()["value"]["stringValue"].as_str().unwrap().to_string();
            (get("gen_ai.evaluation.name"), get("gen_ai.evaluation.score.label"))
        })
        .collect();
    assert_eq!(
        labels,
        vec![
            ("verification.exit_code".to_string(), "verified".to_string()),
            ("verification.fs_diff".to_string(), "verified".to_string()),
            ("verification.test_rerun".to_string(), "unknown".to_string()),
        ]
    );
}
