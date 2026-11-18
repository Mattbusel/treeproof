# treeproof plugin

Claude Code (and Codex) plugin for [treeproof](https://github.com/Mattbusel/treeproof). After the agent changes code, `Stop` and `TaskCompleted` are blocked until a passing, signed test receipt exists for the exact git tree on disk.

## Install

In Claude Code:

```
/plugin install treeproof --marketplace Mattbusel/treeproof
```

### Install the pinned binary first

The hooks and the MCP entry run the `treeproof` binary from `PATH`. Install the version this plugin was released with:

```sh
cargo install treeproof --version 0.1.2 --locked
```

Or download the v0.1.2 archive from the [release page](https://github.com/Mattbusel/treeproof/releases/tag/v0.1.2) and check it against that release's `SHA256SUMS` (`sha256sum -c SHA256SUMS --ignore-missing`). See [docs/install.md](../docs/install.md).

The `SessionStart` hook prints a warning when the `treeproof` on `PATH` is a different version from this plugin.

## What it turns on

- **The gate, in every git repository.** Not only repositories with a `treeproof.toml`. A session that changes nothing, or runs outside a git repository, is never gated. After 3 blocks on the same tree the agent may stop with a warning, and the hooks fail open if treeproof itself errors.
- **FileGuard**: edits to a file that changed on disk since the agent read it are denied.
- **MCP tools** `receipt_run`, `receipt_query`, `receipt_verify`. `receipt_run` runs only the commands committed in `treeproof.toml` `[gate] commands`, or, when none are committed, a recognised test runner with standard arguments (`cargo test`, `pytest`, `go test ./...`, `npm test`, ...), never through a shell. Everything else is refused. `receipt_verify` with `rerun` never trusts keys from the checkout.

## Turning it off

| Scope | How |
|---|---|
| One repository | Commit `treeproof.toml` with `[gate]` `mode = "off"` (or `"warn"`). |
| Everywhere | `TREEPROOF_GATE=off` (or `warn`), for example `"env": {"TREEPROOF_GATE": "off"}` in `~/.claude/settings.json`. |
| The plugin | `/plugin disable treeproof` |

## Network

Nothing is sent anywhere by default. The `Stop` hook exports OpenTelemetry evaluation events only if you set `TREEPROOF_OTLP_ENDPOINT`, or set `TREEPROOF_OTEL=1` together with the generic `OTEL_EXPORTER_OTLP_ENDPOINT`. The generic variable alone does not turn export on. `TREEPROOF_OTEL=0` turns it off entirely. See [docs/otel.md](../docs/otel.md).

## Privacy

treeproof sends nothing to us or to anyone else: no account, no telemetry, no update check. Receipts, the signing key and per-session hook state stay in your user cache and config directories on your machine. The only network traffic it can make is the opt-in OTLP export above, to the endpoint you set. Full policy: [docs/privacy.md](https://github.com/Mattbusel/treeproof/blob/main/docs/privacy.md). Questions: mattbusel@tensorust.com.
