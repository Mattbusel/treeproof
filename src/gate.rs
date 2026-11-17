//! The "done" gate: is there a fresh, valid, passing receipt for the current tree?

use crate::config::{command_matches, Config};
use crate::receipt::{self, Counts, MODE_OBSERVED};
use crate::store::Store;
use crate::util::join_argv;
use crate::{keys, Result};
use serde::Serialize;
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

/// Evaluate the gate for `tree` (or the current working tree) of the repository at `root`.
pub fn evaluate(root: &Path, cfg: &Config, tree: Option<String>) -> Result<GateReport> {
    let tree = match tree {
        Some(t) => t,
        None => crate::git::tree_digest(root)?,
    };
    let store = Store::open()?;
    let trusted = keys::trusted_keys(Some(root), true, &[])?;
    let mut considered = Vec::new();
    let mut chosen: Option<ReceiptBrief> = None;
    for (path, text) in store.receipts_for_tree(&tree) {
        let r = match receipt::open_text(&text, &trusted) {
            Ok(r) => r,
            Err(e) => {
                considered.push(ReceiptBrief {
                    path,
                    result: "INVALID".into(),
                    command: String::new(),
                    mode: String::new(),
                    counts: Counts::default(),
                    finished_at: String::new(),
                    accepted: false,
                    why_not: Some(e.to_string()),
                });
                continue;
            }
        };
        let command = if r.run.shell { r.run.argv.join(" ") } else { join_argv(&r.run.argv) };
        let why_not = if r.tree != tree {
            Some("receipt is for a different tree".to_string())
        } else if !matches!(r.statement.predicate.result.as_str(), "PASSED" | "WARNED") {
            Some(format!("result {}: {}", r.statement.predicate.result, r.run.reason.clone().unwrap_or_default()))
        } else if r.run.counts.passed < cfg.gate.min_tests {
            Some(format!("{} passed tests, policy needs at least {}", r.run.counts.passed, cfg.gate.min_tests))
        } else if r.run.mode == MODE_OBSERVED && !cfg.gate.accept_observed {
            Some("observed receipts are not accepted by policy".to_string())
        } else if !cfg.gate.commands.is_empty() && !cfg.gate.commands.iter().any(|p| command_matches(p, &r.run.argv, r.run.shell)) {
            Some(format!("command {command:?} is not one of the required commands {:?}", cfg.gate.commands))
        } else {
            None
        };
        let brief = ReceiptBrief {
            path,
            result: r.statement.predicate.result.clone(),
            command,
            mode: r.run.mode.clone(),
            counts: r.run.counts,
            finished_at: r.run.finished_at.clone(),
            accepted: why_not.is_none(),
            why_not,
        };
        if brief.accepted && chosen.is_none() {
            chosen = Some(brief.clone());
        }
        considered.push(brief);
    }
    let suggestion = format!("treeproof run -- {}", suggest_command(root, cfg));
    let (ok, reason) = match &chosen {
        Some(b) => (true, format!("verified: `{}` {} on tree {} ({} passed)", b.command, b.result, &tree[..12], b.counts.passed)),
        None if considered.is_empty() => (
            false,
            format!("no treeproof receipt exists for the current tree {}. Run the test suite through treeproof, for example `{suggestion}`, fix any failures, and finish only when it passes.", &tree[..12]),
        ),
        None => {
            let why: Vec<String> = considered.iter().filter_map(|b| b.why_not.clone().map(|w| format!("`{}`: {w}", b.command))).take(3).collect();
            (
                false,
                format!(
                    "no receipt for the current tree {} satisfies the gate ({}). Run `{suggestion}` and finish only when it passes.",
                    &tree[..12],
                    why.join("; ")
                ),
            )
        }
    };
    Ok(GateReport { tree, ok, reason, receipt: chosen, considered, suggestion })
}
