# MCP server

`treeproof mcp` serves three tools over stdio (newline-delimited JSON-RPC 2.0). It negotiates MCP protocol versions 2025-11-25, 2025-06-18, 2025-03-26 and 2024-11-05.

```json
{ "mcpServers": { "treeproof": { "command": "treeproof", "args": ["mcp"] } } }
```

The Claude Code plugin registers it automatically.

## Tools

### `receipt_run`

Run a test command and sign a receipt for the current tree.

| Argument | Type | |
|---|---|---|
| `command` | string | Shell command line, for example `"cargo test --workspace"`. |
| `argv` | string[] | Program and arguments, run without a shell (use instead of `command`). |
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

Trusted keys are this machine's signing key, `.treeproof/trusted_keys/*.pem` in the repository, and `TREEPROOF_TRUSTED_KEYS`.

Command output is captured, never written to the server's stdout, so it cannot corrupt the JSON-RPC stream.
