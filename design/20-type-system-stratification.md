# Type System Stratification

> A unified model of Graphcal's type universe: what types exist, how they compose, and where each kind of entity can appear.

## Status

**Decision level:** Accepted. Supersedes the ad-hoc layering in docs 03-07 with a single coherent model. All open questions resolved.

## Motivation

The original type system documentation describes six "orthogonal layers" (primitives, dimensions, units, ADTs, spaces, indexes). While each layer is individually sound, several questions fall between the cracks:

- What do we call the union of `Scalar(Dim) | Int | Bool | Struct | TaggedUnion`? Is it a "type"?
- Are indexed types (`T[I]`) the same kind of thing as value types (`T`)?
- What is the type of a `for` loop variable? Of an index label? Of a function?
- What can be passed to functions? Returned? Stored?

This document proposes a **three-level type stratification** that answers all of these questions and connects the type system to the computation model (DAG).

## The Three Levels

```
Level 1: Primitive   = Scalar(Dim) | Int | Bool | Str
Level 2: ValueType   = Primitive
                     | Struct(name, fields: [ValueType])
                     | TaggedUnion(name, variants: [Variant(fields: [ValueType])])
Level 3: DeclType    = ValueType
                     | Indexed(ValueType, [Index])   -- written T[I] or T[I, J, ...]
```

### Level 1: Primitive

The indivisible types. Each represents a single atomic datum.

| Type | Representation | Dimension? | Scalable? |
| --- | --- | --- | --- |
| `Scalar(Dim)` | `f64` in SI base units | Yes | Yes |
| `Int` | `i64` | No (dimensionless) | No |
| `Bool` | `bool` | No (dimensionless) | No |
| `Str` | `String` | No | No |

Only `Scalar` carries a physical dimension. `Int` and `Bool` are non-scalable — you cannot multiply an integer by an arbitrary unit scale factor and get a meaningful integer back.

Future candidates: `Datetime` (non-scalable, potentially linked to `Time` dimension for arithmetic).

### Level 2: ValueType

A single logical value. Primitives plus algebraic compositions of them.

```gcl
// Primitives are ValueTypes:
42              // Int
3.14            // Scalar(Dimensionless)
true            // Bool
2.46 km/s       // Scalar(Length / Time)

// Structs are ValueTypes:
type Orbit {
    sma: Length,
    ecc: Dimensionless,
    inc: Angle,
}

// Tagged union variants are ValueTypes:
type ManeuverKind {
    Impulsive(delta_v: Velocity)
    LowThrust(thrust: Force, duration: Time)
}

// Generic structs are ValueTypes:
type Vec3<D: Dim, Frame: Type> derive(Add, Sub, Neg) {
    x: D,
    y: D,
    z: D,
}
```

**Key property:** A ValueType represents ONE value. A `Vec3<Length, ECI>` with three fields is still one logical value — you can pass it to a function, return it, store it.

**Struct field constraint:** All struct fields must be ValueTypes, not DeclTypes. You cannot put `Velocity[Maneuver]` inside a struct field. If you want indexed data alongside structured data, index the struct itself: `Vec3<Velocity, ECI>[Maneuver]`.

**Naming rationale:** "ValueType" was chosen because it emphasizes the key property (this is the type of *one* value). Alternatives considered: `GroundType` (too academic), `DataType` (confused with database), `ElementType` (good but secondary), `CellType` (spreadsheet-specific).

### Level 3: DeclType

What can appear in type annotations of `param`, `node`, and `const` declarations. Either a ValueType (one value) or an indexed collection of ValueTypes.

```gcl
param dry_mass: Mass = 1200 kg;                         // ValueType
param delta_v: Velocity[Maneuver] = { ... };            // Indexed ValueType
node matrix: Dimensionless[Row, Col] = for r, c { ... } // Multi-indexed ValueType
```

`T[I]` is a type constructor that lifts a ValueType into a total map from index labels to values. Multi-indexing `T[I, J]` is a total map from the product `I × J` to values — a flat product-key map, not nested maps. Each DAG node is identified by a flat tuple of labels `(i, j)`, matching the flat binding structure of `for i: I, j: J { ... }`.

## DAG Correspondence

The type stratification has a direct correspondence with the computation model:

> **A node in the evaluation DAG has type `ValueType`. A declaration of type `ValueType[Index]` expands to one DAG node per index label. A declaration of type `ValueType[I, J]` expands to one DAG node per label tuple `(i, j)`.**

| Declaration type | DAG nodes |
| --- | --- |
| `node x: Velocity` | 1 node |
| `node x: Velocity[Maneuver]` (3 labels) | 3 nodes |
| `node x: Velocity[Phase, Maneuver]` (2 x 3) | 6 nodes |

