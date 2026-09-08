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

The CLI automatically generates and embeds a release-optimized engine. Generated
JS, Wasm, and checksums stay in Cargo's build directory, not Git. Source or build
input changes invalidate the cache; CLI-only changes reuse the engine. The first
build and subsequent engine changes are slower because they compile both native
and Wasm code. `cargo check`, Clippy, and editor checks can also trigger generation.
Each native profile/target has its own cache; `cargo clean` removes it. If you add
a previously absent Cargo configuration file **outside the repository** (for
example a new global Cargo config), run `cargo clean -p graphcal` so its settings
are included in the engine's next input check. Existing config files and the
repository's `.cargo` directory are watched automatically.

Build scripts do not install prerequisites or silently use an outdated engine.
For an offline source build, prefetch dependencies and set `CARGO_NET_OFFLINE=true`
so both the outer and nested Cargo builds use offline mode. The Wasm build always
uses the pinned repository toolchain, even when testing the native CLI with an
older supported Rust version. Native coverage/sanitizer flags are not applied to
this browser engine.

Published crate archives already contain the verified engine; **crates.io users
do not need these additional tools**. Report artifacts remain self-contained and
work without a network connection.

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
