# `graphcal format` — Formatter Design Plan

## Motivation

A canonical formatter for `.gcl` files ensures consistent style across projects,
eliminates style debates, and enables format-on-save in the LSP. This document
describes the architecture, implementation phases, and key design decisions.

## Prior Art: Detailed Comparison

### Summary Table

| | **gofmt** | **rustfmt** | **ruff** | **gleam format** |
|---|---|---|---|---|
| **Language** | Go | Rust | Python | Gleam |
| **Tree repr.** | AST + position info + comment list | AST (rustc's libsyntax) | CST (rowan green/red trees) | AST + extracted comment list |
| **Comment model** | Separate comment stream, merged by position | Span-gap recovery ("missed spans") | Trivia attached to CST nodes (leading/trailing/dangling) | Extracted during parse, reattached by position |
| **Pretty-printer** | Custom (token stream → tabwriter) | Custom heuristic rewrite per construct | Prettier-style IR (Wadler-Lindig) | `pretty` crate (Wadler-Lindig) |
| **Line breaking** | Does **not** break long lines | Heuristic budget-based, can bail out | Algorithmic (`group`/`indent`) | Algorithmic (`group`/`nest`) |
| **Configuration** | Zero | Extensive (~100 options) | Minimal (4 options) | Zero |
| **Complexity** | Low–medium | Very high | High (but well-structured) | Low |

### 1. gofmt (Go)

**Architecture**: AST with position info → token stream → merge with comment
stream → tabwriter → output.

gofmt parses Go source into an AST where every node carries position
information (`token.Pos`). Comments are stored as a flat list
(`[]*ast.CommentGroup`) attached to the file node, separate from the AST.
The printer walks the AST emitting tokens, and merges in comments by comparing
byte positions. A final pass through Go's `text/tabwriter` (elastic tabstops)
handles column alignment.

**Pros**:

- **Simplicity**: Deliberately avoids the "hard problem" of automatic line
  breaking. It adjusts indentation and alignment but respects the author's
  line break decisions. This makes the algorithm dramatically simpler.
- **Zero configuration**: One canonical style, no debates. This cultural
  decision is widely considered gofmt's greatest success.
- **Elastic tabstops**: The tabwriter pass elegantly handles column alignment
  in `var` blocks, struct fields, etc. without custom alignment logic.
- **Enables refactoring tools**: Because input/output are both canonical,
  tools like `goimports`, `gofix`, and `gofmt -r` can transform code
  mechanically. Diffs show only semantic changes.
- **Robust**: Always produces output for valid Go code. No bail-out.
- **Fast**: Integer-based position tracking, single-pass printing.
- **Battle-tested**: Shipped with Go since 2009, used on all Go code at Google.

**Cons**:

- **No automatic line breaking**: Accepts arbitrarily long lines. The user
  must manually break lines; gofmt only adjusts indentation afterward. This
  is fine for Go (which has a culture of short lines) but may be problematic
  for graphcal where engineering expressions with unit annotations can be long.
- **Comment placement is fragile**: Comments are a flat list, not attached to
  AST nodes. The Go team has called this the "single biggest mistake" in the
  AST design (golang/go#20744). It makes AST manipulation tools (refactoring,
  code generation) extremely difficult to write correctly because moving a node
  can orphan or misplace its comments.
- **No column-width enforcement**: Since gofmt doesn't break lines, there's no
  way to enforce a maximum line width.
- **Not a pretty-printer**: The approach is more "whitespace fixer" than
  "pretty-printer". It cannot restructure expressions or reflow argument lists.

**Relevance to graphcal**: The zero-configuration philosophy is a strong match.
The elastic tabstops idea could be useful for aligning declaration blocks. But
the lack of automatic line breaking is a significant drawback — graphcal
expressions like `sqrt(2.0 * GM_EARTH * r2 / (r1 * (r1 + r2))) - v1` with
unit annotations can easily exceed any reasonable line width, so the formatter
should be able to break them intelligently.

### 2. rustfmt (Rust)

**Architecture**: AST (rustc internals) → per-construct heuristic rewrite →
string concatenation → output.

rustfmt parses Rust source using rustc's internal `libsyntax` parser, producing
an AST. It then walks the AST with a `FmtVisitor`, and each node type
implements a `Rewrite` trait that returns `Option<String>` — the formatted
output for that node. At each level, a "width budget" is calculated (remaining
characters on the line), and the rewrite function tries to fit the node within
that budget. If it can't, it returns `None`, and the caller tries a different
layout strategy or falls back to the original source.

**Pros**:

- **Highest output quality**: Designed for "impeccable" output that developers
  would be comfortable reading all day. Per-construct heuristics allow
  fine-tuned, Rust-idiomatic formatting.
- **Rich configuration**: ~100 options allow teams to customize style. This is
  valuable for a language with a large, opinionated community.
- **AST-level transformations**: Can restructure code beyond just whitespace
  (e.g., reordering imports, normalizing use statements).
- **Established standard**: Used by virtually all Rust projects. RFC 2437
  provides stability guarantees.

**Cons**:

- **Extreme complexity**: Custom heuristic logic for every syntax construct,
  with complex interactions between rules. The codebase is large and hard to
  maintain. The developers "attempted to specify a complexity metric for
  formatting decisions, but these attempts got complex very quickly."
- **Comment preservation is brittle**: Since the AST discards comments, rustfmt
  uses a "missed spans" recovery system to output unformatted source between
  formatted regions. This leads to many edge-case bugs: comments between
  keywords, comments in method chains, comments inside macro invocations.
  After years of development, comment handling remains a source of issues.
- **Bail-out behavior**: When a construct can't be formatted within the width
  budget, `Rewrite::rewrite()` returns `None`, and the node may be left
  unformatted. This can produce 1000+ character lines, especially in generated
  code. This problem motivated the creation of `prettyplease` as an alternative.
- **Idempotency bugs**: Despite idempotency being a "core tenet", multiple bugs
  have been filed where `rustfmt(rustfmt(x)) != rustfmt(x)`, often involving
  interactions between comments and formatting rules.
- **Coupled to rustc internals**: Depends on `rustc-ap-syntax` which has
  frequent breaking changes. Cannot build with stable Rust. Not regularly
  published to crates.io. This makes it nearly impossible to use as a library.
- **String allocation performance**: "Rustfmt spends a lot of time concatenating
  strings, involving many allocations, memcpys and deallocations." The
  per-construct approach generates many intermediate `String` allocations that
  are concatenated together.
- **Macro formatting is fundamentally limited**: Macros are opaque token trees
  before expansion. Rustfmt can only format macro calls whose arguments happen
  to parse as valid Rust expressions. Open issue since 2015.

**Relevance to graphcal**: The per-construct heuristic approach is a cautionary
tale. It produces beautiful output but at enormous implementation and maintenance
cost. For a small language like graphcal, this complexity is not justified. The
bail-out problem is also relevant: we want the formatter to always succeed.
However, the lesson that AST-based formatting makes comment preservation very
hard is directly applicable to graphcal's current architecture.

**Note — prettyplease**: dtolnay's `prettyplease` crate was created as a
reaction to rustfmt's complexity. It uses Oppen's 1979 pretty-printing
algorithm, targets "95% of rustfmt's quality", has only 3 dependencies, builds
on stable Rust, runs at 60 MB/s (vs rustfmt's 2.8 MB/s), and never bails out.
It demonstrates that a simpler algorithmic approach can be "good enough" for
most use cases.

### 3. ruff (Python)

**Architecture**: CST (rowan green/red trees) → Prettier-style Document IR →
width-aware printer → output.

ruff uses a hand-written recursive descent parser that produces a CST using the
`rowan` library (originally from rust-analyzer). The CST preserves all trivia
(whitespace, comments) attached to tree nodes. The formatter then traverses the
CST and emits a Prettier-style Document IR with primitives like `group()`,
`indent()`, `hard_line_break()`, `soft_line_break_or_space()`, `if_group_breaks()`,
and `line_suffix()`. A greedy single-pass printer resolves the IR into final
text, making line-breaking decisions based on remaining width.

**Pros**:

- **Principled architecture**: Clean separation between CST → IR → output.
  The IR encodes *how* to format in both flat and broken modes; the printer
  makes final layout decisions. This is the most well-structured approach.
- **Excellent comment handling**: Trivia is attached directly to CST nodes,
  categorized as leading, trailing, or dangling. Comments are never "lost"
  because the CST faithfully represents the entire source. The `line_suffix()`
  primitive elegantly handles trailing comments.
- **Algorithmic line breaking**: The Wadler-Lindig algorithm with `group()`
  semantics makes principled, predictable line-breaking decisions. No
  hand-tuned heuristics per construct.
- **Conditional formatting**: `if_group_breaks()` enables trailing commas only
  in multi-line layouts, and similar conditional formatting that depends on
  whether a group was broken.
- **Forked from Rome/Biome**: Inherited a battle-tested, language-agnostic
  formatter core. The IR primitives work for any language.
- **Extremely fast**: Rust implementation, efficient CST representation.

**Cons**:

- **Requires a CST**: The most significant drawback for graphcal. Building a
  CST from scratch (or migrating from an AST parser) is a large undertaking.
  The `rowan` library provides the infrastructure, but integrating it with an
  existing `logos`-based lexer requires significant refactoring.
- **High initial investment**: The Prettier-style IR, while elegant, requires
  implementing a full set of primitives and a printer algorithm. This is more
  upfront work than simpler approaches.
- **Complexity of the IR**: Understanding how `group()`, `indent()`,
  `if_group_breaks()`, and `line_suffix()` interact requires significant
  study. Debugging why a particular layout decision was made can be difficult.
- **Over-engineered for a small language**: Ruff formats Python, a language
  with complex syntax and millions of users. The full Prettier-style IR may
  be more machinery than graphcal needs.
- **Dangling comment heuristics**: Even with CST trivia, deciding where
  "dangling" comments belong (e.g., comments inside empty braces) requires
  heuristic rules that can be tricky.

**Relevance to graphcal**: The Prettier-style IR is the gold standard for
formatter architecture, but it comes with high upfront cost. The CST
requirement is a major barrier given graphcal's current AST-based parser. If
graphcal's syntax grows significantly, migrating to this architecture may be
worthwhile in the future. For now, the key insights to borrow are: the
`group()`/`indent()`/line-break primitive vocabulary, and the
leading/trailing/dangling comment categorization.

### 4. gleam format (Gleam)

**Architecture**: AST + extracted comment list → `pretty` crate Doc IR →
Wadler-Lindig printer → output.

Gleam's formatter parses source into an AST (like graphcal does today), but
**extracts comments with their byte positions during parsing** into a separate
`Vec`. The formatter then walks the AST and builds a `Doc` tree using the
`pretty` crate's Wadler-Lindig combinators (`text`, `line`, `nest`, `group`,
`concat`, `break_`). As it traverses the AST, it "pops" comments from the
comment list by comparing positions, interleaving them into the Doc tree.

**Pros**:

- **Simplest architecture that preserves comments**: No CST needed. The comment
  extraction is a lightweight pre-pass, and the existing AST parser is reused
  unchanged. This is the lowest-friction path for a language with an existing
  AST-based parser — exactly graphcal's situation.
- **Battle-tested pretty-printer**: The `pretty` crate implements Lindig's
  "Strictly Pretty" (2000), a strict-language adaptation of Wadler's algorithm.
  It handles `group()`-based line breaking correctly and efficiently.
- **Low complexity**: Gleam's `format.rs` is a single file (~3000 lines) that
  covers the entire language. The logic is straightforward: for each AST node
  type, emit the corresponding Doc primitives.
- **Zero configuration**: Like gofmt, one canonical style. No options.
- **Automatic line breaking**: Unlike gofmt, Gleam's formatter will break long
  lines using the `group()`/`nest()` mechanism. This is important for graphcal.
- **Proven at scale**: Gleam has a growing community and the formatter is used
  by all Gleam projects. It handles real-world code reliably.
- **Minimal dependencies**: `pretty` crate has only 3 core dependencies.

**Cons**:

- **Position-based comment reattachment has edge cases**: When AST
  transformations reorder nodes, comments can end up in unexpected places.
  However, since graphcal's formatter won't reorder declarations, this is
  unlikely to be a problem.
- **`pretty` crate limitations**: The standard `group()` combinator only
  supports "all-or-nothing" breaking (either the entire group fits on one
  line, or it breaks). It doesn't support Prettier's `if_group_breaks()` or
  conditional trailing commas natively. Workarounds exist (e.g., always
  emitting trailing commas in multi-line lists).
- **No conditional formatting**: Unlike ruff's `if_group_breaks()`, there's no
  built-in way to emit different content depending on whether a group broke.
  This means trailing commas must be either always-on or always-off.
- **Comment "popping" requires careful ordering**: The formatter must process
  AST nodes in source order to correctly interleave comments. If the traversal
  visits nodes out of order, comments can be misplaced.
- **Less sophisticated than ruff**: The `pretty` crate's `group()` is less
  expressive than Prettier's full primitive set. Complex layouts (e.g., method
  chains, deeply nested expressions) may not format as well.
