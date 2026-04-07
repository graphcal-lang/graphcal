# Separate Label Indexes from Tagged Unions

> Revert the unification of label indexes and fieldless tagged unions. Introduce a dedicated `Label(IndexName)` type kind. Keep the existing syntactic distinction: bare variants for tagged unions, qualified `Index::Label` for index labels.

## Status

**Decision level:** Draft. Supersedes the "named index = fieldless tagged union" aspect of doc 20.

## Motivation

Design doc 20 unified named index labels with fieldless tagged union variants. While elegant in theory, the unification creates confusion because the two concepts have fundamentally different referencing syntax:

| Context | Label index | Tagged union |
| --- | --- | --- |
| Construction | `Maneuver::Departure` (qualified) | `Nominal` (bare) |
| Match pattern | `Maneuver::Departure => ...` (qualified) | `Nominal => ...` (bare) |
| Map literal key | `Maneuver::Departure: value` | N/A |

The design claims these are "the same thing," but they behave differently everywhere. This is confusing for users ("they're the same but look different?") and for the implementation (two AST nodes, two code paths, `Option<IndexName>` in match patterns).

The root cause is that the concepts are genuinely different:

- **Tagged union variants** are constructors of a sum type. Each variant is its own "thing" — like a function that produces a value of the union type. You refer to them by name directly, just like you call a function by name. `Nominal` constructs a `Status` value, much like `TransferResult { ... }` constructs a `TransferResult` value. This is the Gleam model, where `type` unifies structs and tagged unions.

- **Index labels** are members of a finite ordered set used as collection axes. They don't construct anything — they *identify* a position within an index. They are namespaced under their index (`Maneuver::Departure`) because that's what they are: "the `Departure` label of the `Maneuver` index."

The syntactic difference reflects a real semantic difference. Rather than forcing them into a single concept, we should separate them properly.

### Empirical Evidence

A survey of the codebase shows that label-as-tagged-union-value capabilities are almost entirely unused:

- **Pass to functions:** 0 instances
- **Store in struct fields:** 0 instances
- **Store in `let` bindings:** 0 instances
- **Equality comparison:** 2 test files (works with `Label` type too)
- **Match on loop variable:** 2 test files (works with `Label` type too)

The overwhelming usage is labels as collection keys. The "first-class tagged union value" story from doc 20 is theoretical.

## Design

### Principle: Two Distinct Concepts, Different Syntax, Different Types

**Label indexes** and **tagged unions** are separate concepts with separate syntax, separate type representations, and separate registries.

| Aspect | Tagged union | Label index |
| --- | --- | --- |
| Declaration | `type Status { Nominal, Warning { code: D } }` | `index Maneuver = { Departure, Correction, Insertion };` |
| Variant syntax | Bare: `Nominal`, `LowThrust { ... }` | Qualified: `Maneuver::Departure` |
| Match syntax | Bare: `Nominal => ...` | Qualified: `Maneuver::Departure => ...` |
| Semantic role | Constructor of a sum type value | Identifier within a collection axis |
| Type kind | `TaggedUnion(name, variants)` | `Label(IndexName)` |
| Can carry fields | Yes | No |
| Can be collection axis | No | Yes |

### Syntax: Unchanged

The existing syntax is preserved exactly. No user-facing syntax changes are needed.

**Tagged union variants** use bare names — consistent with Gleam's model where each variant is a constructor:

```gcl
type ManeuverKind {
    Impulsive { delta_v: Velocity }
    LowThrust { thrust: Force, duration: Time }
}

type Status {
    Nominal
    Warning { code: Dimensionless }
}

// Construction: bare variant name (like calling a constructor)
node maneuver: ManeuverKind = LowThrust {
    thrust: 0.5 N,
    duration: 3600.0 s,
};
node status: Status = Nominal;

// Match: bare variant name
node code: Dimensionless = match @status {
    Nominal => 0.0,
    Warning { code } => code,
};
```

**Index labels** use qualified names — consistent with their role as namespaced identifiers:

