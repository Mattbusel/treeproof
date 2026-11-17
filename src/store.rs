//! The shared on-disk store under the user cache directory.
//!
//! Layout (`<cache>/treeproof/v1/`):
//! - `runs/<k2>/<key>.json`: the signed receipt of a PASSED run, keyed by run key (the cross-worktree cache)
//! - `runs/<k2>/<key>.log`: the tail of that run's output, replayed on a cache hit
//! - `trees/<tree>/<key>.json`: the latest receipt (any result) for each run of a given tree
//! - `locks/<key>.lock`: single-flight locks so identical runs in parallel worktrees execute once
//! - `sessions/`: per-session hook state
//!
//! Every write is a write-to-temp plus rename, so readers never see half a file.
//! Failed runs are never written under `runs/`, so a failure is never served as a pass.

use crate::{Error, Result};
use std::fs::File;
use std::path::{Path, PathBuf};

pub struct Store {
    root: PathBuf,
}

/// Holds an exclusive file lock until dropped.
pub struct LockGuard {
    file: File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn check_id(id: &str) -> Result<()> {
    if id.len() >= 8 && id.len() <= 128 && id.bytes().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(Error::new(format!("invalid store id {id:?}")))
    }
}

/// Session ids come from the host; keep them to a safe file-name alphabet.
pub fn safe_name(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(128).collect();
    if cleaned.is_empty() {
        "default".to_string()
    } else {
        cleaned
    }
}

pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::new(format!("create {}: {e}", dir.display())))?;
    }
    let mut rnd = [0u8; 6];
    let _ = getrandom::fill(&mut rnd);
    let tmp = path.with_extension(format!("tmp-{}-{}", std::process::id(), crate::util::hex(&rnd)));
    std::fs::write(&tmp, data).map_err(|e| Error::new(format!("write {}: {e}", tmp.display())))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::new(format!("rename into {}: {e}", path.display())));
    }
    Ok(())
}

impl Store {
    /// Open the store at `TREEPROOF_CACHE_DIR`, or the user cache directory.
    pub fn open() -> Result<Store> {
        Store::open_in(None)
    }

    /// Open the store under `dir` (the same meaning as `TREEPROOF_CACHE_DIR`), or the default when `None`.
    pub fn open_in(dir: Option<&Path>) -> Result<Store> {
        let base = match dir {
            Some(d) => d.to_path_buf(),
            None => match std::env::var_os("TREEPROOF_CACHE_DIR") {
                Some(p) => PathBuf::from(p),
                None => dirs::cache_dir().ok_or_else(|| Error::new("no user cache directory; set TREEPROOF_CACHE_DIR"))?.join("treeproof"),
            },
        };
        Ok(Store::at(base.join("v1")))
    }

