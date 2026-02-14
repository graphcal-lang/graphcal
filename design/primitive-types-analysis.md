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

### 3. ~~`Str` (string)~~ — Not needed

**Verdict: Removed from the candidate list.**

The primary use cases for strings in engineering calculations are categorical/choice data: mission phase names, fuel types, statuses, region labels. These are better modeled by **fieldless `type` declarations** (simple enums), which already fit within the planned Phase 10 (tagged unions) and are partially available today via `index`:

```
// Today (index — already works for table axes):
index Phase = { Design, Build, Test, Launch }

// Phase 10 (fieldless type — proper enum):
type FuelKind { LH2, RP1, Methane, Solid }
type Status { Active, Inactive, Pending }
```

Fieldless types provide stronger guarantees than strings:
- **Exhaustiveness checking** — `match` must cover all variants
- **Typo protection** — `FuelKind::Metahne` is a compile error, `"Metahne"` is not
- **No need for string comparison semantics** — equality is structural, not textual
- **No string operations to design** — no concatenation, interpolation, length, etc.

The remaining `Str` use cases (free-form descriptions, file paths, formatted output) are metadata concerns that live outside the calculation graph. They can be handled by:
- Comments in source files (already supported)
- A future metadata/annotation system (e.g., `#[description = "..."]`)
- External tooling (CLI flags, scenario files)

**If a genuine need for runtime strings arises** (e.g., dynamically constructed file paths for data import), it can be reconsidered, but it should not be a priority for the type system.

---

### 4. `Datetime`

**Current state:** Listed in the design doc as TBD. Not implemented anywhere.

> **Deep analysis:** See [`.local/2026-02-14_datetime-primitive-deep-analysis.md`](../.local/2026-02-14_datetime-primitive-deep-analysis.md) for the full design document covering time scales, hifitime integration, operator semantics, builtin functions, and implementation strategy.

**Use cases:**
- Mission timelines: launch dates, maneuver epochs (TAI/TT/TDB time scales)
- GNSS operations: GPS receiver timestamps, satellite clock corrections (GPST/GST/BDT)
- Scheduling: deadlines, milestones (UTC)
- Time-series data: timestamps on measurements
- Duration calculations: "days between launch and arrival"

**Design decisions (settled in deep analysis):**

#### 4a. Internal representation: `hifitime::Epoch`

| Option | Representation | Precision | Range | Time Scales | Verdict |
|--------|---------------|-----------|-------|-------------|---------|
| ~~A: Unix timestamp (f64)~~ | Seconds since 1970 UTC | ~microsecond | ±285,000 years | UTC only | Rejected |
| ~~B: Unix timestamp (i64 ns)~~ | Nanoseconds since 1970 UTC | nanosecond | ±292 years | UTC only | Rejected |
| ~~C: Calendar struct~~ | Year/month/day/... | nanosecond | Unlimited | Custom | Rejected |
| ~~D: chrono::DateTime<Utc>~~ | chrono internal | nanosecond | ±262,000 years | UTC only | **Rejected** |
| **E: hifitime::Epoch** | i16 centuries + u64 ns (10 bytes) | nanosecond | ±32,768 centuries | TAI, UTC, TT, TDB, ET, GPST, GST, BDT | **Chosen** |

**Recommendation: `hifitime::Epoch`** — purpose-built for aerospace, validated against NASA SPICE (zero-nanosecond difference post-1972), integer-based (no floating-point drift), supports 9 time scales natively. Neither `chrono` nor `jiff` support multi-scale time — they are designed for civil timekeeping only.

#### 4b. Literal syntax: function-style with ISO 8601 strings

```graphcal
// Daily use (UTC default)
param launch: Datetime = datetime("2024-11-05T12:00:00Z");

// Aerospace use (explicit time scale)
param epoch: Datetime<TT> = datetime("2024-11-05T12:00:00", TT);
param gps_fix: Datetime<GPST> = datetime("2024-11-05T11:59:42", GPST);

// From Julian Date
param j2000: Datetime<TT> = from_jd(2451545.0, TT);
```

