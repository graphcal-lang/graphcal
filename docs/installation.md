---
icon: material/download
---

# Installation

## Requirements

- Rust stable toolchain (1.91 or later)

If you don't have Rust installed, get it from [rustup.rs](https://rustup.rs/).

## Install from crates.io

```bash
cargo install graphcal --locked
```

This downloads Graphcal from [crates.io](https://crates.io/crates/graphcal), builds it, and installs the `graphcal` binary to `~/.cargo/bin/`.

## Verify Installation

```bash
graphcal --version
```

## Editor Setup

For the best experience, set up editor integration to get syntax highlighting, diagnostics, and **inlay hints showing computed values**. See [Editor Setup](editor-setup.md) for VS Code, Zed, Helix, and Neovim instructions.

## Next Steps

Proceed to the [Tutorial](tutorial/index.md) to write your first Graphcal file.
