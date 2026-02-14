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

## Syntax Exploration

The syntax has three independent axes: (1) declaration keyword, (2) annotation syntax for tagging values, and (3) escape syntax for removing tags. Each is evaluated below.

### Axis 1: Declaration Keyword

The word `space` was chosen because of the coordinate-frame origin (Sguaba). As the feature broadens to non-spatial use cases, does the name still work?

| Keyword | Reads Well For | Reads Poorly For |
| --- | --- | --- |
| `space` | Coordinate frames (`space Frame`), mathematical spaces | Chemical species (`space Species`??), material grades (`space Material`??) |
| `tag` | All use cases — it's what the feature IS | Slightly informal/mechanical |
| `context` | Budget, phases, epochs ("the development context") | Coordinate frames ("the ECI context" is unusual) |
| `label` | Counting, categories | Coordinate frames, chemical species |
| `brand` | Type-theory accuracy | Too obscure for engineering users |
| `family` | Categorization | Too abstract, no clear precedent in PLs |

**Evaluation**: `space` is domain-specific jargon that works for physics but confuses elsewhere. `tag` is the most honest name — it describes exactly what the feature does (attach a compile-time tag) without pretending to be domain-specific. `context` is a reasonable runner-up.

### Axis 2: Annotation Syntax

This is where the current design is weakest. `in` reads naturally for spatial/container contexts but breaks down for classification, properties, and counting.

Read each column top-to-bottom to feel how the syntax reads across diverse use cases:

```
  USE CASE                 in (current)                        [brackets]                       for                            of

  Coordinate frame         Length in Frame.ECI                 Length[Frame.ECI]                Length for Frame.ECI           Length of Frame.ECI
  Spacecraft identity      Mass in Craft.Chaser                Mass[Craft.Chaser]               Mass for Craft.Chaser          Mass of Craft.Chaser
  Counting                 Count in Countable.Person           Count[Countable.Person]          Count for Countable.Person     Count of Countable.Person
  Partial pressure         Pressure in Species.O2              Pressure[Species.O2]             Pressure for Species.O2        Pressure of Species.O2
  Material property        Pressure in Material.Steel_A36      Pressure[Material.Steel_A36]     Pressure for Material.Steel    Pressure of Material.Steel
  Budget category          Money in Budget.Allocated           Money[Budget.Allocated]           Money for Budget.Allocated     Money of Budget.Allocated
  Flight phase             Mass in Phase.Ascent                Mass[Phase.Ascent]               Mass for Phase.Ascent          Mass of Phase.Ascent
  Cost category            Money in CostCategory.Development   Money[CostCategory.Development]  Money for CostCategory.Dev     Money of CostCategory.Dev
```

| Syntax | Pros | Cons |
| --- | --- | --- |
| **`in`** | Natural for spatial ("length in ECI frame"), familiar from SQL/natural language | "Pressure in Species.O2" sounds physical (the pressure is literally inside oxygen?). "Pressure in Material.Steel_A36" is worse. Misleading for non-container contexts. |
| **`[Tag]`** (brackets) | Concise, no English-reading pretense, universal across all use cases, familiar from parameterized types in many PLs | Visually similar to existing `index` syntax `T[I]` — could `Pressure[Species.O2]` be confused with `Pressure[Species]` (indexed table)? Potentially disambiguated: bare family = index, qualified variant = tag. |
| **`for`** | Reads well for ownership/purpose ("mass for the ascent phase", "pressure for O2", "money for development") | "Length for Frame.ECI" is awkward — it's not "for" the frame, it's "expressed in" it. |
| **`of`** | Reads well for belonging ("pressure of O2", "mass of the chaser") | "Length of Frame.ECI" is wrong — the frame doesn't have a length. "Count of Countable.Person" is redundant. |

Additional options considered and rejected:

| Syntax | Why Rejected |
| --- | --- |
| `@Tag` suffix | Conflicts with `@name` graph references. |
| `<Tag>` angle brackets | Conflicts with generics (`Vec3<Length>`). |
| `::Tag` double colon | Looks like module paths (namespace confusion). |
| `~Tag` tilde | Novel but unfamiliar, no precedent. |
| `tagged Tag` keyword | Verbose. |

### Axis 3: Escape Syntax

When you intentionally cross a space boundary, how do you remove the tag?

| Syntax | Example | Pros | Cons |
| --- | --- | --- | --- |
| `.untagged` (current) | `@p_O2.untagged` | Reads as an adjective (get the untagged version) | Long (9 chars). Looks like a field access. |
| `.untag` | `@p_O2.untag` | Shorter. Verb form. | Looks like mutation ("untag this value"). |
| `.raw` | `@p_O2.raw` | Very short. Clear intent. | Implies the tagged version is somehow "cooked." |
| `.strip` | `@p_O2.strip` | Active verb. | Overloaded (string stripping in many PLs). |
| `!` suffix | `@p_O2!` | Extremely concise. | Looks like Rust's macro syntax or Ruby's mutation convention. Could be confused with logical NOT. |
| `untag(expr)` | `untag(@p_O2)` | Function-like syntax, visually distinct | Adds a keyword. Wrapping is more verbose for chained expressions. |

### Interaction of Axis 1 + 2: Combined Syntax Options

