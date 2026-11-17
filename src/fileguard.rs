//! FileGuard: refuse an edit when the file changed on disk since the agent last read it.
//!
//! PostToolUse on a read (or on the agent's own edit) records the file's content
//! hash for the session. PreToolUse on an edit compares the current content hash
//! with the recorded one and denies the edit when they differ, which catches a
//! parallel agent or another process writing the same file in between.

use crate::store::{write_atomic, Store};
use crate::util::sha256_hex;
use crate::Result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const KIND: &str = "fileguard";
/// Hash of a file that does not exist.
pub const ABSENT: &str = "absent";

fn hash_file(p: &Path) -> String {
    match std::fs::read(p) {
        Ok(b) => sha256_hex(&b),
        Err(_) => ABSENT.to_string(),
    }
}

fn norm(p: &Path) -> String {
    let abs = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").unwrap_or(&s).to_string();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

fn load(store: &Store, session: &str) -> BTreeMap<String, String> {
    std::fs::read_to_string(store.session_path(KIND, session)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(store: &Store, session: &str, map: &BTreeMap<String, String>) -> Result<()> {
    write_atomic(&store.session_path(KIND, session), serde_json::to_string(map)?.as_bytes())
}

/// Record the current content of each path as "seen" by the session.
pub fn record(store: &Store, session: &str, paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut map = load(store, session);
    for p in paths {
        map.insert(norm(p), hash_file(p));
    }
    save(store, session, &map)
}

/// Paths whose content changed since the session last saw them.
pub fn stale(store: &Store, session: &str, paths: &[PathBuf]) -> Vec<PathBuf> {
    let map = load(store, session);
    paths
        .iter()
        .filter(|p| match map.get(&norm(p)) {
            Some(seen) => *seen != hash_file(p),
            None => false,
        })
        .cloned()
        .collect()
}

/// File paths touched by a Codex `apply_patch` body, resolved against `cwd`.
pub fn patch_paths(patch: &str, cwd: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for l in patch.lines() {
        let l = l.trim();
        for prefix in ["*** Update File: ", "*** Delete File: ", "*** Add File: ", "*** Move to: "] {
            if let Some(rest) = l.strip_prefix(prefix) {
                let p = Path::new(rest.trim());
                out.push(if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_external_write() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::at(d.path().join("store"));
        let f = d.path().join("a.txt");
        std::fs::write(&f, "one").unwrap();
        record(&store, "s1", std::slice::from_ref(&f)).unwrap();
        assert!(stale(&store, "s1", std::slice::from_ref(&f)).is_empty());
        std::fs::write(&f, "two").unwrap();
        assert_eq!(stale(&store, "s1", std::slice::from_ref(&f)), vec![f.clone()]);
        // Another session never read it, so it is not stale for them.
        assert!(stale(&store, "s2", std::slice::from_ref(&f)).is_empty());
        record(&store, "s1", std::slice::from_ref(&f)).unwrap();
        assert!(stale(&store, "s1", &[f]).is_empty());
    }

    #[test]
    fn patch_paths_parse() {
        let p = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-x\n+y\n*** Add File: new.txt\n+hi\n*** End Patch\n";
        let got = patch_paths(p, Path::new("/repo"));
        assert_eq!(got, vec![PathBuf::from("/repo/src/a.rs"), PathBuf::from("/repo/new.txt")]);
    }
}
