# System Dynamics (Vensim Replacement)

> Temporal simulation via `scan` over time axes, not special stock/flow keywords.

## Status

**Decision level:** Mostly settled in concept. The "pattern, not keyword" approach is chosen. Integration quality and solver API need refinement. **Not yet implemented** — the core primitives (`index`, `T[I]`, `for`, `scan`, aggregations) are implemented in Phase 5, but a numeric time axis (as opposed to a named label set) and the `integrate` function for higher-quality ODE solvers are not yet available.

## Summary

Rather than introducing first-class `stock` and `flow` keywords, system dynamics is expressed as a **pattern** using existing primitives: an index as a time axis, struct state, and `scan` for integration. This keeps the language surface area small.

## What Exists Today

Phase 5 provides the building blocks:

```gcl
index Maneuver = { Departure, Correction, Insertion }

param delta_v: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km/s,
    Maneuver::Correction: 0.12 km/s,
    Maneuver::Insertion: 1.83 km/s,
};

// scan: cumulative delta-v
node cumulative_dv: Velocity[Maneuver] = scan(@delta_v, 0.0 m/s, |acc, val| acc + val);

// aggregation
node total_dv: Velocity = sum(for m: Maneuver { @delta_v[m] });
```

The `index` keyword defines a finite set of named labels, `T[I]` gives an indexed value, `scan` accumulates over the index, and `for`/`sum`/`max`/`min`/`mean` provide comprehension and aggregation. These already enable discrete-step simulation over a fixed set of labels.

## What's Missing for System Dynamics

System dynamics requires a **numeric time axis** — not a finite set of named labels, but a range of numeric values with a known step size. This is not yet implemented. The proposed syntax uses `index` with a `range()` expression:

```gcl
// Proposed: numeric index with range (not yet implemented)
index TimeStep = range(0.0 s, 200.0 s, step: 0.1 s);
```

This would generate an index with numeric labels rather than named labels, and would make the step size (`dt`) available for integration calculations.

## Core Pattern

A "stock" is a `scan` (accumulation) over a time axis. A "flow" is an expression inside the scan body. Using the proposed numeric index:

```gcl
// Proposed syntax (not yet implemented)
index TimeStep = range(0.0 s, 200.0 s, step: 0.1 s);

type SIRState { S: f64, I: f64, R: f64 }

param beta: Dimensionless = 0.3;
param gamma: Dimensionless = 0.1;
param N: f64 = 10000.0;

node sir: SIRState[TimeStep] = scan(
    SIRState { S: @N - 1.0, I: 1.0, R: 0.0 },  // initial value can reference graph nodes
    |prev_t, t| {
        let dt = t - prev_t;
        let infection = @beta * @sir[prev_t].S * @sir[prev_t].I / @N;
        let recovery = @gamma * @sir[prev_t].I;
        SIRState {
            S: @sir[prev_t].S - infection * dt,
            I: @sir[prev_t].I + (infection - recovery) * dt,
            R: @sir[prev_t].R + recovery * dt,
        }
    }
);
```

### Initial values from graph nodes

The initial value for `scan` can reference other graph nodes via `@`. This is a natural extension — the initial state is just an expression evaluated before the first time step:

```gcl
node sir: SIRState[TimeStep] = scan(
    SIRState { S: @population - 1.0, I: 1.0, R: 0.0 },  // @population is a param or node
    |prev_t, t| { ... }
);
```

### Accessing other indexed nodes in the evolution closure

The evolution closure should be able to access other nodes that share the same `TimeStep` index. This enables flow variables, external forcing functions, and coupling between dynamic quantities:

