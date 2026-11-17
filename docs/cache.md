# The shared result store

## Where

`<user cache dir>/treeproof/v1/` (for example `~/.cache/treeproof/v1` on Linux, `~/Library/Caches/treeproof/v1` on macOS, `%LOCALAPPDATA%\treeproof\v1` on Windows). `TREEPROOF_CACHE_DIR` overrides the parent directory.

| Path | Holds |
|---|---|
| `runs/<k2>/<key>.json` | Signed receipt of a passing run, by run key. This is the cache. |
| `runs/<k2>/<key>.log` | The last 256 KiB of that run's output, replayed on a cache hit. |
| `trees/<tree>/<key>.json` | The latest receipt of every run of a tree, pass or fail. The gate reads these. |
| `locks/<key>.lock` | Single-flight locks. |
| `sessions/` | Hook state (session start trees, block counts, FileGuard hashes). |

Every write goes to a temporary file first and is renamed into place.

## The key

A cached pass is reused only when all of these match (details in [SPEC.md](../SPEC.md#5-run-key)):

- the working-tree digest (tracked, modified and untracked non-ignored files),
- the exact argv (or shell command line) and the working directory inside the repository,
- versions of the toolchains the command uses (`rustc`/`cargo`, `python`, `node`, `bun`, `deno`, `go`),
- the hashes of lockfiles at the root and in the working directory,
- allowlisted environment variables plus any listed under `[run] env`, as HMAC-SHA256 under a per-machine secret (`env.salt` in the store), so receipts do not publish guessable hashes of their values,
- the working-tree digests of nested repositories (submodules), when there are any,
- OS and architecture.

## Rules

- **Failures are never cached.** Only `PASSED`/`WARNED` receipts go under `runs/`. A failing command runs every time.
- **Single flight.** Before executing, treeproof takes an exclusive lock on `locks/<key>.lock`. A second identical run (another worktree, another agent) waits on that lock, then finds the pass in the cache. If the first run failed, the waiter runs the command itself.
- **Integrity.** A cache entry is used only if its signature verifies with this machine's key and its subject and key match the request; anything else is evicted and the command runs.
- **No daemon.** Everything is files and OS file locks (`flock` on Unix, `LockFileEx` on Windows, through the Rust standard library).

## Tested behaviour

`tests/e2e.rs::worktrees_share_results_and_untracked_files_miss` creates a repository with three extra `git worktree`s, starts the same test command in all four at once, and checks that the command executed exactly once (one `fresh`, three `cache_hit`). It then adds an untracked file in one worktree (a miss there only) and an ignored file in another (still a hit).

## When to bypass

- `--no-cache` always executes.
- Tests that read the network, a database or an environment variable outside the key can pass once and fail later with the same key. Add those variables to `[run] env`, or run them with `--no-cache`.
