---
icon: material/download
---

# Installation

## Requirements

- Rust stable toolchain (1.91 or later)

If you don't have Rust installed, get it from [rustup.rs](https://rustup.rs/).

## Install from crates.io

```bash
cargo install graphcal --version '^0.0.1-alpha' --locked
```

The explicit version requirement is necessary while Graphcal is published as a pre-release. This downloads the latest compatible Graphcal release from [crates.io](https://crates.io/crates/graphcal), builds it, and installs the `graphcal` binary to `~/.cargo/bin/`.

## Verify Installation

```bash
graphcal --version
# graphcal <version> (commit: <sha>)
```

The commit suffix is shown when the build can determine the source commit.

## Editor Setup

For the best experience, set up editor integration to get syntax highlighting, diagnostics, and **inlay hints showing computed values**. See [Editor Setup](editor-setup.md) for VS Code, Zed, Helix, and Neovim instructions.

## Next Steps

Proceed to the [Tutorial](tutorial/index.md) to write your first Graphcal file.