```gcl
// A flow variable indexed by TimeStep
node launch_rate: Mass[TimeStep] = for t: TimeStep {
    if t < 5.0 { 10 ton } else { 20 ton }
};

node supply: SupplyState[TimeStep] = scan(
    SupplyState { on_hand: @initial_stock, total_launched: 0 ton },
    |prev_t, t| {
        let dt = t - prev_t;
        let resupply = @launch_rate[t] * dt;       // access flow at current time step
        let consumed = @consumption_rate * dt;
        SupplyState {
            on_hand: @supply[prev_t].on_hand + resupply - consumed,
            total_launched: @supply[prev_t].total_launched + resupply,
        }
    }
);
```

The closure receives `(prev_t, t)` — the previous and current time step indices. This gives access to:

- `t - prev_t` — the time step size (`dt`)
- `@node_name[prev_t]` — the current node's value at the previous time step
- `@other[t]` — another node's value at the current time step
- `@other[prev_t]` — another node's value at the previous time step

Note that `@node_name[t]` (the current node at the current time step) would be a self-loop and is a compile error.

### Each (node, time step) pair is a DAG node

To make this work, the evaluation model treats each `(node, t)` pair as a separate node in the DAG, not the entire `node[T]` array as one node. This enables fine-grained dependency tracking:

- `(sir, t=3)` depends on `(sir, t=2)` — the previous step of the same scan
- `(sir, t=3)` depends on `(launch_rate, t=3)` — another node at the same step
- `(sir, t=3)` depends on `(other_state, t=2)` — another node at the previous step

A cycle at the `(node, t)` level (e.g., `(A, t=3)` depends on `(B, t=3)` which depends on `(A, t=3)`) is a compile error. But `(A, t=3)` depending on `(B, t=2)` and `(B, t=3)` depending on `(A, t=2)` is valid — both use already-computed values.

## Co-Evolved State

Multiple stocks that reference each other must be bundled in a single struct and evolved together:

```gcl
type SupplyState { on_hand: Mass, total_launched: Mass }

node supply: SupplyState[TimeStep] = scan(
    SupplyState { on_hand: @initial_stock, total_launched: 0 ton },
    |prev_t, t| {
        let dt = t - prev_t;
        let resupply = @launch_rate[t] * dt;
        let consumed = @consumption_rate * dt;
        SupplyState {
            on_hand: @supply[prev_t].on_hand + resupply - consumed,
            total_launched: @supply[prev_t].total_launched + resupply,
        }
    }
);
```

## Time Index Sharing and Pure Evolution Functions

### Time index sharing across files

Two dynamic nodes can only interact (e.g., `@other[t]`) if they share the same time index. This is enforced by the type system — `Mass[T1]` and `Mass[T2]` are distinct types. Since evolution closures commonly access other dynamic nodes at the current time step (e.g., flow variables, coupled quantities), sharing the time index across files is crucial for multi-file dynamic models.

To share a time index across files, use the standard `use` import:

```gcl
// time.gcl
index T = range(0.0 s, 100.0 s, step: 0.1 s);
```

```gcl
// epidemic.gcl
use "./time.gcl" { T };
node sir: SIRState[T] = scan(...);
```

```gcl
// logistics.gcl
use "./time.gcl" { T };
node supply: SupplyState[T] = scan(...);
// Can access @sir[t] because both use the same T
```

No global time index is needed — explicit imports are consistent with graphcal's "explicitness over implicitness" philosophy. However, this does mean that all coupled dynamic models in a project must agree on a shared time index, which the user must coordinate.

### Pure evolution functions for time-index-independent logic

When the evolution logic does _not_ need to access other dynamic nodes at `t` — i.e., it only uses the previous state, `dt`, and scalar parameters — it can be factored into a pure `fn`. This makes it reusable across different time resolutions:

