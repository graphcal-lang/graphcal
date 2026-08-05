---
icon: material/format-list-bulleted-type
---

# Type System

This page is the formal reference for Graphcal's type system. It describes the complete entity stratification — kinds, type-level entities, the three type levels, and the term level — along with the dimension algebra, typing rules for expressions, generics, and type equivalence.

For introductory material, see the [tutorial](../tutorial/index.md). For specific features, see [Dimensions & Units](dimensions-and-units.md), [Algebraic Data Types](algebraic-data-types.md), [Indexes](indexes.md), and [DAG Blocks](functions.md).

## Type-Level Domains and Notation

The capital letters in the definitions below are **metavariables**, not names
that exist implicitly in graphcal source:

- `D` ranges over **dimensions**: canonical products of base dimensions with
  rational exponents. `Dimensionless` is the identity dimension. In a generic
  declaration, `<D: Dim>` introduces a source-level variable over this domain.
- `S` ranges over the closed set of supported **time scales**: `UTC`, `TAI`,
  `TT`, `TDB`, `ET`, `GPST`, `GST`, `BDT`, and `QZSST`. `S` is semantic
  notation only; graphcal does not currently have a `TimeScale` generic kind.
- `A` ranges over names introduced by `type` declarations.
- `Gᵢ` ranges over generic arguments accepted by `A`'s corresponding generic
  parameter. Each argument is checked against that parameter's kind (`Dim`,
  `Type`, `Index`, or `Nat`).
- `Iᵢ` ranges over finite, ordered **index axes**. An axis can come from an
  `index` declaration or from an explicit structural index such as `Fin(3)` in
  `T[Fin(3)]`.
- `J̄` ranges over a possibly empty ordered sequence of state axes. In recurrence
  rules, `T[J̄]` means `T` when the sequence is empty and
  `T[J₁, ..., Jₙ]` otherwise; a comma before an empty `J̄` is omitted, so
  `T[I, J̄]` becomes `T[I]`.
- `N` ranges over type-level **natural numbers**, used as `Fin` cardinalities
  and `Nat` generic arguments. In a generic declaration, `<N: Nat>` introduces
  a source-level variable over this domain.

`Quantity(D)`, `Complex(D)`, `Key(I)`, and `Datetime(S)` use parentheses only
as semantic notation; they are not constructor calls or literal source syntax.
The source types for `Complex(D)` and `Key(I)` are `Complex<D>` and `Key<I>`.
`A<G₁, ..., Gₙ>` and
`T[I₁, ..., Iₘ]` mirror graphcal's source syntax. Angle brackets apply a
generic algebraic type, while square brackets index a value type over one or
more axes.