- **Gleam team notes limitations**: They report being "very happy with it,
  though there were a few places where the format supported was not exactly
  what they wanted."

**Relevance to graphcal**: This is the closest match to graphcal's situation.
Both are small-to-medium languages with AST-based parsers, small teams, and a
preference for simplicity. The `pretty` crate provides algorithmic line
breaking without the complexity of a full Prettier IR. The comment extraction
approach avoids a CST refactor. The main risk is that `pretty` crate's
`group()` limitations may require workarounds for some graphcal constructs, but
this is manageable for a language of graphcal's size.

### Approach Comparison for graphcal

| Criterion | gofmt | rustfmt | ruff | gleam format |
|-----------|-------|---------|------|--------------|
| **Parser change needed?** | Medium (add positions + comment list) | Major (need rustc AST) | Major (need CST) | **Minimal** (add comment extraction) |
| **Automatic line breaking?** | No | Yes (heuristic) | Yes (algorithmic) | **Yes (algorithmic)** |
| **Comment preservation?** | Yes (but fragile) | Yes (but buggy) | **Excellent** | **Good** |
| **Implementation effort** | Low | Very high | High | **Low** |
| **Maintenance burden** | Low | Very high | Medium | **Low** |
| **Output quality** | Good (no line breaking) | Excellent | Excellent | Good–Very Good |
| **Risk of bail-out/crash** | None | Moderate | None | **None** |
| **Fit for graphcal** | Poor (no line breaking) | Poor (overkill) | Poor (needs CST) | **Excellent** |

