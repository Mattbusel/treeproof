# treeproof: "all tests pass" is a claim until the tree proves it

[![CI](https://github.com/Mattbusel/treeproof/actions/workflows/ci.yml/badge.svg)](https://github.com/Mattbusel/treeproof/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/treeproof.svg)](https://crates.io/crates/treeproof)

**Your coding agent says the tests pass. Nothing you run checks that it ran them, on this code, after its last edit.**
It says so without running them, after running a subset, or before the edit that broke them. Both Claude Code and Codex
have open issues about exactly this ([anthropics/claude-code#92505](https://github.com/anthropics/claude-code/issues/92505),
[openai/codex#59490](https://github.com/openai/codex/issues/59490)). If "done" means "the agent said so", your review is
the test suite.

`treeproof run -- cargo test` runs the command and signs an
[in-toto test-result](https://github.com/in-toto/attestation/blob/main/spec/predicates/test-result.md) attestation:
*this exact working tree passed this exact command*. The agent host refuses "done" without one, CI re-runs it, and
parallel worktrees share it.

| "Done" means | Agent's word | Stop hook that re-runs everything | CI after the push | **treeproof** |
|---|---|---|---|---|
| The tests were run on the code being handed in | trusted | yes | yes, minutes later | **yes, signed, bound to the git tree id** |
| An edit after the run is caught | no | yes | yes | **yes: the receipt goes stale** |
| A subset or 0 collected tests counts as a pass | yes | depends on the hook | depends on the job | **no: `FAILED`** |
| Cost when nothing changed | none | the whole suite, every stop | a CI run | **none: cache hit on the same tree** |
| Four agents in four worktrees on one tree | four claims | four suite runs | four CI runs | **one run, shared** |

treeproof takes the other side of "trust the transcript":

- **Bound to the tree.** The receipt's subject is the git tree id of the working tree, including uncommitted and untracked files. Any edit after the run makes it stale.
- **Signed.** Ed25519 over a DSSE envelope. The agent cannot hand-write a passing receipt.
- **Fails closed.** 0 tests collected, everything filtered out, `--collect-only`, an unrecognised runner, or a non-zero exit is `FAILED`. Failures are never cached as passes. A run of part of the suite does not satisfy the gate, and a failing run blocks it until the same command passes.
- **Shared.** Passing results live in a content-addressed store under your user cache directory. Four agents in four worktrees on the same tree run the suite once; identical runs started at the same time execute once and the rest wait for the result.
- **Open format.** [`SPEC.md`](SPEC.md) defines the receipt so other tools can produce and verify it. CI checks treeproof's receipts with securesystemslib and the in-toto-attestation validators.

Who it is for: anyone who lets Claude Code, Codex or an MCP agent mark work done, and maintainers who get agent-written
pull requests. What it costs you: one binary on `PATH`, and a gate that will block an agent that has not run the suite.
That is the point.

## Install

```sh
cargo install treeproof --version 0.1.3 --locked
```

Pin the version. The Claude Code plugin and its hooks call whatever `treeproof` is on `PATH`, so install the version that matches the plugin (the plugin's `SessionStart` hook warns when they differ). `--locked` builds with the dependency versions in the published `Cargo.lock`.

Prebuilt binaries for Linux (x86_64, aarch64), macOS and Windows are on the [releases page](https://github.com/Mattbusel/treeproof/releases), with a `SHA256SUMS` file in each release. Check the archive before you unpack it, for example `sha256sum -c SHA256SUMS --ignore-missing` (v0.1.0 has no Intel macOS build; use `cargo install` there). A Python wheel (`pip install treeproof`) and an npm shim (`npm i -g treeproof`) that install the same binary are built by the release workflow; see [docs/install.md](docs/install.md) for their status.

## Use it

```sh
treeproof run -- cargo test --workspace     # or pytest, npm test, go test ./..., cargo nextest run
treeproof status                            # receipts for the current tree, and the gate verdict
treeproof gate                              # exit 0 only if a valid passing receipt covers the current tree
treeproof export                            # copy that receipt and your public key into .treeproof/ to commit
treeproof verify --tree current --rerun .treeproof/receipts/*.json
```

Supported runners: `cargo test` (libtest, including doctests), `cargo nextest`, `pytest` (including `-v` and pytest-xdist), `jest`, `vitest`, and `go test` (default, `-v` and `-json`). Output from several runners in one command is summed.

## Agent hosts

| Surface | What it does | Docs |
|---|---|---|
| Claude Code plugin | `Stop` and `TaskCompleted` hooks block completion until a passing receipt exists for the current tree; `PostToolUse` records test runs the agent made with Bash; FileGuard refuses edits to files that changed since the agent read them | [docs/claude-code.md](docs/claude-code.md) |
| Codex hooks | Same gate and recording through Codex's `Stop`, `PostToolUse` and `PreToolUse` hooks | [docs/codex.md](docs/codex.md) |
| MCP server | `receipt_run` (only the committed `[gate] commands` or a recognised test runner, never a general shell), `receipt_query`, `receipt_verify` for any MCP client | [docs/mcp.md](docs/mcp.md) |
| GitHub Action | `uses: Mattbusel/treeproof/verify@v0.1.3` verifies the committed receipt against keys and policy from the base branch, re-runs its command on the checked-out tree, and fails on any mismatch | [docs/github-action.md](docs/github-action.md) |
| Orchestrators | `treeproof gate --json`, `treeproof verify --json`, or the Rust crate | [docs/orchestrators.md](docs/orchestrators.md) |

Claude Code:

```text
/plugin marketplace add Mattbusel/treeproof
/plugin install treeproof@treeproof
```

The plugin calls the `treeproof` binary, so install that first, pinned to the plugin's version: `cargo install treeproof --version 0.1.3 --locked`.

**The gate is on in every git repository by default.** Once the plugin is enabled, any session that changes files in a git repository has to end with a passing `treeproof run` for that tree (or hits the 3-block limit and stops with a warning). To turn it off:

- for one repository: commit `treeproof.toml` with `[gate]` `mode = "off"` (or `"warn"` to only warn);
- for everything: set `TREEPROOF_GATE=off` (or `warn`) in the environment Claude Code runs in, for example `"env": {"TREEPROOF_GATE": "off"}` in `~/.claude/settings.json`;
- or disable the plugin: `/plugin disable treeproof@treeproof`.

## What a stop looks like

With the plugin enabled, an agent that edits code and then says it is done without a passing run gets this back from the `Stop` hook, and keeps working:

```text
treeproof: no treeproof receipt exists for the current tree 3f1c0a9e2b77. Run the test suite through
treeproof, for example `treeproof run -- cargo test`, fix any failures, and finish only when it passes.
```

If the session changed nothing, the hook does not ask for anything. After three blocks on the same tree it lets the agent stop with a visible warning, so a broken environment cannot trap a session.

## Policy

An optional `treeproof.toml` at the repository root:

```toml
[gate]
commands = ["cargo test --workspace"]   # only receipts from these commands count (a trailing * allows extra args)
min_tests = 1
accept_observed = false                  # receipts recorded from the agent's own Bash runs
allow_subset = false                     # runs of part of the suite (name filters, -k, ::node ids)
mode = "block"                           # block, warn or off

[run]
env = ["TEST_FEATURES"]                  # extra environment variables that are part of the cache key

[fileguard]
enabled = true
```

The gate reads this file as committed, never from the working copy: hooks use the version committed when the session started, `treeproof gate` uses `HEAD`, and the GitHub Action uses the pull request's base commit. See [docs/configuration.md](docs/configuration.md).

## Telemetry (OpenTelemetry)

**Nothing is sent anywhere by default.** treeproof has no usage telemetry and no phone-home. The only network traffic it can make is an OTLP export of the events below, and the hooks do that only when you opt in:

- `TREEPROOF_OTLP_ENDPOINT=http://...` sends to that endpoint;
- the generic `OTEL_EXPORTER_OTLP_ENDPOINT` / `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` are used only together with `TREEPROOF_OTEL=1`. (Before 0.1.2 the hooks also exported when only the generic variable was set; if you have it set for other tools, 0.1.2 stops that.)
- `TREEPROOF_OTEL=0` turns every export off, including `TREEPROOF_OTLP_ENDPOINT`.

What is sent: the three evaluation results, their explanations, and the git tree id. No source, test output, file names or environment values. `treeproof otel` (run by hand) also reads the generic variables, because you asked it to export.

`treeproof otel` and the `Stop` hook can emit OpenTelemetry `gen_ai.evaluation.result` events (`verification.exit_code`, `verification.fs_diff`, `verification.test_rerun`, labelled `verified`, `contradicted` or `unknown`) over OTLP/HTTP JSON, linked to the host span through `TRACEPARENT` when one is present. CI sends them to a real OpenTelemetry Collector and checks what arrives. See [docs/otel.md](docs/otel.md).

## What it does not do

- It does not make an agent honest. It makes the honest path cheap and the dishonest path explicit. With shell access an agent running as you can still reach the signing key or write a fake test runner; the hooks refuse the obvious attempts, and a fake runner or edited test shows up in the diff. Pin the real command with `[gate] commands` and re-run in CI with the Action, which trusts only the base branch's keys and policy. See [docs/threat-model.md](docs/threat-model.md).
- The signing key sits in your user config directory. An agent with shell access running as you could read it; the hooks refuse the obvious attempts, but this is not a sandbox.
- Tests that depend on things outside the tree (network, a database, environment variables not in the allowlist) can pass once and fail later with the same key. Add those variables to `[run] env`, or use `--no-cache`.

## Documentation

- [SPEC.md](SPEC.md): receipt format, tree digest, run key, verification and gate rules
- [docs/cache.md](docs/cache.md): the cross-worktree store and single-flight
- [docs/threat-model.md](docs/threat-model.md)
- [docs/fileguard.md](docs/fileguard.md)
- [CHANGELOG.md](CHANGELOG.md)

## License

MIT or Apache-2.0, at your option.
