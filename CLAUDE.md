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
- The formal grammar is in `grammar.ebnf` at the repository root. It serves as the source of truth referenced by tree-sitter and TextMate grammars.
- Design ideas and feature proposals are tracked as GitHub issues.
- The detailed discussion file can be found in `.local/` directory. Note that these are raw notes and may contain incomplete or obsolete information.

## Implementation Guidelines

- When you add/modify/remove a feature, please also update the followings accordingly:
  - The test cases in the codebase (unit tests, integration tests, snapshot tests, property-based tests, etc.).
  - The corresponding LSP features in the `crates/graphcal-lsp/` directory (diagnostics, code actions, inlay hints, etc.).
  - The user-facing documentation in `docs/` directory and the `README.md` file in the project root.
  - The tree-sitter grammar in the `graphcal-lang/tree-sitter-graphcal` repository.
  - The Zed extension in the `graphcal-lang/zed-graphcal` repository (syntax highlighting, LSP features, etc.).
  - The VS Code extension in the `graphcal-lang/vscode-graphcal` repository, including the TextMate grammar and LSP features.

## Type Safety: Encode Semantics in Types, Not Conventions

The compiler is the language's first user — its own implementation must hold itself to the same explicitness standard the language enforces on graphcal programs. Distinct semantic concepts must be distinct types; never lean on a string convention, naming pattern, or "everyone knows" rule when a typed alternative is possible.

### Hard rules

- **No flat-string encodings of structured data.** If a value has parts (qualified name, indexed key, scoped binding, …), model it as a struct or enum that names the parts. Do not concatenate fields with a separator (`"::"`, `"."`, `"@"`, `"/"`, …) and rely on later splits/contains to recover them. Concretely, ban patterns like:
  - `format!("{prefix}::{name}")` to fabricate a qualified name.
  - `s.split_once("::")` / `s.contains("::")` / `s.starts_with("@")` to recover structure that should have been preserved typewise.
  - `HashMap<String, …>` or `HashSet<&str>` keyed by an ad-hoc composite string when the components are already typed.
- **No casing-based dispatch.** Don't use `is_upper_snake_case(name)` to decide "is this a const?" or `name.starts_with('_')` to decide visibility. Carry the category (`DeclCategory`, `Visibility`, …) as a typed field on the data.
- **No string-matched control flow on internal identifiers.** A function that branches on `name == "sum"` or `kind == "node"` is missing an enum. Built-in classifications belong in `enum SpecialFnKind { Aggregation(AggregationFn), … }`–style hierarchies whose `parse(&str) -> Option<Self>` is the _only_ place strings cross into the typed core.
- **Stringify only at boundaries.** Rendering for diagnostics, debug output, file/wire serialization, or third-party APIs is fine — but the conversion happens at the boundary, not throughout the functional core. Inside the core, pattern-match on the typed variant.

### Functional core, imperative shell

Treat the compiler as a functional core (parser → AST → IR → TIR → eval plan) with imperative shells at the I/O edges (file loader, LSP server, CLI). The core holds typed values and pure functions over them; the shell handles disk reads, process spawns, network calls, and any necessary serialization. A flat string that exists _only_ because the shell uses one (e.g., `HashMap<DeclName, …>` lookups) is acceptable transitionally, but it is a _boundary_ concern — not a license to spread the convention upstream.

### When you reach for a string

Stop. Ask yourself:

1. Does this string carry structure (multiple fields, a closed set of variants, a parse-able shape)?
2. Will multiple sites need to construct or destructure it the same way?
3. If the convention changed (separator, casing rule, prefix), how many sites would I have to touch?

If any answer is "yes / many", introduce a type. A newtype for opaque identifiers (see `crates/graphcal-compiler/src/syntax/names.rs`'s `define_name_type!` macro), an enum for finite variants, a struct for composites. Place it where the data lives in the layering, not where it's first consumed.

### When the rule conflicts with adjacent code

If you find an existing string convention that violates the rule, prefer fixing it over matching it. If the fix is too large for the current change, fence it into the smallest possible boundary, leave a `// TODO(#NNN):` pointing at a tracking issue, and _do not_ widen the convention. Patterns spread; types contain them.
