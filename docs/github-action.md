# GitHub Action: `Mattbusel/treeproof/verify`

The action checks that the receipt committed with a change really describes the checked-out tree, and re-runs its command to confirm the result. It fails the job when:

- no receipt in `.treeproof/receipts/` is for the checked-out tree,
- a receipt's signature does not verify against `.treeproof/trusted_keys/`,
- the receipt records a failure,
- the re-run's outcome differs from the receipt (receipt says PASSED, re-run fails).

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
      - uses: Mattbusel/treeproof/verify@v0.1.0
```

Put the action right after checkout and toolchain setup: steps that write files the repository does not ignore would change the tree before it is measured.

## Inputs

| Input | Default | |
|---|---|---|
| `receipts` | `.treeproof/receipts/*.json` | Glob of receipt files. |
| `rerun` | `true` | Re-run the command and compare. |
| `require-current-tree` | `true` | Only receipts for the checked-out tree count; at least one is required. |
| `keys` | | Extra trusted public key files, one per line. |
| `version` | the action's tag | treeproof version to download. |
| `install` | `release` | `release` (prebuilt binary), `cargo` (`cargo install`), or `none` (already on `PATH`). |
| `working-directory` | `.` | Repository directory. |

Outputs: `ok` (`"true"` or `"false"`) and `report` (path of the JSON report).

## Notes

- The re-run executes the command recorded in a receipt signed by a key the repository trusts. That is the same trust as running the repository's own test code. Use it on `pull_request`, not `pull_request_target`.
- The receipt's command must be runnable on the CI runner (for example `python3` rather than `python` on Linux runners).
- The repository's own CI runs this action against a fixture on Linux and Windows, including two must-fail cases: a re-run that disagrees and a tampered receipt.
