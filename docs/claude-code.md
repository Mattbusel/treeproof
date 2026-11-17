# Claude Code

The treeproof plugin turns "done" into "done with a passing receipt for the current tree".

## Install

1. Install the binary so it is on `PATH`: `cargo install treeproof` or a release archive.
2. In Claude Code:

   ```text
   /plugin marketplace add Mattbusel/treeproof
   /plugin install treeproof@treeproof
   ```

The plugin lives in [`plugin/`](../plugin). It contains hooks, the MCP server entry and a short skill that tells the agent to run tests through treeproof.

To wire the hooks without the plugin, print the configuration and merge it into `.claude/settings.json` (project) or `~/.claude/settings.json` (user):

```sh
treeproof hooks-config claude
```

## What each hook does

| Event | Matcher | Behaviour |
|---|---|---|
| `SessionStart` | all | Records the tree the session started from. |
| `Stop` | all | If the tree differs from the session's starting tree, requires a valid passing receipt for the current tree. Otherwise returns `{"decision":"block","reason":...}` and the agent keeps working. |
| `TaskCompleted` | all | Same check before a task can be marked completed. |
| `PostToolUse` | `Bash` | When the agent ran a test command itself (`cargo test`, `pytest`, `npm test`, ...) and the output contains a recognised summary, signs an *observed* receipt for the current tree. The agent sees a one-line note with the result. |
| `PostToolUse` | `Read`, `Edit`, `Write`, ... | FileGuard: remembers the content hash of each file the agent read or wrote. |
| `PreToolUse` | `Edit`, `Write`, ... | FileGuard: denies an edit when the file changed on disk since the agent last saw it. Also denies writes to the signing key or receipt store. |
| `PreToolUse` | `Bash` | Denies `treeproof hook ...` and commands that touch the signing key or receipt store. |

`SubagentStop` is not gated by default (subagents often hand back partial work). Set `TREEPROOF_GATE_SUBAGENTS=1` to gate it too.

## Behaviour details

- **Nothing changed, nothing asked.** A session that never changes the tree can stop freely. Without a `SessionStart` record (plugin enabled mid-session), the baseline is `HEAD`'s tree.
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

A bare `python run_tests.py` is not recorded as an observed run because the command does not name a known runner (`cargo test`, `pytest`, `npm test`, `go test`, ...). Observed recording only looks at commands that do.