### Recommendation

Follow the **Gleam model**: AST + extracted comments + `pretty` crate.

**Rationale**:

1. **Minimal parser changes**: Only a comment extraction pre-scan is needed.
   The existing `logos` lexer and recursive descent parser remain unchanged.
2. **Algorithmic line breaking**: The `pretty` crate's Wadler-Lindig algorithm
   handles `group()`-based line breaking, which is essential for graphcal's
   long engineering expressions.
3. **Good-enough comment preservation**: Position-based comment reattachment
   covers the common cases (leading, trailing, standalone comments) without
   requiring a CST.
4. **Low implementation and maintenance cost**: Gleam's formatter is ~3000
   lines for a language larger than graphcal. Graphcal's formatter should be
   smaller.
5. **Proven approach**: Gleam has validated this architecture in production.

**Future upgrade path**: If graphcal's syntax grows and the `pretty` crate's
limitations become constraining, the formatter can be migrated to a
Prettier-style IR (ruff's approach) later. The AST → Doc traversal logic would
be largely reusable; only the Doc type and printer would change.

### Pretty-Printer Library Choice

| Library | Type | Maintained | Best for |
|---------|------|------------|----------|
| **`pretty`** (Marwes/pretty.rs) | General Wadler-Lindig | Stable | General DSL formatting |
| **`prettyless`** (typstyle-rs) | Enhanced fork of `pretty` | Active (2025) | Enhanced layout needs |
| **`prettyplease`** (dtolnay) | Rust-specific (syn) | Active | Generated Rust code only |
| **Roll your own** | Custom | You | Full control, zero deps |

**Recommendation**: Start with `pretty`. It is battle-tested (used by Gleam),
general-purpose, and has a stable API. If limitations surface later,
`prettyless` (an enhanced fork with improved layout logic) is a drop-in
upgrade. Rolling our own is only justified if dependency minimization is
critical, which it isn't for a CLI/LSP tool.

## Architecture Overview

```text
Source (.gcl)
    │
    ▼
┌───────────────────┐
│  Comment-Aware     │  Phase 1: Extract comments with positions
│  Lexer             │  (modify existing logos lexer)
└─────────┬─────────┘
          │ tokens + Vec<Comment>
          ▼
┌───────────────────┐
│  Parser            │  Existing parser, unchanged
│  (AST + Spans)     │
└─────────┬─────────┘
          │ AST + Vec<Comment>
          ▼
┌───────────────────┐
│  AST → Doc IR      │  Phase 2: Traverse AST, emit Doc primitives
│  (format module)   │  Interleave comments by position
└─────────┬─────────┘
          │ Doc (pretty crate IR)
          ▼
┌───────────────────┐
│  pretty::Printer   │  Phase 3: Render Doc to string
│  (Wadler-Lindig)   │  Handles line-width fitting
└─────────┬─────────┘
          │
          ▼
    Formatted .gcl
```

## Phase 0: Foundation — Comment Extraction in Lexer

### Problem

The current lexer (`logos`) skips whitespace and comments entirely:

```rust
#[logos(skip r"[ \t\r\n]+")]
#[logos(skip(r"//[^\n]*", allow_greedy = true))]
```

This means comments are **permanently lost** during lexing, making comment-preserving
formatting impossible.

### Solution

Add a **comment-collecting layer** on top of the existing lexer.
Two approaches:

**Option A — Pre-scan (recommended)**:
Before lexing, scan the source with a simple regex to extract all comments
and their byte positions into a `Vec<Comment>`. The lexer continues to skip
comments as before. The formatter uses the comment list + AST spans to
reattach them.

```rust
pub struct Comment {
    pub kind: CommentKind,
    pub text: String,     // including the `//` or `///` prefix
    pub span: Span,       // byte offset + length
}