`Dim`, `Type`, `Index`, and `Nat` are **generic parameter kinds**, not value
types. They are defined precisely in [Generic Parameter Kinds](#generic-parameter-kinds).
In kinding notation:

```text
D : Dim        D is a dimension
S : TimeScale  S is a supported time scale (semantic notation only)
T : Type       T is a single-value type, called a ValueType below
I : Index      I is a finite, ordered index axis
N : Nat        N is a type-level natural number
```

The built-in and user-declared type-level formers have these kind-level
signatures. The arrows are not graphcal function types:

```text
Quantity : Dim -> Type
Complex  : Dim -> Type
Key      : Index -> Type
Int      : Type
Bool     : Type
Datetime : TimeScale -> Type
A        : K₁ × ... × Kₙ -> Type     for generic kinds Kᵢ declared by A
Fin      : Nat -> Index               with the validity obligation N > 0
range    : Quantity(D) × Quantity(D) × Quantity(D) -> Index   (start, end, step:)
linspace : Quantity(D) × Quantity(D) × Nat -> Index           (start, end, points:)
_(min: _, max: _) : Quantity(D) | Int | Datetime(S) -> ConstrainedType
_[_]     : ConstrainedType × one-or-more Index axes -> DeclType
```

Thus `Type` is the kind whose inhabitants are single-value types; this page
uses **ValueType** for those inhabitants. `Index` is the kind whose inhabitants
are axes, not labels or indexed collections. `range` and `linspace` are legal
only on the right-hand side of an `index` declaration, while `Fin(N)` may be
written inline in any Index position. Every ValueType is a ConstrainedType
with no bounds, so plain `T[I]` is the common indexed form; both
ConstrainedType and DeclType are defined in
[Entity Stratification](#entity-stratification).

## Entity Stratification

Every entity in a Graphcal program belongs to exactly one layer of one
universe. The grammar below is the whole picture in one place: the kinds, the
type-level entities they classify, the three type levels, and the term level
that inhabits them.

```text
Kinds:   Dim | TimeScale | Type | Index | Nat

Level 0: type-level entities, classified by kinds
  D : Dim        = B | Dimensionless | D * D | D / D | D^(p/q)
  S : TimeScale  = UTC | TAI | TT | TDB | ET | GPST | GST | BDT | QZSST
  N : Nat        = 0 | 1 | 2 | ... | N + N | N * N
  I : Index      = X | C | Fin(N)                                (N >= 1)

Level 1: Primitive = Quantity(D) | Complex(D) | Key(I) | Int | Bool | Datetime(S)

Level 2: ValueType (T : Type) = Primitive | A | A<G₁, ..., Gₙ>
         GenericArg         G = D | T | I | N

Level 3: DeclType             = CT | CT[I₁, ..., Iₘ]             (m >= 1)
         ConstrainedType   CT = T | T(min: b, max: b)

Term level: runtime entities, classified by types
  v : T                a single value — one DAG node
  w : T[I₁, ..., Iₘ]   an indexed value — a total map, one v per label tuple
```

Additional metavariables in this grammar:

- `B` ranges over **base dimensions**: `base dim` declarations and the
  prelude's base dimensions (the seven SI base dimensions plus `Angle`).
- `X` ranges over **named indexes**, each declaring ordered labels
  `X.ℓ₁, ..., X.ℓₖ`.
- `C` ranges over **coordinate indexes**: finite ordered `Quantity(D)`
  coordinate sequences built by `range` or `linspace`.
- `b` ranges over compile-time constant bound expressions; either bound of a
  constraint may be omitted.

Every type-level domain also contains the **generic parameters** of its kind
that are in scope: inside `type Vec3<D: Dim, I: Index, N: Nat, F: Type>`, the
parameter `D` is a dimension, `I` is an index axis, `N` is a Nat, and `F` is a
ValueType. `TimeScale` is the one kind with neither generic parameters nor
user declarations: its nine inhabitants are closed and appear only in
`Datetime<S>` type syntax and static `epoch<S>` arguments.

The formers that connect these layers — from `Quantity : Dim -> Type` down to
`_[_] : ConstrainedType × Index axes -> DeclType` — are listed with kind-level
arrow signatures in
[Type-Level Domains and Notation](#type-level-domains-and-notation); their
term-level counterparts appear in [The Term Level](#the-term-level).

Entities that live *outside* this tower — units, labels, constructors,
functions, declarations, modules — are cataloged in
[Declarations and the Entities They Introduce](#declarations-and-the-entities-they-introduce)
and [Built-in, Local, and Boundary Entities](#built-in-local-and-boundary-entities)
below.

### Level 0: Type-Level Entities

These entities exist only while the program is checked. None of them is a
value, and each is classified by a kind rather than by a type.

**Dimensions** (`D : Dim`) form an algebra over base dimensions, with
`Dimensionless` as the identity:

```text
_ * _    : Dim × Dim -> Dim
_ / _    : Dim × Dim -> Dim
_ ^(p/q) : Dim -> Dim                for each non-zero rational p/q
```

A dimension's canonical form is a product of base dimensions with rational
exponents; named derived dimensions such as `Velocity` are transparent
aliases. See [Dimension Algebra](#dimension-algebra).

**Time scales** (`S : TimeScale`) are the closed nine-member set above. `S` is
semantic notation only: there is no source-level `TimeScale` kind and no
time-scale variable.

**Nats** (`N : Nat`) are type-level natural numbers: literals, `Nat` generic
parameters, and polynomial expressions over them:

```text
_ + _ : Nat × Nat -> Nat
_ * _ : Nat × Nat -> Nat
```

There is no subtraction; express the larger side additively (`Fin(N + 1)`
input, `Fin(N)` output). Nat expressions are normalized, and two Nats are
equal exactly when their normal forms coincide. Nats appear in exactly two
roles: `Fin(N)` cardinalities and `Nat` generic arguments.

**Index axes** (`I : Index`) come in three concrete forms, each carrying an
intrinsic element order:

| Form | Declared by | Elements | Element order |
|------|-------------|----------|---------------|
| Named `X` | `index X = { ℓ₁, ..., ℓₖ };` | labels `X.ℓᵢ` | label declaration order |
| Coordinate `C` | `index C = range(...);` or `linspace(...)` | `Quantity(D)` coordinates | coordinate order from start toward end |
| Structural `Fin(N)` | written in place in Index positions | integer positions `0 ... N-1` | ascending position |

The index formers `range`, `linspace`, and `Fin` carry the kind-level arrow
signatures listed in
[Type-Level Domains and Notation](#type-level-domains-and-notation); a named
index has no former — its label set is the declaration itself.

`Fin(0)` is invalid and a cardinality beyond the compiler's practical limit is
rejected, so every axis is finite and non-empty. The elements of an axis stay
type-level entities, but each is uniformly reflected at the term level as a
**key** of type `Key(I)` (see [Index Capabilities](#index-capabilities)):
a qualified label is a `Key(X)` constant, and coordinate and `Fin` loop
variables bind `Key(C)` / `Key(Fin(N))` values whose coordinate or integer
content is extracted explicitly with `coord` / `to_int`.

### Levels 1–3: The Type Hierarchy

- **Primitive (Level 1)** — an indivisible atomic datum: a real quantity, a
  dimension-aware complex quantity, an index key, an integer, a boolean, or a
  datetime in one time scale.
- **ValueType (Level 2)** — a single logical value: a primitive or an
  instance of a nominal algebraic type. This is the domain of kind `Type` —
  what one DAG node stores, one DAG parameter carries, and expressions
  compute.
- **ConstrainedType** — a ValueType optionally refined by inclusive
  `min`/`max` domain constraints. Constraints attach only to quantity, `Int`,
  and `Datetime<S>` types. See [Domain Constraints](#domain-constraints).
- **DeclType (Level 3)** — what a type annotation denotes: a constrained
  type, optionally indexed by one or more axes. Annotations appear on
  `param`, `node`, and `const node` declarations, DAG parameters, and
  constructor payload fields. Indexing lifts one ValueType into a total map
  from label tuples to values; constraints on an indexed annotation apply
  element-wise.

A bare `A` is a non-generic algebraic type (or a generic type whose arguments
all have defaults). Every `type` declaration defines this same nominal
algebraic-type concept, regardless of whether it has one constructor or many.
Constructors and their payload fields describe how values of `A` are formed;
they are not separate types and therefore do not appear as separate cases in
the `ValueType` definition.

Generic arguments `G` are kind-checked, never classified by spelling: each
argument position expects the declared `Dim`, `Type`, `Index`, or `Nat` kind,
and crossing kinds is an error (`ByNat<Fin(3)>`, `Dimensionless[3]`). The
`Type` kind contains exactly the ValueTypes: an index axis or an indexed
DeclType such as `Velocity[Maneuver]` is not a legal `Type` argument.

### Dimension-Aware Complex Quantities

`Complex : Dim -> Type` is a built-in primitive type former. A value of
`Complex<D>` stores real and imaginary binary64 components in SI base units,
and both components have the same dimension `D`:

```graphcal
node displacement: Complex<Length> = complex(3.0 m, 4.0 m);
node dimensionless: Complex<Dimensionless> = complex(1.0, -2.0);
```

The dimension argument is mandatory and is a dimension, not a unit:
`Complex<Length>` and `Complex<Length / Time>` are valid; bare `Complex` and
`Complex<m>` are errors. Generic dimension parameters can appear directly:

```graphcal
type Phasor<D: Dim> {
    Phasor(value: Complex<D>),
}
```

Graphcal has no complex literal syntax. Construction is explicit through
`complex(re, im)`, `polar(magnitude, phase)`, or `to_complex(real)`. Explicit
construction prevents an identifier such as `i` from acquiring special
lexical meaning and makes real-to-complex promotion visible. See
[Complex Functions](built-ins.md#complex-functions) and the
[complex arithmetic matrix](expressions.md#complex-arithmetic).

A complex value may be indexed and may be carried in algebraic payloads.
`count` accepts an indexed complex value because it is element-type agnostic;
quantity aggregations and linear-algebra kernels do not yet accept complex
elements. Complex values also do not cross the experimental plugin ABI. Plot
`re(z)`, `im(z)`, `abs(z)`, or `phase(z)` rather than plotting `z` directly.

### Required Entities (Holes)

Dimensions, algebraic types, and named or coordinate indexes may be declared
*required* — `pub(bind) dim D;`, `pub(bind) type T;`, `pub(bind) index X;`,
`pub(bind) index C: Time;` — introducing an abstract member of the
corresponding universe with no definition. A required entity is used like a
concrete one inside its library or local `dag` block and is bound to a concrete
entity by name in an `include` binding, the same binding surface that supplies
`param` values: the include boundary is where both term-level and type-level
arguments cross. This explicit required-entity pattern provides dimension- and
axis-polymorphic reusable DAGs without implicit generic inference.
Units and time scales have no required form. See
[Visibility, Bindability, and Input Ports](multi-file.md#visibility-bindability-and-input-ports).

### The Term Level

Values inhabit ValueTypes. A value is a real or complex quantity, an integer,
a boolean, a datetime, or an algebraic value built by a constructor; an
indexed value is a total map with one value per label tuple of its axes. Two
pieces of display metadata ride on values without affecting their types: a
real or complex quantity's **unit** and a datetime's **timezone**. Both select
rendering only — real and complex components are stored in SI base units,
instants in their time scale.

The structural value formers and eliminators have these signatures, where
`ℓᵢ` is a label of axis `Iᵢ`, `U` is the recurrence state's element ValueType,
and `J̄` is its possibly empty sequence of fixed axes:

```text
Cᵢ          : DT₁ × ... × DTₖ -> A                value former, one per constructor of A
map / table : T at each label tuple -> T[I₁, ..., Iₘ]
for         : T at each label tuple -> T[I₁, ..., Iₘ]
_[_]        : T[I₁, ..., Iₘ] × Key(I₁) × ... × Key(Iₘ) -> T
_.fᵢ        : A -> DTᵢ                            single-constructor A only
scan        : T[I] × U[J̄] × (U[J̄] × T -> U[J̄]) -> U[I, J̄]
unfold      : I × U[J̄] × (U[J̄] × Key(I) × Key(I) -> U[J̄]) -> U[I, J̄]
sum, maximum, minimum, mean, rss : Quantity(D)[I] -> Quantity(D)
argmax, argmin : Quantity(D)[I] -> Key(I)
product     : Quantity(D)[I] -> Quantity(D^|I|)
count       : T[I] -> Int
X.ℓ         : Key(X)                              label expression, one per label of X
key         : Fin(N) × static Nat -> Key(Fin(N))  compile-time range check
fin_key     : Fin(N) × Int -> Key(Fin(N))         runtime range check
floor_key, ceil_key, nearest_key : C × Quantity(D) -> Key(C)
coord       : Key(C) -> Quantity(D)               C a coordinate axis over D
to_int      : Key(Fin(N)) -> Int                  Fin keys only
_ + _       : Key(Fin(N)) × static Nat c -> Key(Fin(N + c))
dot         : Quantity(D1)[I] × Quantity(D2)[I] -> Quantity(D1 × D2)
matmul      : Quantity(D1)[I, J] × Quantity(D2)[J, K] -> Quantity(D1 × D2)[I, K]
transpose   : Quantity(D)[I, J] -> Quantity(D)[J, I]
trace       : Quantity(D)[I, I] -> Quantity(D)
norm        : Quantity(D)[I] -> Quantity(D)
cross       : Quantity(D1)[I₃] × Quantity(D2)[I₃] -> Quantity(D1 × D2)[I₃]
outer       : Quantity(D1)[I] × Quantity(D2)[J] -> Quantity(D1 × D2)[I, J]
solve       : Quantity(D1)[I, I] × Quantity(D2)[I] -> Quantity(D2 / D1)[I]
inverse     : Quantity(D)[I, I] -> Quantity(D⁻¹)[I, I]
det         : Quantity(D)[I, I] -> Quantity(D^|I|)
```

As everywhere on this page, the arrows are semantic notation, not function
types. Constructor calls always spell field names (`Cᵢ(f₁: v₁, ...)`), and a
constructor of a generic `A` produces the applied `A<G₁, ..., Gₙ>`. The
closure positions in `scan` and `unfold` are special syntax, not function
values; `unfold`'s first argument is an explicit reference to a coordinate
index over dimension `D`, and its closure binds the previous and current
element keys `Key(I)`. Recurrence state may carry the fixed axes `J̄`; the
recurrence axis is prepended to them, without making an indexed declaration
type an inhabitant of the `Type` kind. Map and table literals must be total and are
normalized to index order, element access must supply a key for every axis
(a qualified label is the constant-key spelling), and aggregations —
including `argmax`/`argmin`, which tie-break to the first extremum in index
order — reduce exactly one axis. `key`'s static position and Fin-key `+` are
checked while the program is compiled; `fin_key`, `floor_key`, and
`ceil_key` range-check at evaluation time. Linear-algebra contractions
require the same typed axis at each paired position; `I₃` denotes an axis
with exactly three entries.

Term-level names are bound by `param` (a named DAG input port), `node` (a
computed value), and `const node` (a compile-time value); a
multi-declaration binds several such slots from one shared table literal.
Local names — loop variables, `match` bindings, and `scan`/`unfold` closure
variables — bind values (element keys, accumulator or element values) inside
a single expression.
There are no first-class functions at the term level: built-ins, extern
functions, and DAG blocks are called or instantiated, never stored.

### DAG Correspondence

The stratification connects directly to the computation model:

> A node in the evaluation DAG has type **ValueType**. A declaration of type `ValueType[Index]` expands to one DAG node per index label. A declaration of type `ValueType[I, J]` expands to one DAG node per label tuple `(i, j)`.

| Declaration type | DAG nodes |
|-----------------|-----------|
| `node x: Velocity` | 1 node |
| `node x: Velocity[Maneuver]` (3 labels) | 3 nodes |
| `node x: Velocity[Phase, Maneuver]` (2 x 3) | 6 nodes |

The `for` comprehension expands a single declaration into multiple DAG nodes. Each node is independently evaluable (modulo data dependencies), making indexed values naturally parallelizable. This also explains why element-wise arithmetic and comparison on indexed values require explicit `for`: you are defining the computation for each individual DAG node, not operating on the collection as a whole.

### Declarations and the Entities They Introduce

Each declaration form introduces entities into one universe. The
[name universes](#name-universes) below keep leaf names exclusive per scope;
this table is the complete inventory of declaration forms:

| Declaration | Introduces | Universe | Classified by |
|-------------|-----------|----------|---------------|
| `base dim Length;` | a base dimension | type | `Dim` |
| `dim Velocity = Length / Time;` | a derived dimension (transparent alias) | type | `Dim` |
| `base unit m: Length;` | a base unit | unit | its dimension |
| `const unit km: Length = 1000 m;` | a compile-time-scaled unit | unit | its dimension |
| `unit EUR: Money = (@rate) USD;` | a runtime-scaled unit | unit | its dimension |
| `type A<...> { C1(...), C2, ... }` | an algebraic type and its constructors | type; constructors in the constructor namespace | `Type`; constructors form values of `A` |
| `index X = { ... };` | a named axis and its labels | index; labels qualified under `X.` | `Index` |
| `index C = range(...);` / `linspace(...);` | a coordinate axis | index | `Index` |
| `pub(bind) dim D;` / `type T;` / `index X;` / `index C: D;` | a required entity (hole) | type or index | its kind |
| `param p: DT;` / `param p: DT = expr;` | a named DAG input port | value | its DeclType |
| `node n: DT = expr;` | a computed graph value | value | its DeclType |
| `const node k: DT = expr;` | a compile-time value | value | its DeclType |
| `param a: T[I], node b: U[I] = table[...] { ... };` | several ports/values sharing axes (multi-declaration) | value | per-slot DeclTypes |
| `assert a = expr;` | a checked assertion | value | body: `Bool`, `Bool[I]`, or `actual ~= expected +/- tolerance` |
| `plot p = { ... };` | a chart specification | value | — |
| `figure f = { ... };` / `layer l = { ... };` | plot compositions (tiled / overlaid) | value | — |
| `dag d { ... }` | a reusable sub-DAG blueprint | value | — (there is no function type) |
| `import pkg.mod;` / `... as m;` / `....{ items }` | a callable DAG-module alias or the listed items | module/DAG or per item | — |
| `import plugin "name" as ns { fn ...; }` | extern functions, callable as `ns.fn(...)` | plugin alias | extern signatures over `Dim`/`Index` variables |
| `include pkg.dag(bindings) ...;` | an embedded DAG instance; selected outputs as nodes | value | outputs' DeclTypes |
| `<P: Dim>` etc. in a `type` header | a generic parameter | scoped to its declaration | its declared kind |

### Built-in, Local, and Boundary Entities

The remaining entities are built into the language, local to one expression,
or exist only at construction and rendering boundaries:

| Entity | Comes from | Classified by | Where it appears |
|--------|-----------|---------------|------------------|
| Prelude dimensions and units | prelude | `Dim` / their dimension | type syntax; literals and conversion targets |
| Time scales `UTC` ... `QZSST` | built-in closed set | `TimeScale` (semantic) | `Datetime<S>` type syntax, `epoch<S>` |
| Built-in constants `PI`, `E`, `TAU` | prelude | `Dimensionless` | expressions, referenced bare |
| Built-in functions (`sqrt`, `sum`, ...) | prelude | dimension-polymorphic signatures | call position only |
| `scan` / `unfold` | special syntax forms | their typing rules | expressions; their closures are syntax, not values |
| Index label `X.ℓ` | named-index declaration | `Key(X)` as an expression; a selector in pattern-like positions | expressions, index access, map/table keys, `match` patterns, `#[expected_fail(...)]` keys, include index bindings |
| Coordinate | coordinate-index declaration | `Key(C)`; coordinate exposed via `coord` | via loop variables and coordinate searches: indexing, equality, `coord` extraction |
| `Fin` position | structural axis | `Key(Fin(N))`; integer exposed via `to_int` | via loop variables, `key`, and `fin_key`: indexing, equality, additive key arithmetic |
| Constructor | `type` declaration | forms values of its algebraic type | construction calls and `match` patterns |
| Payload field | constructor declaration | its DeclType | `Ctor(field: expr)` construction, `.field` access, pattern bindings |
| Loop variable | `for v: I` | `Key(I)` for the iterated axis | the comprehension body |
| `match` binding | `field: name` in a pattern | the bound field's type | the arm expression |
| Closure variables | `scan` binders `acc`, `item`; `unfold` binders `prev_state`, `prev_i`, `i` | accumulator / element values; `unfold`'s `prev_i`, `i` are `Key(I)` | the `scan` / `unfold` body |
| Attributes | `#[...]` before a declaration | closed set: `assumes`, `expected_fail`, `hidden`, `lazy` | declaration metadata |
| String literals | single-line quoted text | — (there is no `String` ValueType) | `datetime`/`epoch` arguments, timezone display targets, plot properties, plugin paths |
| Timezone | quoted IANA name | validated against the bundled tzdb | `datetime(..., tz)` construction, `-> "Area/Location"` display |
| Module / package | directory tree and `import` | — | dot-separated path prefixes |

## Name Universes

Graphcal keeps three declaration universes exclusive inside one scope:

- **Type universe**: algebraic types and dimensions.
- **Index universe**: named and coordinate index declarations.
- **Value universe**: `param`, `node`, `const node`, `assert`, plot, figure,
  layer, and `dag` declarations.

A leaf name can be declared in only one of those universes in the same scope.
For example, `type M { Mk }` and `index M = { A }` are a duplicate-name error,
as are `dim M = Length;` with either `type M { ... }` or `index M = { ... }`.

Constructors live in their own constructor namespace. This is why a record-like
declaration can still use the conventional `type T { T(...) }` spelling: the
type name and its constructor name do not compete in type, index, or value
positions.

Units are referenced only in unit syntax, such as unit literals and conversion
targets. A unit name can match a type, index, or value leaf without changing
which declaration a type, index, or value position resolves.

## Type Categories

### Primitives (Level 1)

The indivisible types. Each represents a single atomic datum.

| Semantic type | Representation | Carries a dimension? |
|---------------|----------------|----------------------|
| `Quantity(D)` | IEEE 754 binary64 magnitude in SI base units | Yes |
| `Key(I)` | Axis identity plus element position | No |
| `Int` | 64-bit signed integer | No |
| `Bool` | Boolean | No |
| `Datetime(S)` | High-precision epoch in time scale `S` | No |

`Quantity(D)`, `Key(I)`, and `Datetime(S)` are semantic notation, not literal
source syntax. Their source spellings are:

```text
Quantity(D)    -> D
Key(I)         -> Key<I>
Datetime(UTC)  -> Datetime
Datetime(S)    -> Datetime<S>
```

!!! important "No `Quantity` source constructor"
    Write a quantity type as its dimension, such as `Length`,
    `Dimensionless`, or `Length / Time`. `Quantity<Length>` and bare
    `Quantity` are not Graphcal types.

#### Quantity Types

A quantity is a binary64 floating-point magnitude with a **dimension** known at
compile time. Its unit is value/display metadata, not part of its type.

```
param mass: Mass = 1200.0 kg;           // quantity of dimension Mass
param ratio: Dimensionless = 0.85;      // identity-dimension quantity
```

`Dimensionless` is the identity-dimension quantity type. When two quantities
of the same dimension are divided, the result has type `Dimensionless`.

Floating-point arithmetic follows IEEE 754 binary64 rules. The runtime detects
and reports NaN and infinity.

Only quantities carry physical dimensions. `Key(I)`, `Int`, `Bool`, and
`Datetime(S)` are separate primitive families, not quantities.

#### Index Keys

`Key<I>` is the ValueType of element keys of axis `I` — the term-level
reflection of the type-level axis, mirroring the `Dim`/`Quantity(D)` pattern.
`I` may be a named, coordinate, `Fin`, or required axis. A key stores the
axis identity and an element position, so coordinate keys re-index exactly.

```
param delta_v: Velocity[Maneuver] = { ... };
node critical: Key<Maneuver> = argmax(@delta_v);
node critical_dv: Velocity = @delta_v[@critical];
```

Keys are introduced by qualified label expressions
(`Maneuver.Departure : Key<Maneuver>`), loop variables, `argmax`/`argmin`,
and the explicit formers `key`, `fin_key`, `floor_key`, `ceil_key`, and
`nearest_key`; they are eliminated by element access, same-axis `==`/`!=`,
exhaustive `match` (concrete named axes), and the extractions `coord`
(coordinate keys) and `to_int` (`Fin` keys). See
[Index Keys](indexes.md#index-keys) for the full reference. Like `Complex`,
keys do not cross the experimental plugin ABI.

#### Int

`Int` is a 64-bit signed integer. It is always dimensionless and cannot carry a physical dimension.

```
param count: Int = 42;
const node seven: Int = 7;
```

Integer arithmetic uses checked operations -- overflow is a runtime error, not silent wraparound.

#### Bool

`Bool` is used in conditions and logical expressions.

```
param enabled: Bool = true;
node active: Bool = @enabled && @count > 0;
```

#### Datetime

`Datetime` represents a precise point in time. It is parameterized by a **time scale** that determines how the instant is interpreted. Bare `Datetime` defaults to UTC.

```
param launch: Datetime = datetime("2024-11-05T12:00:00Z");
param t_tt: Datetime<TT> = epoch<TT>("2024-11-05T12:00:00");
```

Supported time scales: `UTC`, `TAI`, `TT`, `TDB`, `ET`, `GPST`, `GST`, `BDT`, `QZSST`.

Civil `datetime` and scientific `epoch<S>` construction are deliberately
separate. `datetime("...")` requires an explicit `Z` or numeric UTC offset;
`datetime("...", timezone)` accepts only offset-free local civil coordinates;
and `epoch<S>("...")` accepts only offset/zone/scale-free calendar coordinates
whose interpretation comes from static `S`. All literal forms are parsed while
the program is checked, so their static and runtime scales cannot disagree.
Timezone-based civil coordinates must identify exactly one instant in the
bundled tzdb: nonexistent DST-gap times and repeated DST-fold times are check
errors. Use one-argument `datetime` with an explicit numeric offset when the
source must select a particular instant.

Named civil timezones use quoted contextual `TimezoneLiteral`s. A timezone
literal is not a first-class `String` value: checking validates and
canonicalizes it into an opaque IANA identifier before evaluation. The IANA
database is bundled and pinned with the Graphcal toolchain. Compilation,
evaluation, and display never depend on the host operating system's zoneinfo
database; invalid-timezone diagnostics expose the bundled tzdb release.

Datetime values follow **point-vs-vector** semantics:

| Operation | Result | Notes |
|-----------|--------|-------|
| `Datetime - Datetime` | `Time` | Both must be the same time scale |
| `Datetime + Time` | `Datetime` | Add a duration |
| `Time + Datetime` | `Datetime` | Add a duration (commuted form) |
| `Datetime - Time` | `Datetime` | Subtract a duration |
| `Datetime == Datetime` | `Bool` | Equality comparison |
| `Datetime < Datetime` | `Bool` | Ordering comparison |

Datetime values cannot be added together, multiplied, divided, or used as the right-hand operand of duration subtraction (`Time - Datetime`).

Cross-scale operations are type errors: `Datetime<UTC> - Datetime<TT>` does not compile. Use explicit time scale conversion functions (`to_utc`, `to_tt`, etc.) first.

Calendar extractors (`year`, `month`, `day`, `hour`, `minute`, `second`,
`weekday`, and `day_of_year`) use the value's declared `Datetime<S>` scale.
`weekday` follows ISO 8601 numbering from Monday = 1 through Sunday = 7.
Timezone metadata applied with `-> "Area/Location"` affects display only and
never changes these computational fields.

See [Built-in Reference](built-ins.md#datetime-functions) for the full list of datetime constructors, conversions, and extraction functions.

### Value Types (Level 2)

A `ValueType` is a single logical value: a primitive or an instance of one
nominal algebraic type declared with `type`. Every body-bearing `type`
declaration lists one or more constructors. Constructor count does not create
separate "struct" and "union" type categories.

Constructor payload fields are declared with full DeclTypes: a field may
carry domain constraints and index axes, as in
`Budget(dv: Velocity(min: 0.0 m/s)[Maneuver])` or the generic
`Vector(values: D[Fin(N)])`. Field names must be unique within each
constructor; the same field name may be used independently by different
constructors. What stays ValueType-only is the `Type` generic
kind: an indexed DeclType such as `Velocity[Maneuver]` cannot be passed as a
`Type` argument, so `Holder<Velocity[Maneuver]>` is rejected. To parameterize
structured data over an axis, either index the payload field directly or
index the algebraic type as a whole: `Vec3<Velocity, ECI>[Maneuver]`.

#### Algebraic Type with One Constructor

A one-constructor type can be record-shaped. By convention, its constructor
has the same name as its type:

```
type Orbit {
    Orbit(sma: Length, ecc: Dimensionless, inc: Angle),
}

type Vec3<D: Dim, Frame: Type> {
    Vec3(x: D, y: D, z: D),
}
```

Because every value has the same constructor and payload shape, its fields can
be accessed directly with `@value.field`.

#### Algebraic Type with Multiple Constructors

The same `type` declaration syntax can list multiple constructors. Each
constructor has its own payload or is a bare unit constructor:

```
type ManeuverKind {
    Impulsive(delta_v: Velocity),
    LowThrust(thrust: Force, duration: Time),
    Coast,
}
```

In value-former signature notation, this declaration introduces:

```text
Impulsive : Velocity -> ManeuverKind
LowThrust : Force × Time -> ManeuverKind
Coast     : ManeuverKind
```

The arrows are semantic notation: construction always spells field names, as
in `Impulsive(delta_v: 3.1 km/s)`, and a constructor is not a function value.

Field access is rejected when a type has multiple constructors because no
single payload shape is guaranteed. Destructure the value through `match`
instead.

### Declaration Types (Level 3)

A DeclType is a possibly-constrained ValueType, optionally indexed by one or
more axes. This is what type annotations denote — on `param`, `node`, and
`const node` declarations, DAG parameters, and constructor payload fields:

```
param dry_mass: Mass = 1200.0 kg;                         // ValueType
param delta_v: Velocity[Maneuver] = { ... };              // Indexed ValueType
node matrix: Dimensionless[Row, Col] = for r, c { ... };  // Multi-indexed ValueType
type Budget { Budget(dv: Velocity[Maneuver]) }            // Indexed payload field
```

`T[I]` is a type constructor that lifts a ValueType into a total map from index labels to values. Multi-indexing `T[I, J]` is a flat product-key map (not nested). Axis order is significant: `T[I, J]` and `T[J, I]` are different types.

## Domain Constraints

Type expressions can carry **domain constraints** that declare valid value ranges. Constraints are written as `(min: expr, max: expr)` after the base type:

```
param bus_mass: Mass(min: 100.0 kg, max: 2000.0 kg) = 500.0 kg;
param thrust: Force(min: 0.01 N) = 0.5 N;           // min only
param efficiency: Dimensionless(max: 1.0) = 0.85;    // max only
param count: Int(min: 1, max: 100) = 10;             // exact Int constraints
param launch: Datetime(
    min: datetime("2025-01-01T00:00:00Z"),
    max: datetime("2025-12-31T23:59:59Z"),
) = datetime("2025-06-01T12:00:00Z");
```

### Syntax

The constraint clause goes between the base type and the optional `[Index]` suffix:

```
T(min: expr, max: expr)           // both bounds
T(min: expr)                      // lower bound only
T(max: expr)                      // upper bound only
T(min: expr, max: expr)[I]        // constrained indexed type (element-wise)
```

Here `T` must be a quantity type, `Int`, or `Datetime<S>`, and `I` is an
index axis. Both `min` and `max` are optional—you can specify one or both.
Bounds are inclusive compile-time constant expressions. Each bound must have
the type required by `T`; in particular, `Int` bounds are exactly `Int`, and
datetime bounds are exactly the target `Datetime<S>` scale.

### Supported Types

Domain constraints are valid on:

- **Quantity types** (including `Dimensionless`): `Mass(min: ...)`,
  `Velocity(max: ...)`, `Dimensionless(min: 0.0, max: 1.0)`, etc.
- **`Int`**: `Int(min: 1, max: 100)`. Bounds stay as `i64`; a
  `Dimensionless` quantity is not implicitly converted into an integer bound.
- **`Datetime<S>`**: `Datetime<TT>(min: epoch<TT>("..."), max: ...)`.
  Bare `Datetime` is `Datetime<UTC>`, so its bounds use offset-aware
  `datetime("...Z")` values.

Domain constraints are **not** valid on `Bool` or any algebraic type,
regardless of its constructor count. Attempting to use constraints on these
types is a compile error.

### Indexed Types

For indexed types, constraints apply **element-wise** to each entry:

```
param delta_v: Velocity(min: 0.0 m/s, max: 10000.0 m/s)[Maneuver] = {
    Maneuver.Departure: 3200.0 m/s,
    Maneuver.Correction: 500.0 m/s,
    Maneuver.Insertion: 1800.0 m/s,
};
```

Each entry in the indexed value is independently checked against the constraint bounds.
This includes indexed `Int` and `Datetime<S>` values.

### Datetime Constraints

Datetime bounds use the constrained value's declared time scale:

```graphcal
const node TT_START: Datetime<TT> = epoch<TT>("2025-01-01T00:00:00");

param observation: Datetime<TT>(
    min: @TT_START,
    max: epoch<TT>("2025-12-31T23:59:59"),
) = epoch<TT>("2025-06-01T12:00:00");
```

A bound on `Datetime<TT>` must itself be `Datetime<TT>`. A UTC bound is
rejected even when it denotes a comparable physical instant; write an explicit
conversion such as `to_tt(datetime("...Z"))`. `min` and `max` are inclusive,
and `min > max` is rejected during compilation. Display-only timezone metadata
from `-> "Area/Location"` does not affect comparison or constraint checking.

### Constructor Payload Field Constraints

Constraints can also annotate the payload field types of a
constructor:

```
type SatelliteSpec {
    SatelliteSpec(
        mass: Mass(min: 100.0 kg, max: 2000.0 kg),
        altitude: Length(min: 200.0 km),
        commissioned: Datetime(
            min: datetime("2025-01-01T00:00:00Z"),
        ),
    ),
}

pub type ManeuverResult {
    Burn(dv: Velocity(min: 0.0 m/s, max: 10.0 km/s), duration: Time(min: 0.0 s)),
    Coast,
}
```

Field constraints are resolved in the module that defines the type. Importing
an exported type is therefore sufficient even when its bounds use custom
units and dimensions from that defining module; consumers do not need
redundant imports for those implementation dependencies.

Field constraints fire at **construction time** for each
`Ctor(field: ...)` call:

- For a `const node` whose value is a constructor call, violations are caught at compile time as `DomainViolation`.
- For a `param` or `node` that constructs a value at runtime, a violation is reported as a per-node `EvalFailed` error keyed to the constructor and field (e.g., `field SatelliteSpec.mass above maximum (2000 kg)`).

### Generic Type Arguments

Domain constraints are **not** allowed on generic type arguments — they have no enforcement site after type erasure and ambiguous semantics. Put the constraint on the payload field of the constructor instead:

```
// REJECTED at compile time:
pub type Vec3<D: Dim> { Vec3(x: D, y: D, z: D) }
param p: Vec3<Length(min: 0.0 m)> = ...;

// Use a non-generic field constraint instead:
pub type SignedLength { SignedLength(value: Length(min: 0.0 m)) }
```

### Runtime Checking

Domain constraints on `param` and `node` declarations are checked at
**runtime** after evaluation: a violation produces a per-node error and
downstream nodes receive a `DependencyFailed`. Constraints apply element-wise
to indexed values and at construction time to constrained payload fields.
Constraints on `const node` declarations and on constructor payload fields
constructed inside a `const` are checked at compile time, since the values are
known statically.

All bound expressions are compile-time constants, regardless of whether the
constrained declaration is a `param`, `node`, or `const node`. Bounds may read
`const node` values, but may not read params or runtime nodes.

### Compile-Time Validation

The following are always caught at compile time, regardless of where the
constraint sits (top-level declaration or constructor payload field):

- **Invalid target type**: Constraint on an unsupported type (e.g., `Bool(min: 0)`)
- **Invalid key**: Unknown constraint key (e.g., `Mass(step: 10)` — only `min` and `max` are valid)
- **Min exceeds max**: When both bounds are specified and `min > max`
- **Dimension/type mismatch**: When a quantity bound has the wrong dimension,
  or an `Int` bound is not exactly `Int`
- **Datetime time-scale mismatch**: A bound is not exactly the target
  `Datetime<S>`; cross-scale bounds require an explicit conversion
- **Non-constant bound**: A bound references a param or runtime node
- **Generic type-arg constraint**: A constraint placed on a `TypeApplication` argument like `Vec3<Length(min: 0.0 m)>`

### Use Cases

Domain constraints are useful for:

- **Parameter sweeping/sampling**: Declaring valid ranges for design space exploration
- **Input validation**: Catching obviously wrong parameter values before they propagate through the graph
- **Documentation**: Making valid ranges explicit in the type annotation, visible in LSP hover

## Indexes and Indexed Types

An index declares a finite, ordered collection axis usable in `T[I]`. Graphcal
has named, coordinate, and structural finite indexes. Each kind carries an
intrinsic **index order**: a named index is ordered by label declaration
order, a coordinate index by its coordinate sequence from `start` toward
`end`, and `Fin(N)` by ascending position. Indexed values never carry an
order of their own — every construction is normalized to the index order, and
order-sensitive consumers (`scan`, rendered output, extern-function buffers)
follow it. Multi-axis extern buffers use the lexicographic row-major product of
those per-axis orders and carry every axis extent explicitly.

### Named Index

A named index declares a finite, ordered set of labels usable as a collection axis. The `index` keyword declares:

1. An **axis marker**: `Maneuver` can be used in `T[Maneuver]` to create indexed types.
2. An ordered, closed set of **index labels**: `Maneuver.Departure` identifies one position on that axis. The label declaration order is the axis's index order, so reordering the labels of an `index` declaration is a semantic change for order-sensitive consumers such as `scan`.

```
index Maneuver = { Departure, Correction, Insertion };
```

Named index labels use qualified syntax (`Maneuver.Departure`), distinguishing
them from algebraic-type constructors, which use bare syntax (`Nominal`). This
reflects a genuine semantic difference: labels identify positions within a
collection axis, while constructors form values of an algebraic type.

A qualified label is a **self-typed constant**: as an expression,
`Maneuver.Departure` has type `Key<Maneuver>` and is an ordinary value — it
can be stored in nodes, compared with `==`, passed through DAG parameters,
and used in constructor payloads (see [Index Keys](indexes.md#index-keys)).
Exactly as a constructor name is both an expression and a pattern, the label
spelling additionally remains a *syntactic selector* in pattern-like
positions:

- Indexed type axes name the index itself: `Velocity[Maneuver]`
- Element access: `@delta_v[Maneuver.Departure]` (the label as a constant key)
- Map and table keys: `{ Maneuver.Departure: 2.46 km/s, ... }` (selector)
- `for` bindings and index access through their loop variables: `for m: Maneuver { @delta_v[m] }`
- `match` patterns over `Key<Maneuver>` scrutinees: `match m { Maneuver.Departure => ..., ... }`

An algebraic type with only unit constructors (e.g., `type Foo { A, B }`) is
NOT automatically an index. The `index` keyword explicitly marks an
enumeration as usable in `T[I]`, preventing accidental use of marker types as
collection axes.

### Coordinate Index

A coordinate index is a finite sequence of quantity values in one dimension:

```
index TimeStep = range(0.0 s, 100.0 s, step: 0.1 s);
index Samples = linspace(0.0 s, 100.0 s, points: 1001);
```

`range` makes the exact increment authoritative; `linspace` makes the exact
point count authoritative. Their arguments are static and finite. A coordinate
loop variable is a key of type `Key<C>`: it indexes and compares with `==`
directly, while its quantity coordinate is extracted explicitly with
`coord(t)` for arithmetic and ordering.

### Structural Finite Index

`Fin(N)` constructs an anonymous positional axis explicitly:

```
param vector: Dimensionless[Fin(3)] = for i: Fin(3) { 0.0 };
```

Its loop variable is a key of type `Key<Fin(N)>`: it indexes directly,
supports additive bound-tracking arithmetic (`i + c : Key<Fin(N + c)>` for a
static Nat `c`), and exposes its integer position explicitly through
`to_int(i)`. The Nat `N` and Index `Fin(N)` remain distinct sorts, so a bare
`D[3]` is invalid.

### Index Capabilities

| Capability | Named (`Maneuver`) | Coordinate (`TimeStep`) | Finite (`Fin(3)`) |
|-----------|----------------------|--------------------------|-------------------|
| Loop variable type | `Key<Maneuver>` | `Key<TimeStep>` | `Key<Fin(3)>` |
| Indexing: `@x[i]` | Yes | Yes | Yes |
| Explicit map key | Yes | No | No |
| Equality comparison | Yes (same axis) | Yes (same axis) | Yes (same axis) |
| Pattern matching | Yes (qualified label) | No | No |
| Arithmetic | No | Via `coord(t)` quantity arithmetic | Additive `i + c` (bound-tracking); integer via `to_int(i)` |
| Pass to DAG param | Yes (as key) | Yes (as key) | Yes (as key) |

Every loop variable is a key of its axis. All keys index and compare for
equality on the same axis; none is ordered. Beyond that, the axis kinds
differ in what the key exposes: a named key drives exhaustive `match` and is
otherwise opaque, a coordinate key exposes its quantity through `coord(t)`,
and a `Fin` key exposes its position through `to_int(i)` and carries the
additive arithmetic above. See [Index Keys](indexes.md#index-keys).

### Construction of Indexed Values

**Map literal** (total -- all labels must be present):

```
param delta_v: Velocity[Maneuver] = {
    Maneuver.Departure: 2.46 km/s,
    Maneuver.Correction: 0.05 km/s,
    Maneuver.Insertion: 1.48 km/s,
}
```

**Multi-axis map literal** (total -- all label tuples must be present):

```
param delta_v_budget: Velocity[Phase, Maneuver] = {
    (Phase.Launch, Maneuver.Departure): 2.46 km/s,
    (Phase.Launch, Maneuver.Correction): 0.0 m/s,
    (Phase.Launch, Maneuver.Insertion): 0.0 m/s,
    (Phase.Cruise, Maneuver.Departure): 0.0 m/s,
    (Phase.Cruise, Maneuver.Correction): 0.05 km/s,
    (Phase.Cruise, Maneuver.Insertion): 0.0 m/s,
    (Phase.Arrival, Maneuver.Departure): 0.0 m/s,
    (Phase.Arrival, Maneuver.Correction): 0.0 m/s,
    (Phase.Arrival, Maneuver.Insertion): 1.48 km/s,
}
```

Single-axis map literals use bare keys (`Maneuver.Departure: ...`); multi-axis map literals use tuple keys (`(Phase.Launch, Maneuver.Departure): ...`).

Map (and `table`) literal entry order is not significant: the constructed
value is normalized to the index order, so a literal that lists
`Maneuver.Insertion` first is identical to one written in declaration order.

**`for` comprehension** (one value per label):

```
node fuel: Mass[Maneuver] = for m: Maneuver {
    @dry_mass * (exp(@delta_v[m] / @v_exhaust) - 1.0)
}
```

**Multi-axis `for`** (one value per label tuple):

```
node matrix: Dimensionless[Row, Col] = for r: Row, c: Col {
    @A[r, c] + @B[r, c]
}
```

### Consumption of Indexed Values

**Indexing** -- extracts a single element by providing all index labels:

```
@delta_v[Maneuver.Departure]                // Velocity[Maneuver] -> Velocity
@matrix[Row.R1, Col.C2]                    // Dimensionless[Row, Col] -> Dimensionless
```

No partial indexing -- all axes must be specified. To extract a "slice" along one axis, use explicit `for`:

```
node row1: Dimensionless[Col] = for c: Col { @matrix[Row.R1, c] }
```

**Aggregation** -- reduces exactly one axis. Direct multi-axis aggregation is
rejected with `D021`; select one axis first with an explicit `for`:

```
sum(for m: Maneuver { @fuel[m] })        // Mass[Maneuver] -> Mass
product(for i: Fin(3) { @edge[i] })       // Length[Fin(3)] -> Volume
rss(for m: Maneuver { @sigma[m] })        // Mass[Maneuver] -> Mass
maximum(for m: Maneuver { @delta_v[m] }) // Velocity[Maneuver] -> Velocity
count(for m: Maneuver { @fuel[m] })      // Mass[Maneuver] -> Int
```

The quantity reductions require quantity elements. `product` needs a concrete
axis cardinality when its elements are dimensioned because that cardinality is
the result's dimension exponent. `rss` preserves the element dimension.
`count` accepts any non-indexed element type and preserves cardinality as `Int`.

**Scan** -- ordered accumulation:

```
scan(
    for m: Maneuver { @delta_v[m] },
    0.0 m/s,
    |acc, item| acc + item,
)   // Velocity[Maneuver] -> Velocity[Maneuver]
```

`scan` folds in the index order — label declaration order for a named axis
such as `Maneuver`. Its source has exactly one axis; it does not implicitly
select an axis from a multi-axis value. Each output element is the accumulated
result up to and including that position. The accumulator may independently
carry fixed state axes, which follow the source axis in the result.

### No Implicit Broadcasting

Arithmetic and comparison operators require unindexed operands. Element-wise
work on indexed values must use an explicit `for`; comparison operators
`==`, `!=`, `<`, `<=`, `>`, and `>=` reject an indexed operand with `D019`.
This is a deliberate safety decision:

```gcl
// ERROR: neither arithmetic nor comparison broadcasts indexed values
node bad_sum: Velocity[Maneuver] = @delta_v + @extra_dv;
node bad_limit: Bool[Maneuver] = @delta_v < 3.0 km/s;

// CORRECT: explicit element-wise operations
node sum: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] + @extra_dv[m]
};
node below_limit: Bool[Maneuver] = for m: Maneuver {
    @delta_v[m] < 3.0 km/s
};
```

This prevents the class of silent broadcasting bugs common in NumPy and Excel,
where mismatched shapes may otherwise be resolved without making the selected
axes or element pairing explicit.

## Type Conversions

Graphcal has **no implicit type conversions**. You must use explicit conversion functions:

| Function | From | To | Example |
|----------|------|----|---------|
| `to_float(x)` | `Int` | `Dimensionless` | `to_float(42)` yields `42.0` |
| `to_int(x)` | `Dimensionless` | `Int` | `to_int(3.0)` yields `3`; `to_int(3.7)` errors |
| `to_int(k)` | `Key(Fin(N))` | `Int` | Position of a `Fin` key |
| `coord(k)` | `Key(C)` | `Quantity(D)` | Coordinate of a coordinate-axis key |
| `to_utc(x)` | `Datetime(S)` | `Datetime(UTC)` | Time scale conversion |
| `to_tai(x)` | `Datetime(S)` | `Datetime(TAI)` | Time scale conversion |
| `to_tt(x)` | `Datetime(S)` | `Datetime(TT)` | Time scale conversion |

The existing `to_float` name describes conversion to the floating-point
representation; it does not name a source-level `Float` type. `to_int` requires
a finite, integer-valued binary64 input in the `Int` range. It does not choose a
rounding policy: use `to_int(trunc(x))`, `to_int(floor(x))`, `to_int(ceil(x))`,
or `to_int(round(x))` to state that policy explicitly. On keys, `to_int` and
`coord` are the only extractions: `to_int` accepts `Fin` keys only, and
`coord` accepts coordinate-axis keys only — a named key is opaque, and there
is no reverse quantity-to-key conversion (use the explicit key formers).
Time scale conversion
functions (`to_utc`, `to_tai`, `to_tt`, `to_tdb`, `to_et`, `to_gpst`, `to_gst`,
`to_bdt`, `to_qzsst`) convert between time scales without changing the physical
instant.

## Dimension Algebra

Dimensions are compile-time types that form an algebra over base dimensions. See [Dimensions & Units](dimensions-and-units.md) for the full reference on declaring dimensions and units. This section describes the algebraic rules the compiler enforces.

### Representation

Internally, a dimension is a product of base dimensions with rational exponents:

$$
D = L^{a_1} \cdot T^{a_2} \cdot M^{a_3} \cdot \ldots
$$

where each exponent $a_i$ is a rational number (stored as a reduced fraction). `Dimensionless` is the case where all exponents are zero.

### Arithmetic Rules

The compiler determines the dimension of the result of each arithmetic operation:

| Operation | Dimension Rule | Constraint |
|-----------|---------------|------------|
| `a + b` | Same as operands | `dim(a)` must equal `dim(b)` |
| `a - b` | Same as operands | `dim(a)` must equal `dim(b)` |
| `a * b` | Product | `dim(a) * dim(b)` -- exponents add |
| `a / b` | Quotient | `dim(a) / dim(b)` -- exponents subtract |
| `a ^ n` | Power | `dim(a) ^ n` -- a dimensioned base requires exact integer/rational syntax |
| `a % b` | Same as operands | `dim(a)` must equal `dim(b)` |

Examples:

- `Length * Length` = `Length^2`
- `Length / Time` = `Velocity` (a prelude derived dimension)
- `Length / Length` = `Dimensionless`
- `(Mass * Length / Time^2) * (Length / Time)` = `Mass * Length^2 / Time^3`

### Equivalence

Two dimension expressions are equivalent if and only if they reduce to the same canonical form (same set of base dimension exponents). Named dimensions are transparent -- `Velocity` and `Length / Time` are the same type if `Velocity` is defined as `Length / Time`.

### Built-in Function Dimension Rules

Built-in math functions have specific dimension constraints:

| Function | Argument Dimension | Result Dimension |
|----------|--------------------|------------------|
| `sqrt(x)` | Any `D` | `D^(1/2)` (exponents halved) |
| `sin(x)`, `cos(x)`, `tan(x)` | `Angle` | `Dimensionless` |
| `asin(x)`, `acos(x)` | `Dimensionless` | `Angle` |
| `atan2(y, x)` | Both same `D` | `Angle` |
| `exp(x)` | `Dimensionless` or `Complex<Dimensionless>` | Same real/complex kind as input, dimensionless |
| `ln(x)`, `log10(x)` | `Dimensionless` | `Dimensionless` |
| `abs(x)` | Any real `D` or `Complex<D>` | Real `D` |
| `least(a, b)`, `greatest(a, b)` | Both same `D` | `D` |
| `floor(x)`, `ceil(x)`, `round(x)`, `trunc(x)` | `Dimensionless` | `Dimensionless` |

Rounding functions accept only `Dimensionless` arguments: "round to an
integer" has no unit-independent meaning for a dimensioned quantity. To round
a quantity, divide by an explicit granularity, e.g.
`floor(@x / 1.0 cm) * 1.0 cm` -- see
[Built-in Reference](built-ins.md#math-functions).

## Unit Conversion

The `->` operator converts between units of the same dimension. It binds at the lowest precedence. For a complex value, one display unit scales both components.

```
node speed_kmh: Velocity = @speed -> km/h;
node displacement_cm: Complex<Length> = @displacement -> cm;
```

There is no type-cast operator. To change a value's phantom type parameter (e.g., re-label a reference frame), construct a new instance and assign each field explicitly:

```
node pos_body: Vec3<Length, Body> = Vec3<Length, Body>(
    x: @pos_eci.x,
    y: @pos_eci.y,
    z: @pos_eci.z,
);
```

The verbosity is intentional: every relabeling is a deliberate, field-by-field act, visible at the call site.

## Typing Rules for Expressions

This section lists the type of each expression form and the constraints the compiler enforces.

### Literals

| Expression | Type | Notes |
|-----------|------|-------|
| `42` | `Int` | No decimal point or exponent |
| `3.14` | `Dimensionless` | Floating-point literal without a unit |
| `400.0 km` | Dimension of the unit (`Length`) | Floating-point literal with a unit; integer literals cannot have units |
| `true`, `false` | `Bool` | |

### References

| Expression | Type |
|-----------|------|
| `@name` | Declared type of param/node/const node `name` |
| `BUILTIN_NAME` | Type of the built-in constant (`PI`, `E`, `TAU`, etc.) |
| `local_var` | Type of the loop variable or match binding |

### Arithmetic Operators

| Expression | Result Type | Constraint |
|-----------|-------------|------------|
| `a + b` | type of `a` | Same numeric kind and dimension |
| `a - b` | type of `a` | Same numeric kind and dimension |
| `a * b` | numeric kind lifted over `dim(a) * dim(b)` | Supports real×real, real×complex, complex×real, and complex×complex |
| `a / b` | numeric kind lifted over `dim(a) / dim(b)` | Supports real÷real, real÷complex, complex÷real, and complex÷complex |
| `a % b` | `dim(a)` | Real quantities or integers only; dimensions must match |
| `a ^ n` | `dim(a) ^ n` | Real quantities or integers only; `n` is dimensionless and a dimensioned base requires an exact integer or parenthesized rational |
| `-a` | type and dimension of `a` | Real quantity, complex quantity, or integer |

Complex addition and subtraction require two `Complex<D>` operands with the
same `D`; Graphcal never promotes a real quantity implicitly. Use
`to_complex(x)` when that promotion is intended. Complex remainder and power
are not supported.

For a `Dimensionless` real base, `n` may be a runtime `Dimensionless` quantity. For
any other quantity dimension, `n` must be written exactly as an integer such as
`2` or `-2`, or as a parenthesized rational such as `(3/2)`. Decimal syntax
cannot determine a dimension exponent: use `(1/4)` instead of `0.25` and `2`
instead of `2.0`. The compiler preserves that rational exactly through
checking and evaluation rather than recovering it from binary64.

### Comparison and Logical Operators

| Expression | Result Type | Constraint |
|-----------|-------------|------------|
| `a == b`, `a != b` | `Bool` | `a` and `b` must be unindexed and have the same type; complex equality compares both components exactly; key equality requires the same axis |
| `a < b`, `a > b`, `a <= b`, `a >= b` | `Bool` | operands must be unindexed and either both `Int`, same-dimension real quantities, or same-scale datetimes; complex values and keys are unordered — extract with `coord(t)` / `to_int(i)` to order key contents |
| `a && b`, `a \|\| b` | `Bool` | `a` and `b` must be `Bool` |
| `!a` | `Bool` | `a` must be `Bool` |

Comparisons are non-chaining: `a < b < c` is a parse error; write `a < b && b < c`.

### Conditional

```
if condition { then_expr } else { else_expr }
```

- `condition` must be `Bool`.
- `then_expr` and `else_expr` must have the same type.
- The result type is the type of the branches.

### Function Call

```
function_name(arg1, arg2, ...)
```

- Arguments are matched positionally against the function's parameter types.
- Generic dimensions and indexes in built-in or extern signatures are inferred
  from ordinary value arguments.
- `epoch<S>(civil_literal)` consumes one explicit static time-scale argument.
  Other non-empty function generic arguments, such as `sqrt<3>(x)`, are
  rejected rather than silently ignored.
- The result type is the function's return type after generic substitution.

### Field Access

```
expr.field_name
```

- `expr` must have a record-shaped algebraic type: exactly one constructor with
  payload fields, using the same name as the type.
- `field_name` must be a payload field of that constructor.
- The result type is the declared field type, with generic parameters
  substituted.

### Index Access

```
expr[Index.Variant]        // access a specific element
expr[loop_var]              // access with a for-binding variable
expr[Index1.V1, Index2.V2] // multi-dimensional access
```

- `expr` must be an indexed type `T[I]` (or `T[I1, I2]` for multi-dimensional).
- All axes must be specified (no partial indexing).
- Each argument is a `Key<I>` term for its axis position — a qualified label
  (the constant-key spelling), a loop variable, or any computed key such as
  `@x[argmax(@x)]`. Axis identity governs: `Key<Fin(N)>` widens into
  `Fin(M)` positions when `N <= M`; no other cross-axis access exists.
- Statically discharged constant-`Int` positions on `Fin` axes (`@x[2]`)
  remain legal; a runtime `Int` or quantity never indexes implicitly (use
  `fin_key` or the coordinate searches).
- The result type is the element type `T`.

### Algebraic Value Construction

```
ConstructorName(field1: expr1, field2: expr2)
ConstructorName<Arg1, Arg2>(field1: expr1, field2: expr2)
ConstructorName                                   // unit constructor
```

- The constructor determines one enclosing algebraic result type; a
  constructor does not introduce a separate type.
- Each field expression must match the declared type of that payload field.
- Every payload field must be written as `field: expr`; shorthand `field` is
  not supported.
- Use explicit `field: @node_name` when passing graph nodes.
- Generic arguments instantiate the enclosing algebraic type's parameters.

### Index Label

```
IndexName.VariantName
```

- References a specific label of a named index.
- As an expression, it is a self-typed constant of type `Key<IndexName>` —
  storable, comparable, and passable like any other value.
- In pattern-like positions — map/table keys, `#[expected_fail(...)]` keys,
  include index bindings, and `match` patterns — the same spelling is a
  syntactic selector, mirroring the expression/pattern duality of
  constructor names.

### Match Expression

```
match scrutinee {
    VariantA(field1: field1, field2: binding) => expr_a,
    VariantB => expr_b,
}
```

- `scrutinee` must be an algebraic-type value or a `Key<X>` value of a
  concrete named axis `X` (a loop variable, a stored key, an
  `argmax`/`argmin` result, ...). Keys of coordinate, `Fin`, and required
  axes cannot be matched.
- `match` is for exhaustive case analysis over closed finite alternatives. Use `if` for ordinary boolean predicates and comparisons.
- All members/labels must be covered (exhaustiveness check).
- For algebraic-type scrutinees, arms use constructor patterns (bare or
  module-qualified) and can bind payload fields explicitly with
  `field: variable` or `field: _`.
- For named-axis key scrutinees, arms use qualified index-label patterns (`Index.Label` or `module.Index.Label`) and cannot bind fields.
- All arm expressions must have the same type.
- The result type is the common type of the arms.

### Map Literal

```
{ Index.Variant1: expr1, module.Index.Variant2: expr2, ... }
```

- All variants of the index must be covered.
- All value expressions must have the same type `T`.
- The result type is `T[Index]`; when the index is imported, the owner is the resolved module item, not just the leaf spelling.

For multi-axis map literals, use tuple keys:

```
{ (I1.V1, I2.V2): expr1, (I1.V1, I2.V3): expr2, ... }
```

- All label tuples must be present.
- The result type is `T[I1, I2]`.

### For Comprehension

```
for var: IndexName { body_expr }
for v1: Index1, v2: Index2 { body_expr }
```

- `var` is bound to each element of the index in turn, as a per-node
  constant key of type `Key<IndexName>`.
- For named indexes, the `Key<X>` loop variable indexes (`@x[var]`),
  compares with `==` against other `Key<X>` values, and drives exhaustive
  `match`.
- For coordinate indexes, the `Key<C>` loop variable indexes and compares;
  its quantity coordinate is extracted with `coord(var)`.
- For `Fin(N)`, the `Key<Fin(N)>` loop variable indexes (including additive
  `var + c` accesses), compares, and exposes its integer position via
  `to_int(var)`.
- `body_expr` is evaluated for each binding; its type is `T`.
- The result type is `T[IndexName]` (or `T[Index1, Index2]` for multiple bindings).

### Scan

```
scan(source, init, |acc, item| body)
```

In signature notation,
`scan : T[I] × U[J̄] × (U[J̄] × T -> U[J̄]) -> U[I, J̄]`:

- `source` must have exactly one axis, with type `T[I]`. A multi-axis source is
  rejected because `scan` does not choose an axis implicitly.
- `init` has accumulator type `U[J̄]`, where `J̄` may be empty, one axis, or
  several fixed axes.
- `acc` is bound to type `U[J̄]`; `item` is bound to type `T`.
- `body` must return exactly `U[J̄]`, including the same axes in the same order.
- The result type is `U[I, J̄]`: the source axis is prepended to the accumulator
  axes.

For example, scanning scalar inputs `Dimensionless[Step]` with an initial
vector `Dimensionless[Element]` produces
`Dimensionless[Step, Element]`. Each iteration reads the complete previous
vector before constructing the next vector.

The `|acc, item| body` is special syntax, not a function value.

### Unfold

```
unfold(index, init, |prev_state, prev_i, i| body)
```

In signature notation,
`unfold : I × U[J̄] × (U[J̄] × Key(I) × Key(I) -> U[J̄]) -> U[I, J̄]`
for a coordinate index `I` over dimension `D`:

- `index` is an explicit reference to a coordinate index `I`, not a value expression.
- `init` has state type `U[J̄]` and becomes the result at the first coordinate.
  `J̄` may be empty, one axis, or several fixed axes.
- `prev_state` is bound to the complete previous state `U[J̄]`.
- `prev_i` and `i` are bound to consecutive element keys `Key<I>`; the
  coordinate quantities they stand on are extracted with `coord(prev_i)` and
  `coord(i)`.
- `body` must return exactly `U[J̄]`, including the same axes in the same order.
- The result type is `U[I, J̄]`: the coordinate axis is prepended to the state
  axes. For example, vector state `U[Element]` produces
  `U[Time, Element]`, and matrix state `U[Row, Column]` produces
  `U[Time, Row, Column]`.

For coordinates `i₀, i₁, …`, `result[i₀] = init` and each later value is
`body(result[iₖ₋₁], iₖ₋₁, iₖ)`. The complete previous state is frozen while
that body constructs the next state, so component iteration order has no
semantic effect. The
`|prev_state, prev_i, i| body` form is special syntax, not a function value.
Use `prev_state` directly; an explicit reference to the declaration being
defined is an ordinary dependency cycle, regardless of the coordinate used to
index that reference.

## DAG Blocks

DAG blocks are named, reusable sub-DAGs. They are declarations, not values. There is no function type in the type system.

DAG block parameters and nodes use the same DeclTypes (ValueTypes or indexed types) as top-level declarations:

```
dag hohmann_transfer {
    param gm: Length^3 / Time^2;
    param r1: Length;
    param r2: Length;
    node dv1: Velocity = ...;
    node total_dv: Velocity = @dv1 + ...;
}
```

DAG blocks are instantiated with `include`, which embeds their nodes into the enclosing computation graph:

```
include hohmann_transfer(
    gm: @gm_earth,
    r1: @r_earth + @parking_alt,
    r2: @r_earth + @target_alt,
).{ total_dv };
```

## Generics

Algebraic types can be parameterized by values from four type-level domains.
A generic parameter's annotation is its **kind**: it determines which arguments
are legal and how the parameter may be used in the declaration.

### Generic Parameter Kinds

| Kind | Syntax | Parameter ranges over | Valid uses |
|------|--------|-----------------------|------------|
| `Dim` | `<D: Dim>` | Any dimension, such as `Length` or `Length / Time` | Real quantity fields, `Complex<D>`, and dimension expressions such as `D^2` |
| `Type` | `<T: Type>` | Any `ValueType`, such as `Bool`, `Length`, or `Vec3<Length, Eci>` | A payload field type, or a phantom/tag parameter if unused in any payload |
| `Index` | `<I: Index>` | A finite ordered index axis | An axis in an indexed type such as `D[I]`, or a key type `Key<I>` |
| `Nat` | `<N: Nat>` | A non-negative natural number used in type-level size arithmetic | A finite cardinality such as `D[Fin(N)]`, subject to the non-empty-axis rule |

These kinds are not themselves `ValueType`s. For example, `Index` denotes the
domain of collection axes; an axis only reaches the term level through its
reflection type `Key<I>`, whose values are the axis's element keys. Likewise,
`Type` denotes the `ValueType` domain and excludes both a bare index axis and
an indexed `DeclType` such as `Velocity[Maneuver]`.

A `Type` parameter is not inherently phantom. It is phantom only when the
declaration carries it for nominal distinction without using it in a
constructor payload. In `Vec3<D, Frame>`, for example, `D` determines the field
types while `Frame` is a phantom parameter.

### Generic Arguments and Defaults

Type annotations and constructors use the same generic-argument surface. Angle
brackets always contain at least one argument or parameter; omit the brackets
entirely when using no arguments or declaring a non-generic type. Empty forms
such as `Vec3<>` and `type Marker<> { Marker }` are invalid. Each argument is
checked positionally against the declared `Dim`, `Index`, `Nat`, or `Type` kind;
spelling or identifier casing never determines its kind. Ordinary functions do
not declare or consume these arguments, so a non-empty application such as
`sqrt<3>(x)` is rejected rather than silently ignored.

A `Nat` argument may be an integer literal, an in-scope `Nat` parameter, or a
polynomial expression using `+` and `*`:

```gcl
type Fixed<N: Nat> {
    Fixed(value: Dimensionless),
}

type Matrix<M: Nat, N: Nat> {
    Matrix(value: Fixed<M * N + 1>),
}

param matrix: Matrix<2, 3> = Matrix<2, 3>(
    value: Fixed<7>(value: 1.0),
);
```

Bare names and products such as `N` or `M * N` are intentionally ambiguous in
source syntax. The compiler resolves them only after it knows the corresponding
parameter kind.

Generic parameters of every kind can have defaults:

```gcl
index Component = { X, Y, Z };
type Unframed { Unframed }

type Vec3<
    D: Dim,
    I: Index = Component,
    F: Type = Unframed,
    N: Nat = 3,
> {
    Vec3(x: D, y: D, z: D),
}

// The omitted arguments are Component, Unframed, and 3.
param pos: Vec3<Length> = Vec3<Length>(x: 1.0 m, y: 2.0 m, z: 3.0 m);
```

Defaulted parameters must form a trailing suffix, and each default may refer
only to parameters declared earlier in the list. Arguments may be omitted only
from that trailing defaulted portion. If every parameter has a default, type
and constructor names without an angle-bracket list apply all of them.

`Nat` and `Index` are distinct sorts. A bare Nat is never lifted into an Index:
use `Fin(3)` for an `Index` argument and `3` for a `Nat` argument. Accordingly,
`D[3]` is rejected with a suggestion to write `D[Fin(3)]`.

### Generic Parameter Unification

When using generic types, type parameters are unified from context:

```
param pos: Vec3<Length, Eci> = Vec3<Length, Eci>(x: 1.0 km, y: 0.0 km, z: 0.0 km);
```

The compiler performs **unification**: it matches the declared type parameters (which may contain generic variables) against the concrete types to determine bindings for each generic variable.

If a generic variable appears multiple times, all occurrences must unify to the same concrete type.

### Dimension Expressions in Generics

Generic dimension parameters can appear in compound dimension expressions in type definitions.

### Structural Finite Indexes

`Fin(N)` explicitly constructs an Index with integer positions `0` through
`N - 1`:

```
param v: Dimensionless[Fin(3)] = for i: Fin(3) { 1.0 };
param mat: Dimensionless[Fin(2), Fin(3)] =
    for i: Fin(2), j: Fin(3) { 1.0 };
node transposed: Dimensionless[Fin(3), Fin(2)] =
    for j: Fin(3), i: Fin(2) { @mat[i, j] };
```

Two finite structural indexes are equal exactly when their normalized
cardinality expressions are equal. Nat expressions support addition and
multiplication, with `*` binding more tightly than `+`; subtraction is not
supported. Express the larger side additively, for example use
`D[Fin(N + 1)]` for an input and `D[Fin(N)]` for its smaller output.

`Fin(0)` is invalid. Loop variables from `for i: Fin(N)` are keys of type
`Key<Fin(N)>` and can index values carrying that finite axis, including
through additive bound-tracking arithmetic (`i + c : Key<Fin(N + c)>`).
Integer use is explicit: `to_int(i)` produces an ordinary `Int`.

## Type Equivalence

Two types are equivalent if:

- **Quantities**: They have the same dimension in canonical form. Named dimensions are transparent (e.g., `Velocity` equals `Length / Time`).
- **Int**: Both are `Int`.
- **Bool**: Both are `Bool`.
- **Datetime**: Same time scale. `Datetime<UTC>` and `Datetime<TT>` are different types.
- **Algebraic types**: Same owner-qualified declared type name and equivalent
  generic arguments. Constructor count and payload shape belong to that
  nominal declaration; they do not create separate structural type identities.
- **Indexed**: Same element type, same indexes in the same order. `T[I, J]` and `T[J, I]` are different types.

There is no general subtyping. `Length` is not assignable to `Dimensionless`,
and `Vec3<Length, ECI>` is not assignable to `Vec3<Length, Unframed>` even if
both have the same fields. The one structural exception is `Fin`-key
widening: `Key<Fin(N)>` is accepted where `Key<Fin(M)>` is expected when
`N <= M`. Named and coordinate keys are nominal — `Key<Maneuver>` and
`Key<Phase>` never unify, and neither does a key with its content type
(`Key<TimeStep>` is not a `Time`; extract with `coord`).

A qualified label expression has the key type of its axis
(`Maneuver.Departure : Key<Maneuver>`), so keys participate in type
equivalence like any other primitive. In pattern position the same spelling
stays a selector; use `match` over a `Key<X>` value for case analysis.

## Complete Entity Map

| Entity | Is a type? | First-class value? | DAG param? | DAG node? | Appears in expressions |
|--------|-----------|---------------------|------------|-----------|----------------------|
| Quantity value | ValueType | Yes | Yes | Yes | Yes |
| Int value | ValueType | Yes | Yes | Yes | Yes |
| Bool value | ValueType | Yes | Yes | Yes | Yes |
| Datetime value | ValueType | Yes | Yes | Yes | Yes |
| Algebraic value | ValueType | Yes | Yes | Yes | Yes |
| Algebraic constructor | No | No | No | No | Construction and match patterns |
| Named index label | ValueType (`Key<X>`) as expression | Yes (constant key) | Yes (as key) | Yes | Expressions and indexing; a selector in match patterns and map/table keys |
| Indexed value | DeclType | Yes | Yes | Yes | Via `for` |
| Coordinate index label | ValueType (`Key<C>`) | Yes (as key) | Yes (as key) | Yes | Indexing, equality, `coord` extraction |
| `Fin` position | ValueType (`Key<Fin(N)>`) | Yes (as key) | Yes (as key) | Yes | Indexing, equality, additive key arithmetic, `to_int` extraction |
| Built-in constant (`PI`, `E`, `TAU`) | ValueType (`Dimensionless`) | Yes | Yes | Yes | Bare reference, no `@` |
| Built-in function | No | No | No | No | Calling only |
| Extern function | No | No | No | No | Qualified calling only (`ns.fn(...)`) |
| DAG module (file root or inline block) | No | No | No | No | `include` instantiation and inline `@d(args).out` calls |
| Imported module alias | No | No | No | No | Direct DAG calls (`@m(args).out`) and qualification (`m.item`, `@m.child(args).out`) |
| Dimension | No; inhabits `Dim` | No | As generic `<D: Dim>` | As generic | In quantity type syntax |
| Time scale | No; inhabits semantic `TimeScale` | No | No | No | In `Datetime<TT>`-style type syntax |
| Unit | No (compile-time) | No | No | No | In literals and conversion targets |
| Timezone | No (display metadata) | No | No | No | In `datetime(...)` arguments and `-> "Area/Location"` targets |
| Index axis | No; inhabits `Index` | No | As generic `<I: Index>` | As generic | In indexed type syntax |
| Natural number | No; inhabits `Nat` | No | As generic `<N: Nat>` | As generic | In Nat expressions and `Fin(N)` cardinalities |
| String literal | No (no `String` type) | No | No | No | Single-line boundary syntax: datetime arguments, timezone targets, plot properties, plugin paths |
| Attribute | No | No | No | No | Declaration metadata only |

Named index labels use qualified syntax (`Maneuver.Departure`) while algebraic
constructors use bare or module-qualified syntax (`Nominal` or
`module.Nominal`). Labels belong to the index universe; constructors belong to
the constructor namespace and form values of their enclosing algebraic type.
