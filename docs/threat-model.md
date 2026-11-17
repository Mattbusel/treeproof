# Threat model

treeproof is aimed at one failure: an agent (or a person) reporting that tests pass when, for the code as it now stands, they do not. It is not a sandbox and does not defend against an adversary with your user account.

## What it stops

| Situation | Outcome |
|---|---|
| The agent says "all tests pass" without running anything | No receipt for the tree; the Stop/TaskCompleted gate blocks. |
| The agent ran tests, then edited code | The tree changed; the old receipt does not match; the gate blocks. |
| The agent ran tests, then edited code inside a submodule or nested repository | The nested repository's working-tree digest is recorded with the run; the gate blocks. |
| The agent ran a filtered subset that matched nothing, or `--collect-only` | `FAILED` receipt; the gate blocks. |
| The agent ran only a subset (`cargo test foo`, `pytest -k`, `pytest file.py::test`, `go test -run`, `jest -t`, ...) | The receipt is valid but does not satisfy the gate unless the policy pins that command or sets `allow_subset`. |
| The full suite failed and the agent then ran something narrower that passes | The unresolved failure blocks the gate until the same command passes on the same tree. |
| The runner's output was not recognised | `FAILED` unless explicitly allowed, and even then the gate refuses it unless the committed policy sets `run.allow_unrecognized`. |
| Some tests failed | `FAILED`; never cached as a pass, and a failure withdraws an earlier cached pass for the same run. |
| The agent prints a fake runner summary in its shell (`echo`, `printf`, pipes) | Not recorded: observed recording only reads a plain runner invocation with nothing chained, piped or redirected. Observed receipts do not satisfy the gate unless `accept_observed` is set. |
| The agent edits `treeproof.toml` to turn the gate off or weaken it | No effect on the running session: hooks use the policy committed at `HEAD` when the session started. The `PreToolUse` hook also refuses edits to `treeproof.toml`. |
| The agent commits its changes before stopping | The baseline is the tree recorded at `SessionStart` (or at the session's first tool hook), not `HEAD`; the gate blocks. |
| Someone edits a receipt file | The signature no longer verifies. |
| A receipt for an older commit is committed with a newer change | CI computes the checked-out tree; no receipt matches; the Action fails. |
| A pull request brings its own signing key and receipt | The Action trusts only keys committed on the base branch; the receipt does not verify. |
| A pull request weakens `treeproof.toml` | The Action applies the policy from the base branch. |
| A receipt was produced on a machine where the tests happened to pass | The Action re-runs the command in CI and fails on disagreement. |
| Two agents edit the same file | FileGuard denies the second edit until the agent re-reads the file. |

## What it does not stop

- **A determined agent with shell access.** The signing key is a file in your user config directory. The hooks deny the obvious commands (`treeproof hook ...`, reading `signing.key`, writing into the store, writing `treeproof.toml`), but an agent running as you can find other ways. Keep `TREEPROOF_KEY_FILE` somewhere the agent's sandbox cannot read if your host supports that.
- **A fake test runner.** `treeproof run -- python fake_tests.py` that prints "3 passed" produces a valid receipt. Pin the real command with `[gate] commands = ["cargo test --workspace"]`; the pin is read from the committed policy, so the agent cannot change it. The command, like any file the agent writes, is visible in the diff and in the receipt.
- **Editing the tests.** A receipt proves the tests in the tree passed, not that they are good tests. Review test changes.
- **Out-of-tree state.** Network, databases, clocks, and environment variables outside the key are not captured.
- **Package or target selection.** `cargo test -p one_crate` or `go test ./one/pkg/...` is not detected as a subset. If your suite needs a particular invocation, pin it with `[gate] commands`.
- **A session with no hook record that committed before any hook ran.** The baseline then falls back to where the branch left its upstream, or to `HEAD` when there is no upstream. With the plugin enabled from the start this does not happen, because every `Bash` and edit call goes through `PreToolUse` first.

## Observed receipts

`PostToolUse` records receipts from the host's own Bash tool output, but only for a plain invocation of a known runner (`cargo test`, `pytest`, `python -m pytest`, `npm test`, `go test`, `jest`, `vitest`, ...), optionally after `cd <dir> &&`, environment assignments and a launcher such as `uv run` or `npx`, with nothing chained, piped, redirected or substituted. Claude Code does not report the exit status of a Bash call, and the agent controls what its shell prints, so observed receipts do not satisfy the gate by default. They are still useful: the agent sees the result, and an observed failure counts against a later narrower run. Set `[gate] accept_observed = true` in the committed policy to count observed passes.

## Where the policy comes from

| Check | Policy | Trusted keys |
|---|---|---|
| Claude Code / Codex hooks | `treeproof.toml` committed at `HEAD` when the session started | This machine's key and `TREEPROOF_TRUSTED_KEYS` |
| `treeproof gate`, `status`, `export`, MCP `receipt_query` | `treeproof.toml` committed at `HEAD` (or `--policy-ref`) | This machine's key and `TREEPROOF_TRUSTED_KEYS` |
| `treeproof verify --tree current` | `HEAD` by default, or `--policy-ref` | `--key`, `--keys-ref <rev>`, or the working copy's `.treeproof/trusted_keys/` |
| GitHub Action | The pull request's base commit (`trust-ref`) | Keys committed at the base commit, plus the `keys` input |

The working copy's `treeproof.toml` and `.treeproof/trusted_keys/` are never trusted by a gate.

## The CI check is the strong one

The local gate makes the honest path the easy path. The GitHub Action is the check that does not depend on the agent's machine: it recomputes the tree from the checkout, verifies the signature against keys committed on the base branch, applies the base branch's policy, and re-runs the command.

## Privacy of receipts

Receipts are meant to be committed. Environment variables in the allowlist are recorded as HMAC-SHA256 under a per-machine secret (`env.salt` in the store), so a published receipt does not let anyone test guesses of their values. Variable names are recorded.
