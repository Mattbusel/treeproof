//! `treeproof run`: execute a test command once per identical input, sign the result.

use crate::config::Config;
use crate::context::Context;
use crate::dsse::PublicKey;
use crate::fingerprint::{self, Fingerprint};
use crate::parse::{self, Summary};
use crate::receipt::{self, Counts, RunInfo, MODE_FRESH, MODE_OBSERVED};
use crate::store::Store;
use crate::util::{now_ms, rfc3339_ms, tail};
use crate::{Error, Result};
use ed25519_dalek::SigningKey;
use serde::Serialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Output kept in memory for parsing (the tail, if a run prints more).
pub const MAX_CAPTURE: usize = 32 * 1024 * 1024;
/// Output tail stored with a cached pass and replayed on a hit.
pub const MAX_LOG: usize = 256 * 1024;

/// Options for [`run`]. Build one with [`RunOptions::new`] and the `with_*` methods so
/// new options in later versions do not break your code.
#[derive(Clone, Debug)]
pub struct RunOptions {
    pub argv: Vec<String>,
    /// Run `argv[0]` through the platform shell (`sh -c` / `cmd /C`).
    pub shell: bool,
    pub cwd: PathBuf,
    pub no_cache: bool,
    /// Stream the command's output to this process's stdout/stderr (CLI) or capture only (MCP).
    pub echo: bool,
    pub allow_unrecognized: Option<bool>,
    pub timeout: Option<Duration>,
}

impl RunOptions {
    /// Run `argv` (program and arguments, no shell) in `cwd`, with the cache on, output
    /// captured rather than echoed, policy defaults, and no timeout.
    pub fn new(argv: Vec<String>, cwd: impl Into<PathBuf>) -> RunOptions {
        RunOptions { argv, shell: false, cwd: cwd.into(), no_cache: false, echo: false, allow_unrecognized: None, timeout: None }
    }

    /// Run one command line through the platform shell (`sh -c` / `cmd /C`).
    pub fn shell_line(line: impl Into<String>, cwd: impl Into<PathBuf>) -> RunOptions {
        RunOptions { shell: true, ..RunOptions::new(vec![line.into()], cwd) }
    }

    pub fn with_no_cache(mut self, v: bool) -> RunOptions {
        self.no_cache = v;
        self
    }

    pub fn with_echo(mut self, v: bool) -> RunOptions {
        self.echo = v;
        self
    }

    pub fn with_timeout(mut self, t: Option<Duration>) -> RunOptions {
        self.timeout = t;
        self
    }

    pub fn with_allow_unrecognized(mut self, v: Option<bool>) -> RunOptions {
        self.allow_unrecognized = v;
        self
    }
}

/// The in-toto result of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum TestResult {
    Passed,
    Warned,
    Failed,
}

impl TestResult {
    pub fn parse(s: &str) -> Option<TestResult> {
        match s {
            "PASSED" => Some(TestResult::Passed),
            "WARNED" => Some(TestResult::Warned),
            "FAILED" => Some(TestResult::Failed),
            _ => None,
        }
    }

    /// `PASSED` or `WARNED`.
    pub fn is_pass(self) -> bool {
        !matches!(self, TestResult::Failed)
    }
}

/// How a run's receipt was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RunStatus {
    /// The command was executed.
    Fresh,
    /// An identical passing run was served from the store.
    CacheHit,
    /// Recorded from a host's own tool output.
    Observed,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunOutcome {
    /// `fresh` or `cache_hit`.
    pub status: String,
    pub result: String,
    pub reason: Option<String>,
    pub tree: String,
    pub key: String,
    pub counts: Counts,
    pub runners: Vec<String>,
    pub notes: Vec<String>,
    /// Exit code `treeproof run` should exit with.
    pub exit_code: i32,
    pub receipt_path: PathBuf,
    #[serde(skip)]
    pub receipt_json: String,
    #[serde(skip)]
    pub output: String,
}

impl RunOutcome {
    /// The result as an enum (`result` holds the same value as a string).
    pub fn test_result(&self) -> TestResult {
        TestResult::parse(&self.result).unwrap_or(TestResult::Failed)
    }

    /// The status as an enum (`status` holds the same value as a string).
    pub fn run_status(&self) -> RunStatus {
        match self.status.as_str() {
            "cache_hit" => RunStatus::CacheHit,
            "observed" => RunStatus::Observed,
            _ => RunStatus::Fresh,
        }
    }

    /// True for `PASSED` or `WARNED`.
    pub fn passed(&self) -> bool {
        self.test_result().is_pass()
    }
}

