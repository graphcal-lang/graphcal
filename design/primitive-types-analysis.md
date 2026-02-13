# Analysis: Supporting Additional Primitive Types

> Brainstorm and analysis for extending graphcal beyond the current f64-only scalar model.

## Current State

Today, graphcal's runtime value model is:

```rust
enum RuntimeValue {
    Scalar(f64),           // all numeric + bool values
    Struct { ... },        // user-defined struct instances
    Indexed { ... },       // indexed collections (tables)
    VariantLabel { ... },  // transient loop variable
}
```

Every scalar—whether dimensioned float, dimensionless float, or boolean—is stored as a single `f64`. Booleans are encoded as `0.0`/`1.0`. There is no `i64`, `Str`, `Datetime`, or `Option<T>` at the runtime level.

The type system (dim_check.rs) tracks **dimensions** via `DeclaredType` / `InferredType`, both of which have `Scalar(Dimension)` and `Struct(String)` variants. A `Scalar(Dimension::DIMENSIONLESS)` covers both dimensionless floats and booleans—there is no separation between `f64`, `i64`, and `bool` at the dimension-checking level.

### Files that would need modification for any new primitive type

| Layer | File | What changes |
|-------|------|-------------|
| Lexer | `token.rs` | New literal tokens (int literal, string literal, datetime literal) |
| Parser | `parser.rs` | New `ExprKind` parsing, type annotation parsing |
| AST | `ast.rs` | New `ExprKind` variants, new `TypeExprKind` variants |
| Dim check | `dim_check.rs` | `DeclaredType` / `InferredType` variants, inference rules, compatibility checks |
| Registry | `registry.rs` | Type resolution for non-dimension types |
| Evaluator | `eval_expr.rs` | `RuntimeValue` variants, operator dispatch |
| Const eval | `const_eval.rs` | Support for new types in const context |
| Output | `eval.rs` | Display formatting for new types |
| CLI | `main.rs` | JSON output for new types |
| Builtins | `builtins.rs` | Type-aware function signatures |

---

## Candidate Types

### 1. `bool` (first-class)

**Current state:** `bool` literals (`true`/`false`) exist in the lexer, parser, and AST (`ExprKind::Bool`), but are evaluated as `RuntimeValue::Scalar(0.0 | 1.0)`. The type checker treats them as `Dimensionless`.

**What "first-class" means:**
- `DeclaredType::Bool` / `InferredType::Bool` distinct from `Scalar(Dimensionless)`
- `RuntimeValue::Bool(bool)` instead of encoding in f64
- Type errors when mixing `bool` and `f64` (e.g., `true + 1.0` is an error)
- Comparison operators (`==`, `<`, etc.) return `Bool`, not `Dimensionless`
- Logical operators (`&&`, `||`, `!`) require `Bool` operands and return `Bool`
- `if` condition must be `Bool`, not any `Dimensionless` scalar

**Difficulty:** Low-medium. The plumbing exists; it's mostly about splitting the `Scalar` path.

**Priority:** High. This is a correctness issue—today `true + 1.0` silently evaluates to `2.0`.

**Design questions:**
- Should `bool` be usable in `if` expressions only, or also in arithmetic context (C-style 0/1)?
  - Recommendation: strict separation. No implicit bool-to-number. Provide `if b { 1.0 } else { 0.0 }` for the rare case.
- Can struct fields be `bool`-typed? (Yes—needed for flags, status, constraints.)
- Can indexed values be `bool`-typed? (Yes—boolean masks over indexes.)

---

### 2. `i64` (64-bit integer)

**Current state:** Not implemented. All numbers parse as `f64`. The design doc lists `i64` as a planned primitive.

**Use cases:**
- Discrete counts (number of satellites, number of maneuvers, iterations)
- Index arithmetic (if range-based indexes are added later)
- Exact integer arithmetic where floating-point is inappropriate
- Pixel counts, sample counts, packet counts

