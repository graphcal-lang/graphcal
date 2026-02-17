# Graphcal

> [!WARNING]
> Graphcal is under active development. Expect breaking changes and bugs.

**A type-safe, unit-aware, Git-friendly reactive programming language for engineering calculations.**

Graphcal replaces the spreadsheets and simulation tools that engineers reluctantly depend on -- Excel mass budgets, ad-hoc Python scripts -- with a single typed, version-controlled, reactive computation graph.

```gcl
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

```sh
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
cargo install --git https://github.com/shunichironomura/graphcal --locked
```

## Usage

```sh
# Evaluate a .gcl file (text output)
graphcal eval path/to/file.gcl

# JSON output
graphcal eval path/to/file.gcl --format json

# Override a param value
graphcal eval path/to/file.gcl --set 'isp=450.0 s'

# Override params from a JSON file
graphcal eval path/to/file.gcl --input params.json

# Multi-file project (use declarations resolved automatically)
graphcal eval project/main.gcl
```

## Features

### Dimensions and units

Declare physical dimensions and annotate values with units.
The compiler catches mismatched operations (e.g., `km + kg`) at compile time.
A built-in prelude provides SI base dimensions, derived dimensions, and common units.

```gcl
param parking_alt: Length = 200 km;
node speed: Velocity = 3138.128 m/s;
node tof_hours: Time = @transfer.tof -> hour;  // unit conversion
```

You can also define your own base dimensions and units for domain-specific quantities:

```gcl
dimension Information;              // new base dimension
unit bit: Information;              // base unit
unit byte: Information = 8.0 bit;   // derived unit
unit kB: Information = 1000.0 byte;

dimension Bandwidth = Information / Time;

param storage: Information = 500.0 kB;
param rate: Bandwidth = 100.0 bit / s;
node transfer_time: Time = @storage / @rate;
```

### Reactive computation graph

Three declaration kinds -- `param` (inputs), `node` (computed), `const` (compile-time) -- form a DAG that is automatically evaluated in dependency order. Override any `param` at the command line with `--set` or `--input`.

```sh
graphcal eval rocket.gcl --set 'isp=450.0 s'
```

When a node fails at runtime, only its dependents are affected -- independent nodes still evaluate successfully.

### Type system

Graphcal has three primitive types:

| Type | Description | Example |
| --- | --- | --- |
| `Float` | 64-bit float with a physical dimension | `1200 kg`, `9.8 m/s^2`, `3.14` |
| `Int` | 64-bit signed integer | `42`, `1_000` |
| `Bool` | Boolean | `true`, `false` |

Every `Float` carries a dimension -- `Length`, `Velocity`, `Dimensionless`, etc. -- and the compiler ensures dimensional consistency across all operations. On top of these primitives, algebraic types (`type` declarations) compose them into structs and tagged unions, and index sets (`index` declarations) create indexed collections.

Integer arithmetic uses checked overflow detection. Convert between types explicitly:

```gcl
param a: Int = 10;
node a_float: Dimensionless = to_float(@a);
node back_to_int: Int = to_int(3.7);  // truncating
```

### Structs and tagged unions

Group related values into typed structs. Define tagged unions with multiple variants, and use `match` expressions to destructure them.

```gcl
// Single-variant struct
type TransferResult {
    dv1: Velocity,
    dv2: Velocity,
    total_dv: Velocity,
    tof: Time,
}

// Tagged union with multiple variants
type ManeuverKind {
    Impulsive { delta_v: Velocity }
    LowThrust { thrust: Force, duration: Time }
}

// Bare variants (no fields)
type Status {
    Nominal
    Warning { code: Dimensionless }
}
```

Match expressions support field binding, renaming, and wildcards:

```gcl
node maneuver: ManeuverKind = LowThrust {
    thrust: @thrust_level,
    duration: @burn_duration,
};

node fuel_proxy: Force = match @maneuver {
    Impulsive { delta_v: _ } => 0.0 N,
    LowThrust { thrust, duration: _ } => thrust,
};