Native literal syntax (`2024-11-05T12:00:00Z` without quotes) is rejected because `2024-11` parses as `2024 - 11` (integer subtraction).

#### 4c. Two-tier type design: simple default + scale-aware aerospace mode

**Tier 1 (daily use):** `Datetime` with no type parameter defaults to UTC.
**Tier 2 (aerospace):** `Datetime<TT>`, `Datetime<TAI>`, etc. with compile-time scale checking.

Cross-scale arithmetic (e.g., `Datetime<TT> - Datetime<GPST>`) is a type error — explicit conversion required via `to_tt()`, `to_gpst()`, etc.

#### 4d. Interaction with the `Time` dimension (point vs vector)

This is the most important design question. `Datetime` is a **point** in time, `Time` dimension values are **vectors** (durations). This is analogous to the point-vs-vector distinction in geometry.

| Operation | Result type | Semantics |
|-----------|-------------|-----------|
| `Datetime<S> - Datetime<S>` | `Scalar(Time)` | Duration between two instants |
| `Datetime<S> + Scalar(Time)` | `Datetime<S>` | Advance an instant by a duration |
| `Scalar(Time) + Datetime<S>` | `Datetime<S>` | Commutative with above |
| `Datetime<S> - Scalar(Time)` | `Datetime<S>` | Go back by a duration |
| `Time + Time` | `Time` | Add durations (already works) |
| `Datetime + Datetime` | **Type Error** | Adding two points is meaningless |
| `Datetime * anything` | **Type Error** | Scaling a point is meaningless |
| `Datetime / anything` | **Type Error** | Dividing a point is meaningless |
| `Datetime<S1> - Datetime<S2>` | **Type Error** (if S1 ≠ S2) | Must convert to same scale first |

No separate `Duration` primitive needed — the existing `Time` dimension serves this role.

**Interaction with Spaces (Phase 9):** When Spaces are implemented, time scales can be modeled as a built-in space:

```graphcal
space TimeScale { TAI; UTC; TT; TDB; GPST; }
param launch: Datetime in TimeScale.TT = ...;
```

This unifies two concepts (time scale and space tag) and could replace the type parameter approach. The point-vs-vector semantics also naturally fit an "affine space" extension of the Spaces feature.

#### 4e. Display and conversion

- Display format: ISO 8601 with time scale suffix: `2024-11-05T12:00:32.184000000 TT`
- For UTC-only datetimes: standard ISO 8601: `2024-11-05T12:00:00Z`
- JSON output: `{"type": "datetime", "value": "2024-11-05T12:00:00", "scale": "TT"}`
- No `now()` function (breaks determinism) — inject current time via `param`

#### 4f. Builtin functions

| Category | Functions |
|----------|-----------|
| Construction | `datetime(str)`, `datetime(str, scale)`, `from_jd(f64, scale)`, `from_mjd(f64, scale)`, `from_unix(f64)` |
| Conversion | `to_utc(dt)`, `to_tai(dt)`, `to_tt(dt)`, `to_tdb(dt)`, `to_gpst(dt)` |
| Extraction | `year(dt)`, `month(dt)`, `day(dt)`, `hour(dt)`, `minute(dt)`, `second(dt)`, `day_of_year(dt)` |
| Julian | `to_jd(dt)`, `to_mjd(dt)`, `to_unix(dt)`, `leap_seconds(dt)` |
| Rounding | `floor_dt(dt, interval)`, `ceil_dt(dt, interval)`, `round_dt(dt, interval)` |
| Aggregation | `min(dt[I])`, `max(dt[I])` (but NOT `sum` or `mean` — adding points is meaningless) |

#### 4g. Open questions

