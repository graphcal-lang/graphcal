# Cellgraph

**A type-safe, unit-aware, Git-friendly reactive programming language for engineering calculations.**

Cellgraph replaces the spreadsheets and simulation tools that engineers reluctantly depend on — Excel mass budgets, Vensim logistics models, ad-hoc Python scripts — with a single typed, version-controlled, reactive computation graph.

```rust
param parking_alt = 200 km;
param target_alt  = 35786 km;       /// GEO altitude
param isp         = 320 s;
param dry_mass    = 1200 kg;

node transfer: TransferResult = {
    let r1 = @R_earth + @parking_alt;
    let r2 = @R_earth + @target_alt;

    let v1 = sqrt(@GM_earth / r1);
    let v2 = sqrt(@GM_earth / r2);
    let dv1 = sqrt(2 * @GM_earth * r2 / (r1 * (r1 + r2))) - v1;
    let dv2 = v2 - sqrt(2 * @GM_earth * r1 / (r2 * (r1 + r2)));

    TransferResult { dv1, dv2, total_dv: dv1 + dv2 }
};

node fuel_mass = @dry_mass * (exp(@transfer.total_dv / (@isp * @G0)) - 1);
```

Change `parking_alt` → everything recomputes. Review the diff in a pull request. Run a 10,000-point parameter sweep in under a second.

## Why

Engineering teams do critical calculations in Excel. Those spreadsheets are untyped, unversioned, opaque, and fragile. A wrong `SUM` range silently corrupts a mass budget. A coordinate frame mix-up goes unnoticed. Nobody can code-review a `.xlsx`.

Cellgraph fixes this:

- **Typed** — the compiler catches `km + kg` and `body_frame + inertial_frame` before you run anything
- **Text** — `.graph` files are plain text: `git diff`, `git blame`, pull request reviews
- **Reactive** — change a parameter, everything downstream recomputes (like a spreadsheet)
- **Fast** — Rust engine; parameter sweeps and Monte Carlo at native speed
- **Interoperable** — import/export Excel; call Python (scipy, numpy, astropy) for complex nodes

## Quick Tour

### Parameters, Nodes, and the `@` Sigil

```rust
param mass   = 5000 kg;        // input — user adjusts this
const G0     = 9.80665 m/s^2;  // constant — never changes
node  thrust = @mass * @G0;    // computed — depends on graph values
```

`@` marks graph-level references. Bare names are local. In any diff, you instantly see what a node depends on — no LSP needed.

### Physical Dimensions and Units

