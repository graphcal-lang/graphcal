# Non-SI Base Dimensions

> Extending the dimension system beyond the 7 SI base quantities.

## Status

**Decision level:** Proposal. Needs design review.

## Problem Statement

Graphcal's dimension system currently has 8 hard-coded base dimensions (7 SI + Angle) represented as a fixed-size `[Rational; 8]` exponent vector. Some real-world engineering calculations require dimensions that are **irreducible** — they cannot be expressed as products of powers of SI base dimensions.

The design doc ([04](./04-dimensions-and-units.md)) already lists "User-defined base dimensions" as an open question. The language syntax already supports bodyless `dimension` declarations (`dimension Foo;`), but the implementation currently skips them (`ir.rs:184-186`).

## Catalog of Non-SI Dimension Use Cases

### True Non-SI Base Dimensions

These represent fundamentally new quantities that need their own dimension axis because they participate in **dimensional algebra** — they appear as numerators or denominators in derived dimensions:

| Dimension | Example Units | Derived Dimensions | Use Cases |
| --- | --- | --- | --- |
| **Information** | bit, byte, kB, MB, GB, TB, KiB, MiB, GiB | `Bandwidth = Information / Time`, `DataDensity = Information / Area`, `DataCost = Money / Information` | Data storage, bandwidth, compression ratios, memory sizing |
| **Money** | USD, EUR, JPY, GBP | `UnitCost = Money / Mass`, `PowerCost = Money / Energy`, `LaunchCost = Money / Mass` | Cost estimation, economic analysis, $/kg launch cost, levelized cost of energy |

These are **not** just counting things — they form rich families of derived dimensions through algebra with SI quantities.

### Counting Dimensions → Better Modeled as `Count` + Spaces

Many other commonly cited examples are "counts of discrete things" that need to be kept distinct. At first glance, these look like candidates for new base dimensions:

| Quantity | Example Units | Seems Like |
| --- | --- | --- |
| People | crew_member, person, FTE | `dimension People;` |
| Pixel | px, Mpx | `dimension Pixel;` |
| Cycle | cycle, revolution | `dimension Cycle;` |
| Packet | packet, frame | `dimension Packet;` |
| Vehicle | vehicle, spacecraft | `dimension Vehicle;` |
| Sample/Event | sample, event | `dimension Sample;` |
| Request | request, query | `dimension Request;` |
| Cell | cell | `dimension Cell;` |

However, these are all **the same kind of thing** — a dimensionless count of discrete items. The safety requirement (`5 crew_member + 3 packet` must be a compile error) doesn't require separate *dimensions*; it requires separate *semantic tags*.

This is exactly what Graphcal's **Tags** feature ([06](./06-spaces.md)) provides. Tags are orthogonal semantic labels (type parameters on dimensions) that prevent cross-context mixing:

```
// A single Count dimension + tags for type safety:
dimension Count;
unit count: Count;

tag Countable { Person, Pixel, Cycle, Packet, Vehicle }

param crew: Count<Countable.Person> = 7 count;
param sensors: Count<Countable.Pixel> = 4096 count;

node bad = @crew + @sensors;
//  error[T001]: tag conflict: Countable.Person ≠ Countable.Pixel
```

**Why Tags are better than separate dimensions for counting:**

1. **Conceptual clarity**: All these quantities really *are* counts. Making each one a separate base dimension pollutes the dimension algebra with artificial axes.
2. **No spurious derived dimensions**: With separate dimensions, `Pixel / Person` would be a "meaningful" dimension — but it isn't. It's just a dimensionless ratio. With tags, `Count<Countable.Pixel> / Count<Countable.Person>` requires an explicit `as` cast, signaling the intentional cross-context operation.
3. **Consistent with existing design**: Tags already exist for exactly this purpose (coordinate frames, spacecraft identity, budget categories, time zones — all same-dimension-different-context).
4. **Scalability**: Adding 20 counting dimensions would create a 28-element exponent vector (wasteful). Tags add no overhead to the dimension system.