```gcl
index Maneuver = { Departure, Correction, Insertion };

// Map literal keys
param delta_v: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km/s,
    Maneuver::Correction: 0.12 km/s,
    Maneuver::Insertion: 1.83 km/s,
};

// Index access
node departure_dv: Velocity = @delta_v[Maneuver::Departure];

// Match on loop variable
node scaled: Velocity[Maneuver] = for m: Maneuver {
    match m {
        Maneuver::Departure => @delta_v[m] * 2.0,
        Maneuver::Correction => @delta_v[m] * 0.5,
        Maneuver::Insertion => @delta_v[m] * 1.5,
    }
};
```

**Single-variant types** (structs) also use bare names, consistent with the Gleam model:

```gcl
type TransferResult {
    dv1: Velocity,
    dv2: Velocity,
}

node result: TransferResult = TransferResult { dv1: 100.0 m/s, dv2: 200.0 m/s };
```

### `Label(IndexName)` as a Distinct Type Kind

Label index labels get their own type kind, separate from tagged unions.

#### Type Stratification (revised from doc 20)

```
Level 1: Primitive   = Scalar(Dim) | Int | Bool
Level 2: ValueType   = Primitive
                     | Struct(name, fields: [ValueType])
                     | TaggedUnion(name, variants: [Variant(fields: [ValueType])])
Level 3: DeclType    = ValueType
                     | Indexed(ValueType, [Index])

Expression-level:
  Label(IndexName)   -- not a ValueType or DeclType; exists only in expressions
```

`Label(IndexName)` is an **expression-level type** — it exists within the type checker and at runtime, but cannot appear in type annotations for `param`, `node`, or `const node` declarations. You cannot write `node m: Maneuver = Maneuver::Departure;`. Labels arise as loop variables in `for` comprehensions, function parameters, `let` bindings, and intermediate values in expressions (comparisons, match scrutinees, index access arguments). It supports:

| Operation | `Label(I)` | `TaggedUnion` |
| --- | --- | --- |
| Equality comparison (`==`, `!=`) | Yes | Yes |
| Pattern matching (`match`) | Yes | Yes |
| Index access (`@x[label]`) | Yes | No |
| Map literal key | Yes | No |
| `for` loop variable | Yes | No |
| Pass to function | Yes | Yes |
| Return from function | Yes | Yes |
| Store in struct field | Yes | Yes |
| Field access (`.field`) | No | Yes |
| Carry data (variant fields) | No | Yes |

Labels are proper values — they can be compared, matched, passed to functions, and stored in struct fields. They are just not tagged union variants.

### Separate Registries

A `index` declaration creates **only** an index registry entry. It does **not** register a `TypeDef` in the type registry.

```
index Maneuver = { Departure, Correction, Insertion };
  → IndexDef in index registry          ✓
  → TypeDef in type registry            ✗ (removed)

type Status { Nominal, Warning { code: Dimensionless } }
  → TypeDef in type registry            ✓
  → IndexDef in index registry          ✗ (as before)
```

The index registry stores:

```rust
pub enum IndexKind {
    Named { variants: Vec<VariantName> },
    Range { start, end, step, dimension, display_label, display_scale },
}

pub struct IndexDef {
    pub name: IndexName,
    pub kind: IndexKind,
}
```

The type registry stores tagged unions and structs only. The `variant_to_type` reverse lookup no longer contains index label entries.

### Revised Type System Representation

```rust
// Inferred types (dim_check)
pub enum InferredType {
    Scalar(Dimension),
    Bool,
    Int,
    Label(IndexName),                    // NEW — type of an index label
    Struct(StructTypeName, Vec<Self>),
    Indexed { element: Box<Self>, index: IndexName },
}

// Declared types (dim_check)
pub enum DeclaredType {
    Scalar(Dimension),
    Bool,
    Int,
    Label(IndexName),                    // NEW
    Struct(StructTypeName, Vec<Self>),
    Indexed { element: Box<Self>, index: IndexName },
}

// TIR resolved types
pub enum ResolvedTypeExpr {
    Dimensionless,
    Bool,
    Int,
    Label(IndexName, Span),              // NEW
    Scalar(Dimension),
    Struct(StructTypeName, Span),
    GenericStruct { name, type_args, span },
    GenericDimParam(GenericParamName, Span),
    GenericDimExpr { terms, span },
    Indexed { base: Box<Self>, indexes: Vec<ResolvedIndex> },
}
```

