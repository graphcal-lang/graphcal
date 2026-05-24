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

## VS Code

The VS Code extension provides syntax highlighting (via TextMate grammar) and full LSP support.

### Installation

1. Clone the extension repository:

    ```bash
    git clone https://github.com/graphcal-lang/vscode-graphcal.git
    cd vscode-graphcal
    ```

2. Build the extension:

    ```bash
    npm install
    npm run build
    ```

3. Install as a dev extension by symlinking into your VS Code extensions directory:

    ```bash
    ln -s "$(pwd)" ~/.vscode/extensions/graphcal
    ```

4. Restart VS Code.

### Configuration

| Setting | Default | Description |
|---------|---------|-------------|
| `graphcal.lsp.enabled` | `true` | Enable/disable the LSP server |
| `graphcal.lsp.path` | `"graphcal"` | Path to the `graphcal` binary |

If `graphcal` is not on your `PATH`, set `graphcal.lsp.path` to the full path of the binary.

## Zed

The Zed extension provides syntax highlighting (via tree-sitter grammar) and LSP support.

### Setup

1. Clone the extension repository: `https://github.com/graphcal-lang/zed-graphcal`
2. Open the command palette in Zed
3. Select **"Extensions: Install Dev Extension"**
4. Navigate to the cloned `zed-graphcal` directory
5. The extension will be installed and activated

## Neovim / Helix

For Neovim, Helix, and other editors that support tree-sitter:

### Tree-Sitter Grammar

The tree-sitter grammar is in [`graphcal-lang/tree-sitter-graphcal`](https://github.com/graphcal-lang/tree-sitter-graphcal). Install it according to your editor's tree-sitter plugin instructions.

For Neovim with `nvim-treesitter`:

1. Add `https://github.com/graphcal-lang/tree-sitter-graphcal` as the grammar source in your tree-sitter config
2. Copy the highlight queries from the repository's `queries/highlights.scm`

### LSP Configuration

For any editor with LSP support, configure it to run `graphcal lsp` over stdin/stdout for `.gcl` files.

Example Neovim configuration (with `nvim-lspconfig`):

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
