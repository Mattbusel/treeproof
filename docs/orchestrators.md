# Using treeproof from an orchestrator

Orchestrators that run several agents (worktree managers, kanban boards, task runners) can gate "merged" or "verified" on a receipt instead of on the agent's word.

## Shell out

```sh
treeproof gate --json        # exit 0 when a valid passing receipt covers the current tree
treeproof status --json      # same report; never needed for the decision, useful for UIs
treeproof run --json -- npm test
treeproof verify --json --tree current --rerun path/to/receipt.json
```

`treeproof gate --json` prints:

```json
{
  "tree": "59c443ae396bebbe0e43206ad932ada076cf76ae",
  "ok": true,
  "reason": "verified: `cargo test` PASSED on tree 59c443ae396b (60 passed)",
  "receipt": { "path": "...", "result": "PASSED", "command": "cargo test", "mode": "fresh", "counts": {"passed": 60, "failed": 0, "skipped": 0, "warned": 0}, "finished_at": "...", "accepted": true, "why_not": null },
  "considered": [ ... ],
  "suggestion": "treeproof run -- cargo test"
}
```

Exit codes: `0` ok, `1` not ok, `2` treeproof itself failed (not a git repository, bad policy file).

## As a Rust dependency

```toml
[dependencies]
treeproof = "0.1"
```

```rust
use treeproof::{config::Config, gate, git};

fn verified(dir: &std::path::Path) -> treeproof::Result<bool> {
    let root = git::repo_root(dir)?;
    // The committed policy, never the working copy the agent can write.
    let report = gate::evaluate(&root, &Config::load_committed(&root)?, None)?;
    if !report.ok {
        eprintln!("not verified: {}", report.reason);
    }
    Ok(report.ok)
}
```

Run and sign with explicit store and key locations (no process-wide environment variables, so it is safe in parallel tests):

```rust
use treeproof::run::{run_in, RunOptions};
use treeproof::Context;

let ctx = Context::new().with_store_dir("/var/lib/orchestrator/treeproof").with_key_file("/etc/orchestrator/treeproof.key");
let out = run_in(&RunOptions::shell_line("cargo test --workspace", worktree), &ctx)?;
if out.passed() { /* out.receipt_json is the signed receipt */ }
```

Other entry points: `treeproof::gate::evaluate_in` (gate with a `Context`), `treeproof::gate::policy_rejection` (apply a policy to a receipt), `treeproof::receipt::open_text` (verify a receipt), `treeproof::verify::verify_text` (verify, optionally against a tree and with a re-run), `treeproof::keys::trusted_keys_at_rev` (keys committed at a revision), `treeproof::git::tree_digest`.

If you gate merges, read the policy and trusted keys from the target branch (`Config::load_at_rev(root, "origin/main")`, `keys::trusted_keys_at_rev`), not from the agent's branch.

## Worktrees

All worktrees on a machine share one store, so an orchestrator that fans one task out to several worktrees pays for each distinct tree once. See [cache.md](cache.md).
