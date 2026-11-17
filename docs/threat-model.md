# Threat model

treeproof is aimed at one failure: an agent (or a person) reporting that tests pass when, for the code as it now stands, they do not. It is not a sandbox and does not defend against an adversary with your user account.

## What it stops

| Situation | Outcome |
|---|---|
| The agent says "all tests pass" without running anything | No receipt for the tree; the Stop/TaskCompleted gate blocks. |
| The agent ran tests, then edited code | The tree changed; the old receipt does not match; the gate blocks. |
| The agent ran a filtered subset that matched nothing, or `--collect-only` | `FAILED` receipt; the gate blocks. |
| The runner's output was not recognised | `FAILED` unless explicitly allowed. |
| Some tests failed | `FAILED`; never cached as a pass. |
| Someone edits a receipt file | The signature no longer verifies. |
| A receipt for an older commit is committed with a newer change | CI computes the checked-out tree; no receipt matches; the Action fails. |
| A receipt was produced on a machine where the tests happened to pass | The Action re-runs the command in CI and fails on disagreement. |
| Two agents edit the same file | FileGuard denies the second edit until the agent re-reads the file. |

## What it does not stop

- **A determined agent with shell access.** The signing key is a file in your user config directory. The hooks deny the obvious commands (`treeproof hook ...`, reading `signing.key`, writing into the store), but an agent running as you can find other ways. Keep `TREEPROOF_KEY_FILE` somewhere the agent's sandbox cannot read if your host supports that.
- **A fake test runner.** `treeproof run -- python fake_tests.py` that prints "3 passed" produces a valid receipt. Pin the real command with `[gate] commands = ["cargo test --workspace"]`. The command, like any file the agent writes, is visible in the diff and in the receipt.
- **Editing the tests.** A receipt proves the tests in the tree passed, not that they are good tests. Review test changes.
- **Out-of-tree state.** Network, databases, clocks, and environment variables outside the key are not captured.

## Observed receipts

`PostToolUse` records receipts from the host's own Bash tool output. The agent cannot fabricate that output through the host, but it could pipe fake JSON into `treeproof hook` itself, which the `PreToolUse` guard denies. If that residual risk matters, set `[gate] accept_observed = false` so only `treeproof run` receipts count.

## The CI check is the strong one

The local gate makes the honest path the easy path. The GitHub Action is the check that does not depend on the agent's machine: it recomputes the tree from the checkout, verifies the signature against keys committed in the repository, and re-runs the command.