node status_code: Dimensionless = match @current_status {
    Nominal => 0.0,
    Warning { code } => code,
};
```

### Block expressions

Use block bodies with `let` bindings for complex computations:

```gcl
node transfer: TransferResult = {
    let r1 = R_EARTH + @parking_alt;
    let r2 = R_EARTH + @target_alt;
    let a = (r1 + r2) / 2.0;
    // ...
    TransferResult { dv1, dv2, total_dv: dv1 + dv2, tof: PI * sqrt(a ^ 3.0 / GM_EARTH) }
};
```

### If/else expressions

```gcl
const SEVEN: Int = 7;
node clamped: Int = if @a > SEVEN { SEVEN } else { @a };
node result: Dimensionless = if @enabled { 1.0 } else { 0.0 };
```

### Pure functions with dimension generics

Define reusable functions with compile-time dimension checking. Dimension generics (`<D: Dim>`) let you write functions that work across any physical quantity. Index generics (`<I: Index>`) enable generic aggregation.

```gcl
fn orbital_velocity(gm: GravParam, r: Length) -> Velocity = sqrt(gm / r);
fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;
fn total<D: Dim, I: Index>(values: D[I]) -> D = sum(values);
```

Recursive functions are detected and rejected at compile time.

### Generic types with phantom parameters

Type declarations support generic parameters with three constraint kinds: `D: Dim` (dimension), `I: Index` (index set), and `F: Type` (unconstrained/phantom). Default type parameters are supported.

```gcl
type Eci {}    // marker type for ECI reference frame
type Body {}   // marker type for body frame

type Vec3<D: Dim, F: Type> {
    x: D, y: D, z: D,
}

param pos_eci: Vec3<Length, Eci> = Vec3<Length, Eci> {
    x: 6878.0 km, y: 0.0 km, z: 0.0 km,
};

// Cast phantom type parameter (e.g., reference frame conversion)
node pos_body: Vec3<Length, Body> = @pos_eci as Vec3<Length, Body>;
```

Types can derive component-wise arithmetic:

```gcl
type DeriveVec3<D: Dim, F: Type> derive(Add, Sub, Neg) {
    x: D, y: D, z: D,
}

node dv_sum: DeriveVec3<Velocity, Eci> = @dv_a + @dv_b;   // component-wise
node dv_neg: DeriveVec3<Velocity, Eci> = -@dv_a;           // component-wise
```

### Indexed values and aggregation

Define named index sets and operate over them with `for` comprehensions and aggregation functions.

```gcl
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
```

Available aggregation functions: `sum`, `min`, `max`, `mean`, `count`.

### Range indexes and `unfold`

Define numeric-stepping indexes for time-series or iterative computations. Use `unfold` to generate indexed values from a seed with recurrence relations.

```gcl
index TimeStep = range(0.0 s, 1.0 s, step: 0.5 s);

param rate: Frequency = 0.5 Hz;
param x0: Dimensionless = 1.0;

