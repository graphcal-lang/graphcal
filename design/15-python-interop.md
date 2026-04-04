# Python Interop

> PyO3 bindings for Python ecosystem access: parameter manipulation, sweeps, and visualization.

## Status

**Decision level:** Conceptual. Not yet implemented. The API surface is sketched below; details around type mapping and error handling need specification.

## Summary

Graphcal provides Python bindings via PyO3. Python can load `.gcl` graphs, read/write `param` values (triggering reactive recomputation), and run parameter sweeps at Rust speed. This enables integration with the scientific Python ecosystem (numpy, scipy, matplotlib, marimo, etc.) while keeping the core evaluation in Rust.

## Current State

Graphcal is implemented as a Rust workspace with four crates (`graphcal-cli`, `graphcal-eval`, `graphcal-syntax`, `graphcal-lsp`). Phases 0-5 are implemented:

- Phase 0: Scalar graph (`param`/`node`/`const`, `@` sigil, `f64`)
- Phase 1: Dimensions & units (`dimension`, `unit`, `->` conversion)
- Phase 2: Structs & multi-line nodes (`type`, block bodies, `let` bindings)
- Phase 3: Pure functions (`fn`, `<D: Dim>` generics)
- Phase 4: Multi-file & namespaces (`import "./file.gcl" { name }`, `private`)
- Phase 5: Indexed values (`index`, `T[I]`, `for` comprehensions, `sum`/`scan`)

The CLI currently supports `graphcal eval <file>` with `--set 'name=expr'` for parameter overrides and `--format {text|json}` for output format. No Python bindings exist yet.

## Python API

```python
import graphcal

g = graphcal.load("rocket.gcl")

# Read/write params (triggers reactive recomputation)
g["dry_mass"] = 600.0  # in base SI units (kg)
print(g["delta_v"])

# Bulk parameter sweep (computed in Rust, returned as DataFrame)
results = g.sweep({
    "dry_mass": np.linspace(400, 800, 100),
    "isp": [300, 350, 400],
})
```

### Sweep API

The sweep API is the primary motivation for Python interop. Unlike the `.gclv` value file format ([19](./19-value-files-and-sampling.md)), which only supports independent sampling via `range(...)`, the Python API allows arbitrary parameter generation — including correlated parameters, custom distributions, and design-of-experiments methods:

```python
import graphcal
import numpy as np
from scipy.stats import qmc

g = graphcal.load("rocket.gcl")

# Independent sweep (Cartesian product)
results = g.sweep({
    "dry_mass": np.linspace(800, 2000, 50),
    "isp": [300, 350, 400],
})

# Sobol sequence for space-filling design
sampler = qmc.Sobol(d=2, scramble=True)
samples = sampler.random(n=1000)
results = g.sweep({
    "dry_mass": qmc.scale(samples[:, 0:1], 800, 2000).flatten(),
    "isp": qmc.scale(samples[:, 1:2], 250, 450).flatten(),
})

# Correlated parameters (impossible with .gclv range())
for mass, isp in correlated_samples:
    g["dry_mass"] = mass
    g["isp"] = isp
    print(g["delta_v"])
```

## Open Questions

- **Type mapping:** How do graphcal types (dimensions, units, spaces, structs, tagged unions) map to Python types? Options include Pydantic models, plain dicts, custom classes, or dataclasses. Structs and tagged unions ([05](./05-algebraic-data-types.md)) need a clear Python representation.
- **Unit handling in Python:** When Python reads `g["altitude"]`, does it get a plain float (in base SI units), or a value-with-unit object (e.g., via the `pint` library)?
- **Sweep return type:** Does `g.sweep()` return a pandas DataFrame, a polars DataFrame, or something else? How are multi-dimensional sweep results shaped?
- **Performance boundary:** Which operations happen in Rust vs Python? Is there a way to profile the Rust/Python boundary?
- **Async / parallel:** Can multiple sweep evaluations run in parallel via Rayon, or does the GIL serialize them?
- **Notebook integration:** Can graphcal graphs be used inside marimo notebooks with reactive updates?
- **Graph modification from Python:** Can Python add/remove nodes, or only read/write parameter values?
- **Error propagation:** How do graphcal evaluation errors (dimension mismatches, missing params) surface in Python? As Python exceptions with diagnostic info?
- **Loading `.gclv` files from Python:** Can the Python API load a `.gclv` file ([19](./19-value-files-and-sampling.md)) as a parameter set, or are `.gclv` files only for the CLI?

## Dependencies on Other Aspects

- **Computation Model** ([01](./01-computation-model.md)): Python reads/writes `param` values and reads `node` results.
- **Algebraic Data Types** ([05](./05-algebraic-data-types.md)): Structs and tagged unions need Python representations.
- **Dimensions** ([04](./04-dimensions-and-units.md)): How dimensions/units cross the Rust-Python boundary.
- **Value Files** ([19](./19-value-files-and-sampling.md)): `.gclv` handles independent sampling; Python handles correlated and complex sampling.