**Design decisions needed:**

#### 2a. Can `i64` carry physical dimensions?

| Option | Syntax | Example | Implication |
|--------|--------|---------|-------------|
| **A: Always dimensionless** | `i64` only | `param n_sats: i64 = 24;` | Simple. `i64` is a counter type, never has units. |
| **B: Dimensioned integers** | `i64` with dimension | `param pixel_width: i64<Length> = 1920 px;` | Complex. Need `i64<D>` in type system. Operators like `+` must preserve int-ness. Division becomes tricky (integer division returns `i64`? or promotes to `f64`?). |

**Recommendation:** Option A (always dimensionless) for simplicity. If dimensioned integers are needed, they can be a future extension. Most engineering calculations that need physical dimensions use floating-point anyway.

#### 2b. Integer literal syntax

The current lexer regex `[0-9][0-9_]*(\.[0-9][0-9_]*)?([eE][+-]?[0-9]+)?` accepts both `42` and `42.0`. Options:

| Option | Rule | Example |
|--------|------|---------|
| **A: Dot distinguishes** | `42` → i64, `42.0` → f64 | Simple, familiar (Rust, Python 3) |
| **B: Suffix** | `42i` → i64, `42` → f64 | Explicit but verbose |
| **C: Type annotation only** | `42` is polymorphic; resolved by annotation | Flexible but more complex inference |

**Recommendation:** Option A. `42` is `i64`, `42.0` is `f64`. This is the most natural for an engineering audience. The lexer already distinguishes the presence of `.` in the regex.

#### 2c. `i64 ↔ f64` conversion

| Option | Behavior |
|--------|----------|
| **A: No implicit conversion** | Explicit `to_float(n)` / `to_int(x)` required |
| **B: i64 → f64 implicit, f64 → i64 explicit** | `42 * 1.5` works, `to_int(42.7)` needed for the reverse |
| **C: Fully implicit both ways** | Dangerous—silent truncation |

**Recommendation:** Option B. Implicit widening (i64→f64) is safe and ergonomic. Narrowing (f64→i64) requires explicit `floor()`, `ceil()`, `round()`, or `to_int()`.

#### 2d. Integer operators

- `+`, `-`, `*` on two `i64` → `i64`
- `/` on two `i64` → **open question**: integer division (truncating)? or promote to `f64`?
  - Recommendation: `i64 / i64 → i64` (truncating, like Rust). Provide `to_float()` if float division is needed.
- `^` on `i64` base with `i64` exponent → `i64` (with overflow check)
- Comparison operators on two `i64` → `bool`
- Mixed `i64 op f64` → promote `i64` to `f64`, result is `f64`

#### 2e. Implementation scope

- **Lexer:** Split `Number` token into `IntLiteral` and `FloatLiteral` (or use the parsed value to distinguish)
- **AST:** Add `ExprKind::Integer(i64)` alongside `ExprKind::Number(f64)`
- **RuntimeValue:** Add `Integer(i64)` variant
- **dim_check:** Add `InferredType::Integer` (always dimensionless) or embed in `Scalar` with a "numeric kind" tag
- **eval_expr:** Handle mixed-type arithmetic with promotion rules
- **Builtins:** Most math builtins (`sqrt`, `sin`, etc.) would only accept `f64`, not `i64`

**Estimated scope:** Medium. The promotion rules and mixed-type arithmetic are the trickiest part.

---

### 3. `Str` (string)

**Current state:** `Token::StringLiteral` exists in the lexer. The parser uses it only for `use` paths. There is no string expression type in the AST or evaluator.

**Use cases:**
- Labels, descriptions, metadata
- Formatting output: `"Transfer dv: " ++ to_string(@dv)`
- CSV/data file paths
- Notes on parameters

**Design decisions needed:**

#### 3a. What operations does `Str` support?

