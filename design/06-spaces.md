# Type System -- Spaces

> Layer 5: Semantic context tags preventing cross-context mixing (Sguaba-inspired).

## Status

**Decision level:** Advancing. Syntax: **`tag` + generics `<>` + `as` cast** (Option E). Tag propagation: **uniform `merge`** across all arithmetic operations (including `+`/`-`). Some questions remain (see Open Questions).

## Summary

Inspired by [Sguaba](https://github.com/helsing-ai/sguaba), which uses Rust phantom types to prevent mixing vectors from different coordinate systems. The core insight: values can share the same dimension but live in **different semantic spaces** that must not be mixed.

A `tag` declares a family of semantically distinct contexts. Tags are applied as type parameters using generics syntax (`Length<Frame.ECI>`), and removed via explicit `as` cast (`@pos as Length`).

## Why Spaces (Not Alternatives)

Several mechanisms could solve the "same dimension, must not mix" problem. Spaces are the best fit for the full range of use cases:

| Mechanism | How It Works | Limitations |
| --- | --- | --- |
| **Separate base dimensions** | `dimension Person; dimension Packet;` — each gets its own axis | Only works for counting. Breaks for tagging same physical dimension (`Length` in ECI vs Body). Creates spurious derived dimensions (`Person / Packet`). |
| **Branded newtypes** | Opaque wrapper types, manual unwrap | Very verbose. Doesn't compose with dimensional algebra. |
| **Parameterized dimensions** | Tags embedded in the dimension type | Elegant but deeply couples tagging with the dimension algebra, making the type system significantly more complex. |
| **Tags (orthogonal layer)** | `Length<Frame.ECI>` — tags are type parameters, separate from dimensions | Composes cleanly with dimensions. Works for all use case families. |

The key advantage of tags over alternatives: the tagging is **orthogonal** to dimensional algebra. `Length<Frame.ECI> * Time` doesn't require the dimension system to understand tags — the tag layer and dimension layer compose independently via the `merge` function.

## Use Cases

### Original Use Cases (Aerospace / Physics)

| Domain | Tag Family | Variants | Prevents |
| --- | --- | --- | --- |
| Coordinate frames | `Frame` | `Body`, `ECI`, `ECEF`, `LVLH` | Mixing reference frames |
| Spacecraft identity | `Craft` | `Chaser`, `Target`, `Depot` | Mixing per-vehicle budgets |
| Budget categories | `Budget` | `Allocated`, `Spent`, `Remaining` | Mixing budget columns |
| Time epochs | `Epoch` | `UTC`, `GPS`, `TAI`, `TDB`, `MissionElapsed` | Mixing astrodynamic time scales |
| Time zones | `TimeZone` | `UTC`, `EST`, `PST`, `JST`, `CET` | Mixing civil time references |

### Counting Discrete Things

Counting dimensions (crew members, packets, pixels, etc.) are a major new use case. Rather than creating a separate base dimension per countable thing ([18-non-si-dimensions.md](./18-non-si-dimensions.md)), they should be a single `Count` dimension with tags:

| Domain | Tag Family | Variants | Prevents |
| --- | --- | --- | --- |
| Countable entities | `Countable` | `Person`, `Satellite`, `Cycle`, `Pixel`, `Packet` | Mixing counts of different things |

```
dimension Count;
unit count: Count;

tag Countable { Person, Satellite, Cycle }

param crew: Count<Countable.Person> = 7 count;
param sats: Count<Countable.Satellite> = 24 count;

node bad = @crew + @sats;
//  error[T001]: tag mismatch: Countable.Person != Countable.Satellite
```

This use case is important because it **stress-tests the arithmetic propagation rules**. For counting to work with tags, multiplication must interact correctly:

```
// "7 people * 80 kg/person = 560 kg associated with crew"
param crew: Count<Countable.Person> = 7 count;
param mass_per_person: (Mass / Count)<Countable.Person> = 80 kg / count;
node crew_mass: Mass<Countable.Person> = @crew * @mass_per_person;

// To combine with structural mass, explicitly strip the Person tag:
node total_mass: Mass = @crew_mass as Mass + @structure_mass;
```

### Time Zones

Time zones are a universally understood tagging use case. Every programmer has dealt with time zone bugs; tags prevent them at compile time.

```
tag TimeZone { UTC, EST, PST, JST, CET }

param launch_time: Time<TimeZone.UTC> = 14.5 hr;      // 14:30 UTC
param local_display: Time<TimeZone.JST> = 23.5 hr;    // 23:30 JST

// Duration (untagged Time) — timezone-independent
param coast: Time = 45 min;

// Tagged time + untagged duration → tagged time (tag sticks via merge)
node arrival_utc: Time<TimeZone.UTC> = @launch_time + @coast;
// merge({TimeZone: UTC}, {}) = {TimeZone: UTC} ✓

// Mixing time zones → compile error
node bad = @launch_time + @local_display;
// merge({TimeZone: UTC}, {TimeZone: JST}) → COMPILE ERROR ✓

// Explicit conversion via as:
node combined: Time = @launch_time as Time + @local_display as Time;
// Programmer asserts: "I know these are in different zones; I want to add the raw durations."
```

**Why this use case matters for the merge-rule debate:** Under strict `+`/`-` rules, `@launch_time + @coast` would be rejected because `{TimeZone: UTC} ≠ {}`. You'd need to write `@coast as Time<TimeZone.UTC>`, which is nonsensical — a 45-minute duration doesn't "belong to" UTC. Uniform merge handles this naturally: the untagged duration acquires the tag from the other operand. See [Arithmetic Rules](#arithmetic-rules) below.

**Relationship to `Epoch`:** Time zones (`TimeZone`) and time epochs (`Epoch`) are distinct tag families:
- `Epoch` (UTC, GPS, TAI, TDB): Different time *scales* with fixed mathematical offsets. Critical in astrodynamics.
- `TimeZone` (UTC, EST, JST): Civil time offsets that can vary (DST). Critical in operations.

A launch time could carry both: `Time<Epoch.UTC, TimeZone.UTC>` — the epoch and display zone are both UTC but convey different information.

### Engineering Domains

| Domain | Tag Family | Variants | Example | Prevents |
| --- | --- | --- | --- | --- |
| Chemical species | `Species` | `O2`, `N2`, `CO2` | `param p_O2: Pressure<Species.O2> = 21.3 kPa;` | Mixing partial pressures |
| Material grades | `Material` | `Steel_A36`, `Al_6061` | `param fy: Pressure<Material.Steel_A36> = 250 MPa;` | Using wrong material properties |
| Load cases | `LoadCase` | `Static`, `Thermal`, `Dynamic` | `node sigma: Pressure<LoadCase.Static> = ...;` | Mixing load case results |
| Flight phases | `Phase` | `Ascent`, `Coast`, `Descent` | `param fuel: Mass<Phase.Ascent> = 1000 kg;` | Mixing phase-specific budgets |
| Cost categories | `CostCategory` | `Development`, `Production`, `Operations` | `param dev: Money<CostCategory.Development> = 50M USD;` | Mixing lifecycle cost categories |

### Cross-Tag Operations in Practice

Many real calculations intentionally combine values from different tag variants. Examples:

**Dalton's law** (total pressure = sum of partial pressures):
```
node total_pressure: Pressure =
    @p_O2 as Pressure + @p_N2 as Pressure + @p_CO2 as Pressure;
```

**Total lifecycle cost**:
```
node total_cost: Money =
    @dev_cost as Money + @prod_cost as Money + @ops_cost as Money;
```

**Total fuel budget** (across flight phases):
```
node total_fuel: Mass =
    @fuel_ascent as Mass + @fuel_coast as Mass + @fuel_descent as Mass;
```

**Time zone conversion** (display same instant in different zones):
```
param offset_jst: Time = 9 hr;
node launch_jst: Time<TimeZone.JST> =
    (@launch_utc as Time + @offset_jst) as Time<TimeZone.JST>;
```

In all cases, `as` serves as an intentional marker that the cross-tag combination is deliberate.

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

### Recommendation: Option E — `tag` + Generics `<>` + `as` Cast

After exploring options A–D, a fifth option emerged that resolves most open questions cleanly: **tags are type parameters on dimensions**, using existing generics syntax, with `as` for explicit cast/untag.

```
tag Frame { Body, ECI, ECEF, LVLH }
tag Species { O2, N2, CO2 }
tag Countable { Person, Satellite, Cycle }
tag CostCategory { Development, Production, Operations }

param pos: Length<Frame.ECI> = 6878 km;
param p_O2: Pressure<Species.O2> = 21.3 kPa;
param crew: Count<Countable.Person> = 7 count;
param fy: Pressure<Material.Steel_A36> = 250 MPa;
param thrust: Vec3<Force<Frame.Body>> = ...;
param dev_cost: Money<CostCategory.Development> = 50 M_USD;

// Multi-tag (independent families, comma-separated):
param force: Force<Frame.ECI, Craft.Chaser> = ...;

// Untag via `as` cast — explicit, deliberate, familiar:
node total_pressure: Pressure =
    @p_O2 as Pressure + @p_N2 as Pressure;

// Partial untag — strip one family, keep others:
node force_eci: Force<Frame.ECI> = @force as Force<Frame.ECI>;

// Generic functions:
fn magnitude<F: Frame>(v: Vec3<Length<F>>) -> Length = ...;
```

**Why generics over brackets:**

| Aspect | `[brackets]` (Option C) | `<generics>` (Option E) |
| --- | --- | --- |
| Index disambiguation | Ambiguous: `Velocity[Maneuver]` (index) vs `Length[Frame.ECI]` (tag) | Clean: `[...]` = index, `<...>` = type parameter |
| Nesting | `Vec3<Force>[Frame.Body]` — tag on the Vec3 | `Vec3<Force<Frame.Body>>` — tag on the Force (correct) |
| Multi-tag | `Force[Frame.ECI][Craft.Chaser]` — chained brackets | `Force<Frame.ECI, Craft.Chaser>` — comma-separated params |
| Partial untag | No obvious syntax | `@x as Force<Frame.ECI>` — natural |
| Escape mechanism | `.untag` (method-like) | `as` (cast, familiar from Rust/TS/Python) |
| Generics | `fn f<S: Frame>(x: Length[S])` — mixed brackets | `fn f<F: Frame>(x: Length<F>)` — uniform `<>` |

## Formal Type Model

### Type Representation

A value's type is a pair of a **dimension** and a **tag set**:

```
Type = (Dimension, TagSet)
TagSet = { family₁: variant₁, family₂: variant₂, ... }   // may be empty
```

Examples:

| Graphcal Syntax | Internal Representation |
| --- | --- |
| `Length` | `(Length, {})` |
| `Length<Frame.ECI>` | `(Length, {Frame: ECI})` |
| `Force<Frame.Body, Craft.Chaser>` | `(M·L·T⁻², {Frame: Body, Craft: Chaser})` |
| `Dimensionless` | `(1, {})` |

### Tag Merge Function

The core of tag propagation is the `merge` function, which combines two tag sets family-by-family:

```
merge(T₁, T₂):
    result = {}
    for each family F present in T₁ or T₂:
        if F ∈ T₁ and F ∈ T₂:
            if T₁[F] == T₂[F]:   result[F] = T₁[F]    // same variant → keep
            else:                  COMPILE ERROR          // conflict → reject
        if F ∈ T₁ only:           result[F] = T₁[F]     // sticky: tagged × untagged → keep
        if F ∈ T₂ only:           result[F] = T₂[F]     // sticky: untagged × tagged → keep
    return result
```

### Arithmetic Rules

All arithmetic operations use the **uniform merge** rule for tags:

| Operation | Dimension Rule | Tag Rule | Result |
| --- | --- | --- | --- |
| `a + b` | `D₁ == D₂` (must match) | `merge(T₁, T₂)` | `(D₁, merge(T₁,T₂))` |
| `a - b` | `D₁ == D₂` (must match) | `merge(T₁, T₂)` | `(D₁, merge(T₁,T₂))` |
| `a * b` | `D₁ * D₂` (multiply) | `merge(T₁, T₂)` | `(D₁*D₂, merge(T₁,T₂))` |
| `a / b` | `D₁ / D₂` (divide) | `merge(T₁, T₂)` | `(D₁/D₂, merge(T₁,T₂))` |
| `a ^ n` | `D₁ ^ n` (power) | `T₁` (preserve) | `(D₁^n, T₁)` |
| `sqrt(a)` | `D₁ ^ ½` | `T₁` (preserve) | `(D₁^½, T₁)` |
| `sin(a)` | `D₁ == Angle` | `T₁` must be `{}` | `(1, {})` |
| `exp(a)` | `D₁ == 1` | `T₁` must be `{}` | `(1, {})` |
| `a -> unit` | unchanged | unchanged | `(D₁, T₁)` |
| `a as D` | `D₁ ~> D` (cast) | stripped/narrowed | `(D, T_target)` |

**Why uniform merge (not strict-for-addition):**

An earlier design used strict tag matching for `+`/`-` (requiring `T₁ == T₂` exactly) while using `merge` for `*`/`/`. The rationale: adding `Pressure<Species.O2> + Pressure` (tagged + untagged) seemed error-prone. However, this distinction is empirical, not principled, and **the time zone use case shows it breaks down**:

```
param launch_utc: Time<TimeZone.UTC> = 14.5 hr;
param coast: Time = 45 min;  // untagged duration

// Under strict-for-addition: ERROR — {TimeZone: UTC} ≠ {}
// Under uniform merge: merge({TimeZone: UTC}, {}) = {TimeZone: UTC} ✓
node arrival: Time<TimeZone.UTC> = @launch_utc + @coast;
```

Adding a duration to a timestamped value is natural and correct. The duration is semantically "timezone-independent" — it doesn't *conflict* with UTC, it simply doesn't carry a timezone. Forcing `@coast as Time<TimeZone.UTC>` is nonsensical (a 45-minute interval doesn't belong to a timezone). The same pattern applies elsewhere:

- `Force<Frame.Body> + gravitational_force` — an untagged gravity vector should be addable to a body-frame force without explicit tagging.
- `Mass<Phase.Ascent> + tank_dry_mass` — untagged structural mass should be addable to phase-specific fuel mass.

The merge function already does the right thing here: `merge({tag}, {}) = {tag}` (sticky). Only `merge({tag_A}, {tag_B})` when `A ≠ B` errors. The remaining concern — that `Pressure<Species.O2> + Pressure` might mask an error — is addressed by the observation that **if the untagged value was meant to carry a tag, it should have been tagged at its declaration site**. An untagged value is explicitly "not tagged," which is a valid semantic state, not an error.

### Worked Traces

```
// Velocity in ECI * Time = Length in ECI
Velocity<Frame.ECI> * Time
= (L·T⁻¹, {Frame: ECI}) * (T, {})
  Dimension: L·T⁻¹ * T = L ✓
  Tags: merge({Frame: ECI}, {}) = {Frame: ECI}
= Length<Frame.ECI> ✓

// Force in Body * Length in Body = Energy in Body
Force<Frame.Body> * Length<Frame.Body>
= (M·L·T⁻², {Frame: Body}) * (L, {Frame: Body})
  Dimension: M·L²·T⁻² ✓
  Tags: merge({Frame: Body}, {Frame: Body}) = {Frame: Body}
= Energy<Frame.Body> ✓

// Count of persons * mass-per-person = mass of persons
Count<Countable.Person> * (Mass / Count)<Countable.Person>
= (Count, {Countable: Person}) * (M·Count⁻¹, {Countable: Person})
  Dimension: M ✓
  Tags: merge({Countable: Person}, {Countable: Person}) = {Countable: Person}
= Mass<Countable.Person> ✓

// Mixing frames → error
Force<Frame.Body> * Length<Frame.ECI>
  Tags: merge({Frame: Body}, {Frame: ECI})
  Frame: Body ≠ ECI → COMPILE ERROR ✓

// Multi-tag * untagged → tags preserved
Force<Frame.ECI, Craft.Chaser> * Time
= (M·L·T⁻², {Frame: ECI, Craft: Chaser}) * (T, {})
  Dimension: M·L·T⁻¹ ✓
  Tags: merge({Frame: ECI, Craft: Chaser}, {}) = {Frame: ECI, Craft: Chaser}
= Impulse<Frame.ECI, Craft.Chaser> ✓

// Addition with mismatched tags → error
Pressure<Species.O2> + Pressure<Species.N2>
  Tags: merge({Species: O2}, {Species: N2})
  Species: O2 ≠ N2 → COMPILE ERROR ✓
  Fix: (@p_O2 as Pressure) + (@p_N2 as Pressure)

// ---- Time Zone Examples (uniform merge for addition) ----

// Tagged time + untagged duration → tagged time
Time<TimeZone.UTC> + Time
= (T, {TimeZone: UTC}) + (T, {})
  Dimension: T == T ✓
  Tags: merge({TimeZone: UTC}, {}) = {TimeZone: UTC}
= Time<TimeZone.UTC> ✓
// "14:30 UTC + 45 min = 15:15 UTC"

// Mixing time zones → error
Time<TimeZone.UTC> + Time<TimeZone.JST>
  Tags: merge({TimeZone: UTC}, {TimeZone: JST})
  TimeZone: UTC ≠ JST → COMPILE ERROR ✓
  Fix: (@t_utc as Time) + (@t_jst as Time)

// Subtracting same-zone times → same-zone duration
Time<TimeZone.UTC> - Time<TimeZone.UTC>
= (T, {TimeZone: UTC}) - (T, {TimeZone: UTC})
  Tags: merge({TimeZone: UTC}, {TimeZone: UTC}) = {TimeZone: UTC}
= Time<TimeZone.UTC> ✓
// Result is "a duration within the UTC frame." If you want an
// untagged duration: (@t2 - @t1) as Time
```

### The `as` Cast

`as` performs an explicit type cast that can strip or narrow tags:

```
// Full untag (strip all tags):
@p_O2 as Pressure
= (Pressure, {Species: O2}) → (Pressure, {})

// Partial untag (strip one family, keep others):
@force as Force<Frame.ECI>
= (M·L·T⁻², {Frame: ECI, Craft: Chaser}) → (M·L·T⁻², {Frame: ECI})

// The dimension must be compatible — you cannot cast Length to Mass:
@pos as Mass   // COMPILE ERROR: dimension mismatch

// Tagging (adding a tag) at a declaration site:
param p_O2: Pressure<Species.O2> = 21.3 kPa;
// The literal 21.3 kPa is (Pressure, {}). The type annotation adds the tag.
// This is the primary way to create tagged values.

// Tagging via as (adding a tag to an untagged value):
node p_tagged: Pressure<Species.O2> = @raw_pressure as Pressure<Species.O2>;
// This is the "downcast" direction — programmer asserts the tag is correct.
```

### Generic Functions

Tags integrate with the existing `<D: Dim>` generics:

```
// Generic over a tag variant:
fn magnitude<F: Frame>(v: Vec3<Length<F>>) -> Length =
    sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
// Magnitude is frame-independent, so the result is untagged Length.

// Generic over dimension — tags flow through automatically:
fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D =
    a + (b - a) * t;
// If called with Length<Frame.ECI>, D binds to the full tagged type.
// Result is Length<Frame.ECI>.

// Generic over both dimension and tag:
fn scale<D: Dim, F: Frame>(v: D<F>, s: Dimensionless) -> D<F> =
    v * s;
// Requires higher-kinded types (D is a type constructor parameterized by F).
// May be deferred — the simpler `fn scale<D: Dim>(v: D, s: Dimensionless) -> D`
// already handles this if D binds to the full tagged type.
```

## Crossing Tag Boundaries

Two mechanisms for intentional cross-tag operations:

### 1. `as` Cast (General Mechanism)

```
node total_pressure: Pressure =
    @p_O2 as Pressure + @p_N2 as Pressure;

node total_cost: Money =
    @dev_cost as Money + @prod_cost as Money;
```

The `as` keyword is familiar from Rust, TypeScript, Python, and Kotlin. It signals a deliberate, explicit type operation — the programmer is asserting that stripping the tag is intentional.

### 2. `Transform` (Coordinate Frames Only)

```
node eci_to_body: Transform<Frame.ECI, Frame.Body> = {
    Transform.from_rotation(@attitude_quaternion)
};
node thrust_eci: Vec3<Force<Frame.ECI>> = @eci_to_body.inverse() * @thrust_body;
```

`Transform` is domain-specific to coordinate frames. Most tag families have no meaningful transform — for those, `as` is the only crossing mechanism.

## Interaction with Other Layers

- Tags are **optional**. Untagged values are the default.
- Tags are **type parameters** on dimensions. `Length<Frame.ECI>` is a more specific type than `Length`.
- The tag layer and dimension layer are independent: dimension algebra produces the result dimension, `merge` produces the result tag set.
- `as` casts go from more specific (tagged) to less specific (untagged). At declaration sites, untagged values can be assigned to tagged types (the annotation provides the tag).

## Open Questions

### Critical (Must Resolve Before Implementation)

- **`as` both directions:** Is `@raw as Pressure<Species.O2>` (adding a tag to an untagged value) allowed freely, or should it require a special marker since it's the "unsafe" direction? Adding a wrong tag is where real errors happen.

### Important (Should Resolve Before Maturity)

- **Higher-kinded generics:** Can `D<F>` appear in function signatures where `D: Dim` and `F: Frame`? Or does `D` always bind to the full tagged type? The latter is simpler and covers most use cases.
- **Open vs closed tags:** Can tag families be extended across files? Coordinate frames are typically a closed set, but `Countable` variants may need to be extensible across libraries.
- **Tag-generic functions:** `fn magnitude<F: Frame>(v: Vec3<Length<F>>) -> Length` — the return type is untagged. How does the checker know this is intentional and not a mistake? Perhaps `-> Length` is allowed when `F` doesn't appear in the return type (the function "consumes" the tag).
- **Transform generality:** Is `Transform<A, B>` a coordinate-frame-specific concept, or a general mechanism for all tag families?

### Deferred

- **Transform composition:** `Transform<A, B> * Transform<B, C> -> Transform<A, C>` tracking in the type system.
- **`as` auditing:** Should uses of `as` be flagged in a lint pass or report? Helps code review.
- **Tag hierarchy:** Can tags have sub-variants? E.g., `Frame.Inertial` as parent of `Frame.ECI`.
- **Runtime tag selection:** Can the tag variant be determined at runtime, or is it always compile-time?

## Dependencies on Other Aspects

- **Dimensions** ([04](./04-dimensions-and-units.md)): Tags are type parameters on dimensions; the two layers compose independently.
- **Non-SI Dimensions** ([18](./18-non-si-dimensions.md)): Counting quantities use `Count<Countable.X>` tags rather than separate base dimensions.
- **Algebraic Data Types** ([05](./05-algebraic-data-types.md)): `Transform` may be a built-in type or user-defined.
- **Pure Functions** ([12](./12-pure-functions.md)): Functions can be generic over tag variants.
- **Syntax** ([02](./02-syntax-design.md)): `tag` declaration keyword and `as` cast keyword to be added to keyword inventory.
- **Scoping** ([08](./08-scoping.md)): Tags appear in type annotations at the `@` reference site.
