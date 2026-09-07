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
| Plan construction | Construction of each retained callable/instance-closure plan during preparation |
| Schedule construction | Actual per-body runtime graph construction and combined instance-closure scheduling |
| Imported source resolution | Checking's imported-constant body lookup; runtime uses retained pool references |
| Frame execution | Entry into the shared root/call schedule machine |
| Constructor fact consumption | Reading the retained checked constructor application |
| Presentation evaluation | Resolving a selected display-only unit request in its live owning frame (never selector replay) |
| Call frame value nodes | Actual computational value-tree nodes in a completed call, before dropping its frame |
| Call output evidence nodes | Actual selected evidence-tree nodes returned by that call |
| Presentation evidence copy node | Every node actually copied by `PresentationInstance::clone` |

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
first branch's `km`. A native `Fn` signature does not establish purity. The
subsequently accepted plugin contract makes this impure counter's calculation
results undefined. It is historical evidence, not a future scheduling or
presentation correctness oracle. The maintained replay test now uses a pure,
always-true selector with non-semantic call-count instrumentation: its label is
already `km`, but the Phase C baseline still recorded two invocations. Phase D now requires **one**, and repeated rendering leaves that count unchanged.

The old body-copy hooks were removed with their copying paths; retaining an
unrecorded event and asserting zero would not prove sharing. The current observer
instead compares actual body addresses at completed module publication, and
requires a positive number of reference checks for imported projects.

The callable-plan checkpoint changes the two-call fixture to **2 plans during
preparation and 0 during each evaluation**. A separate, active schedule counter
observes both checking's runtime graph builder and the combined scheduler;
preparation has positive schedule work and evaluation must have **zero**. This
prevents zero plan counts from hiding a reintroduced call to a sorting helper.
Constructor resolution (**1**) and presentation evaluation (**3**) remain
positive baselines for C/D, not desirable
behavior. Pure-plugin tests permute declaration order and compare root/call values
without asserting independent invocation order.

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
allocation, or performance instrumentation. Callable schedules and physical
locations are now prepared: missing/misowned plans and missing locations fail
closed. The shared-machine checkpoint also retains multi-body constant pools and
imported references without copying their payloads during preparation. Pointer
checks inspect actual values in both views. Calls consume prepared runtime-import
keys instead of rediscovering lexical bindings or locating constant bodies.

The two-call cost fixture enters the same frame machine **three times**: root
and both calls. Prepared evaluation has **zero imported-source resolutions** as
well as zero plan/schedule construction. A separate nonvacuous import/instance
fixture observes positive source resolution during preparation and none during
evaluation. Frame initialization still copies values into mutable invocation
maps. Phase D now returns only selected output evidence and discards the call map.

Temporary mutations independently test rejected dependency-order corruption,
copied imported pools, accidental root fail-fast policy, and reintroduced runtime
sorting. The layer ratchet forbids interpreter access to checking/runtime facade
adapters; assertion evaluation uses an expression callback and contract result
types directly.

## Invocation-state ownership

Phase D deletes `presentation_calls.rs`, its handles/store, and compiler-side selector/projection HIR clones. `presentation_evidence.rs` is a contract consumed by the interpreter and output adapters; `runtime_presentation.rs` pairs values atomically with it. Branches and projections transfer selected subtrees rather than storing an ExprId-indexed decision cache. Locals and concrete recurrence iterations carry their own evidence.

Display-only unit requests retain owner/source and checked unit terms, not values or lexical environments. Called-frame requests resolve before that frame is discarded; caller-owned requests pass through child calls until the caller is complete. Constant selections are immutable compile-time data, with dynamic display requests deferred. Checked/prepared artifacts contain no runtime invocation state.

The retention regression grows a called DAG's unrelated temporary from 2 to 2,000 leaves while its selected two-element output still returns **3 evidence nodes**. Growing the output to 200 leaves instead returns **201**. The active frame-value counter must grow by over 1,900 nodes in the first control. These are structural retention measurements, not RSS or latency claims. Deferred requests provide a clear future aggregate-budget charging point without adding resource policy in this migration.

Borrowed field/index projection returns references into the selected subtree (or valid sparse absence/broadcast evidence), with pointer-identity and zero-copy controls. Scan and match clone only the selected subtree. Increasing annotated inputs/pattern fields from 4 to 64 measures **18 → 258** copied nodes for scan and **6 → 66** for match. Reintroducing whole-source clones fails the controls at **34 → 4,354** and **22 → 4,162**, respectively. These count structural copies, not allocated bytes or elapsed time.

Final display attachment accepts only a public value and resolved evidence. It has no interpreter, context, owner lookup, or host capability. Separate typed, path-addressed presentation diagnostics preserve SI; the scheduling regression also proves failed conversion-only dependencies cannot erase SI before projection. Quantity-literal dependencies remain computational. Canonical label-format accumulation overflow is a typed presentation formatting failure, even for constant literals; it cannot erase a successfully computed SI value. Plot, figure and layer property adapters retain fatal invariant/cancellation classification, including non-executable contextual String facts, while ordinary render failures remain contained.

## Bare-Wasm numerical boundary regression

`just wasm-test` also exercises determinant exponent cancellation, the tiny
cancellation mean, signed complex subnormals, signed zero, and contained overflow
through the actual JavaScript request/result boundary. It checks SI number bits,
not rendered strings. This is Node-hosted Wasm coverage, not a full-browser/UI
claim or proof of arbitrary numerical conditioning.
