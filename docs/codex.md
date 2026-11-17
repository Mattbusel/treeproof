# Codex

Codex CLI hooks use the same JSON shapes as Claude Code for the events treeproof needs (`SessionStart`, `Stop`, `PreToolUse`, `PostToolUse`), so the same `treeproof hook` handler serves both. treeproof detects Codex by the `turn_id` field in the hook input.

## Install

1. Install the binary so it is on `PATH`.
2. Write the hooks file, for one project or for your user:

   ```sh
   mkdir -p .codex && treeproof hooks-config codex > .codex/hooks.json     # project (the project must be trusted)
   treeproof hooks-config codex > ~/.codex/hooks.json                        # user
   ```

   If you already have a `hooks.json`, merge the `hooks` object by hand.
3. Review and trust the new hooks with `/hooks` in the Codex CLI. Codex skips new or changed hooks until you do.

The repository also ships a Codex plugin manifest (`plugin/.codex-plugin/plugin.json`) and a marketplace entry (`.agents/plugins/marketplace.json`). Codex documents plugin-bundled lifecycle hooks as supported for manually installed desktop plugins; for the CLI, the `hooks.json` route above is the dependable one.

## Differences from Claude Code

- Codex `Stop` expects JSON on stdout when the hook exits 0, so treeproof always prints `{}` when it allows a stop.
- Codex has no `TaskCompleted` event; the `Stop` gate covers it.
- File edits arrive as `apply_patch`. FileGuard reads the `*** Update File:` / `*** Delete File:` / `*** Add File:` lines of the patch to find the paths.
- `PostToolUse` for Bash also runs after commands that exit non-zero. When Codex reports an exit code, treeproof records it in the observed receipt. Observed passes do not satisfy the gate unless the committed policy sets `accept_observed`.
- If Codex does not send `SessionStart`, the first `PreToolUse` or `PostToolUse` call records the session's baseline tree and policy snapshot.
