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

## Invocation-state ownership

`presentation_calls.rs` owns mutable per-evaluation call storage separately from
`execution_facts.rs`. Checked fact records already did not contain the mutable
store; the module split makes that ownership boundary explicit. Invocation
handles now carry a private allocation identity as well as their local ordinal:
a handle from another evaluation is rejected even at an identical static call
site and ordinal. Tests also cover repeated-call separation, shared read access,
wrong call sites, and identity exhaustion without partial publication.

This does **not** remove whole-call environment retention or presentation replay;
those remain Phase D work, and the positive replay counter/defect fixture stays.

## Bare-Wasm numerical boundary regression

`just wasm-test` also exercises determinant exponent cancellation, the tiny
cancellation mean, signed complex subnormals, signed zero, and contained overflow
through the actual JavaScript request/result boundary. It checks SI number bits,
not rendered strings. This is Node-hosted Wasm coverage, not a full-browser/UI
claim or proof of arbitrary numerical conditioning.