### Revised Runtime Representation

```rust
pub enum RuntimeValue {
    Scalar(f64),
    Bool(bool),
    Int(i64),
    Label {                              // NEW — replaces Struct-with-empty-fields for labels
        index_name: IndexName,
        variant: VariantName,
    },
    Struct {
        type_name: StructTypeName,
        variant: VariantName,
        fields: IndexMap<FieldName, Self>,
    },
    Indexed {
        index_name: IndexName,
        entries: IndexMap<VariantName, Self>,
    },
    RangeLabel {
        step_index: usize,
        value: f64,
    },
}
```

### AST: Retain Distinct Nodes

Since the syntax is different (bare vs qualified), the AST keeps distinct representations:

```rust
pub enum ExprKind {
    // Index label literal: Maneuver::Departure (qualified, no fields)
    VariantLiteral {
        index: Spanned<IndexName>,
        variant: Spanned<VariantName>,
    },

    // Tagged union / struct construction: Nominal, LowThrust { ... }, TransferResult { ... }
    StructConstruction {
        type_name: Spanned<StructTypeName>,
        type_args: Vec<TypeExpr>,
        fields: Vec<(Spanned<FieldName>, Expr)>,
    },

    // ...
}
```

The parser continues to distinguish the two based on syntax:

- `PascalCase::PascalCase` → `VariantLiteral` (index label)
- `PascalCase { ... }` or `PascalCase` alone → `StructConstruction` (tagged union variant or struct)

The match pattern similarly retains the `qualified_index` field, but its meaning changes from "optional index qualifier" to "this is an index label pattern (vs a tagged union pattern)":

```rust
pub struct MatchPattern {
    /// `Some` for index label patterns: `Maneuver::Departure`
    /// `None` for tagged union patterns: `Nominal`, `Warning { code }`
    pub qualified_index: Option<Spanned<IndexName>>,
    pub variant_name: Spanned<VariantName>,
    pub bindings: Vec<PatternBinding>,
    pub span: Span,
}
```

This is no longer a code smell — the `Option` reflects a genuine semantic distinction.

## Examples

### Mixed Usage

```gcl
index Phase = { Coast, Burn };

type BurnStrategy {
    Impulsive { delta_v: Velocity }
    LowThrust { thrust: Force }
}

// Label index for the axis, tagged union for the value
param strategy: BurnStrategy[Phase] = {
    Phase::Coast: Impulsive { delta_v: 0.0 m/s },
    Phase::Burn: LowThrust { thrust: 0.5 N },
};

node thrust: Force[Phase] = for p: Phase {
    match @strategy[p] {
        Impulsive { delta_v: _ } => 0.0 N,
        LowThrust { thrust } => thrust,
    }
};
```

Note the natural reading: `Phase::Coast` is an index label (qualified), while `Impulsive { ... }` and `LowThrust { ... }` are tagged union constructors (bare).

### Label in Function Parameter

```gcl
index Maneuver = { Departure, Correction, Insertion };

fn maneuver_fuel(m: Maneuver, exhaust_vel: Velocity, dv: Velocity) -> Mass =
    1000.0 kg * (exp(dv / exhaust_vel) - 1.0);
```

Here `m: Maneuver` in the function signature means "a label of the `Maneuver` index." The type is `Label(Maneuver)`, not `Struct(Maneuver)`.

## Resolved Questions

### Can a tagged union be used as an index?

**No.** Indexes and types are separate concepts. If you need both, declare both:

```gcl
index Maneuver = { Departure, Correction, Insertion };
type ManeuverKind { Departure, Correction, Insertion }
```

In practice this need has not arisen.

### Can an index and a type share the same name?

**No.** Index names and type names share a single namespace. Declaring both `index Foo = { A };` and `type Foo { A }` is a compile-time error.

### How does the parser know if a bare PascalCase name is a tagged union variant or something else?

