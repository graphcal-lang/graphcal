# Post-MVP Phases (7+)

> After Phase 6, the MVP is complete. These phases add power but
> are not needed for a usable tool. Order within this group is flexible.

## Phase 7: N-Dimensional Tables and Indexes

**Adds:** `cat`/`range` keywords, multi-axis tables `[region: Region, fuel: Fuel]`,
aggregation across axes `sum(over: fuel)`.

**Depends on:** Phase 5 (1D tables).

**Key design questions:**

- How does `sum(over: axis)` interact with the dimensional type system?
- How are N-dim tables populated? Literal syntax for 2D+ data?
- How are cross-table references resolved? `@other_table[row.key].col`?

**Design docs:** [07-indexes](../07-indexes.md), [10-tables-and-autofill](../10-tables-and-autofill.md)

---

## Phase 8: System Dynamics

**Adds:** Time-axis tables, `scan` for temporal simulation, `integrate` for
ODE solving, mixed static + dynamic in one file.

**Depends on:** Phase 7 (N-dim tables, since time is a table axis).

**Key design questions:**

- Where does `row.dt` come from? Automatically available for time-axis tables?
- How does `integrate(init, method: RK4, |state, t| derivatives)` work?
- Adaptive step size?
- Events / discontinuities during simulation?

**Design docs:** [11-system-dynamics](../11-system-dynamics.md)

---

## Phase 9: Spaces

**Adds:** `space` keyword, `in` tag, `.untagged` escape hatch, `Transform` type.

**Depends on:** Phase 1 (dimensions). Independent of tables and functions.

**Key design questions:**

- Space-generic functions (`<S: Frame>`) -- needed?
- Multi-space values (`in Frame.ECI in Craft.Chaser`)?
- `Transform` composition type checking?
- Variant separator syntax (`;` vs `,` -- consistency with `cat`)?

**Design docs:** [06-spaces](../06-spaces.md)

---

## Phase 10: Tagged Unions and Pattern Matching

**Adds:** Multi-variant `type`, `match` expression, exhaustiveness checking.

**Depends on:** Phase 2 (structs, since tagged unions extend the same `type` keyword).

**Key design questions:**

- `match` syntax and exhaustiveness enforcement?
- Can `match` be used in node expressions?
- Interaction with dimensions (can variants carry dimensioned data)?

**Design docs:** [05-algebraic-data-types](../05-algebraic-data-types.md)

---

## Phase 11: Live View (TUI)

**Adds:** `ratatui` terminal grid, param editing, dependency highlighting,
table rendering, time series plots.

**Depends on:** Phase 6 (CLI workflow -- TUI is an alternative interface).

**Key design questions:**

- Interaction modes (view, edit, code, commit)?
- How are N-dim tables rendered (slice selectors)?
- Graph visualization (node-and-edge diagram)?

**Design docs:** [13-live-view](../13-live-view.md)

---

## Phase 12: Spreadsheet Compatibility

**Adds:** Excel import/export, `.sheetmap` bidirectional sync.

**Depends on:** Phase 5 (tables -- the mapping target).

**Key design questions:**

- How much formula reverse-engineering is feasible?
- `.sheetmap` file format?
- Conflict resolution for bidirectional sync?

**Design docs:** [14-spreadsheet-compatibility](../14-spreadsheet-compatibility.md)

---

## Phase 13: Python Interop

**Adds:** PyO3 bindings, `#[python]` nodes, parameter sweeps, DataFrame output.

**Depends on:** Phase 6 (CLI -- Python wraps the same engine).

**Key design questions:**

- Type mapping across the Rust/Python boundary?
- Unit handling in Python (plain float vs unit object)?
- Error propagation from Python exceptions?
- Effect annotations for Python nodes?

**Design docs:** [15-python-interop](../15-python-interop.md)

---

## Suggested Post-MVP Ordering

The phases above have loose dependencies. A reasonable order:

```
Phase 7 (N-dim tables) -> Phase 8 (system dynamics)
Phase 9 (spaces)        -- can be done in parallel with 7-8
Phase 10 (tagged unions) -- can be done in parallel with 7-9
Phase 11 (TUI)          -- can start after Phase 6
Phase 12 (spreadsheet)  -- can start after Phase 5
Phase 13 (Python)       -- can start after Phase 6
```

Pick based on user demand and strategic value. For an engineering audience,
the likely priority order is:

1. **Phase 9 (spaces)** -- coordinate frame safety is a key differentiator
2. **Phase 7+8 (N-dim tables + system dynamics)** -- Vensim replacement
3. **Phase 11 (TUI)** -- interactive exploration
4. **Phase 13 (Python)** -- ecosystem access
5. **Phase 12 (spreadsheet)** -- organizational adoption
6. **Phase 10 (tagged unions)** -- nice-to-have