pub enum CommentKind {
    Line,     // `// ...`
    Doc,      // `/// ...`
    Module,   // `//// ...` (if we add this later)
}

/// Extract all comments from source text before parsing.
pub fn extract_comments(source: &str) -> Vec<Comment> { ... }
```

**Option B — Modify lexer to emit comment tokens**:
Add `Comment` and `DocComment` variants to `Token`, stop skipping them in logos,
and have the `Lexer` wrapper filter them out while collecting them. This requires
parser changes to handle the filtered stream.

**Decision**: Option A is simpler — it doesn't touch the parser at all and keeps
the existing lexer/parser pipeline intact.

### Blank Line Preservation

In addition to comments, the formatter should preserve **intentional blank lines**
between declarations (collapsed to at most one blank line, similar to `gofmt` and
`rustfmt`). The pre-scan can also record blank-line positions.

```rust
pub struct SourceMetadata {
    pub comments: Vec<Comment>,
    /// Byte offsets where 2+ consecutive newlines occur (blank line separators)
    pub blank_lines: Vec<usize>,
}
```

## Phase 1: New Crate & CLI Subcommand

### Crate: `graphcal-fmt`

Create a new crate `crates/graphcal-fmt/` with:

```text
crates/graphcal-fmt/
├── Cargo.toml
└── src/
    ├── lib.rs          # Public API: format_source(source: &str) -> String
    ├── comments.rs     # Comment extraction (extract_comments)
    ├── format.rs       # AST → Doc IR traversal
    └── doc_ext.rs      # Helper extensions for the pretty crate
