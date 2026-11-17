# Configuration

## `treeproof.toml`

Optional, at the repository root. It is part of the tree, so changing the policy needs a fresh receipt like any other change. Unknown keys are an error.

```toml
[gate]
mode = "block"          # block | warn | off
commands = []           # if set, only receipts from these commands count; a trailing * allows extra arguments
min_tests = 1           # minimum passed tests
accept_observed = true  # count receipts recorded from the agent's own Bash test runs
max_blocks = 3          # consecutive blocks on one tree before letting the agent stop with a warning

[run]
env = []                    # extra environment variable names in the cache key
allow_unrecognized = false  # sign a pass from the exit status alone when no runner summary is found

[fileguard]
enabled = true
```

## Environment variables

| Variable | Effect |
|---|---|
| `TREEPROOF_KEY_FILE` | Path of the Ed25519 signing key (default: `<config dir>/treeproof/signing.key`). |
| `TREEPROOF_CACHE_DIR` | Parent directory of the store (default: `<cache dir>/treeproof`). |
| `TREEPROOF_TRUSTED_KEYS` | Extra public key files to trust, separated like `PATH`. |
| `TREEPROOF_GATE` | Overrides `[gate] mode` (`block`, `warn`, `off`). |
| `TREEPROOF_GATE_SUBAGENTS` | `1` gates `SubagentStop` too. |
| `TREEPROOF_FILEGUARD` | `0` disables FileGuard. |
| `TREEPROOF_RUN_URL` | Written to the predicate's `url` (for example a CI run link). |
| `TREEPROOF_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_*`, `TREEPROOF_OTEL` | See [otel.md](otel.md). |

Commands run under `treeproof run` see `TREEPROOF_ACTIVE=1`.

## Keys

```sh
treeproof key init     # create the key if needed, print its id
treeproof key show     # id, file and public key
treeproof key export   # public key PEM
```

The key is created on first use with owner-only permissions on Unix.