// Exponential growth: x(t+dt) = x(t) * (1 + rate * dt)
node x: Dimensionless[TimeStep] = unfold(
    @x0,
    |prev_t, t| {
        let dt = t - prev_t;
        @x[prev_t] * (1.0 + @rate * dt)
    }
);
```

### Multi-file projects

Split calculations across files with `use` imports. All declaration kinds can be imported, and circular dependencies are detected at compile time.

```gcl
use "./constants.gcl" { G0 };
use "./params.gcl" { dry_mass, fuel_mass, isp };
```

### JSON input for parameter overrides

Override params from a JSON file with `--input`. Supports scalars with units (as strings), booleans, integers, floats, structs, tagged unions, and indexed params.

```json
{
  "dry_mass": "1500.0 kg",
  "isp": "450.0 s",
  "enabled": true,
  "count": 42,
  "maneuver": { "variant": "LowThrust", "fields": { "thrust": "0.5 N", "duration": "3600.0 s" } },
  "delta_v": { "Departure": "3.0 km/s", "Correction": "0.2 km/s", "Insertion": "2.0 km/s" }
}
```

When both `--set` and `--input` are provided, `--set` takes precedence for the same param.

### Developer experience

- Rich error diagnostics with source spans and error codes (via [miette](https://github.com/zkat/miette))
- JSON output for tooling integration (`--format json`)
- Naming convention enforcement: `lower_snake_case` for params/nodes/functions, `UPPER_SNAKE_CASE` for constants, `PascalCase` for types/indexes/dimensions
- Runtime safety checks: division by zero, NaN/infinity detection, integer overflow
- Fault isolation: a failing node does not crash unrelated nodes

## Built-in Reference

### Constants

| Constant | Value | Type |
| --- | --- | --- |
| `PI` | 3.141592653589793... | `Dimensionless` |
| `E` | 2.718281828459045... | `Dimensionless` |

### Mathematical functions

| Function | Signature | Description |
| --- | --- | --- |
| `sqrt(x)` | `D -> D^(1/2)` | Square root (dimension exponents halved) |
| `abs(x)` | `D -> D` | Absolute value |
| `floor(x)` | `D -> D` | Floor |
| `ceil(x)` | `D -> D` | Ceiling |
| `exp(x)` | `Dimensionless -> Dimensionless` | Exponential |
| `ln(x)` | `Dimensionless -> Dimensionless` | Natural logarithm |
| `sin(x)` | `Angle -> Dimensionless` | Sine |
| `cos(x)` | `Angle -> Dimensionless` | Cosine |
| `tan(x)` | `Angle -> Dimensionless` | Tangent |
| `asin(x)` | `Dimensionless -> Angle` | Inverse sine |
| `acos(x)` | `Dimensionless -> Angle` | Inverse cosine |
| `atan2(y, x)` | `(D, D) -> Angle` | Two-argument arctangent |
| `min(a, b)` | `(D, D) -> D` | Minimum of two values |
| `max(a, b)` | `(D, D) -> D` | Maximum of two values |

### Type conversion functions

| Function | Signature | Description |
| --- | --- | --- |
| `to_float(x)` | `Int -> Dimensionless` | Convert integer to float |
| `to_int(x)` | `Dimensionless -> Int` | Truncating conversion to integer |

### Aggregation functions (over indexed values)

| Function | Signature | Description |
| --- | --- | --- |
| `sum(xs)` | `D[I] -> D` | Sum all entries |
| `min(xs)` | `D[I] -> D` | Minimum entry |
| `max(xs)` | `D[I] -> D` | Maximum entry |
| `mean(xs)` | `D[I] -> D` | Arithmetic mean |
| `count(xs)` | `D[I] -> Int` | Number of entries |

### Operators

| Operator | Description | Precedence |
| --- | --- | --- |
| `\|\|` | Logical OR | Lowest |
| `&&` | Logical AND | |
| `==` `!=` `<` `>` `<=` `>=` | Comparison | |
| `+` `-` | Addition, subtraction | |
| `*` `/` `%` | Multiplication, division, modulo | |
| `-` `!` | Unary negation, logical NOT | |
| `^` | Exponentiation (right-associative) | |
| `->` | Unit conversion | |
| `as` | Phantom type cast | |
| `.` `[...]` | Field access, index access | Highest |

### Prelude dimensions

**Base dimensions:** `Length`, `Time`, `Mass`, `Temperature`, `ElectricCurrent`, `Amount`, `LuminousIntensity`, `Angle`

**Derived dimensions:** `Velocity`, `Acceleration`, `Force`, `Energy`, `Power`, `Frequency`, `Pressure`, `Area`, `Volume`

### Prelude units

| Dimension | Units |
| --- | --- |
| Length | `m`, `km`, `cm`, `mm` |
| Time | `s`, `hour`, `min` |
| Mass | `kg`, `g` |
| Temperature | `K` |
| ElectricCurrent | `A` |
| Amount | `mol` |
| LuminousIntensity | `cd` |
| Angle | `rad`, `deg` |
| Force | `N`, `kN` |
| Energy | `J`, `kJ` |
| Power | `W`, `kW` |
| Pressure | `Pa`, `kPa`, `MPa` |
| Frequency | `Hz` |

## Examples

### Hohmann transfer orbit

```gcl
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