```gcl
fn evolve_sir(state: SIRState, dt: Time, beta: f64, gamma: f64, N: f64) -> SIRState = {
    let infection = beta * state.S * state.I / N;
    let recovery = gamma * state.I;
    SIRState {
        S: state.S - infection * dt,
        I: state.I + (infection - recovery) * dt,
        R: state.R + recovery * dt,
    }
};

// Apply with different time resolutions
index T_coarse = range(0.0 s, 100.0 s, step: 0.1 s);
index T_fine = range(0.0 s, 100.0 s, step: 0.01 s);

node sir_coarse: SIRState[T_coarse] = scan(
    @initial_state,
    |prev_t, t| evolve_sir(@sir_coarse[prev_t], t - prev_t, @beta, @gamma, @N)
);

node sir_fine: SIRState[T_fine] = scan(
    @initial_state,
    |prev_t, t| evolve_sir(@sir_fine[prev_t], t - prev_t, @beta, @gamma, @N)
);
```

This pattern fits naturally with graphcal's `fn` purity — the evolution function is pure (no `@` references), and the `scan` call site binds it to a specific time index and supplies the graph parameters.

However, this pattern is limited to self-contained dynamics. When the closure references `@launch_rate[t]` or `@other_state[t]`, it is tied to a specific time index and cannot be factored into a pure function.

## Higher-Quality Integration

For simple cases, `scan` gives explicit Euler integration. For serious work, a proposed `integrate` function would wrap a proper ODE solver:

```gcl
// Proposed (not yet implemented)
node state: State[TimeStep] = integrate(
    init,
    method: RK4,
    |state, t| {
        // Returns derivatives, not next state
        State { S: -infection, I: infection - recovery, R: recovery }
    }
);
```

## Mixed Static + Dynamic

Static parametric calculations and dynamic simulation coexist in one file:

```gcl
// Static section (works today)
param dry_mass: Mass = 1200 kg;
node fuel_mass: Mass = @dry_mass * (exp(@total_dv / @v_exhaust) - 1.0);

// Dynamic section (proposed)
index Year = range(0 yr, 10 yr, step: 0.25 yr);
node supply: SupplyState[Year] = scan(...);

// Static references dynamic (proposed)
node supply_margin: Mass = min(for y: Year { @supply[y].on_hand });
```

## Vensim Concept Mapping

| Vensim | Graphcal | Primitive |
| --- | --- | --- |
| Auxiliary variable | `node` | Same |
| Constant | `param` or `const` | Same |
| Stock (Level) | `scan` field in struct | scan over time index |
| Flow (Rate) | Expression in scan body | Math |
| Subscript | `index` | Indexed values `T[I]` |
| Lookup table | Indexed value with interpolation | `lookup()` (proposed) |
| TIME STEP | `range(0 s, 200 s, step: 0.1 s)` | Numeric index with units (proposed) |
| SyntheSim | Live view + param sliders | Auto-rendered (proposed) |
| Causal loop diagram | Dependency graph | Auto from AST |

## Known Limitations

### False cycle detection with cross-scan dependencies

Since each `(node, t)` pair is a separate DAG node, many cross-scan dependencies are handled correctly. For example:

- `(A, t=3)` depends on `(B, t=2)` and `(B, t=3)` depends on `(A, t=2)` — **valid**, no cycle at the `(node, t)` level.
- `(B, t=0)` depends on `(A, t=last)` — **valid**, `A` is fully evaluated first, then `B` starts.

However, there are cases where the `(node, t)` DAG model is still conservative. Consider two dynamic quantities `A` and `B` that both scan over `TimeStep`, where:

- `A`'s evolution closure references a scalar derived from `B` (e.g., `sum(B)` or `B[last]`)
- `B`'s evolution closure references a scalar derived from `A`

At the `(node, t)` level, `(A, t=0)` depends on `B[last]`, which depends on all `(B, t)`, which in turn depend on all `(A, t)`. This creates a cycle in the DAG even though there might be a valid sequential evaluation order (e.g., if we could prove `A` doesn't actually need `B`'s final value until after `A` is fully computed).

This is an accepted limitation: graphcal's cycle detection operates at the `(node, t)` level, which catches most valid cases but may reject some theoretically acyclic computations that require reasoning about the full temporal structure. Users can work around this by sequencing their simulations explicitly (e.g., splitting into separate graphs or introducing intermediate scalar nodes).

