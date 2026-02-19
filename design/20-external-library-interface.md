# External Library Interface

> Design exploration: bridging Graphcal's typed, unit-aware world with external libraries where types and dimensions are not enforced.

## Status

**Decision level:** Exploration. This document surveys the design space and analyzes tradeoffs. No decisions have been made.

## The Problem

Graphcal enforces dimensional correctness at compile time. External libraries (Python, C, etc.) do not. When values cross this boundary, dimensional information must be stripped on the way out and reconstructed on the way in. The reconstruction is an **unverifiable trust assertion** -- the compiler cannot check that a Python function actually returns meters per second when it claims to.

This is exactly the Mars Climate Orbiter problem: two systems exchanging numeric values with an implicit assumption about units. The question is how to make this assumption as explicit, auditable, and hard to get wrong as possible.

## Two Directions

There are two fundamentally different interop directions, and they need different designs:

| Direction | Who is the outer loop? | Example |
| --- | --- | --- |
| **Outward:** Python calls Graphcal | Python | `g = graphcal.load("rocket.gcl"); g["dry_mass"] = 600.0` |
| **Inward:** Graphcal calls Python | Graphcal | A node's value depends on calling scipy, astropy, or a custom Python module |

[Design doc 15](./15-python-interop.md) covers the **outward** direction. This document focuses primarily on the **inward** direction (Graphcal calling external code), but also refines the outward direction where relevant.

### Why both directions matter

The outward direction (Python drives Graphcal) is conceptually simple: Graphcal is a black box that takes SI floats and returns SI floats. The type system is intact inside the box.

The inward direction (Graphcal calls Python) is where the hard design questions live. A Graphcal calculation that depends on a Python library for orbital mechanics, atmospheric models, material properties, or optimization introduces an **untyped hole** in an otherwise fully typed computation graph.

## Scope: Python First

While the principles apply to any external language, this document focuses on Python for practical reasons:

- Python has the richest scientific computing ecosystem (numpy, scipy, astropy, poliastro, CoolProp, etc.)
- PyO3 provides mature Rust-Python bindings
- The engineering user base overwhelmingly uses Python

The interface design should not preclude future extension to other languages, but Python-specific affordances (type hints, Pint integration, numpy arrays) are acceptable.

## Use Cases for Inward Calls (Graphcal → Python)

Before exploring options, here are the concrete scenarios that motivate this:

### 1. Pure computation

Call a Python function that takes numbers, returns numbers. No state.

```
# Python side
def tsiolkovsky_with_losses(ve, m0, mf, gravity_loss, drag_loss):
    ideal_dv = ve * math.log(m0 / mf)
    return ideal_dv - gravity_loss - drag_loss
```

### 2. Library lookup

Query material properties, atmospheric models, thermodynamic data from Python packages.

```
# Python side (using CoolProp)
def water_density(temperature_K, pressure_Pa):
    return CoolProp.CoolProp.PropsSI('D', 'T', temperature_K, 'P', pressure_Pa, 'Water')
```

### 3. Numerical methods

Use scipy for optimization, root finding, ODE integration -- things Graphcal cannot express natively.

```
# Python side
def optimal_transfer_angle(mu, r1, r2):
    result = scipy.optimize.minimize_scalar(
        lambda theta: transfer_cost(mu, r1, r2, theta),
        bounds=(0, 2 * math.pi),
        method='bounded'
    )
    return result.x  # angle in radians
```

### 4. Data from external sources

Load constants, lookup tables, or datasets from files, databases, or APIs via Python.

```
# Python side
def get_planet_mu(planet_name):
    return astropy.constants.__dict__[planet_name].value
```

## Prior Art

### How other typed languages handle this

