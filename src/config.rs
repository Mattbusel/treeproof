//! Per-repository policy, read from `treeproof.toml` at the repository root.
//!
//! The file is part of the tree, so a change to the policy changes the digest
//! and needs a fresh receipt like any other change.

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
    /// Accept receipts recorded from a host's own Bash output (PostToolUse).
    pub accept_observed: bool,
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
        GateConfig { mode: "block".into(), commands: Vec::new(), min_tests: 1, accept_observed: true, max_blocks: 3 }
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

    /// Load `treeproof.toml` from `root`, or defaults when absent.
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
