# Installing

| Channel | Command | Notes |
|---|---|---|
| crates.io | `cargo install treeproof` | Builds from source; Rust 1.89 or newer. |
| Release archives | [releases page](https://github.com/Mattbusel/treeproof/releases) | `x86_64`/`aarch64` Linux (static musl), macOS (Apple silicon and Intel), Windows x64. `SHA256SUMS` alongside. |
| PyPI | `pip install treeproof` | A wheel that contains only the binary (maturin `bindings = "bin"`). Built for every release; published once PyPI trusted publishing is enabled for the project. |
| npm | `npm i -g treeproof` | A small launcher that downloads the matching release binary on install or first use. Published once an npm publishing credential is configured for the repository. |

Check the install:

```sh
treeproof --version
treeproof key init
```

Then see [claude-code.md](claude-code.md), [codex.md](codex.md) or [github-action.md](github-action.md).
