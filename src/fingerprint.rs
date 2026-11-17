//! The run key: what has to be identical for a cached pass to count.
//!
//! key = SHA-256 over a canonical JSON object holding the working-tree digest,
//! the argv, the working directory relative to the repository root, toolchain
//! versions for the runners the command uses, lockfile hashes, allowlisted
//! environment variables (hashed), and the OS and architecture.

use crate::util::sha256_hex;
use crate::Result;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// Lockfiles hashed into the key (looked up at the repository root and in the working directory).
pub const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "poetry.lock",
    "uv.lock",
    "Pipfile.lock",
    "pdm.lock",
    "go.sum",
    "go.work.sum",
];

/// Environment variables that are part of the key by default.
pub const DEFAULT_ENV: &[&str] = &[
    "RUSTFLAGS",
    "RUSTDOCFLAGS",
    "CARGO_BUILD_TARGET",
    "CARGO_PROFILE",
    "PYTHONPATH",
    "PYTHONHASHSEED",
    "PYTEST_ADDOPTS",
    "NODE_ENV",
    "NODE_OPTIONS",
    "GOFLAGS",
    "GOOS",
    "GOARCH",
    "CGO_ENABLED",
    "GOEXPERIMENT",
];

#[derive(Clone, Debug, Default)]
pub struct Fingerprint {
    pub tree: String,
    pub key: String,
    pub cwd: String,
    pub toolchains: BTreeMap<String, String>,
    pub lockfiles: BTreeMap<String, String>,
    pub env: BTreeMap<String, Option<String>>,
    pub os: String,
    pub arch: String,
}

fn words(argv: &[String], shell: bool) -> Vec<String> {
    let src: Vec<String> = if shell {
        argv.iter()
            .flat_map(|s| {
                s.split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')')).map(String::from).collect::<Vec<_>>()
            })
            .collect()
    } else {
        argv.to_vec()
    };
    src.into_iter()
        .filter(|w| !w.is_empty())
        .map(|w| {
            let base = w.rsplit(['/', '\\']).next().unwrap_or(&w).to_ascii_lowercase();
            base.trim_end_matches(".exe").trim_end_matches(".cmd").trim_end_matches(".bat").to_string()
        })
        .collect()
}

fn version_of(dir: &Path, prog: &str, args: &[&str]) -> String {
    let resolved = crate::run::resolve_program(prog);
    match Command::new(&resolved).args(args).current_dir(dir).output() {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            let s = if s.trim().is_empty() { String::from_utf8_lossy(&o.stderr).to_string() } else { s.to_string() };
            s.lines().next().unwrap_or("").trim().to_string()
        }
        Ok(o) => format!("unavailable (exit {})", o.status.code().unwrap_or(-1)),
        Err(_) => "unavailable".to_string(),
    }
}

/// Toolchain versions relevant to the command's words.
pub fn toolchains(dir: &Path, argv: &[String], shell: bool) -> BTreeMap<String, String> {
    let w = words(argv, shell);
    let any = |names: &[&str]| w.iter().any(|x| names.contains(&x.as_str()));
    let mut out = BTreeMap::new();
    if any(&["cargo", "rustc", "cargo-nextest", "nextest", "rustup"]) {
        out.insert("rustc".into(), version_of(dir, "rustc", &["-V"]));
        out.insert("cargo".into(), version_of(dir, "cargo", &["-V"]));
    }
    if any(&["python", "python3", "py", "pytest", "py.test", "uv", "tox", "nox", "poetry", "hatch", "pdm", "pipenv"]) {
        let py = if cfg!(windows) { "python" } else { "python3" };
        out.insert("python".into(), version_of(dir, py, &["--version"]));
    }
    if any(&["node", "npm", "npx", "pnpm", "yarn", "jest", "vitest", "tsx", "ts-node"]) {
        out.insert("node".into(), version_of(dir, "node", &["--version"]));
    }
    if any(&["bun", "bunx"]) {
        out.insert("bun".into(), version_of(dir, "bun", &["--version"]));
    }
    if any(&["deno"]) {
        out.insert("deno".into(), version_of(dir, "deno", &["--version"]));
    }
    if any(&["go", "gotestsum"]) {
        out.insert("go".into(), version_of(dir, "go", &["version"]));
    }
    out
}

