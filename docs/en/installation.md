---
icon: material/download
---

# Installation

## Install a prebuilt binary

Prebuilt binaries are published on [GitHub Releases](https://github.com/graphcal-lang/graphcal/releases) and need no Rust toolchain.

On macOS and Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/graphcal-lang/graphcal/releases/latest/download/graphcal-installer.sh | sh
```

On Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/graphcal-lang/graphcal/releases/latest/download/graphcal-installer.ps1 | iex"
```

The installer places `graphcal` in `~/.local/bin` (or `$XDG_BIN_HOME` when it is set) and, if needed, adds that directory to `PATH` through your shell profile. Open a new terminal afterwards.

Binaries are available for x86_64 and ARM64 Linux, Intel and Apple Silicon macOS, and x86_64 Windows. The Linux binaries are statically linked, so they also run on musl-based distributions such as Alpine.

To install a specific version, replace `latest/download` in the URL with `download/v<version>`, for example `download/v0.0.1-alpha.35`.

If you use [cargo-binstall](https://github.com/cargo-bins/cargo-binstall), it downloads the same binaries. Give the version explicitly while Graphcal is a pre-release:

```bash
cargo binstall graphcal@<version>
```

## Install from crates.io

Building from crates.io requires the Rust stable toolchain (1.95 or later). If you don't have Rust installed, get it from [rustup.rs](https://rustup.rs/).

```bash
cargo install graphcal --version '^0.0.1-alpha' --locked
```

The explicit version requirement is necessary while Graphcal is published as a pre-release. This downloads the latest compatible Graphcal release from [crates.io](https://crates.io/crates/graphcal), builds it, and installs the `graphcal` binary to `~/.cargo/bin/`.

## Build from a source checkout

Unlike a crates.io installation, building the CLI from a checkout also builds its
embedded browser engine. Install these prerequisites first:

- The Rust toolchain pinned in `rust-toolchain.toml`, managed by rustup.
- Its `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`
  from the repository root).
- `wasm-bindgen-cli` matching the **resolved `wasm-bindgen` version in `Cargo.lock`**:
  `cargo install wasm-bindgen-cli --version <locked-version> --locked`.
- Binaryen's `wasm-opt` on `PATH`. CI uses Binaryen 117; prebuilt distributions
  are available from [Binaryen releases](https://github.com/WebAssembly/binaryen/releases/tag/version_117).

Then run the usual command:

```bash
cargo build
```

The CLI automatically builds and embeds the browser engine. The first build
can take longer because it compiles both native and Wasm code. `cargo check`,
Clippy, and editor checks can also trigger this build.

For an offline source build, prefetch dependencies and set `CARGO_NET_OFFLINE=true`.

Published crate archives already contain the verified engine; **crates.io users
do not need these additional tools**. Report artifacts remain self-contained and
work without a network connection.

## Verify Installation

```bash
graphcal --version
# graphcal <version> (commit: <sha>)
```

The commit suffix is shown when the build can determine the source commit.

## Use in GitHub Actions

The [`setup-graphcal`](https://github.com/graphcal-lang/setup-graphcal) action installs a prebuilt `graphcal` on a GitHub Actions runner in seconds. For example, to check every `.gcl` file in a repository and evaluate a model on each push and pull request:

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

`graphcal eval` exits with a non-zero status when an assertion fails, so the job fails too. Pinning `version` keeps CI reproducible while the language still changes between alpha releases; omit it to install the latest release. See the action's README for all inputs.

## Editor Setup

For the best experience, set up editor integration to get syntax highlighting, diagnostics, and **inlay hints showing computed values**. See [Editor Setup](editor-setup.md) for VS Code, Zed, Helix, and Neovim instructions.

## Next Steps

Proceed to the [Tutorial](tutorial/index.md) to write your first Graphcal file.
