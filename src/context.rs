//! Where treeproof keeps its store and signing key, for library callers.
//!
//! The CLI and hooks use the defaults (`TREEPROOF_CACHE_DIR`, `TREEPROOF_KEY_FILE`, then the
//! user cache and config directories). A library caller, for example a test suite or an
//! orchestrator that runs several isolated projects in one process, can pass a [`Context`]
//! instead of setting process-wide environment variables.

use crate::store::Store;
use crate::Result;
use ed25519_dalek::SigningKey;
use std::path::PathBuf;

/// Store and key locations. `Context::default()` means "use the environment and the user
/// directories", which is what the CLI does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Context {
    /// Parent directory of the store (the same meaning as `TREEPROOF_CACHE_DIR`).
    pub store_dir: Option<PathBuf>,
    /// Signing key file (the same meaning as `TREEPROOF_KEY_FILE`). Created on first use.
    pub key_file: Option<PathBuf>,
}

impl Context {
    /// The default context (environment variables, then user directories).
    pub fn new() -> Context {
        Context::default()
    }

    /// Use `dir` as the store parent directory.
    pub fn with_store_dir(mut self, dir: impl Into<PathBuf>) -> Context {
        self.store_dir = Some(dir.into());
        self
    }

    /// Use `path` as the signing key file.
    pub fn with_key_file(mut self, path: impl Into<PathBuf>) -> Context {
        self.key_file = Some(path.into());
        self
    }

    /// Open the store for this context.
    pub fn open_store(&self) -> Result<Store> {
        Store::open_in(self.store_dir.as_deref())
    }

    /// Path of the signing key for this context.
    pub fn key_path(&self) -> Result<PathBuf> {
        match &self.key_file {
            Some(p) => Ok(p.clone()),
            None => crate::keys::key_path(),
        }
    }

    /// Load the signing key, creating it on first use.
    pub fn signing_key(&self) -> Result<SigningKey> {
        crate::keys::load_or_create_at(&self.key_path()?)
    }

    /// Load the signing key if it exists.
    pub fn existing_signing_key(&self) -> Result<Option<SigningKey>> {
        crate::keys::load_from(&self.key_path()?)
    }
}
