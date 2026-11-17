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

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn commit_all(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
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
    // The policy applies once committed; the working copy is never the policy.
    commit_all(&e.repo, "policy");
    assert!(e.run_tests(&e.repo, &[]).status.success());
    let o = e.cmd(&e.repo).arg("gate").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("not one of the required commands"), "{}", stdout(&o));
    assert!(e.run_tests(&e.repo, &["full"]).status.success());
    let o = e.cmd(&e.repo).arg("gate").output().unwrap();
    assert!(o.status.success(), "{}", stdout(&o));
}

const CARGO_OK: &str = "test a ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";

#[test]
fn observed_runs_are_recorded_but_do_not_open_the_gate_by_default() {
    let e = Env::new();
    std::fs::write(e.repo.join("lib.txt"), "v9\n").unwrap();
    let out = e.hook(json!({
        "hook_event_name": "PostToolUse", "session_id": "s-obs", "cwd": e.repo, "tool_name": "Bash",
        "tool_input": {"command": "cargo test"},
        "tool_response": {"stdout": CARGO_OK, "stderr": "", "interrupted": false}
    }));
    assert!(out["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("PASSED"), "{out}");
    let o = e.cmd(&e.repo).args(["status", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["considered"][0]["why_not"].as_str().unwrap().contains("observed"), "{v}");

    // A committed policy can accept them.
    std::fs::write(e.repo.join("treeproof.toml"), "[gate]\naccept_observed = true\n").unwrap();
    commit_all(&e.repo, "accept observed");
    e.hook(json!({
        "hook_event_name": "PostToolUse", "session_id": "s-obs2", "cwd": e.repo, "tool_name": "Bash",
        "tool_input": {"command": "cargo test"},
        "tool_response": {"stdout": CARGO_OK}
    }));
    let o = e.cmd(&e.repo).args(["status", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["receipt"]["mode"], "observed");
}

#[test]
fn agent_cannot_fake_an_observed_pass_with_echo() {
    let e = Env::new();
    // Even where observed receipts are accepted, a command that prints its own "summary" is not recorded.
    std::fs::write(e.repo.join("treeproof.toml"), "[gate]\naccept_observed = true\n").unwrap();
    commit_all(&e.repo, "accept observed");
    let sid = "s-fake";
    e.hook(json!({"hook_event_name": "SessionStart", "session_id": sid, "cwd": e.repo, "source": "startup"}));
    std::fs::write(e.repo.join("lib.txt"), "v2\n").unwrap();
    let stop = json!({"hook_event_name": "Stop", "session_id": sid, "cwd": e.repo, "stop_hook_active": false});
    assert_eq!(e.hook(stop.clone())["decision"], "block");
    let fake = "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s";
    for cmd in [
        format!("echo cargo test; printf '{fake}'"),
        format!("cargo test --no-run && echo '{fake}'"),
        "cargo test | grep ok".to_string(),
        format!("printf '{fake}' # cargo test"),
        format!("python -c \"print('{fake}')\" pytest"),
    ] {
        let out = e.hook(json!({
            "hook_event_name": "PostToolUse", "session_id": sid, "cwd": e.repo, "tool_name": "Bash",
            "tool_input": {"command": cmd},
            "tool_response": {"stdout": fake, "stderr": "", "interrupted": false}
        }));
        assert_eq!(out, Value::Null, "{cmd}: {out}");
    }
    assert_eq!(e.hook(stop)["decision"], "block");
}

#[test]
fn working_copy_policy_cannot_turn_the_gate_off() {
    let e = Env::new();
    let sid = "s-policy";
    e.hook(json!({"hook_event_name": "SessionStart", "session_id": sid, "cwd": e.repo, "source": "startup"}));
    std::fs::write(e.repo.join("lib.txt"), "v2\n").unwrap();
    let stop = json!({"hook_event_name": "Stop", "session_id": sid, "cwd": e.repo, "stop_hook_active": false});
    assert_eq!(e.hook(stop.clone())["decision"], "block");
    for bad in [
        "[gate]\nmode = \"off\"\n",
        "[gate]\nmode = \"warn\"\n",
        "[gate]\nmax_blocks = 0\nmin_tests = 0\n",
        "[run]\nallow_unrecognized = true\n",
    ] {
        std::fs::write(e.repo.join("treeproof.toml"), bad).unwrap();
        let out = e.hook(stop.clone());
        assert_eq!(out["decision"], "block", "{bad}: {out}");
    }
    // Committing it during the session does not help either: the session keeps its snapshot.
    std::fs::write(e.repo.join("treeproof.toml"), "[gate]\nmode = \"off\"\n").unwrap();
    commit_all(&e.repo, "turn it off");
    assert_eq!(e.hook(stop.clone())["decision"], "block");
    // An unrecognised "test" command is not a pass, whatever the working copy says.
    std::fs::write(e.repo.join("treeproof.toml"), "[run]\nallow_unrecognized = true\n").unwrap();
    let mut c = e.cmd(&e.repo);
    c.args(["run", "--allow-unrecognized", "--", python(), "-c", "print('fine')"]);
    assert!(c.output().unwrap().status.success());
    let out = e.hook(stop.clone());
    assert_eq!(out["decision"], "block", "{out}");
    assert!(out["reason"].as_str().unwrap().contains("no test runner summary"), "{out}");

    // The agent's editing tools may not touch the policy.
    let out = e.hook(json!({"hook_event_name": "PreToolUse", "session_id": sid, "cwd": e.repo, "tool_name": "Write",
        "tool_input": {"file_path": e.repo.join("treeproof.toml"), "content": "[gate]\nmode = \"off\"\n"}}));
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny", "{out}");
    let out = e.hook(json!({"hook_event_name": "PreToolUse", "session_id": sid, "cwd": e.repo, "tool_name": "Bash",
        "tool_input": {"command": "printf '[gate]\\nmode = \"off\"' > treeproof.toml"}}));
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny", "{out}");
    let out = e.hook(json!({"hook_event_name": "PreToolUse", "session_id": sid, "cwd": e.repo, "tool_name": "Bash",
        "tool_input": {"command": "cat treeproof.toml"}}));
    assert_eq!(out, Value::Null);
}

#[test]
fn a_passing_subset_does_not_outweigh_a_failing_full_run() {
    let e = Env::new();
    std::fs::write(e.repo.join("lib.txt"), "v2\n").unwrap();
    let gate = |e: &Env| {
        let o = e.cmd(&e.repo).args(["gate"]).output().unwrap();
        (o.status.code(), stdout(&o))
    };
    // A name-filtered run alone is not enough.
    assert!(e.run_tests(&e.repo, &["only::adds"]).status.success());
    let (code, out) = gate(&e);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("subset"), "{out}");

    // The full suite passes (and is cached)...
    assert!(e.run_tests(&e.repo, &[]).status.success());
    assert_eq!(gate(&e).0, Some(0));
    // ...then fails on a re-run (a flaky test), and the agent runs only something narrower.
    let mut c = e.cmd(&e.repo);
    c.env("FAKE_FAIL", "1").args(["run", "--no-cache", "--", python(), "run_tests.py"]);
    assert!(!c.output().unwrap().status.success());
    let mut c = e.cmd(&e.repo);
    c.args(["run", "--", python(), "run_tests.py", "--fast"]);
    assert!(c.output().unwrap().status.success());
    let (code, out) = gate(&e);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("FAILED and no later run of the same command passed"), "{out}");

    // Re-running the same command and passing resolves it (a flaky test, or a fix outside the tree).
    let before = e.executions();
    assert!(e.run_tests(&e.repo, &[]).status.success());
    assert_eq!(e.executions(), before + 1, "a failure must withdraw the cached pass, not replay it");
    let (code, out) = gate(&e);
    assert_eq!(code, Some(0), "{out}");

    // A pinned exact command may carry a filter on purpose.
    std::fs::write(e.repo.join("treeproof.toml"), format!("[gate]\ncommands = [\"{} run_tests.py only::adds\"]\n", python())).unwrap();
    commit_all(&e.repo, "pin");
    assert!(e.run_tests(&e.repo, &["only::adds"]).status.success());
    let (code, out) = gate(&e);
    assert_eq!(code, Some(0), "{out}");
}

#[test]
fn baseline_is_recorded_by_the_first_tool_hook_and_commits_do_not_hide_changes() {
    let e = Env::new();
    // No SessionStart (Codex, or the plugin enabled mid-session): the first PreToolUse records the baseline.
    let sid = "neverstarted";
    let pre = json!({"hook_event_name": "PreToolUse", "session_id": sid, "turn_id": "t1", "cwd": e.repo, "tool_name": "Bash",
        "tool_input": {"command": "git commit -qam wip"}});
    assert_eq!(e.hook(pre), Value::Null);
    std::fs::write(e.repo.join("lib.txt"), "v2\n").unwrap();
    commit_all(&e.repo, "wip");
    let out = e.hook(json!({"hook_event_name": "Stop", "session_id": sid, "turn_id": "t1", "cwd": e.repo, "stop_hook_active": false}));
    assert_eq!(out["decision"], "block", "{out}");

    // With no hook record at all, the baseline is where the branch left its upstream.
    let remote = e.home.join("remote.git");
    git(&e.home, &["init", "-q", "--bare", remote.to_str().unwrap()]);
    git(&e.repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(&e.repo, &["push", "-q", "-u", "origin", "main"]);
    std::fs::write(e.repo.join("lib.txt"), "v3\n").unwrap();
    commit_all(&e.repo, "more");
    let out = e.hook(json!({"hook_event_name": "Stop", "session_id": "no-record", "cwd": e.repo, "stop_hook_active": false}));
    assert_eq!(out["decision"], "block", "{out}");
}

#[test]
fn edits_inside_a_nested_repository_invalidate_the_receipt() {
    let e = Env::new();
    let sub = e.repo.join("vendor_lib");
    std::fs::create_dir_all(&sub).unwrap();
    git(&sub, &["init", "-q", "-b", "main"]);
    git(&sub, &["config", "user.email", "t@example.com"]);
    git(&sub, &["config", "user.name", "t"]);
    std::fs::write(sub.join("code.txt"), "a\n").unwrap();
    commit_all(&sub, "sub");
    assert!(e.run_tests(&e.repo, &[]).status.success());
    let ok = e.cmd(&e.repo).arg("gate").output().unwrap();
    assert!(ok.status.success(), "{}", stdout(&ok));
    let tree_before = stdout(&e.cmd(&e.repo).arg("tree").output().unwrap());
    // The outer tree records only the nested HEAD, so it does not change...
    std::fs::write(sub.join("code.txt"), "b\n").unwrap();
    assert_eq!(tree_before, stdout(&e.cmd(&e.repo).arg("tree").output().unwrap()));
    // ...but the gate sees the nested content.
    let o = e.cmd(&e.repo).arg("gate").output().unwrap();
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(stdout(&o).contains("nested repository"), "{}", stdout(&o));
    // And the cache does not replay the old pass.
    let o = e.run_tests(&e.repo, &[]);
    assert!(stderr(&o).contains("fresh PASSED"), "{}", stderr(&o));
    assert!(e.cmd(&e.repo).arg("gate").output().unwrap().status.success());
}

#[test]
fn environment_values_are_not_published_as_plain_hashes() {
    use base64::Engine;
    use sha2::Digest;
    let e = Env::new();
    let mut c = e.cmd(&e.repo);
    c.env("PYTHONPATH", "postgres://user:hunter2@db/x").args(["run", "--json", "--", python(), "run_tests.py"]);
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    let err = stderr(&o);
    let receipt = err.lines().find_map(|l| l.strip_prefix("treeproof: receipt ")).unwrap().trim().to_string();
    let env: Value = serde_json::from_str(&std::fs::read_to_string(receipt).unwrap()).unwrap();
    let payload = base64::engine::general_purpose::STANDARD.decode(env["payload"].as_str().unwrap()).unwrap();
    let plain = sha2::Sha256::digest(b"postgres://user:hunter2@db/x").iter().map(|b| format!("{b:02x}")).collect::<String>();
    let text = String::from_utf8(payload).unwrap();
    assert!(!text.contains(&plain), "plain SHA-256 of an environment value is in the receipt");
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
fn ci_verify_trusts_base_keys_and_applies_base_policy() {
    let e = Env::new();
    // Base branch: a pinned command and the maintainer's public key.
    std::fs::write(e.repo.join("treeproof.toml"), format!("[gate]\ncommands = [\"{} run_tests.py\"]\n", python())).unwrap();
    let kdir = e.repo.join(".treeproof").join("trusted_keys");
    std::fs::create_dir_all(&kdir).unwrap();
    let pem = e.cmd(&e.repo).args(["key", "export"]).output().unwrap();
    std::fs::write(kdir.join("maintainer.pem"), &pem.stdout).unwrap();
    commit_all(&e.repo, "base");
    git(&e.repo, &["tag", "base"]);

    // A pull request signed by another key, with a command the policy does not allow, that
    // also adds its own key to the working copy.
    let attacker_key = e.home.join("attacker.key");
    std::fs::write(e.repo.join("lib.txt"), "pr change\n").unwrap();
    let mut c = e.cmd(&e.repo);
    c.env("TREEPROOF_KEY_FILE", &attacker_key).args([
        "run",
        "--",
        python(),
        "-c",
        "print('test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s')",
    ]);
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    let src = stderr(&o).lines().find_map(|l| l.strip_prefix("treeproof: receipt ").map(|s| s.trim().to_string())).unwrap();
    let rdir = e.repo.join(".treeproof").join("receipts");
    std::fs::create_dir_all(&rdir).unwrap();
    let receipt = rdir.join("pr.json");
    std::fs::copy(&src, &receipt).unwrap();
    let mut c = e.cmd(&e.repo);
    c.env("TREEPROOF_KEY_FILE", &attacker_key).args(["key", "export"]);
    std::fs::write(kdir.join("attacker.pem"), c.output().unwrap().stdout).unwrap();
    commit_all(&e.repo, "pr");

    let verify = |extra: &[&str]| {
        let mut c = e.cmd(&e.repo);
        c.args(["verify", "--no-local-key", "--tree", "current"]).args(extra).arg(&receipt);
        c.output().unwrap()
    };
    // Keys from the base commit: the PR's own key is not trusted.
    let o = verify(&["--keys-ref", "base", "--policy-ref", "base"]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(stdout(&o).contains("signature does not verify"), "{}", stdout(&o));
    // Even with the PR's key trusted, the base policy refuses the command.
    let o = verify(&["--policy-ref", "base"]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(stdout(&o).contains("not one of the required commands"), "{}", stdout(&o));
    // --tree current applies HEAD's policy by default.
    let o = verify(&[]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    // No keys at the given revision is an error, not a pass.
    let o = verify(&["--keys-ref", "HEAD~2"]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stderr(&o).contains("no trusted keys"), "{}", stderr(&o));

    // The maintainer's own receipt for the pinned command verifies against the base.
    let mut c = e.cmd(&e.repo);
    c.args(["run", "--", python(), "run_tests.py"]);
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    let src = stderr(&o).lines().find_map(|l| l.strip_prefix("treeproof: receipt ").map(|s| s.trim().to_string())).unwrap();
    std::fs::copy(&src, &receipt).unwrap();
    let o = verify(&["--keys-ref", "base", "--policy-ref", "base", "--rerun"]);
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
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
