# Instruction for Coding Agents

## Project Overview

- The goal of this project is to create type-safe, unit-aware, Git-friendly reactive programming language for engineering calculations.
- Because we target engineering projects as one of the use cases, we prioritize safety over usability. We prefer explicitness over implicitness.
  - e.g., no implicit type/unit conversion, no implicit type inference, no implicit null propagation, etc.
  - Remember Mars Climate Orbiter failure due to unit mismatch.
- This project is not yet published, so breaking changes are acceptable for simpler/clever/clean design and implementation.
  - DO NOT implement workarounds for backward compatibility. If a major design change, such as data model modification, is needed, just make the change and update the existing codebase accordingly.

## Documentation

- The user-facing documentation is in the `docs/` directory. The `docs/index.md` is the main entry point for users.
  - It is a Zensical site, so you can run it locally with `zensical serve` in the project root and open `http://localhost:8000` in the browser.
- The language design documents are in the `design/` directory. The `design/README.md` has links to all the design docs.
  - Some ideas on features are documented in `.design/IDEAS.md`.
  - The implementation phases are in `design/phases/` directory and summarized in `design/phases/README.md`.
  - `design/codebase-reading-guide.md` has information for new contributors to understand the codebase.
- The detailed discussion file can be found in `.local/` directory. Note that these are raw notes and may contain incomplete or obsolete information.

## Implementation Guidelines

- When you add/modify/remove a feature, please also update the followings accordingly:
  - The test cases in the codebase (unit tests, integration tests, snapshot tests, property-based tests, etc.).
  - The corresponding LSP features in the `crates/graphcal-lsp/` directory (diagnostics, code actions, inlay hints, etc.).
  - The user-facing documentation in `docs/` directory and the `README.md` file in the project root.
  - The tree-sitter grammar in `tree-sitter-graphcal/` directory.
  - The Zed extension in `editors/zed/` directory (syntax highlighting, LSP features, etc.).
  - The VS Code extension in `editors/vscode/` directory, including the TextMate grammar and LSP features.
