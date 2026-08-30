# Finite-key literals (`#n`)

> **Status:** Proposed semantic unification. Graphcal currently accepts `#n` in
> selected structural-key positions, including `Fin` table slice labels and
> `#[expected_fail(...)]` selectors, but not as a general expression.

This note formalizes hash-prefixed natural literals such as `#0` and `#1` as
**finite-key literals**. The goal is to distinguish positions on structural
`Fin` axes from integer values without relying on context or implicit numeric
conversion.

## Terminology

`Fin(N)` is a finite structural **index axis** with `N > 0`. Its term-level
keys have type `Key<Fin(N)>`.

`#n` is a **finite-key literal** (also called a positional-key literal), not a
“number index.” The index is `Fin(N)`; `#n` denotes one key on that index.
Positions are zero-based, so the inhabitants of `Key<Fin(N)>` are:

```text
#0, #1, ..., #(N - 1)
```

The surface grammar is:

```ebnf
finite_key_literal = "#", NAT_LITERAL;
```

The literal is non-negative by syntax. It is a compile-time key, not a runtime
integer-to-key conversion.

## Typing

A finite-key literal has the smallest structural axis that contains its
position:

```text
────────────────────────────────────
Γ ⊢ #n ⇒ Key<Fin(n + 1)>
```

For example:

```text
#0 : Key<Fin(1)>
#1 : Key<Fin(2)>
#7 : Key<Fin(8)>
```

Graphcal's existing structural-key widening then permits the literal wherever
a sufficiently large `Fin` key is expected:

```text
Key<Fin(A)> is accepted as Key<Fin(B)> when A ≤ B.
```

Equivalently, the contextual checking rule is:

```text
Γ ⊢ #n ⇒ Key<Fin(n + 1)>    n + 1 ≤ N
────────────────────────────────────────
Γ ⊢ #n ⇐ Key<Fin(N)>
```

Consequently, `#1` can inhabit `Key<Fin(N)>` only when `N ≥ 2`. The general
validity condition `N > 0` for `Fin(N)` is not sufficient, because position 1
is out of bounds for `Fin(1)`.

The principal type avoids leaving an unspecified `N` in the type of a
standalone literal. It also allows expressions such as `to_int(#1)` to be
checked without an externally supplied axis.

## Distinction from `1`

The tokens `#1` and `1` belong to different semantic categories:

```text
#1 : Key<Fin(2)>    term-level finite key
 1 : Int            term-level integer literal
 1 : Nat            type-level natural literal in a Nat context
```

`Nat` is a generic-parameter kind, not a term-level value type. The spelling
`1` is interpreted as a `Nat` only in static positions such as `Fin(1)` and
`Nat` generic arguments.

There is no implicit value-level conversion between integers and finite keys:

```gcl
node second: Key<Fin(8)> = #1;
node position: Int = to_int(#1);

param requested: Int;
node requested_key: Key<Fin(8)> = fin_key(Fin(8), @requested);
```

The explicit conversion boundaries remain:

- `to_int(k)` converts a `Key<Fin(N)>` to its `Int` position;
- `fin_key(Fin(N), e)` range-checks a runtime `Int` and produces a key;
- `key(Fin(N), n)` constructs a key from an explicitly stated axis and static
  `Nat` position.

## Contrast with coordinate keys

Graphcal does not provide an analogous quantity-key literal for coordinate
axes. A quantity remains distinct from a key on a coordinate axis:

```gcl
2.0 s                          // Quantity(Time)
nearest_key(TimeStamp, 2.0 s)  // Key<TimeStamp>
```

A form such as `TimeStamp#(2.0 s)` could in principle denote a statically
validated coordinate key, but Graphcal deliberately requires a selection
function instead. An exact quantity-to-key literal or cast is problematic
because:

- the quantity may not lie on the coordinate grid;
- binary floating-point coordinates may not compare exactly; and
- the quantity's dimension does not identify a nominal coordinate axis.

The existing functions make the lookup policy explicit:

```gcl
nearest_key(TimeStamp, @time)
floor_key(TimeStamp, @time)
ceil_key(TimeStamp, @time)
```

`nearest_key` selects the closest coordinate, while `floor_key` and `ceil_key`
select the adjacent coordinate in the stated direction and fail outside their
supported bounds. Once one of these functions has selected an element, the
resulting `Key<TimeStamp>` carries the axis identity and exact element position.
Subsequent indexing therefore uses that position and does not repeat a
floating-point lookup.

This asymmetry is intentional: `#n` directly names the semantic position of a
structural `Fin` axis, whereas a quantity is a query against a nominal
coordinate axis and requires an explicit selection policy.

## Existing specialized uses

The existing `#n` forms should share this meaning even where the grammar does
not parse an ordinary expression:

- A `Fin` table slice label `[#n]` selects the key at position `n` on that
  slice axis.
- An `#[expected_fail(#n)]` component selects the key at position `n` on the
  corresponding assertion axis.

These forms have an axis supplied by their enclosing syntax, so they use the
contextual rule and must prove `n < N`. They should not be represented as an
untyped integer with a rendering convention; the typed core should preserve a
finite-position key and its resolved `Fin` axis.

## Element-access consistency

If finite-key literals become general expressions, the explicit element-access
form is:

```gcl
node selected: Pressure = @readings[#1];
```

The existing special case that accepts a bare compile-time integer position
(`@readings[1]`) is semantically distinct: `1` remains an `Int` literal and the
access rule constructs a key implicitly. To make the `#n` distinction fully
explicit, that special case should be removed in the same language change, or
else retained only with documentation that it is element-access shorthand
rather than evidence that `1` has a key type. This proposal recommends the
explicit `#n` form; implementation and migration are separate work.

## Safety properties

This design ensures that:

1. integer arithmetic cannot accidentally produce an index key;
2. runtime integers cross into an axis only through a checked operation;
3. literal bounds are checked statically;
4. the key's minimum required cardinality is reflected in its principal type;
5. `#n` has one semantic meaning across expressions, tables, diagnostics, and
   assertion selectors.
