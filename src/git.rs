//! Git plumbing: repository discovery and the working-tree digest.
//!
//! The digest of a working tree is the git tree object id that `git add -A`
//! followed by `git write-tree` would produce, computed in a throwaway index so
//! the user's real index is never touched. Tracked, modified and untracked
//! (non-ignored) files all count; ignored files do not. A small fixed set of
//! paths is removed before hashing (see [`DIGEST_EXCLUDES`]) so receipts can be
//! committed next to the code and Python bytecode does not change the digest.

use crate::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Pathspecs removed from the throwaway index before `git write-tree`.
pub const DIGEST_EXCLUDES: &[&str] = &[":(glob).treeproof/**", ":(glob)**/__pycache__/**", ":(glob)**/*.pyc", ":(glob)**/.pytest_cache/**"];

fn git_cmd(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir);
    c.args(["-c", "core.quotepath=off"]);
    c.env("GIT_TERMINAL_PROMPT", "0");
    c.env_remove("GIT_INDEX_FILE");
    c
}

fn run_git(mut c: Command, what: &str) -> Result<String> {
    let out = c.output().map_err(|e| Error::new(format!("could not run git ({what}): {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(Error::new(format!("git {what} failed: {}", err.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The top-level directory of the work tree containing `dir`.
pub fn repo_root(dir: &Path) -> Result<PathBuf> {
    let mut c = git_cmd(dir);
    c.args(["rev-parse", "--show-toplevel"]);
    let s = run_git(c, "rev-parse --show-toplevel")?;
    Ok(PathBuf::from(s))
}

/// `HEAD` commit id, or `None` on an unborn branch.
pub fn head_commit(root: &Path) -> Option<String> {
    let mut c = git_cmd(root);
    c.args(["rev-parse", "--verify", "--quiet", "HEAD"]);
    run_git(c, "rev-parse HEAD").ok().filter(|s| !s.is_empty())
}

/// Tree id of `HEAD`, or `None` on an unborn branch.
pub fn head_tree(root: &Path) -> Option<String> {
    let mut c = git_cmd(root);
    c.args(["rev-parse", "--verify", "--quiet", "HEAD^{tree}"]);
    run_git(c, "rev-parse HEAD^{tree}").ok().filter(|s| !s.is_empty())
}

fn index_path(root: &Path) -> Result<PathBuf> {
    let mut c = git_cmd(root);
    c.args(["rev-parse", "--path-format=absolute", "--git-path", "index"]);
    Ok(PathBuf::from(run_git(c, "rev-parse --git-path index")?))
}

struct TempIndex(PathBuf);

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let mut lock = self.0.clone().into_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(PathBuf::from(lock));
    }
}

/// Git tree id of the working tree rooted at `root` (see the module docs).
pub fn tree_digest(root: &Path) -> Result<String> {
    let real = index_path(root)?;
    let mut rnd = [0u8; 8];
    let _ = getrandom::fill(&mut rnd);
    let tmp = std::env::temp_dir().join(format!("treeproof-index-{}-{}", std::process::id(), crate::util::hex(&rnd)));
    let guard = TempIndex(tmp.clone());
    if real.exists() {
        std::fs::copy(&real, &tmp).map_err(|e| Error::new(format!("copy index: {e}")))?;
        // Keep the index's own timestamp. Git treats an entry as "racily clean" (and re-hashes
        // the file) when the file's mtime is not older than the index file's mtime. A copy with
        // a fresh mtime would hide a same-size edit made in the same second as the last index
        // write, and the digest would miss the change.
        let mtime = std::fs::metadata(&real).and_then(|m| m.modified()).map_err(|e| Error::new(format!("stat index: {e}")))?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&tmp)
            .and_then(|f| f.set_modified(mtime))
            .map_err(|e| Error::new(format!("set index time: {e}")))?;
    }

    let mut add = git_cmd(root);
    add.env("GIT_INDEX_FILE", &tmp);
    add.args(["add", "-A", "--", "."]);
    run_git(add, "add -A (temporary index)")?;

    let mut rm = git_cmd(root);
    rm.env("GIT_INDEX_FILE", &tmp);
    rm.args(["rm", "-r", "-q", "--cached", "--ignore-unmatch", "--"]);
    rm.args(DIGEST_EXCLUDES);
    run_git(rm, "rm --cached (temporary index)")?;

    let mut wt = git_cmd(root);
    wt.env("GIT_INDEX_FILE", &tmp);
    wt.arg("write-tree");
    let oid = run_git(wt, "write-tree")?;
    drop(guard);
    if !crate::util::is_git_oid(&oid) {
        return Err(Error::new(format!("git write-tree returned an unexpected id: {oid:?}")));
    }
    Ok(oid)
}

/// Path of `dir` relative to `root`, with forward slashes; `"."` for the root itself.
pub fn rel_path(root: &Path, dir: &Path) -> String {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let (r, d) = (canon(root), canon(dir));
    match d.strip_prefix(&r) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => ".".to_string(),
    }
}
