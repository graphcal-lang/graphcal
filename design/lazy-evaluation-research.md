# Lazy / Stale Evaluation Research

> Survey of how similar systems handle deferred computation, and recommendations for Graphcal.

## The Problem

Graphcal's LSP eagerly computes every node on every file change. For heavy calculations this is unacceptable. We need a way for users to mark nodes as "don't recompute automatically" — a **stale marker** — so that expensive subgraphs are only evaluated on explicit request.

Two open questions:
1. **Syntax**: comment directive vs. concrete syntax?
2. **Type of stale nodes**: should staleness appear in the type system (`Stale<T>`) or be transparent?

---

## Survey of Existing Systems

### 1. Excel / Google Sheets

**Global calculation modes.** Excel has three application-level modes:
- **Automatic** (default): every edit recalculates all dirty cells and their dependents.
- **Manual**: cells stay dirty until the user presses F9 (smart recalc) or Ctrl+Alt+F9 (full recalc). Shift+F9 recalculates only the active sheet.
- **Automatic Except Data Tables**: normal formulas recalculate automatically, but resource-intensive What-If Data Tables are deferred to manual F9.

**No per-cell control in the UI.** The calculation mode is an application-level singleton; the first non-template workbook opened determines the mode for all open workbooks. Per-cell or per-range control only exists via VBA (`Range.Dirty`, `Range.Calculate`, `Worksheet.EnableCalculation`).

**Dirty propagation.** When a cell changes, Excel marks it and ALL transitive dependents as dirty. In automatic mode, dirty cells are immediately recalculated in topological order (the "calculation chain"). In manual mode, they stay dirty — the UI shows stale values with no visual indicator (a major footgun).

**No early cutoff.** If a cell recalculates to the same value, its dependents are still recalculated. This is a notable gap compared to Salsa/Bazel.

**Volatile functions** (`NOW()`, `RAND()`, `OFFSET()`, `INDIRECT()`) are always dirty and recalculate on every event, cascading to all dependents. They act as a "taint" through the graph.

**Key takeaway for Graphcal:** Excel's all-or-nothing approach (workbook-level manual mode) is too coarse. Users want per-node control. The absence of a visual indicator for stale cells in manual mode is a well-known pain point — Graphcal should make staleness visible.

### 2. Marimo Notebooks (Python)

Marimo is the closest analogue to Graphcal's reactive model among notebook systems.

**Two execution modes:**
- **Autorun** (default): when a cell runs, all descendant cells are automatically re-executed.
- **Lazy**: running a cell marks its descendants as **stale** instead of executing them. Stale cells require explicit user action to run.

**Staleness propagation.** Staleness is transitive: if A → B → C in the dataflow graph and A runs, both B and C become stale. The DAG is built via static analysis of variable definitions and references (zero runtime overhead).

**Ancestor-first guarantee.** In lazy mode, if the user runs a stale cell that has stale ancestors, those ancestors are automatically run first. This ensures you never compute with out-of-date inputs. This is a critical design decision — the user can always trust that running a cell produces a correct result.

**UI indicators:**
- Per-cell: run icon turns yellow when stale.
- Global: a yellow "Run" button appears to run all stale cells at once.
- No execution-order numbers (unlike Jupyter) since the system determines order.