## Settled Design Decisions

- **Numeric index syntax:** The proposed `index TimeStep = range(...)` reuses the existing `index` keyword. Nodes indexed by different time indexes cannot interact directly (consistent with how `index` already works). This is mitigated by sharing time indexes via `use` imports and factoring evolution logic into pure `fn` functions.
- **Step size access (`dt`):** The closure takes `(prev_t, t)`, so `dt = t - prev_t`. The previous state is accessed via `@node_name[prev_t]`. For fixed-step indexes `dt` is constant, but the formulation naturally supports variable step sizes.
- **Units on time axis:** The time axis must carry a dimension (e.g., `range(0.0 s, 200.0 s, step: 0.1 s)`). This makes `dt = t - prev_t` dimensionally typed, ensuring that expressions like `rate * dt` are dimension-checked. Consistent with graphcal's "no implicit dimensionless" philosophy.
- **`integrate` API:** The `integrate` function takes a closure that returns derivatives, not next state. This is the right abstraction — the solver handles the stepping.
- **Multiple time axes:** A project can have multiple independent time indexes (e.g., one for orbital propagation at 0.1s steps, another for logistics at quarterly steps). However, they cannot be mixed — a `scan` closure over index `T1` cannot access nodes indexed by `T2`. This is natural and consistent with how `index` already works.
- **Steady-state detection:** The runtime does not detect or stop at steady state. The simulation always runs for the full range of the time index. Users who need early termination can check the results after evaluation.
- **Dimensional consistency in derivatives:** In the `integrate` form, the compiler checks that the product of the derivative's dimension and the time index's dimension equals the state's dimension. For example, if the state has dimension `D` and the time axis has dimension `Time`, the derivative must have dimension `D / Time`.
- **Adaptive step size:** Out of scope for the initial design. The `range(0 s, 200 s, step: 0.1 s)` syntax fixes the step size.

## Events and Discontinuities

Discrete events (e.g., staging, contact loss, mode switches) can be expressed as conditional expressions inside the `scan` closure:

```gcl
node rocket: RocketState[T] = scan(
    @initial_state,
    |prev_t, t| {
        let dt = t - prev_t;
        // Staging event at t = 120 s
        let mass = if t >= 120 s {
            @rocket[prev_t].mass - @stage1_dry_mass
        } else {
            @rocket[prev_t].mass
        };
        let thrust = if t >= 120 s { @stage2_thrust } else { @stage1_thrust };
        RocketState {
            mass: mass - thrust / @v_exhaust * dt,
            velocity: @rocket[prev_t].velocity + thrust / mass * dt,
        }
    }
);
```

This approach has limitations:

- **No precise event timing:** Events fire at the nearest time step boundary, not at the exact moment the condition becomes true. For most engineering analysis with sufficiently small time steps, this is acceptable.
- **No event detection:** The user must know *when* the event occurs (or express the condition explicitly). Finding the exact time when a continuous variable crosses a threshold (e.g., "when altitude drops below 100 km") requires checking the condition at every step, and the crossing is resolved only to the step size precision.
- **No Vensim-style PULSE/STEP built-ins:** These are just syntactic sugar for conditionals and can be implemented as pure functions if needed.

## Dependencies on Other Aspects

- **Indexes** ([07](./07-indexes.md)): System dynamics extends the index concept to numeric ranges.
- **Computation Model** ([01](./01-computation-model.md)): Dynamic nodes are part of the DAG.
- **Algebraic Data Types** ([05](./05-algebraic-data-types.md)): State structs for co-evolved stocks.
- **Dimensions** ([04](./04-dimensions-and-units.md)): Time axis has a time dimension; derivatives need dimensional analysis.
- **Live View** ([13](./13-live-view.md)): Time series auto-render as plots.
