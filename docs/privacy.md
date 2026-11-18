# Privacy policy: treeproof and the treeproof plugin

Last updated: 2026-11-18. Applies to the `treeproof` binary (crates.io, PyPI, npm, GitHub releases) and the treeproof plugin for Claude Code and Codex (`plugin/` in this repository).

## Who is responsible

Matthew Busel, Tensorust LLC, 404 Read Ave, Tuckahoe, NY 10707, USA. Questions or requests: mattbusel@tensorust.com.

## What treeproof collects

Nothing about you is sent to us or to anyone else. treeproof has no account, no usage telemetry, no crash reporting and no update check. We never receive your code, your test output, your prompts or your conversations.

## What it stores, and where

Everything stays on your machine, under your user account:

- **Receipts**: a content-addressed store under your user cache directory (`<cache dir>/treeproof`, or `TREEPROOF_CACHE_DIR`). A receipt holds the git tree hash and HEAD commit, the test command and working directory, the exit code, test counts, timings, toolchain versions, OS and architecture, SHA-256 hashes (not values) of allowlisted environment variables, and the signing key id. A passing run also keeps the tail of its output so a cache hit can replay it.
- **Signing key**: an Ed25519 key in your user config directory (`<config dir>/treeproof/signing.key`, or `TREEPROOF_KEY_FILE`). It never leaves your machine.
- **Session state for the hooks**: per agent session, the SHA-256 content hash of each file the agent read and the gate state for the current tree, kept under `sessions/` in the same store so the `Stop`, `TaskCompleted` and FileGuard checks can work.

A receipt is only shared if you share it, for example by committing it or attaching it to a pull request.

You can delete any of it at any time by removing those directories. Uninstalling the plugin (`/plugin uninstall treeproof`) stops the hooks; it does not delete receipts you may want to keep.

## Network

The only network traffic treeproof can make is an OpenTelemetry (OTLP/HTTP) export of verification events, and only when you turn it on by setting `TREEPROOF_OTLP_ENDPOINT`, or `TREEPROOF_OTEL=1` together with `OTEL_EXPORTER_OTLP_ENDPOINT`. The events go only to the endpoint you configure (normally a local collector), and contain the verdict, exit code, file-diff and re-run results for the current tree. See [otel.md](otel.md).

The plugin's MCP server (`treeproof mcp`) runs locally over stdio. It does not open a network port.

## Third parties

None. Installing the binary uses the package registry or GitHub release you choose (crates.io, PyPI, npm or GitHub), under that service's own privacy policy.

## Retention

Receipts and keys stay until you delete them. Nothing is kept anywhere else.

## Children

treeproof is a developer tool and is not directed at children under 18.

## Changes

Changes to this policy are made in this file, and the git history shows every version.