| Operation | Syntax | Notes |
|-----------|--------|-------|
| Concatenation | `++` or `+` | `++` avoids ambiguity with numeric `+` |
| Interpolation | `"dv = {expr}"` or `f"dv = {expr}"` | Powerful but complex parser change |
| Comparison | `==`, `!=` | Equality only; ordering is locale-dependent |
| Length | `len(s)` | Returns `i64` |
| Conversion | `to_string(x)` | Converts any scalar to string |

#### 3b. Can `Str` be used in `param`/`node`/`const`?

- `param mission_name: Str = "Artemis I";` — makes sense for metadata
- `node label: Str = "Phase " ++ to_string(@phase_num);` — derived labels

**Recommendation:** Support `Str` as a type for `param`/`node`/`const`. It's dimensionless and non-numeric. The type checker should reject arithmetic on strings.

#### 3c. Interaction with dimensions

`Str` has no dimension. It's a completely separate type branch:
- `DeclaredType::Str` / `InferredType::Str`
- No unit literals on strings
- No unit conversion on strings
- Arithmetic operators are type errors

#### 3d. Implementation scope

- **Lexer:** Already has `StringLiteral` token
- **AST:** Add `ExprKind::StringLiteral(String)`, handle in type annotations
- **TypeExprKind:** Add `Str` variant (or recognize "Str" as a keyword/builtin type)
- **RuntimeValue:** Add `Str(String)` variant
- **Operators:** String concatenation operator (`++` or similar)
- **Builtins:** `to_string()`, `len()`, maybe `contains()`, `starts_with()`

**Estimated scope:** Medium. The main complexity is deciding on string operations and adding a non-numeric type path through the evaluator.

**Priority:** Medium. Useful for metadata and labeling, but not core to engineering calculations.

---

### 4. `Datetime`

**Current state:** Listed in the design doc as TBD. Not implemented anywhere.

**Use cases:**
- Mission timelines: launch dates, maneuver epochs
- Scheduling: deadlines, milestones
- Time-series data: timestamps on measurements
- Duration calculations: "days between launch and arrival"

**Design decisions needed:**

#### 4a. Internal representation

| Option | Representation | Precision | Range |
|--------|---------------|-----------|-------|
| **A: Unix timestamp (f64)** | Seconds since 1970-01-01 UTC | ~microsecond | ±285,000 years |
| **B: Unix timestamp (i64 nanoseconds)** | Nanoseconds since 1970-01-01 UTC | nanosecond | ±292 years |
| **C: Calendar struct** | Year/month/day/hour/min/sec/nanos | nanosecond | Unlimited |
| **D: chrono::DateTime<Utc>** | Use the `chrono` crate | nanosecond | ±262,000 years |
| **E: TAI-based (astro)** | Seconds since J2000 (2000-01-12T11:58:55.816 UTC) | sub-microsecond | Sufficient for space missions |

**Recommendation:** Option D for general use (chrono is the Rust ecosystem standard). For an engineering/aerospace audience, consider also supporting TAI/TDB/TT time scales via a library like `hifitime`. Initial implementation can use `chrono::DateTime<Utc>` and defer multi-time-scale support.

#### 4b. Literal syntax

| Option | Syntax |
|--------|--------|
| **A: ISO 8601 string** | `datetime("2024-11-05T12:00:00Z")` |
| **B: Native literal** | `2024-11-05T12:00:00Z` (lexer recognizes ISO format) |
| **C: Constructor** | `datetime(2024, 11, 5, 12, 0, 0)` |

**Recommendation:** Option A (function-style with ISO string). It avoids complex lexer changes and is unambiguous. Option B is nicer but requires the lexer to recognize ISO 8601 patterns, which conflicts with number/identifier parsing (e.g., `2024-11` looks like `2024 - 11`).

#### 4c. Interaction with the `Time` dimension

This is the most important design question. The `Time` dimension represents physical durations (seconds, hours), while `Datetime` is an absolute point in time.