/// Resolve a bare program name the way a shell would, including `PATHEXT` on Windows,
/// so `npm` finds `npm.cmd`.
pub fn resolve_program(prog: &str) -> PathBuf {
    let p = Path::new(prog);
    if p.components().count() > 1 || p.extension().is_some() || !cfg!(windows) {
        return p.to_path_buf();
    }
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| e.to_ascii_lowercase())
        .collect();
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for ext in &exts {
                let cand = dir.join(format!("{prog}{ext}"));
                if cand.is_file() {
                    return cand;
                }
            }
        }
    }
    p.to_path_buf()
}

fn build_command(argv: &[String], shell: bool) -> Result<Command> {
    if argv.is_empty() {
        return Err(Error::new("no command given (usage: treeproof run -- <command> [args...])"));
    }
    if shell {
        let line = argv.join(" ");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let mut c = Command::new("cmd");
            c.arg("/d").arg("/s").arg("/c").raw_arg(format!("\"{line}\""));
            return Ok(c);
        }
        #[cfg(not(windows))]
        {
            let mut c = Command::new("sh");
            c.arg("-c").arg(line);
            return Ok(c);
        }
    }
    let mut c = Command::new(resolve_program(&argv[0]));
    c.args(&argv[1..]);
    Ok(c)
}

struct Capture {
    buf: Vec<u8>,
}

impl Capture {
    fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        if self.buf.len() > MAX_CAPTURE + MAX_CAPTURE / 4 {
            let cut = self.buf.len() - MAX_CAPTURE;
            self.buf.drain(..cut);
        }
    }
}

fn pump<R: Read + Send + 'static>(mut r: R, cap: Arc<Mutex<Capture>>, echo: Option<bool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match r.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    match echo {
                        Some(true) => {
                            let mut o = std::io::stdout().lock();
                            let _ = o.write_all(&chunk[..n]);
                            let _ = o.flush();
                        }
                        Some(false) => {
                            let mut e = std::io::stderr().lock();
                            let _ = e.write_all(&chunk[..n]);
                            let _ = e.flush();
                        }
                        None => {}
                    }
                    if let Ok(mut c) = cap.lock() {
                        c.push(&chunk[..n]);
                    }
                }
            }
        }
    })
}

