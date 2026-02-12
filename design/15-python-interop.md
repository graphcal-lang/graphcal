# Python Interop

> PyO3 bindings for Python ecosystem access: parameter manipulation, sweeps, and Python-backed nodes.

## Status

**Decision level:** Conceptual. The API surface is sketched, but details around type mapping and error handling need specification.

## Summary

Cellgraph provides Python bindings via PyO3. Python can load graphs, read/write parameters (triggering reactive recomputation), and run parameter sweeps at Rust speed. Individual nodes can also be implemented in Python for complex logic requiring scipy, astropy, etc.

## Python API

```python
import cellgraph

g = cellgraph.load("mission.graph")

# Read/write params (triggers reactive recomputation)
g["mass_initial"] = 600.0
print(g["delta_v"])

# Get typed output as Pydantic model
budget = g.output("MissionBudget")

# Bulk parameter sweep (computed in Rust, returned as DataFrame)
results: pd.DataFrame = g.sweep({
    "mass_initial": np.linspace(400, 800, 100),
    "isp": [300, 350, 400],
})
```

## Python-Backed Nodes

When a node requires Python libraries:

```rust
#[python]
node trajectory: TrajectoryResult {
    from scipy.integrate import solve_ivp
    # ... complex ODE integration
    return TrajectoryResult(...)
}
```

The Rust engine calls back into Python for these nodes while keeping pure-arithmetic nodes in Rust.

## Open Questions

- **Type mapping:** How do Cellgraph types (dimensions, units, spaces) map to Python types? Are they Pydantic models? Plain dicts? Custom classes?
- **Unit handling in Python:** When Python reads `g["altitude"]`, does it get a plain float (in base SI units), or a value-with-unit object?
- **Error propagation:** If a `#[python]` node raises a Python exception, how does it propagate through the Cellgraph DAG?
- **Performance boundary:** Which operations happen in Rust vs Python? Is there a way to profile the Rust/Python boundary?
- **Dependency management:** How are Python dependencies for `#[python]` nodes managed? Is there a `requirements.txt` per project?
- **Async / parallel:** Can `#[python]` nodes run in parallel, or does the GIL serialize them?
- **Notebook integration:** Can Cellgraph graphs be used inside marimo notebooks?
- **Graph modification from Python:** Can Python add/remove nodes, or only read/write parameter values?
- **Sweep API details:** Does `g.sweep()` return a pandas DataFrame, a polars DataFrame, or something else? How are multi-dimensional sweep results shaped?
- **Security:** `#[python]` nodes execute arbitrary Python code. How is this sandboxed (if at all)?

## Dependencies on Other Aspects

- **Computation Model** ([01](./01-computation-model.md)): Python nodes are part of the DAG.
- **Algebraic Data Types** ([05](./05-algebraic-data-types.md)): Return types from Python nodes.
- **Dimensions** ([04](./04-dimensions-and-units.md)): How dimensions/units cross the Rust-Python boundary.
- **System Dynamics** ([11](./11-system-dynamics.md)): `#[python]` for complex ODE solvers.
