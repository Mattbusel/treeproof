//! # treeproof
//!
//! Signed test-result receipts for a git tree.
//!
//! `treeproof run -- cargo test` runs the command once per identical input
//! (working-tree digest, argv, toolchain versions, lockfiles, allowlisted
//! environment), parses the runner's summary, and signs an
//! [in-toto Statement v1](https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md)
//! with the [test-result v0.1](https://github.com/in-toto/attestation/blob/main/spec/predicates/test-result.md)
//! predicate in a [DSSE](https://github.com/secure-systems-lab/dsse) envelope (Ed25519).
//! The subject is the `gitTree` digest of the working tree, so a receipt says
//! "this exact tree passed this exact command".
//!
//! Passing receipts are cached in a content-addressed store under the user
//! cache directory and shared by every worktree on the machine; an identical
//! run in another worktree is a cache hit, and concurrent identical runs execute
//! once. Failures are never served from the cache.
//!
//! The same receipts back a done-gate for coding agents (Claude Code and Codex
//! hooks, an MCP server, and a GitHub Action that re-runs the command in CI).
//! The format is specified in `SPEC.md` so others can produce and verify
//! receipts without this crate.
//!
//! ```no_run
//! use treeproof::run::{run, RunOptions};
//! let out = run(&RunOptions {
//!     argv: vec!["cargo".into(), "test".into()],
//!     shell: false,
//!     cwd: std::env::current_dir().unwrap(),
//!     no_cache: false,
//!     echo: true,
//!     allow_unrecognized: None,
//!     timeout: None,
//! }).unwrap();
//! println!("{} {} ({} passed)", out.status, out.result, out.counts.passed);
//! ```

pub mod config;
pub mod dsse;
pub mod fileguard;
pub mod fingerprint;
pub mod gate;
pub mod git;
pub mod hooks;
pub mod keys;
pub mod mcp;
pub mod otlp;
pub mod parse;
pub mod receipt;
pub mod run;
pub mod store;
pub mod util;
pub mod verify;

/// The crate's error type: a message meant for a human.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    pub fn new(msg: impl Into<String>) -> Error {
        Error(msg.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error(format!("json: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