/// Execute a command, streaming and capturing its output. Returns (exit code, output, timed_out).
pub fn execute(argv: &[String], shell: bool, cwd: &Path, echo: bool, timeout: Option<Duration>) -> Result<(Option<i32>, String, bool)> {
    let mut cmd = build_command(argv, shell)?;
    cmd.current_dir(cwd);
    cmd.env("TREEPROOF_ACTIVE", "1");
    cmd.stdin(if echo { Stdio::inherit() } else { Stdio::null() });
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| Error::new(format!("could not start {:?}: {e}", argv[0])))?;
    let cap = Arc::new(Mutex::new(Capture { buf: Vec::new() }));
    let t1 = pump(child.stdout.take().expect("piped stdout"), cap.clone(), if echo { Some(true) } else { None });
    let t2 = pump(child.stderr.take().expect("piped stderr"), cap.clone(), if echo { Some(false) } else { None });
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| Error::new(format!("wait: {e}")))? {
            break st;
        }
        if let Some(t) = timeout {
            if start.elapsed() > t {
                let _ = child.kill();
                timed_out = true;
                break child.wait().map_err(|e| Error::new(format!("wait: {e}")))?;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let _ = t1.join();
    let _ = t2.join();
    let out = cap.lock().map(|c| String::from_utf8_lossy(&c.buf).into_owned()).unwrap_or_default();
    Ok((status.code(), out, timed_out))
}

/// Decide the in-toto result for a run. Returns (result, reason).
pub fn decide(
    summary: &Summary,
    exit_code: Option<i32>,
    collect_only: Option<&str>,
    allow_unrecognized: bool,
    observed: bool,
) -> (String, Option<String>) {
    let fail = |r: String| ("FAILED".to_string(), Some(r));
    if let Some(why) = collect_only {
        return fail(format!("not a test run: {why}"));
    }
    match exit_code {
        Some(0) => {}
        Some(c) => return fail(format!("command exited with status {c}")),
        None if !observed => return fail("command was terminated by a signal or timeout".into()),
        None => {}
    }
    if summary.failed > 0 {
        return fail(format!("{} test(s) failed", summary.failed));
    }
    if !summary.recognized() {
        if allow_unrecognized && !observed {
            return ("PASSED".into(), Some("no runner summary recognised; result is from the exit status only".into()));
        }
        return fail("no test summary recognised in the output (supported: cargo test, nextest, pytest, jest, vitest, go test)".into());
    }
    if summary.passed == 0 {
        return fail("0 tests ran (nothing collected, or every test was filtered out or skipped)".into());
    }
    if summary.warned > 0 {
        return ("WARNED".into(), None);
    }
    ("PASSED".into(), None)
}

#[allow(clippy::too_many_arguments)]
fn sign_run(
    sk: &SigningKey,
    fp: &Fingerprint,
    nested: &std::collections::BTreeMap<String, String>,
    root: &Path,
    argv: &[String],
    shell: bool,
    summary: &Summary,
    exit_code: Option<i32>,
    result: &str,
    reason: Option<String>,
    mode: &str,
    host: Option<String>,
    started: u64,
    finished: u64,
) -> Result<String> {
    let run = RunInfo {
        schema: receipt::RUN_SCHEMA.to_string(),
        argv: argv.to_vec(),
        shell,
        cwd: fp.cwd.clone(),
        runners: summary.runners.iter().map(|r| r.as_str().to_string()).collect(),
        counts: Counts { passed: summary.passed, failed: summary.failed, skipped: summary.skipped, warned: summary.warned },
        exit_code,
        mode: mode.to_string(),
        cache_key: fp.key.clone(),
        started_at: rfc3339_ms(started),
        finished_at: rfc3339_ms(finished),
        duration_ms: finished.saturating_sub(started),
        head_commit: crate::git::head_commit(root),
        toolchains: fp.toolchains.clone(),
        env: fp.env.clone(),
        os: fp.os.clone(),
        arch: fp.arch.clone(),
        notes: summary.notes.clone(),
        reason,
        host,
        treeproof_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let st = receipt::build_statement_with_nested(&fp.tree, result, &run, &fp.lockfiles, nested, summary)?;
    let env = receipt::sign_statement(sk, &st)?;
    Ok(serde_json::to_string(&env)? + "\n")
}

fn outcome_from_receipt(status: &str, r: &receipt::Receipt, json: String, path: PathBuf, output: String) -> RunOutcome {
    let passed = matches!(r.statement.predicate.result.as_str(), "PASSED" | "WARNED");
    RunOutcome {
        status: status.to_string(),
        result: r.statement.predicate.result.clone(),
        reason: r.run.reason.clone(),
        tree: r.tree.clone(),
        key: r.run.cache_key.clone(),
        counts: r.run.counts,
        runners: r.run.runners.clone(),
        notes: r.run.notes.clone(),
        exit_code: if passed { 0 } else { r.run.exit_code.filter(|c| *c != 0).unwrap_or(1) },
        receipt_path: path,
        receipt_json: json,
        output,
    }
}

/// Environment allowlist and `allow_unrecognized` for a run. `allow_unrecognized` comes from
/// the committed policy only (an agent can write the working copy); extra environment names
/// from either file are added to the key, which can only make the cache stricter.
fn run_policy(root: &Path) -> Result<(Vec<String>, bool)> {
    let committed = Config::load_committed(root)?;
    let mut env = committed.run.env.clone();
    if let Ok(wc) = Config::load(root) {
        for e in wc.run.env {
            if !env.contains(&e) {
                env.push(e);
            }
        }
    }
    Ok((env, committed.run.allow_unrecognized))
}

/// Run a command under treeproof (see the crate docs), with the default store and key.
pub fn run(opts: &RunOptions) -> Result<RunOutcome> {
    run_in(opts, &Context::default())
}

/// Run a command under treeproof with the store and key from `ctx`.
pub fn run_in(opts: &RunOptions, ctx: &Context) -> Result<RunOutcome> {
    let root = crate::git::repo_root(&opts.cwd)?;
    let (env_names, policy_allow) = run_policy(&root)?;
    let sk = ctx.signing_key()?;
    let pk = PublicKey { key: sk.verifying_key() };
    let store = ctx.open_store()?;
    let salt = store.env_salt()?;
    let (fp, nested) = fingerprint::compute_full(&root, &opts.cwd, &opts.argv, opts.shell, &env_names, &salt)?;

    let try_cache = |store: &Store| -> Option<RunOutcome> {
        if opts.no_cache {
            return None;
        }
        let text = store.get_pass(&fp.key)?;
        match receipt::open_text(&text, std::slice::from_ref(&pk)) {
            Ok(r) if r.passed() || r.statement.predicate.result == "WARNED" => {
                if r.tree != fp.tree || r.run.cache_key != fp.key {
                    store.evict(&fp.key);
                    return None;
                }
                let path = store.record(&fp.tree, &fp.key, &text).ok()?;
                let log = store.get_log(&fp.key).unwrap_or_default();
                Some(outcome_from_receipt("cache_hit", &r, text, path, log))
            }
            _ => {
                store.evict(&fp.key);
                None
            }
        }
    };

    if let Some(hit) = try_cache(&store) {
        return Ok(hit);
    }
    let (_guard, waited) = store.lock(&fp.key)?;
    if waited {
        if let Some(hit) = try_cache(&store) {
            return Ok(hit);
        }
    }

    let started = now_ms();
    let (code, output, timed_out) = execute(&opts.argv, opts.shell, &opts.cwd, opts.echo, opts.timeout)?;
    let finished = now_ms();
    let mut summary = parse::parse(&output, &opts.argv);
    if timed_out {
        summary.notes.push("timed out and was killed".into());
    }
    let allow = opts.allow_unrecognized.unwrap_or(policy_allow);
    let (result, mut reason) = decide(&summary, if timed_out { None } else { code }, parse::is_collect_only(&opts.argv), allow, false);

    // A run that changes the tree it tested is still a true statement about the tree it started
    // from, but the agent's current tree will not match it. Say so.
    if let Ok(after) = crate::git::tree_digest(&root) {
        if after != fp.tree {
            let msg = format!(
                "the command changed the working tree ({} -> {}); add generated files to .gitignore so receipts match",
                &fp.tree[..12],
                &after[..12.min(after.len())]
            );
            summary.notes.push(msg.clone());
            if reason.is_none() {
                reason = Some(msg);
            }
        }
    }

    let json =
        sign_run(&sk, &fp, &nested, &root, &opts.argv, opts.shell, &summary, code, &result, reason, MODE_FRESH, None, started, finished)?;
    let path = store.record(&fp.tree, &fp.key, &json)?;
    if result == "PASSED" || result == "WARNED" {
        store.put_pass(&fp.key, &json, tail(&output, MAX_LOG))?;
    } else {
        // A failure of an identical run (for example a flaky test under --no-cache) withdraws any
        // cached pass, so the next run executes instead of replaying it.
        store.evict(&fp.key);
    }
    let r = receipt::open_text(&json, std::slice::from_ref(&pk))?;
    Ok(outcome_from_receipt("fresh", &r, json, path, output))
}

/// Record a receipt for a test command that a host ran itself (PostToolUse hook).
/// Observed receipts are never put in the run cache, and by default they do not satisfy the
/// gate (`accept_observed`); a failing one still counts against a later subset run.
pub fn record_observed(cwd: &Path, command: &str, output: &str, exit_code: Option<i32>, host: &str) -> Result<Option<RunOutcome>> {
    record_observed_in(cwd, command, output, exit_code, host, &Context::default())
}

/// [`record_observed`] with the store and key from `ctx`.
pub fn record_observed_in(
    cwd: &Path,
    command: &str,
    output: &str,
    exit_code: Option<i32>,
    host: &str,
    ctx: &Context,
) -> Result<Option<RunOutcome>> {
    let root = crate::git::repo_root(cwd)?;
    let (env_names, _) = run_policy(&root)?;
    let argv = vec![command.to_string()];
    let summary = parse::parse(output, &argv);
    if !summary.recognized() {
        return Ok(None);
    }
    let words: Vec<String> = command.split_whitespace().map(String::from).collect();
    let (result, reason) = decide(&summary, exit_code, parse::is_collect_only(&words), false, true);
    let sk = ctx.signing_key()?;
    let pk = PublicKey { key: sk.verifying_key() };
    let store = ctx.open_store()?;
    let salt = store.env_salt()?;
    let (fp, nested) = fingerprint::compute_full(&root, cwd, &argv, true, &env_names, &salt)?;
    let now = now_ms();
    let json = sign_run(
        &sk,
        &fp,
        &nested,
        &root,
        &argv,
        true,
        &summary,
        exit_code,
        &result,
        reason,
        MODE_OBSERVED,
        Some(host.to_string()),
        now,
        now,
    )?;
    let path = store.record(&fp.tree, &fp.key, &json)?;
    let r = receipt::open_text(&json, std::slice::from_ref(&pk))?;
    Ok(Some(outcome_from_receipt("observed", &r, json, path, String::new())))
}
