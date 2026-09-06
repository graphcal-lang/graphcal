# Pipeline cost observations

`crates/graphcal-eval/src/pipeline_metrics.rs` observes selected expensive
boundaries in unit-test builds. Production observation is a no-op. Counters are
thread-local and measure deltas without resetting enclosing measurements.

Run the fixtures with:

```sh
cargo test --locked -p graphcal-eval --lib pipeline_cost_baseline -- --nocapture
```

| Event | Instrumented boundary |
| --- | --- |
| DAG body copy | Retaining a module's cloned DAG registry and cloning imported DAG bodies during merge |
| Plan construction | Root execution-plan preparation and combined callable/instance scheduling |
| Constructor resolution | The expression evaluator's constructor generic-argument reconstruction |
| Presentation evaluation | Branch/match selector replay and expression-valued index projection arguments |

These are not allocation counters, exhaustive clone counts, wall-clock
benchmarks, or RSS measurements. In particular, nested runtime-value clones and
compiler-internal temporary rigid views are not counted as imported DAG copies.

## Initial Phase A observations

- Three-module ordinary-import chain preparation: **6 body copies**, **1 plan**.
- Two-, four-, and eight-module chains: **2**, **12**, and **56** copies,
  respectively, with one plan each. These fixtures reproduce the `n(n-1)` copy
  count at the instrumented boundaries; they do not establish an RSS/time law.
- Single-file two-call fixture preparation: **0 imported body copies**, **1 plan**.
- Each prepared evaluation of that fixture: **2 plan constructions**, **1
  constructor resolution**, **3 presentation evaluations**. Two consecutive
  evaluations have identical counts.

A native-host selector fixture also reproduces a semantic presentation defect:
checking invokes no host function, but evaluation calls an alternating Boolean
selector **twice**. The SI result correctly retains the first branch's 1000 m,
while presentation selects the second branch's `m` label instead of the authored
first branch's `km`. Phase D must change this baseline to **one host call** and
`km` presentation; a native `Fn` signature does not establish purity.

Positive runtime counts document work to remove, not desirable behavior.
Phases B–D must change the corresponding fixture assertions to zero when
ownership, planning, checked constructor facts, and presentation evidence become
their authorities. Retain the value/equivalence assertions when changing cost
expectations.