**When a true base dimension IS needed**: When the quantity participates in rich dimensional algebra. `Information / Time = Bandwidth` is meaningful. `Money / Mass = SpecificCost` is meaningful. These aren't just "counts of bits" or "counts of dollars" — they form families of derived dimensions with distinct physical interpretations.

**Tag propagation**: The Count + Tags approach requires the tag `merge` function to propagate tags through arithmetic. For example, `Count<Countable.Person> * Mass / Count<Countable.Person>` yields `Mass<Countable.Person>`, with the tag carrying through multiplication. This is now formalized in [06-spaces.md](./06-spaces.md) — all arithmetic operations use uniform `merge`, which combines tag sets family-by-family (same variant → keep, conflict → error, one missing → sticky).

### Anti-Examples: Things That DON'T Need New Base Dimensions

These are sometimes confused as needing new dimensions but can be expressed with SI:

| Quantity | Why It's Already Covered |
| --- | --- |
| RPM | Frequency = Angle / Time (or 1/Time) |
| Decibel (dB) | Dimensionless (logarithmic scale — separate concern) |
| Sievert, Gray | Mass^-1 * Length^2 * Time^-2 (SI derived) |
| pH | Dimensionless (logarithmic) |
| Mach number | Dimensionless (ratio) |

## Current Architecture: Why This Is Hard

### The Fixed Vector Problem

`Dimension` in `dimension.rs` is:

```rust
pub struct Dimension {
    pub exponents: [Rational; 8],
}
```

The 8 slots are hardcoded via `BaseDim` enum:

```rust
pub enum BaseDim {
    Length = 0,
    Time = 1,
    Mass = 2,
    Temperature = 3,
    ElectricCurrent = 4,
    Amount = 5,
    LuminousIntensity = 6,
    Angle = 7,
}
```

This means:
- There is no slot for `Information`, `Money`, or any user-defined base dimension.
- Bodyless `dimension Information;` declarations are parsed but silently skipped during IR lowering.
- All dimension algebra (`Mul`, `Div`, `pow`) operates on exactly 8 elements.

### What Numbat Does