| Operation | Result type | Semantics |
|-----------|-------------|-----------|
| `Datetime - Datetime` | `Time` (duration) | Time difference between two instants |
| `Datetime + Time` | `Datetime` | Advance an instant by a duration |
| `Datetime - Time` | `Datetime` | Go back by a duration |
| `Time + Time` | `Time` | Add durations (already works) |
| `Datetime + Datetime` | **Error** | Adding two instants is meaningless |
| `Datetime * anything` | **Error** | Multiplication of instants is meaningless |
| `Datetime / anything` | **Error** | Division of instants is meaningless |

This is analogous to the point-vs-vector distinction in geometry. Datetime is a "point" in time, `Time` dimension values are "vectors."

**This interacts with the planned Spaces feature (Phase 9).** A Datetime could be modeled as a "point in Time-space" while a duration is a "vector in Time-space." However, this may be over-engineering for the initial implementation.

**Recommendation:** Implement `Datetime` as a distinct type (not a dimension). The type checker enforces the point-vs-vector algebra above. Internally, store as `i64` nanoseconds or `chrono::DateTime<Utc>`.

#### 4d. Display and conversion

- Display format: ISO 8601 by default, customizable via format strings
- Timezone handling: store UTC internally, convert for display
  - `@launch_time -> "America/New_York"` (stretch goal)
- Duration display: `@arrival - @launch -> days` (uses existing `Time` dimension units)

#### 4e. Implementation scope

- **New dependency:** `chrono` crate (or similar)
- **Lexer:** No change needed if using function-style literals
- **AST:** `ExprKind::DatetimeLiteral` or handled via `FnCall` to `datetime()`
- **TypeExprKind:** New `Datetime` variant
- **DeclaredType/InferredType:** New `Datetime` variant
- **RuntimeValue:** New `Datetime(chrono::DateTime<Utc>)` variant
- **Operators:** Custom rules for datetime±duration, datetime-datetime
- **Builtins:** `datetime()`, `now()`, `year()`, `month()`, `day()`, `hour()`, `minute()`, `second()`

**Estimated scope:** Large. The interaction with the Time dimension requires careful type-checker work. The operator overloading (datetime + duration → datetime) is a new pattern not present in the current type system.

**Priority:** Medium-high for aerospace/mission planning use cases. Can be deferred for general engineering.

---

### 5. `Option<T>` (nullable/optional)

**Current state:** Listed in the design doc. Not implemented.

**Use cases:**
- Spreadsheet import: blank cells become `None`
- Conditional computation: "this field only has a value if condition X"
- Optional parameters: `param backup_orbit: Option<Length> = None;`

**Design decisions needed:**

#### 5a. Syntax

| Aspect | Options |
|--------|---------|
| **Type** | `Option<Length>` or `Length?` |
| **Construction** | `Some(400 km)` / `None` or `400 km` / `None` |
| **Access** | `match`, `unwrap_or(default)`, `??` operator |

**Recommendation:** `Option<Length>` for type syntax (explicit, familiar). `Some(expr)` / `None` for construction. `unwrap_or(default)` for safe access. This mirrors the Rust conventions the codebase already follows.

#### 5b. Interaction with dimension system

`Option<D>` wraps any `DeclaredType`. The dimension of the inner value is preserved:
- `Option<Length>` can hold `Some(400 km)` or `None`
- `Some(400 km) + Some(200 km)` → **error** (no implicit unwrap)
- `unwrap_or(@x, 0 km)` → `Length` (the default must match the inner dimension)

#### 5c. Propagation rules

Two possible models:
- **Strict:** Any operation on `Option<T>` requires explicit unwrap. Safe but verbose.
- **Null-propagating:** `None + anything = None`. Convenient but hides errors.

**Recommendation:** Start strict. Add null-propagation as a later convenience if user demand warrants it.

#### 5d. Implementation scope

