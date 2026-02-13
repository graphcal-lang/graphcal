# Kasuri

**A type-safe, unit-aware, Git-friendly reactive programming language for engineering calculations.**

Kasuri replaces the spreadsheets and simulation tools that engineers reluctantly depend on -- Excel mass budgets, Vensim logistics models, ad-hoc Python scripts -- with a single typed, version-controlled, reactive computation graph.

## Current Status: Phase 5 + Multi-File Imports

Phases 0--5 and multi-file imports are implemented. Kasuri supports dimensioned
arithmetic with physical units, user-defined struct types, multi-line node bodies
with `let` bindings, pure functions with dimension generics, indexed values with
aggregation, multi-file projects with `use` imports, and runtime parameter
overrides via `--set`.

### Rocket equation with units

```
// rocket.ksr
dimension Velocity = Length / Time;
dimension Acceleration = Length / Time^2;

param dry_mass: Mass = 1200 kg;
param fuel_mass: Mass = 2800 kg;
param isp: Time = 320 s;
const G0: Acceleration = 9.80665 m/s^2;

node v_exhaust: Velocity = @isp * G0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

```
$ kasuri eval rocket.ksr
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 320 s
G0         = 9.80665 m/s^2
v_exhaust  = 3138.128 m/s
mass_ratio = 3.333333
delta_v    = 3778.220768 m/s
```

### Hohmann transfer with structs and blocks

```
// hohmann.ksr
dimension Velocity = Length / Time;
dimension GravParam = Length^3 / Time^2;

type TransferResult {
    dv1: Velocity,
    dv2: Velocity,
    total_dv: Velocity,
    tof: Time,
}

const R_EARTH: Length = 6371 km;
const GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;

param parking_alt: Length = 200 km;
param target_alt: Length = 35786 km;

node transfer: TransferResult = {
    let r1 = R_EARTH + @parking_alt;
    let r2 = R_EARTH + @target_alt;
    let a = (r1 + r2) / 2.0;

    let v1 = sqrt(GM_EARTH / r1);
    let v2 = sqrt(GM_EARTH / r2);
    let dv1 = sqrt(2.0 * GM_EARTH * r2 / (r1 * (r1 + r2))) - v1;
    let dv2 = v2 - sqrt(2.0 * GM_EARTH * r1 / (r2 * (r1 + r2)));

    TransferResult {
        dv1,
        dv2,
        total_dv: dv1 + dv2,
        tof: PI * sqrt(a ^ 3.0 / GM_EARTH),
    }
};

node total_dv: Velocity = @transfer.total_dv;
node tof_hours: Time = @transfer.tof -> hour;
```

```
$ kasuri eval hohmann.ksr
R_EARTH           = 6371 km
GM_EARTH          = 398600.4418 km^3/s^2
parking_alt       = 200 km
target_alt        = 35786 km
transfer.dv1      = 2456.55318 m/s
transfer.dv2      = 1478.029792 m/s
transfer.total_dv = 3934.582972 m/s
transfer.tof      = 18923.604861 s
total_dv          = 3934.582972 m/s
tof_hours         = 5.256557 hour
```

### Reusable functions with dimension generics

```
// functions.ksr
dimension Velocity = Length / Time;
dimension GravParam = Length^3 / Time^2;

const R_EARTH: Length = 6371 km;
const GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;

fn orbital_velocity(gm: GravParam, r: Length) -> Velocity = sqrt(gm / r);

fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;

param parking_alt: Length = 200 km;
param target_alt: Length = 35786 km;

node v_parking: Velocity = orbital_velocity(GM_EARTH, R_EARTH + @parking_alt);
node midpoint_alt: Length = lerp(@parking_alt, @target_alt, 0.5);
```

```
$ kasuri eval functions.ksr
R_EARTH      = 6371 km
GM_EARTH     = 398600.4418 km^3/s^2
parking_alt  = 200 km
target_alt   = 35786 km
v_parking    = 7788.487985 m/s
midpoint_alt = 17993000 m
```

### Indexed values with aggregation

```
// indexed.ksr
dimension Velocity = Length / Time;

index Maneuver = { Departure, Correction, Insertion }

param delta_v: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km/s,
    Maneuver::Correction: 0.12 km/s,
    Maneuver::Insertion: 1.83 km/s,
};

node double_dv: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] * 2.0
};

node total_dv: Velocity = sum(for m: Maneuver { @delta_v[m] });
node cumulative_dv: Velocity[Maneuver] = scan(@delta_v, 0.0 m/s, |acc, val| acc + val);

fn total<D: Dim, I: Index>(values: D[I]) -> D = sum(values);
node total_check: Velocity = total(@delta_v);
```

```
$ kasuri eval indexed.ksr
delta_v[Departure]          = 2.46 km/s
delta_v[Correction]         = 0.12 km/s
delta_v[Insertion]          = 1.83 km/s
double_dv[Departure]        = 4920 m/s
double_dv[Correction]       = 240 m/s
double_dv[Insertion]        = 3660 m/s
total_dv                    = 4410 m/s
cumulative_dv[Departure]    = 2460 m/s
cumulative_dv[Correction]   = 2580 m/s
cumulative_dv[Insertion]    = 4410 m/s
total_check                 = 4410 m/s
```

### Multi-file projects with imports

```
// constants.ksr
dimension Acceleration = Length / Time^2;
const G0: Acceleration = 9.80665 m/s^2;
```

```
// params.ksr
param dry_mass: Mass = 1200 kg;
param fuel_mass: Mass = 2800 kg;
param isp: Time = 320 s;
```

```
// main.ksr
use "./constants.ksr" { G0 };
use "./params.ksr" { dry_mass, fuel_mass, isp };

dimension Velocity = Length / Time;

node v_exhaust: Velocity = @isp * G0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

