//! Per-repository policy, read from `treeproof.toml` at the repository root.
//!
//! The gate never reads the policy from the working copy, which an agent can write.
//! Hooks use the policy committed at `HEAD` when the session started; `treeproof gate`,
//! `status` and `export` use the policy committed at `HEAD` (or `--policy-ref`); the
//! GitHub Action uses the policy at the pull request's base commit. See
//! `docs/configuration.md`.

use crate::{Error, Result};
use serde::Deserialize;
use std::path::Path;

pub const CONFIG_FILE: &str = "treeproof.toml";

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub gate: GateConfig,
    pub run: RunConfig,
    pub fileguard: FileGuardConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GateConfig {
    /// `block` (default), `warn` (allow but tell the agent), or `off`.
    pub mode: String,
    /// If non-empty, only receipts whose command matches one of these count.
    /// A trailing `*` matches any further arguments.
    pub commands: Vec<String>,
    /// Minimum passed tests for a receipt to satisfy the gate.
    pub min_tests: u64,
    /// Accept receipts recorded from a host's own Bash output (PostToolUse). Off by default:
    /// an agent controls what its own shell prints.
    pub accept_observed: bool,
    /// Accept receipts from runs that selected a subset of the tests (name filters such as
    /// `cargo test foo`, `pytest -k`, `pytest file.py::test`, `go test -run`, `jest -t`).
    pub allow_subset: bool,
    /// After this many consecutive blocks on the same tree in one session, let the agent stop with a warning.
    pub max_blocks: u32,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RunConfig {
    /// Extra environment variable names that are part of the cache key.
    pub env: Vec<String>,
    /// Sign a receipt even when no runner summary is recognised (counts will be zero).
    pub allow_unrecognized: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct FileGuardConfig {
    pub enabled: bool,
}

impl Default for GateConfig {
    fn default() -> Self {
        GateConfig { mode: "block".into(), commands: Vec::new(), min_tests: 1, accept_observed: false, allow_subset: false, max_blocks: 3 }
    }
}

impl Default for FileGuardConfig {
    fn default() -> Self {
        FileGuardConfig { enabled: true }
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Config> {
        let c: Config = toml::from_str(text).map_err(|e| Error::new(format!("{CONFIG_FILE}: {e}")))?;
        if !matches!(c.gate.mode.as_str(), "block" | "warn" | "off") {
            return Err(Error::new(format!("{CONFIG_FILE}: gate.mode must be block, warn or off")));
        }
        Ok(c)
    }

    /// Load the policy committed at `rev` (for example `HEAD` or a base branch), or defaults when
    /// the file does not exist there. Also returns the file text (empty when absent).
    pub fn load_at_rev(root: &Path, rev: &str) -> Result<(Config, String)> {
        match crate::git::show_file(root, rev, CONFIG_FILE)? {
            Some(t) => Ok((Config::parse(&t).map_err(|e| Error::new(format!("{rev}:{e}")))?, t)),
            None => Ok((Config::default(), String::new())),
        }
    }

    /// The policy the gate uses outside a hook session: the one committed at `HEAD`, or
    /// defaults on an unborn branch or when no `treeproof.toml` is committed.
    pub fn load_committed(root: &Path) -> Result<Config> {
        if crate::git::head_commit(root).is_none() {
            return Ok(Config::default());
        }
        Config::load_at_rev(root, "HEAD").map(|(c, _)| c)
    }

    /// Load `treeproof.toml` from the working copy at `root`, or defaults when absent.
    ///
    /// The working copy is writable by an agent, so the gate does not use this; see
    /// [`Config::load_committed`].
    pub fn load(root: &Path) -> Result<Config> {
        match std::fs::read_to_string(root.join(CONFIG_FILE)) {
            Ok(t) => Config::parse(&t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(Error::new(format!("read {CONFIG_FILE}: {e}"))),
        }
    }
}

/// Does a receipt's command (argv or shell string) match a policy pattern?
pub fn command_matches(pattern: &str, argv: &[String], shell: bool) -> bool {
    let actual: Vec<&str> = if shell {
        argv.first().map(|s| s.split_whitespace().collect()).unwrap_or_default()
    } else {
        argv.iter().map(|s| s.as_str()).collect()
    };
    let pat = pattern.trim();
    let (pat, prefix) = match pat.strip_suffix('*') {
        Some(p) => (p.trim_end(), true),
        None => (pat, false),
    };
    let want: Vec<&str> = pat.split_whitespace().collect();
    if prefix {
        actual.len() >= want.len() && actual[..want.len()] == want[..]
    } else {
        actual == want
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_parse() {
        let c = Config::parse("").unwrap();
        assert_eq!(c, Config::default());
        let c = Config::parse("[gate]\ncommands = [\"cargo test --workspace\"]\nmin_tests = 5\n").unwrap();
        assert_eq!(c.gate.min_tests, 5);
        assert!(Config::parse("[gate]\nmode = \"maybe\"\n").is_err());
        assert!(Config::parse("[gate]\nunknown = 1\n").is_err());
        assert!(!Config::default().gate.accept_observed);
        assert!(!Config::default().gate.allow_subset);
    }

    #[test]
    fn matching() {
        let a = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert!(command_matches("cargo test --workspace", &a("cargo test --workspace"), false));
        assert!(!command_matches("cargo test --workspace", &a("cargo test --workspace foo"), false));
        assert!(command_matches("cargo test *", &a("cargo test --workspace foo"), false));
        assert!(!command_matches("cargo test *", &a("cargo build"), false));
        assert!(command_matches("npm test", &["npm   test".to_string()], true));
    }
}