| System | Boundary mechanism | Trust model |
| --- | --- | --- |
| F* (`assume val`) | Type signature declared in verified language; implementation provided externally | Explicit axiom -- programmer asserts correctness |
| Idris (`%foreign`) | Primitive type allowlist + `PrimIO` for effects | Compiler verifies type is marshallable; signature trusted |
| Haskell (`foreign import`) | Marshalling types (`CDouble`, `CInt`) + IO monad | Programmer asserts signature matches C header |
| Rust/PyO3 | `FromPyObject`/`IntoPyObject` traits + runtime type checking | Compile-time Rust safety + runtime Python type checks |
| TypeScript (`.d.ts`) | Declaration files for untyped JavaScript | Community-maintained; intentionally unsound |
| Cython (`cdef extern`) | Three-tier function visibility (`def`/`cdef`/`cpdef`) | C types cannot leak to Python-facing signatures |
| Julia (`ccall`) | Inline type annotations | Programmer responsible; no verification |
| Numbat | No user-facing FFI; built-ins are internal Rust functions | Fully trusted (internal only) |

### Graphcal's existing trust boundary pattern: `as` cast

Graphcal already has a mechanism for explicit trust assertions: the `as` cast in the spaces system ([doc 06](./06-spaces.md)):

```gcl
node v_eci = @v_body as Vec3<Length, ECI>;
// Programmer asserts: "I know this value is in the ECI frame"
```

Properties of `as`:
- **Per-value** (minimal blast radius)
- **Auditable** (grep-able)
- **Explicit** (visible at the usage site)
- **Structural** (compiler verifies field types match; only phantom parameters change)

An external function call is analogous: the programmer asserts "I trust that this Python function returns a value with this dimension in SI base units." The design should make this assertion equally explicit and auditable.

## The Design Space

### Option A: `extern fn` -- Typed Function Declarations

Declare the type signature in Graphcal; provide the implementation in Python.

```gcl
extern fn water_density(
    temperature: Temperature,
    pressure: Pressure,
) -> Mass / Length^3
    python("coolprop_bridge.water_density");

// Usage -- just like any other function
node rho = water_density(@coolant_temp, @chamber_pressure);
```

The Graphcal compiler:
1. Converts each argument from its internal SI representation to a plain `f64`
2. Calls the Python function via PyO3
3. Takes the returned `f64` and wraps it with the declared return dimension
4. Steps 2 and 3 are the trust boundary -- the compiler cannot verify them

**Python side:**

```python
# coolprop_bridge.py
import CoolProp.CoolProp as CP

def water_density(temperature_K, pressure_Pa):
    """Args and return value are in SI base units."""
    return CP.PropsSI('D', 'T', temperature_K, 'P', pressure_Pa, 'Water')
```

**Purity assertion:** `extern fn` is treated as pure by Graphcal's evaluation model (same inputs → same outputs). The programmer asserts this. If the Python function has side effects or non-determinism, the caching/memoization model may produce incorrect results. This is analogous to Haskell's `unsafePerformIO` -- the programmer takes responsibility.

**Compound types:** Structs, indexed values, and tagged unions need a marshalling convention:

```gcl
extern fn optimize_trajectory(
    initial_orbit: Orbit,       // struct → Python dict
    target_orbit: Orbit,
    method: Str,
) -> TransferResult             // Python dict → struct (validated)
    python("trajectory.optimize");
```

| Graphcal type | Python representation |
| --- | --- |
| `f64` (with dimension) | `float` (SI base units) |
| `i64` | `int` |
| `bool` | `bool` |
| `Str` | `str` |
| `Option<T>` | `T \| None` |
| Struct | `dict` with string keys (field values recursively converted) |
| Indexed `T[I]` | `dict` with variant name string keys |
| Tagged union | Not supported at boundary (too complex for initial version) |

**Analysis:**

| Property | Assessment |
| --- | --- |
| Explicitness | High -- `extern` keyword marks the trust boundary |
| Auditability | High -- grep for `extern fn` to find all external calls |
| Safety | Moderate -- type signature is checked on the Graphcal side; Python side is trusted |
| Ergonomics | Good -- looks like a normal function call at the usage site |
| Purity model | Fits naturally -- `extern fn` follows `fn` rules (no `@` in body, because there is no body) |
| Compound types | Workable but needs marshalling specification |
| Error handling | Needs design -- what happens when Python raises an exception? |

### Option B: `extern` Node Blocks -- Inline Python with Typed Boundary

