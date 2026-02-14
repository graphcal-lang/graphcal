# Type System -- Spaces

> Layer 5: Semantic context tags preventing cross-context mixing (Sguaba-inspired).

## Status

**Decision level:** Concept established, design details open. The core idea (compile-time tags that prevent mixing same-dimension values from different contexts) is sound, but arithmetic propagation rules, multi-space composition, and several other design questions remain unresolved.

## Summary

Inspired by [Sguaba](https://github.com/helsing-ai/sguaba), which uses Rust phantom types to prevent mixing vectors from different coordinate systems. The core insight: values can share the same dimension but live in **different semantic spaces** that must not be mixed.

A `space` declares a family of semantically distinct contexts. The `in` keyword tags a value with its space.

## Why Spaces (Not Alternatives)

Several mechanisms could solve the "same dimension, must not mix" problem. Spaces are the best fit for the full range of use cases:

| Mechanism | How It Works | Limitations |
| --- | --- | --- |
| **Separate base dimensions** | `dimension Person; dimension Packet;` — each gets its own axis | Only works for counting. Breaks for tagging same physical dimension (`Length` in ECI vs Body). Creates spurious derived dimensions (`Person / Packet`). |
| **Branded newtypes** | Opaque wrapper types, manual unwrap | Very verbose. Doesn't compose with dimensional algebra. |
| **Parameterized dimensions** | `Length<Frame.ECI>` — tags are part of the dimension type | Elegant but deeply couples tagging with the dimension algebra, making the type system significantly more complex. |
| **Spaces (orthogonal tags)** | `Length in Frame.ECI` — tags are a separate layer | Composes cleanly with dimensions. Works for all use case families. Arithmetic propagation rules need design work. |

The key advantage of Spaces over alternatives: the tagging is **orthogonal** to dimensional algebra. `Length in Frame.ECI * Time` doesn't require the dimension system to understand space tags — the space layer and dimension layer compose independently.

## Use Cases

### Original Use Cases (Aerospace / Physics)

| Domain | Space | Variants | Prevents |
| --- | --- | --- | --- |
| Coordinate frames | `Frame` | `Body`, `ECI`, `ECEF`, `LVLH` | Mixing reference frames |
| Spacecraft identity | `Craft` | `Chaser`, `Target`, `Depot` | Mixing per-vehicle budgets |
| Budget categories | `Budget` | `Allocated`, `Spent`, `Remaining` | Mixing budget columns |
| Time epochs | `Epoch` | `UTC`, `GPS`, `MissionElapsed` | Mixing time references |

### Counting Discrete Things

Counting dimensions (crew members, packets, pixels, etc.) are a major new use case. Rather than creating a separate base dimension per countable thing ([18-non-si-dimensions.md](./18-non-si-dimensions.md)), they should be a single `Count` dimension with space tags:

| Domain | Space | Variants | Prevents |
| --- | --- | --- | --- |
| Countable entities | `Countable` | `Person`, `Satellite`, `Cycle`, `Pixel`, `Packet` | Mixing counts of different things |

```
dimension Count;
unit count: Count;

space Countable {
    Person;
    Satellite;
    Cycle;
}

param crew: Count in Countable.Person = 7 count;
param sats: Count in Countable.Satellite = 24 count;

node bad = @crew + @sats;
//  error[S001]: space mismatch: Countable.Person != Countable.Satellite
```

This use case is important because it **stress-tests the arithmetic propagation rules** (see open questions below). For counting to work with spaces, multiplication must interact correctly with tags:

```
// "7 people * 80 kg/person = 560 kg associated with crew"
param crew: Count in Countable.Person = 7 count;
param mass_per_person: Mass / Count in Countable.Person = 80 kg / count;
node crew_mass: Mass in Countable.Person = @crew * @mass_per_person;

// To combine with structural mass, explicitly leave the Person context:
node total_mass: Mass = @crew_mass.untagged + @structure_mass;
```

### Engineering Domains

| Domain | Space | Variants | Example | Prevents |
| --- | --- | --- | --- | --- |
| Chemical species | `Species` | `O2`, `N2`, `CO2` | `param p_O2: Pressure in Species.O2 = 21.3 kPa;` | Mixing partial pressures |
| Material grades | `Material` | `Steel_A36`, `Al_6061` | `param fy: Pressure in Material.Steel_A36 = 250 MPa;` | Using wrong material properties |
| Load cases | `LoadCase` | `Static`, `Thermal`, `Dynamic` | `node sigma: Pressure in LoadCase.Static = ...;` | Mixing load case results |
| Flight phases | `Phase` | `Ascent`, `Coast`, `Descent` | `param fuel: Mass in Phase.Ascent = 1000 kg;` | Mixing phase-specific budgets |
| Cost categories | `CostCategory` | `Development`, `Production`, `Operations` | `param dev: Money in CostCategory.Development = 50M USD;` | Mixing lifecycle cost categories |

### Cross-Space Operations in Practice

Many real calculations intentionally combine values from different space variants. Examples:

**Dalton's law** (total pressure = sum of partial pressures):
```
node total_pressure: Pressure =
    @p_O2.untagged + @p_N2.untagged + @p_CO2.untagged;
```

**Total lifecycle cost**:
```
node total_cost: Money =
    @dev_cost.untagged + @prod_cost.untagged + @ops_cost.untagged;
```

**Total fuel budget** (across flight phases):
```
node total_fuel: Mass =
    @fuel_ascent.untagged + @fuel_coast.untagged + @fuel_descent.untagged;
```

In all cases, `.untagged` serves as an intentional marker that the cross-context combination is deliberate.

## Syntax

### Declaration

```rust
space Frame {
    Body;
    ECI;
    ECEF;
    LVLH;
}
```

### Tagging Values

```rust
param sat_position: Vec3<Length> in Frame.ECI = ...;
param thrust_body: Vec3<Force> in Frame.Body = ...;
```

### Compile-Time Enforcement

```rust
node bad = @sat_position + @thrust_body;
//  error[S001]: space mismatch: Frame.ECI != Frame.Body
```

## Crossing Space Boundaries

Two mechanisms for intentional cross-space operations:

### 1. `.untagged` -- Manual Escape Hatch

```rust
node combined_mass: Mass =
    @chaser_mass.untagged
    + @target_mass.untagged;
```

The `.untagged` call is a signal to reviewers that a cross-space operation is intentional.

### 2. `Transform` -- Typed Conversion

```rust
node eci_to_body: Transform<Frame.ECI, Frame.Body> = {
    Transform.from_rotation(@attitude_quaternion)
};

node thrust_eci: Vec3<Force> in Frame.ECI = @eci_to_body.inverse() * @thrust_body;
```

Note: `Transform` is specifically suited to coordinate frame conversions. Not all space families have a meaningful transform concept (e.g., there is no "transform" between `Species.O2` and `Species.N2`). For most space families, `.untagged` is the only crossing mechanism.

## Interaction with Other Layers

- The `in` tag is **optional**. Untagged values are the default.
- Spaces are **orthogonal** to dimensions: `Vec3<Length> in Frame.ECI` has dimension `Length` and space `Frame.ECI`.
- Space tags compose with all other type layers.

## Open Questions

### Critical (Must Resolve Before Implementation)

- **Arithmetic propagation rules:** How do space tags propagate through multiplication and division? This is the single most important unresolved question and blocks the counting-as-spaces use case. Candidate rules:

  - **(a) Additive only:** Spaces are checked on `+`/`-` (must match). `*`/`/` always strips tags. Simple but loses information — `Force in Frame.Body * Length in Frame.Body` loses the Body tag.
  - **(b) Propagate on match:** If both operands share the same space tag, the result inherits it. If one is untagged, result inherits the tagged operand's space. If they have *different* tags from the same space family, it's an error. This is the richest rule and handles coordinate frames (`Force in Body * Length in Body → Torque in Body`) and counting (`Count in Person * Mass/Count in Person → Mass in Person`).
  - **(c) Propagate only for addition, error for mismatched multiplication:** `*`/`/` between differently-tagged values is a compile error. You must `.untagged` first. Very safe, possibly too restrictive.

  The choice affects every downstream use case. Rule (b) seems most practical but needs careful specification for edge cases.

- **Tagged × untagged interaction:** When one operand has a space tag and the other is untagged, what happens?
  - Option 1: Result inherits the tag (`Mass in Frame.Body * Dimensionless → Mass in Frame.Body`). This is the "tag is sticky" rule.
  - Option 2: Result is untagged. This is the "tag is fragile" rule.
  - Option 1 seems more useful — you want `@force_body * @dt` to remain in Body frame.

- **Multi-space values:** Can a value belong to multiple spaces simultaneously? E.g., `in Frame.ECI in Craft.Chaser`? If yes, each space family is tracked independently during arithmetic. This is important for real-world models (force on the chaser, in the ECI frame).

### Important (Should Resolve Before Maturity)

- **Space-generic functions:** Should functions be generic over spaces? E.g., `fn magnitude<S: Frame>(v: Vec3<Length> in S) -> Length`? Essential for writing reusable code that works across space variants.
- **Open vs closed spaces:** Can space families be extended across files? Coordinate frames are typically a closed set, but `Countable` variants may need to be extensible across libraries.
- **Space in type position vs value position:** Is `in Frame.ECI` part of the type signature or an annotation on the value? This affects type inference, function signatures, and error messages.
- **Transform generality:** Is `Transform<A, B>` a coordinate-frame-specific concept, or a general mechanism for all space families? Most space families (Species, Material, CostCategory) have no meaningful transform.

### Deferred

- **Space arithmetic (Transform composition):** When two `Transform` types are composed, how does the type system track the chain? E.g., `Transform<A, B> * Transform<B, C> -> Transform<A, C>`?
- **`.untagged` auditing:** Should uses of `.untagged` be flagged in a lint pass or report? This would help code review.
- **Inheritance / hierarchy:** Can spaces have sub-spaces? E.g., `Frame.ECI` is a refinement of `Frame.Inertial`?
- **Runtime space selection:** Can the space variant be determined at runtime (e.g., from a parameter), or is it always compile-time?
- **Variant separators:** The examples show semicolons after each variant. Is this consistent with `index` (which uses commas)?

## Dependencies on Other Aspects

- **Dimensions** ([04](./04-dimensions-and-units.md)): Spaces are orthogonal to dimensions; they compose.
- **Non-SI Dimensions** ([18](./18-non-si-dimensions.md)): Counting quantities use spaces rather than separate base dimensions. Requires resolving arithmetic propagation rules.
- **Algebraic Data Types** ([05](./05-algebraic-data-types.md)): `Transform` may be a built-in type or user-defined.
- **Pure Functions** ([12](./12-pure-functions.md)): Functions can accept space-tagged parameters. Space-generic functions are an open question.
- **Scoping** ([08](./08-scoping.md)): Space tags appear in type annotations at the `@` reference site.