    /// The per-machine secret that environment values are keyed with before they enter a
    /// run key or a receipt (see SPEC.md, section 3.2). Created on first use.
    pub fn env_salt(&self) -> Result<Vec<u8>> {
        let p = self.root.join("env.salt");
        let parse = |t: &str| crate::util::unhex(t.trim()).filter(|b| b.len() == 32);
        if let Some(b) = std::fs::read_to_string(&p).ok().as_deref().and_then(parse) {
            return Ok(b);
        }
        std::fs::create_dir_all(&self.root).map_err(|e| Error::new(format!("create {}: {e}", self.root.display())))?;
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| Error::new(format!("system random source failed: {e}")))?;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        match opts.open(&p) {
            Ok(mut f) => {
                use std::io::Write;
                f.write_all(crate::util::hex(&seed).as_bytes()).map_err(|e| Error::new(format!("write {}: {e}", p.display())))?;
                Ok(seed.to_vec())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another process created it first and may still be writing it.
                for _ in 0..100 {
                    if let Some(b) = std::fs::read_to_string(&p).ok().as_deref().and_then(parse) {
                        return Ok(b);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(Error::new(format!("{} is not a valid salt; delete it to start over", p.display())))
            }
            Err(e) => Err(Error::new(format!("create {}: {e}", p.display()))),
        }
    }

    pub fn at(root: PathBuf) -> Store {
        Store { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn run_base(&self, key: &str) -> Result<PathBuf> {
        check_id(key)?;
        Ok(self.root.join("runs").join(&key[..2]).join(key))
    }

    /// Cached PASSED receipt for a run key.
    pub fn get_pass(&self, key: &str) -> Option<String> {
        let p = self.run_base(key).ok()?.with_extension("json");
        std::fs::read_to_string(p).ok()
    }

    /// Output tail stored with a cached pass.
    pub fn get_log(&self, key: &str) -> Option<String> {
        let p = self.run_base(key).ok()?.with_extension("log");
        std::fs::read_to_string(p).ok()
    }

    /// Store a PASSED receipt and its output tail in the run cache.
    pub fn put_pass(&self, key: &str, receipt_json: &str, log: &str) -> Result<()> {
        let base = self.run_base(key)?;
        write_atomic(&base.with_extension("log"), log.as_bytes())?;
        write_atomic(&base.with_extension("json"), receipt_json.as_bytes())
    }

    /// Remove a cache entry (used when a stored receipt fails verification).
    pub fn evict(&self, key: &str) {
        if let Ok(base) = self.run_base(key) {
            let _ = std::fs::remove_file(base.with_extension("json"));
            let _ = std::fs::remove_file(base.with_extension("log"));
        }
    }

    /// Record a receipt of any result under its tree.
    pub fn record(&self, tree: &str, key: &str, receipt_json: &str) -> Result<PathBuf> {
        check_id(tree)?;
        check_id(key)?;
        let p = self.root.join("trees").join(tree).join(format!("{key}.json"));
        write_atomic(&p, receipt_json.as_bytes())?;
        Ok(p)
    }

    /// All receipts recorded for a tree, newest first by modification time.
    pub fn receipts_for_tree(&self, tree: &str) -> Vec<(PathBuf, String)> {
        if check_id(tree).is_err() {
            return Vec::new();
        }
        let dir = self.root.join("trees").join(tree);
        let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
        let mut items: Vec<(std::time::SystemTime, PathBuf)> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
            .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p))
            .collect();
        items.sort_by_key(|x| std::cmp::Reverse(x.0));
        items.into_iter().filter_map(|(_, p)| std::fs::read_to_string(&p).ok().map(|t| (p, t))).collect()
    }

    /// Take the single-flight lock for a run key. Returns the guard and whether we had to wait.
    pub fn lock(&self, key: &str) -> Result<(LockGuard, bool)> {
        check_id(key)?;
        let dir = self.root.join("locks");
        std::fs::create_dir_all(&dir).map_err(|e| Error::new(format!("create {}: {e}", dir.display())))?;
        let p = dir.join(format!("{key}.lock"));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&p)
            .map_err(|e| Error::new(format!("open {}: {e}", p.display())))?;
        match file.try_lock() {
            Ok(()) => Ok((LockGuard { file }, false)),
            Err(std::fs::TryLockError::WouldBlock) => {
                file.lock().map_err(|e| Error::new(format!("lock {}: {e}", p.display())))?;
                Ok((LockGuard { file }, true))
            }
            Err(std::fs::TryLockError::Error(e)) => Err(Error::new(format!("lock {}: {e}", p.display()))),
        }
    }

    /// Directory for per-session hook state.
    pub fn session_path(&self, kind: &str, session: &str) -> PathBuf {
        self.root.join("sessions").join(kind).join(format!("{}.json", safe_name(session)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_checked() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::at(d.path().to_path_buf());
        assert!(s.record("../../etc", "abcdef0123", "{}").is_err());
        assert!(s.put_pass("abcdef01/../x", "{}", "").is_err());
        assert!(s.get_pass("zz").is_none());
        assert_eq!(safe_name("a/b\\c..d"), "a_b_c__d");
        assert_eq!(safe_name(""), "default");
    }

    #[test]
    fn put_get_record() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::at(d.path().to_path_buf());
        let key = "ab".repeat(32);
        let tree = "cd".repeat(20);
        s.put_pass(&key, "{\"r\":1}", "log").unwrap();
        assert_eq!(s.get_pass(&key).unwrap(), "{\"r\":1}");
        assert_eq!(s.get_log(&key).unwrap(), "log");
        s.record(&tree, &key, "{\"r\":1}").unwrap();
        assert_eq!(s.receipts_for_tree(&tree).len(), 1);
        s.evict(&key);
        assert!(s.get_pass(&key).is_none());
    }

    #[test]
    fn lock_is_exclusive_across_handles() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::at(d.path().to_path_buf());
        let key = "ef".repeat(32);
        let (g, waited) = s.lock(&key).unwrap();
        assert!(!waited);
        let p = d.path().join("locks").join(format!("{key}.lock"));
        let f2 = std::fs::OpenOptions::new().read(true).write(true).open(&p).unwrap();
        assert!(matches!(f2.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        drop(g);
        assert!(f2.try_lock().is_ok());
    }
}
