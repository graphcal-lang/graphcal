# Error Messages and Diagnostics

> How the compiler communicates errors, warnings, and suggestions.

## Status

**Decision level:** Design settled. Error code scheme, severity levels, message format, and implementation crate ([miette](https://docs.rs/miette/latest/miette/)) are chosen. Specific error variants will be defined as each language feature is implemented.

## Summary

Kasuri aims for Rust-quality error messages: specific error codes, source location with labeled spans, context, and actionable suggestions. The diagnostic system is built on the **miette** crate, which extends `std::error::Error` with a `Diagnostic` trait providing structured metadata.

## Why miette

miette was chosen over alternatives (ariadne, codespan-reporting) because:

- **Integrates with Rust's error ecosystem.** `Diagnostic` extends `std::error::Error`, so Kasuri errors work with `?`, `Result`, and standard Rust patterns.
- **Derive macro.** `#[derive(Diagnostic)]` with field-level `#[label]`, `#[source_code]`, and `#[related]` annotations maps directly to the error patterns needed.
- **Multiple output formats.** Graphical (terminal with Unicode/ANSI), narratable (screen reader), JSON (machine-readable) -- all built in.
- **Multi-file errors.** Related diagnostics with different source files support cross-module errors (e.g., import cycles, type mismatches across files).

| Alternative | Why not |
| --- | --- |
| ariadne | Prettier rendering, but no error trait integration, no derive macro, no JSON output |
| codespan-reporting | Unmaintained since ~2021 |
| Custom system | Unnecessary given miette's maturity and Nushell's successful large-scale usage |

## Severity Levels

| Level | miette type | Usage | Symbol |
| --- | --- | --- | --- |
| Error | `Severity::Error` | Compilation failures: type errors, cycles, unknown references | x |
| Warning | `Severity::Warning` | Non-fatal issues: unused nodes, shadowed imports, implicit conversions | ! |
| Advice | `Severity::Advice` | Style suggestions: zero-argument fn (suggest const), overly complex expressions | i |

## Error Code Scheme

Error codes follow the pattern `kasuri::{PREFIX}{NUMBER}`:

| Prefix | Domain | Example codes |
| --- | --- | --- |
| `P0xx` | Parse errors | `kasuri::P001` unexpected token, `kasuri::P002` unterminated string |
| `T0xx` | Type/dimension errors | `kasuri::T001` dimension mismatch, `kasuri::T002` wrong argument type |
| `S0xx` | Space errors | `kasuri::S001` cross-space mixing |
| `G0xx` | Graph errors | `kasuri::G001` cyclic dependency, `kasuri::G002` self-reference |
| `N0xx` | Namespace errors | `kasuri::N001` ambiguous reference, `kasuri::N002` unknown reference |
| `F0xx` | Function errors | `kasuri::F001` `@` in fn body, `kasuri::F002` wrong arity |
| `U0xx` | Unit errors | `kasuri::U001` incompatible conversion, `kasuri::U002` ambiguous unit |
| `X0xx` | Table errors | `kasuri::X001` axis mismatch, `kasuri::X002` missing column |
| `W0xx` | Warnings | `kasuri::W001` unused node, `kasuri::W002` shadowed import |

The `kasuri::` prefix enables linking error codes to documentation URLs in the future.

## Error Format Examples

### Dimension mismatch (T001)

```
  x Dimension mismatch: cannot add Length and Mass
   ,----[fuel_budget.ksr:12:20]
11 | node total = @fuel_mass + @thrust;
   :              ---------    -------
   :              |            `-- Mass
   :              `-- Length
   `----
  help: operands of + must have the same dimension
```

### Unknown reference (N002)

```
  x Unknown graph reference `@transfer`
   ,----[fuel_budget.ksr:7:42]
 7 | node fuel_mass = ... @transfer.total_dv ...
   :                      ---------
   :                      `-- not found
   `----
  help: did you mean @orbit_transfer?
  help: add `use orbit.transfer.{ transfer };`
```

### Graph reference in function body (F001)

```
  x Graph reference not allowed in function body
   ,----[orbital.ksr:15:10]
14 | fn bad(r: Length) -> Velocity {
   :    --- inside this function
15 |     sqrt(@GM_earth / r)
   :          ---------
   :          `-- @ not allowed here
   `----
  help: pass GM_earth as a parameter instead
```

### Cyclic dependency (G001)

```
  x Cyclic dependency: a -> b -> c -> a
   ,----[model.ksr:3:5]
 3 | node a = @b + 1;
   :          --
   :          `-- cycle starts here
   `----
  Related:
   ,----[model.ksr:4:5]
 4 | node b = @c * 2;
   :          --
   :          `-- references @c
   `----
   ,----[model.ksr:5:5]
 5 | node c = @a - 3;
   :          --
   :          `-- references @a (back to start)
   `----
```

## Implementation Architecture

### Error Types per Compilation Phase

Each compilation phase defines its own error enum deriving both `thiserror::Error` and `miette::Diagnostic`:

```rust
// Parse errors
#[derive(Debug, Error, Diagnostic)]
pub enum ParseError {
    #[error("Unexpected token '{found}'")]
    #[diagnostic(code(kasuri::P001), help("expected {expected}"))]
    UnexpectedToken {
        found: String,
        expected: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unexpected")]
        span: SourceSpan,
    },
    // ...
}

// Type/dimension errors
#[derive(Debug, Error, Diagnostic)]
pub enum TypeError {
    #[error("Dimension mismatch: cannot {op} {lhs_dim} and {rhs_dim}")]
    #[diagnostic(code(kasuri::T001))]
    DimensionMismatch {
        op: String,
        lhs_dim: String,
        rhs_dim: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label(primary, "{lhs_dim}")]
        lhs_span: SourceSpan,
        #[label("{rhs_dim}")]
        rhs_span: SourceSpan,
    },
    // ...
}

// Graph structure errors
#[derive(Debug, Error, Diagnostic)]
pub enum GraphError {
    #[error("Cyclic dependency: {cycle_description}")]
    #[diagnostic(code(kasuri::G001))]
    CyclicDependency {
        cycle_description: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("cycle starts here")]
        start_span: SourceSpan,
        #[related]
        links: Vec<CycleLink>,
    },
    // ...
}
```

### Source Code Storage

Source text is stored in `Arc<String>` and wrapped in `NamedSource<Arc<String>>` so that errors are cheap to clone (only cloning the Arc, not the text). This is important because diagnostics are collected via accumulators during incremental evaluation (see [01-computation-model.md](./01-computation-model.md)), and accumulators require `Clone`.

### Span Representation

- **User-facing spans:** miette's byte-offset `SourceSpan` (offset + length) for error rendering. Created on-demand when constructing diagnostics.
- **Internal node identity:** Name-based, not position-based. The computation graph and caching system uses node names, not byte offsets. This keeps the incremental computation's identity model separate from the diagnostic span model.

### Error Recovery

The parser implements **error recovery** to report multiple errors per compilation:

- Syntactic errors produce error nodes in the AST rather than aborting.
- Type checking continues past the first error, collecting all diagnostics.
- All diagnostics are returned at once, not fix-one-recompile-repeat.
- The live view (see [13-live-view.md](./13-live-view.md)) displays all diagnostics simultaneously.

```rust
#[derive(Debug, Error, Diagnostic)]
#[error("Compilation failed with {count} error(s)")]
struct CompilationResult {
    count: usize,
    #[related]
    diagnostics: Vec<KasuriError>,
}
```

### Help Messages

Help messages should be **actionable** -- telling the user what to do, not just what went wrong:

| Error | Help |
| --- | --- |
| `@` in fn body | `help: pass {name} as a parameter instead` |
| Unknown reference `@x` | `help: did you mean @transfer? add 'use orbit.{ transfer };'` |
| Dimension mismatch | `help: operands of {op} must have the same dimension` |
| Space mismatch | `help: use .untagged to strip the space tag, or apply a Transform` |
| Unused node | `help: prefix with _ to suppress this warning, or remove the node` |

Dynamic help (e.g., "did you mean?" suggestions) requires fuzzy matching on available names.

### LSP Mapping

miette's structured diagnostic fields map directly to LSP `Diagnostic`:

| miette | LSP `Diagnostic` |
| --- | --- |
| `severity()` | `severity` |
| `code()` | `code` |
| `Display` message | `message` |
| `labels()` primary span | `range` |
| `labels()` secondary spans | `relatedInformation` |
| `help()` | Appended to `message` or in `relatedInformation` |
| `url()` | `codeDescription.href` |

### Machine-Readable Output

miette's built-in `JSONReportHandler` provides JSON diagnostic output for CI pipelines, editor plugins, and tool integration beyond LSP.

## Resolved Questions

- **Error code system:** Systematic scheme with domain prefixes. See table above.
- **Warning levels:** Three levels: Error, Warning, Advice. Mapped to `miette::Severity`.
- **LSP diagnostics:** Direct mapping from miette fields. See table above.
- **Machine-readable output:** JSON via `JSONReportHandler`, built into miette.

## Open Questions

- **Error recovery strategy:** How aggressive should error recovery be? Recover at statement boundaries (simple) or attempt expression-level recovery (complex but more errors reported)?
- **Warning suppression:** Per-file or per-line suppression (e.g., `// kasuri-allow(W001)`)? Or only project-wide via configuration?
- **Lints:** Beyond errors and warnings, should there be a configurable lint system (like `clippy` for Rust)?
- **Internationalization:** Should error messages be localizable? Defer until there is demand.
- **Suggestions:** How intelligent should "did you mean?" suggestions be? Levenshtein distance? Scope-aware?
- **Error documentation site:** Should each error code have a dedicated documentation page (like Rust's `--explain`)?

## Prior Art

- **miette** ([docs.rs/miette](https://docs.rs/miette/latest/miette/)): The diagnostic crate Kasuri will use.
- **Nushell**: Largest real-world miette user, with hundreds of error variants using per-variant `#[diagnostic]` attributes.
- **Salsa accumulators**: Side-channel error collection pattern that keeps diagnostics separate from return values, preventing them from contaminating memoization.
- Research notes: `.local/2026-02-12_miette-insights.md`, `.local/2026-02-12_miette-research.md`

## Dependencies on Other Aspects

- **All other aspects:** Each language feature produces its own class of errors.
- **Computation Model** ([01](./01-computation-model.md)): Accumulator pattern for error collection during incremental evaluation.
- **Live View** ([13](./13-live-view.md)): Errors displayed inline in the grid.
- **Syntax** ([02](./02-syntax-design.md)): Parse errors depend on grammar design.