```sh
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

### Tagged unions and match expressions

```gcl
// maneuver.gcl
dimension Velocity = Length / Time;
dimension Force = Mass * Length / Time^2;

type ManeuverKind {
    Impulsive { delta_v: Velocity }
    LowThrust { thrust: Force, duration: Time }
}

param thrust_level: Force = 0.5 N;
param burn_duration: Time = 3600.0 s;

node maneuver: ManeuverKind = LowThrust {
    thrust: @thrust_level,
    duration: @burn_duration,
};

node fuel_proxy: Force = match @maneuver {
    Impulsive { delta_v: _ } => 0.0 N,
    LowThrust { thrust, duration: _ } => thrust,
};
```

### Generic types with reference frame safety

```gcl
// frames.gcl
dimension Velocity = Length / Time;

type Eci {}
type Body {}

type Vec3<D: Dim, F: Type> derive(Add, Sub, Neg) {
    x: D, y: D, z: D,
}

param pos_eci: Vec3<Length, Eci> = Vec3<Length, Eci> {
    x: 6878.0 km, y: 0.0 km, z: 0.0 km,
};

// Explicit frame cast required -- no implicit mixing
node pos_body: Vec3<Length, Body> = @pos_eci as Vec3<Length, Body>;

// Component-wise arithmetic
param dv_a: Vec3<Velocity, Eci> = Vec3<Velocity, Eci> {
    x: 100.0 m/s, y: 200.0 m/s, z: 300.0 m/s,
};
param dv_b: Vec3<Velocity, Eci> = Vec3<Velocity, Eci> {
    x: 10.0 m/s, y: 20.0 m/s, z: 30.0 m/s,
};

node dv_sum: Vec3<Velocity, Eci> = @dv_a + @dv_b;
node dv_neg: Vec3<Velocity, Eci> = -@dv_a;
```

### Indexed values with aggregation

```gcl
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

```sh
$ graphcal eval indexed.gcl
delta_v[Departure]        = 2.46 km/s
delta_v[Correction]       = 0.12 km/s
delta_v[Insertion]        = 1.83 km/s
double_dv[Departure]      = 4920 m/s
double_dv[Correction]     = 240 m/s
double_dv[Insertion]      = 3660 m/s
total_dv                  = 4410 m/s
cumulative_dv[Departure]  = 2460 m/s
cumulative_dv[Correction] = 2580 m/s
cumulative_dv[Insertion]  = 4410 m/s
total_check               = 4410 m/s
```

### Time-series with range index and unfold

```gcl
// decay.gcl
dimension Frequency = Time^-1;

index Step = range(0.0 s, 1.0 s, step: 0.25 s);

param k: Frequency = 2.0 Hz;
param y0: Dimensionless = 10.0;

// Exponential decay: y(t+dt) = y(t) * (1 - k * dt)
node y: Dimensionless[Step] = unfold(
    @y0,
    |prev_t, t| {
        let dt = t - prev_t;
        @y[prev_t] * (1.0 - @k * dt)
    }
);
```

### User-defined dimensions and units

```gcl
// information.gcl
dimension Information;
unit bit: Information;
unit byte: Information = 8.0 bit;
unit kB: Information = 1000.0 byte;

dimension Bandwidth = Information / Time;

param storage: Information = 500.0 kB;
param rate: Bandwidth = 100.0 bit / s;
node transfer_time: Time = @storage / @rate;
```

### Multi-file projects with imports

```gcl
// constants.gcl
dimension Acceleration = Length / Time^2;
const G0: Acceleration = 9.80665 m/s^2;
```

