//! The library API with an explicit [`treeproof::Context`]: no process-wide environment
//! variables, so these tests run in parallel with each other and with anything else.

use std::path::Path;
use std::process::Command;
use treeproof::config::Config;
use treeproof::run::{RunOptions, RunStatus, TestResult};
use treeproof::{gate, Context};

fn git(dir: &Path, args: &[&str]) {
    let o = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

fn project() -> (tempfile::TempDir, std::path::PathBuf, Context) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["config", "core.autocrlf", "false"]);
    std::fs::write(repo.join("run_tests.py"), include_str!("fixtures/run_tests.py")).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let ctx = Context::new().with_store_dir(tmp.path().join("store")).with_key_file(tmp.path().join("key"));
    (tmp, repo, ctx)
}

#[test]
fn run_and_gate_with_an_explicit_context() {
    let (tmp, repo, ctx) = project();
    let opts = RunOptions::new(vec![python().into(), "run_tests.py".into()], &repo);
    let out = treeproof::run::run_in(&opts, &ctx).unwrap();
    assert_eq!(out.test_result(), TestResult::Passed);
    assert_eq!(out.run_status(), RunStatus::Fresh);
    assert!(out.passed());
    assert_eq!(out.counts.passed, 3);
    assert!(out.receipt_path.starts_with(tmp.path().join("store")));
    assert!(tmp.path().join("key").exists());

    let again = treeproof::run::run_in(&opts, &ctx).unwrap();
    assert_eq!(again.run_status(), RunStatus::CacheHit);

    let report = gate::evaluate_in(&repo, &Config::load_committed(&repo).unwrap(), None, &ctx).unwrap();
    assert!(report.ok, "{}", report.reason);

    // Another context sees none of it.
    let (_tmp2, _repo2, other) = project();
    let report = gate::evaluate_in(&repo, &Config::default(), None, &other).unwrap();
    assert!(!report.ok);
}

#[test]
fn a_failing_run_is_a_failed_result_not_an_error() {
    let (_tmp, repo, ctx) = project();
    let opts = RunOptions::new(vec![python().into(), "run_tests.py".into(), "zero".into()], &repo).with_no_cache(true);
    let out = treeproof::run::run_in(&opts, &ctx).unwrap();
    assert_eq!(out.test_result(), TestResult::Failed);
    assert!(!out.passed());
}