```

Dependencies:

- `graphcal-syntax` (for AST, parser, Span)
- `pretty` crate (Wadler-Lindig pretty printer)
- `regex` (for comment extraction pre-scan, or a hand-written scanner)

### CLI: `graphcal format`

Add a `Format` variant to the `Commands` enum in `graphcal-cli`:

```rust
/// Format .gcl files
Format {
    /// Files or directories to format (default: current directory)
    paths: Vec<PathBuf>,
    /// Check formatting without modifying files (exit 1 if unformatted)
    #[arg(long)]
    check: bool,
    /// Print diff instead of modifying files
    #[arg(long)]
    diff: bool,
    /// Read from stdin and write to stdout
    #[arg(long)]
    stdin: bool,
}
```

Behavior:

- `graphcal format` — format all `.gcl` files under `.` recursively, in-place
- `graphcal format src/` — format all `.gcl` files under `src/`
- `graphcal format file.gcl` — format a single file
- `graphcal format --check` — exit 1 if any file would change (CI mode)
- `graphcal format --diff` — print unified diff of changes
- `graphcal format --stdin` — read from stdin, write formatted output to stdout

### LSP Integration

Add `textDocument/formatting` capability to the LSP server, delegating to
`graphcal_fmt::format_source()`. This enables format-on-save in editors.

## Phase 2: Formatting Rules (AST → Doc IR)

### Pretty-Printer Primitives

Using the `pretty` crate's `Doc` type (Wadler-Lindig algorithm):

| Primitive | Meaning |
|-----------|---------|
| `Doc::text(s)` | Literal text |
| `Doc::nil()` | Empty |
| `Doc::hardline()` | Always break |
| `Doc::softline()` | Break if group doesn't fit, else space |
| `Doc::softline_()` | Break if group doesn't fit, else nothing |
| `Doc::group(d)` | Try to fit `d` on one line; break if > width |
| `Doc::nest(i, d)` | Indent `d` by `i` spaces when broken |
| `Doc::concat(ds)` | Concatenate documents |

### Formatting Rules by Construct

#### Top-Level Declarations

- One blank line between declarations (or preserve original blank lines,
  collapsed to at most one)
- No trailing blank line at end of file, one trailing newline

#### `dimension` Declarations

```gcl
// Single-line
dimension Velocity = Length / Time;