```gcl
// params.gcl
param dry_mass: Mass = 1200 kg;
param fuel_mass: Mass = 2800 kg;
param isp: Time = 320 s;
```

```gcl
// main.gcl
use "./constants.gcl" { G0 };
use "./params.gcl" { dry_mass, fuel_mass, isp };

dimension Velocity = Length / Time;

node v_exhaust: Velocity = @isp * G0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

```sh
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

```sh
$ graphcal eval main.gcl --set 'isp=450.0 s'
G0         = 9.80665 m/s^2
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 450 s
v_exhaust  = 4412.9925 m/s
mass_ratio = 3.333333
delta_v    = 5313.122956 m/s
```

## Editor Support

### VS Code

A TextMate grammar extension is included in `editors/vscode/`. To enable syntax highlighting for `.gcl` files before the extension is published to the marketplace, create a symlink:

```sh
ln -s "$(pwd)/editors/vscode" ~/.vscode/extensions/graphcal-0.0.1
```

Then restart VS Code (or run "Developer: Reload Window" from the command palette).

### Zed

A Zed extension is included in `editors/zed/`, providing syntax highlighting and LSP diagnostics. To install it locally as a dev extension:

1. Build Graphcal: `cargo build --release -p graphcal`
2. Open Zed
3. Open the command palette: `Cmd+Shift+P`
4. Run `zed: install dev extension`
5. Select the `editors/zed/` directory
6. Open a `.gcl` file -- syntax should be highlighted and diagnostics should appear

The extension finds `graphcal` on your `PATH` and runs `graphcal lsp`, or you can override the binary path in `.zed/settings.json` (already included in this repo). See the [LSP section](#lsp-language-server) for details.

**Prerequisites:** Rust must be installed via [rustup](https://rustup.rs/) (Zed compiles the tree-sitter grammar from source).

See [`editors/zed/README.md`](editors/zed/README.md) for more details.

### Tree-sitter (Neovim, Helix)

A tree-sitter grammar with highlight queries is available in `tree-sitter-graphcal/`. Refer to your editor's documentation on how to register a custom tree-sitter grammar.

### LSP (Language Server)

The `graphcal lsp` subcommand starts a minimal LSP server that provides real-time diagnostics (parse errors, type/dimension mismatches, unknown references, etc.) in any editor that supports the Language Server Protocol.

Build:

```sh
cargo build --release -p graphcal
```

Start the server (communicates over stdin/stdout):

```sh
graphcal lsp
```

**VS Code** -- Use a generic LSP client extension such as [vscode-lsp-sample](https://github.com/AverageMarcus/vscode-lsp-sample) or add the following to your `settings.json` if you have a generic LSP client installed:

```jsonc
{
  "lsp-client.serverCommand": ["<path-to>/graphcal", "lsp"],
  "lsp-client.languageId": "graphcal"
}
```

**Zed** -- The Zed extension (`editors/zed/`) includes LSP support. After installing it as a dev extension, the server launches automatically if `graphcal` is on your `PATH`. To use a local build instead, add a `.zed/settings.json` to your project (already included in this repo):

```jsonc
{
  "lsp": {
    "graphcal-lsp": {
      "binary": {
        "path": "<path-to>/target/release/graphcal",
        "arguments": ["lsp"]
      }
    }
  }
}
```

**Neovim** -- Use `vim.lsp.start()` or a custom `nvim-lspconfig` server:

```lua
vim.lsp.start({
  name = "graphcal-lsp",
  cmd = { "<path-to>/graphcal", "lsp" },
  filetypes = { "graphcal" },
  root_dir = vim.fn.getcwd(),
})
```

> **Note:** The LSP currently provides diagnostics only. Hover, completions, go-to-definition, and other features are planned for future releases.

## Vision

Graphcal is designed to eventually support:

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
    graphcal-cli/      # CLI binary (clap + miette) -- includes `graphcal lsp` subcommand
    graphcal-lsp/      # LSP server library (tower-lsp) -- diagnostics
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
