# Visibility & Bindability — Axiomatic Formalization

This document specifies graphcal's visibility and bindability model as a
small set of axioms (§2), from which all surface diagnostics (V001–V007)
are derived (§6). The intent is that a question of the form "does this
declaration / include / import compile?" can be answered by checking the
axioms directly, rather than by enumerating cases.

Companion design discussions live in GitHub issues — primarily
[#444](https://github.com/shunichironomura/graphcal/issues/444) (the
two-axis split) and
[#452](https://github.com/shunichironomura/graphcal/issues/452)
(re-export syntax). This document is the canonical statement; the issues
are the running discussion.

## 1. Primitive notions

A **symbol** is a named declaration. Each symbol has three attributes:

- **V** (visibility) ∈ {`private`, `visible`}
- **B** (bindability) ∈ {`fixed`, `bindable`}
- **R** (requiredness) ∈ {`optional`, `required`}

Symbol identity is **nominal**: identified by (file, name), not by
structure. Two symbols sharing a surface name in different files (or at
different declaration sites) are distinct.

An **include override** is a binding `(s ↦ e)` at an `include` site,
where `s` is a symbol in the included file.

## 2. Axioms

| #  | Statement |
|----|-----------|
| **A1** | A cross-file reference to `s` requires `V(s) = visible`. |
| **A2** | An `include` may override `s` only if `B(s) = bindable`. |
| **A3** | `B(s) = bindable ⇒ V(s) = visible`. (You can't bind what you can't name.) |
| **A4** | `R(s) = required ⇒ B(s) = bindable`. (External binding is the only way to satisfy.) |
| **A5** | `B = bindable` is meaningful for `param`, `index`, `type`, `dimension`. For all other kinds (including `unit`, `const`, `node`, `dag`, `assert`, `plot`, `figure`, `layer`), `B ≡ fixed`. |
| **A6** | **Nominal identity.** Two symbols with the same surface name but distinct declaration sites are distinct. The compiler never silently re-keys values between them. |
| **A7** | **No inference at boundaries.** Every type in a declaration's signature is written explicitly; no `impl Trait`-shaped escape hatch exists. |
| **A8** | **Importer obligation under override.** When an `include` overrides a bindable `s`, every `bindable` symbol whose body or default mentions a name nominally tied to `s` (e.g. variant literals `s::v` for an index `s`) must be re-bound by the importer in the same `include`. |
| **A9** | **Visibility composition.** Every name appearing in a `visible` signature — written or introduced via an `include` binding that the importer re-exports — must itself satisfy `V = visible` at the importing site. |
| **A10** | **Library-local bindability propagation.** For every declaration `x` whose body or default mentions a name nominally tied to a `bindable` symbol `s`: if `x`'s kind admits bindability (`param`/`index`/`type`/`dimension`), then `B(x) = bindable`; otherwise (`node`/`const`/`assert`/…), the body must abstract over `s` (e.g. `sum(p in I, …)`) — no literal `s::v` allowed. Checkable from the library's source alone. |
| **A11** | **Library-standalone validity.** Every library file is checkable for validity in isolation, with no reference to any importing file. The full set A1–A10 is required to satisfy this — A10 in particular ensures no library can produce a valid kept expression that A8 would later reject for "no reconciliation possible." |

### Notes on the axioms

- **A5 — why `unit` is excluded.** A unit's job is display + conversion
  factor. The niche case where a library bakes a literal like `100 jpy`
  against an in-library `unit jpy: Currency = 100.0 usd` is handled by
  the canonical idiom (§5), not by adding `unit` to the bindable set.
- **A6 → "indexes are nominal."** This is the principle that drove
  Policy B in §7 of issue #444: distinct names are distinct types even
  when structurally identical. It generalises to types and dimensions.
  For `unit`, nominality is moot under A5.
- **A8 flavour.** A8 fires for *nominal-rebind* bindings (where the
  symbol's name is replaced — `index`, `type`, `dimension`). For
  *value-override* bindings (where the same name gets a new body —
  `param`), reconciliation is trivial: the user supplied the new body.
  The two flavours coexist under one `bindings` map at the include
  site; the resolver dispatches on the bound symbol's kind.
- **A8 + A10 division of labour.** A10 (declaration-time, library-local)
  ensures no library declaration can be in a state where override of a
  bindable symbol would orphan a non-bindable body. A8 (include-time,
  importer obligation) only fires when the importer overrides a bindable
  `s` and forgets to also re-bind a `bindable` symbol whose body
  references `s`-tied names. Under A10, A8 cannot have an "unresolvable"
  variant — every triggering symbol is itself bindable, so re-binding is
  always possible. The error therefore always blames the importer.
- **A11 is meta.** A11 isn't an independent rule — it's a property the
  rest of the axioms collectively guarantee. Listed for emphasis: any
  proposed extension to the axiom set must preserve A11.

## 3. Legal attribute states

A1–A4 collapse 2 × 2 × 2 = 8 combinations to **4 legal states**:

| V       | B        | R        | Meaning                                    |
|---------|----------|----------|--------------------------------------------|
| private | fixed    | optional | file-local (default)                       |
| visible | fixed    | optional | exported, frozen                           |
| visible | bindable | optional | exported + overridable, has default        |
| visible | bindable | required | exported + overridable, must be bound      |

Excluded:

- V=private + B=bindable → A3.
- B=fixed + R=required → A4.
- All other excluded states reduce to one of the two above.

## 4. Surface syntax

```text
              param x: Length = 10 m;   // private          (V=private, B=fixed)
pub           param x: Length = 10 m;   // visible, fixed   (V=visible, B=fixed)
pub(bind)     param x: Length = 10 m;   // visible+bindable (V=visible, B=bindable)
```

Same shape applies to every kind for which `B = bindable` is meaningful
(`param`, `index`, `type`, `dimension`). For all other kinds, only `pub`
or bare are legal.

Reuses Rust's `pub(...)` shape, leaving room for a future scoped-
visibility extension to compose as `pub(crate, bind)`, `pub(in lib, bind)`,
etc.

### 4.1 Visibility on `include` and `import`

`include` and `import` are use-sites, not declarations — A5 says they
don't admit bindability (`B ≡ fixed`). The V-axis still applies: a bare
form is file-local; a `pub`-prefixed form re-exports.

Two forms (per [issue #452](https://github.com/shunichironomura/graphcal/issues/452)):

```text
// Whole-module re-export under a namespace
pub include "./container.gcl"(Element: Inner) as c;
pub import  "./types.gcl" as types;

// Selective re-export
include "./container.gcl"(Element: Inner) { pub items };
import  "./types.gcl" { pub Length, pub Mass };
```

Both forms feed A9 Case 2 (V006). For the whole-module form, the check
ranges over every `pub` item in the included file; for the selective
form, only the listed `pub`-marked items.

**Open:** whether the two forms are mutually exclusive, or whether
`pub include "X"(...) as c { pub items }` can both re-export under
namespace `c` and surface `items` flat. Current recommendation:
exclusive.

## 5. Canonical idioms

### 5.1 Injectable dependency over an index

```text
// lib.gcl
pub(bind) index Phase;                      // required, no fixed variants
pub(bind) param cost:   Dimensionless[Phase];
pub(bind) param weight: Dimensionless[Phase];

node weighted_total: Dimensionless =
    sum(p in Phase, @cost[p] * @weight[p]);  // no Phase::X literal
```

```text
// main.gcl
index MyPhase = { Plan, Make };
include "./lib.gcl"(
    Phase: MyPhase,
    cost:   { MyPhase::Plan: 100, MyPhase::Make:  50 },
    weight: { MyPhase::Plan: 0.7, MyPhase::Make: 0.3 },
);
```

`weighted_total`'s body contains no nominally-tied name from `Phase`. A8
is vacuous — the dependency is genuinely injectable.

### 5.2 Variable conversion rate (currency)

Library models the rate as a `param`; the `unit` derives from it via `@`:

```text
// lib.gcl
pub(bind) param jpy_in_usd: Dimensionless = 0.01;
unit jpy: Currency = @jpy_in_usd usd;

// uses `100 jpy` freely — at eval, the literal resolves through the
// current value of jpy_in_usd
```

```text
// main.gcl
include "./lib.gcl"(jpy_in_usd: 0.0067);   // override the rate
```

This works because graphcal's `unit` definitions may reference computed
node values. The dual form **`const unit`** marks a unit whose
conversion factor does *not* depend on runtime values — useful for
dim-check / const-folding that needs to assume a fixed factor.

## 6. Derived rules (V001–V006)

| Rule | Derivation | Surface change vs. today |
|------|------------|--------------------------|
| **V001** ImportPrivateItem | A1 | rename only — same rule under the new model |
| **V002** RequiredMustBeBindable | A4 (with A3) | re-stated: "required → must be `pub(bind)`" |
| **V003** PrivateInPublic | A9 on written annotations | unchanged in spirit; A7 keeps it a syntactic walk |
| **V004** VariantLiteralOfBindableIndex | A10 (non-bindable case) | re-stated: "variant literal of a `pub(bind)` index in a non-bindable body" — fires for `node`/`const`/`assert`/…, never for `pub(bind) param` |
| **V005** IncludeMustReconcileOverride | A8 at the include site | new |
| **V006** GenericsLeakage | A9 applied to `include` bindings of bindable `type`/`dimension` | new |
| **V007** *(implied)* BindableMentionInFixedDecl | A10 (bindable case, contrapositive) | new — "`pub param y` whose default mentions `I::v` for a `pub(bind)` index `I` must itself be `pub(bind) param y`" |

V005 corresponds to issue #444 §7. Under this formalisation it is keyed
off A8 directly; the prior staged narrowing (Steps 1–3) collapses
because A10 absorbs the non-bindable case at declaration time.

## 7. Worked-case derivations

| # | Scenario | Derivation | Verdict |
|---|----------|------------|---------|
| 1 | `pub(bind)` index with literal-default `param`, no include | A8 only fires under override; here there is none | **OK** (V004 narrowed) |
| 2 | Importer overrides index, omits `param` binding | A8: `param` default mentions `Phase::v` and was not re-bound | **V005** |
| 3 | Library declares `pub(bind) index Phase` and `node total = @cost[Phase::a] + @cost[Phase::b]` | A10: `node` is non-bindable, body has `Phase::v` literals → forbidden at declaration | **V004 at library compile time** (no include needed) |
| 4 | Dependency abstracts over `Phase` via `sum(p in Phase, …)` | body mentions no name nominally tied to `Phase`; A8 vacuous | **OK** |
| 5 | `pub include "container.gcl"(Element: PrivateInner) as c;` (or `{ pub items }`) | A9 Case 2: re-exported items' effective signatures name `PrivateInner` (V=private at importer) | **V006** |
| 6 | Required `param` declared as bare or `pub`-only | A4 forbids the state | **V002** |
| 7 | `pub` declaration whose annotation names a private symbol | A9 directly | **V003** |
| 8 | Library uses `100 jpy`; importer wants different rate; library follows §5.2 idiom | importer binds `jpy_in_usd`; A8 trivially satisfied | **OK** |
| 9 | Library uses `100 jpy`; library defines `jpy` directly with literal rate; importer wants different rate | A5 says `unit` is `fixed`; no override possible | **rejected at include site** — library must refactor to §5.2 idiom |
| 10 | Library declares `pub(bind) index I = {a,b}` and `pub param y: D[I] = {I::a:1, I::b:2}` | A10: `param` admits bindability and default mentions `I::v` → `y` must be `pub(bind)` | **V007 at library compile time** |
| 11 | Library declares `pub(bind) index I = {a,b}` and `pub(bind) param y: D[I] = {I::a:1, I::b:2}` | A10 satisfied (`y` is bindable). Importer overrides `I` and forgets to re-bind `y`. | **V005 at importer** |

## 8. What the axioms intentionally don't decide

- **Re-export semantics.** Resolved by issue #452: `pub include` / `pub
  import` for whole-module, `{ pub items }` for selective. See §4.1.
  V006 becomes implementable once #452 lands.
- **Scoped visibility.** A1 / A9 generalise trivially by replacing
  `visible` with a lattice of scopes. `pub(bind)` syntax composes as
  `pub(crate, bind)` etc. Not needed for the initial implementation.
- **Sealed bindability.** A symbol bindable by direct importers but not
  by transitive ones — would need a fourth attribute. Open.
- **Bindability flavour distinction.** A8 collapses value-override
  (`param`) and nominal-rebind (`index`, `type`, `dimension`) into one
  rule. If they ever need different surface semantics, A8 splits.
- **Migration path.** Rust's permissive-first / lint-tightening
  playbook (`unreachable_pub`-style) is worth borrowing. Not encoded in
  the axioms.

## 9. Open questions

1. **Is A9 strong enough as stated?** A9 speaks of "re-exported items".
   With #452 settling the re-export construct, the definition is
   concrete; whether A9 should be split into A9a (written annotations —
   V003) and A9b (re-export — V006) is mainly an exposition choice.
2. **Should `dag` be in the bindable set?** A5 currently excludes it.
   Issue #444 §4 marks `dag` as "instantiated, not re-bound" — kept here
   pending an explicit reopen.
3. **Required-ness for `index` / `type` / `dimension`.** `R = required`
   means no body / no variants. Whether this is already grammatically
   expressed for all three, or requires new grammar (e.g.
   `pub(bind) type Element;` with no body), is not yet pinned down.
4. **A8 detection algorithm.** "Names nominally tied to `s`" needs a
   precise per-kind definition. For `index`: variant literals `s::v`.
   For `type` / `dimension`: TBD — worth pinning down before
   implementation.
5. **V005 step ordering.** Issue #444 §7 originally staged V005 in three
   steps. Under A8 + A10, the staging is largely redundant; V005 can
   land as a single change keyed off A8.

## 10. Cross-reference to issue #444

| Issue #444 section | Axiom(s) covering it |
|--------------------|----------------------|
| §3 two-axis model | A1, A2, A3 |
| §4 per-kind matrix | A5 |
| §5 nominal typing | A6 |
| §6 V001–V006 restatement | derived rules in §6 above |
| §7 V005 spec | A8 (and A10 for the originally-staged Step 3) |
| §8 Rust analogues | informs A7, A9 |
| §9 syntax options | §4 above |
| §10 required-ness | A4 |
| §11 non-goals | A7 (no `impl Trait`); flat visibility (A1 unchanged) |
| §12 open questions | §9 above |

## 11. Settled design choices

These are the load-bearing choices the axioms encode. Each is the
outcome of an explicit design decision rather than a derivation from
something more primitive.

- **Bindable set is `{param, index, type, dimension}`.** `unit` is
  deliberately excluded in favour of the §5.2 canonical idiom
  (rate-as-`param`, unit derives via `@`). `const unit` marks the
  runtime-independent case.
- **Surface syntax is `pub(bind)`** (Option B in #444 §9). Bare for
  private, `pub` for visible-only, `pub(bind)` for visible + bindable.
  Re-exports use `pub include` / `pub import` and `{ pub item }` per
  #452.
- **A8 covers both flavours.** Value-override (`param`) and nominal-
  rebind (`index`, `type`, `dimension`) reconcile under the same
  axiom; the resolver dispatches on the bound symbol's kind.
- **Library validity is standalone (A11).** A library file is checkable
  in isolation. A10 is the axiomatic enforcement point; A8 only fires
  for importer mistakes, never for library bugs.
- **Indexes are nominal (A6).** Two indexes with identical variant names
  are distinct types. Override binding renames the index identifier;
  variants are never silently re-keyed across distinct indexes.
