# Kasuri

**A type-safe, unit-aware, Git-friendly reactive programming language for engineering calculations.**

Kasuri replaces the spreadsheets and simulation tools that engineers reluctantly depend on -- Excel mass budgets, Vensim logistics models, ad-hoc Python scripts -- with a single typed, version-controlled, reactive computation graph.

## Current Status: Phase 2 (Structs & Multi-Line Nodes)

Phases 0--2 are implemented. Kasuri supports dimensioned arithmetic with physical
units, user-defined struct types, and multi-line node bodies with `let` bindings.

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
v_exhaust  = 3138.128
mass_ratio = 3.333333
delta_v    = 3778.220768
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
transfer.dv1      = 2456.55318
transfer.dv2      = 1478.029792
transfer.total_dv = 3934.582972
transfer.tof      = 18923.604861
total_dv          = 3934.582972
tof_hours         = 5.256557 hour
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
```

## Project Structure

```
kasuri/
  crates/
    kasuri-syntax/   # lexer (logos) + recursive descent parser + AST
    kasuri-eval/     # name resolution, dim check, const eval, DAG, runtime eval
    kasuri-cli/      # CLI binary (clap + miette)
  design/            # language design documents
  tests/fixtures/    # .ksr test files
```

## Vision

Kasuri is designed to eventually support:

- **Coordinate frame safety** -- prevents mixing vectors from different reference frames at compile time
- **Pure functions** -- reusable computation with full type checking
- **Tables with autofill** -- add a row, computed columns extend automatically
- **Dynamic simulation** -- `scan` over a time axis for system dynamics
- **Multi-file projects** -- explicit imports, namespaces, prelude
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
