# Type System -- Spaces

> Layer 5: Semantic context safety via phantom type parameters, opt-in derive, and user-defined operators (Sguaba-inspired).

## Status

**Decision level:** Redesigning. Moving from a built-in tag system (`tag` keyword, `merge` rules, `as` cast) to a generics-based approach: **phantom type parameters on user-defined structs + opt-in `derive` + user-defined operators**. This replaces the special-purpose tag subsystem with general-purpose language primitives.

## Summary

Inspired by [Sguaba](https://github.com/helsing-ai/sguaba), which uses Rust phantom types to prevent mixing vectors from different coordinate systems. The core insight: values can share the same dimension but live in **different semantic spaces** that must not be mixed.

Rather than a built-in tag layer with special rules (`merge`, `dimensionless auto-clear`, `as` cast), Graphcal provides general-purpose tools — **generic structs with phantom type parameters**, **default type parameters**, **opt-in operator derive**, **user-defined operators**, and **`as` cast for escape** — that let users build Sguaba-style safety themselves. The compiler enforces frame safety through type unification, not through special tag algebra.

Two usability features keep the system accessible: **default type parameters** let users write `Vec3<Length>` when they don't care about frames, and **`as` cast** provides a per-value escape hatch for deliberate cross-frame operations.

**Primary use cases:** Coordinate frames and time zones. These are the domains where mixing is always wrong and consequences are severe. Lighter labeling use cases (chemical species, cost categories) don't justify the annotation cost.

## Sguaba: Prior Art and Lessons

[Sguaba](https://github.com/helsing-ai/sguaba) is the primary inspiration. It uses Rust phantom types to prevent mixing vectors from different coordinate frames.

### How Sguaba Works

Every spatial type carries a frame as a phantom type parameter:
```rust
pub struct Vector<In, Time = Z0> {
    inner: Vector3,            // raw data
    system: PhantomData<In>,   // frame marker — zero cost at runtime
}
```

Frames are zero-sized marker structs with a convention (axis naming):
```rust
system! { pub struct PlaneNed using NED }
// → struct PlaneNed; impl CoordinateSystem for PlaneNed { type Convention = NedLike; }
```

Transforms are parameterized by source and destination frames:
```rust
pub struct Rotation<From, To> { inner: UnitQuaternion, ... }

// Application: Vector<From> * Rotation<From, To> → Vector<To>
// Composition: Rotation<A, B> * Rotation<B, C> → Rotation<A, C>  (middle-type cancellation)
```

Constructing a transform is `unsafe` — not for memory safety, but because the programmer is asserting "these numbers really do represent the claimed frame relationship." This is Sguaba's trust boundary.

Key Sguaba properties:
- **No untagged spatial values.** Every `Vector` and `Coordinate` must have a frame. Scalars (`f64`, `Length`) are frameless because they are separate types.
- **No implicit frame conversions.** All frame changes require explicit transforms or `unsafe` casts.
- **Affine space distinction.** `Coordinate` (point) vs `Vector` (displacement) with different algebraic rules: `Point - Point = Vector`, `Point + Vector = Point`, `Point + Point` is undefined.
- **Convention-based axis naming.** Two-level system: frame identity (`PlaneNed`) + convention (`NedLike` with north/east/down accessors).

### What Graphcal Adapts from Sguaba

| Sguaba Pattern | Graphcal Adaptation |
| --- | --- |
| Phantom type parameter per value | Generic struct type parameters (e.g., `Vec3<D, F>`) |
| Zero-sized frame marker structs | Empty `type` declarations (e.g., `type ECI {}`) |
| Compiler rejects mismatched frames | Generic type unification rejects `Vec3<D, ECI> + Vec3<D, Body>` |
| `unsafe` transform construction | `as` cast (per-value escape hatch) + user-defined cross-type operators |
| `Rotation<A, B> * Rotation<B, C> → Rotation<A, C>` | User-defined operators with multi-parameter generics |
| No implicit frame conversions | No operator derives for cross-type; all explicit |
| Affine `Coordinate` vs `Vector` distinction | User-defined as separate types with different operators |

### Where Graphcal Differs from Sguaba

**Sguaba is a library; Graphcal is a language.** Sguaba requires `PhantomData<T>` boilerplate and manual trait implementations. Graphcal makes phantom type parameters and operator derive first-class language features — no boilerplate for the struct declaration, opt-in derive for common operators.

**Sguaba's operator implementations are manual.** Every `impl Add for Vector<In>` is hand-written. Graphcal's `derive(Add)` auto-generates same-type component-wise operators, while user-defined operators handle cross-type cases.

## The Approach

Five mechanisms work together — three for safety, two for usability:

### 1. Generic Structs with Phantom Type Parameters

Types can have type parameters that appear in the type signature but not in the runtime data. These serve as compile-time markers:

```
// Frame markers — empty types, zero runtime cost
type ECI {}
type Body {}
type LVLH {}

// A 3D vector parameterized by dimension and frame
type Vec3<D: Dim, F> derive(Add, Sub, Neg) {
    x: D,
    y: D,
    z: D,
}

// F is a phantom parameter — it doesn't correspond to a field,
// but it prevents mixing: Vec3<Length, ECI> + Vec3<Length, Body> is a type error.
```

The phantom type parameter `F` does not appear in the struct's fields — it exists purely in the type system. The compiler rejects operations between `Vec3<Length, ECI>` and `Vec3<Length, Body>` because the types don't unify, the same way `Vec3<Length, ECI> + Vec3<Velocity, ECI>` fails because `Length ≠ Velocity`.

### 2. Default Type Parameters

Many users don't need frame safety — they're doing simple calculations where everything is in one implicit frame. Forcing them to write `Vec3<Length, ECI>` everywhere is unnecessary friction. **Default type parameters** let the type author specify a default for phantom parameters:

```
type Unframed {}

type Vec3<D: Dim, F = Unframed> derive(Add, Sub, Neg) {
    x: D, y: D, z: D,
}

// Users who don't care about frames — just use Vec3 as a simple 3D vector:
param velocity: Vec3<Velocity> = Vec3 { x: 1 km/s, y: 2 km/s, z: 3 km/s };
// F defaults to Unframed. This is Vec3<Velocity, Unframed>.

// Users who want frame safety — specify the frame:
param pos_eci: Vec3<Length, ECI> = Vec3 { x: 6878 km, y: 0 km, z: 0 km };
// This is Vec3<Length, ECI>.
```

**Key properties:**

- **The default is chosen by the type author**, not the language. `Vec3` defaults to `Unframed` because the author decided that's sensible. `Timestamp<TZ>` might have no default — forcing every timestamp to declare its timezone.
- **`Unframed` is not special.** It's a regular empty type like `ECI` or `Body`. The compiler treats `Vec3<Length, Unframed>` exactly like `Vec3<Length, ECI>` — they are different types that cannot be mixed.
- **Gradual adoption.** A team can start with unframed vectors (`Vec3<Length>`) and add frame annotations later as the project matures, without changing the type definition. The change is per-usage-site, not per-type.
- **Mixing framed and unframed is a type error.** `Vec3<Length, Unframed> + Vec3<Length, ECI>` fails because `Unframed ≠ ECI`. This is deliberate — if you introduce frame tracking, you must be consistent.

**Syntax follows Rust/C++ convention:** `F = Unframed` in the type parameter list. Default parameters must come after non-default parameters.

### 3. Opt-In Operator Derive (Safety)

Operators are NOT available by default on user-defined types. Each derivable operator must be explicitly requested, following the Rust principle that even `Debug` is opt-in:

```
// No derive → no operators at all. This is a pure data container.
type Timestamp<TZ> {
    epoch_seconds: Time,
}

// Opt-in derive → only the listed operators are generated.
type Vec3<D: Dim, F> derive(Add, Sub, Neg) {
    x: D,
    y: D,
    z: D,
}
```

**What derive generates:** For each derived operator, a component-wise implementation where both operands and the result are the same concrete type:

| Derive | Generated Signature | Semantics |
| --- | --- | --- |
| `Add` | `(Self, Self) -> Self` | Component-wise `+` on each field |
| `Sub` | `(Self, Self) -> Self` | Component-wise `-` on each field |
| `Neg` | `Self -> Self` | Component-wise negation on each field |
| `Eq` | `(Self, Self) -> Bool` | Component-wise `==`, all must match |
| `Ord` | `(Self, Self) -> Bool` | Requires total ordering on all fields |

**Derive constraints:** A derive is only valid if each field's type supports the underlying operation. `derive(Add)` on a struct with a `Str` field is a compile error.

**No implicit cross-type derive.** Operations like scalar multiplication (`Vec3 * Dimensionless`), dimension-changing multiplication (`Vec3<Velocity, F> * Time`), and cross-frame transforms are NEVER derived. They require explicit user-defined operators.

### 4. User-Defined Operators (Safety)

For operations where the operand types differ or the result type differs from the operands, users define custom operators:

```
// Scalar multiplication — different operand types, same result type
fn operator*<D: Dim, F>(v: Vec3<D, F>, s: Dimensionless) -> Vec3<D, F> =
    Vec3 { x: v.x * s, y: v.y * s, z: v.z * s };

// Dimension-changing multiplication — result type changes
fn operator*<D: Dim, F>(v: Vec3<D, F>, t: Time) -> Vec3<D * Time, F> =
    Vec3 { x: v.x * t, y: v.y * t, z: v.z * t };

// Transform application — frame changes (the dangerous operation)
type Rotation<From, To> {
    // quaternion representation fields
}
fn operator*<A, B>(r: Rotation<A, B>, v: Vec3<Length, A>) -> Vec3<Length, B> = {
    // rotation logic
};

// Transform composition — middle-type cancellation
fn operator*<A, B, C>(r1: Rotation<A, B>, r2: Rotation<B, C>) -> Rotation<A, C> = {
    // quaternion multiplication
};

// Magnitude — consumes the frame, returns frameless scalar
fn magnitude<D: Dim, F>(v: Vec3<D, F>) -> D =
    sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
```

**Cross-type operators are the trust boundary.** Sguaba uses Rust's `unsafe` to mark "the programmer asserts this frame relationship is correct." In Graphcal, user-defined cross-type operators serve the same role — they are the explicit, auditable locations where frame relationships are established.

### 5. `as` Cast (Escape Hatch)

Sometimes you need to deliberately override the type system — you're writing a test, initializing a value from raw data, or you know two frames are aligned. The `as` cast provides a **per-value escape hatch** that changes phantom type parameters:

```
// Reframe a vector — the programmer asserts this is correct:
node v_eci = @v_body as Vec3<Length, ECI>;

// Reframe a timestamp:
node t_jst = @t_utc as Timestamp<JST>;

// Strip frames (go back to Unframed):
node v_plain = @v_eci as Vec3<Length, Unframed>;
// Now v_plain is Vec3<Length>, usable without frame tracking.
```

**Validity rule:** `expr as T` is valid if and only if:
1. The source and target are instantiations of the **same generic type** (same type constructor).
2. After substituting type parameters, **all fields have identical types**.

This means only phantom parameters (which don't affect field types) can be changed via `as`. Parameters that affect field types are rejected:

```
// ✓ Valid: F is phantom (doesn't appear in fields), so changing it is safe.
@v_body as Vec3<Length, ECI>
// Source fields: x: Length, y: Length, z: Length
// Target fields: x: Length, y: Length, z: Length  — match ✓

// ✗ Invalid: D affects field types, so changing it is rejected.
@v_body as Vec3<Velocity, Body>
// Source fields: x: Length, y: Length, z: Length
// Target fields: x: Velocity, y: Velocity, z: Velocity  — mismatch ✗

// ✗ Invalid: different type constructors.
@some_pair as Vec3<Length, ECI>
// Pair ≠ Vec3  — different types ✗
```

**Why per-value, not block-scoped:**

Sguaba uses Rust's `unsafe` blocks, where everything inside the block bypasses safety checks. Graphcal uses per-value `as` instead because:

- **Minimal blast radius.** Each cast is one value, one assertion. An `unchecked {}` block silently downgrades ALL type checking inside it, including non-phantom parameters — you could accidentally add `Vec3<Length>` and `Vec3<Velocity>` without noticing.
- **Auditable.** Every `as` cast is grep-able. Code review can find all frame assertions by searching for `as Vec3` or `as Timestamp`.
- **Composable.** You can cast one operand and keep the other checked: `@v_eci + (@v_body as Vec3<Force, ECI>)` — the first operand is still type-checked.

**Relationship to `Rotation`:** The `as` cast and `Rotation<A, B>` serve different purposes:
- `Rotation<A, B>` is a **physically meaningful transform** — it rotates the vector's components.
- `as` is a **type assertion** — it changes the type without changing the value. Use it when you know the frames are aligned (e.g., at epoch, Body ≈ ECI) or when you're constructing values from raw data.

## Use Cases

### Coordinate Frames

The primary use case. Mixing reference frames is always wrong and has caused real mission failures (Mars Climate Orbiter).

```
// Frame marker types
type ECI {}
type Body {}
type LVLH {}
type Unframed {}

// Framed 3D vector — F defaults to Unframed for users who don't need frames
type Vec3<D: Dim, F = Unframed> derive(Add, Sub, Neg) {
    x: D, y: D, z: D,
}

// ---- Simple usage (no frame tracking) ----

// Users who don't care about frames just write Vec3<Velocity>:
param delta_v: Vec3<Velocity> = Vec3 { x: 0.1 km/s, y: 0 km/s, z: 0 km/s };
// This is Vec3<Velocity, Unframed>. Simple, no cognitive overhead.

// ---- Frame-safe usage ----

// Position and velocity in ECI
param pos: Vec3<Length, ECI> = Vec3 { x: 6878 km, y: 0 km, z: 0 km };
param vel: Vec3<Velocity, ECI> = Vec3 { x: 0 km/s, y: 7.5 km/s, z: 0 km/s };

// Same-frame addition — derived, just works
node total_force: Vec3<Force, Body> = @thrust + @drag;

// Cross-frame — compile error, types don't unify
node bad = @thrust_body + @gravity_eci;
//         ^^^^^^^^^^^^   ^^^^^^^^^^^^
//         Vec3<Force, Body>  Vec3<Force, ECI>  — type error

// Mixing framed and unframed — also a compile error
node also_bad = @pos + @delta_v;
//              ^^^^   ^^^^^^^^
//              Vec3<_, ECI>  Vec3<_, Unframed>  — type error (good! be explicit)

// Frame transform — user-defined operator, explicit
param attitude: Rotation<ECI, Body> = ...;
node gravity_body: Vec3<Force, Body> = @attitude * @gravity_eci;

// Escape hatch — deliberate reframing via as cast
node gravity_approx: Vec3<Force, Body> = @gravity_eci as Vec3<Force, Body>;
// Programmer asserts: "I know ECI ≈ Body for this calculation."
```

### Time Zones

Time zone bugs are universally understood. Mixing UTC with JST is always wrong.

```
type UTC {}
type JST {}
type EST {}

type Timestamp<TZ> {
    epoch_seconds: Time,
}

// Duration is a separate, unframed type — no timezone parameter
// (this is just the built-in Time dimension)

// User-defined: timestamp + duration → timestamp (timezone preserved)
fn operator+<TZ>(t: Timestamp<TZ>, d: Time) -> Timestamp<TZ> =
    Timestamp { epoch_seconds: t.epoch_seconds + d };

// User-defined: timestamp - timestamp → duration (timezone consumed)
fn operator-<TZ>(a: Timestamp<TZ>, b: Timestamp<TZ>) -> Time =
    a.epoch_seconds - b.epoch_seconds;

param launch: Timestamp<UTC> = Timestamp { epoch_seconds: 14.5 hr };
param coast: Time = 45 min;

// Timezone-safe addition — user-defined operator
node arrival: Timestamp<UTC> = @launch + @coast;

// Mixing timezones — compile error
param display_jst: Timestamp<JST> = ...;
node bad = @launch + @display_jst;   // type error: Timestamp<UTC> vs Timestamp<JST>

// Timezone conversion — explicit user-defined function (knows the offset)
fn utc_to_jst(t: Timestamp<UTC>) -> Timestamp<JST> =
    Timestamp { epoch_seconds: t.epoch_seconds + 9 hr };

// Escape hatch — reframe via as when you know what you're doing
// (e.g., test data, or the offset has already been applied)
node t_jst_raw = @some_utc_timestamp as Timestamp<JST>;
```

**Key difference from the old tag design:** Duration (`Time`) is not an "untagged Timestamp" — it's a completely different type. There is no "sticky merge" where `Timestamp<UTC> + Time → Timestamp<UTC>` happens via magic rules. Instead, the user explicitly defines `operator+(Timestamp<TZ>, Time) -> Timestamp<TZ>`. This is one line of code and makes the semantics visible.

### Affine Space (Points vs Displacements)

The generics approach naturally supports the affine space distinction that was awkward in the old tag system:

```
type Position<F> derive(Eq) {
    x: Length, y: Length, z: Length,
}

type Displacement<F> derive(Add, Sub, Neg) {
    x: Length, y: Length, z: Length,
}

// Position - Position = Displacement (user-defined)
fn operator-<F>(a: Position<F>, b: Position<F>) -> Displacement<F> =
    Displacement { x: a.x - b.x, y: a.y - b.y, z: a.z - b.z };

// Position + Displacement = Position (user-defined)
fn operator+<F>(p: Position<F>, d: Displacement<F>) -> Position<F> =
    Position { x: p.x + d.x, y: p.y + d.y, z: p.z + d.z };

// Position + Position → NOT DEFINED → compile error
// Displacement + Displacement → derived via derive(Add)
```

This was a deferred open question under the old design. With user-defined operators, it's straightforward.

### Transform Composition

```
type Rotation<From, To> { /* quaternion fields */ }

// Application: Rotation<A, B> * Displacement<A> → Displacement<B>
fn operator*<A, B>(r: Rotation<A, B>, v: Displacement<A>) -> Displacement<B> = ...;

// Composition: Rotation<A, B> * Rotation<B, C> → Rotation<A, C>
fn operator*<A, B, C>(r1: Rotation<A, B>, r2: Rotation<B, C>) -> Rotation<A, C> = ...;

// Inverse
fn inverse<A, B>(r: Rotation<A, B>) -> Rotation<B, A> = ...;
```

The middle-type cancellation (`B` must match in composition) falls out of generic type unification — no special language rule needed.

## Comparison with Previous Tag System

The old design used a built-in `tag` keyword with special rules. Here is why the new approach is preferred:

| Aspect | Old: Built-in Tags | New: Generics + Derive + Operators |
| --- | --- | --- |
| **Mechanism count** | Special `tag` keyword, `merge` function, `dimensionless auto-clear`, `as` cast — four new concepts | Generic structs, default params, `derive`, `fn operator`, `as` cast — five concepts that each serve many purposes |
| **Safety enforcement** | Built-in merge rules | Type unification (the same mechanism that catches `Length + Mass`) |
| **Affine space** | Deferred open question | User-defined naturally (Position vs Displacement types) |
| **Transform composition** | Deferred open question | User-defined naturally (multi-parameter generics) |
| **Sticky merge** | Automatic: `tagged + untagged → tagged` | Explicit: user defines `operator+(Timestamp<TZ>, Time)` |
| **Dimensionless auto-clear** | Built-in rule | Not needed — types are structural, no flat tag set to clear |
| **Use case scope** | Tried to cover everything (species, materials, budgets, cost categories) | Focused on high-value cases (coordinate frames, time zones) |
| **Operator behavior** | Hardcoded, same for all tag families | User-defined per type, can express domain-specific algebra |
| **Learning cost** | New concepts unique to Graphcal | Patterns familiar from Rust, Haskell, C++ (phantom types, operator overloading) |
| **Explicitness** | `merge` rules run implicitly on every arithmetic op | Every operator is visibly defined or derived |

## Known Gaps

### 1. No Bare Scalar Tagging

The old tag system allowed `Length<Frame.ECI>` — a tagged bare scalar with no wrapper struct. The new approach requires a struct for any tagged value:

```
// Old: Length<Frame.ECI> — direct
// New: needs a wrapper
type FramedLength<F> derive(Add, Sub, Neg) { value: Length }
```

**Mitigation:** The primary use cases (coordinate frames, time zones) naturally involve multi-field structs (`Vec3`, `Timestamp`). The wrapper is one line for the rare 1D case. There is no use case where this is prohibitive.

### 2. Unconstrained Phantom Type Parameters

Without a trait/kind system, phantom type parameters accept any type:

```
type Vec3<D: Dim, F> { x: D, y: D, z: D }

// D is constrained (D: Dim), but F is unconstrained:
// Vec3<Length, ECI>     ✓  (intended)
// Vec3<Length, Bool>    ✓  (compiles but nonsensical)
// Vec3<Length, Length>  ✓  (compiles but nonsensical)
```

**Mitigation:** This is not dangerous — `Vec3<Length, Bool>` still can't be mixed with `Vec3<Length, ECI>`. It's messy, not unsafe. A future constraint system (`F: Frame`) could tighten this, but it's not required for correctness.

### 3. Type-Level Dimension Arithmetic in Generics

To avoid defining separate types per dimension (`Vec3Position`, `Vec3Velocity`, `Vec3Force`...), the type system must support dimension arithmetic in generic return types:

```
// This requires evaluating D * Time at the type level:
fn operator*<D: Dim, F>(v: Vec3<D, F>, t: Time) -> Vec3<D * Time, F> =
    Vec3 { x: v.x * t, y: v.y * t, z: v.z * t };
```

This is a non-trivial type system feature. Without it, users must define separate types and operators for each dimension combination (type explosion). With it, one generic `Vec3<D, F>` and a few generic operators cover all cases.

**Mitigation:** Graphcal already has dimension algebra as a built-in. Extending it to work inside generic type expressions is a natural generalization, not a fundamentally new capability.

### 4. More Boilerplate Than Built-In Tags

The old system required one `tag Frame { ECI, Body }` declaration. The new system requires: empty marker types, a generic struct, derive annotations, and user-defined cross-type operators. For coordinate frames:

```
// ~15 lines of setup (defined once, used everywhere):
type ECI {}
type Body {}
type LVLH {}

type Vec3<D: Dim, F> derive(Add, Sub, Neg) {
    x: D, y: D, z: D,
}

fn operator*<D: Dim, F>(v: Vec3<D, F>, s: Dimensionless) -> Vec3<D, F> =
    Vec3 { x: v.x * s, y: v.y * s, z: v.z * s };

fn operator*<D: Dim, F>(v: Vec3<D, F>, t: Time) -> Vec3<D * Time, F> =
    Vec3 { x: v.x * t, y: v.y * t, z: v.z * t };
```

**Mitigation:** This is library code — defined once, imported everywhere. The standard library or a prelude package can provide common types (`Vec3`, `Quaternion`, `Rotation`, `Timestamp`). The boilerplate pays for itself in expressiveness (affine space, transforms, custom algebra).

### 5. Operator Overloading Makes `+` Context-Dependent

This is the strongest objection given Graphcal's "explicitness over implicitness" philosophy. When you see `a + b`, you now need to know the types of `a` and `b` to know what `+` does:

```
v1 + v2          // Vec3 derive(Add): component-wise
p + d            // Position + Displacement: user-defined, returns Position
t1 - t2          // Timestamp - Timestamp: user-defined, returns Time
```

**Mitigation:** This is the same tradeoff every language with operator overloading makes. The opt-in derive makes it visible at the type declaration site. User-defined operators are explicit function definitions that can be audited. The alternative (named functions only: `add_vec(v1, v2)`, `translate(p, d)`) is safe but significantly more verbose for math-heavy engineering calculations. The opt-in derive model is a deliberate compromise: we accept context-dependent `+` because the alternative hurts readability for the primary use case (engineering math).

## Open Questions

### Critical (Must Resolve Before Implementation)

- **Derive set:** Exactly which operators can be derived? Proposed: `Add`, `Sub`, `Neg`, `Eq`, `Ord`. Should `ScalarMul` and `ScalarDiv` be derivable (they are cross-type but structurally obvious)? Or always user-defined?
- **Operator resolution:** When both a derived operator and a user-defined operator could match, which wins? Options: (a) compile error (no ambiguity allowed), (b) user-defined wins, (c) most specific type wins.
- **Derive syntax:** `derive(Add, Sub)` on the type declaration? Or a separate `derive Add for Vec3`? The inline syntax is simpler; the separate syntax allows deriving in a different file.

### Important (Should Resolve Before Maturity)

- **Type-level dimension arithmetic:** Can `D * Time` appear as a type expression when `D: Dim`? This is essential for avoiding type explosion (see Gap 3). Needs formal specification of what type-level expressions are allowed.
- **Phantom parameter constraints:** Should there be a lightweight way to constrain phantom parameters (e.g., `type Vec3<D: Dim, F: marker>` where `marker` means "zero-sized type only")? Or is the unconstrained approach acceptable?
- **Operator generality:** Can user-defined operators be generic over dimension (`<D: Dim>`)? This is needed for the `Vec3<D, F> * Time → Vec3<D * Time, F>` pattern. Requires generics in operator signatures.
- **Standard library types:** Should the prelude include `Vec3<D, F>`, `Quaternion<F>`, `Rotation<From, To>`, `Timestamp<TZ>` with standard derives and operators? Or should these always be user-defined?

### Deferred

- **Trait / typeclass system:** A full trait system would allow `F: Frame` constraints, default operator implementations, and more. This is a large language feature and may not be needed if the phantom parameter approach covers the primary use cases.
- **Conventions (axis naming):** Sguaba's two-level system (frame identity + NED/FRD/ENU convention). Could be modeled as a trait: `type PlaneNed: NedConvention {}`. Deferred until traits are considered.
- **Runtime frame selection:** Can the frame type parameter be determined at runtime? Probably not — phantom types are compile-time only. If runtime frame dispatch is needed, that's a tagged union, not a phantom type.

## Dependencies on Other Aspects

- **Dimensions** ([04](./04-dimensions-and-units.md)): Struct fields carry dimensions. Type-level dimension arithmetic (`D * Time`) extends the existing dimension algebra.
- **Algebraic Data Types** ([05](./05-algebraic-data-types.md)): Generic structs are the foundation. `derive` extends the ADT system. Phantom type parameters are a new capability for `type` declarations.
- **Pure Functions** ([12](./12-pure-functions.md)): `fn operator*<A, B>(...)` extends the function system with operator names and generics. Operator overloading is a new capability for `fn`.
- **Syntax** ([02](./02-syntax-design.md)): `derive(...)` clause on type declarations. `fn operator+` / `fn operator*` / etc. for operator definitions. `type ECI {}` for zero-field marker types.
- **Non-SI Dimensions** ([18](./18-non-si-dimensions.md)): Counting quantities may still use separate dimensions or phantom-typed wrappers — the approach doesn't prescribe either.

## Superseded Designs

The following concepts from the previous tag-based design are **no longer part of this approach**:

- **`tag` keyword** — replaced by empty `type` declarations for markers and generic struct type parameters for tagging.
- **`merge` function** — replaced by generic type unification. No implicit tag propagation; all operations are derived or user-defined.
- **`dimensionless auto-clear` rule** — not needed. There is no flat tag set that needs clearing; the type system is structural.
- **`as` cast for tag stripping** — repurposed. `as` now changes phantom type parameters on generic structs (e.g., `@v as Vec3<Length, ECI>`), validated by field-type compatibility. No longer operates on bare dimensions.
- **Sticky merge (`tagged + untagged → tagged`)** — replaced by user-defined operators (e.g., `operator+(Timestamp<TZ>, Time) -> Timestamp<TZ>`).
- **Tag families and variants** — replaced by plain empty types. `type ECI {}` instead of `tag Frame { ECI, ... }`. Frames are just types, not members of a declared family.
- **Multi-family tag sets** — replaced by multiple type parameters. `Vec3<D, Frame, Craft>` instead of `Force<Frame.ECI, Craft.Chaser>`.
