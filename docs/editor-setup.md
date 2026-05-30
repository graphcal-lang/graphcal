---
icon: material/puzzle
---

# Editor Setup

Graphcal provides editor extensions with rich language support through the built-in LSP server. A key feature is **inlay hints that display computed values inline**, turning your editor into a live calculation sheet.

## LSP Features

The Graphcal LSP server (`graphcal lsp`) provides:

| Feature | Description |
|---------|-------------|
| **Inlay hints** | Computed param and node values displayed inline next to each declaration |
| **Diagnostics** | Real-time error reporting: parse errors, dimension mismatches, unknown references, visibility violations |
| **Code actions** | Quick fixes for common errors (e.g., "Add `pub`" for visibility violations) |
| **Go to definition** | Jump from a reference to its declaration |
| **Hover** | Show type, dimension, and unit information |
| **Find references** | Locate all usages of a declaration |
| **Document symbols** | Outline view of all declarations in the file |
| **Formatting** | Format the current document (same as `graphcal format`) |
| **Document links** | Clickable links for `import` paths |

!!! tip "Inlay hints: live calculation view"
    The inlay hints feature is what makes Graphcal feel like a live spreadsheet. As you edit your `.gcl` file, the LSP evaluates the computation graph and shows the resulting values next to each `param` and `node` declaration. Change an input and watch all dependent values update.

For multi-file projects, editor navigation follows module-qualified identity for
same-leaf declarations. If `a.gcl` and `b.gcl` both export `Phase`, `Item`, and
`Pick`, go-to-definition on `a.Phase`, `a.Phase.Burn`, `a.Item`, or `a.Pick(...)`
jumps to `a.gcl`, not whichever same-leaf symbol was seen first.

## VS Code

The VS Code extension provides syntax highlighting (via TextMate grammar) and full LSP support.

### Installation

Install the published [Graphcal extension](https://marketplace.visualstudio.com/items?itemName=Graphcal.graphcal) from the Visual Studio Marketplace.

You can also install it from VS Code:

1. Open the Extensions view.
2. Search for **Graphcal**.
3. Install the extension published by **Graphcal**.

### Configuration

| Setting | Default | Description |
|---------|---------|-------------|
| `graphcal.lsp.enabled` | `true` | Enable/disable the LSP server |
| `graphcal.lsp.path` | `"graphcal"` | Path to the `graphcal` binary |

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

Restart Helix and verify the setup:

```sh
hx --health graphcal
```

`graphcal` must be available on your `PATH`; otherwise, set `command` to the full path of the Graphcal binary.

The configuration above is enough for file detection, grammar installation, formatting, and LSP features. Helix does not install query files from custom grammar repositories automatically; if `hx --health graphcal` reports `Highlight queries: ✘`, syntax highlighting is not enabled yet.

To enable syntax highlighting, optionally install the highlight query from the tree-sitter grammar repository into Helix's runtime directory. Copying the query file is recommended over symlinking into Helix's grammar source cache, because `hx --grammar fetch` may replace that cache and leave the symlink broken.

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
