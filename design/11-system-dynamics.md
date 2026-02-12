# System Dynamics (Vensim Replacement)

> Temporal simulation via `scan` over time axes, not special stock/flow keywords.

## Status

**Decision level:** Mostly settled in concept. The "pattern, not keyword" approach is chosen. Integration quality and solver API need refinement.

## Summary

Rather than introducing first-class `stock` and `flow` keywords, system dynamics is expressed as a **pattern** using existing primitives: tables with a time axis, struct state, and `scan` for integration. This keeps the language surface area small.

## Core Pattern

A "stock" is a `scan` (accumulation) over a time axis. A "flow" is an expression inside the scan body.

```rust
table sim [t: range(0.0, 200.0, step: 0.1) : unitless] {}

type SIRState { S: f64, I: f64, R: f64 }

node sim.sir: SIRState = scan(
    SIRState { S: 9999.0, I: 1.0, R: 0.0 },
    |prev, row| {
        let infection = @beta * prev.S * prev.I / @N;
        let recovery = @gamma * prev.I;
        SIRState {
            S: prev.S - infection * row.dt,
            I: prev.I + (infection - recovery) * row.dt,
            R: prev.R + recovery * row.dt,
        }
    }
);
```

## Co-Evolved State

Multiple stocks that reference each other must be bundled in a single struct and evolved together:

```rust
type SupplyState { on_hand: Mass, total_launched: Mass }

node year.supply: SupplyState = scan(
    SupplyState { on_hand: 100 ton, total_launched: 0 ton },
    |prev, row| {
        let resupply = @launch_rate * row.dt;
        let consumed = @consumption_rate * row.dt;
        SupplyState {
            on_hand: prev.on_hand + resupply - consumed,
            total_launched: prev.total_launched + resupply,
        }
    }
);
```

## Higher-Quality Integration

For simple cases, `scan` gives explicit Euler integration. For serious work, an `integrate` function wraps a proper ODE solver:

```rust
node sim.state: State = integrate(
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

```rust
// Static section
param dry_mass = 1200 kg;
node fuel_mass = @dry_mass * (exp(@total_dv / @v_exhaust) - 1);

// Dynamic section
table year [t: range(0, 10, step: 0.25) : yr] {}
node year.supply: SupplyState = scan(...);

// Static references dynamic
node supply_margin: Mass = year.supply.on_hand.min();
```

## Vensim Concept Mapping

| Vensim | Cellgraph | Primitive |
| --- | --- | --- |
| Auxiliary variable | `node` | Same |
| Constant | `param` or `const` | Same |
| Stock (Level) | `scan` field in struct | scan over time axis |
| Flow (Rate) | Expression in scan body | Math |
| Subscript | Table dimension | N-dim table |
| Lookup table | Table with interpolation | `lookup()` |
| TIME STEP | `range(start, end, step:)` | Table axis |
| SyntheSim | Live view + param sliders | Auto-rendered |
| Causal loop diagram | Dependency graph | Auto from AST |

## Open Questions

- **`row.dt`:** Where does `row.dt` come from? Is it automatically available for time-axis tables? What is its type and dimension?
- **Adaptive step size:** The `range(0, 200, step: 0.1)` syntax fixes the step size. How would adaptive-step ODE solvers work where the step size varies?
- **Events / discontinuities:** How to handle discrete events during simulation (e.g., staging, contact loss)? Vensim has PULSE/STEP functions.
- **`integrate` API:** The `integrate` function returns derivatives rather than next state. Is this the right abstraction? How does it interact with the table axis?
- **Multiple time axes:** Can a project have multiple independent time axes (e.g., one for orbital propagation at 0.1s steps, another for logistics at quarterly steps)?
- **Initial conditions from graph:** Can the initial state of a `scan` reference graph nodes (e.g., `scan(SIRState { S: @population - 1, I: 1, R: 0 }, ...)`)?
- **Steady-state detection:** Should the runtime detect when a dynamic simulation has reached steady state and stop early?
- **Dimensional consistency in derivatives:** In the `integrate` form, derivatives have different dimensions than state values (e.g., `dS/dt` has dimension `1/Time`). How does the type system handle this?

## Dependencies on Other Aspects

- **Tables** ([10](./10-tables-and-autofill.md)): System dynamics uses table `scan`.
- **Computation Model** ([01](./01-computation-model.md)): Dynamic nodes are part of the DAG.
- **Algebraic Data Types** ([05](./05-algebraic-data-types.md)): State structs for co-evolved stocks.
- **Dimensions** ([04](./04-dimensions-and-units.md)): Time axis has a time dimension; derivatives need dimensional analysis.
- **Live View** ([13](./13-live-view.md)): Time series auto-render as plots.
