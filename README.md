# Graphcal

**A type-safe, unit-aware, Git-friendly reactive programming language for engineering calculations.**

Graphcal replaces the spreadsheets and simulation tools that engineers reluctantly depend on -- Excel mass budgets, Vensim logistics models, ad-hoc Python scripts -- with a single typed, version-controlled, reactive computation graph.

```
// rocket.gcl
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
$ graphcal eval rocket.gcl
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 320 s
G0         = 9.80665 m/s^2
v_exhaust  = 3138.128 m/s
mass_ratio = 3.333333
delta_v    = 3778.220768 m/s
```

## Installation

```sh
cargo build -p graphcal
```

## Usage

```sh
# Evaluate a .gcl file (text output)
graphcal eval path/to/file.gcl

# JSON output
graphcal eval path/to/file.gcl --format json

# Override a param value
graphcal eval path/to/file.gcl --set 'isp=450 s'

# Multi-file project (use declarations resolved automatically)
graphcal eval project/main.gcl
```

## Features

**Dimensions and units** --
Declare physical dimensions and annotate values with units.
The compiler catches mismatched operations (e.g., `km + kg`) at compile time.
A built-in prelude provides SI base dimensions, derived dimensions, and common units.

```
param parking_alt: Length = 200 km;
node speed: Velocity = 3138.128 m/s;
node tof_hours: Time = @transfer.tof -> hour;  // unit conversion
```

**Reactive computation graph** --
Three declaration kinds -- `param` (inputs), `node` (computed), `const` (compile-time) -- form a DAG that is automatically evaluated in dependency order. Override any `param` at the command line with `--set`.

```sh
graphcal eval rocket.gcl --set 'isp=450 s'
```

**Structs and multi-line nodes** --
Group related values into typed structs. Use block bodies with `let` bindings for complex computations.

```
type TransferResult {
    dv1: Velocity,
    dv2: Velocity,
    total_dv: Velocity,
    tof: Time,
}

node transfer: TransferResult = {
    let r1 = R_EARTH + @parking_alt;
    let r2 = R_EARTH + @target_alt;
    let a = (r1 + r2) / 2.0;
    // ...
    TransferResult { dv1, dv2, total_dv: dv1 + dv2, tof: PI * sqrt(a ^ 3.0 / GM_EARTH) }
};
```

**Pure functions with dimension generics** --
Define reusable functions with compile-time dimension checking. Dimension generics (`<D: Dim>`) let you write functions that work across any physical quantity.

```
fn orbital_velocity(gm: GravParam, r: Length) -> Velocity = sqrt(gm / r);
fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;
```

**Indexed values and aggregation** --
Define named index sets and operate over them with `for` comprehensions and aggregation functions (`sum`, `min`, `max`, `mean`, `count`, `scan`).

```
index Maneuver = { Departure, Correction, Insertion }

param delta_v: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km/s,
    Maneuver::Correction: 0.12 km/s,
    Maneuver::Insertion: 1.83 km/s,
};

node total_dv: Velocity = sum(for m: Maneuver { @delta_v[m] });
```

**Multi-file projects** --
Split calculations across files with `use` imports. All declaration kinds can be imported, and circular dependencies are detected at compile time.

```
use "./constants.gcl" { G0 };
use "./params.gcl" { dry_mass, fuel_mass, isp };
```

**Developer experience** --
Rich error diagnostics with source spans and error codes (via [miette](https://github.com/zkat/miette)), JSON output for tooling integration, and naming convention enforcement.

## Examples

### Hohmann transfer orbit

```
// hohmann.gcl
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
$ graphcal eval hohmann.gcl
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
// functions.gcl
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
$ graphcal eval functions.gcl
R_EARTH      = 6371 km
GM_EARTH     = 398600.4418 km^3/s^2
parking_alt  = 200 km
target_alt   = 35786 km
v_parking    = 7788.487985 m/s
midpoint_alt = 17993000 m
```

### Indexed values with aggregation

```
// indexed.gcl
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
$ graphcal eval indexed.gcl
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
// constants.gcl
dimension Acceleration = Length / Time^2;
const G0: Acceleration = 9.80665 m/s^2;
```

```
// params.gcl
param dry_mass: Mass = 1200 kg;
param fuel_mass: Mass = 2800 kg;
param isp: Time = 320 s;
```

```
// main.gcl
use "./constants.gcl" { G0 };
use "./params.gcl" { dry_mass, fuel_mass, isp };

dimension Velocity = Length / Time;

node v_exhaust: Velocity = @isp * G0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

```
$ graphcal eval main.gcl
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
$ graphcal eval main.gcl --set 'isp=450 s'
G0         = 9.80665 m/s^2
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 450 s
v_exhaust  = 4412.9925 m/s
mass_ratio = 3.333333
delta_v    = 5313.122956 m/s
```

## Vision

Graphcal is designed to eventually support:

- **Coordinate frame safety** -- prevents mixing vectors from different reference frames at compile time
- **Dynamic simulation** -- `scan` over a time axis for system dynamics
- **Python interop** -- parameter sweeps and Monte Carlo at native speed
- **Spreadsheet import/export** -- maintain `.gcl` source, domain experts keep their Excel

See the [design documents](design/README.md) for details.

## Project Structure

```
graphcal/
  crates/
    graphcal-syntax/   # lexer (logos) + recursive descent parser + AST
    graphcal-eval/     # name resolution, dim check, const eval, DAG, runtime eval
    graphcal-cli/      # CLI binary (clap + miette)
  design/            # language design documents
  tests/fixtures/    # .gcl test files (single-file and multi-file)
```

## Design Influences

- [Numbat](https://numbat.dev) -- dimensions as types, units as values
- [Sguaba](https://github.com/helsing-ai/sguaba) -- phantom-typed coordinate frames
- [Gleam](https://gleam.run) -- unified `type` for structs and enums
- [marimo](https://marimo.io) -- reactive DAG on cells, pure text files

## License

TBD