// Base dimension
dimension Length;
```

#### `unit` Declarations

```gcl
unit km: Length = 1000 m;
unit kPa: Pressure = 1000 kg / m * s^2;
```

#### `const` / `param` / `node` Declarations

Short form (expression fits on one line):

```gcl
const R_EARTH: Length = 6371.0 km;
param parking_alt: Length = 200.0 km;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

Block form (expression is a block):

```gcl
node transfer: TransferResult = {
    let r1 = R_EARTH + @parking_alt;
    let r2 = R_EARTH + @target_alt;
    TransferResult { dv1, dv2, total_dv: dv1 + dv2 }
};
```

Rule: If the value expression is a `Block`, always use multi-line form with
4-space indentation inside braces.

#### `type` Declarations

Single-variant (struct sugar):

```gcl
type TransferResult {
    dv1: Velocity,
    dv2: Velocity,
    total_dv: Velocity,
}
```

Multi-variant (tagged union):

```gcl
type ManeuverKind {
    Impulsive { delta_v: Velocity }
    LowThrust { thrust: Force, duration: Time }
}
```

Empty marker type:

```gcl
type ECI {}
```

#### `fn` Declarations

Short form:

```gcl
fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;
```

Block form:

```gcl
fn hohmann_dv(gm: GravParam, r1: Length, r2: Length) -> TransferResult {
    let v1 = sqrt(gm / r1);
    let v2 = sqrt(gm / r2);
    TransferResult { dv1, dv2, total_dv: dv1 + dv2 }
}
```

