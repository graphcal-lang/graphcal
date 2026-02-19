# Type System Stratification

> A unified model of Graphcal's type universe: what types exist, how they compose, and where each kind of entity can appear.

## Status

**Decision level:** Proposal. Supersedes the ad-hoc layering in docs 03-07 with a single coherent model.

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
                     | Indexed(ValueType, Index)    -- written T[I]
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

`T[I]` is a type constructor that lifts a ValueType into a total map from index labels to values. Multi-indexing `T[I, J]` is sugar for `(T[J])[I]`.

## DAG Correspondence

The type stratification has a direct correspondence with the computation model:

> **A node in the evaluation DAG has type `ValueType`. A declaration of type `ValueType[Index]` expands to one DAG node per index label.**

| Declaration type | DAG nodes |
| --- | --- |
| `node x: Velocity` | 1 node |
| `node x: Velocity[Maneuver]` (3 labels) | 3 nodes |
| `node x: Velocity[Phase, Maneuver]` (2 x 3) | 6 nodes |

The `for` comprehension is the mechanism that expands a single declaration into multiple DAG nodes. Each node is independently evaluable (modulo data dependencies), making indexed values naturally parallelizable.

This also explains why arithmetic on indexed values requires explicit `for`: you are defining the computation for each individual DAG node, not operating on the collection as a whole.

## Indexes

An index declares a finite, ordered set of labels. Two flavors, one concept.

### Named Index

```gcl
index Maneuver = { Departure, Correction, Insertion }
```

Labels are identifiers with no value. They have identity (you can compare them) but carry no data.

### Range Index

```gcl
index TimeStep = range(0.0 s, 100.0 s, step: 0.1 s);
```

Labels are scalar values in a specific dimension. They have identity AND a numeric value.

### Unified Interface

Both flavors support the same type constructor `T[I]` and the same operations (`for`, indexing, aggregation, scan). The difference is in what the loop variable can do:

| Capability | Named index label | Range index label |
| --- | --- | --- |
| Indexing: `@x[m]` | Yes | Yes |
| Map literal key | Yes | No (range labels are implicit) |
| Equality comparison: `m == I::Label` | Yes | Yes |
| Pattern matching: `match m { ... }` | Yes | Yes |
| Arithmetic: `t + 0.1 s` | No | Yes (acts as `Scalar(Dim)`) |

### Index Labels Are Not First-Class Values

Index labels exist only within specific syntactic contexts. They are structural identifiers, not data.

**Where labels can appear:**

1. **Map literal keys:** `{ Maneuver::Departure: 2.46 km/s, ... }`
2. **Index arguments:** `@x[Maneuver::Departure]`
3. **`for` bindings:** `for m: Maneuver { ... }` — introduces `m` into the body scope
4. **Comparison / matching within `for`:** `match m { Maneuver::Departure => ..., _ => ... }`

**Where labels CANNOT appear:**

- As function parameters: `fn f(m: Maneuver)` — NO
- As function return types — NO
- As let bindings outside `for`: `let x = Maneuver::Departure` — NO
- As struct field values — NO

This restriction keeps labels as a compile-time / structural concept. If you need variant values as data (passing to functions, storing, pattern matching outside `for`), use tagged unions instead.

### Comparison and matching in `for` bodies

Within a `for` body, the loop variable supports equality comparison and pattern matching. This enables per-label logic that goes beyond what map literals can express:

```gcl
// Per-label logic with match
node delta_v_budget: Velocity[Phase, Maneuver] = for p: Phase, m: Maneuver {
    match (p, m) {
        (Phase::Launch, Maneuver::Departure) => 2.46 km/s,
        (Phase::Cruise, Maneuver::Correction) => 0.05 km/s,
        (Phase::Arrival, Maneuver::Insertion) => 1.48 km/s,
        _ => 0.0 m/s,
    }
}

// Per-label function dispatch
node result: Mass[Maneuver] = for m: Maneuver {
    match m {
        Maneuver::Departure => compute_departure_fuel(@params),
        Maneuver::Correction => compute_correction_fuel(@params),
        Maneuver::Insertion => compute_insertion_fuel(@params),
    }
}
```

