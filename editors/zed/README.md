# Graphcal Extension for Zed

Provides syntax highlighting and LSP diagnostics for Graphcal (`.gcl`) files in [Zed](https://zed.dev/).

- **Syntax highlighting** via a tree-sitter grammar
- **Diagnostics** via `graphcal lsp` (parse errors, type/dimension mismatches, unknown references, etc.)

## Local Testing

### Prerequisites

- [Zed](https://zed.dev/) editor
- [Rust](https://rustup.rs/) installed via `rustup` (not Homebrew — Zed compiles the tree-sitter grammar from source)

### Install as Dev Extension

1. Build Graphcal: `cargo build --release -p graphcal`
2. Delete the `grammars/` directory if it exists: `rm -rf editors/zed/grammars/`
   (Zed needs to clone the grammar into this directory itself; a leftover
   directory from a previous install will cause a "not a git clone" error.)
3. Open Zed
4. Open the command palette: `Cmd+Shift+P`
5. Run `zed: install dev extension`
6. Select the `editors/zed/` directory
7. Open a `.gcl` file — syntax should be highlighted and diagnostics should appear

### LSP Configuration

The extension looks for `graphcal` on your `PATH` and runs `graphcal lsp`. To use a local build instead, add a `.zed/settings.json` to your project (already included in this repo):

```jsonc
{
  "lsp": {
    "graphcal-lsp": {
      "binary": {
        "path": "/absolute/path/to/target/release/graphcal",
        "arguments": ["lsp"]
      }
    }
  }
}
```

### Testing a Feature Branch

To test grammar changes on a feature branch, update the `rev` field in
`extension.toml` to point to your branch:

```toml
[grammars.graphcal]
repository = "https://github.com/shunichironomura/graphcal"
rev = "my-feature-branch"  # default: "main"
path = "tree-sitter-graphcal"
```

Then delete `editors/zed/grammars/` and re-install the dev extension.
Remember to revert `rev` back to `"main"` before merging.

### After Making Changes

- **Grammar changes** (`tree-sitter-graphcal/grammar.js`): Run `tree-sitter generate` in `tree-sitter-graphcal/`, commit the updated `src/` files, push, delete `editors/zed/grammars/`, then re-run `zed: install dev extension`.
- **Highlighting changes** (`editors/zed/languages/graphcal/highlights.scm`): Re-run `zed: install dev extension`.
- **LSP extension changes** (`editors/zed/src/lib.rs`): Re-run `zed: install dev extension`.

## How It Works

The `extension.toml` references the tree-sitter grammar in the same repository via the `[grammars.graphcal]` section. Zed fetches the grammar source using `git fetch --depth 1 origin <rev>`, compiles `parser.c` with clang (targeting WASM), and loads it for parsing.

### Key Details

- **`rev` must be a branch/tag name, not a raw SHA.** The git fetch protocol does not support fetching by commit SHA. The `rev` field defaults to `main`.
- **Generated `src/` must be committed.** Zed does not run `tree-sitter generate` — it expects `tree-sitter-graphcal/src/parser.c` to already exist in the repository.
- **`grammars/` is a build artifact.** Zed clones the grammar into `editors/zed/grammars/` at install time. This directory is gitignored.