```
$ kasuri eval main.ksr
G0         = 9.80665 m/s^2
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 320 s
v_exhaust  = 3138.128 m/s
mass_ratio = 3.333333
delta_v    = 3778.220768 m/s
```

Imported params can be overridden at the command line:

```
$ kasuri eval main.ksr --set 'isp=450 s'
G0         = 9.80665 m/s^2
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 450 s
v_exhaust  = 4412.9925 m/s
mass_ratio = 3.333333
delta_v    = 5313.122956 m/s
```

### Features

**Core language (Phase 0)**

- Three declaration kinds: `param` (inputs), `node` (computed), `const` (compile-time)
- `@` sigil for graph references, bare `UPPER_SNAKE_CASE` for const references
- Arithmetic (`+`, `-`, `*`, `/`, `^`), comparison, boolean (`&&`, `||`, `!`), `if`/`else`
- 14 built-in functions (`sqrt`, `sin`, `cos`, `ln`, `exp`, `atan2`, `min`, `max`, etc.)
- Built-in constants (`PI`, `E`)
- Two-phase evaluation: compile-time (const) then runtime (param/node DAG)
- Casing enforcement: `const` must be `UPPER_SNAKE_CASE`, `param`/`node` must be `lower_snake_case`
- Rich error diagnostics via [miette](https://github.com/zkat/miette) with error codes and source spans
- JSON output (`--format json`)

**Dimensions and units (Phase 1)**

- `dimension` and `unit` declarations for physical quantities
- Type annotations on declarations (`param x: Length = 100 km;`)
- Unit literals (`100 km`, `9.80665 m/s^2`) and unit conversion (`-> hour`)
- Compile-time dimension checking catches mismatched operations (`km + kg`)
- Prelude with SI base dimensions, derived dimensions, and common units

**Structs and multi-line nodes (Phase 2)**

- `type` declarations for user-defined struct types with dimensioned fields
- Block bodies with `let` bindings for multi-line node expressions
- Struct construction with explicit and shorthand (`{ dv1 }`) field syntax
- Field access with `.` operator, chainable (`@transfer.result.inner`)
- Implicit return (last expression in block without `;`)
- No shadowing within blocks (duplicate `let` is a compile error)

**Pure functions (Phase 3)**

- `fn` declarations with short (`= expr;`) and block (`{ let ...; expr }`) forms
- Dimension generics (`<D: Dim>`) with compile-time unification at call sites
- Purity enforcement: `@` graph references forbidden in function bodies
- Functions can call other user-defined functions and builtins
- Recursion detection (direct and mutual) with compile-time error
- SI unit fallback display for computed values (e.g., `m/s` for Velocity)

**Indexed values (Phase 5)**

- `index` declarations with named variants (`index Maneuver = { Departure, Correction, Insertion }`)
- Indexed types as first-class (`Velocity[Maneuver]`, multi-axis `T[A, B]`)
- Map literals with totality checking (`{ Maneuver::Departure: 2.46 km/s, ... }`)
- `for` comprehensions with explicit iteration (`for m: Maneuver { @delta_v[m] * 2.0 }`)
- Indexing by variant (`@x[Maneuver::Departure]`) or loop variable (`@x[m]`)
- Aggregation functions: `sum`, `min`, `max`, `mean`, `count` collapse indexed values to scalars
- `scan` for ordered accumulation (`scan(@delta_v, 0.0 m/s, |acc, val| acc + val)`)
- Generic index constraint (`<I: Index>`) alongside dimension generics

**Multi-file imports (Phase 4)**

- `use "./path/to/file.ksr" { name1, name2 };` imports declarations from other files
- File paths are resolved relative to the importing file's directory
- All declaration kinds can be imported (const, param, node, dimension, unit, type, index, fn)
- Imported params/nodes participate in the DAG and can be overridden with `--set`
- Circular import detection with clear error messages
- Diamond imports (A imports B and C, both import D) are deduplicated

**Parameter overrides (Phase 6)**

- `--set 'param_name=value'` overrides param defaults at the command line
- Works with both local and imported params
- Supports unit expressions (`--set 'isp=450 s'`)
- Multiple `--set` flags can be used simultaneously

## Installation

```sh
cargo build -p kasuri
```

## Usage

```sh
# Evaluate a .ksr file (text output)
kasuri eval path/to/file.ksr

# JSON output
kasuri eval path/to/file.ksr --format json

# Override a param value
kasuri eval path/to/file.ksr --set 'isp=450 s'

# Multi-file project (use declarations resolved automatically)
kasuri eval project/main.ksr
```

## Project Structure

```
kasuri/
  crates/
    kasuri-syntax/   # lexer (logos) + recursive descent parser + AST
    kasuri-eval/     # name resolution, dim check, const eval, DAG, runtime eval
    kasuri-cli/      # CLI binary (clap + miette)
  design/            # language design documents
  tests/fixtures/    # .ksr test files (single-file and multi-file)
```

## Vision

Kasuri is designed to eventually support:

- **Coordinate frame safety** -- prevents mixing vectors from different reference frames at compile time
- **Dynamic simulation** -- `scan` over a time axis for system dynamics
- **Python interop** -- parameter sweeps and Monte Carlo at native speed
- **Spreadsheet import/export** -- maintain `.ksr` source, domain experts keep their Excel

See the [design documents](design/README.md) and [phase roadmap](design/phases/README.md) for details.

## Design Influences

- [Numbat](https://numbat.dev) -- dimensions as types, units as values
- [Sguaba](https://github.com/helsing-ai/sguaba) -- phantom-typed coordinate frames
- [Gleam](https://gleam.run) -- unified `type` for structs and enums
- [marimo](https://marimo.io) -- reactive DAG on cells, pure text files

## License

TBD
