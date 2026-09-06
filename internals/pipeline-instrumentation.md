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
| Imported body reference | Comparing a completed importing TIR's body address with its publishing module's canonical body |
| Unshared imported body | A reference check found different body addresses |
| Plan construction | Root execution-plan preparation and combined callable/instance scheduling |
| Constructor resolution | The expression evaluator's constructor generic-argument reconstruction |
| Presentation evaluation | Branch/match selector replay and expression-valued index projection arguments |

These are not allocation counters, exhaustive clone counts, wall-clock
benchmarks, or RSS measurements. In particular, nested runtime-value clones and
compiler-internal temporary rigid views are not counted as imported DAG copies.

## Initial Phase A observations (historical)

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

The old body-copy hooks were removed with their copying paths; retaining an
unrecorded event and asserting zero would not prove sharing. The current observer
instead compares actual body addresses at completed module publication, and
requires a positive number of reference checks for imported projects.

Positive runtime planning, constructor, and presentation counts still document
work to remove, not desirable behavior. Retain value/equivalence assertions when
changing their expectations in the remaining B–D work.

## Module ownership checkpoint

- A three-module chain checks **3** canonical/importer body references, with
  **0** unshared bodies. Chains of 2/4/8 modules check **1/6/28** references,
  respectively, also with zero mismatches. The current assembly indexes prior
  published modules; these reference counts are not a linear-time claim.
- A real four-file diamond checks **6** references and evaluates imported
  constants through both selective aliases and called module bodies.
- Every completed module in evaluator unit tests also checks that its project
  type-store address matches the session's shared store.
- Compiler-library diamond tests verify body **and runtime-unit** identity,
  local-only publication, duplicate-ID rejection in both insertion orders, and
  rejection of unit overlays without a defining body.
- An explicit equal-Static specialization test retains distinct runtime instance
  IDs, parameter values, SI results, and dynamic display scales.
- Required imported constants are distinguished from deferred runtime imports;
  missing pools/values or a category mismatch fail rather than disappearing or
  falling back to caller values.

Independent temporary mutations reintroduced each of: a duplicate shared ID,
a copied imported unit, a copied imported body, and a copied project type store.
Each failed its intended assertion, after which exact source was restored and
positive tests rerun. These are bounded mutation checks, not exhaustive clone,
allocation, or performance instrumentation. Callable planning and runtime owner
searches still remain to be removed in Phase B.

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