This is essential for practical use: multi-axis parameters with sparse special cases, per-label function dispatch, and conditional logic based on which label is being computed.

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

**Multi-axis `for`** (sugar for nested `for`):

```gcl
node matrix: Dimensionless[Row, Col] = for r: Row, c: Col {
    @A[r, c] + @B[r, c]
}
```

### Consumption

**Indexing** — extracts a single element or reduces one axis:

```gcl
@delta_v[Maneuver::Departure]       // Velocity[Maneuver] → Velocity
@matrix[Row::R1]                    // Dimensionless[Row, Col] → Dimensionless[Col]
@matrix[Row::R1, Col::C2]           // Dimensionless[Row, Col] → Dimensionless
```

**Aggregation** — collapses one or more axes:

```gcl
sum(for m: Maneuver { @fuel[m] })   // Mass[Maneuver] → Mass
max(for m: Maneuver { @delta_v[m] })// Velocity[Maneuver] → Velocity
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

- **Index labels:** No `fn f(m: Maneuver)`. Labels are structural, not data.
- **Functions:** No higher-order functions. No `fn apply(f: (A) -> B, x: A) -> B`.
- **Dimensions/units as values:** Dimensions appear as generic params (`<D: Dim>`), not as runtime values.

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
| Indexed value | DeclType | Yes | Yes | Yes | Via `for` |
| Named index label | No | No | No | No | Indexing, comparison in `for` |
| Range index label | No | Partially (scalar in `for`) | No | No | Indexing, arithmetic in `for` |
| Function | No | No | No | No | Calling only |
| Dimension | No (compile-time) | No | As generic `<D: Dim>` | As generic | No |
| Unit | No (compile-time) | No | No | No | In literals only |
| Index | No (compile-time) | No | As generic `<I: Index>` | As generic | No |

## Implications for Existing Design Documents

This document unifies concepts spread across multiple existing docs:

- **03-primitives.md**: Level 1 of this stratification.
- **04-dimensions-and-units.md**: Compile-time metadata on `Scalar` types. Unchanged.
- **05-algebraic-data-types.md**: Level 2. Struct fields must be ValueTypes.
- **06-spaces.md**: Orthogonal — spaces are phantom type parameters on structs. Unchanged.
- **07-indexes.md**: Refines the "two flavors, one concept" model with explicit capability table.
- **10-tables-and-autofill.md**: Superseded. The `table` keyword is replaced by co-indexed declarations. `T[I]` is the table type.
- **12-pure-functions.md**: Clarifies that functions are declarations, not types. `scan` lambda is special syntax.

## Open Questions

- **Option type:** Where does `Option<T>` fit? It is a built-in tagged union (ValueType). `Option<Velocity>` is a ValueType. `Option<Velocity>[Maneuver]` is a DeclType. An indexed value with Option elements can have per-label missing values.
- **Exhaustiveness in `match` on labels:** Should `match m { ... }` in a `for` body require exhaustive cases? Or is a `_` wildcard always sufficient? (Recommendation: require exhaustiveness for safety, consistent with tagged union matching.)
- **Index label equality across indexes:** Can you compare labels from different indexes? `m == p` where `m: Maneuver` and `p: Phase`? (Recommendation: no, type error. Labels are typed by their index.)
- **Nested indexed types:** Is `T[I][J]` always equivalent to `T[I, J]`? What about `(T[I])[J]`? (Current design: yes, `T[I, J]` = `(T[J])[I]`, outermost index first.)
- **Str in indexed types:** Is `Str[Maneuver]` useful? E.g., `param names: Str[Maneuver] = { ... }`. (Yes, this is natural for label metadata.)

## Dependencies

- **01-computation-model.md**: The DAG correspondence principle connects types to computation.
- **03-primitives.md** through **07-indexes.md**: This document unifies all type system layers.
- **12-pure-functions.md**: Function entity rules.
- **Phase 5 (indexed values)**: Primary implementation target for DeclType / indexed types.
