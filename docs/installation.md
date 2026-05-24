---
icon: material/download
---

# Installation

## Requirements

- Rust stable toolchain (1.91 or later)

If you don't have Rust installed, get it from [rustup.rs](https://rustup.rs/).

## Install from Source

```bash
cargo install --git https://github.com/graphcal-lang/graphcal --locked
```

This builds and installs the `graphcal` binary to `~/.cargo/bin/`.

## Verify Installation

```bash
graphcal --version
```

## Editor Setup

For the best experience, set up editor integration to get syntax highlighting, diagnostics, and **inlay hints showing computed values**. See [Editor Setup](editor-setup.md) for VS Code, Zed, and Neovim instructions.

## Next Steps

Proceed to the [Tutorial](tutorial/index.md) to write your first Graphcal file.