The `for` comprehension is the mechanism that expands a single declaration into multiple DAG nodes. Each node is independently evaluable (modulo data dependencies), making indexed values naturally parallelizable.

This also explains why arithmetic on indexed values requires explicit `for`: you are defining the computation for each individual DAG node, not operating on the collection as a whole.

## Indexes

An index declares a finite, ordered set of labels usable as collection axes in `T[I]`. Two flavors exist.

### Named Index — A Fieldless Tagged Union

```gcl
index Maneuver = { Departure, Correction, Insertion }
```

A named index is a **fieldless tagged union** that is additionally registered as a collection axis. The `index` keyword declares two things at once:

1. A **ValueType**: `Maneuver` is a tagged union whose variants (`Departure`, `Correction`, `Insertion`) carry no fields. `Maneuver::Departure` has type `Maneuver`.
2. An **axis marker**: `Maneuver` can be used in `T[Maneuver]` to create indexed types.

Because named index labels are proper ValueType values, they follow all ValueType rules uniformly — no special cases:

- **Pass to functions:** `fn f(m: Maneuver) -> Velocity` works.
- **Return from functions:** `fn pick() -> Maneuver` works.
- **Store in variables:** `let x = Maneuver::Departure` works.
- **Compare:** `m == Maneuver::Departure` works.
- **Pattern match:** `match m { Maneuver::Departure => ..., _ => ... }` works.
- **Use in struct fields:** `type Config { phase: Phase, maneuver: Maneuver }` works.

This eliminates the need for a separate "where can labels appear" rule. Named index labels are values. Period.

**Why not just use `type`?** A regular fieldless tagged union (`type Foo { A, B }`) is NOT automatically an index. The `index` keyword explicitly marks it as usable in `T[I]`. This prevents accidentally using marker types (like `type ECI {}`) as collection axes. If you just need a fieldless enum without collection semantics, use `type`.

### Range Index

```gcl
index TimeStep = range(0.0 s, 100.0 s, step: 0.1 s);
```

A range index is a finite sequence of scalar values in a specific dimension. Unlike named indexes, range index labels are **not** tagged union variants — they are scalar values generated from the range parameters.

Range indexes do not introduce a ValueType. The loop variable in `for t: TimeStep { ... }` acts as a `Scalar(Time)` — it can be used in arithmetic (`t + 0.1 s`) and for indexing (`@x[t]`).

### Unified Interface

Both flavors support the same type constructor `T[I]` and the same operations (`for`, indexing, aggregation, scan). The difference is in the nature of the loop variable:

| Capability | Named index (`Maneuver`) | Range index (`TimeStep`) |
| --- | --- | --- |
| Loop variable type | `Maneuver` (ValueType) | `Scalar(Dim)` (Primitive) |
| Indexing: `@x[m]` | Yes | Yes |
| Map literal key | Yes | No (range labels are implicit) |
| Equality comparison | Yes (as ValueType) | Yes (as Scalar) |
| Pattern matching | Yes (as tagged union) | No (not a tagged union) |
| Arithmetic | No (not a scalar) | Yes |
| Pass to function | Yes | Yes (as scalar) |
| Store in variable | Yes | Yes (as scalar) |

Note that both loop variable types are ValueTypes — named index labels are tagged union values, range index labels are scalar values. There are no second-class citizens.

### Examples

```gcl
// Per-label logic with match (named index labels are tagged union values)
node delta_v_budget: Velocity[Phase, Maneuver] = for p: Phase, m: Maneuver {
    match (p, m) {
        (Phase::Launch, Maneuver::Departure) => 2.46 km/s,
        (Phase::Cruise, Maneuver::Correction) => 0.05 km/s,
        (Phase::Arrival, Maneuver::Insertion) => 1.48 km/s,
        _ => 0.0 m/s,
    }
}

// Per-label function dispatch (labels are passable to functions)
fn maneuver_fuel(m: Maneuver, params: MissionParams) -> Mass {
    match m {
        Maneuver::Departure => compute_departure_fuel(params),
        Maneuver::Correction => compute_correction_fuel(params),
        Maneuver::Insertion => compute_insertion_fuel(params),
    }
}

node fuel: Mass[Maneuver] = for m: Maneuver {
    maneuver_fuel(m, @params)
}

// Index labels in struct fields
type ManeuverAssignment {
    phase: Phase,
    maneuver: Maneuver,
    priority: Int,
}
```

## Indexed Values

### Construction

**Map literal** (total — all labels must be present):

```gcl
param delta_v: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km/s,
    Maneuver::Correction: 0.05 km/s,
    Maneuver::Insertion: 1.48 km/s,
}
```

