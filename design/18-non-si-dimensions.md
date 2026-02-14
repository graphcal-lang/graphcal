# Non-SI Base Dimensions

> Extending the dimension system beyond the 7 SI base quantities.

## Status

**Decision level:** Proposal. Needs design review.

## Problem Statement

Graphcal's dimension system currently has 8 hard-coded base dimensions (7 SI + Angle) represented as a fixed-size `[Rational; 8]` exponent vector. Many real-world engineering calculations require dimensions that are **irreducible** — they cannot be expressed as products of powers of SI base dimensions.

The design doc ([04](./04-dimensions-and-units.md)) already lists "User-defined base dimensions" as an open question. The language syntax already supports bodyless `dimension` declarations (`dimension Foo;`), but the implementation currently skips them (`ir.rs:184-186`).

## Catalog of Non-SI Dimension Use Cases

### Tier 1: Very Common, Broadly Needed

| Dimension | Example Units | Use Cases |
| --- | --- | --- |
| **Information** | bit, byte, kB, MB, GB, TB, KiB, MiB, GiB | Data storage, bandwidth (`Information / Time`), compression ratios, memory sizing |
| **Currency / Money** | USD, EUR, JPY, GBP | Cost estimation, economic analysis, $/kg launch cost, levelized cost of energy |

### Tier 2: Common in Specific Engineering Domains

| Dimension | Example Units | Use Cases |
| --- | --- | --- |
| **Pixel** | px, Mpx | Image processing, display resolution, sensor sizing, px/mm for optical systems |
| **Count (discrete items)** | items, units, pieces | Manufacturing throughput (items/hour), inventory, batch sizing |
| **People** | crew_member, person, FTE | Staffing models, life support (kg/person/day), person-hours |
| **Cycle** | cycle, revolution | Fatigue analysis (cycles to failure), RPM reinterpretation, vibration |
| **Sample / Event** | sample, event | Signal processing (samples/s), statistical analysis, sensor fusion |
| **Packet** | packet, frame | Network engineering (packets/s), protocol analysis |

### Tier 3: Niche but Legitimate

| Dimension | Example Units | Use Cases |
| --- | --- | --- |
| **Vehicle** | vehicle, spacecraft | Traffic flow (vehicles/hour), fleet sizing |
| **Request** | request, query, transaction | API capacity planning, database sizing |
| **Cell** | cell | Battery pack design (cells in series/parallel) |
| **Gene / Base pair** | bp, kbp, Mbp | Bioinformatics, genome sizing |
| **Dose** | dose | Pharmacokinetics, radiation treatment planning |
| **Story point** | SP | Software project estimation (SP/sprint) |

### Cross-Cutting Pattern: "Counting Dimensions"

Most Tier 2/3 examples are **counting dimensions** — they represent discrete, countable quantities that need to be kept distinct from each other and from dimensionless numbers. The key safety property: `5 crew_member + 3 packet` should be a compile-time error, even though both are "just numbers."

This is exactly the use case for bodyless `dimension` declarations:

```
dimension Information;
dimension Money;
dimension Pixel;

// Each is orthogonal to all others and to all SI dimensions
```

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
dimension Information;
unit bit: Information;
unit byte: Information = 8 bit;
unit kB: Information = 1000 byte;
unit KiB: Information = 1024 byte;
unit MB: Information = 1000 kB;
unit MiB: Information = 1024 KiB;

dimension Money;
unit USD: Money;
unit EUR: Money;    // Note: no conversion between currencies by default
                    // (that would require runtime exchange rates)

// Derived dimensions compose naturally:
dimension Bandwidth = Information / Time;
dimension DataCost = Money / Information;

// Type-safe calculations:
param storage: Information = 500 GB;
param price: DataCost = 0.023 USD / GB;
node monthly_cost: Money = @storage * @price;
```

### Currency: A Special Consideration

Currency is interesting because exchange rates are **not constant** — they vary over time. Two approaches:

1. **Incommensurable currencies** (recommended default): Each currency is its own base dimension. `1 USD + 1 EUR` is a type error. Conversion requires an explicit exchange rate parameter.

2. **Single Money dimension**: All currencies share one dimension, with scale factors. Simpler but the scale factors are lies (they're not physical constants).

Approach 1 is more aligned with Graphcal's safety philosophy. You would model it as:

```
dimension USD;
dimension EUR;
unit usd: USD;
unit eur: EUR;

param exchange_rate: EUR / USD = 0.92 eur / usd;
node cost_eur: EUR = @cost_usd * @exchange_rate;
```

This makes the exchange rate an explicit parameter that can be varied in scenarios — exactly the right modeling pattern.

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

### Custom Counting Units (`unit launch;`)

The design doc mentions `unit launch;` auto-creating a dimension. With this proposal, the explicit form is preferred:

```
dimension Launch;
unit launch: Launch;
```

Two declarations instead of one, but more explicit. If shorthand syntax is desired, it could be sugar that expands to these two declarations.

### SI Prefix Mechanism

Orthogonal to this proposal. SI prefixes (`kilo`, `mega`, etc.) are about unit derivation, not dimension definition. Could be addressed separately with a `@metric_prefixes` annotation similar to Numbat.

## Dependencies on Other Aspects

- **Dimensions & Units** ([04](./04-dimensions-and-units.md)): This proposal directly extends it.
- **Syntax** ([02](./02-syntax-design.md)): No syntax changes needed.
- **Namespace & Multi-File** ([09](./09-namespace.md)): Base dimensions defined in libraries need to be importable and get consistent IDs across compilation units.
- **Phases**: This is primarily a Phase 1 (Dimensions & Units) concern, but could be deferred to Phase 4 (Multi-File) since that's when libraries defining new base dimensions become practical.

## Open Questions

1. **ID stability across compilations**: If `BaseDimId` is assigned by registration order, do IDs need to be stable across separate compilations of the same project? Probably not — IDs are internal and never serialized. But this needs verification if incremental compilation is added.

2. **Prelude base dimension ordering**: Should the prelude always register SI dimensions as IDs 0-7? This is convenient for debugging but not semantically necessary.

3. **Maximum base dimensions**: Should there be a limit? Numbat has no limit. A practical limit of 64 or 128 would catch runaway declarations while being generous.

4. **Display order**: When showing a dimension like `Information * Length / Time^2`, what order should base dimensions appear in? Registration order? Alphabetical? SI-first-then-custom?
