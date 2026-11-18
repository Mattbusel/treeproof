# MCP server

`treeproof mcp` serves three tools over stdio (newline-delimited JSON-RPC 2.0). It negotiates MCP protocol versions 2025-11-25, 2025-06-18, 2025-03-26 and 2024-11-05.

```json
{ "mcpServers": { "treeproof": { "command": "treeproof", "args": ["mcp"] } } }
```

The Claude Code plugin registers it automatically.

## Tools

### `receipt_run`

Run the repository's test command and sign a receipt for the current tree.

MCP tools run outside the host's shell permissions, deny rules and sandbox, so `receipt_run` is not a general command runner. It accepts:

1. a command matching one of the entries in `[gate] commands` of the `treeproof.toml` **committed at `HEAD`** (the working copy's file is not read), or
2. when no commands are committed, a recognised test runner with standard arguments: `cargo test` / `cargo nextest run` (with `--workspace`, `--all-features`, `--no-default-features`, `--all-targets`, `--lib`, `--bins`, `--tests`, `--doc`, `--locked`, `--offline`, `--frozen`, `--release`, `--no-fail-fast`, `-q`, `-v`, `-p`/`--package <name>`, `--features <list>`, `--exclude <name>`), `pytest` / `python -m pytest` (with `-q`, `-v`, `-vv`, `-x`, `-ra`, `-rA`, `--no-header`, `--tb=short|line|no`, `-n <workers>`), `go test` (with `./...`, `-v`, `-race`, `-short`, `-cover`, `-count=1`), `npm test`, `pnpm test`, `yarn test`, `bun test` (or `run test`), `jest` (`--ci`, `--silent`), `vitest run`, and `npx jest` / `npx vitest run`.

Anything else is refused with an error that says why and what is allowed. Flags that load code or change what runs (`cargo --config`, `pytest -p`, `go test -exec`, ...) are not in the list; pin such a command in `[gate] commands` if you need it. The command is split on whitespace and run as a program with arguments, never through a shell: quotes, `|`, `;`, `&`, `<`, `>`, `$`, backticks and `%` are refused. Commands that need a shell belong in the agent's normal Bash tool (`treeproof run -- <cmd>`), where the host's permissions apply.

| Argument | Type | |
|---|---|---|
| `command` | string | Command line, for example `"cargo test --workspace"`. Split on whitespace; no shell. |
| `argv` | string[] | Program and arguments (use instead of `command`). The same check applies. |
| `cwd` | string | Directory inside the repository. Defaults to `CLAUDE_PROJECT_DIR`, then the server's working directory. |
| `no_cache` | bool | Execute even when an identical pass is cached. |
| `timeout_secs` | int | Kill the command after this many seconds (default 1800). |

Returns `status` (`fresh` or `cache_hit`), `result`, `reason`, `tree`, `counts`, `runners`, `notes`, `exit_code`, `receipt_path`, the full `receipt` envelope and the last 8000 characters of output.

### `receipt_query`

Returns the current tree, every receipt recorded for it (with why each one does or does not satisfy the policy), the gate verdict and a suggested command.

### `receipt_verify`

| Argument | Type | |
|---|---|---|
| `receipt` | string | A receipt file path, or the envelope JSON itself. |
| `current_tree` | bool | Require the receipt to be for the current working tree. |
| `rerun` | bool | Re-run the command and compare. |
| `cwd` | string | Repository directory. |

Trusted keys are this machine's signing key, `TREEPROOF_TRUSTED_KEYS`, and, only when `rerun` is false, `.treeproof/trusted_keys/*.pem` in the working copy. With `rerun: true` the working copy's keys are never trusted, the same rule as the local gate: a re-run executes the receipt's command, and a cloned repository could otherwise sign its own receipt. The re-run command also has to pass the `receipt_run` check above. To re-run someone else's receipts, add their public key to `TREEPROOF_TRUSTED_KEYS` (a path outside the repository).

The server's instructions tell the client to call `receipt_query` before claiming work is done. They do not tell it to route test runs through `receipt_run` instead of its own tools.

Command output is captured, never written to the server's stdout, so it cannot corrupt the JSON-RPC stream.