**`for` comprehension** (one value per label):

```gcl
node fuel: Mass[Maneuver] = for m: Maneuver {
    @dry_mass * (exp(@delta_v[m] / @v_exhaust) - 1.0)
}
```

**Multi-axis `for`** (flat — one value per label tuple):

```gcl
node matrix: Dimensionless[Row, Col] = for r: Row, c: Col {
    @A[r, c] + @B[r, c]
}
```

**Multi-axis map literal** (total — all label tuples must be present):

```gcl
param delta_v_budget: Velocity[Phase, Maneuver] = {
    (Phase::Launch, Maneuver::Departure): 2.46 km/s,
    (Phase::Launch, Maneuver::Correction): 0.0 m/s,
    (Phase::Launch, Maneuver::Insertion): 0.0 m/s,
    (Phase::Cruise, Maneuver::Departure): 0.0 m/s,
    (Phase::Cruise, Maneuver::Correction): 0.05 km/s,
    (Phase::Cruise, Maneuver::Insertion): 0.0 m/s,
    (Phase::Arrival, Maneuver::Departure): 0.0 m/s,
    (Phase::Arrival, Maneuver::Correction): 0.0 m/s,
    (Phase::Arrival, Maneuver::Insertion): 1.48 km/s,
}
```

Note: single-axis map literals use bare keys (`Maneuver::Departure: ...`), multi-axis map literals use tuple keys (`(Phase::Launch, Maneuver::Departure): ...`). This mirrors the flat `for` binding structure.

### Consumption

**Indexing** — extracts a single element by providing all index labels:

```gcl
@delta_v[Maneuver::Departure]                // Velocity[Maneuver] → Velocity
@matrix[Row::R1, Col::C2]                    // Dimensionless[Row, Col] → Dimensionless
```

No partial indexing — all axes must be specified. To extract a "slice" along one axis, use explicit `for`:

```gcl
// Extract one row from a multi-indexed value
node row1: Dimensionless[Col] = for c: Col { @matrix[Row::R1, c] }
```

This is consistent with the "explicit `for`" philosophy: slicing is a computation over an axis, so it uses `for`.

**Aggregation** — collapses one or more axes:

```gcl
sum(for m: Maneuver { @fuel[m] })    // Mass[Maneuver] → Mass
max(for m: Maneuver { @delta_v[m] }) // Velocity[Maneuver] → Velocity
```

**Scan** — ordered accumulation:

```gcl
scan(
    for m: Maneuver { @delta_v[m] },
    0.0 m/s,
    |acc, val| acc + val,
)   // Velocity[Maneuver] → Velocity[Maneuver]
```

### No Implicit Broadcasting

Arithmetic on indexed values requires explicit `for`. This is a deliberate safety decision:

```gcl
// ERROR: cannot add Velocity[Maneuver] + Velocity[Maneuver]
node bad = @delta_v + @extra_dv;

// CORRECT: explicit element-wise operation
node good: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] + @extra_dv[m]
}
```

This prevents the class of silent broadcasting bugs common in NumPy and Excel, where mismatched shapes are silently resolved.

## Functions

Functions are declarations, not values. There is no function type in the type system.

```gcl
fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;
```

### What functions can accept and return

Functions accept and return DeclTypes (ValueTypes or indexed types):

```gcl
// ValueType params and return
fn hohmann(gm: Length^3 / Time^2, r1: Length, r2: Length) -> TransferResult { ... }

// Indexed type params and return
fn total<D: Dim, I: Index>(values: D[I]) -> D = sum(values);
fn normalize<I: Index>(v: Dimensionless[I]) -> Dimensionless[I] = for i: I {
    v[i] / sum(v)
};
```

### What functions CANNOT accept

- **Functions:** No higher-order functions. No `fn apply(f: (A) -> B, x: A) -> B`.
- **Dimensions/units as values:** Dimensions appear as generic params (`<D: Dim>`), not as runtime values.

Note: Named index labels ARE passable to functions because they are ValueTypes (fieldless tagged union values). `fn f(m: Maneuver) -> Velocity` is valid.

### Scan and other special forms

`scan`'s `|acc, val| body` is **special syntax**, not a function value. It is comparable to how `for m: I { body }` is special syntax that introduces bindings. Both are syntactic forms that the compiler handles directly, not expressions that evaluate to function values.

If higher-order functions are ever added (deferred per design doc 12), they would introduce a new kind in the type universe. But for engineering calculations, `for`/`sum`/`scan` as special forms cover the use cases.

## Complete Entity Map