Embed Python code directly in a node declaration. The boundary is at the node level, not the function level.

```gcl
node drag_force: Force = extern python {
    import aerodynamics
    cd = aerodynamics.lookup_cd(mach=$mach, alpha=$alpha_deg)
    return 0.5 * $density * $velocity**2 * $area * cd
};
```

Where `$name` references are resolved to Graphcal values (injected as SI floats) and the return value is asserted to match the declared type annotation (`: Force`).

**Analysis:**

| Property | Assessment |
| --- | --- |
| Explicitness | High -- `extern python` block is visually distinct |
| Auditability | Moderate -- Python code is inline, mixed with Graphcal code |
| Safety | Same as Option A -- return type is an assertion |
| Ergonomics | Good for one-off calls; bad for reusable logic (code duplication) |
| Purity model | Awkward -- the node itself contains impure-looking code |
| Reusability | Poor -- can't call the same Python code from multiple nodes without duplication |
| Tooling | Hard -- LSP, formatter, syntax highlighting must handle two languages |

### Option C: External Module Declarations -- TypeScript `.d.ts` Style

Declare an entire Python module's interface in a Graphcal declaration file.

```gcl
// coolprop.gcl.d -- declaration file (no implementation)
extern module python("CoolProp.CoolProp") {
    fn PropsSI(
        output: Str,
        input1_name: Str, input1_value: f64,
        input2_name: Str, input2_value: f64,
        fluid: Str,
    ) -> f64;  // dimensionless -- caller must know what they asked for
}
```

```gcl
// usage.gcl
use "./coolprop.gcl.d" { PropsSI };

node density: Mass / Length^3 = PropsSI(
    "D", "T", @temperature → K, "P", @pressure → Pa, "Water"
);
```

**Analysis:**

This is worse than Option A for Graphcal's use case. The Python module's API is designed for Python, not for Graphcal's type system. Wrapping it 1:1 loses type information (the return type of `PropsSI` depends on the string argument `"D"`). A thin Graphcal-typed wrapper (Option A) is more useful because it can encode the specific dimension of each specific call.

### Option D: Protocol/Adapter on the Python Side

Define a Python-side protocol that external functions must implement to participate:

```python
import graphcal

@graphcal.extern(
    inputs={"temperature": "Temperature", "pressure": "Pressure"},
    output="Mass / Length^3",
)
def water_density(temperature, pressure):
    return CoolProp.PropsSI('D', 'T', temperature, 'P', pressure, 'Water')
```

Graphcal discovers the function and its type signature via Python introspection at load time.

**Analysis:**

| Property | Assessment |
| --- | --- |
| Explicitness | Moderate -- type info is on the Python side, not visible in `.gcl` files |
| Auditability | Poor from Graphcal's perspective -- must look at Python code to see the boundary |
| Safety | Same runtime trust level as other options |
| Ergonomics | Good for Python developers; bad for Graphcal developers |
| Discoverability | Poor -- Graphcal compiler can't check signatures without running Python |

This approach is problematic because **the type declarations live outside Graphcal's compilation model**. The Graphcal compiler cannot verify or even see them without starting a Python interpreter. This violates the principle that Graphcal's type system should be statically checkable.

### Option E: Subprocess/IPC Boundary -- Maximum Isolation

Graphcal never embeds Python. External calls go through a serialized IPC boundary.

```gcl
extern fn water_density(
    temperature: Temperature,
    pressure: Pressure,
) -> Mass / Length^3
    command("python3 coolprop_bridge.py");
    // stdin: JSON {"temperature": 373.15, "pressure": 101325.0}
    // stdout: JSON {"result": 997.05}
```

**Analysis:**

Maximum isolation and language-independence, but extremely poor performance (process startup per call) and ergonomics. Only viable for batch operations, not per-node evaluation. Not appropriate as the primary mechanism, but could be a useful fallback for non-Python languages.

## Recommendation: Option A (`extern fn`) as Primary Mechanism

Option A (`extern fn`) is the best fit for Graphcal's design philosophy:

