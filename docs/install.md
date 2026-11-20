# Installing

| Channel | Command | Notes |
|---|---|---|
| crates.io | `cargo install treeproof --version 0.1.3 --locked` | Builds from source; Rust 1.89 or newer. Pin the version and use `--locked`. |
| Release archives | [releases page](https://github.com/Mattbusel/treeproof/releases) | `x86_64`/`aarch64` Linux (static musl), macOS (Apple silicon, and Intel where the release lists it), Windows x64. Each release has a `SHA256SUMS` file; check it before unpacking. v0.1.0 has no Intel macOS archive; use `cargo install` there. |
| PyPI | `pip install treeproof` | A wheel that contains only the binary (maturin `bindings = "bin"`). Built for every release; published once PyPI trusted publishing is enabled for the project. |
| npm | `npm i -g treeproof` | A small launcher that downloads the matching release binary on install or first use. Published once an npm publishing credential is configured for the repository. |

## Pin the version

The Claude Code and Codex hooks and the MCP entry run `treeproof` from `PATH`. Install the same version as the plugin you enabled, and upgrade both together. The plugin's `SessionStart` hook prints a warning when the binary on `PATH` is a different version from the plugin.

Release archive, checked:

```sh
v=0.1.3
t=x86_64-unknown-linux-musl     # or aarch64-unknown-linux-musl, aarch64-apple-darwin, x86_64-apple-darwin
curl -LO https://github.com/Mattbusel/treeproof/releases/download/v$v/treeproof-$t.tar.gz
curl -LO https://github.com/Mattbusel/treeproof/releases/download/v$v/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing   # macOS: shasum -a 256 -c SHA256SUMS --ignore-missing
tar xzf treeproof-$t.tar.gz
```

On Windows, compare `Get-FileHash treeproof-x86_64-pc-windows-msvc.zip` with the line in `SHA256SUMS`.

Check the install:

```sh
treeproof --version
treeproof key init
```

Then see [claude-code.md](claude-code.md), [codex.md](codex.md) or [github-action.md](github-action.md).
