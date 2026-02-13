# Graphcal Extension for Zed

Provides syntax highlighting for Graphcal (`.gcl`) files in [Zed](https://zed.dev/) using a tree-sitter grammar.

## Local Testing

### Prerequisites

- [Zed](https://zed.dev/) editor
- [Rust](https://rustup.rs/) installed via `rustup` (not Homebrew — Zed compiles the grammar from source)

### Install as Dev Extension

1. Open Zed
2. Open the command palette: `Cmd+Shift+P`
3. Run `zed: install dev extension`
4. Select the `editors/zed/` directory
5. Open a `.gcl` file — syntax should be highlighted

### After Making Changes

- **Grammar changes** (`tree-sitter-graphcal/grammar.js`): Run `tree-sitter generate` in `tree-sitter-graphcal/`, commit the updated `src/` files, push, then re-run `zed: install dev extension`.
- **Highlighting changes** (`editors/zed/languages/graphcal/highlights.scm`): Re-run `zed: install dev extension`.

## How It Works

The `extension.toml` references the tree-sitter grammar in the same repository via the `[grammars.graphcal]` section. Zed fetches the grammar source using `git fetch --depth 1 origin <rev>`, compiles `parser.c` with clang (targeting WASM), and loads it for parsing.

### Key Details

- **`rev` must be a branch/tag name, not a raw SHA.** The git fetch protocol does not support fetching by commit SHA. The `rev` field currently points to the `tree-sitter-zed` branch.
- **Generated `src/` must be committed.** Zed does not run `tree-sitter generate` — it expects `tree-sitter-graphcal/src/parser.c` to already exist in the repository.
- **`grammars/` is a build artifact.** Zed clones the grammar into `editors/zed/grammars/` at install time. This directory is gitignored.