Dimensions are types. Units are values. Inspired by [Numbat](https://numbat.dev).

```rust
dimension Velocity = Length / Time;
unit km: Length = 1000 m;

param alt = 400 km;
param t   = 90 min;

node speed = 2 * pi * (@R_earth + @alt) / @t;   // inferred: Velocity
node bad   = @alt + @mass;                        // compile error: Length + Mass
```

The compiler tracks dimension algebra through every operation: `sqrt`, `exp`, multiplication, division.  Conversion is explicit: `@fuel_mass -> lb`.

### Spaces — Coordinate Frame Safety

Inspired by [Sguaba](https://github.com/helsing-ai/sguaba). Prevents mixing vectors from different reference frames, spacecraft, or budget categories — at compile time.

```rust
space Frame { Body, ECI, ECEF }

param pos_eci:  Vec3<Length> in Frame.ECI  = ...;
param pos_body: Vec3<Length> in Frame.Body = ...;

node bad = @pos_eci + @pos_body;
//  error[S001]: space mismatch — Frame.ECI ≠ Frame.Body

node ok = @eci_to_body.inverse() * @pos_body;   // explicit transform
```

### Multi-Line Nodes

No more single-cell formulas. A node can be a full function body with local variables:

```rust
node hohmann: TransferResult = {
    let r1 = @R_earth + @parking_alt;
    let r2 = @R_earth + @target_alt;
    let a  = (r1 + r2) / 2;
    // ... 15 lines of orbital mechanics ...
    TransferResult { dv1, dv2, total_dv: dv1 + dv2, tof }
};
```

`let` bindings are local. Only `param` and `node` appear in the graph.

### Tables with Autofill

```rust
param maneuvers = [
    ("Departure",  2.46 km/s, 300 s),
    ("Correction", 0.05 km/s,  60 s),
    ("Insertion",  1.48 km/s, 240 s),
];

node maneuvers.fuel_mass = row.delta_v / @v_exhaust * @dry_mass;  // map
node maneuvers.cum_dv    = scan(0 m/s, |acc, row| acc + row.delta_v);  // scan
node total_fuel          = maneuvers.fuel_mass.sum();  // reduce
```

Add a row → computed columns extend automatically.

### N-Dimensional Tables

```rust
index Region { LEO, GTO, Lunar }
index Fuel   { LH2, Methane }

table mass_budget [region: Region, fuel: Fuel] {
    isp: SpecificImpulse,
    cost_per_kg: Money / Mass,
}

// Aggregate across one axis
node cost_by_region [region] = mass_budget.cost_per_kg.sum(over: fuel);
```

The compiler checks axis consistency. The renderer auto-picks matrix views, heatmaps, or slice selectors.

### Dynamic Simulation

No special `stock`/`flow` keywords — just `scan` over a time axis:

```rust
table sim [t: range(0, 200, step: 0.1)] {}

type SIRState { S: f64, I: f64, R: f64 }

node sim.sir = scan(
    SIRState { S: 9999, I: 1, R: 0 },
    |prev, row| {
        let infection = @beta * prev.S * prev.I / @N;
        let recovery  = @gamma * prev.I;
        SIRState {
            S: prev.S - infection * row.dt,
            I: prev.I + (infection - recovery) * row.dt,
            R: prev.R + recovery * row.dt,
        }
    }
);
```

Static calculations and dynamic simulations coexist in one file, one graph.

### Multi-File Projects

```
mission/
├── project.graph        # project root
├── prelude.graph        # dimensions, units, constants (auto-imported)
├── orbit/transfer.graph
├── propulsion/fuel_budget.graph
└── scenarios/high_isp.scenario
```

Explicit imports. Public-by-default. Prelude is the single auto-import exception.

```rust
// propulsion/fuel_budget.graph
use orbit.transfer.{ transfer };

node fuel_mass = @dry_mass * (exp(@transfer.total_dv / @v_exhaust) - 1);
```

### Python Interop

```python
import cellgraph

g = cellgraph.load("mission.graph")
g["parking_alt"] = 400    # triggers recomputation
print(g["fuel_mass"])     # updated value

# Parameter sweep — computed in Rust, returned as DataFrame
results = g.sweep({
    "isp": [300, 350, 400],
    "dry_mass": np.linspace(1000, 2000, 50),
})
```

### Spreadsheet Import/Export

```sh
cellgraph import budget.xlsx > budget.graph    # infer schema
cellgraph export budget.graph --format xlsx    # generate workbook
```

Engineers maintain the `.graph` source. Domain experts keep their Excel.

### Scenarios

```yaml
# scenarios/high_isp.scenario
base: mission_budget.graph
overrides:
  isp: 450 s
  mass_initial: 4500 kg
```

Replaces `budget_v3_final_FINAL(2).xlsx`. Git-trackable. Diffable. Comparable.

## Type System at a Glance

| Layer | Keyword | Catches | Example |
| --- | --- | --- | --- |
| Dimensions | `dimension` | `km + kg` | `dimension Velocity = Length / Time` |
| Units | `unit` | scale errors | `unit km: Length = 1000 m` |
| Types | `type` | structural errors | `type Orbit { sma: Length, ecc: f64 }` |
| Spaces | `space` | frame mix-ups | `space Frame { Body, ECI }` |
| Indexes | `index` | axis mismatches | `index Region { LEO, GTO }` |

Five orthogonal layers, each compile-time, each zero runtime cost.

## Implementation

Written in Rust. Key dependencies: `calamine`, `rust_xlsxwriter`, `petgraph`, `PyO3`, `serde`.

| Phase | Target | Stack |
| --- | --- | --- |
| 1 | CLI + terminal grid | `ratatui` |
| 2 | Web UI | Rust → WASM |
| 3 | VS Code extension | Language server + custom editor |

## Design Influences

- [Numbat](https://numbat.dev) — dimensions as types, units as values, full inference
- [Sguaba](https://github.com/helsing-ai/sguaba) — phantom-typed coordinate frames
- [Gleam](https://gleam.run) — unified `type` for structs and enums
- [Mermaid.js](https://mermaid.js.org) — text-first, auto-layout visualization
- [marimo](https://marimo.io) — reactive DAG on cells, pure text files
- [xarray](https://xarray.dev) — N-dimensional labeled data model

## License

TBD

---

*The first tool where you can code-review a mass budget in a pull request.*