- **AST:** `TypeExprKind::Option { inner: Box<TypeExpr> }`
- **DeclaredType/InferredType:** `Option { inner: Box<Self> }`
- **RuntimeValue:** `Option { value: Option<Box<RuntimeValue>> }`
- **Builtins:** `Some()`, `is_some()`, `is_none()`, `unwrap_or()`
- **Keywords:** `None` as a keyword or builtin constant

**Estimated scope:** Medium-large. The generic nature of `Option<T>` means every type-checking rule needs to consider the optional wrapper.

**Priority:** Medium. Essential for spreadsheet compatibility (Phase 12), less critical for pure engineering calculations.

---

### 6. Other candidate types (lower priority)

#### 6a. Complex numbers

**Use cases:** Transfer functions, signal processing, impedance calculations, quantum mechanics.

**Representation:** `Complex { re: f64, im: f64 }` or a native `c64` type.

**Dimension interaction:** A complex number can carry a dimension: `Complex<Impedance>` has real and imaginary parts both in ohms.

**Syntax options:**
- `3.0 + 4.0i` (suffix `i` for imaginary unit)
- `complex(3.0, 4.0)` (constructor function)

**Recommendation:** Defer. Can be implemented as a user-defined struct `type Complex { re: Dimensionless, im: Dimensionless }` for now, though this loses dimension generics on the inner type. If needed, add as a built-in generic type later.

**Priority:** Low for general engineering; high for specific domains (EE, signal processing).

#### 6b. Decimal / fixed-point

**Use cases:** Financial calculations where f64 rounding is unacceptable.

**Representation:** `rust_decimal::Decimal` (128-bit) or similar.

**Recommendation:** Defer entirely. The target audience (engineering) doesn't typically need exact decimal arithmetic. f64 is standard for scientific/engineering work.

**Priority:** Low.

#### 6c. Duration (explicit type)

**Use cases:** Representing time spans distinctly from the `Time` dimension.

**Analysis:** The `Time` dimension already serves this purpose. `3600 s`, `1 hour`, `0.5 day` are all `Time`-dimensioned values. A separate `Duration` type would be redundant. The only case for it would be pairing with `Datetime` arithmetic, and that can be handled by having `Datetime ± Time → Datetime`.

**Recommendation:** Don't add. Use the existing `Time` dimension.

#### 6d. Range / interval

**Use cases:** Uncertainty bounds, tolerance specifications, min/max ranges.

**Representation:** `Range<T> { min: T, max: T }` or `Interval<D> { lower: D, upper: D }`.

**Recommendation:** Defer. Can be modeled as a struct. If interval arithmetic (automatic propagation of uncertainty) is desired, that's a significant feature requiring operator overloading for interval types.

**Priority:** Low-medium. Interesting for engineering tolerancing but complex.

---

## Recommended Implementation Order

Based on priority, difficulty, and dependencies:

### Phase A: First-class `bool` (prerequisite for everything else)

**Why first:** Separating `bool` from `f64` establishes the pattern for non-numeric types in the type checker and evaluator. Every subsequent type addition will follow the same structural pattern. It's also a correctness fix.

**Scope:** ~2-3 days focused work. Changes to `DeclaredType`, `InferredType`, `RuntimeValue`, `dim_check.rs`, `eval_expr.rs`.

### Phase B: `i64` integers

**Why second:** Adds a second numeric type, which forces solving the mixed-type arithmetic promotion problem once and for all. This pattern will be reused if `Decimal` or `Complex` are added later.

**Scope:** Medium. The lexer change (splitting int/float literals) and promotion rules are the main work.

### Phase C: `Str` strings

**Why third:** Adds the first non-numeric, non-boolean type. Relatively self-contained since strings don't interact with dimensions or arithmetic.

**Scope:** Small-medium. Mostly additive—new type path through the system.

### Phase D: `Datetime`

**Why fourth:** Most complex due to the point-vs-vector interaction with the `Time` dimension. Benefits from having `i64` (for internal representation) and `Str` (for parsing ISO strings) already implemented.

