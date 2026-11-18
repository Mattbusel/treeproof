use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use treeproof::config::Config;
use treeproof::dsse::PublicKey;
use treeproof::otlp::{self, Evaluation, Label};
use treeproof::run::{self, RunOptions};
use treeproof::store::Store;
use treeproof::{gate, git, hooks, keys, mcp, receipt, verify, Error, Result};

#[derive(Parser)]
#[command(name = "treeproof", version, about = "Signed test-result receipts for a git tree", long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a test command and sign a receipt for the current tree (cached across worktrees).
    Run {
        /// Execute even when an identical passing run is cached.
        #[arg(long)]
        no_cache: bool,
        /// Run the command line through the platform shell (sh -c / cmd /C).
        #[arg(long)]
        shell: bool,
        /// Sign a pass from the exit status when no runner summary is recognised.
        #[arg(long)]
        allow_unrecognized: bool,
        /// Kill the command after this many seconds.
        #[arg(long)]
        timeout: Option<u64>,
        /// Print the outcome as JSON on stderr when done.
        #[arg(long)]
        json: bool,
        /// The command and its arguments (after `--`).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Verify receipt files: signature, schema, optionally tree and a re-run.
    Verify {
        /// Receipt files.
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Trusted public key file (PEM, hex or base64). Repeatable.
        #[arg(long = "key")]
        keys: Vec<PathBuf>,
        /// Do not trust this machine's own signing key.
        #[arg(long)]
        no_local_key: bool,
        /// Trust the keys committed under .treeproof/trusted_keys/ at this git revision (for
        /// example a pull request's base commit) instead of the keys in the working copy.
        #[arg(long, value_name = "REV")]
        keys_ref: Option<String>,
        /// Do not trust keys from .treeproof/trusted_keys/ in the working copy.
        #[arg(long)]
        no_repo_keys: bool,
        /// Apply the gate policy (treeproof.toml) committed at this revision: required commands,
        /// minimum tests, observed and subset runs. Defaults to HEAD when --tree current is given.
        #[arg(long, value_name = "REV")]
        policy_ref: Option<String>,
        /// Do not apply a gate policy.
        #[arg(long, conflicts_with = "policy_ref")]
        no_policy: bool,
        /// Only consider receipts for this tree: a git tree id, or `current`.
        /// Receipts for other trees are skipped; at least one must match.
        #[arg(long)]
        tree: Option<String>,
        /// Re-run each receipt's command and fail if the result disagrees.
        #[arg(long)]
        rerun: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show the current tree, its receipts and the gate verdict.
    #[command(alias = "query")]
    Status {
        #[arg(long)]
        json: bool,
        /// Read treeproof.toml as committed at this revision (default HEAD; the working copy is never used).
        #[arg(long, value_name = "REV")]
        policy_ref: Option<String>,
    },
    /// Exit 0 when a valid passing receipt for the current tree satisfies the policy, 1 otherwise.
    Gate {
        #[arg(long)]
        json: bool,
        /// Read treeproof.toml as committed at this revision (default HEAD; the working copy is never used).
        #[arg(long, value_name = "REV")]
        policy_ref: Option<String>,
    },
    /// Print the working-tree digest (git tree id) receipts are bound to.
    Tree,
    /// Copy the accepted receipt for the current tree (and the public key) into the repository.
    Export {
        #[arg(long, default_value = ".treeproof/receipts")]
        dir: PathBuf,
        /// Keep receipts for older trees in the directory.
        #[arg(long)]
        keep_old: bool,
        /// Read treeproof.toml as committed at this revision (default HEAD).
        #[arg(long, value_name = "REV")]
        policy_ref: Option<String>,
    },
    /// Manage the signing key.
    Key {
        #[command(subcommand)]
        action: KeyCmd,
    },
    /// Run as a Claude Code or Codex hook (reads the hook JSON on stdin).
    Hook {
        /// Event name; defaults to `hook_event_name` from the input.
        event: Option<String>,
    },
    /// Serve the MCP tools receipt_run, receipt_query and receipt_verify over stdio.
    Mcp,
    /// Print hook configuration for a host.
    HooksConfig {
        /// `claude` or `codex`.
        host: String,
    },
    /// Export gen_ai.evaluation.result events for a receipt over OTLP/HTTP JSON.
    Otel {
        /// Receipt file; defaults to the newest receipt for the current tree.
        file: Option<PathBuf>,
        /// Also re-run the command (verification.test_rerun).
        #[arg(long)]
        rerun: bool,
        /// OTLP logs endpoint; defaults to TREEPROOF_OTLP_ENDPOINT / OTEL_EXPORTER_OTLP_* settings.
        #[arg(long)]
        endpoint: Option<String>,
        /// Print the OTLP request instead of sending it.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum KeyCmd {
    /// Create the signing key if it does not exist and print its id.
    Init,
    /// Print the key id, file location and public key.
    Show,
    /// Print the public key as PEM.
    Export,
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn short(s: &str) -> &str {
    &s[..12.min(s.len())]
}

fn cmd_run(
    command: Vec<String>,
    shell: bool,
    no_cache: bool,
    allow_unrecognized: bool,
    timeout: Option<u64>,
    json: bool,
) -> Result<ExitCode> {
    let opts = RunOptions {
        argv: command,
        shell,
        cwd: cwd(),
        no_cache,
        echo: true,
        allow_unrecognized: if allow_unrecognized { Some(true) } else { None },
        timeout: timeout.map(Duration::from_secs),
    };
    let o = run::run(&opts)?;
    if o.status == "cache_hit" {
        print!("{}", o.output);
    }
    let c = o.counts;
    eprintln!(
        "treeproof: {} {} tree {} ({} passed, {} failed, {} skipped){}",
        o.status,
        o.result,
        short(&o.tree),
        c.passed,
        c.failed,
        c.skipped,
        o.reason.as_ref().map(|r| format!(": {r}")).unwrap_or_default()
    );
    eprintln!("treeproof: receipt {}", o.receipt_path.display());
    if json {
        eprintln!("{}", serde_json::to_string_pretty(&o)?);
    }
    Ok(ExitCode::from(o.exit_code.clamp(0, 255) as u8))
}

fn read_keys(paths: &[PathBuf]) -> Result<Vec<PublicKey>> {
    paths
        .iter()
        .map(|p| {
            let t = std::fs::read_to_string(p).map_err(|e| Error::new(format!("read {}: {e}", p.display())))?;
            PublicKey::parse(&t).map_err(|e| Error::new(format!("{}: {e}", p.display())))
        })
        .collect()
}

/// The policy for gate/status/export: committed at `policy_ref`, or at HEAD.
fn load_policy(root: &std::path::Path, policy_ref: Option<&str>) -> Result<(Config, String)> {
    match policy_ref {
        Some(r) => Ok((Config::load_at_rev(root, r)?.0, format!("{r}:treeproof.toml"))),
        None => Ok((Config::load_committed(root)?, "HEAD:treeproof.toml".to_string())),
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_verify(
    files: Vec<PathBuf>,
    key_files: Vec<PathBuf>,
    no_local_key: bool,
    keys_ref: Option<String>,
    no_repo_keys: bool,
    policy_ref: Option<String>,
    no_policy: bool,
    tree: Option<String>,
    rerun: bool,
    json: bool,
) -> Result<ExitCode> {
    let root = git::repo_root(&cwd()).ok();
    let need_root = |what: &str| root.clone().ok_or_else(|| Error::new(format!("{what} needs a git repository")));
    let mut extra = read_keys(&key_files)?;
    let repo_keys_dir = if let Some(r) = &keys_ref {
        let rr = need_root("--keys-ref")?;
        extra.extend(keys::trusted_keys_at_rev(&rr, r)?);
        None
    } else if no_repo_keys || rerun {
        // --rerun executes the receipt's command: keys from the checkout are never trusted for
        // that (a cloned repository could sign its own receipt). Use --key, --keys-ref or
        // TREEPROOF_TRUSTED_KEYS for someone else's receipts.
        None
    } else {
        root.clone()
    };
    let trusted = keys::trusted_keys(repo_keys_dir.as_deref(), !no_local_key, &extra)?;
    if trusted.is_empty() {
        let where_ = keys_ref.as_deref().map(|r| format!(" at {r}")).unwrap_or_default();
        return Err(Error::new(format!(
            "no trusted keys: none committed under .treeproof/trusted_keys/{where_} and none given with --key{}. Commit the signer's public key (treeproof key export) to the base branch first.",
            if rerun && keys_ref.is_none() { " (with --rerun the working copy's keys are not trusted; pass --keys-ref <rev> or --key)" } else { "" }
        )));
    }
    let current = tree.as_deref() == Some("current");
    let want = match tree.as_deref() {
        Some("current") => Some(git::tree_digest(&need_root("--tree current")?)?),
        Some(t) if treeproof::util::is_git_oid(t) => Some(t.to_string()),
        Some(t) => return Err(Error::new(format!("--tree must be a git tree id or `current`, not {t:?}"))),
        None => None,
    };
    let policy_rev = if no_policy { None } else { policy_ref.or_else(|| current.then(|| "HEAD".to_string())) };
    let policy = match &policy_rev {
        Some(r) => {
            let rr = need_root("--policy-ref")?;
            let cfg = if r == "HEAD" { Config::load_committed(&rr)? } else { Config::load_at_rev(&rr, r)?.0 };
            Some(cfg)
        }
        None => None,
    };
    let nested_now = if current { Some(git::nested_trees(&need_root("--tree current")?)?) } else { None };
    let rerun_root = if rerun { Some(need_root("--rerun")?) } else { None };
    let mut reports = Vec::new();
    let mut skipped = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).map_err(|e| Error::new(format!("read {}: {e}", f.display())))?;
        let src = f.display().to_string();
        // Check the signature, subject and policy first: receipts for other trees are skipped and
        // nothing is re-run unless everything else holds.
        let mut pre = verify::verify_text(&src, &text, &trusted, None, None, false);
        if let Some(w) = &want {
            if pre.signature_ok && pre.tree.as_deref() != Some(w.as_str()) {
                skipped.push(src);
                continue;
            }
        }
        if !pre.ok {
            reports.push(pre);
            continue;
        }
        let r = receipt::open_text(&text, &trusted)?;
        let mut problems = Vec::new();
        if let (Some(cfg), Some(rev)) = (&policy, &policy_rev) {
            if let Some(why) = gate::policy_rejection(&r, cfg) {
                problems.push(format!("policy {rev}:treeproof.toml: {why}"));
            }
        }
        if let Some(n) = &nested_now {
            if r.nested_trees() != *n {
                problems.push(
                    "nested repository (submodule) content differs from the receipt; check out submodules before verifying".to_string(),
                );
            }
        }
        if !problems.is_empty() {
            pre.ok = false;
            pre.errors.extend(problems);
            reports.push(pre);
            continue;
        }
        reports.push(verify::verify_text(&src, &text, &trusted, want.as_deref(), rerun_root.as_deref(), !json));
    }
    let mut ok = !reports.is_empty() && reports.iter().all(|r| r.ok);
    let mut problems = Vec::new();
    if let Some(w) = &want {
        if reports.is_empty() {
            ok = false;
            problems.push(format!("no receipt for tree {w} among {} file(s)", files.len()));
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"ok": ok, "reports": reports, "skipped_other_trees": skipped, "problems": problems, "policy": policy_rev.as_ref().map(|r| format!("{r}:treeproof.toml"))})
            )?
        );
    } else {
        for r in &reports {
            if r.ok {
                println!(
                    "ok      {}  {} tree {} `{}`{}",
                    r.source,
                    r.result.clone().unwrap_or_default(),
                    short(r.tree.as_deref().unwrap_or("")),
                    r.command.clone().unwrap_or_default(),
                    r.rerun.as_ref().map(|x| format!(" (re-run {}, {} passed)", x.result, x.counts.passed)).unwrap_or_default()
                );
            } else {
                println!("FAILED  {}  {}", r.source, r.errors.join("; "));
            }
            if let Some(rr) = &r.rerun {
                for d in &rr.details {
                    println!("        {d}");
                }
            }
        }
        for s in &skipped {
            println!("skip    {s}  (receipt for another tree)");
        }
        for p in &problems {
            println!("FAILED  {p}");
        }
    }
    Ok(if ok { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

fn cmd_status(json: bool, gate_only: bool, policy_ref: Option<String>) -> Result<ExitCode> {
    let root = git::repo_root(&cwd())?;
    let (cfg, source) = load_policy(&root, policy_ref.as_deref())?;
    let rep = gate::evaluate(&root, &cfg, None)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rep)?);
    } else if gate_only {
        println!("{}", rep.reason);
    } else {
        println!("tree     {}", rep.tree);
        println!("gate     {}", if rep.ok { "PASS" } else { "BLOCK" });
        println!("verdict  {}", rep.reason);
        println!("policy   {source}");
        if let Ok(wc) = Config::load(&root) {
            if wc != cfg {
                println!("note     the working copy's treeproof.toml differs from {source}; policy changes apply once committed");
            }
        }
        for b in &rep.considered {
            println!(
                "receipt  {:7} {:8} {} passed  `{}`  {}{}",
                b.result,
                b.mode,
                b.counts.passed,
                b.command,
                b.finished_at,
                b.why_not.as_ref().map(|w| format!("  ({w})")).unwrap_or_default()
            );
        }
    }
    Ok(if rep.ok { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

fn cmd_export(dir: PathBuf, keep_old: bool, policy_ref: Option<String>) -> Result<ExitCode> {
    let root = git::repo_root(&cwd())?;
    let (cfg, _) = load_policy(&root, policy_ref.as_deref())?;
    let rep = gate::evaluate(&root, &cfg, None)?;
    let Some(chosen) = rep.receipt else {
        eprintln!("treeproof: {}", rep.reason);
        return Ok(ExitCode::from(1));
    };
    let dir = if dir.is_absolute() { dir } else { root.join(dir) };
    std::fs::create_dir_all(&dir)?;
    if !keep_old {
        for e in std::fs::read_dir(&dir)?.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("json") {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    let target = dir.join(format!("{}.json", rep.tree));
    std::fs::copy(&chosen.path, &target)?;
    let sk = keys::load_or_create()?;
    let pk = PublicKey { key: sk.verifying_key() };
    let kdir = root.join(".treeproof").join("trusted_keys");
    std::fs::create_dir_all(&kdir)?;
    let kfile = kdir.join(format!("{}.pem", short(&pk.keyid())));
    if !kfile.exists() {
        std::fs::write(&kfile, pk.to_pem())?;
    }
    println!("{}", target.display());
    eprintln!("treeproof: exported receipt for tree {} and public key {}", short(&rep.tree), kfile.display());
    Ok(ExitCode::SUCCESS)
}

fn cmd_key(action: KeyCmd) -> Result<ExitCode> {
    match action {
        KeyCmd::Init => {
            let sk = keys::load_or_create()?;
            println!("{}", PublicKey { key: sk.verifying_key() }.keyid());
        }
        KeyCmd::Show => {
            let p = keys::key_path()?;
            match keys::load()? {
                Some(sk) => {
                    let pk = PublicKey { key: sk.verifying_key() };
                    println!("keyid  {}", pk.keyid());
                    println!("file   {}", p.display());
                    print!("{}", pk.to_pem());
                }
                None => {
                    println!("no signing key yet at {} (created on first run, or `treeproof key init`)", p.display());
                    return Ok(ExitCode::from(1));
                }
            }
        }
        KeyCmd::Export => {
            let sk = keys::load_or_create()?;
            print!("{}", PublicKey { key: sk.verifying_key() }.to_pem());
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn hooks_config(host: &str) -> Result<serde_json::Value> {
    let cmd = |ev: &str| serde_json::json!({"type": "command", "command": format!("treeproof hook {ev}"), "timeout": 120});
    let edit_matcher = if host == "codex" { "Bash|apply_patch" } else { "Bash|Edit|Write|MultiEdit|NotebookEdit" };
    let read_matcher = if host == "codex" { "apply_patch" } else { "Read|Edit|Write|MultiEdit|NotebookEdit" };
    let mut hooks = serde_json::json!({
        "SessionStart": [{"hooks": [cmd("SessionStart")]}],
        "Stop": [{"hooks": [cmd("Stop")]}],
        "PostToolUse": [
            {"matcher": "Bash", "hooks": [cmd("PostToolUse")]},
            {"matcher": read_matcher, "hooks": [cmd("PostToolUse")]}
        ],
        "PreToolUse": [{"matcher": edit_matcher, "hooks": [cmd("PreToolUse")]}]
    });
    match host {
        "claude" => {
            hooks["TaskCompleted"] = serde_json::json!([{"hooks": [cmd("TaskCompleted")]}]);
        }
        "codex" => {}
        other => return Err(Error::new(format!("unknown host {other:?} (claude or codex)"))),
    }
    Ok(serde_json::json!({"hooks": hooks}))
}

fn cmd_otel(file: Option<PathBuf>, rerun: bool, endpoint: Option<String>, dry_run: bool) -> Result<ExitCode> {
    let root = git::repo_root(&cwd()).ok();
    let trusted = if rerun { keys::rerun_trusted_keys(&[])? } else { keys::trusted_keys(root.as_deref(), true, &[])? };
    let current = root.as_deref().and_then(|r| git::tree_digest(r).ok());
    let text = match &file {
        Some(f) => std::fs::read_to_string(f).map_err(|e| Error::new(format!("read {}: {e}", f.display())))?,
        None => {
            let t = current.clone().ok_or_else(|| Error::new("no receipt file given and not in a git repository"))?;
            Store::open()?
                .receipts_for_tree(&t)
                .into_iter()
                .next()
                .map(|(_, txt)| txt)
                .ok_or_else(|| Error::new("no receipt recorded for the current tree"))?
        }
    };
    let r = receipt::open_text(&text, &trusted)?;
    let mut evals = vec![Evaluation {
        name: otlp::CHECK_EXIT_CODE,
        label: if matches!(r.statement.predicate.result.as_str(), "PASSED" | "WARNED") { Label::Verified } else { Label::Contradicted },
        explanation: format!(
            "receipt result {} ({} passed, {} failed)",
            r.statement.predicate.result, r.run.counts.passed, r.run.counts.failed
        ),
    }];
    evals.push(match &current {
        Some(t) if *t == r.tree => Evaluation {
            name: otlp::CHECK_FS_DIFF,
            label: Label::Verified,
            explanation: "receipt tree equals the current working tree".into(),
        },
        Some(t) => Evaluation {
            name: otlp::CHECK_FS_DIFF,
            label: Label::Contradicted,
            explanation: format!("working tree changed since the run ({} -> {})", short(&r.tree), short(t)),
        },
        None => Evaluation { name: otlp::CHECK_FS_DIFF, label: Label::Unknown, explanation: "no working tree to compare".into() },
    });
    evals.push(if rerun {
        let rt = root.clone().ok_or_else(|| Error::new("--rerun needs a git repository"))?;
        let rr = verify::rerun(&r, &rt, false)?;
        Evaluation {
            name: otlp::CHECK_TEST_RERUN,
            label: if rr.agrees { Label::Verified } else { Label::Contradicted },
            explanation: format!("re-run {} ({} passed, {} failed)", rr.result, rr.counts.passed, rr.counts.failed),
        }
    } else {
        Evaluation { name: otlp::CHECK_TEST_RERUN, label: Label::Unknown, explanation: "not re-run".into() }
    });
    let extra = vec![("vcs.ref.head.revision".to_string(), r.tree.clone())];
    let tp = std::env::var("TRACEPARENT").ok();
    let body = otlp::build_request(&evals, &extra, tp.as_deref());
    if dry_run {
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(ExitCode::SUCCESS);
    }
    let ep = endpoint
        .or_else(|| otlp::endpoint_from_env_with(true))
        .ok_or_else(|| Error::new("no OTLP endpoint: pass --endpoint or set TREEPROOF_OTLP_ENDPOINT / OTEL_EXPORTER_OTLP_ENDPOINT"))?;
    let code = otlp::post(&ep, &body, &otlp::headers_from_env())?;
    eprintln!("treeproof: sent {} evaluation event(s) to {ep}: HTTP {code}", evals.len());
    Ok(if (200..300).contains(&code) { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let res = match cli.cmd {
        Cmd::Run { no_cache, shell, allow_unrecognized, timeout, json, command } => {
            cmd_run(command, shell, no_cache, allow_unrecognized, timeout, json)
        }
        Cmd::Verify { files, keys, no_local_key, keys_ref, no_repo_keys, policy_ref, no_policy, tree, rerun, json } => {
            cmd_verify(files, keys, no_local_key, keys_ref, no_repo_keys, policy_ref, no_policy, tree, rerun, json)
        }
        Cmd::Status { json, policy_ref } => cmd_status(json, false, policy_ref),
        Cmd::Gate { json, policy_ref } => cmd_status(json, true, policy_ref),
        Cmd::Tree => git::repo_root(&cwd()).and_then(|r| git::tree_digest(&r)).map(|t| {
            println!("{t}");
            ExitCode::SUCCESS
        }),
        Cmd::Export { dir, keep_old, policy_ref } => cmd_export(dir, keep_old, policy_ref),
        Cmd::Key { action } => cmd_key(action),
        Cmd::Hook { event } => return ExitCode::from(hooks::main(event.as_deref()).clamp(0, 255) as u8),
        Cmd::Mcp => return ExitCode::from(mcp::serve() as u8),
        Cmd::HooksConfig { host } => hooks_config(&host).map(|v| {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            ExitCode::SUCCESS
        }),
        Cmd::Otel { file, rerun, endpoint, dry_run } => cmd_otel(file, rerun, endpoint, dry_run),
    };
    match res {
        Ok(c) => c,
        Err(e) => {
            eprintln!("treeproof: error: {e}");
            ExitCode::from(2)
        }
    }
}