Some combinations feel more natural than others. Here are the most coherent pairings:

**Option A: `space` + `in`** (current)
```
space Frame { Body, ECI, ECEF, LVLH }

param pos: Length in Frame.ECI = 6878 km;
param p_O2: Pressure in Species.O2 = 21.3 kPa;
param crew: Count in Countable.Person = 7 count;
```
Verdict: Reads naturally for coordinates, poorly for chemistry/materials.

**Option B: `tag` + `in`**
```
tag Frame { Body, ECI, ECEF, LVLH }

param pos: Length in Frame.ECI = 6878 km;
param p_O2: Pressure in Species.O2 = 21.3 kPa;
param crew: Count in Countable.Person = 7 count;
```
Verdict: Better declaration keyword, but `in` still reads poorly for non-spatial use cases.

**Option C: `tag` + `[brackets]`**
```
tag Frame { Body, ECI, ECEF, LVLH }

param pos: Length[Frame.ECI] = 6878 km;
param p_O2: Pressure[Species.O2] = 21.3 kPa;
param crew: Count[Countable.Person] = 7 count;
param fy: Pressure[Material.Steel_A36] = 250 MPa;

// Escape: use .untag or similar
node total_pressure: Pressure = @p_O2.untag + @p_N2.untag;

// Generic function:
fn magnitude<S: Frame>(v: Vec3<Length>[S]) -> Length = ...;

// Multi-tag:
param force: Force[Frame.ECI][Craft.Chaser] = ...;
```
Verdict: Universal readability. Concise. But the `[Tag]` vs `[Index]` disambiguation needs a rule.

**Option D: `tag` + `for`**
```
tag Frame { Body, ECI, ECEF, LVLH }

param pos: Length for Frame.ECI = 6878 km;
param p_O2: Pressure for Species.O2 = 21.3 kPa;
param crew: Count for Countable.Person = 7 count;
param fuel: Mass for Phase.Ascent = 1000 kg;
```
Verdict: Reads well for most cases. "Length for Frame.ECI" is the weakest link.

### `[Tag.Variant]` vs `[Index]` Disambiguation

If brackets are chosen, there is a potential ambiguity with indexed types (`Velocity[Maneuver]` = a table of velocities). However, tags always use a **qualified variant** (`Frame.ECI`) while indexes use a **bare family name** (`Maneuver`):

```
param dv: Velocity[Maneuver]             // Indexed table — one value per Maneuver variant
param pos: Length[Frame.ECI]             // Tagged — a single value tagged with ECI

// Alternatively, if the disambiguation feels fragile, a sigil could mark tags:
param pos: Length[.Frame.ECI]            // Leading dot signals "specific variant, not table axis"
param pos: Length[#Frame.ECI]            // Hash signals "tag"
```

The bare vs. qualified distinction is arguably natural — asking for "all maneuvers" (table) vs. "specifically ECI" (tag) — but needs to be validated with more examples.

### Recommendation

No final recommendation yet — this section presents the trade-offs for discussion. The strongest contenders are:

1. **`tag` + `[brackets]`** (Option C): Most universal, most concise, most "PL-like." Needs index disambiguation.
2. **`tag` + `for`** (Option D): Most readable English, but "for" is slightly wrong for coordinate frames.
3. **`tag` + `in`** (Option B): Keeps familiar annotation, better declaration keyword.

The current `space` + `in` (Option A) is the weakest because it optimizes for coordinate frames at the expense of the broader use cases now envisioned.

## Crossing Space Boundaries

Two mechanisms for intentional cross-space operations:

### 1. Escape Hatch (`.untagged` / `.untag`)

```rust
// Using current syntax (space + in):
node combined_mass: Mass =
    @chaser_mass.untagged + @target_mass.untagged;

// Using bracket syntax (tag + []):
node combined_mass: Mass =
    @chaser_mass.untag + @target_mass.untag;
```

The escape call is a signal to reviewers that a cross-space operation is intentional.

### 2. `Transform` -- Typed Conversion (Coordinate Frames Only)

```rust
// Using current syntax:
node eci_to_body: Transform<Frame.ECI, Frame.Body> = {
    Transform.from_rotation(@attitude_quaternion)
};
node thrust_eci: Vec3<Force> in Frame.ECI = @eci_to_body.inverse() * @thrust_body;
```

`Transform` is specifically suited to coordinate frame conversions. Not all tag families have a meaningful transform concept (e.g., there is no "transform" between `Species.O2` and `Species.N2`). For most tag families, the escape hatch is the only crossing mechanism.

## Interaction with Other Layers

- Tags are **optional**. Untagged values are the default.
- Tags are **orthogonal** to dimensions: the dimension and tag layers compose independently.
- A value can potentially carry multiple tags from different families (see open questions).

## Open Questions

### Critical (Must Resolve Before Implementation)

- **Syntax choice:** The declaration keyword (`space` vs `tag` vs `context`), annotation syntax (`in` vs `[brackets]` vs `for`), and escape syntax (`.untagged` vs `.untag` vs others) are all open. See the Syntax Exploration section above for a detailed comparison. The current `space` + `in` reads well for coordinate frames but poorly for counting, chemistry, materials, and other non-spatial use cases.

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