**Scope:** Large. New operator semantics, new crate dependency, careful type-checker work.

### Phase E: `Option<T>`

**Why last among the core types:** Generic over all other types, so it benefits from having the full type zoo available first. Also the most complex type-checker change (every inference rule must handle the optional wrapper).

**Scope:** Large.

---

## Cross-Cutting Concerns

### Type annotations

Currently, type annotations are dimension expressions (`Length`, `Mass * Length / Time^2`, `Dimensionless`). Adding non-dimension types requires extending `TypeExprKind`:

```
// Current
enum TypeExprKind {
    Dimensionless,
    DimExpr(DimExpr),
    Indexed { base, indexes },
}

// Proposed
enum TypeExprKind {
    Dimensionless,
    DimExpr(DimExpr),
    Indexed { base, indexes },
    Bool,                          // new
    Int,                           // new
    Str,                           // new
    Datetime,                      // new
    Optional { inner: Box<Self> }, // new
}
```

Alternatively, `Bool`, `Int`, `Str`, `Datetime` could be recognized as builtin type names within `DimExpr` resolution, avoiding AST changes. However, this conflates dimensions with non-dimension types, which may cause confusion.

**Recommendation:** Add explicit `TypeExprKind` variants. This keeps the distinction between "dimensioned scalar" and "non-dimensioned primitive" clear in the AST.

### `DeclaredType` / `InferredType` expansion

```
// Proposed
enum DeclaredType {
    Scalar(Dimension),    // dimensioned f64
    Integer,              // i64 (dimensionless)
    Bool,                 // bool
    Str,                  // string
    Datetime,             // datetime
    Optional(Box<Self>),  // Option<T>
    Struct(String),       // user-defined struct
    Indexed { element: Box<Self>, index: String },
}
```

### `RuntimeValue` expansion

```
// Proposed
enum RuntimeValue {
    Scalar(f64),
    Integer(i64),
    Bool(bool),
    Str(String),
    Datetime(chrono::DateTime<Utc>),
    Optional(Option<Box<Self>>),
    Struct { type_name: String, fields: IndexMap<String, Self> },
    Indexed { index_name: String, entries: IndexMap<String, Self> },
    VariantLabel { variant: String },
}
```

### Struct field types

Currently, struct fields are typed by dimension only (`StructField { name, dimension }`). With non-dimension types, this needs expansion:

```
// Proposed
struct StructField {
    name: String,
    field_type: DeclaredType,  // was: dimension: Dimension
}
```

### Builtin function signatures

Currently, builtin functions use `DimSignature` which describes dimension constraints. With multiple primitive types, signatures need to also express type constraints (e.g., `sqrt` accepts `f64` but not `i64` or `bool`).

### JSON output

The CLI's JSON output would need to represent new types:
- `bool`: `true` / `false` (not `1.0` / `0.0`)
- `i64`: integer number (not float)
- `Str`: JSON string
- `Datetime`: ISO 8601 string
- `Option`: value or `null`

---

## Summary Table

| Type | Priority | Scope | Dim interaction | Key challenge |
|------|----------|-------|-----------------|---------------|
| `bool` (first-class) | High | Low-med | None (separate) | Splitting from f64, operator return types |
| `i64` | Medium-high | Medium | None (dimensionless) | Mixed-type promotion, int division semantics |
| `Str` | Medium | Small-med | None | String operations, non-numeric type path |
| `Datetime` | Medium-high | Large | Yes (point vs vector with Time) | Operator semantics, crate dependency |
| `Option<T>` | Medium | Large | Wraps any type | Generic type in checker, unwrap semantics |
| `Complex` | Low | Large | Yes (same dim for re/im) | Operator overloading, function signatures |
| `Decimal` | Low | Medium | Like f64 | New numeric type, crate dependency |
| `Range<T>` | Low | Large | Wraps dimensioned types | Interval arithmetic propagation |
