---
icon: material/power-plug
---

# Extern Functions (Plugins)

!!! warning "Experimental"
    The plugin system is experimental and relatively immature: the WASM
    runtime is new, has seen little real-world use, and its surface — the
    ABI, the manifest format, the diagnostics, and the lockfile pinning
    rules — may change in any release. Treat results from plugins with the
    same skepticism you would apply to any unreviewed external code, and
    please [report issues](https://github.com/graphcal-lang/graphcal/issues).

Extern functions let a graphcal project call scalar functions implemented
outside the language — WebAssembly plugin modules vendored in the project,
or native functions provided by the *embedder* (the CLI, the language
server, or a program embedding the evaluation engine). They are the escape
hatch for computations that cannot be expressed as a `dag` block: iterative
solvers, special functions, property libraries, coordinate transforms.

## Declaring a Plugin

An `import plugin` block declares which functions a plugin provides and,
crucially, their full dimensional signatures — in graphcal vocabulary, at
the import site:

```
import plugin "plugins/fluids.wasm" as fluids {
    fn density(p: Pressure, t: Temperature) -> Mass * Length^-3;
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
    fn geometric_mean<D1: Dim, D2: Dim>(x: D1, y: D2) -> D1^(1/2) * D2^(1/2);
}
```

- The **path string** identifies the plugin. A path ending in `.wasm` names
  a WebAssembly module file, resolved relative to the project root (never
  the importing file) and required to stay inside it. Any other spelling —
  such as the built-in `"graphcal:demo"` — is an identity provided natively
  by the embedder's host function registry.
- The **alias** (`as fluids`) is mandatory. Extern functions are only
  callable qualified through it — `fluids.density(...)` — never bare. This
  mirrors the explicitness of module imports and keeps the built-in
  function namespace closed.
- Each `fn` declares **named parameters** and a **result type**. Parameter
  and result types may be `Bool`, `Int`, scalar dimension expressions, or
  arrays of scalars over a declared index variable (`xs: D[I]`); the result
  may additionally be a record type in scope (see
  [Struct Returns](#struct-returns)).

Signatures are declared explicitly rather than inferred from the plugin:
the declaration in your source is the contract your project type-checks
against, reviewable in plain-text diffs and usable by editor tooling
without the binary. At load time, each declaration is **verified
structurally against the manifest embedded in the `.wasm` module** —
renaming dimension variables or parameters is fine, but any difference in
dimensional shape is a compile error (P005), so drift between source and
binary can never be silently reinterpreted.

## Dimension Variables

A signature may declare *dimension variables* in explicit angle-bracket
binders, making a function polymorphic over dimensions:

```
fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
```

At each call site, `D` binds to the actual argument dimension, every
other `D` parameter must match it, and the result dimension is computed
from the binding. Result types may combine several variables with
rational powers — full cross-variable dimension algebra:

```
fn geometric_mean<D1: Dim, D2: Dim>(x: D1, y: D2) -> D1^(1/2) * D2^(1/2);
```

```
node scale: Length = demo.geometric_mean(4.0 m, 9.0 m);   // = 6 m
```

One rule keeps checking decidable: **every dimension variable must first
appear as a bare parameter** (`x: D`) before it is used in a compound form
(`D^2`, `D1 * D2`) or in the result. A signature like `fn sq<D: Dim>(x: D^2) -> D`
is rejected — it would require solving for `D` rather than binding it.

Dimension polymorphism is deliberately *parametric*: the plugin never
learns which dimension `D` was bound to, so it cannot branch on units —
the implicit behavior graphcal bans stays banned across the plugin
boundary.

## Arrays over Index Variables

A signature may also declare *index variables* (`I: Index`) and take or
return arrays of scalars over them:

```
import plugin "plugins/dsp.wasm" as dsp {
    fn smooth<D: Dim, I: Index>(xs: D[I], window: Dimensionless) -> D[I];
    fn total<D: Dim, I: Index>(xs: D[I]) -> D;
}

index Maneuver = { Departure, Correction, Insertion };
node dv: Velocity[Maneuver] = { Maneuver.Departure: 2.0 km/s, Maneuver.Correction: 0.5 km/s, Maneuver.Insertion: 1.5 km/s };
node dv_smooth: Velocity[Maneuver] = dsp.smooth(@dv, 3.0);
```

Index variables follow the same explicit discipline as dimension
variables:

- An array's index position must name one of the declared `Index` binders
  — concrete indexes (`Velocity[Maneuver]`) and Nat lengths (`D[3]`)
  cannot cross the plugin boundary. Like dimension variables, index
  variables are parametric: the plugin sees each array only as a dense
  buffer of SI values in index order, never the index's identity.
- Two parameters sharing an index variable must be passed arrays over the
  *same* index.
- A **result array must reuse an index variable that indexes some
  parameter** — its length is always determined by an input, so a plugin
  can never invent its output length. Results are rebuilt over exactly the
  binding argument's index, ready for `sum`, indexing, and `for`
  comprehensions like any other indexed value.
- A bare array element (`xs: D[I]`) is a binding occurrence for `D`, just
  like a bare scalar parameter.
- Array elements are scalars in this phase (`Bool[I]`/`Int[I]` and
  multi-axis arrays are not supported).

## Struct Returns

A function may return several named values at once by declaring a
**record type in scope** as its result:

```
import plugin "plugins/stats.wasm" as stats {
    fn span<D: Dim, I: Index>(xs: D[I]) -> DvSpan;
}

type DvSpan { DvSpan(lo: Velocity, hi: Velocity) }

node dv_span: DvSpan = stats.span(@dv);
node spread: Velocity = @dv_span.lo - @dv_span.hi;
```

The plugin's manifest never learns the type's *name* — it declares the
flattened field shape (names, order, and kinds), and the declaration
binds that shape to the nominal record type. Field names and order are
part of the contract: a plugin declaring `{min, max}` does not match a
declaration whose record has `{lo, hi}` (P005). The result evaluates to
an ordinary struct value with working field access and matching.

Restrictions in this phase, each with a dedicated compile error:

- The named type must be a **record** — a single constructor named after
  the type. Multi-variant unions have no flattened layout to cross the
  boundary.
- Fields must be `Bool`, `Int`, or **concrete** scalar dimensions —
  generic records and dimension-variable fields cannot cross yet.
- Structs are result-only. A struct *parameter* should be passed as
  separate scalar parameters instead.

## Calling Extern Functions

Extern calls look like qualified function calls and participate in the
graph like any other expression:

```
param v0: Velocity = 100.0 m/s;
param v1: Velocity = 300.0 m/s;

node v_mid: Velocity = demo.lerp(@v0, @v1, 0.25);
```

Restrictions, all enforced at compile time:

- Extern functions are **runtime-provided**, so they cannot appear in
  `const` expressions, domain bounds, or unit scale expressions (P004).
- Calls must be alias-qualified; a bare `lerp(...)` is an unknown
  function.
- There is **no auto-lifting** over indexed values: an extern scalar
  function applies element-wise only through an explicit `for`
  comprehension, keeping the iteration visible in the source.

```
index Sample = { A, B };
node xs: Length[Sample] = { Sample.A: 1.0 m, Sample.B: 2.0 m };
node mids: Length[Sample] = for s: Sample {
    demo.lerp(@xs[s], 10.0 m, 0.5)
};
```

## WASM Plugin Modules

A graphcal plugin is a **core WebAssembly module**, vendored in the
project (committed next to the sources) and executed by an embedded,
deterministic interpreter. The module must satisfy the ABI, all checked
at load time before any plugin code runs:

- **Manifest.** The module embeds a JSON manifest in a custom section
  named `graphcal-manifest`, declaring `abi_version: 2` and each provided
  function's dimensional signature (dimension and index variables, named
  parameters, the result — including array kinds and struct field
  layouts). Fixed dimensions are spelled structurally over the eight
  prelude base dimensions (`Length`, `Time`, `Mass`, `Temperature`,
  `ElectricCurrent`, `Amount`, `LuminousIntensity`, `Angle`) with rational
  exponents — `Velocity` is `Length^1 * Time^-1`. User-defined base
  dimensions cannot cross the binary boundary.
- **Value ABI.** Each function's wasm export type follows its signature:
  scalar/`Bool`/`Int` parameters are one `f64` each (raw SI base units;
  `Int` as exactly-representable integers, `Bool` as `1.0`/`0.0`), and an
  array parameter is an `(i32 ptr, i32 len)` pair pointing at `len` dense
  little-endian `f64` elements in index order. A scalar result is the
  single `f64` return value; an array or struct result replaces the
  return with one trailing `i32` out-pointer the plugin fills — `len`
  elements for an array (the length of the input bound to the result's
  index variable) or one slot per field for a struct. A non-finite scalar
  flows into graphcal's ordinary non-finite containment.
- **Allocator exports.** A module that takes or returns arrays or structs
  must export its memory as `"memory"` plus
  `graphcal_alloc(size: i32) -> i32` (8-byte-aligned) and
  `graphcal_free(ptr: i32, size: i32)`. The host allocates every buffer
  before a call, writes the inputs, and frees everything after reading
  the result — a plugin never retains a buffer across calls.
- **No imports.** The module may import nothing — with one exception:
  `graphcal::fail(ptr: i32, len: i32)`, the host-provided failure
  reporter. The import ban is what makes plugins pure and I/O-free by
  construction; a module importing WASI or other host APIs is rejected
  with a dedicated diagnostic (P007). A module importing `graphcal::fail`
  must export its linear memory as `"memory"` so the failure message can
  be read.
- **Resource bounds.** Every call runs under a fuel budget (roughly,
  an instruction count) and a linear-memory cap. These bounds are why
  plugins can be trusted by default: the sandbox removes filesystem and
  network access, and fuel plus the memory cap bound how much time and
  memory a call can consume — the language server re-evaluates on every
  debounced keystroke, so this protects the editor, not just one CLI run.
- **Determinism.** Plugin arithmetic is IEEE-754 deterministic and the
  math is compiled into the module, so results are bit-identical across
  platforms.

To report a domain failure (say, an out-of-range property lookup), a
plugin calls `graphcal::fail` with a UTF-8 message; the call is aborted
and the message surfaces in the node's diagnostic. Traps and exhausted
fuel are reported the same way, without a custom message.

Compiled modules are cached by content hash, so re-analysis in the
language server does not recompile unchanged plugins.

### Authoring

The `graphcal-plugin` Rust SDK declares each function once — signature in
graphcal's extern-declaration syntax, body in Rust — and generates both
the wasm export and the embedded manifest from that single source of
truth:

```rust
graphcal_plugin::plugin! {
    /// Linear interpolation between `a` and `b`.
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D {
        (b - a).mul_add(t, a)
    }
}
```

`graphcal plugin new` scaffolds a ready-to-build crate and
`graphcal plugin test` validates and calls the built module. See the
[Plugin Authoring](../authoring-plugins.md) guide for the full workflow —
including failure reporting, native testing, and authoring without the
SDK (a plugin is any toolchain output satisfying the module contract
above; the `graphcal-plugin-abi` crate provides the manifest model and an
`embed_manifest` helper for build tooling).

## Trust: Lockfile Pins

For projects with a `graphcal.toml`, the **lockfile is the trust
boundary** for plugin code. `graphcal deps lock` scans the package's
sources for wasm plugin imports and records each file's SHA-256 in
`graphcal.lock`:

```toml
[[plugin]]
path = "plugins/fluids.wasm"
sha256 = "3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855c"
```

At load time the pin is enforced, hard errors and never prompts: a plugin
without a pin fails with P009 ("run `graphcal deps lock`"), and a plugin
whose bytes hash differently from the pin fails with P010. New or changed
plugin code can therefore only enter the project through a reviewable
`graphcal.lock` diff.

Two boundary cases:

- **Ad-hoc files** (no `graphcal.toml` anywhere above) load plugins
  unpinned — there is no lock regime to audit against, and the sandbox
  plus resource bounds still apply.
- **Dependency packages** need no `[[plugin]]` entries: a `.wasm` vendored
  inside a Git dependency is already covered by that package's pinned
  source tree hash. (In this phase, `import plugin "….wasm"` is itself
  restricted to the root package; dependency packages may still use
  embedder-provided plugins.)

## Failure Semantics

Extern functions can fail at runtime (a plugin reports a failure, traps,
runs out of fuel, or a host function returns an error). Failures follow
graphcal's per-node containment model:

- The failing node reports an evaluation error naming the alias, function,
  and plugin (e.g. ``extern function `inv.inverse` (plugin
  "plugins/inv.wasm") failed: division by zero``).
- Nodes that depend on it report `dependency failed`.
- Unrelated nodes keep evaluating. A failed call also discards the plugin
  instance, so a damaged plugin cannot corrupt later calls.

If a declared extern function is missing entirely — the plugin file is
absent, fails validation, or its manifest does not provide the function —
that is a **load-time** error reported on the declaration before
evaluation starts (P003, P005–P010 depending on the cause).

## The Host Function Registry

Embedders provide native implementations by injecting a
`HostFunctionRegistry` — a map from `(plugin path, function name)` to a
function of shape `fn(&[HostFnValue]) -> Result<HostFnValue, HostFnError>`,
where a `HostFnValue` is a scalar `f64` or a dense `f64` buffer (arrays in
index order; struct results as one slot per field). WASM plugins
register through the same interface (the `graphcal-plugin-host` crate
loads a project's vendored modules into the registry), so the evaluator
itself stays WASM-free:

```rust
use graphcal_eval::eval::compile_and_eval_from_project_with_host_fns;
use graphcal_eval::host_fns::demo_registry;
use graphcal_plugin_host::{PluginHost, register_project_plugins};

let mut registry = demo_registry();
register_project_plugins(&PluginHost::new(), &project, &mut registry);
let result = compile_and_eval_from_project_with_host_fns(&project, &overrides, &registry)?;
```

Native registry entries carry no manifest, so their declarations are
trusted as-is — appropriate for embedder-controlled functions. The CLI
and language server inject the built-in demo plugin (`"graphcal:demo"`:
`lerp`, `inverse`, `geometric_mean`, `normalize`, `dv_range`) so extern
declarations — including array and struct-returning ones — can be
exercised without any plugin file.