Numbat (Graphcal's primary inspiration) uses a **fully dynamic** representation:
- `Registry<()>` stores base entries in a `Vec<(CompactString, ())>`.
- Derived dimensions resolve to a `BaseRepresentation` = `Product<BaseRepresentationFactor>`, a sparse representation where each factor is `(BaseEntry, Exponent)`.
- `dimension DigitalInformation` and `dimension Money` are declared in Numbat's standard library (`bit.nbt`, `currency.nbt`) as bodyless base dimensions, alongside the 8 SI/Angle base dimensions declared in `dimensions.nbt`.
- There is no hard-coded limit on the number of base dimensions.

## Proposed Design

### Approach: Sparse Dynamic Dimension Vector

Replace the fixed `[Rational; 8]` with a sparse map from dimension IDs to exponents.

```rust
/// A unique identifier for a base dimension, assigned at registration time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BaseDimId(u32);

/// A physical dimension represented as a sparse vector of rational exponents
/// over base dimensions.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Dimension {
    /// Non-zero exponents only. Sorted by BaseDimId for deterministic Eq/Hash.
    exponents: BTreeMap<BaseDimId, Rational>,
}
```

### Why BTreeMap Over Other Options

| Option | Pros | Cons |
| --- | --- | --- |
| `[Rational; 8]` (current) | Fast, simple, `Copy` | Cannot extend, wastes slots |
| `HashMap<BaseDimId, Rational>` | O(1) lookup | Non-deterministic iteration, no `Eq`/`Hash` |
| `BTreeMap<BaseDimId, Rational>` | Deterministic, sorted, `Eq`/`Hash` derivable | Slightly slower than array for small N |
| `SmallVec<[(BaseDimId, Rational); 8]>` | Stack-allocated for common case | Manual sort maintenance, complex `Eq`/`Hash` |

`BTreeMap` is the right choice: deterministic ordering for `Eq`/`Hash`/`Display`, good enough performance (N is small — typically 1-3 non-zero entries), and natural support for sparsity.

### Dimension Algebra on BTreeMap

```rust
impl Mul for Dimension {
    fn mul(self, other: Self) -> Self {
        let mut result = self.exponents.clone();
        for (id, exp) in &other.exponents {
            let entry = result.entry(*id).or_insert(Rational::ZERO);
            *entry = *entry + *exp;
            if entry.is_zero() { result.remove(id); }
        }
        Dimension { exponents: result }
    }
}
```

The `is_zero()` cleanup ensures that `Length / Length = Dimensionless` (empty map) — important for correct equality.

### BaseDimId Assignment

Base dimension IDs are assigned by the registry at registration time:

```rust
impl Registry {
    fn next_base_dim_id: u32,

    pub fn register_base_dimension(&mut self, name: DimName) -> BaseDimId {
        let id = BaseDimId(self.next_base_dim_id);
        self.next_base_dim_id += 1;
        let dim = Dimension::base(id);
        self.dimensions.insert(name, dim);
        self.base_dim_names.insert(id, name);
        id
    }
}
```

The prelude registers the 8 standard base dimensions first (so they get IDs 0-7), then user code can register additional ones.

### Prelude Changes

The prelude can now be more naturally expressed. The `BaseDim` enum becomes a convenience for prelude registration rather than a fundamental type:

```rust
fn load_base_dimensions(r: &mut Registry) {
    // These get IDs 0-7 by registration order
    r.register_base_dimension(DimName::new("Length"));
    r.register_base_dimension(DimName::new("Time"));
    r.register_base_dimension(DimName::new("Mass"));
    // ... etc
}
```

### IR Lowering Fix

The current `ir.rs` skips bodyless dimension declarations. The fix:

```rust
DeclKind::Dimension(d) => {
    if let Some(def) = &d.definition {
        // Derived dimension — resolve the expression
        let dim = registry.resolve_dim_expr(def).ok_or_else(|| ...)?;
        registry.register_dimension(d.name.value.clone(), dim);
    } else {
        // Base dimension — register a new orthogonal axis
        registry.register_base_dimension(d.name.value.clone());
    }
}
```

### Display and SI Unit Strings

The `Display` impl needs the registry to map `BaseDimId` back to names. Two options:

**Option A**: `Dimension::display_with(registry)` method returning a wrapper.
**Option B**: Store base dimension names in a thread-local or `Arc` inside `Dimension`.

Option A is cleaner and aligns with Graphcal's explicit-over-implicit philosophy:

```rust
impl Dimension {
    pub fn display_with<'a>(&'a self, registry: &'a Registry) -> DimensionDisplay<'a> {
        DimensionDisplay { dim: self, registry }
    }
}
```

The `si_unit_string()` method similarly needs registry access to map custom base dimensions to their base unit symbols.

## User-Facing Syntax

No syntax changes needed. The existing bodyless `dimension` declaration is the mechanism:

```
// In user's .gcl file or a library:

// -- True non-SI base dimensions --
dimension Information;
unit bit: Information;
unit byte: Information = 8 bit;
unit kB: Information = 1000 byte;
unit KiB: Information = 1024 byte;
unit MB: Information = 1000 kB;
unit MiB: Information = 1024 KiB;
unit GB: Information = 1000 MB;
unit GiB: Information = 1024 MiB;

dimension Money;
unit USD: Money;
unit EUR: Money = 0.92 USD;     // snapshot rate — see currency section
unit JPY: Money = 0.0067 USD;

// Derived dimensions compose naturally:
dimension Bandwidth = Information / Time;
dimension DataCost = Money / Information;
dimension SpecificCost = Money / Mass;

// Type-safe calculations:
param storage: Information = 500 GB;
param price: DataCost = 0.023 USD / GB;
node monthly_cost: Money = @storage * @price;

// -- Counting quantities (use Count + Tags, not new dimensions) --
dimension Count;
unit count: Count;

tag Countable { Person, Satellite, Cycle }

param crew: Count<Countable.Person> = 7 count;
param sats: Count<Countable.Satellite> = 24 count;
```

### Currency: Single Dimension with Unit-Based Conversion

Currencies should be a **single `Money` base dimension** with units providing conversion factors:

```
dimension Money;
unit USD: Money;                   // base unit
unit EUR: Money = 0.92 USD;       // 1 EUR = 0.92 USD
unit JPY: Money = 0.0067 USD;     // 1 JPY = 0.0067 USD
```

This means `1 EUR + 1 USD` is **well-typed** (both are `Money`) and the system handles conversion via the scale factors, just like `1 km + 1 m`.

**Why a single dimension is correct for currency:**

1. **Currencies are commensurable**: Unlike meters and kilograms, you *can* convert USD to EUR. They measure the same thing (economic value) in different scales.
2. **Consistent with the unit model**: Units are scaling factors within a dimension. `EUR` is to `USD` as `km` is to `m` — a different scale for the same quantity.
3. **Practical**: Most engineering cost models need to add costs in different currencies (e.g., US-manufactured parts + European subcontractors). Blocking `USD + EUR` creates friction without adding safety.

**The variable exchange rate concern**: Exchange rates change over time, unlike physical conversion factors. This is addressed by making the exchange rate a **parameter**:

```
param eur_to_usd: Dimensionless = 0.92;
unit EUR: Money = @eur_to_usd USD;    // if dynamic unit defs are supported
```

If dynamic unit definitions are not supported (units must have compile-time-constant scales), then the user picks a snapshot rate for their analysis and documents it. This is standard practice in engineering economics — you always state your assumed exchange rate. Different rates can be explored via scenarios.

## Impact Analysis

### What Changes

| Component | Change |
| --- | --- |
| `dimension.rs` | `Dimension` struct: `[Rational; 8]` → `BTreeMap<BaseDimId, Rational>` |
| `dimension.rs` | `BaseDim` enum: removed or kept as prelude-only convenience |
| `dimension.rs` | `Mul`, `Div`, `pow`: operate on `BTreeMap` |
| `dimension.rs` | `Display`: needs registry for name lookup |
| `registry.rs` | Add `register_base_dimension()`, `BaseDimId` tracking |
| `prelude.rs` | Register base dims via `register_base_dimension()` |
| `ir.rs` | Handle bodyless `dimension` declarations (currently skipped) |
| `dim_check.rs` | Mostly unchanged (operates on `Dimension` values) |
| `eval_expr.rs` | Unchanged (operates on `f64` + `UnitInfo`) |

### What Doesn't Change

- User syntax for dimension/unit declarations
- Unit conversion semantics
- How values are stored internally (still base-unit-scaled `f64`)
- Dimension checking logic (still compare `Dimension` values for equality)
- Generic dimension parameters (`<D: Dim>`)

### Performance

For the vast majority of calculations, dimensions have 1-3 non-zero exponents. `BTreeMap` with 1-3 entries is fast — dominated by allocation cost, not lookup. If profiling shows this matters, a `SmallVec`-based sorted representation could be used as an optimization without changing the API.

### Loss of `Copy`

`Dimension` currently derives `Copy` because `[Rational; 8]` is `Copy`. `BTreeMap` is not `Copy`. This means `Dimension` becomes `Clone`-only. This is a mechanical change (add `.clone()` at call sites) but affects many files. Worth it for the extensibility.

## Alternative: Two-Tier Hybrid

A less invasive alternative preserves the fixed array for SI and adds a sparse map for extensions:

```rust
pub struct Dimension {
    si_exponents: [Rational; 8],            // Fast path for SI
    custom_exponents: BTreeMap<BaseDimId, Rational>,  // Extension
}
```

**Pros**: Keeps `Copy` for SI-only dimensions (the common case), no regression in performance for existing code.

**Cons**: Two-tier logic everywhere (every operation checks both), conceptual complexity, SI dimensions are "special" when the design philosophy says they shouldn't be.

**Recommendation**: The fully dynamic approach is cleaner. The project is pre-1.0 and breaking changes are acceptable per CLAUDE.md. The performance difference is negligible for the dimension sizes involved.

## Relationship to Other Open Questions

### Dimension Aliases (Torque vs Energy)

This proposal doesn't solve the Torque/Energy problem (same algebraic dimension, different semantics). That's orthogonal — it could be solved with semantic tags on top of either the fixed or dynamic representation. See [04](./04-dimensions-and-units.md) open questions.

### Counting Quantities and Tags

As discussed above, most "counting dimensions" are better modeled as a single `Count` dimension with tags from [06-spaces.md](./06-spaces.md). This keeps the dimension vector lean while providing the same type-safety guarantees through the orthogonal tag layer.

The test: *does this quantity form meaningful derived dimensions through algebra?*
- **Yes** (Information, Money) → new base dimension.
- **No** (crew members, packets, pixels) → `Count` + tag.

### Custom Counting Units (`unit launch;`)

The design doc mentions `unit launch;` auto-creating a dimension. With this proposal, the explicit form is preferred:

```
dimension Count;
unit count: Count;
tag Countable { Launch }

param launches: Count<Countable.Launch> = 5 count;
```

### SI Prefix Mechanism

Orthogonal to this proposal. SI prefixes (`kilo`, `mega`, etc.) are about unit derivation, not dimension definition. Could be addressed separately with a `@metric_prefixes` annotation similar to Numbat.

## Dependencies on Other Aspects

- **Dimensions & Units** ([04](./04-dimensions-and-units.md)): This proposal directly extends it.
- **Tags / Spaces** ([06](./06-spaces.md)): Counting quantities use tags rather than new base dimensions.
- **Syntax** ([02](./02-syntax-design.md)): No syntax changes needed.
- **Namespace & Multi-File** ([09](./09-namespace.md)): Base dimensions defined in libraries need to be importable and get consistent IDs across compilation units.
- **Phases**: This is primarily a Phase 1 (Dimensions & Units) concern, but could be deferred to Phase 4 (Multi-File) since that's when libraries defining new base dimensions become practical.

## Open Questions

1. **ID stability across compilations**: If `BaseDimId` is assigned by registration order, do IDs need to be stable across separate compilations of the same project? Probably not — IDs are internal and never serialized. But this needs verification if incremental compilation is added.

2. **Prelude base dimension ordering**: Should the prelude always register SI dimensions as IDs 0-7? This is convenient for debugging but not semantically necessary.

3. **Maximum base dimensions**: Should there be a limit? Numbat has no limit. A practical limit of 64 or 128 would catch runaway declarations while being generous.

4. **Display order**: When showing a dimension like `Information * Length / Time^2`, what order should base dimensions appear in? Registration order? Alphabetical? SI-first-then-custom?

5. **Count + Tags interaction**: When Tags ([06](./06-spaces.md)) are implemented, should `Count` be a prelude-provided base dimension or something users always declare themselves? A prelude-provided `Count` dimension with a user-extensible `Countable` tag family seems most ergonomic.

6. **Dynamic unit definitions**: Can unit scale factors reference parameters (`unit EUR: Money = @exchange_rate USD;`)? This would elegantly handle variable exchange rates but blurs the compile-time/runtime boundary.

7. **Borderline cases**: Some quantities sit between "true dimension" and "tagged count." For example, `Pixel / Length` = spatial resolution is a useful derived dimension, suggesting Pixel might deserve its own base dimension rather than being `Count<Countable.Pixel>`. The guideline ("does it form meaningful derived dimensions?") needs case-by-case judgment.
