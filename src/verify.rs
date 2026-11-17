//! Verifying receipts, optionally by re-running their command.

use crate::dsse::PublicKey;
use crate::receipt::{self, Counts, Receipt};
use crate::{parse, run, Error, Result};
use serde::Serialize;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Serialize)]
pub struct RerunReport {
    pub result: String,
    pub reason: Option<String>,
    pub counts: Counts,
    pub exit_code: Option<i32>,
    /// True when the re-run agrees with the receipt.
    pub agrees: bool,
    pub details: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct VerifyReport {
    pub source: String,
    /// Signature, schema and (when requested) tree all check out, and a re-run (when requested) agrees.
    pub ok: bool,
    pub signature_ok: bool,
    pub keyids: Vec<String>,
    pub result: Option<String>,
    pub tree: Option<String>,
    pub tree_matches: Option<bool>,
    pub command: Option<String>,
    pub counts: Option<Counts>,
    pub rerun: Option<RerunReport>,
    pub errors: Vec<String>,
}

impl VerifyReport {
    fn failed(source: &str, err: String) -> VerifyReport {
        VerifyReport {
            source: source.to_string(),
            ok: false,
            signature_ok: false,
            keyids: Vec::new(),
            result: None,
            tree: None,
            tree_matches: None,
            command: None,
            counts: None,
            rerun: None,
            errors: vec![err],
        }
    }
}

/// Reject receipt working directories that would leave the repository.
pub fn safe_rel_dir(rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel);
    if p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir | Component::Prefix(_) | Component::RootDir)) {
        return Err(Error::new(format!("receipt cwd {rel:?} is not a path inside the repository")));
    }
    Ok(p.to_path_buf())
}

/// Re-run a verified receipt's command at `root` and compare.
pub fn rerun(r: &Receipt, root: &Path, echo: bool) -> Result<RerunReport> {
    let dir = root.join(safe_rel_dir(&r.run.cwd)?);
    let (code, output, _) = run::execute(&r.run.argv, r.run.shell, &dir, echo, None)?;
    let summary = parse::parse(&output, &r.run.argv);
    let words: Vec<String> =
        if r.run.shell { r.run.argv.join(" ").split_whitespace().map(String::from).collect() } else { r.run.argv.clone() };
    let (result, reason) = run::decide(
        &summary,
        code,
        parse::is_collect_only(&words),
        r.run.reason.as_deref().map(|s| s.starts_with("no runner summary")).unwrap_or(false),
        false,
    );
    let claimed_pass = matches!(r.statement.predicate.result.as_str(), "PASSED" | "WARNED");
    let rerun_pass = matches!(result.as_str(), "PASSED" | "WARNED");
    let mut details = Vec::new();
    let agrees = claimed_pass == rerun_pass;
    if !agrees {
        details.push(format!("receipt says {} but the re-run is {}", r.statement.predicate.result, result));
    }
    if summary.passed != r.run.counts.passed {
        details.push(format!("passed count differs: receipt {}, re-run {}", r.run.counts.passed, summary.passed));
    }
    for name in &summary.failed_names {
        if r.statement.predicate.passed_tests.contains(name) {
            details.push(format!("{name} passed in the receipt but failed on re-run"));
        }
    }
    Ok(RerunReport {
        result,
        reason,
        counts: Counts { passed: summary.passed, failed: summary.failed, skipped: summary.skipped, warned: summary.warned },
        exit_code: code,
        agrees,
        details,
    })
}

/// Verify one receipt's text.
pub fn verify_text(
    source: &str,
    text: &str,
    trusted: &[PublicKey],
    want_tree: Option<&str>,
    rerun_root: Option<&Path>,
    echo: bool,
) -> VerifyReport {
    let r = match receipt::open_text(text, trusted) {
        Ok(r) => r,
        Err(e) => return VerifyReport::failed(source, e.to_string()),
    };
    let mut rep = VerifyReport {
        source: source.to_string(),
        ok: true,
        signature_ok: true,
        keyids: r.keyids.clone(),
        result: Some(r.statement.predicate.result.clone()),
        tree: Some(r.tree.clone()),
        tree_matches: want_tree.map(|t| t == r.tree),
        command: Some(if r.run.shell { r.run.argv.join(" ") } else { crate::util::join_argv(&r.run.argv) }),
        counts: Some(r.run.counts),
        rerun: None,
        errors: Vec::new(),
    };
    if rep.tree_matches == Some(false) {
        rep.ok = false;
        rep.errors.push(format!("receipt is for tree {} but the expected tree is {}", r.tree, want_tree.unwrap_or_default()));
        return rep;
    }
    if !r.passed() && r.statement.predicate.result != "WARNED" {
        rep.ok = false;
        rep.errors.push(format!(
            "receipt is a signed record of a {} run{}",
            r.statement.predicate.result,
            r.run.reason.as_ref().map(|x| format!(": {x}")).unwrap_or_default()
        ));
        return rep;
    }
    if let Some(root) = rerun_root {
        match rerun(&r, root, echo) {
            Ok(rr) => {
                if !rr.agrees {
                    rep.ok = false;
                    rep.errors.extend(rr.details.iter().filter(|d| d.starts_with("receipt says")).cloned());
                }
                rep.rerun = Some(rr);
            }
            Err(e) => {
                rep.ok = false;
                rep.errors.push(format!("re-run failed to start: {e}"));
            }
        }
    }
    rep
}
