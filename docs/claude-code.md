# Claude Code

The treeproof plugin turns "done" into "done with a passing receipt for the current tree".

## Install

1. Install the binary so it is on `PATH`, pinned to the plugin's version: `cargo install treeproof --version 0.1.3 --locked`, or the v0.1.3 release archive checked against its `SHA256SUMS` (see [install.md](install.md)). The hooks run whatever `treeproof` is on `PATH`; the `SessionStart` hook warns when that binary's version differs from the plugin's.
2. In Claude Code:

   ```text
   /plugin marketplace add Mattbusel/treeproof
   /plugin install treeproof@treeproof
   ```

## On by default, and how to turn it off

Once the plugin is enabled, the gate applies in **every git repository** you open, not only ones with a `treeproof.toml`: a session that changes the tree has to end with a passing `treeproof run` for that tree. Sessions that change nothing, and directories that are not git repositories, are never gated. After 3 blocks on the same tree it lets the agent stop with a warning.

To turn it off or soften it:

| Scope | How |
|---|---|
| One repository | Commit `treeproof.toml` with `[gate]` `mode = "off"` (or `"warn"`). It applies from the next session. |
| Everywhere | `TREEPROOF_GATE=off` (or `warn`) in the environment Claude Code runs in, for example `"env": {"TREEPROOF_GATE": "off"}` in `~/.claude/settings.json`. |
| The plugin | `/plugin disable treeproof@treeproof`. |

## Plugin contents

The plugin lives in [`plugin/`](../plugin). It contains hooks, the MCP server entry and a short skill that tells the agent to run tests through treeproof.

To wire the hooks without the plugin, print the configuration and merge it into `.claude/settings.json` (project) or `~/.claude/settings.json` (user):

```sh
treeproof hooks-config claude
```

## What each hook does

| Event | Matcher | Behaviour |
|---|---|---|
| `SessionStart` | all | Records the tree the session started from and a snapshot of the policy committed at `HEAD`. |
| `Stop` | all | If the tree differs from the session's starting tree, requires a valid passing receipt for the current tree that satisfies the session's policy. Otherwise returns `{"decision":"block","reason":...}` and the agent keeps working. |
| `TaskCompleted` | all | Same check before a task can be marked completed. |
| `PostToolUse` | `Bash` | When the agent ran a plain test command itself (`cargo test`, `pytest`, `npm test`, ... with nothing chained, piped or redirected) and the output contains a recognised summary, signs an *observed* receipt for the current tree. The agent sees a one-line note with the result. Observed passes do not satisfy the gate unless the policy sets `accept_observed`; observed failures do count against a later narrower run. |
| `PostToolUse` | `Read`, `Edit`, `Write`, ... | FileGuard: remembers the content hash of each file the agent read or wrote. |
| `PreToolUse` | `Edit`, `Write`, ... | FileGuard: denies an edit when the file changed on disk since the agent last saw it. Also denies writes to the signing key, the receipt store and `treeproof.toml`. |
| `PreToolUse` | `Bash` | Denies `treeproof hook ...`, commands that touch the signing key or receipt store, and commands that write `treeproof.toml`. |

The MCP server's `receipt_run` tool runs outside Claude Code's Bash permissions and sandbox, so it only runs the commands committed in `treeproof.toml` `[gate] commands`, or, when none are committed, a recognised test runner with standard arguments, and never through a shell. See [mcp.md](mcp.md). The skill tells the agent to run tests with `treeproof run -- <cmd>` through its normal Bash tool, where your permission rules apply.

The hooks send nothing over the network unless you opt in to OTLP export (`TREEPROOF_OTLP_ENDPOINT`, or `TREEPROOF_OTEL=1` with the generic `OTEL_EXPORTER_OTLP_ENDPOINT`). See [otel.md](otel.md).

`SubagentStop` is not gated by default (subagents often hand back partial work). Set `TREEPROOF_GATE_SUBAGENTS=1` to gate it too.

## Behaviour details

- **Nothing changed, nothing asked.** A session that never changes the tree can stop freely. Without a `SessionStart` record (plugin enabled mid-session), the first `PreToolUse` or `PostToolUse` call records the baseline; if `Stop` is the first hook the session sends, the baseline is where the branch left its upstream, or `HEAD`'s tree when there is no upstream. Commits made during the session do not move the baseline.
- **The policy is fixed for the session.** The gate uses `treeproof.toml` as committed when the session started. Editing or committing it mid-session has no effect until the next session, and the agent's tools may not write it.
- **Whole suite.** A run that selected part of the suite (`cargo test foo`, `pytest -k`, `pytest file.py::test`, ...) does not satisfy the gate unless the policy pins that command or sets `allow_subset`, and a failing run blocks until the same command passes again. See [configuration.md](configuration.md).
- **Loop protection.** After `max_blocks` (default 3) consecutive blocks on the same tree in one session, the hook lets the agent stop and shows a `systemMessage` saying the tree is unverified. Claude Code also caps consecutive stop-hook continuations on its own.
- **Fail open.** If treeproof itself errors (no git, unreadable store), the hook reports the error as a `systemMessage` and does not block.
- **Modes.** `TREEPROOF_GATE=warn` (or `[gate] mode = "warn"`) never blocks and only warns; `off` disables the gate.

## Example: a fabricated pass is blocked

The end-to-end test `fabricated_claim_is_blocked_until_a_real_pass` in `tests/e2e.rs` drives the hook exactly as Claude Code does:

1. `SessionStart` records the tree.
2. The agent edits a file and stops with "Done. All tests pass." The `Stop` hook answers `{"decision": "block", "reason": "treeproof: no treeproof receipt exists for the current tree ..."}`.
3. A failing `treeproof run` still blocks, and the reason names the failure.
4. A passing `treeproof run` lets the stop through (`{}`).

## Live run

A real `claude -p` session (Claude Code 2.1.326, `--plugin-dir plugin`) on a fixture repository, asked to change `lib.txt` and then reply "Done, all tests pass.":

1. The agent edited `lib.txt` and ran `python run_tests.py` directly (3 passed), then replied "Done, all tests pass."
2. The `Stop` hook answered: `treeproof: no treeproof receipt exists for the current tree 971675d9038a. Run the test suite through treeproof, for example treeproof run -- <your test command>, fix any failures, and finish only when it passes.`
3. The agent ran `treeproof run -- python run_tests.py` (fresh PASSED, 3 passed) and stopped; the hook let it through.

A bare `python run_tests.py` is not recorded as an observed run because the command does not name a known runner (`cargo test`, `pytest`, `npm test`, `go test`, ...). Observed recording only looks at plain invocations of those, and since treeproof 0.1.1 observed passes do not open the gate by default.

(The live run above was recorded with treeproof 0.1.0.)
