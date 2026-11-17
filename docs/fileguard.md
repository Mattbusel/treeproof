# FileGuard

Parallel agents, formatters and background processes can rewrite a file between the moment an agent reads it and the moment the agent edits it. The edit then lands on content the agent never saw.

FileGuard records the SHA-256 of every file an agent reads or writes (per session, from `PostToolUse`) and checks it again in `PreToolUse` before an edit. If the file on disk no longer matches, the edit is denied with:

```text
treeproof FileGuard: src/lib.rs changed on disk since you last read it (another agent or process wrote it).
Read it again and redo the edit against the current content.
```

- Claude Code: `Read`, `Edit`, `Write`, `MultiEdit` and `NotebookEdit`.
- Codex: `apply_patch`; the paths come from the patch's `*** Update File:`, `*** Delete File:` and `*** Add File:` lines.
- Files the session never read are not checked.
- Edits made through Bash (`sed -i`, scripts) are not seen by the hooks, so the next tool edit of that file after such a change is denied once, until the agent reads it again.

Disable with `[fileguard] enabled = false` in `treeproof.toml` or `TREEPROOF_FILEGUARD=0`.