**Configuration granularity:** Currently notebook-level (`on_cell_change = "lazy"` in script metadata, `pyproject.toml`, or user config). There is an active proposal (issue #2100) for per-cell modes with three states:
1. **Reactive** (default) — automatic execution
2. **Lazy/Manual** — requires explicit trigger
3. **Protected** — requires confirmation dialog (for side-effect cells like DB writes)

**Caching:** `mo.cache` for in-memory function memoization, `mo.persistent_cache` for disk-based block caching (invalidated when cell or ancestors change).

**Key takeaway for Graphcal:** Marimo's per-cell lazy mode proposal (reactive/lazy/protected) maps well to what Graphcal needs. The ancestor-first guarantee is essential — it lets the stale marker be purely a performance optimization without changing semantics. The visual indicator for staleness is important UX.

### 3. Observable Notebooks

Observable uses a reactive dataflow model where each cell is a function of its referenced variables, executed in topological order.

**Immutability as a constraint.** All variables are immutable by default; `mutable` is an explicit opt-in. This ensures the dependency graph is complete — mutations create invisible dependencies that static analysis cannot track.

**No lazy mode.** Observable is always eager. Cells re-execute whenever their inputs change. There is no built-in mechanism to defer computation.

**Key takeaway for Graphcal:** Observable's lesson is that immutability is essential for correct reactivity. Graphcal already has this — `node` values are immutable, and the `@` sigil makes all dependencies explicit. This means a stale marker can be sound.

### 4. Jupyter / IPython

**No dependency tracking.** Cells share mutable kernel state. There is no automatic re-execution, no staleness detection, and no visual indicator that a cell's output is stale relative to edited upstream cells. This is Jupyter's most criticized design flaw — studies show 36%+ of notebooks are not reproducible.

**Key takeaway for Graphcal:** Jupyter is the cautionary tale. Graphcal should never silently show stale values without indicating they are stale.

### 5. Build Systems (Make, Bazel/Skyframe, Nix)

**Make:** Timestamp-based staleness. If a prerequisite is newer than the target, rebuild. No early cutoff — if a `.o` file rebuilds identically, linking still re-runs. Simple but imprecise.

**Bazel/Skyframe:** Content-based staleness with **change pruning (early cutoff)**. When a dirty node re-evaluates to the same value, its dependents are NOT re-evaluated. The dependency graph is built from pure `SkyFunction`s that can only access declared dependencies (hermeticity). Two invalidation strategies: bottom-up (mark all transitive dependents dirty) and top-down (walk from target, checking each node).

**Nix:** Input-addressed derivations (hash of all inputs determines output path) have no early cutoff — changing one byte of glibc rebuilds everything downstream. Content-addressed derivations (experimental) add early cutoff by hashing outputs, so identical outputs prevent cascading rebuilds.

**Key takeaway for Graphcal:** Bazel's change pruning is exactly what Graphcal's design doc already describes ("early cutoff / backdating"). The stale marker is orthogonal to this — early cutoff prevents unnecessary recomputation when values don't change; the stale marker prevents recomputation entirely until explicitly requested.

### 6. Salsa / rust-analyzer

Salsa (which Graphcal's design doc cites as prior art) uses a red-green algorithm:
- **Green queries:** verified unchanged in the current revision.
- **Red queries:** known to have changed.
- **Unverified:** not yet checked in this revision.

When a query is demanded, Salsa walks its dependency tree. If all inputs are green, the cached result is reused. If an input is red, the query re-executes. If the result is the same ("backdating"), dependents stay green.

Salsa's model is **demand-driven** (lazy) by default — queries only execute when their result is demanded. This is the opposite of Graphcal's current eager model. The key idea: Salsa never computes something nobody asked for.

**Key takeaway for Graphcal:** The stale marker essentially converts a node from "eagerly demanded by the LSP" to "only demanded on explicit request." The incremental computation machinery (early cutoff, dirty tracking) is orthogonal and complementary.

---

## Analysis of the Two Open Questions

### Question 1: Syntax — Comment Directive vs. Concrete Syntax

| Approach | Pros | Cons |
|----------|------|------|
| Comment directive (`// @lazy`) | Easy to add, no parser changes, familiar from eslint/pragma | Invisible to type checker, lost in AST, can't be used in `graphcal watch`, not a "real" language feature |
| Attribute (`#[lazy]`) | Already precedented in Graphcal's syntax (`#[output]`, `#[python]`), metadata-level, doesn't change semantics | Could be seen as "optional hint" that tools may ignore |
| Keyword modifier (`lazy node`) | Clear, explicit, first-class language feature, visible in AST | Adds a keyword, changes the grammar |
| Expression wrapper (`defer(@heavy)`) | Fine-grained (can defer subexpressions) | Too granular, changes expression types |

**Recommendation: keyword modifier `lazy node`.**

Rationale:
- Graphcal already uses keyword modifiers (`private node`). `lazy node` follows the same pattern.
- It is a concrete syntax element, visible in the AST, usable by the LSP, `graphcal watch`, the live view, and any future tooling.
- It communicates intent clearly: "this node exists but should not be eagerly recomputed."
- It applies at the declaration level, which is the right granularity — Graphcal's DAG is a graph of declarations, so the staleness control should be per-declaration.
- Comment directives are wrong for Graphcal's philosophy of explicitness over implicitness ("Remember Mars Climate Orbiter").

The attribute `#[lazy]` is a reasonable alternative if you want to keep keywords minimal, but it reads as an "annotation hint" rather than a semantic guarantee. Given that Graphcal prioritizes safety and explicitness, a keyword is more appropriate.

### Question 2: What Type Should Stale Nodes Have?

The fundamental tension:

| Approach | Pros | Cons |
|----------|------|------|
| `Stale<T>` wrapper type | Explicit, type-safe, forces downstream to acknowledge staleness | Type juggling infects the entire downstream graph. User must unwrap everywhere. Defeats "just works" goal. |
| Transparent `T` (same type, stale value) | No type changes, downstream code unchanged, "just works" on the eager path | Stale values silently used — exactly what Jupyter does wrong |
| `T` with runtime staleness tracking (value + freshness bit) | Same type, but operations on stale inputs produce stale outputs | Still requires some propagation mechanism |
| **Demand-driven: stale nodes have no value until demanded** | Clean separation, no stale values in circulation | Need a mechanism to "demand" (run) a stale node |

**Recommendation: demand-driven model with transparent types.**

The key insight from marimo and Salsa: **a stale node should not have a stale value — it should have no current value at all, until demanded.** Here is the proposed semantics:

1. A `lazy node` is not evaluated when its inputs change. It is marked **stale** (like marimo's yellow indicator).

2. A stale node's **last computed value** is retained for display purposes (shown grayed out / with a stale indicator in the LSP inlay hints and live view), but is not treated as the "current" value for downstream computation.

3. **Downstream nodes of a stale node are also stale.** Staleness propagates transitively through the graph. This prevents silently computing with out-of-date inputs (Jupyter's mistake).

4. **When the user explicitly requests evaluation** (via LSP code action, `graphcal eval`, `graphcal watch` trigger, or live view "Run" button), the stale node and its stale ancestors are evaluated in topological order (marimo's ancestor-first guarantee). The result has type `T` — no wrapper, no type juggling.

5. **Eager nodes downstream of a lazy node are automatically computed** once the lazy node is evaluated. The laziness boundary is at the `lazy node` declaration; everything downstream that is not itself lazy is eager as usual.

6. **From the type system's perspective, `lazy` is invisible.** A `lazy node x: Length` has type `Length`. You can reference `@x` in any expression that expects `Length`. The `lazy` modifier only affects *when* the node is evaluated, not *what type* it has.

This means:
- **On the eager path (normal editing):** stale nodes show their last value with a visual stale indicator. Downstream stale nodes also show stale indicators. No type errors, no code changes needed.
- **On the evaluation path (explicit run):** everything computes normally with type `T`. The lazy modifier is transparent.
- **In `graphcal watch`:** lazy nodes are not recomputed on file change. They are recomputed when the user sends a "run" signal (e.g., pressing Enter, or a specific watch command).

---

## Proposed Design

### Syntax

```gcl
// Normal eager node (unchanged)
node thrust: Force = @mass * @g0;

// Lazy node — not eagerly recomputed
lazy node trajectory: Trajectory = {
    // expensive orbital mechanics computation
    let steps = propagate_orbit(@initial_state, @duration, @dt);
    Trajectory { steps }
};

// Lazy param — unusual but allowed (e.g., param loaded from expensive external source)
lazy param dataset: Dataset = load("measurements.csv");
```

`lazy` is a declaration modifier, like `private`. It can combine:
```gcl
private lazy node _internal_result: Result = heavy_computation(@inputs);
```

### Staleness Propagation Rules

```
  [param a]          // eager, fresh
      |
  [lazy node b]      // lazy, stale (not recomputed when a changes)
      |
  [node c]           // eager, but stale (depends on stale b)
      |
  [node d]           // eager, but stale (depends on stale c)
```

- Changing `param a` makes `lazy node b` stale.
- `node c` and `node d` are transitively stale because they depend on a stale node.
- Running `lazy node b` causes `b`, `c`, and `d` to all evaluate (b first, then c and d in topological order).

### LSP Behavior

- **Inlay hints for stale nodes within a session:** if the node was previously evaluated in the current LSP session, show the last computed value in a muted/gray style with a "(stale)" suffix, e.g., `= 3935.2 m/s (stale)`.
- **Inlay hints for stale nodes after LSP restart:** show `(not evaluated)` — no value is displayed. This avoids the need for disk-based value caching (see "Disk Cache Considerations" below).
- **Code action:** "Evaluate stale nodes" on a lazy node to trigger recomputation of that node and its stale ancestors/descendants.
- **Diagnostics:** No errors for stale nodes. Staleness is informational, not an error.

### `graphcal watch` Behavior

- File change triggers recomputation of all eager nodes.
- Lazy nodes are skipped (marked stale).
- A "run" command (e.g., pressing `r` in the TUI, or a CLI flag) triggers full evaluation including lazy nodes.

### `graphcal eval` Behavior

- `graphcal eval` evaluates everything, including lazy nodes. The `lazy` modifier is only a hint for interactive/watch contexts.
- `graphcal eval --lazy` respects lazy markers and skips stale nodes (useful for CI where you want fast feedback on the eager subgraph).

---

## Disk Cache Considerations

Showing the "last computed value" for stale nodes across LSP restarts requires persisting evaluation results to disk. The current LSP stores computed values only in memory (`eval_values: HashMap<String, String>` in `AnalysisResult`), which is lost when the process exits.

**Three options:**

| Option | Stale display after restart | Infrastructure needed |
|--------|---------------------------|----------------------|
| **A: Disk cache** | Grayed-out last value | Cache dir, serialization, invalidation, eviction, `.gitignore`, corruption handling |
| **B: No disk cache** | "(not evaluated)" | None — works with existing in-memory cache |
| **C: Hybrid (opt-in disk cache)** | Last value if cache present, else "(not evaluated)" | Cache as separate opt-in feature |

**Recommendation: Start with Option B (no disk cache).** The in-memory cache already covers the most common workflow — editing a file and seeing stale indicators within a session. Disk persistence is an orthogonal enhancement that can be added later without changing `lazy` semantics. Building cache infrastructure now (serialization for all `Value` types, cache keying, versioning across schema changes, cross-branch correctness) would delay the feature for marginal UX gain.

If disk caching is added later, key design questions:
- **Cache location:** `.graphcal-cache/` in project root (`.gitignore`d) or XDG cache dir?
- **Cache key:** node name + content hash of transitive inputs (like Bazel's SkyKey), or revision number?
- **Cross-machine validity:** cached values are only valid for the same input state. Sharing caches across machines (like Bazel's remote cache) is a future concern.
- **Schema versioning:** cache must be invalidated when `Value` representation changes between Graphcal versions.

---

## Comparison with Graphcal's Existing Design

The `lazy` modifier is **compatible with** the incremental computation model described in `design/01-computation-model.md`:

| Mechanism | Role | Relationship to `lazy` |
|-----------|------|----------------------|
| Dirty tracking | Detect which nodes need recomputation | `lazy` nodes are dirty but not recomputed until demanded |
| Early cutoff (backdating) | Prevent cascading recomputation when value unchanged | Applies when a `lazy` node is finally evaluated — if its value hasn't changed, downstream eager nodes skip |
| Durability classification | Skip validation for stable subgraphs | Orthogonal — durability classifies input change frequency; `lazy` classifies evaluation urgency |
| Revision counter | Track param changes | `lazy` nodes accumulate revision debt — they skip revisions until explicitly caught up |

The `lazy` modifier adds a new dimension to the evaluation strategy: **when** to evaluate, complementing dirty tracking's **whether** to evaluate and early cutoff's **how far** to propagate.

---

## Prior Art Summary

| System | Granularity | Staleness Visible? | Stale Value | Trigger to Evaluate | Early Cutoff? |
|--------|-------------|-------------------|-------------|-------------------|---------------|
| Excel (manual mode) | Workbook-level | No (!) | Shows last value, no indicator | F9 / Shift+F9 | No |
| Marimo (lazy mode) | Notebook-level (per-cell proposed) | Yes (yellow icon) | No value until run | Click run / run-all | N/A |
| Observable | None (always eager) | N/A | N/A | N/A | N/A |
| Jupyter | None (always manual) | No (!) | Shows last value, no indicator | Manual cell execution | N/A |
| Bazel | Per-target | Yes (needs rebuild) | No value until built | `bazel build` | Yes |
| Salsa | Per-query (demand-driven) | N/A (internal) | Cached until verified | Query demand | Yes |
| **Graphcal (proposed)** | **Per-node** | **Yes (stale indicator)** | **In-session: last value shown as stale. After restart: "(not evaluated)"** | **Explicit run command** | **Yes (existing design)** |

---

## Recommendations Summary

1. **Use `lazy` keyword modifier** (`lazy node`, `lazy param`) — concrete syntax, explicit, fits existing modifier pattern.
2. **Transparent types** — `lazy node x: Length` has type `Length`. No `Stale<T>` wrapper.
3. **Transitive staleness propagation** — downstream nodes of a stale node are also stale.
4. **Ancestor-first evaluation** — running a stale node first evaluates its stale ancestors (marimo's guarantee).
5. **Visual staleness indicators** — LSP inlay hints show "(stale)" with muted styling. Live view grays out stale values.
6. **`graphcal eval` ignores `lazy`** — full evaluation always computes everything. `lazy` only affects interactive/incremental contexts (LSP, watch, live view).