The parser does not resolve this — it produces `StructConstruction` with empty fields for bare PascalCase names (like `Nominal`). Type checking validates that the name is a known variant of the expected type. If it's not a known variant, an error is reported.

This is the same as today. No parser changes are needed.

### How does type checking resolve bare variant names?

When type-checking an expression like `node status: Status = Nominal`, the checker:

1. Sees `StructConstruction { type_name: "Nominal", fields: [] }`.
2. Looks up `Nominal` in the type registry — not found as a type name.
3. Looks up `Nominal` in the `variant_to_type` reverse map — finds it belongs to `Status`.
4. Validates that the expected type matches.

This is unchanged from today, except the `variant_to_type` map no longer contains index label entries.

### What about the formatter adding `{ }` to bare variants?

This behavior (`Nominal` → `Nominal { }`) should be revisited independently. It's a formatter concern unrelated to this design change. The natural formatting for a fieldless variant is without braces: `Nominal`.

## Impact on Existing Features

### Type System Stratification (doc 20)

The three-level model is preserved. `Label(IndexName)` is added as a separate expression-level type, outside the three levels. It is not a ValueType (cannot appear in declarations) and not a DeclType. The claim "named index = fieldless tagged union" is retracted. The entity map table gains a row for `Label(IndexName)` and the row for "Named index label" no longer says "same as tagged union variant."

### Functions (doc 12)

Functions can accept `Label(I)` parameters. In a type annotation, an index name refers to the `Label` type:

```gcl
fn f(m: Maneuver) -> Velocity { ... }
// m has type Label(Maneuver)
```

### LSP Features

- **Inlay hints:** Show `Label(Maneuver)` or just `Maneuver` (as a label type) instead of `Struct(Maneuver)` for loop variables.
- **Go to definition:** `Maneuver::Departure` goes to the `index` declaration. `Nominal` goes to the variant in the `type` declaration.
- **Hover:** Shows "label of index `Maneuver`" vs "variant of type `Status`."

### Tree-sitter Grammar

No changes needed — the grammar already supports both bare and qualified syntax.

### Formatter

No changes needed for this design. The `{ }` appending issue is a separate concern.

## Implementation Plan

### Phase 1: Introduce `Label` Type Kind

1. Add `Label(IndexName)` to `InferredType`, `DeclaredType`, `ResolvedTypeExpr`.
2. Add `RuntimeValue::Label { index_name, variant }` variant.
3. Update dim_check to infer `Label(I)` for `ExprKind::VariantLiteral` (index label literals).
4. Update dim_check to assign `Label(I)` to `for` loop variables over named indexes.
5. Update eval to produce `RuntimeValue::Label` for index label literals.
6. Update equality comparison to handle `Label` vs `Label`.
7. Update match expression handling for `Label` patterns.

### Phase 2: Decouple Registries

1. Remove `TypeDef` registration from `index` declaration processing in `ir.rs`.
2. Remove index label entries from `variant_to_type` reverse lookup in the type registry.
3. Update type resolution to look up index names in the index registry (not type registry) when resolving type annotations like `m: Maneuver`.
4. Update any code that assumes index labels are in the type registry.

### Phase 3: LSP and Tooling Updates

1. Update LSP hover, go-to-definition, inlay hints for the new type kind.
2. Update error messages that mention "tagged union" for index labels.
3. Update diagnostic related information.

### Phase 4: Documentation

1. Update `docs/language/type-system.md` to describe `Label` as a separate type kind.
2. Update `docs/language/indexes.md` to clarify that index labels are not tagged union variants.
3. Update `docs/language/algebraic-data-types.md` if it mentions index labels.
4. Update `README.md` examples if needed.

## Dependencies

- **20-type-system-stratification.md:** Partially superseded (the "named index = fieldless tagged union" section). The three-level model and everything else in doc 20 remains valid.
- **07-indexes.md:** Updated to clarify indexes are not types.
- **05-algebraic-data-types.md:** No changes needed (it doesn't mention indexes).
- **12-pure-functions.md:** Updated to show `Label(I)` as a valid parameter type.