Long parameter list (break after each param if doesn't fit):

```gcl
fn long_name<D: Dim>(
    param_one: D,
    param_two: D,
    param_three: Dimensionless,
) -> D = param_one + param_two * param_three;
```

#### `index` Declarations

```gcl
index Maneuver = { Departure, Correction, Insertion }
```

If variants don't fit on one line:

```gcl
index Maneuver = {
    Departure,
    Correction,
    Insertion,
}
```

#### `use` Declarations

```gcl
use "./helper.gcl" { G0, isp };
```

#### Expressions

**Binary operations**: Spaces around operators, parentheses preserved as-is.

```gcl
a + b * c
(a + b) * c
```

**If expressions**: Always braces, single-line if short enough:

```gcl
if cond { then_expr } else { else_expr }
```

Multi-line if branches are long:

```gcl
if @altitude > 100.0 km {
    high_altitude_drag
} else {
    low_altitude_drag
}
```

**Match expressions**:

```gcl
match @maneuver {
    Impulsive { delta_v: _ } => 0.0 N,
    LowThrust { thrust, duration: _ } => thrust,
}
```

**Map literals** (indexed values):

```gcl
{
    Maneuver::Departure: 2.46 km/s,
    Maneuver::Correction: 0.12 km/s,
    Maneuver::Insertion: 1.83 km/s,
}
```

**For comprehensions**:

```gcl
for m: Maneuver { @delta_v[m] * 2.0 }
```

**Struct construction**: group — single-line if fits, else multi-line:

```gcl
TransferResult { dv1, dv2, total_dv: dv1 + dv2 }

// or when long:
TransferResult {
    dv1,
    dv2,
    total_dv: dv1 + dv2,
}
```

### Comment Placement Rules

- **Leading comment**: Comment on the line(s) immediately before a declaration or
  expression. Attached to the next AST node.
- **Trailing comment**: Comment on the same line after code (`x = 1; // comment`).
  Kept on the same line.
- **Standalone comment**: Comment separated by blank lines from surrounding code.
  Preserved in position between the nearest declarations.
- **Doc comments** (`///`): Always attached to the following declaration, printed
  immediately before it.

## Phase 3: Testing Strategy

### Idempotency Tests

For every test case: `format(format(source)) == format(source)`.

### Snapshot Tests

Use `insta` for snapshot testing:

- For each `.gcl` fixture in `tests/fixtures/`, format it and snapshot the output
- Add dedicated formatter test cases for edge cases

### Round-Trip Semantic Tests

`parse(format(source))` should produce a semantically equivalent AST to
`parse(source)` (ignoring spans). This ensures the formatter doesn't
change program meaning.

### Comment Preservation Tests

Verify that comments appear in the formatted output at correct positions.

### Fuzz Testing

Use `cargo fuzz` or property-based testing to ensure the formatter never
panics on arbitrary input.

## Implementation Order

| Step | Description | Estimated Scope |
|------|-------------|-----------------|
| **0a** | Comment extraction (`extract_comments`) in `graphcal-syntax` | ~100 LOC |
| **0b** | Create `graphcal-fmt` crate with `format_source` stub | Boilerplate |
| **1a** | Format simple declarations (dimension, unit, const, param, node without blocks) | ~200 LOC |
| **1b** | Add `graphcal format` CLI subcommand | ~50 LOC |
| **1c** | Idempotency + snapshot tests for step 1a | ~100 LOC |
| **2a** | Format expressions (binop, unary, if, fn call, unit literal, convert) | ~300 LOC |
| **2b** | Format blocks, let bindings | ~100 LOC |
| **2c** | Format type declarations (struct, tagged union) | ~100 LOC |
| **2d** | Format fn declarations (short + block, generics) | ~150 LOC |
| **2e** | Format indexed types (index, map literal, for comp, scan, unfold, match) | ~200 LOC |
| **3a** | Comment interleaving in formatter output | ~200 LOC |
| **3b** | LSP `textDocument/formatting` integration | ~50 LOC |
| **3c** | `--check` and `--diff` modes | ~50 LOC |
| **3d** | Comprehensive tests, fuzz testing | ~200 LOC |

## Configuration

Following the philosophy of `gofmt`, `gleam format`, and `ruff` — **no
configuration**. One canonical style. This aligns with the project's
preference for explicitness and safety over flexibility.

Fixed settings:

- **Line width**: 100 characters (engineering calculations tend to have long expressions)
- **Indentation**: 4 spaces
- **Trailing commas**: Yes, in multi-line lists (fields, params, map entries)
- **Trailing semicolons**: As required by grammar (`;` after declarations)

## Open Questions

1. **Line width**: 80 vs 100? Engineering expressions with unit annotations
   can be quite long. Suggest starting with 100.
2. **Blank line preservation**: Should we preserve user's blank lines between
   declarations (capped at 1), or enforce a fixed rule (always exactly 1)?
   Suggest preserving with cap at 1, like `gofmt`.
3. **Number formatting**: Should the formatter normalize `1_000.0` vs `1000.0`?
   Suggest preserving the original representation (use source spans to
   recover the literal text).
4. **Expression parentheses**: Should redundant parentheses be removed (e.g.,
   `(a + b) + c` → `a + b + c`)? Suggest no — keep parentheses as written,
   since they may communicate intent for engineering readability.

## References

- Wadler, "A Prettier Printer" (1998)
- Lindig, "Strictly Pretty" (2000) — strict-evaluation adaptation
- `pretty` crate: <https://crates.io/crates/pretty>
- Ruff formatter architecture: <https://astral.sh/blog/the-ruff-formatter>
- Gleam formatter: <https://github.com/gleam-lang/gleam/blob/main/compiler-core/src/format.rs>