| Entity | Is a type? | First-class value? | Pass to `fn`? | Return from `fn`? | Appears in expressions |
| --- | --- | --- | --- | --- | --- |
| Scalar value | ValueType | Yes | Yes | Yes | Yes |
| Int value | ValueType | Yes | Yes | Yes | Yes |
| Bool value | ValueType | Yes | Yes | Yes | Yes |
| Str value | ValueType | Yes | Yes | Yes | Yes |
| Struct instance | ValueType | Yes | Yes | Yes | Yes |
| Tagged union variant | ValueType | Yes | Yes | Yes | Yes |
| Named index label | ValueType | Yes | Yes | Yes | Yes |
| Indexed value | DeclType | Yes | Yes | Yes | Via `for` |
| Range index label | Scalar(Dim) | Yes | Yes (as scalar) | Yes (as scalar) | Indexing, arithmetic |
| Function | No | No | No | No | Calling only |
| Dimension | No (compile-time) | No | As generic `<D: Dim>` | As generic | No |
| Unit | No (compile-time) | No | No | No | In literals only |
| Index | No (compile-time) | No | As generic `<I: Index>` | As generic | No |

Note: Named index labels and tagged union variants share the same row conceptually — a named index IS a fieldless tagged union. They are listed separately only to highlight that index labels are full ValueType citizens.

## Implications for Existing Design Documents

This document unifies concepts spread across multiple existing docs:

- **03-primitives.md**: Level 1 of this stratification.
- **04-dimensions-and-units.md**: Compile-time metadata on `Scalar` types. Unchanged.
- **05-algebraic-data-types.md**: Level 2. Struct fields must be ValueTypes. Named indexes are fieldless tagged unions within this framework.
- **06-spaces.md**: Orthogonal — spaces are phantom type parameters on structs. Unchanged.
- **07-indexes.md**: Named indexes are now defined as fieldless tagged unions + axis marker. The separate "index labels vs tagged union variants" distinction is collapsed — they are the same thing.
- **10-tables-and-autofill.md**: Superseded. The `table` keyword is replaced by co-indexed declarations. `T[I]` is the table type.
- **12-pure-functions.md**: Clarifies that functions are declarations, not types. `scan` lambda is special syntax. Functions CAN accept named index labels since they are ValueTypes.

### Implications for Implementation

- **`LoopVar` hack eliminated:** In the current codebase, `InferredType::LoopVar(IndexName)` is a special type for `for` loop variables marked "not a real value type." With named index labels as ValueTypes, the loop variable `m: Maneuver` has inferred type `Struct(Maneuver, [])` (a fieldless tagged union) — a normal ValueType. No special case needed.
- **`VariantLabel` runtime value unified:** The current `RuntimeValue::VariantLabel { variant }` can be represented as a `RuntimeValue::Struct { type_name, variant, fields: {} }` — a struct with no fields. Or a dedicated `Tag` variant if performance matters.
- **Index registry shares with type registry:** An `index Maneuver = { ... }` declaration creates entries in BOTH the index registry (for `T[I]` semantics) and the type registry (for ValueType semantics).

## Resolved Questions

- **Option type:** `Option<T>` is just a tagged union — a normal ValueType. Nothing special about it. `Option<Velocity>` is a ValueType. `Option<Velocity>[Maneuver]` is a DeclType. No special-casing needed.
- **Exhaustiveness in `match`:** Yes, require exhaustive cases. Since named indexes ARE tagged unions, the same exhaustiveness rules apply uniformly.
- **Cross-index label equality:** Type error. `m == p` where `m: Maneuver` and `p: Phase` is a compile-time error. They are different tagged union types.
- **Axis order significance:** `T[I, J]` and `T[J, I]` are different types. Axis order determines `for` binding order, `scan` direction, and display order. No transpose operation — use explicit `for` to construct the transposed value.
- **`type` vs `index` for fieldless tagged unions:** Require explicit `index` declaration. A regular `type Foo { A, B }` is NOT usable as a collection axis. The `index` keyword communicates intent and prevents accidental use of marker types as axes.
- **Can `index` types also carry fields?** No. Indexes are fieldless. If you need data-carrying variants as an axis, compose: use an `index` for the axis and a separate `type` for the per-variant data. Too complex for too little value.
- **Named index vs range index keyword:** Both use `index`. The declaration syntax already disambiguates (`{ A, B, C }` vs `range(...)`), and both serve the same role as collection axes with an identical consumption interface (`for`, indexing, aggregation, `scan`).

## Dependencies

- **01-computation-model.md**: The DAG correspondence principle connects types to computation.
- **03-primitives.md** through **07-indexes.md**: This document unifies all type system layers.
- **12-pure-functions.md**: Function entity rules.
- **Phase 5 (indexed values)**: Primary implementation target for DeclType / indexed types.
