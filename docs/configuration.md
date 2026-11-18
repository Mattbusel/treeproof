# Configuration

## `treeproof.toml`

Optional, at the repository root. Unknown keys are an error.

```toml
[gate]
mode = "block"          # block | warn | off
commands = []           # if set, only receipts from these commands count; a trailing * allows extra arguments
min_tests = 1           # minimum passed tests
accept_observed = false # count receipts recorded from the agent's own Bash test runs
allow_subset = false    # count runs that select a subset of the tests (name filters, -k, ::node ids, -run, -t)
max_blocks = 3          # consecutive blocks on one tree before letting the agent stop with a warning

[run]
env = []                    # extra environment variable names in the cache key
allow_unrecognized = false  # sign a pass from the exit status alone when no runner summary is found

[fileguard]
enabled = true
```

### Which copy of the file counts

The gate never reads the policy from the working copy, because the agent can write it:

- **Hooks** (Claude Code, Codex) take a snapshot of `treeproof.toml` as committed at `HEAD` when the session starts, and use it for the whole session. Writing or committing a different policy during the session changes nothing until the next session.
- **`treeproof gate`, `status`, `export`** and the MCP `receipt_query` tool read the file committed at `HEAD`. Pass `--policy-ref <rev>` to read it from another revision (for example `origin/main`). `treeproof status` says when the working copy differs.
- **`treeproof verify --tree current`** applies the policy at `HEAD`, or at `--policy-ref`. `--no-policy` turns that off.
- **The GitHub Action** reads it from the pull request's base commit.

So: edit `treeproof.toml`, commit it, and it applies from the next session on. The `PreToolUse` hook refuses agent edits to `treeproof.toml`; change it yourself.

`run.allow_unrecognized` is also read from the committed policy. `treeproof run --allow-unrecognized` still signs such a run, but the gate accepts it only when the committed policy allows unrecognised output. `run.env` names from the working copy are added to the cache key as well, which can only make the cache stricter.

### Subsets

A receipt from a run that selected part of the suite does not satisfy the gate unless `allow_subset = true` or the command matches an exact (no trailing `*`) entry in `commands`. treeproof recognises:

- from the output: `cargo test` "N filtered out" (also `--ignored`), pytest "N deselected", jest "Ran all test suites matching ...";
- from the arguments: pytest `-k`, `-m`, `--lf`, `--deselect`, `--sw`, node ids (`file.py::test`) and single `.py` files; `go test -run`/`-skip`; jest and vitest `-t`, `--testNamePattern`, `--testPathPattern`, `--onlyChanged`, `--changed`, `--findRelatedTests`, `--shard`, `--project`, `vitest related` and file filters; nextest `-E`/`--filterset`, `--partition` and name filters.

Package or target selection (`cargo test -p x`, `go test ./pkg/...`) is not treated as a subset. Pin the invocation you want with `commands`.

### Failures

When a run of a command records failing tests on a tree, the gate stays closed for that tree until a later run of the same command passes, even if a different command passed in between.

## Environment variables

| Variable | Effect |
|---|---|
| `TREEPROOF_KEY_FILE` | Path of the Ed25519 signing key (default: `<config dir>/treeproof/signing.key`). |
| `TREEPROOF_CACHE_DIR` | Parent directory of the store (default: `<cache dir>/treeproof`). |
| `TREEPROOF_TRUSTED_KEYS` | Extra public key files to trust, separated like `PATH`. |
| `TREEPROOF_GATE` | Overrides `[gate] mode` (`block`, `warn`, `off`). Read by the hook process from the host's environment. With the plugin enabled the gate is on in every git repository; `TREEPROOF_GATE=off` turns it off everywhere. |
| `TREEPROOF_GATE_SUBAGENTS` | `1` gates `SubagentStop` too. |
| `TREEPROOF_FILEGUARD` | `0` disables FileGuard. |
| `TREEPROOF_RUN_URL` | Written to the predicate's `url` (for example a CI run link). |
| `TREEPROOF_OTLP_ENDPOINT` | OTLP/HTTP endpoint the hooks export evaluation events to. Unset by default: nothing is sent. |
| `TREEPROOF_OTEL` | `1` lets the hooks also use the generic `OTEL_EXPORTER_OTLP_ENDPOINT` / `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`; `0` turns all export off. See [otel.md](otel.md). |

Commands run under `treeproof run` see `TREEPROOF_ACTIVE=1`.

Library callers can pass a `treeproof::Context` (store directory and key file) to `run::run_in` and `gate::evaluate_in` instead of setting `TREEPROOF_CACHE_DIR` and `TREEPROOF_KEY_FILE`.

## Keys

```sh
treeproof key init     # create the key if needed, print its id
treeproof key show     # id, file and public key
treeproof key export   # public key PEM
```

The key is created on first use with owner-only permissions on Unix.
