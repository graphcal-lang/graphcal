# Contributing to Graphcal

This file collects the commands and conventions the repository already uses.
The full project guidelines live in [`CLAUDE.md`](CLAUDE.md).

## Build

Install the source-build prerequisites listed in
[Build from source](docs/en/installation.md#build-from-source) (the pinned Rust
toolchain, the `wasm32-unknown-unknown` target, `wasm-bindgen-cli`, and
Binaryen's `wasm-opt`). Then build with Cargo:

```bash
cargo build
```

The CLI build generates and verifies the embedded browser engine under Cargo's
`OUT_DIR`. Do not bypass its freshness or integrity checks, and do not commit
the generated engine files.

## Tools for the checks

The `just` recipes below call tools beyond the build prerequisites. Install
the ones for the recipes you run:

| Tool | Used by |
|------|---------|
| [`just`](https://github.com/casey/just) | every recipe |
| `lake`, through [`elan`](https://github.com/leanprover/elan) (Lean) | `formal`, part of `just test` and `just lint` |
| [`ripgrep`](https://github.com/BurntSushi/ripgrep) (`rg`) | `formal` |
| `vp` (see [`internals/playground.md`](internals/playground.md)) | playground and view recipes, part of `just test` and `just lint` |
| [`wasm-pack`](https://github.com/drager/wasm-pack) | `playground-assets`, part of `just test` |
| [Node.js](https://nodejs.org/) (`node`) | report-runtime tests in `just test` |
| [`cargo-audit`](https://github.com/rustsec/rustsec/tree/main/cargo-audit) | `just lint` |
| [Nushell](https://www.nushell.sh/) (`nu`) | `refactor-metrics`, part of `just lint` |
| [`uv`](https://docs.astral.sh/uv/) | `reading-order`, part of `just lint` |

## Test

```bash
just test
```

This runs `cargo test --workspace` together with the formal conformance,
pipeline-layer, playground, view, and report-runtime tests defined in the
[`justfile`](justfile).

## Format and lint

```bash
cargo fmt
just lint
```

`just lint` runs `cargo fmt --check`, Clippy with warnings denied, `cargo audit`,
`cargo doc`, and the repository-specific checks (formal model, pipeline layers,
playground and view checks, refactor metrics, and reading order).

`just hawk` runs the `cargo-hawk` audit of the CLI. It needs `cargo-hawk` built
for the exact Rust version pinned as `hawk_rustc` in the `justfile`.

The repository also has a [`pre-commit`](https://pre-commit.com/) configuration
in [`.pre-commit-config.yaml`](.pre-commit-config.yaml), including `rumdl` for
Markdown and `typos` for spelling:

```bash
pre-commit install
pre-commit run --all-files
```

## Coding conventions

Graphcal prioritizes safety over usability and prefers explicitness over
implicitness: no implicit type or unit conversion, no implicit type inference,
and no implicit null propagation.

The compiler follows the same standard. The
[Type Safety section of `CLAUDE.md`](CLAUDE.md#type-safety-encode-semantics-in-types-not-conventions)
describes the rules, including:

- Encode distinct semantic concepts as distinct types, not string conventions.
- Keep a functional core with I/O at the shell (loader, LSP server, CLI).
- Prefer small modules with a single responsibility and a clear dependency
  direction.

When you add, change, or remove a feature, update the tests, the LSP features in
`crates/graphcal-lsp/`, and the editor grammars and extensions listed under
[Implementation Guidelines in `CLAUDE.md`](CLAUDE.md#implementation-guidelines).

## Architecture

Start with the
[codebase reading guide](internals/codebase-reading-guide.md). It describes
the compiler pipeline, the crate boundaries, and a suggested reading order for
the source files.

The formal grammar is [`grammar.ebnf`](grammar.ebnf).

## Documentation

The user-facing documentation is a learning resource, not a changelog. Do not
update it by default when fixing bugs or adding features; change it only when
the change gives lasting value to someone learning Graphcal. See the
[Documentation section of `CLAUDE.md`](CLAUDE.md#documentation).

The documentation lives in `docs/en/` and `docs/ja/`. Update both languages
together, following [`internals/docs-localization.md`](internals/docs-localization.md),
and build the site with:

```bash
just docs-build
```

## Pull requests

Fill in the [pull request template](.github/pull_request_template.md): a
summary, the AI-assistance disclosure, and the breaking-change checklist.

A **breaking change** is a change that requires existing Graphcal source
programs to be modified. Changes only to Rust APIs or internal implementation
interfaces do not count.

## License

Graphcal is licensed under either the [MIT License](LICENSE-MIT) or the
[Apache License, Version 2.0](LICENSE-APACHE), at your option.
