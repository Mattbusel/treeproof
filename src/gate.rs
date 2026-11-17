//! The "done" gate: is there a fresh, valid, passing receipt for the current tree?

use crate::config::{command_matches, Config};
use crate::context::Context;
use crate::receipt::{self, Counts, Receipt, MODE_OBSERVED};
use crate::{keys, parse, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize)]
pub struct ReceiptBrief {
    pub path: PathBuf,
    pub result: String,
    pub command: String,
    pub mode: String,
    pub counts: Counts,
    pub finished_at: String,
    pub accepted: bool,
    pub why_not: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GateReport {
    pub tree: String,
    pub ok: bool,
    pub reason: String,
    pub receipt: Option<ReceiptBrief>,
    pub considered: Vec<ReceiptBrief>,
    pub suggestion: String,
}

/// A test command to suggest when no receipt exists.
pub fn suggest_command(root: &Path, cfg: &Config) -> String {
    if let Some(c) = cfg.gate.commands.first() {
        return c.trim_end_matches('*').trim().to_string();
    }
    let has = |f: &str| root.join(f).exists();
    if has("Cargo.toml") {
        "cargo test".into()
    } else if has("go.mod") {
        "go test ./...".into()
    } else if has("pyproject.toml") || has("pytest.ini") || has("setup.py") || has("tox.ini") {
        "python -m pytest".into()
    } else if has("package.json") {
        "npm test".into()
    } else {
        "<your test command>".into()
    }
}

/// Why a verified receipt does not satisfy the policy `cfg`, or `None` when it does.
///
/// This covers the result, the runner, the test count, observed receipts, required commands and
/// subset runs. Whether the receipt is for the right tree is the caller's check.
pub fn policy_rejection(r: &Receipt, cfg: &Config) -> Option<String> {
    let g = &cfg.gate;
    if !r.passed_or_warned() {
        return Some(format!("result {}: {}", r.statement.predicate.result, r.run.reason.clone().unwrap_or_default()));
    }
    if r.run.runners.is_empty() && !cfg.run.allow_unrecognized {
        return Some(
            "no test runner summary was recognised (the pass rests on the exit status alone) and the policy does not set run.allow_unrecognized"
                .to_string(),
        );
    }
    if r.run.counts.passed < g.min_tests {
        return Some(format!("{} passed tests, policy needs at least {}", r.run.counts.passed, g.min_tests));
    }
    if r.run.mode == MODE_OBSERVED && !g.accept_observed {
        return Some(
            "recorded from the agent's own shell output (observed); only `treeproof run` receipts count unless gate.accept_observed is set"
                .to_string(),
        );
    }
    let exact_pin = if g.commands.is_empty() {
        false
    } else {
        let matching: Vec<&String> = g.commands.iter().filter(|p| command_matches(p, &r.run.argv, r.run.shell)).collect();
        if matching.is_empty() {
            return Some(format!("command {:?} is not one of the required commands {:?}", r.command_line(), g.commands));
        }
        matching.iter().any(|p| !p.trim_end().ends_with('*'))
    };
    if !exact_pin && !g.allow_subset {
        if let Some(why) = parse::subset_reason(&parse::command_words(&r.run.argv, r.run.shell), &r.run.notes) {
            return Some(format!(
                "this run selected a subset of the tests ({why}); run the whole suite, or pin the command with gate.commands or set gate.allow_subset"
            ));
        }
    }
    None
}

/// A failing receipt that records real test failures (not a typo or an unrecognised command).
fn records_test_failure(r: &Receipt) -> bool {
    !r.passed_or_warned()
        && (r.run.counts.failed > 0
            || !r.statement.predicate.failed_tests.is_empty()
            || (!r.run.runners.is_empty() && r.run.exit_code.is_some_and(|c| c != 0)))
}

/// Normalised command words, for "the same command" comparisons.
fn command_key(r: &Receipt) -> Vec<String> {
    let mut w = parse::command_words(&r.run.argv, r.run.shell);
    if let Some(first) = w.first_mut() {
        *first = parse::prog_name(first);
    }
    w
}

/// Evaluate the gate for `tree` (or the current working tree) of the repository at `root`,
/// with the default store and key.
pub fn evaluate(root: &Path, cfg: &Config, tree: Option<String>) -> Result<GateReport> {
    evaluate_in(root, cfg, tree, &Context::default())
}

/// Evaluate the gate with the store and key from `ctx`.
///
/// `cfg` should be a policy the agent cannot write: [`Config::load_committed`], or the
/// snapshot a hook session took when it started. Receipts are trusted only when signed by this
/// machine's key or a key in `TREEPROOF_TRUSTED_KEYS`, never by keys in the working copy.
pub fn evaluate_in(root: &Path, cfg: &Config, tree: Option<String>, ctx: &Context) -> Result<GateReport> {
    let tree = match tree {
        Some(t) => t,
        None => crate::git::tree_digest(root)?,
    };
    let nested_now = crate::git::nested_trees(root)?;
    let store = ctx.open_store()?;
    let trusted = keys::local_trusted_keys(ctx)?;
    // Newest first.
    let mut items: Vec<(ReceiptBrief, Option<Receipt>)> = Vec::new();
    for (path, text) in store.receipts_for_tree(&tree) {
        let r = match receipt::open_text(&text, &trusted) {
            Ok(r) => r,
            Err(e) => {
                items.push((
                    ReceiptBrief {
                        path,
                        result: "INVALID".into(),
                        command: String::new(),
                        mode: String::new(),
                        counts: Counts::default(),
                        finished_at: String::new(),
                        accepted: false,
                        why_not: Some(e.to_string()),
                    },
                    None,
                ));
                continue;
            }
        };
        let why_not = if r.tree != tree {
            Some("receipt is for a different tree".to_string())
        } else if r.nested_trees() != nested_now {
            Some(nested_mismatch(&r.nested_trees(), &nested_now))
        } else {
            policy_rejection(&r, cfg)
        };
        let brief = ReceiptBrief {
            path,
            result: r.statement.predicate.result.clone(),
            command: r.command_line(),
            mode: r.run.mode.clone(),
            counts: r.run.counts,
            finished_at: r.run.finished_at.clone(),
            accepted: why_not.is_none(),
            why_not,
        };
        items.push((brief, Some(r)));
    }

    // A failing run of a command blocks the gate until a later run of the same command passes,
    // so a passing subset cannot outweigh a failing full run on the same tree.
    let mut unresolved: Vec<String> = Vec::new();
    for (i, (b, r)) in items.iter().enumerate() {
        let Some(r) = r else { continue };
        if r.tree != tree || !records_test_failure(r) {
            continue;
        }
        let key = command_key(r);
        let resolved = items[..i].iter().any(|(_, n)| {
            n.as_ref().is_some_and(|n| {
                n.tree == tree && n.passed_or_warned() && command_key(n) == key && (n.run.mode != MODE_OBSERVED || cfg.gate.accept_observed)
            })
        });
        if !resolved {
            unresolved.push(format!(
                "`{}` ({} failed)",
                b.command,
                r.run.counts.failed.max(r.statement.predicate.failed_tests.len() as u64)
            ));
        }
    }
    if !unresolved.is_empty() {
        for (b, _) in items.iter_mut() {
            if b.accepted {
                b.accepted = false;
                b.why_not = Some(format!("outweighed by the unresolved failure of {}", unresolved.join(", ")));
            }
        }
    }

    let considered: Vec<ReceiptBrief> = items.into_iter().map(|(b, _)| b).collect();
    let chosen = considered.iter().find(|b| b.accepted).cloned();
    let suggestion = format!("treeproof run -- {}", suggest_command(root, cfg));
    let short = &tree[..12];
    let (ok, reason) = match &chosen {
        Some(b) => (true, format!("verified: `{}` {} on tree {short} ({} passed)", b.command, b.result, b.counts.passed)),
        None if !unresolved.is_empty() => (
            false,
            format!(
                "the latest run of {} on the current tree {short} FAILED and no later run of the same command passed. Fix the failures and re-run that command through treeproof; a passing run of a different or narrower command does not count.",
                unresolved.join(", ")
            ),
        ),
        None if considered.is_empty() => (
            false,
            format!("no treeproof receipt exists for the current tree {short}. Run the test suite through treeproof, for example `{suggestion}`, fix any failures, and finish only when it passes."),
        ),
        None => {
            let why: Vec<String> = considered.iter().filter_map(|b| b.why_not.clone().map(|w| format!("`{}`: {w}", b.command))).take(3).collect();
            (
                false,
                format!("no receipt for the current tree {short} satisfies the gate ({}). Run `{suggestion}` and finish only when it passes.", why.join("; ")),
            )
        }
    };
    Ok(GateReport { tree, ok, reason, receipt: chosen, considered, suggestion })
}

fn nested_mismatch(then: &BTreeMap<String, String>, now: &BTreeMap<String, String>) -> String {
    let mut paths: Vec<&String> = then.keys().chain(now.keys()).collect();
    paths.sort();
    paths.dedup();
    let changed: Vec<&str> = paths.into_iter().filter(|p| then.get(*p) != now.get(*p)).map(|p| p.as_str()).take(3).collect();
    format!("nested repository content changed since the run ({})", changed.join(", "))
}
