---
icon: material/download
---

# Installation

Graphcal is a single command-line program, `graphcal`. Prebuilt binaries are available for Linux (x86_64, ARM64), macOS (Intel, Apple Silicon), and Windows (x86_64).

## Install with the installer script

On macOS and Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/graphcal-lang/graphcal/releases/latest/download/graphcal-installer.sh | sh
```

On Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/graphcal-lang/graphcal/releases/latest/download/graphcal-installer.ps1 | iex"
```

Open a new terminal afterwards so that `graphcal` is on your `PATH`.

## Install with a package manager

While Graphcal is a pre-release, these commands need an explicit version. Replace `<version>` with a version from the [releases page](https://github.com/graphcal-lang/graphcal/releases), such as `0.0.1-alpha.35`.

### cargo-binstall

[cargo-binstall](https://github.com/cargo-bins/cargo-binstall) downloads the prebuilt binary:

```bash
cargo binstall graphcal@<version>
```

### cargo install

`cargo install` builds Graphcal from [crates.io](https://crates.io/crates/graphcal). It requires Rust 1.95 or later, which you can get from [rustup.rs](https://rustup.rs/):

```bash
cargo install graphcal@<version> --locked
```

## Build from source

Building from a source checkout requires these tools:

- The Rust toolchain pinned in `rust-toolchain.toml`, managed by rustup.
- Its `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`
  from the repository root).
- `wasm-bindgen-cli` matching the **resolved `wasm-bindgen` version in `Cargo.lock`**:
  `cargo install wasm-bindgen-cli --version <locked-version> --locked`.
- Binaryen's `wasm-opt` on `PATH`. CI uses Binaryen 117; prebuilt distributions
  are available from [Binaryen releases](https://github.com/WebAssembly/binaryen/releases/tag/version_117).

Then build as usual:

```bash
cargo build
```

The first build takes longer than usual. `cargo check` and Clippy also need these tools.

## Verify Installation

```bash
graphcal --version
# graphcal <version> (commit: <sha>)
```

## Use in GitHub Actions

The [`setup-graphcal`](https://github.com/graphcal-lang/setup-graphcal) action installs `graphcal` on a GitHub Actions runner. This workflow checks the `.gcl` files and evaluates a model on each push and pull request:

```yaml
name: Graphcal

on:
  push:
    branches: [main]
  pull_request:

permissions:
  contents: read

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: graphcal-lang/setup-graphcal@v1
        with:
          version: 0.0.1-alpha.35
      - run: graphcal check
      - run: graphcal eval model.gcl
```

The job fails when an assertion fails. Graphcal still changes between alpha releases, so pin `version` to keep the results reproducible. See the [action's README](https://github.com/graphcal-lang/setup-graphcal#readme) for other options.

## Editor Setup

For the best experience, set up editor integration to get syntax highlighting, diagnostics, and **inlay hints showing computed values**. See [Editor Setup](editor-setup.md) for VS Code, Zed, Helix, and Neovim instructions.

## Next Steps

Proceed to the [Tutorial](tutorial/index.md) to write your first Graphcal file.
