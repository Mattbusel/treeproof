# GitHub Action: `Mattbusel/treeproof/verify`

The action checks that the receipt committed with a change really describes the checked-out tree, and re-runs its command to confirm the result. It fails the job when:

- no receipt in `.treeproof/receipts/` is for the checked-out tree,
- a receipt's signature does not verify against the keys committed on the base branch (plus any `keys` input),
- the receipt does not satisfy the base branch's `treeproof.toml` (required commands, minimum tests, observed or subset runs),
- the receipt records a failure,
- the re-run's outcome differs from the receipt (receipt says PASSED, re-run fails).

## What it trusts

On `pull_request` events the action reads `.treeproof/trusted_keys/` and `treeproof.toml` from the pull request's **base commit**, not from the pull request. A change cannot add its own signing key or relax the policy and have CI accept it. On other events it uses `HEAD`. Set `trust-ref` to choose another revision.

This means a signer's public key has to be on the base branch before their receipts verify. To set up a repository:

1. On a machine you trust, `treeproof key export > .treeproof/trusted_keys/<name>.pem` (or `treeproof export` after a passing run, which writes the key too).
2. Commit it to the default branch, with `treeproof.toml` if you want a policy (for example `[gate] commands = ["cargo test --workspace"]`).
3. From then on, pull requests whose receipts are signed by that key verify.

A pull request that adds a new key verifies once that key is merged. Until then, a maintainer can pass the key through the `keys` input from a trusted location, or re-run the tests and push a receipt signed with an already trusted key.

## Producing the receipt

On the developer's (or agent's) machine, after the last edit:

```sh
treeproof run -- cargo test --workspace
treeproof export          # writes .treeproof/receipts/<tree>.json and .treeproof/trusted_keys/<keyid>.pem
git add .treeproof && git commit
```

Files under `.treeproof/` are excluded from the tree digest, so committing them does not invalidate the receipt.

## Workflow

```yaml
on: [pull_request]
jobs:
  treeproof:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable     # whatever the receipt's command needs
      - uses: Mattbusel/treeproof/verify@v0.1.3
```

Put the action right after checkout and toolchain setup: steps that write files the repository does not ignore would change the tree before it is measured. If the base commit is not in a shallow checkout, the action fetches it. Repositories with submodules need `submodules: recursive` on checkout, because receipts record the content of nested repositories.

## Inputs

| Input | Default | |
|---|---|---|
| `receipts` | `.treeproof/receipts/*.json` | Glob of receipt files. |
| `rerun` | `true` | Re-run the command and compare. |
| `require-current-tree` | `true` | Only receipts for the checked-out tree count; at least one is required. |
| `trust-ref` | base commit on `pull_request`, else `HEAD` | Revision whose `.treeproof/trusted_keys/` and `treeproof.toml` are trusted. |
| `keys` | | Extra trusted public key files, one per line. |
| `version` | the action's tag | treeproof version to download. |
| `install` | `release` | `release` (prebuilt binary, falling back to `cargo install` when there is none for the runner), `cargo` (`cargo install`), or `none` (already on `PATH`). |
| `working-directory` | `.` | Repository directory. |

Outputs: `ok` (`"true"` or `"false"`) and `report` (path of the JSON report).

## Notes

- The re-run executes the command recorded in a receipt signed by a key the base branch trusts and allowed by the base branch's policy. Use it on `pull_request`, not `pull_request_target`.
- The receipt's command must be runnable on the CI runner (for example `python3` rather than `python` on Linux runners).
- The repository's own CI runs this action against fixtures on Linux and Windows, including three must-fail cases: a re-run that disagrees, a tampered receipt, and a pull request signed by a key the base branch does not trust.
