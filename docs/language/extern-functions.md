---
icon: material/power-plug
---

# Extern Functions (Plugins)

Extern functions let a graphcal project call scalar functions provided by
the *embedder* — the CLI, the language server, or a program embedding the
evaluation engine. They are the first phase of the plugin system: the
declaration surface, dimension checking, and evaluation path are final,
while the WebAssembly runtime that will back them arrives in a later
phase. Today, extern functions are implemented by a **host function
registry** of native functions injected by the embedder.

## Declaring a Plugin

An `import plugin` block declares which functions a plugin provides and,
crucially, their full dimensional signatures — in graphcal vocabulary, at
the import site:

```
import plugin "graphcal:demo" as demo {
    fn lerp<D>(a: D, b: D, t: Dimensionless) -> D;
    fn inverse<D>(x: D) -> D^-1;
    fn geometric_mean<D1, D2>(x: D1, y: D2) -> D1^(1/2) * D2^(1/2);
}
```

- The **path string** identifies the plugin. In this phase it has no
  filesystem meaning; it names the plugin in the embedder's host function
  registry. (`"graphcal:demo"` is the built-in demo plugin provided by the
  CLI and language server.)
- The **alias** (`as demo`) is mandatory. Extern functions are only
  callable qualified through it — `demo.lerp(...)` — never bare. This
  mirrors the explicitness of module imports and keeps the built-in
  function namespace closed.
- Each `fn` declares **named parameters** and a **result type**. Parameter
  and result types may be `Bool`, `Int`, or scalar dimension expressions.

Signatures are declared explicitly rather than inferred from the plugin:
the declaration in your source is the contract your project type-checks
against. When the WASM runtime lands, the declared signatures are verified
against the plugin's embedded manifest at load time.

## Dimension Variables

A signature may declare *dimension variables* in explicit angle-bracket
binders, making a function polymorphic over dimensions:

```
fn lerp<D>(a: D, b: D, t: Dimensionless) -> D;
```

At each call site, `D` binds to the actual argument dimension, every
other `D` parameter must match it, and the result dimension is computed
from the binding. Result types may combine several variables with
rational powers — full cross-variable dimension algebra:

```
fn geometric_mean<D1, D2>(x: D1, y: D2) -> D1^(1/2) * D2^(1/2);
```

```
node scale: Length = demo.geometric_mean(4.0 m, 9.0 m);   // = 6 m
```

One rule keeps checking decidable: **every dimension variable must first
appear as a bare parameter** (`x: D`) before it is used in a compound form
(`D^2`, `D1 * D2`) or in the result. A signature like `fn sq<D>(x: D^2) -> D`
is rejected — it would require solving for `D` rather than binding it.

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

## Failure Semantics

Extern functions can fail at runtime (a host function returns an error or
a non-finite value). Failures follow graphcal's per-node containment
model:

- The failing node reports an evaluation error naming the alias, function,
  and plugin (e.g. ``extern function `demo.inverse` (plugin
  "graphcal:demo") failed: division by zero``).
- Nodes that depend on it report `dependency failed`.
- Unrelated nodes keep evaluating.

If a declared extern function has no entry in the embedder's registry at
all, that is a **load-time** error (P003), reported on the declaration
before evaluation starts.

## The Host Function Registry

Embedders provide implementations by injecting a
`HostFunctionRegistry` — a map from `(plugin path, function name)` to a
native function of shape `fn(&[f64]) -> Result<f64, HostFnError>`:

```rust
use graphcal_eval::host_fns::HostFunctionRegistry;
use graphcal_eval::eval::compile_and_eval_from_project_with_host_fns;

let mut registry = HostFunctionRegistry::new();
registry.register(plugin_path, fn_name, |args| Ok(args[0].tanh()));
let result = compile_and_eval_from_project_with_host_fns(&project, &overrides, &registry)?;
```

Arguments arrive as raw `f64` values in SI base units (Int arguments are
converted exactly; Bool arguments become `1.0`/`0.0`), and the result is
converted back per the declared result kind. The registry interface is
deliberately shaped so the upcoming WASM runtime replaces its backend, not
its interface.

The CLI and language server inject the built-in demo plugin
(`"graphcal:demo"`: `lerp`, `inverse`, `geometric_mean`) so extern
declarations can be exercised end-to-end today.
