---
name: treeproof
description: Run the test suite through treeproof so the result is a signed receipt for the current git tree. Use before saying tests pass or that work is done.
---

This repository gates "done" on a treeproof receipt: a signed record that this exact working tree passed this exact test command.

1. Run tests through treeproof, never bare:
   `treeproof run -- cargo test` (or `pytest`, `npm test`, `go test ./...`, the project's usual command).
   The `receipt_run` MCP tool does the same for the repository's committed test command or a standard runner invocation; it refuses anything else, so use your normal shell for other commands.
2. If the result is FAILED, fix the code and run it again. A run with 0 tests, an unrecognised runner, `--collect-only`, or any failing test is FAILED.
3. Run the whole suite. A run of part of it (a test-name filter, `-k`, `file.py::test`, `-run`, `-t`) does not count, and after a failing run the same command has to pass again.
4. Any edit after a passing run changes the tree, so run the tests again after your last edit.
5. Check `treeproof status` (or the `receipt_query` tool) before you say the work is complete.

Never claim tests pass without a passing receipt for the current tree. Do not run `treeproof hook`, do not read or write the treeproof signing key or receipt store, and do not edit `treeproof.toml` (the repository's gate policy; ask the user if it needs to change). The host runs the hooks.