/// SHA-256 of each lockfile present at the root or in `cwd`, keyed by path relative to the root.
pub fn lockfiles(root: &Path, cwd: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let rel = crate::git::rel_path(root, cwd);
    for dir_rel in [".".to_string(), rel] {
        let dir = if dir_rel == "." { root.to_path_buf() } else { root.join(&dir_rel) };
        for name in LOCKFILES {
            if let Ok(b) = std::fs::read(dir.join(name)) {
                let key = if dir_rel == "." { name.to_string() } else { format!("{dir_rel}/{name}") };
                out.insert(key, sha256_hex(&b));
            }
        }
    }
    out
}

/// Hashed values of the allowlisted environment variables.
pub fn env_values(extra: &[String]) -> BTreeMap<String, Option<String>> {
    let mut out = BTreeMap::new();
    for name in DEFAULT_ENV.iter().map(|s| s.to_string()).chain(extra.iter().cloned()) {
        let v = std::env::var_os(&name).map(|v| sha256_hex(v.to_string_lossy().as_bytes()));
        out.insert(name, v);
    }
    out
}

/// Compute the full fingerprint for running `argv` in `cwd` inside the repository at `root`.
pub fn compute(root: &Path, cwd: &Path, argv: &[String], shell: bool, extra_env: &[String]) -> Result<Fingerprint> {
    let tree = crate::git::tree_digest(root)?;
    Ok(with_tree(tree, root, cwd, argv, shell, extra_env))
}

/// Fingerprint for an already-computed tree digest.
pub fn with_tree(tree: String, root: &Path, cwd: &Path, argv: &[String], shell: bool, extra_env: &[String]) -> Fingerprint {
    let mut f = Fingerprint {
        tree,
        key: String::new(),
        cwd: crate::git::rel_path(root, cwd),
        toolchains: toolchains(cwd, argv, shell),
        lockfiles: lockfiles(root, cwd),
        env: env_values(extra_env),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    };
    f.key = key_of(&f, argv, shell);
    f
}

/// The canonical key computation (documented in SPEC.md).
pub fn key_of(f: &Fingerprint, argv: &[String], shell: bool) -> String {
    let v = serde_json::json!({
        "v": 1,
        "tree": f.tree,
        "argv": argv,
        "shell": shell,
        "cwd": f.cwd,
        "toolchains": f.toolchains,
        "lockfiles": f.lockfiles,
        "env": f.env,
        "os": f.os,
        "arch": f.arch,
    });
    sha256_hex(v.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_split_shell() {
        let w = words(&["cd sub && /usr/bin/cargo test; npm.cmd run x".to_string()], true);
        assert!(w.contains(&"cargo".to_string()));
        assert!(w.contains(&"npm".to_string()));
    }

    #[test]
    fn key_changes_with_inputs() {
        let base = Fingerprint { tree: "a".repeat(40), cwd: ".".into(), os: "linux".into(), arch: "x86_64".into(), ..Default::default() };
        let argv = vec!["cargo".to_string(), "test".to_string()];
        let k1 = key_of(&base, &argv, false);
        assert_eq!(k1, key_of(&base, &argv, false));
        let mut other = base.clone();
        other.tree = "b".repeat(40);
        assert_ne!(k1, key_of(&other, &argv, false));
        assert_ne!(k1, key_of(&base, &["cargo".into(), "test".into(), "x".into()], false));
        assert_ne!(k1, key_of(&base, &argv, true));
        let mut e = base.clone();
        e.env.insert("RUSTFLAGS".into(), Some("x".into()));
        assert_ne!(k1, key_of(&e, &argv, false));
    }
}