- **Str prerequisite?** `datetime("...")` needs string literal parsing. Recommendation: support string literals in the lexer/parser without making `Str` a full runtime type (Option A from the deep analysis).
- **TimeScale representation:** Initially as builtin constants (like `true`/`false`). Migrate to a builtin enum when tagged unions (Phase 10) arrive.
- **Additional time units:** Add `day` (86400 s) and `week` (604800 s) to the prelude. Do NOT add `month` or `year` as time units (variable length). Add `julian_year` (365.25 days) for astronomical use.
- **Precision note:** `Datetime +/- f64(Time)` has ~100ns precision at Unix epoch magnitude due to f64 limitations. Document clearly. Users needing higher precision should use `Datetime - Datetime` (stays in integer math internally).

#### 4h. Implementation scope

- **New dependency:** `hifitime` crate (v4)
- **Lexer:** String literal support (minimal — for `datetime()` arguments)
- **Parser:** `Datetime` and `Datetime<Scale>` type annotations, time scale constants
- **AST:** `TypeExprKind::Datetime` / `TypeExprKind::DatetimeScaled { scale }`
- **DeclaredType/InferredType:** `Datetime(Option<TimeScale>)` variant
- **RuntimeValue:** `Datetime(hifitime::Epoch)` variant
- **Operators:** Custom point-vs-vector rules (new pattern, but well-defined)
- **Builtins:** Datetime functions require generalizing the builtin system beyond `fn(&[f64]) -> f64`
- **Output:** ISO 8601 with scale, JSON with type/value/scale

**Estimated scope:** Large. Comparable to i64 plus new crate dependency and builtin system generalization.

**Priority:** Medium-high for aerospace/mission planning. The two-tier design ensures daily-life users aren't burdened by aerospace complexity.

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

### Phase C: `Datetime`

**Why third:** Most complex due to the point-vs-vector interaction with the `Time` dimension. Benefits from having `i64` (for internal representation) already implemented.

**Scope:** Large. New operator semantics, new crate dependency, careful type-checker work.

### Phase D: `Option<T>`

**Why last among the core types:** Generic over all other types, so it benefits from having the full type zoo available first. Also the most complex type-checker change (every inference rule must handle the optional wrapper).

**Scope:** Large.

*Note: `Str` was removed from this list. Categorical data is better served by fieldless `type` declarations (see section 3 above).*

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
    Datetime,                      // new
    Optional { inner: Box<Self> }, // new
}
```

Alternatively, `Bool`, `Int`, `Datetime` could be recognized as builtin type names within `DimExpr` resolution, avoiding AST changes. However, this conflates dimensions with non-dimension types, which may cause confusion.

**Recommendation:** Add explicit `TypeExprKind` variants. This keeps the distinction between "dimensioned scalar" and "non-dimensioned primitive" clear in the AST.

### `DeclaredType` / `InferredType` expansion

```
// Proposed
enum DeclaredType {
    Scalar(Dimension),              // dimensioned f64
    Integer,                        // i64 (dimensionless)
    Bool,                           // bool
    Datetime(Option<TimeScale>),    // datetime (None = UTC default, Some = explicit scale)
    Optional(Box<Self>),            // Option<T>
    Struct(String),                 // user-defined struct
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
    Datetime(hifitime::Epoch),  // 10 bytes, time-scale aware, nanosecond precision
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
- `Datetime`: ISO 8601 string
- `Option`: value or `null`

---

## Summary Table

| Type | Priority | Scope | Dim interaction | Key challenge |
|------|----------|-------|-----------------|---------------|
| `bool` (first-class) | High | Low-med | None (separate) | Splitting from f64, operator return types |
| `i64` | Medium-high | Medium | None (dimensionless) | Mixed-type promotion, int division semantics |
| `Datetime` | Medium-high | Large | Yes (point vs vector with Time) | hifitime integration, multi-scale type param, builtin generalization |
| `Option<T>` | Medium | Large | Wraps any type | Generic type in checker, unwrap semantics |
| ~~`Str`~~ | ~~Removed~~ | — | — | Superseded by fieldless `type` (enums) |
| `Complex` | Low | Large | Yes (same dim for re/im) | Operator overloading, function signatures |
| `Decimal` | Low | Medium | Like f64 | New numeric type, crate dependency |
| `Range<T>` | Low | Large | Wraps dimensioned types | Interval arithmetic propagation |
