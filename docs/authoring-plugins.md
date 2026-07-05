---
icon: material/toy-brick
---

# Plugin Authoring

!!! warning "Experimental"
    The plugin system is experimental: the ABI, the SDK macro surface, and
    the CLI commands on this page may change in any release. Please
    [report issues](https://github.com/graphcal-lang/graphcal/issues).

This guide walks through writing a WASM plugin in Rust with the
`graphcal-plugin` SDK — from scaffold to a pinned, evaluating module. For
the language-side view (declaring and calling extern functions, the module
contract, trust rules), see
[Extern Functions](language/extern-functions.md).

A graphcal plugin is a **pure, sandboxed scalar kernel library**: functions
from `f64`s to an `f64`, with dimensional signatures checked by the
graphcal compiler at every call site. Plugins fit computations that cannot
be a `dag` block — iterative solvers, special functions, property
libraries, coordinate transforms. They cannot touch the filesystem or
network by construction.

## 1. Scaffold

```bash
graphcal plugin new fluid-props
cd fluid-props
```

This creates a ready-to-build Rust crate:

```text
fluid-props/
├── Cargo.toml            # cdylib + rlib, graphcal-plugin dependency
├── rust-toolchain.toml   # stable + the wasm32-unknown-unknown target
├── justfile              # `just build`, `just test`
├── src/lib.rs            # a plugin! block with sample kernels
└── README.md
```

## 2. Declare and implement

Everything lives in one `plugin!` block — signatures in graphcal's
extern-declaration syntax, bodies in Rust:

```rust
graphcal_plugin::plugin! {
    /// Ideal-gas density of dry air.
    fn air_density(p: Pressure, t: Temperature) -> Mass / Volume {
        const R_SPECIFIC: f64 = 287.052874; // J/(kg*K)
        if t <= 0.0 {
            graphcal_plugin::fail!("temperature must be positive, got {t} K");
        }
        p / (R_SPECIFIC * t)
    }

    /// Linear interpolation, polymorphic over the dimension of `a`/`b`.
    fn lerp<D>(a: D, b: D, t: Dimensionless) -> D {
        (b - a).mul_add(t, a)
    }
}
```

From this single declaration the macro generates the wasm exports **and**
the manifest embedded in the module — arity, parameter order, and
dimensional signatures cannot drift apart, and the signature lines can be
pasted verbatim into the `.gcl` import site.

### Signature syntax

Parameter and result types are `Bool`, `Int`, or dimension expressions
built from:

| Vocabulary | Names |
|------------|-------|
| Dimension variables | declared per function in `<...>`, e.g. `<D>`, `<D1, D2>` |
| Prelude base dimensions | `Length`, `Time`, `Mass`, `Temperature`, `ElectricCurrent`, `Amount`, `LuminousIntensity`, `Angle` |
| Prelude derived dimensions | `Velocity`, `Acceleration`, `Force`, `Energy`, `Power`, `Frequency`, `Pressure`, `Area`, `Volume` |
| The empty product | `Dimensionless` |

combined with `*`, `/`, parentheses, and `^` exponents — integers (`^2`,
`^-3`) or parenthesized rationals (`^(1/2)`, `^(-1/2)`). Derived names are
expanded to base-dimension exponents in the manifest, so `Pressure` and
`Mass * Length^-1 * Time^-2` declare the same contract.

Two rules mirror the compiler's checks (violations are compile errors in
the plugin crate, with the same meaning as P005/P016 on the graphcal
side):

- every dimension variable must first appear as a **bare** parameter type
  (`x: D`) before any compound use (`D^2`, `D1 * D2`, or the result);
- exponents are non-zero.

### In the body

Parameters arrive with their declared names and natural Rust types —
`f64` for scalars, `bool` for `Bool`, `i64` for `Int` — and the body
evaluates to `f64`, `bool`, or `i64` to match the declared result.

**Scalar values are SI base units, always.** A `Pressure` parameter is
pascals; a `Velocity` result is metres per second. Graphcal checks
dimensions at every call site, but it cannot see whether your math treats
a pascal as a bar — that residual risk lives inside the plugin, so keep
kernel math in SI throughout.

Dimension variables are parametric: the body never learns which dimension
`D` was bound to, so dimension-polymorphic kernels must be
dimension-uniform (interpolation yes, `sin` of a `D` no).

### Failures and panics

Call `graphcal_plugin::fail!("...")` (or `fail(&str)`) for domain
failures — the message aborts the call and surfaces in the failing node's
diagnostic, while unrelated nodes keep evaluating. Rust panics (from
`assert!`, `unwrap`, arithmetic checks) are forwarded through the same
channel with the panic message, so they are diagnosable rather than
anonymous traps. On non-wasm targets both are ordinary panics.

## 3. Test natively

The `plugin!` expansion is plain Rust off-wasm, so kernels are unit-tested
with `cargo test` exactly like any crate — failures appear as panics with
the `fail!` message:

```rust
#[test]
fn density_of_air_at_stp() {
    let rho = super::air_density(101_325.0, 288.15);
    assert!((rho - 1.225).abs() < 1e-3);
}
```

## 4. Build and validate

```bash
cargo build --release --target wasm32-unknown-unknown
graphcal plugin test target/wasm32-unknown-unknown/release/fluid_props.wasm \
    --call air_density 101325 288.15
```

`graphcal plugin test` runs every load-time ABI check (manifest, import
ban, export types), prints the module's SHA-256 and a **paste-ready
`import plugin` block**, and `--call` executes one function under the same
fuel and memory limits evaluation uses — arguments in SI base units,
`true`/`false` for `Bool`, integers for `Int`.

## 5. Vendor, declare, pin

Copy the module into the graphcal project (say `plugins/`), paste the
declarations, and pin:

```text
import plugin "plugins/fluid_props.wasm" as fluids {
    fn air_density(p: Pressure, t: Temperature) -> Mass / Volume;
    fn lerp<D>(a: D, b: D, t: Dimensionless) -> D;
}

node rho: Mass / Volume = fluids.air_density(@chamber_p, @chamber_t);
```

```bash
graphcal deps lock    # records the module's SHA-256 in graphcal.lock
```

The lockfile is the trust boundary: plugin bytes can only change together
with a reviewable `graphcal.lock` diff. See
[Trust: Lockfile Pins](language/extern-functions.md#trust-lockfile-pins).

## Scope and limits (ABI v1)

- Values are scalars (`f64` in SI), `Bool`, or `Int`; arrays, structs, and
  `Datetime` do not cross the boundary yet.
- One `plugin!` block per plugin (a second block fails the wasm link with
  a duplicate-symbol error); helper functions can live anywhere in the
  crate.
- Plugins may import nothing (the SDK's failure channel is the one
  exception, wired automatically), so crates that pull in I/O, threads,
  or randomness will be rejected at load time.
- Keep vocabulary in `.gcl`: plugins cannot define units, dimensions, or
  types.

## Authoring without the SDK

The SDK is a convenience, not part of the trust model — a plugin is any
core wasm module satisfying the [module
contract](language/extern-functions.md#wasm-plugin-modules): exports of
type `(f64 × arity) -> f64`, a `graphcal-manifest` custom section, no
imports beyond the optional `graphcal::fail`. For non-Rust toolchains,
emit the manifest JSON (the `graphcal-plugin-abi` crate documents the
model and provides `embed_manifest` for build tooling) and verify the
result with `graphcal plugin test`.
