//! Signing key storage and trusted public keys.
//!
//! The signing key lives outside the repository, in the user config directory
//! (`TREEPROOF_KEY_FILE` overrides the path). It is created on first use with
//! owner-only permissions on Unix. See `docs/threat-model.md` for what this does
//! and does not protect against.

use crate::dsse::PublicKey;
use crate::util::{hex, unhex};
use crate::{Error, Result};
use ed25519_dalek::SigningKey;
use std::path::{Path, PathBuf};

const PREFIX: &str = "treeproof-ed25519-seed-v1:";

/// Path of the signing key file.
pub fn key_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("TREEPROOF_KEY_FILE") {
        return Ok(PathBuf::from(p));
    }
    let base = dirs::config_dir().ok_or_else(|| Error::new("no user config directory; set TREEPROOF_KEY_FILE"))?;
    Ok(base.join("treeproof").join("signing.key"))
}

fn parse_seed(text: &str) -> Result<SigningKey> {
    let t = text.trim();
    let h = t.strip_prefix(PREFIX).unwrap_or(t);
    let b = unhex(h).ok_or_else(|| Error::new("signing key file is not valid hex"))?;
    let arr: [u8; 32] = b.as_slice().try_into().map_err(|_| Error::new("signing key seed must be 32 bytes"))?;
    Ok(SigningKey::from_bytes(&arr))
}

/// Load the signing key if it exists.
pub fn load() -> Result<Option<SigningKey>> {
    let p = key_path()?;
    match std::fs::read_to_string(&p) {
        Ok(t) => Ok(Some(parse_seed(&t)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::new(format!("read {}: {e}", p.display()))),
    }
}

/// Load the signing key, creating it on first use.
pub fn load_or_create() -> Result<SigningKey> {
    if let Some(k) = load()? {
        return Ok(k);
    }
    let p = key_path()?;
    create_at(&p)
}

/// Create a fresh key at `p`. Fails if the file already exists.
pub fn create_at(p: &Path) -> Result<SigningKey> {
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::new(format!("create {}: {e}", dir.display())))?;
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| Error::new(format!("system random source failed: {e}")))?;
    let text = format!("{PREFIX}{}\n", hex(&seed));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    match opts.open(p) {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(text.as_bytes()).map_err(|e| Error::new(format!("write {}: {e}", p.display())))?;
            Ok(SigningKey::from_bytes(&seed))
        }
        // Another process won the race: use its key.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let t = std::fs::read_to_string(p).map_err(|e| Error::new(format!("read {}: {e}", p.display())))?;
            parse_seed(&t)
        }
        Err(e) => Err(Error::new(format!("create {}: {e}", p.display()))),
    }
}

/// Public keys trusted for verification:
/// the local signing key (if `include_local`), every `*.pem`/`*.pub` file in
/// `<repo>/.treeproof/trusted_keys/`, every path in `TREEPROOF_TRUSTED_KEYS`
/// (separated like `PATH`), and `extra`.
pub fn trusted_keys(repo: Option<&Path>, include_local: bool, extra: &[PublicKey]) -> Result<Vec<PublicKey>> {
    let mut out: Vec<PublicKey> = Vec::new();
    let mut add = |k: PublicKey| {
        if !out.contains(&k) {
            out.push(k);
        }
    };
    for k in extra {
        add(k.clone());
    }
    if include_local {
        if let Some(sk) = load()? {
            add(PublicKey { key: sk.verifying_key() });
        }
    }
    if let Some(r) = repo {
        let dir = r.join(".treeproof").join("trusted_keys");
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut paths: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
            paths.sort();
            for p in paths {
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext == "pem" || ext == "pub" {
                    let t = std::fs::read_to_string(&p).map_err(|e| Error::new(format!("read {}: {e}", p.display())))?;
                    add(PublicKey::parse(&t).map_err(|e| Error::new(format!("{}: {e}", p.display())))?);
                }
            }
        }
    }
    if let Some(list) = std::env::var_os("TREEPROOF_TRUSTED_KEYS") {
        for p in std::env::split_paths(&list) {
            if p.as_os_str().is_empty() {
                continue;
            }
            let t = std::fs::read_to_string(&p).map_err(|e| Error::new(format!("read {}: {e}", p.display())))?;
            add(PublicKey::parse(&t).map_err(|e| Error::new(format!("{}: {e}", p.display())))?);
        }
    }
    Ok(out)
}
