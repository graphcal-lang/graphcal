---
icon: material/puzzle
---

# Editor Setup

Graphcal provides editor extensions with rich language support through the built-in LSP server. A key feature is **inlay hints that display computed values inline**, turning your editor into a live calculation sheet.

## LSP Features

The Graphcal LSP server (`graphcal lsp`) provides:

| Feature              | Description                                                                                              |
| -------------------- | -------------------------------------------------------------------------------------------------------- |
| **Inlay hints**      | Computed param and node values displayed inline next to each declaration                                 |
| **Diagnostics**      | Real-time error reporting: parse errors, dimension mismatches, unknown references, visibility violations |
| **Code actions**     | Quick fixes for common errors (e.g., "Add `pub`" for visibility violations)                              |
| **Go to definition** | Jump from a reference to its declaration                                                                 |
| **Hover**            | Show type, dimension, and unit information                                                               |
| **Find references**  | Locate all usages of a declaration                                                                       |
| **Document symbols** | Outline view of all declarations in the file                                                             |
| **Formatting**       | Format the current document (same as `graphcal format`)                                                  |
| **Document links**   | Clickable links for `import` paths                                                                       |

Diagnostics distinguish precise source ranges, whole-file failures, and
source-less built-in/internal failures. A source-less failure is never rendered
as a misleading caret at the first byte of the file.

!!! tip "Inlay hints: live calculation view"
The inlay hints feature is what makes Graphcal feel like a live spreadsheet. As you edit your `.gcl` file, the LSP evaluates the computation graph and shows the resulting values next to each `param` and `node` declaration. Change an input and watch all dependent values update.

For multi-file projects, editor navigation follows module-qualified identity for
same-leaf declarations. If `a.gcl` and `b.gcl` both export `Phase`, `Item`, and
`Pick`, go-to-definition on `a.Phase`, `a.Phase.Burn`, `a.Item`, or `a.Pick(...)`
jumps to `a.gcl`, not whichever same-leaf symbol was seen first. Navigation and
rename also preserve Graphcal namespaces: a term and a unit may share a spelling
without being merged. Selective imports retain every authored alias while all
aliases still navigate to the one canonical declaration. References and rename
use one canonical index across every source in the active loaded project,
including re-exports and includes. A canonical API rename changes unaliased
imports and uses while preserving authored aliases. The server refuses rename
when an occurrence is ambiguous, an affected open snapshot is stale, or an
exported definition may have reverse importers outside the loaded project.

Analysis is dependency- and revision-aware and bounded. Each result records the
exact open-buffer revisions it consumed. Editing, saving, opening, closing, or
reverting an imported file invalidates and reanalyzes every transitive open
importer, so diagnostics, navigation, completion, and evaluated inlay hints stay
project-coherent without touching the importer. A new edit cancels superseded
loading, compilation, and evaluation work; only a result whose full dependency
snapshot is still current can publish. If an analysis exceeds the server deadline, the
LSP keeps the last completed result available rather than replacing source
diagnostics with a server-timeout error. If analysis must fall back to an empty
symbol table or module resolver, the server emits a warning through the LSP
client log (`window/logMessage`) so unavailable navigation or completion can be
diagnosed instead of appearing indistinguishable from an empty result.

## VS Code

The VS Code extension provides syntax highlighting (via TextMate grammar) and full LSP support.

### Installation

Install the published [Graphcal extension](https://marketplace.visualstudio.com/items?itemName=Graphcal.graphcal) from the Visual Studio Marketplace.

You can also install it from VS Code:

1. Open the Extensions view.
2. Search for **Graphcal**.
3. Install the extension published by **Graphcal**.

### Configuration

| Setting                | Default      | Description                   |
| ---------------------- | ------------ | ----------------------------- |
| `graphcal.lsp.enabled` | `true`       | Enable/disable the LSP server |
| `graphcal.lsp.path`    | `"graphcal"` | Path to the `graphcal` binary |

If `graphcal` is not on your `PATH`, set `graphcal.lsp.path` to the full path of the binary.

## Zed

The Zed extension provides syntax highlighting (via tree-sitter grammar) and LSP support.

### Setup

The Zed extension is not yet published. Install it as a dev extension for now:

1. Clone the extension repository: `https://github.com/graphcal-lang/zed-graphcal`
2. Open the command palette in Zed
3. Select **"Extensions: Install Dev Extension"**
4. Navigate to the cloned `zed-graphcal` directory
5. The extension will be installed and activated

## Helix

Helix can use the Graphcal tree-sitter grammar and the built-in LSP server.

Add the following to `~/.config/helix/languages.toml`:

```toml
[[grammar]]
name = "graphcal"
source = { git = "https://github.com/graphcal-lang/tree-sitter-graphcal", rev = "main" }

[language-server.graphcal-lsp]
command = "graphcal"
args = ["lsp"]

[[language]]
name = "graphcal"
scope = "source.graphcal"
file-types = ["gcl"]
roots = ["graphcal.toml"]
comment-token = "//"
language-servers = ["graphcal-lsp"]
indent = { tab-width = 2, unit = "  " }
auto-format = true
```

Then fetch and build the grammar:

```sh
hx --grammar fetch
hx --grammar build
```

`graphcal` must be available on your `PATH`; otherwise, set `command` to the full path of the Graphcal binary.

### Helix syntax highlighting queries

The commands above install/update the parser binary, but Helix does **not** automatically install or refresh highlight query files from custom grammar repositories. Graphcal highlighting is loaded separately from:

```text
~/.config/helix/runtime/queries/graphcal/highlights.scm
```

Install the highlight query after fetching the grammar:

```sh
mkdir -p ~/.config/helix/runtime/queries/graphcal
cp ~/.config/helix/runtime/grammars/sources/graphcal/queries/highlights.scm \
  ~/.config/helix/runtime/queries/graphcal/highlights.scm
```

If you prefer not to rely on Helix's grammar source cache, download the query directly instead:

```sh
mkdir -p ~/.config/helix/runtime/queries/graphcal
curl -fsSL \
  https://raw.githubusercontent.com/graphcal-lang/tree-sitter-graphcal/main/queries/highlights.scm \
  -o ~/.config/helix/runtime/queries/graphcal/highlights.scm
```

## Neovim

For Neovim with `nvim-treesitter`:

1. Add `https://github.com/graphcal-lang/tree-sitter-graphcal` as the grammar source in your tree-sitter config
2. Copy the highlight queries from the repository's `queries/highlights.scm`

For LSP support, configure Neovim to run `graphcal lsp` over stdin/stdout for `.gcl` files.

Example configuration with `nvim-lspconfig`:

```lua
vim.api.nvim_create_autocmd("FileType", {
  pattern = "graphcal",
  callback = function()
    vim.lsp.start({
      name = "graphcal",
      cmd = { "graphcal", "lsp" },
    })
  end,
})
```

## Other editors

For any editor with tree-sitter and LSP support, install the grammar from [`graphcal-lang/tree-sitter-graphcal`](https://github.com/graphcal-lang/tree-sitter-graphcal) and configure the language server to run `graphcal lsp` for `.gcl` files.