1. **Explicit trust boundary.** The `extern` keyword is a clear signal, analogous to `as` cast for spaces. Every external call is declared, typed, and auditable.

2. **Fits the existing function model.** `extern fn` follows the same rules as `fn` -- no `@` references (there's no body), dimension generics work, it can be imported/exported via `use`.

3. **Separation of concerns.** The Graphcal file declares *what* the function does (its type signature). The Python file provides *how*. This is the same separation as F*'s `assume val` and Haskell's `foreign import`.

4. **Static checking.** The Graphcal compiler can check all call sites against the declared signature without starting Python. Dimension mismatches at call sites are caught at compile time.

5. **Composable.** An `extern fn` can be called from pure `fn` bodies (since it looks like any other function), from `node` expressions, or from other contexts.

## Design Details for `extern fn`

### Syntax

```gcl
extern fn <name>(<params>) -> <return_type>
    python(<module_path>);
```

- `extern` marks this as an external function with no Graphcal body
- The parameter and return types follow existing Graphcal type syntax
- `python(...)` specifies the Python module and function path
- The Python function name defaults to `<name>` (same as the Graphcal name)
- An explicit mapping can override: `python("module.submodule.different_name")`

### Convention: All Values Cross in SI Base Units

This is the single most important design decision. Every numeric value crossing the boundary is in SI base units:

| Crossing | Direction | Value |
| --- | --- | --- |
| `400 km` → Python | Graphcal → Python | `400000.0` (meters) |
| Python returns `9.81` | Python → Graphcal | `9.81` (m/s^2, if declared `Acceleration`) |

**No implicit unit conversion.** The Python function must expect and return SI base units. This is the convention, documented and enforced by culture (not by the compiler). The alternative -- allowing unit specification at the boundary -- adds complexity without adding safety (the compiler still can't verify Python's behavior).

**Why SI base units and not user-specified units?** Because it eliminates an entire class of errors. If the convention were "units specified per parameter," then two versions of the same Python function might disagree on whether altitude is in meters or kilometers. With a single universal convention (SI base), there is exactly one correct interpretation. This mirrors how Graphcal stores values internally.

### Dimension Generics

`extern fn` should support dimension generics, same as regular `fn`:

```gcl
extern fn clamp<D: Dim>(value: D, low: D, high: D) -> D
    python("gcl_utils.clamp");
```

The Graphcal compiler monomorphizes the dimension at the call site. The Python function receives plain floats regardless.

### Index Generics

```gcl
extern fn normalize<D: Dim, I: Index>(values: D[I]) -> f64[I]
    python("gcl_utils.normalize");
```

Indexed values are marshalled as `dict[str, float]` with variant names as keys.

### Struct Parameters and Return Values

```gcl
type Orbit {
    semi_major_axis: Length,
    eccentricity: f64,
    inclination: Angle,
}

extern fn propagate(orbit: Orbit, dt: Time) -> Orbit
    python("orbital.propagate");
```

Marshalling:
- **Graphcal → Python:** `Orbit` becomes `{"semi_major_axis": 6878000.0, "eccentricity": 0.001, "inclination": 0.9}`
- **Python → Graphcal:** The returned `dict` is validated against the struct definition. Missing fields, extra fields, or wrong types produce runtime errors with clear diagnostics.

### Error Handling

When a Python function raises an exception:

1. The exception is caught at the PyO3 boundary
2. It becomes a Graphcal runtime error attached to the calling node
3. The error propagates downstream as an error value (like Excel's `#ERROR!`)
4. The error message includes the Python traceback for debugging

```
error[E002]: external function error
  ┌─ rocket.gcl:15:14
  │
15 │ node rho = water_density(@temp, @pressure);
  │            ^^^^^^^^^^^^^^ Python function raised an exception
  │
  = Python error: CoolProp.CoolProp.PropsSI: Pressure out of range
  = Traceback:
      File "coolprop_bridge.py", line 5, in water_density
        return CP.PropsSI('D', 'T', temperature_K, 'P', pressure_Pa, 'Water')
```

### Purity and Caching

`extern fn` is treated as pure for evaluation purposes. The memoization system caches results based on input values. If the Python function is actually impure (reads files, calls APIs, uses randomness), the programmer must understand that:

1. Results may be cached and reused
2. The function may not be called on every evaluation cycle
3. Call order is not guaranteed

If a function is genuinely impure (e.g., reads a sensor), a future `extern impure fn` variant could opt out of caching. This is deferred.

### Module Resolution

The `python(...)` path follows Python's standard module resolution:

```gcl
extern fn f(...) -> ... python("my_package.my_module.f");
//                              ^^^^^^^^^^^^^^^^^^^^^^^^
//                              Equivalent to: from my_package.my_module import f
```

Graphcal does not manage Python dependencies. The user is responsible for ensuring the Python module is importable (e.g., via `pip install`, `PYTHONPATH`, or a virtual environment). This is documented, not enforced.

### When is Python initialized?

The Python interpreter is started lazily -- only when the first `extern fn` is actually called during evaluation. If a `.gcl` file declares `extern fn` but never calls it (e.g., it's in an unused branch of an `if`), Python is never started. This keeps evaluation fast for purely Graphcal computations.

## The Outward Direction (Revisiting Doc 15)

The existing [doc 15](./15-python-interop.md) design for the outward direction (Python drives Graphcal) is sound. A few refinements based on this analysis:

### Unit Metadata

When Python reads values from a Graphcal graph, it should have access to dimension/unit information:

```python
g = graphcal.load("rocket.gcl")

# Option 1: Plain float (SI base units) -- simple, fast
value = g["altitude"]  # 400000.0 (meters)

# Option 2: Value with metadata
value = g.get("altitude")
value.si_value    # 400000.0
value.dimension   # "Length"
value.display_unit  # "km" (from the .gcl source)
value.in_unit("km")  # 400.0
```

The plain float access (`g["altitude"]`) should always return SI base units for consistency with the inward direction convention. The metadata-rich access (`g.get("altitude")`) provides additional information for debugging and display.

### Optional Pint Integration

For users who want unit safety on the Python side:

```python
g = graphcal.load("rocket.gcl", units="pint")
value = g["altitude"]  # pint.Quantity(400000.0, 'meter')
```

This is opt-in. The default is plain floats for simplicity and performance. Pint integration requires `pint` to be installed.

## Marshallable Type Allowlist

Not all Graphcal types can cross the boundary. The following types are marshallable:

| Graphcal type | Python type | Notes |
| --- | --- | --- |
| `f64` (any dimension) | `float` | Always SI base units |
| `i64` | `int` | |
| `bool` | `bool` | |
| `Str` | `str` | |
| `Option<T>` | `T \| None` | `T` must be marshallable |
| Struct | `dict[str, Any]` | Field values recursively marshalled; all in SI base units |
| `T[I]` (indexed) | `dict[str, T]` | Keys are variant names as strings |
| Tagged union | Not marshallable | Too complex for initial version; future work |

Types that are **not** marshallable:
- Phantom type parameters (compile-time only; no runtime representation)
- Function types (no higher-order functions across the boundary)
- Graphcal-internal types (dimensions, units themselves)

Attempting to use a non-marshallable type in an `extern fn` signature is a compile error.

## Spaces at the Boundary

Phantom type parameters (spaces) exist only at compile time. They have no runtime representation. At the boundary:

```gcl
extern fn rotate_eci_to_body(v: Vec3<Length, ECI>) -> Vec3<Length, Body>
    python("frame_transforms.eci_to_body");
```

Python receives `{"x": ..., "y": ..., "z": ...}` -- a plain dict with no frame information. The `ECI` and `Body` annotations exist only in Graphcal's type checker. The programmer asserts that the Python function correctly transforms from ECI to Body. This is the same trust model as `as` cast, but applied at the function boundary.

This is consistent with Sguaba's Rust model, where the phantom type parameter has no runtime cost.

## Comparison with Alternative Approaches

### Why not embedded Python (Option B)?

Inline Python blocks mix two languages in one file. This creates problems for:
- **Tooling:** The Graphcal formatter, LSP, and syntax highlighter must understand Python syntax inside `.gcl` files.
- **Testing:** Python code embedded in `.gcl` files cannot be independently tested with pytest.
- **Reuse:** The same Python logic used in multiple nodes must be duplicated.
- **Version control:** Python and Graphcal changes are interleaved in diffs.

`extern fn` keeps Python code in `.py` files where Python tooling works. The boundary is clean.

### Why not Python-side declarations (Option D)?

Type declarations belong in the language that enforces them. If declarations live in Python (via decorators), the Graphcal compiler cannot see them without running Python. This means:
- `graphcal check` cannot verify call sites without a Python environment
- IDE features (hover, autocomplete, go-to-definition) break for external functions
- The "source of truth" for the type boundary is split across two languages

`extern fn` keeps all type information in Graphcal, where the compiler can use it.

### Why not IPC (Option E)?

Process-level isolation is appropriate for batch operations (the sweep API in doc 15) but too expensive for per-node evaluation. A node that calls a Python function may be evaluated thousands of times during a parameter sweep. Process startup overhead would dominate.

PyO3 in-process calls are the right mechanism for per-node external functions. IPC could be offered as a future option for non-Python languages.

## Open Questions

### Critical

- **Dependency management:** How does the user specify which Python packages are required? A `requirements.txt`? A `[python]` section in a project config? Or is this entirely the user's responsibility?
- **Virtual environment handling:** Should Graphcal auto-detect and use a `.venv`? Or require the user to activate it before running `graphcal eval`?
- **Compile-time vs. load-time checking:** Should `graphcal check` verify that the Python module exists and has the expected function? This requires starting Python. Or should it only check the Graphcal-side types and defer Python resolution to evaluation time?
- **Return value validation:** Should Graphcal validate the structure of Python return values (e.g., check that a returned dict has the right keys for a struct)? Or trust blindly? Validation adds overhead but catches bugs earlier.

### Important

- **numpy array support:** Should `extern fn` support numpy arrays as parameters or return values? This would enable efficient data exchange for numerical methods. What Graphcal type maps to a numpy array?
- **Async/parallel:** Can `extern fn` calls be parallelized via Rayon? The GIL is a constraint. `asyncio` or `concurrent.futures` on the Python side could help.
- **Caching control:** Should `extern fn` support a `#[cache = false]` attribute for genuinely impure functions? Or is this a future concern?
- **Multiple backends:** The syntax `python(...)` implies other backends could exist: `c(...)`, `rust(...)`, `wasm(...)`. Should the syntax anticipate this, or is Python-only acceptable for now?

### Deferred

- **Tagged union marshalling:** How to represent Graphcal tagged unions in Python. Requires design work on pattern matching and variant representation.
- **Callback support:** Can Python call back into Graphcal during an `extern fn` call? Almost certainly not (re-entrancy into the evaluation engine), but should be explicitly prohibited.
- **Hot reloading:** Can Python modules be reloaded without restarting Graphcal? Useful during development.

## Dependencies on Other Aspects

- **Computation Model** ([01](./01-computation-model.md)): `extern fn` calls are pure from the DAG's perspective. Caching and incremental recomputation apply.
- **Pure Functions** ([12](./12-pure-functions.md)): `extern fn` extends the `fn` system. No `@` references (no body). Same calling conventions.
- **Dimensions & Units** ([04](./04-dimensions-and-units.md)): All values cross the boundary in SI base units. Dimension annotations on parameters and return types are checked at Graphcal call sites.
- **Algebraic Data Types** ([05](./05-algebraic-data-types.md)): Struct marshalling to/from Python dicts.
- **Spaces** ([06](./06-spaces.md)): Phantom type parameters are erased at the boundary. The programmer asserts frame correctness.
- **Indexes** ([07](./07-indexes.md)): Indexed values marshalled as `dict[str, T]`.
- **Python Interop** ([15](./15-python-interop.md)): This document complements doc 15. Doc 15 covers Python → Graphcal (outward); this covers Graphcal → Python (inward).
- **Error Messages** ([17](./17-error-messages.md)): Python exceptions must be surfaced as Graphcal diagnostics.
