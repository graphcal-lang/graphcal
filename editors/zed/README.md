# Graphcal Extension for Zed

Provides syntax highlighting and LSP diagnostics for Graphcal (`.gcl`) files in [Zed](https://zed.dev/).

- **Syntax highlighting** via a tree-sitter grammar
- **Diagnostics** via `graphcal-lsp` (parse errors, type/dimension mismatches, unknown references, etc.)

## Local Testing

### Prerequisites

- [Zed](https://zed.dev/) editor
- [Rust](https://rustup.rs/) installed via `rustup` (not Homebrew — Zed compiles the tree-sitter grammar from source)

### Install as Dev Extension

1. Build the LSP server: `cargo build --release -p graphcal-lsp`
2. Open Zed
3. Open the command palette: `Cmd+Shift+P`
4. Run `zed: install dev extension`
5. Select the `editors/zed/` directory
6. Open a `.gcl` file — syntax should be highlighted and diagnostics should appear

### LSP Configuration

The extension looks for `graphcal-lsp` on your `PATH`. To use a local build instead, add a `.zed/settings.json` to your project (already included in this repo):

```jsonc
{
  "lsp": {
    "graphcal-lsp": {
      "binary": {
        "path": "/absolute/path/to/target/release/graphcal-lsp"
      }
    }
  }
}
```

### After Making Changes

- **Grammar changes** (`tree-sitter-graphcal/grammar.js`): Run `tree-sitter generate` in `tree-sitter-graphcal/`, commit the updated `src/` files, push, then re-run `zed: install dev extension`.
- **Highlighting changes** (`editors/zed/languages/graphcal/highlights.scm`): Re-run `zed: install dev extension`.
- **LSP extension changes** (`editors/zed/src/lib.rs`): Re-run `zed: install dev extension`.

## How It Works

The `extension.toml` references the tree-sitter grammar in the same repository via the `[grammars.graphcal]` section. Zed fetches the grammar source using `git fetch --depth 1 origin <rev>`, compiles `parser.c` with clang (targeting WASM), and loads it for parsing.

### Key Details

- **`rev` must be a branch/tag name, not a raw SHA.** The git fetch protocol does not support fetching by commit SHA. The `rev` field currently points to the `tree-sitter-zed` branch.
- **Generated `src/` must be committed.** Zed does not run `tree-sitter generate` — it expects `tree-sitter-graphcal/src/parser.c` to already exist in the repository.
- **`grammars/` is a build artifact.** Zed clones the grammar into `editors/zed/grammars/` at install time. This directory is gitignored.
