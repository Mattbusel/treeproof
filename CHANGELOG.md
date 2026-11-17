# Changelog

## 0.1.0

First release.

- `treeproof run`: runs a test command, parses cargo test, cargo nextest, pytest (including xdist), jest, vitest and go test output, and signs an in-toto Statement v1 with the test-result v0.1 predicate in a DSSE envelope (Ed25519). The subject is the git tree id of the working tree including uncommitted and untracked files.
- Strict results: 0 tests, everything filtered out, `--collect-only` and similar, unrecognised output, or a non-zero exit is FAILED.
- Cross-worktree store under the user cache directory: passing runs are cached by tree, argv, toolchain versions, lockfiles and allowlisted environment; identical concurrent runs execute once; failures are never cached.
- `treeproof verify` (signature, schema, `--tree current`, `--rerun`), `status`, `gate`, `export`, `tree`, `key`.
- Claude Code plugin: Stop and TaskCompleted gate, PostToolUse recording of the agent's own test runs, FileGuard stale-edit check, guard against agents calling the hooks or touching the key.
- Codex hooks (`treeproof hooks-config codex`).
- MCP server with `receipt_run`, `receipt_query`, `receipt_verify`.
- GitHub Action `Mattbusel/treeproof/verify`: re-runs the committed receipt's command on the checked-out tree and fails on mismatch.
- OpenTelemetry `gen_ai.evaluation.result` events (`verification.exit_code`, `verification.fs_diff`, `verification.test_rerun`) over OTLP/HTTP JSON.
- `SPEC.md` receipt specification; CI checks receipts with securesystemslib and the in-toto-attestation validators, and OTLP events with a real OpenTelemetry Collector.
