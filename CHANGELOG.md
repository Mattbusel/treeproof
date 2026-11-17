# Changelog

## 0.1.1

Security fixes for the gate, from an independent review of 0.1.0. Upgrade if you rely on the gate or the Action.

- **Observed receipts no longer open the gate by default** (`[gate] accept_observed` now defaults to `false`). An agent could print a fake runner summary through the host's Bash tool and have it recorded as a pass. Observed recording also only reads a plain runner invocation now (`cargo test`, `pytest`, `npm test`, ... with nothing chained, piped, redirected or substituted).
- **The gate no longer reads its policy from the working copy.** Hooks use the `treeproof.toml` committed at `HEAD` when the session started; `treeproof gate`, `status`, `export` and MCP `receipt_query` use the one committed at `HEAD` (`--policy-ref` to choose). The `PreToolUse` hook refuses agent edits to `treeproof.toml`. `run.allow_unrecognized` is read from the committed policy, and the gate refuses unrecognised-output passes unless that policy allows them.
- **A passing subset no longer satisfies the gate.** Runs that select part of the suite (cargo "filtered out", pytest `-k`/`-m`/node ids/single files/"deselected", `go test -run`, jest/vitest `-t` and file filters, nextest filters) need `[gate] allow_subset = true` or an exact pinned command. A run that records failing tests blocks the gate for that tree until the same command passes again. A failing run withdraws any cached pass for the same run key.
- **`treeproof verify` and the GitHub Action apply a policy and stop trusting keys from the change itself.** New `verify` options `--keys-ref <rev>`, `--no-repo-keys`, `--policy-ref <rev>` and `--no-policy`; `--tree current` applies the policy at `HEAD` by default. The Action reads trusted keys and `treeproof.toml` from the pull request's base commit (new `trust-ref` input).
- **Session baseline without `SessionStart`.** The first `PreToolUse`/`PostToolUse` of a session records the baseline tree and policy snapshot; if `Stop` comes first, the baseline is where the branch left its upstream (`HEAD` when there is none). Committing changes no longer slips past a session that had no `SessionStart`.
- **Nested repositories.** Receipts record the working-tree digest of each initialised submodule or embedded repository (SPEC 3.5, 4.1), the run key includes them, and the gate compares them, so edits inside a submodule invalidate a receipt.
- **Environment values in receipts are keyed.** Allowlisted variables are recorded as HMAC-SHA256 under a per-machine secret (`env.salt` in the store) instead of a plain SHA-256, so a committed receipt does not allow offline guessing. Run keys change, so the first run after upgrading is a cache miss.
- Receipts with more than one subject are rejected, as SPEC section 2 requires.
- The local gate trusts only this machine's key and `TREEPROOF_TRUSTED_KEYS`, not keys in the working copy's `.treeproof/trusted_keys/`.
- Library: `treeproof::Context` (store directory and key file) with `run::run_in`, `run::record_observed_in` and `gate::evaluate_in`, so callers no longer need `std::env::set_var`; `RunOptions::new`/`shell_line` and `with_*` builders; `TestResult` and `RunStatus` enums with `RunOutcome::test_result()`, `run_status()` and `passed()`; `gate::policy_rejection`; `Config::load_committed` and `load_at_rev`; `keys::trusted_keys_at_rev`; `git::nested_trees`. Existing signatures are unchanged. `GateConfig` gains the `allow_subset` field.
- The Action's installer falls back to `cargo install` when the release has no binary for the runner (v0.1.0 has no Intel macOS archive).
- Docs: threat model, configuration, SPEC (2, 2.1, 3.2, 3.4, 3.5, 4.1, 5, 7) and Action docs describe the above.

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
