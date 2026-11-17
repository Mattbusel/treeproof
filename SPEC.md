# treeproof receipt specification

Version: 1 (receipts produced by treeproof 0.1.x)
Status: draft, open for implementation by other tools

A **treeproof receipt** is a signed statement that one exact git working tree passed (or failed) one exact test command. It is an [in-toto Statement v1](https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md) carrying the [test-result v0.1 predicate](https://github.com/in-toto/attestation/blob/main/spec/predicates/test-result.md), wrapped in a [DSSE v1](https://github.com/secure-systems-lab/dsse/blob/master/protocol.md) envelope and signed with Ed25519.

Nothing in this document requires the treeproof crate. Any tool can produce receipts that treeproof verifies, and verify receipts that treeproof produces. The key words MUST, SHOULD and MAY are used as in RFC 2119.

## 1. Envelope

A receipt is one JSON object, the DSSE envelope:

```json
{
  "payloadType": "application/vnd.in-toto+json",
  "payload": "<base64 of the serialized Statement>",
  "signatures": [
    { "keyid": "<hex sha256 of the 32 raw public key bytes>", "sig": "<base64 Ed25519 signature>" }
  ]
}
```

- `payloadType` MUST be `application/vnd.in-toto+json`.
- The signature is Ed25519 (RFC 8032) over `PAE(payloadType, payload_bytes)`, where `PAE` is the DSSE pre-authentication encoding: `"DSSEv1" SP LEN(type) SP type SP LEN(body) SP body`.
- Producers SHOULD write standard base64 with padding. Verifiers MUST accept standard or URL-safe base64, padded or not.
- `keyid` is the lowercase hex SHA-256 of the 32-byte raw Ed25519 public key. It is a hint only: verifiers MUST NOT make security decisions on it and SHOULD try every trusted key.
- A receipt file MAY end with a newline. Receipt files are conventionally named `<gitTree>.json`.

## 2. Statement

```json
{
  "_type": "https://in-toto.io/Statement/v1",
  "subject": [ { "name": "worktree", "digest": { "gitTree": "<tree id>" } } ],
  "predicateType": "https://in-toto.io/attestation/test-result/v0.1",
  "predicate": {
    "result": "PASSED",
    "configuration": [ <run descriptor>, <lockfile descriptors...> ],
    "url": "<optional link to the run>",
    "passedTests": ["..."],
    "warnedTests": ["..."],
    "failedTests": ["..."]
  }
}
```

- `subject` MUST contain exactly one descriptor, and it MUST have a `gitTree` digest (40 lowercase hex characters for SHA-1 repositories, 64 for SHA-256 repositories), computed as in section 4. Its `name` SHOULD be `worktree`. Verifiers MUST reject a statement with any other number of subjects.
- The predicate uses only the fields defined by test-result v0.1. treeproof-specific data lives in `configuration` descriptors (section 3), so a strict test-result parser accepts the predicate.
- `passedTests`, `warnedTests` and `failedTests` hold test names when the runner printed them (for example `cargo test`, `pytest -v`, `go test -v`). They MAY be empty when the runner printed only totals; the counts in the run record are authoritative for the totals. Producers SHOULD cap each list (treeproof keeps at most 5000 names).

### 2.1 Result rules

A producer MUST set `result` by these rules, in order:

1. The command only lists or collects tests (`--collect-only`, `--co`, `--list`, `--listTests`, `--no-run`, `nextest list`, `--passWithNoTests`): `FAILED`.
2. The command exited with a non-zero status, or was killed (signal or timeout): `FAILED`.
3. Any test failed or errored: `FAILED`.
4. No runner summary was recognised in the output: `FAILED`, unless the operator explicitly allowed unrecognised output, in which case `PASSED` with `reason` set in the run record and zero counts.
5. Zero tests passed (nothing collected, everything filtered out, deselected or skipped): `FAILED`.
6. Some tests passed with a warning (pytest XPASS): `WARNED`.
7. Otherwise: `PASSED`.

For a `fresh` receipt produced without the rule 4 exception, `PASSED` therefore means: the command ran to completion with exit status 0, the runner reported at least one passing test, and no test failed. Two kinds of `PASSED` receipt carry less: an `observed` receipt (section 3.4) may have `exitCode` null, because some hosts do not report it, and its output came from the agent's own shell; a rule 4 receipt has empty `runners` and zero counts and rests on the exit status alone. Gates treat both as not satisfying the policy unless it says otherwise (section 7).

## 3. Configuration descriptors

### 3.1 Run descriptor (required)

Exactly one `configuration` entry MUST have `"name": "treeproof-run"`:

```json
{
  "name": "treeproof-run",
  "digest": { "sha256": "<run key>" },
  "mediaType": "application/vnd.treeproof.run+json",
  "annotations": { <run record v1> }
}
```

`digest.sha256` MUST equal `annotations.cacheKey`.

### 3.2 Run record v1

The annotations object. Unknown fields MUST be ignored by verifiers.

| Field | Type | Meaning |
|---|---|---|
| `schema` | string | `https://github.com/Mattbusel/treeproof/blob/main/SPEC.md#run-record-v1` |
| `argv` | array of string | The command. When `shell` is true it holds one string, the shell command line. |
| `shell` | bool | The command ran through the platform shell (`sh -c` or `cmd /C`). |
| `cwd` | string | Working directory relative to the repository root, `/`-separated; `.` for the root. MUST NOT be absolute or contain `..`. |
| `runners` | array of string | Runners whose summary was recognised: `cargo`, `nextest`, `pytest`, `jest`, `vitest`, `go`. |
| `counts` | object | `passed`, `failed`, `skipped`, `warned` (integers). For `go test` without `-v` or `-json` the counts are packages, and `notes` says so. |
| `exitCode` | int or null | Exit status; null when killed or unknown (observed runs). |
| `mode` | string | `fresh` (the producer executed the command) or `observed` (recorded from a host's own tool output, see 3.4). |
| `cacheKey` | string | The run key (section 5). |
| `startedAt`, `finishedAt` | string | RFC 3339 UTC timestamps. |
| `durationMs` | int | Wall time. |
| `headCommit` | string or null | `HEAD` at the time of the run, for orientation only. The subject, not this field, is what was tested. |
| `toolchains` | object | Version strings for toolchains the command uses, for example `{"rustc": "rustc 1.91.0 (...)"}`. |
| `env` | object | Allowlisted environment variable names mapped to HMAC-SHA256 (hex) of their value under a per-machine secret key, or null when unset. Values are never stored, and without the secret a published value cannot be tested against guesses. Verifiers MUST NOT interpret these values; they only make the run key differ when a value differs. (Receipts from treeproof 0.1.0 hold an unkeyed SHA-256 here.) |
| `os`, `arch` | string | Rust `std::env::consts::OS` / `ARCH` names. |
| `notes` | array of string | Human-readable notes (filtered tests, timeouts, tree changed during the run). |
| `reason` | string or null | Why the result is not `PASSED`, or a caveat. |
| `host` | string or null | For observed runs, the host that ran the command (`claude`, `codex`). |
| `treeproofVersion` | string | Producer version. |

### 3.3 Lockfile descriptors (optional)

One descriptor per lockfile found at the repository root or in `cwd`, `{"name": "<path relative to root>", "digest": {"sha256": "<hex>"}}`. Lockfiles are usually also part of the tree; listing them makes dependency pins visible without unpacking the tree.

### 3.4 Observed runs

A host hook (for example Claude Code or Codex `PostToolUse`) MAY record a receipt from the output of a test command the agent ran itself. Such receipts MUST set `mode` to `observed`, `shell` to true, and `exitCode` to the exit status when the host reports one and null otherwise. Observed receipts are never cache entries. A producer SHOULD record observed runs only for a plain invocation of a known test runner, with nothing chained, piped, redirected or substituted, so the captured output is the runner's own. Policies decide whether observed passes count (`accept_observed`, default false).

### 3.5 Nested-repository descriptors (optional)

One descriptor per nested git repository in the working tree (an initialised submodule or an embedded repository), `{"name": "<path relative to root>", "digest": {"gitTree": "<working-tree digest of that repository>"}, "mediaType": "application/vnd.treeproof.nested-tree"}`, computed with section 4 inside the nested repository, recursively up to 4 levels. See 4.1.

## 4. Working-tree digest

The subject digest is the git tree id of the working tree as `git add -A` would stage it, computed without touching the user's index:

1. Copy the repository's index file (`git rev-parse --path-format=absolute --git-path index`) to a temporary path `T`. If there is no index, start empty.
2. With `GIT_INDEX_FILE=T`, run `git add -A -- .` at the repository root. Tracked, modified and untracked non-ignored files are staged; ignored files are not.
3. With `GIT_INDEX_FILE=T`, run `git rm -r -q --cached --ignore-unmatch --` with these pathspecs:
   `:(glob).treeproof/**`, `:(glob)**/__pycache__/**`, `:(glob)**/*.pyc`, `:(glob)**/.pytest_cache/**`.
4. With `GIT_INDEX_FILE=T`, `git write-tree` prints the tree id. Delete `T`.

Consequences:

- For a clean checkout whose tree contains none of the excluded paths, the digest equals `git rev-parse HEAD^{tree}`.
- Receipts and trusted keys committed under `.treeproof/` do not change the digest, so a receipt can be committed next to the code it covers.
- Git's clean filters and end-of-line normalisation apply, so the same content checked out on Windows and Linux has the same digest.
- Step 2 writes blob objects for new content into the object database. They are unreferenced and are removed by `git gc` in due course.

### 4.1 Nested repositories

`git add -A` stages a submodule or an embedded repository as a gitlink: the commit its `HEAD` points at. Uncommitted changes inside it do not change the digest. A producer SHOULD therefore record the working-tree digest of each nested repository as a configuration descriptor (3.5) and include them in the run key (section 5), and a gate MUST compare them with the current nested digests (section 7). An uninitialised submodule (no `.git` inside its directory) has no working tree and is skipped. Receipts without nested descriptors say nothing about nested content.

## 5. Run key

The run key decides when a cached pass may be reused. It is SHA-256 (lowercase hex) of the compact JSON serialisation, with object keys sorted and no insignificant whitespace, of:

```json
{"arch": ..., "argv": [...], "cwd": ..., "env": {...}, "lockfiles": {...}, "os": ..., "shell": ..., "toolchains": {...}, "tree": "<gitTree>", "v": 1}
```

with the values defined in 3.2 and `lockfiles` mapping path to SHA-256. When the working tree contains nested repositories (4.1), the object also has a `"nested"` member mapping each path to its working-tree digest; it is omitted otherwise. The default environment allowlist is `RUSTFLAGS RUSTDOCFLAGS CARGO_BUILD_TARGET CARGO_PROFILE PYTHONPATH PYTHONHASHSEED PYTEST_ADDOPTS NODE_ENV NODE_OPTIONS GOFLAGS GOOS GOARCH CGO_ENABLED GOEXPERIMENT`, extended per repository by `[run] env` in `treeproof.toml`.

The run key is a cache index, not a security property. Verifiers MUST NOT recompute it to decide trust; they check the signature and the subject.

## 6. Verification

To verify a receipt against a set of trusted Ed25519 public keys:

1. Parse the envelope. Reject if it is not a JSON object with `payloadType`, `payload` and a non-empty `signatures` array.
2. Reject if `payloadType` is not `application/vnd.in-toto+json`.
3. Decode `payload` once. Accept the envelope only if at least one signature verifies over `PAE(payloadType, payload_bytes)` with a trusted key.
4. Parse those same payload bytes (never re-read the envelope) as the Statement. Reject unless `_type`, `predicateType`, `result` and the subject `gitTree` are as in section 2, and the run descriptor exists with `digest.sha256 == annotations.cacheKey`.
5. If the verifier expects a specific tree (for example the checked-out tree in CI, computed as in section 4), reject when the subject differs.
6. A receipt whose `result` is `FAILED` verifies as a record of a failure; it does not satisfy a gate.
7. Optionally, re-run: execute `argv` in `cwd` (through the shell when `shell` is true) on a checkout of the subject tree, apply the result rules of 2.1, and reject if the re-run's pass/fail outcome differs from the receipt's.

## 7. Gate semantics

A **gate** (a host hook, an orchestrator, CI) is satisfied for a tree when it holds at least one receipt that:

- verifies (section 6) against its trusted keys,
- has the current tree as subject, and nested-repository descriptors (3.5) equal to the current nested digests,
- has `result` `PASSED` or `WARNED`,
- has at least one recognised runner, unless the policy allows unrecognised output,
- has `counts.passed` at least the policy minimum (default 1),
- if the policy lists required commands, has a matching command,
- has `mode` `fresh`, unless the policy accepts observed receipts (default: it does not),
- did not select a subset of the tests, unless the policy allows subsets or the command matches a required command exactly (treeproof's detection is described in `docs/configuration.md`),

and no receipt for the same tree records failing tests for a command (failed tests, or a non-zero exit with a recognised runner) without a later passing receipt of the same command.

"Fresh" means "for the current tree". Any change to a tracked or untracked non-ignored file, or inside a nested repository, changes the tree or its nested digests and invalidates every earlier receipt for gating purposes.

The policy and the trusted keys MUST come from a source the party being checked cannot write. For a coding agent that is not the working copy: treeproof's hooks use the policy committed when the session started and trust only the local signing key; CI uses the keys and policy of the pull request's base commit.

## 8. Keys

- Public keys are distributed as PEM `PUBLIC KEY` (SubjectPublicKeyInfo, RFC 8410), the format OpenSSL and cosign use. Hex or base64 of the 32 raw bytes is also accepted by treeproof.
- A repository that wants CI to verify receipts commits the public keys it trusts under `.treeproof/trusted_keys/*.pem`. CI reads them from the base commit of a change, never from the change itself.
- The private key never appears in a receipt or in the repository.

## 9. Example

Envelope (payload shortened):

```json
{
  "payloadType": "application/vnd.in-toto+json",
  "payload": "eyJfdHlwZSI6Imh0dHBzOi8vaW4tdG90by5pby9TdGF0ZW1lbnQvdjEiLCJz...",
  "signatures": [
    {
      "keyid": "873656325708a3235f0902c829b6464c9f795e32da14f569537f658f0b139d84",
      "sig": "nK/P1o9haUYA6hzPBJWbDJIoOGaFmZhGTnmSFx3rqGXxCKzGs5vw/8Ji8tCb3qNzZoYqB019rnBWkJIMs/vgBw=="
    }
  ]
}
```

Decoded statement (environment allowlist shortened):

```json
{
  "_type": "https://in-toto.io/Statement/v1",
  "subject": [{ "name": "worktree", "digest": { "gitTree": "59c443ae396bebbe0e43206ad932ada076cf76ae" } }],
  "predicateType": "https://in-toto.io/attestation/test-result/v0.1",
  "predicate": {
    "result": "PASSED",
    "configuration": [
      {
        "name": "treeproof-run",
        "digest": { "sha256": "b9c4cbd71da076257ac6314553ac4549fdb469f2765510695b970b9424304e51" },
        "mediaType": "application/vnd.treeproof.run+json",
        "annotations": {
          "schema": "https://github.com/Mattbusel/treeproof/blob/main/SPEC.md#run-record-v1",
          "argv": ["python", "run_tests.py"],
          "shell": false,
          "cwd": ".",
          "runners": ["cargo"],
          "counts": { "passed": 3, "failed": 0, "skipped": 0, "warned": 0 },
          "exitCode": 0,
          "mode": "fresh",
          "cacheKey": "b9c4cbd71da076257ac6314553ac4549fdb469f2765510695b970b9424304e51",
          "startedAt": "2026-11-17T17:43:47.020Z",
          "finishedAt": "2026-11-17T17:43:47.046Z",
          "durationMs": 26,
          "headCommit": "2667395d39cae3ccca9143e651911e07fd364538",
          "toolchains": { "python": "Python 3.12.10" },
          "env": { "PYTHONPATH": null, "RUSTFLAGS": null },
          "os": "windows",
          "arch": "x86_64",
          "notes": [],
          "reason": null,
          "host": null,
          "treeproofVersion": "0.1.0"
        }
      }
    ],
    "passedTests": ["math::adds", "math::subs", "math::divs"],
    "warnedTests": [],
    "failedTests": []
  }
}
```

(The fixture prints libtest-format output, which is why `runners` is `cargo`.)

## 10. Conformance

The treeproof repository checks its receipts against reference implementations in CI:

- the DSSE signature with [securesystemslib](https://github.com/secure-systems-lab/securesystemslib) `Envelope.verify`,
- the Statement and ResourceDescriptors with the [in-toto-attestation](https://github.com/in-toto/attestation/tree/main/python) Python validators,
- the predicate with a strict protobuf JSON parse into the test-result v0 message (unknown fields rejected),
- the public key PEM with OpenSSL through `cryptography`,
- PAE against the DSSE protocol's published test vector and Ed25519 against RFC 8032 test 1.

See `tests/interop/check_receipt.py`; it takes a receipt and a PEM and can be used to check any producer.

## 11. Versioning

Additive changes to the run record keep version 1. A change to the tree digest procedure, the result rules or the meaning of an existing field creates a new `schema` URI, and verifiers SHOULD reject schemas they do not know when they gate on a receipt.
