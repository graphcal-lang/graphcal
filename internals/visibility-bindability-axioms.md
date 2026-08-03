# Visibility & Bindability — Axiomatic Formalization

This document specifies graphcal's visibility and bindability model as a
small set of axioms (§2), from which all surface diagnostics (V001–V006)
are derived (§6). The intent is that a question of the form "does this
declaration / include / import compile?" can be answered by checking the
axioms directly, rather than by enumerating cases.

Companion design discussions live in GitHub issues — primarily
[#444](https://github.com/graphcal-lang/graphcal/issues/444) (the
two-axis split) and
[#452](https://github.com/graphcal-lang/graphcal/issues/452)
(re-export syntax). This document is the canonical statement; the issues
are the running discussion.

## 1. Primitive notions

A **symbol** is a named declaration. Ordinary declarations have these
attributes:

- **V** (export visibility) ∈ {`private`, `exported`}
- **B** (bindability) ∈ {`fixed`, `bindable`}
- **R** (requiredness) ∈ {`optional`, `required`}

A `param` instead has the distinct external role **input port**. It does not
occupy an implicit `V = exported` state: the declaration kind itself makes the
port externally nameable and bindable. Requiredness still distinguishes a port
without a default from one with a default.

Write **N(s)** for external nameability: `N(s)` holds when `s` is either an
explicitly exported ordinary declaration or a param input port. For value
symbols, both roles are also output-selectable: selecting a param reads its
effective bound/default value. Keeping the provenance separate is load-bearing,
because only input ports accept value bindings and only explicit exports carry
the `pub`/`pub(bind)` role. Compiler queries should therefore be operation-
specific (`is_input_port`, `is_explicit_export`, `can_select_output`) rather
than treating the union as one undifferentiated visibility set.

Symbol identity is **nominal**: identified by (file, name), not by
structure. Two symbols sharing a surface name in different files (or at
different declaration sites) are distinct.

An **include override** is a binding `(s ↦ e)` at an `include` site,
where `s` is a symbol in the included file.

A name is **nominally tied to** a symbol `s` when its meaning is fixed
by `s`'s declaration structure (not just by `s`'s identity as a graph
node). Concretely:

| Kind of `s` | Substitution under override | Names nominally tied to `s`                                            |
| ----------- | --------------------------- | ---------------------------------------------------------------------- |
| `index`     | partial                     | variant literals `s::v`                                                |
| `type`      | partial                     | constructors of `s`, field accesses on values of `s`, variant patterns |
| `dimension` | total (algebraic)           | _(none)_ — substitution propagates through dim algebra cleanly         |
| `param`     | total (value)               | _(none)_ — `param` has no nominal substructure                         |

Two camps:

- **Partial-substitution** (`index`, `type`): the symbol carries
  nominal substructure (variants, constructors, fields) that can fail
  to exist in the override. Mentions of those substructures are
  nominally tied to `s` and can orphan under rebinding. A8 and A10
  fire on them.
- **Total-substitution** (`dim`, `param`): override rewrites references
  cleanly — for `param` by reactive value propagation through `@`-refs,
  for `dim` by algebraic substitution that always yields a valid dim
  (Length × NewD₁, NewD₁ / Time, etc., all compose). No expression
  orphans. A8 and A10 are vacuous for these references.

Plain reactive graph references (`@s`) are **not** nominally tied to `s`.
They propagate value through evaluation, not name through the AST, so
they recompute correctly under any value change to `s`. A8 and A10 do
not fire on plain `@s` mentions.

**Caveat for `dim` rebinding (downstream of A8 / A10).** A library's
`base unit kg: Length` substitutes to `base unit kg: NewLength` under
override. If `NewLength` already has its own base units, this can
create unit-system collisions. That is a name-collision concern handled
by unit-system validation, not an A8 / A10 trigger.

A declaration kind is a **sink kind** when nothing in the language can
depend on a declaration of that kind. The sink kinds are `plot`,
`assert`, `figure`, and `layer`: they are observation / specification
points, not data sources. By construction, no future declaration can
introduce a dependency on a sink-kind declaration, so a private sink
declaration can never be reached from any `pub` declaration in the same
file.

## 2. Axioms

| #       | Statement                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| ------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **A1**  | A cross-file reference to `s` requires `N(s)`: the target must be an explicit export or a param input port.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| **A2**  | An `include` may override `s` only if `B(s) = bindable`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| **A3**  | `B(s) = bindable ⇒ N(s)`. (You cannot bind what you cannot name.) For ordinary declarations this requires export visibility; params satisfy it through their input-port role.                                                                                                                                                                                                                                                                                                                                                                                                                   |
| **A4**  | `R(s) = required ⇒ B(s) = bindable`. (External binding is the only way to satisfy.)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| **A5**  | Every `param` is an input port and therefore bindable. Annotation-controlled `B = bindable` is meaningful for `index`, `type`, and `dimension`. For all other kinds (including `unit`, `const`, `node`, `dag`, `assert`, `plot`, `figure`, `layer`), `B ≡ fixed`.                                                                                                                                                                                                                                                                        |
| **A6**  | **Nominal identity.** Two symbols with the same surface name but distinct declaration sites are distinct. The compiler never silently re-keys values between them.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| **A7**  | **No inference at boundaries.** Every type in a declaration's signature is written explicitly; no `impl Trait`-shaped escape hatch exists.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| **A8**  | **Importer obligation under override.** When an `include` overrides a bindable `s`, every `bindable` symbol whose body or default mentions a name nominally tied to `s` (in the sense of §1) must be re-bound by the importer in the _same include statement that performs the override_. Plain `@s` graph references do not trigger A8.                                                                                                                                                                                                                                                                                                                       |
| **A9**  | **External-signature composition.** Every type-system name appearing in an export signature or input-port signature — written or introduced via an `include` binding that the importer re-exports — must itself be explicitly exported at the importing site.                                                                                                                                                                                                                                                                                                                                      |
| **A10** | **Library-local bindability propagation.** For every declaration `x` whose body or default mentions a name nominally tied to a `bindable` symbol `s` (in the sense of §1): (a) if `x`'s kind admits bindability (`param`/`index`/`type`/`dimension`), then `B(x) = bindable`; (b) else if `x`'s kind is a sink kind (`plot`/`assert`/`figure`/`layer`), then `V(x) = private` — public sink kinds must abstract over `s`; (c) otherwise (`node`/`const`/…), the body must abstract over `s` — i.e. mention `s` only via plain refs like `@s` or via binders like `for p : s { … }`, never via nominally-tied names. Checkable from the library's source alone. |

### Notes on the axioms

- **A5 — why `unit` is excluded.** A unit's job is display + conversion
  factor. The niche case where a library bakes a literal like `100 jpy`
  against an in-library `unit jpy: Currency = 100.0 usd` is handled by
  the canonical idiom (§5), not by adding `unit` to the bindable set.
- **A6 → "indexes are nominal."** This is the principle that drove
  Policy B in §7 of issue #444: distinct names are distinct types even
  when structurally identical. It generalises to types and dimensions.
  For `unit`, nominality is moot under A5.
- **A3 and A4 are technically theorems from {A1, A2}.** A3 (`B = bindable
  ⇒ N(s)`) follows because override at an include site requires naming the
  symbol across the boundary (A1 + A2). For params that nameability comes from
  the input-port role, not from export visibility. A4 (`R = required ⇒ B = bindable`)
  follows because required symbols have no body and external `include`
  binding (A2) is the only supply mechanism. Both are listed as axioms
  for clarity; flagged here as derived for honesty.
- **A8 flavour.** A8 fires for _nominal-rebind_ bindings (where the
  symbol's name is replaced — `index`, `type`, `dimension`). For
  _value-override_ bindings on `param`, the table in §1 records that
  `param` has no nominal substructure, so A8's trigger is empty: a
  `param x: Length = @y` with only `y` overridden does not require
  re-binding `x` — the reactive graph recomputes `x` from the new `y`
  automatically. The two flavours coexist under one `bindings` map at
  the include site; the resolver dispatches on the bound symbol's
  kind.
- **A8 + A10 division of labour.** A10 (declaration-time, library-local)
  ensures no library declaration can be in a state where override of a
  bindable symbol would orphan a non-bindable body. A8 (include-time,
  importer obligation) only fires when the importer overrides a bindable
  `s` and forgets to also re-bind a `bindable` symbol whose body
  references `s`-tied names. Under A10, A8 cannot have an "unresolvable"
  variant — every triggering symbol is itself bindable, so re-binding is
  always possible. The error therefore always blames the importer.
- **A10 makes nominal mention part of the bindable contract.** Combined
  with A3 (`bindable ⇒ externally nameable`), A10(a) has the consequence that
  _any_ non-sink declaration mentioning a name nominally tied to a
  `pub(bind)` symbol becomes part of the bindable external surface. There is no
  legal "private node helper that names `I::v` for a `pub(bind)` index
  `I`" — such a helper would still participate in the merged IR after
  override, and the importer would have no name through which to re-bind
  it. This is intentional. An author who wants to keep an internal
  helper private has two options:
  - **Variant-free body** — refer to `s` only via plain refs (`@s`)
    or binders (`for p : s { … }`), never via nominally-tied names.
    The helper survives any override.
  - **Fix `s`** — declare `s` as `pub` (exported + fixed) or as private,
    not `pub(bind)`. Without the bindability promise, A10 does not fire
    on literal mentions in any declaration.

  Equivalently: marking `s` as `pub(bind)` is a commitment that every
  non-sink in-library declaration mentioning `s`'s nominal substructure
  is itself part of the bindable surface. The compiler enforces this;
  the author cannot opt out one declaration at a time.

- **A10(b) — sink kinds and the private-only carve-out.** Sink kinds
  (`plot`, `assert`, `figure`, `layer`) are inherently leaves: nothing
  in the language can depend on them. So a _private_ sink declaration
  is unreachable from any `pub` surface and is pruned from the merged
  IR when the file is used as a library. Under that pruning, literal
  mentions of `I::v` in a private sink body cannot orphan anything —
  they only run when the file is the main entry point, where no
  override happens. A10(b) therefore allows them. Public sink kinds
  (`pub plot`, `pub assert`, `pub figure`, `pub layer`) must still
  abstract over `s`, because they travel with the include and need to
  remain meaningful under any override — this is what enables an
  importer to reuse the plot/assert logic with a different concrete
  index.

  No analogous carve-out is granted to `node` / `const`. A reachability-
  based carve-out — "private node with no `pub` descendant" — would be
  _fragile_: adding a new `pub` consumer of the chain would silently
  invalidate a previously-valid file, with the error appearing at a
  distance from the edit. If a future need for "leaf-private node"
  arises, the right path is an explicit attribute the author opts into
  (so the contract is local and the error stays at the declaration
  site), not implicit reachability. See §9.

## 2.5 Properties guaranteed by the axioms

### P1 — Library-standalone validity

**Statement.** Every library file `F` is checkable for validity using
only A1–A10, without reference to any include site of `F`.

**Sketch.** Partition the axioms by what they constrain:

- **Per-declaration, file-local** (A1, A2, A3, A4, A5, A7, A9, A10):
  each is checkable from `F`'s source alone — they constrain the
  external role, attributes (V/B/R where applicable), kind eligibility,
  signature exportability, and
  nominal-mention propagation, all of which are syntactically present
  in `F`.
- **Compiler / model** (A6, nominal identity): a property of how the
  compiler tracks symbols, satisfied by construction.
- **Include-time only** (A8): fires only at an `include` site when an
  override is supplied. Does not constrain `F` in isolation.

The remaining concern is whether `F` could compile in isolation yet
still produce an include-site error A8 would catch. This requires
showing that A10 leaves no "unreconcilable" expressions in `F`:

- For declarations whose kind admits bindability (A10 case a):
  nominal mentions of a `pub(bind) s` force the declaration to be
  `pub(bind)` itself, so the importer can re-bind it under any
  override of `s`. A8's reconciliation path exists.
- For private sink-kind declarations (A10 case b): pruned from the
  merged IR at include time. A8 has no expression to fire on.
- For other kinds (A10 case c — `node` / `const`): variant-freeness
  is required, so no nominally-tied name appears in the merged IR.
  A8's trigger condition is never met.

In every case, A8 either has a reconciliation path or no trigger.
Hence library-side compliance with A1–A10 implies that any
include-site A8 firing reflects an importer error, not a latent
library bug. ∎

**Why this is a Property, not an Axiom.** P1 is derived from A1–A10;
it doesn't add independent constraint. Listing it as a separate
property (rather than as A11) keeps the axiom set minimal and makes
the standalone-validity guarantee explicit and citable. Any future
extension to A1–A10 must preserve P1 — that is the discipline this
section enforces.

## 3. Legal attribute states

For ordinary declaration kinds that admit annotation-controlled bindability,
A1–A4 collapse 2 × 2 × 2 = 8 combinations to **4 legal states**:

| V        | B        | R        | Meaning                               |
| -------- | -------- | -------- | ------------------------------------- |
| private  | fixed    | optional | file-local (default)                  |
| exported | fixed    | optional | exported, frozen                      |
| exported | bindable | optional | exported + overridable, has default   |
| exported | bindable | required | exported + overridable, must be bound |

Excluded (all four illegal combinations enumerated):

- V=private + B=bindable + R=optional → A3.
- V=private + B=bindable + R=required → A3.
- V=exported + B=fixed + R=required → A4.
- V=private + B=fixed + R=required → A3 _and_ A4 (either alone suffices).

Params are not a fifth visibility state in this table. They have the input-port
role, inherent bindability, and either optional or required `R` depending on
whether a default is present.

### 3.1 Per-kind required forms

For the three annotation-controlled bindable kinds plus param input ports, the
surface form for `R = required` is uniformly "no body":

| Kind    | required form                 | optional / with default                                            | External annotation/role                    |
| ------- | ----------------------------- | ------------------------------------------------------------------ | ------------------------------------------- |
| `param` | `param x: T;`                 | `param x: T = expr;`                                               | named input port; no visibility annotation  |
| `index` | `index I;` / `index I: Time;` | `index I = { … };` / `range(…)` / `linspace(…)`                    | bare / `pub` / `pub(bind)`                  |
| `dim`   | `dim D;`                      | `dim D = expr;` (and the non-bindable form `base dim D;`)          | bare / `pub` / `pub(bind)`                  |
| `type`  | `type T;`                     | `type T { C(...), U }`; unit marker: `type T { T }`                | bare / `pub` / `pub(bind)`                  |

Combined with A4, this means any required declaration must be bindable. A param
already is an input port; the other three kinds must carry `pub(bind)` (for
example, `pub(bind) index I;`, `pub(bind) dim D;`, or `pub(bind) type T;`).

**Grammar adjustments required to land this matrix** (vs. pre-axiom
grammar):

- **Add** `dim D;` form for required dimensions.
- **Repurpose** `type T;` to mean required type; move unit type to
  `type T {}` (an empty record).
- **Add** `base unit U: Dim;` form for canonical units (analogous to
  `base dim D;`); **tighten** `unit_decl` so non-base unit declarations
  always carry a body. The previous `unit USD: Money;` form is replaced
  by `base unit USD: Money;`.

These changes make "no body" mean `R = required` uniformly across the
three annotation-controlled bindable kinds and param input ports, removing the
`type T;` ↔ unit-type collision and the missing `dim` required form.

**Wrinkle within `dim` (and analogously `unit`).** `dim` is in the
bindable set per A5, but the `base dim D;` form declares a _new
fundamental dimension_ — rebinding it would be semantically incoherent.
`pub(bind) base dim D;` is therefore rejected. This is a per-form
constraint within a bindable kind, not a violation of A5. The `base
unit U: Dim;` form has the same character: it declares the canonical
unit, which by definition is the reference and is not subject to
override. (`unit` is not in the bindable set per A5 anyway, so the
constraint is vacuous for it — but worth stating for symmetry with
`base dim`.)

## 4. Surface syntax

The V/B annotation is `pub` or `pub(bind)` (or omitted) on the
declaration. Illustrated for `index` (the canonical example since all
three states are reachable):

```text
              index I = { … };    // private           (V=private, B=fixed)
pub           index I = { … };    // exported, fixed   (V=exported, B=fixed)
pub(bind)     index I = { … };    // exported+bindable (V=exported, B=bindable)
```

Same shape applies to `type` and `dim`. Reuses Rust's `pub(...)` shape,
leaving room for a future scoped-visibility extension to compose as
`pub(crate, bind)`, `pub(in lib, bind)`, etc.

For all kinds outside the bindable set (`node`, `const`, `unit`, `dag`,
sink kinds, …), only `pub` or bare are legal — there is no `pub(bind)`
form.

### 4.0 `param` declares an annotation-free input port

`param` is not an ordinary declaration with an implicit visibility default. It
selects the named-input-port role directly, analogous to a function parameter.
The role is externally nameable and bindable by definition, so a visibility or
bindability annotation would encode no additional state and is forbidden:

```text
param x: Length = 10 m;        // legal — input port with a default
param x: Length;               // legal — required input port

pub param x: …;                // ILLEGAL — `pub` is not the input-port spelling
pub(bind) param x: …;          // ILLEGAL — the port role already permits binding
```

Fixed or computed values use node declarations instead:

| Intended role                  | Declaration                             |
| ------------------------------ | --------------------------------------- |
| Private computed value         | `node x: T = expr;`                     |
| Private fixed value            | `const node x: T = expr;`               |
| Exported computed value        | `pub node x: T = expr;`                 |
| Exported fixed value           | `pub const node x: T = expr;`           |

The effective port value remains externally readable and projectable:

```text
include model().{ x as default_x };      // reads x's default
include model(x: 20 m).{ x as bound_x }; // reads the supplied value
node y: Length = @model().x;              // inline-call projection
```

Automatic projectability also preserves output compatibility when an eligible
`pub node x = default` is refactored into `param x = default`: callers retain
access to `x`, while gaining the option to bind it.

Selecting or re-exporting the effective value does not preserve the input-port
role through an intermediate DAG. That DAG must declare and forward its own
param if its callers should still bind the value.

If internal parameterization is needed, put the input port on a private DAG or
behind a private module boundary. Do not encode an "internal param" distinction
with a naming convention.

A1–A10 still apply to param ports through external nameability `N`, bindability
`B`, and requiredness `R`, while preserving the port role separately from
ordinary export visibility `V`.

### 4.1 Visibility on `include` and `import`

`include` and `import` are use-sites, not declarations — A5 says they
don't admit bindability (`B ≡ fixed`). The use-site itself also has no
V-axis. Bare/aliased forms create private qualified bindings; visibility
may be attached only to bindings explicitly named in a selective list.

```text
// Private qualified bindings
include pkg.container(Element: Inner) as c;
import pkg.types as types;

// Selective re-exports
include pkg.container(Element: Inner).{ pub origin, helper };
import pkg.types.{ pub dim Length, pub dim Mass, helper };
```

Only `origin`, `Length`, and `Mass` become public in this example. `helper`
and both qualified aliases remain private. `pub import ...`, `pub include
...`, and their `pub(bind)` variants are parse errors.

Selective public items feed A9 Case 2 (V006). The check ranges only over
signatures of explicitly `pub`-marked items after include substitutions.
This prevents a dependency's newly added public declaration/output from
silently widening a facade or introducing a new leakage error.

## 5. Canonical idioms

### 5.1 Injectable dependency over an index

```text
// lib.gcl
pub(bind) index Phase;                      // required, no fixed variants
param cost:   Dimensionless[Phase];
param weight: Dimensionless[Phase];

node weighted: Dimensionless[Phase] =
    for p : Phase { @cost[p] * @weight[p] };   // no Phase::X literal
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

The load-bearing property is **variant-freeness**: `weighted`'s body
contains no name nominally tied to `Phase` (no `Phase::v` literal).
Under index rebinding, `Phase` → `MyPhase` substitution rewrites the
`for` binder cleanly and nothing else needs to change. A10 is satisfied;
A8 is vacuous.

A reduction (e.g. summing across phases) would compose with a primitive
like `scan` if an unindexed result is wanted; the reduction primitive is
orthogonal to the injectability story — what matters is whether any
`Phase::v` literal appears in the kept expressions.

### 5.2 Variable conversion rate (currency)

Library models the rate as a `param`; the `unit` derives from it via a
parenthesised graph ref in the scale slot:

```text
// lib.gcl
base dim Currency;
unit usd: Currency;
param jpy_in_usd: Dimensionless = 0.01;
unit jpy: Currency = (@jpy_in_usd) usd;

// uses `100 jpy` freely — at eval, the literal resolves through the
// current value of jpy_in_usd
```

```text
// main.gcl
include "./lib.gcl"(jpy_in_usd: 0.0067);   // override the rate
```

The parens around `@jpy_in_usd` are required by the surface grammar
(`unit_scale = NUMBER | "(" expr ")"`). See
`tests/fixtures/valid/dynamic_units.gcl` for the canonical reference.

The dual form **`const unit`** marks a unit whose conversion factor does
_not_ depend on runtime values — useful for dim-check / const-folding
that needs to assume a fixed factor, and the right choice for units that
aren't part of a runtime-variable conversion graph.

**`const unit` does not change visibility/bindability semantics.** Both
`unit` and `const unit` declarations have `B ≡ fixed` (per A5 — `unit`
is not in the bindable set), and both participate in V composition (A9)
identically. The `const` marker is purely about whether the conversion
factor is computable at compile time; it has no interaction with the
axioms in this document.

## 6. Derived rules (V001–V006)

| Rule                                   | Derivation                                                      | Surface change vs. today                                                                                                                                                                                                  |
| -------------------------------------- | --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **V001** ImportPrivateItem             | A1                                                              | rename only — same rule under the new model                                                                                                                                                                               |
| **V002** RequiredMustBeBindable        | A4 (with A3)                                                    | required `index` / `type` / `dim` must be `pub(bind)`; required params are already input ports                                                                                                                             |
| **V003** PrivateInPublic               | A9 on written external signatures                               | applies to explicit export signatures and param input-port signatures                                                                                                                                                     |
| **V004** VariantLiteralOfBindableIndex | A10 (non-bindable case)                                         | re-stated: "variant literal of a `pub(bind)` index in a `node` / `const` / public-sink body". Does not fire for `param` (input ports are bindable; A10 case (a) trivially satisfied) or for private sink kinds (A10(b) carve-out). |
| **V005** IncludeMustReconcileOverride  | A8 at the include site                                          | new                                                                                                                                                                                                                       |
| **V006** GenericsLeakage               | A9 applied to `include` bindings of bindable `type`/`dimension` | new                                                                                                                                                                                                                       |

V005 corresponds to issue #444 §7. Under this formalisation it is keyed
off A8 directly; the prior staged narrowing (Steps 1–3) collapses
because A10 absorbs the non-bindable case at declaration time.

### 6.1 Design point foreclosed by V004

A11 / A10 / V004 collectively eliminate a state that an author might
intuitively want: _"this indexed table is exposed for downstream
readers but cannot be overridden, and its default names variants of a
`pub(bind)` index."_ Under the axioms such a state has no surface form:

- As `pub const node y: D[I] = { I::a: …, I::b: … }` — rejected by
  V004 (A10 case c, `const node` is not a bindable kind, body has
  literals).
- As `param y: D[I] = { I::a: …, I::b: … }` — legal, but `param`
  declares a bindable input port (see §4), so the importer _can_ override
  `y`. Not "fixed."
- Removing the literals (variant-free body) requires a constructor
  that doesn't name `I`'s variants, which is rare for a value of type
  `D[I]` populated from per-variant data.

The foreclosure is intentional, with the same root cause as A10's
contagion principle: any non-trivial structural dependence on a
`pub(bind)` symbol is part of the bindable contract. If the author
truly wants "fixed table, readable downstream," the right move is to
either (a) downgrade `I` from `pub(bind)` to `pub` (so V004 does not
fire on `pub const node y`) or (b) accept that exposing `y` as a
`param` carries the same overridability promise as exposing `I`.

## 7. Worked-case derivations

| #   | Scenario                                                                                                | Derivation                                                                                                            | Verdict                                                               |
| --- | ------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| 1   | `pub(bind)` index with literal-default `param`, no include                                              | A8 only fires under override; here there is none                                                                      | **OK** (V004 narrowed)                                                |
| 2   | Importer overrides index, omits `param` binding                                                         | A8: `param` default mentions `Phase::v` and was not re-bound                                                          | **V005**                                                              |
| 3   | Library declares `pub(bind) index Phase` and `node total = @cost[Phase::a] + @cost[Phase::b]`           | A10: `node` is non-bindable, body has `Phase::v` literals → forbidden at declaration                                  | **V004 at library compile time** (no include needed)                  |
| 4   | Dependency abstracts over `Phase` via `sum(p in Phase, …)`                                              | body mentions no name nominally tied to `Phase`; A8 vacuous                                                           | **OK**                                                                |
| 5   | `include pkg.container(Element: PrivateInner).{ pub origin };`                                         | A9 Case 2: `origin`'s effective signature names `PrivateInner` (V=private at importer)                               | **V006**                                                              |
| 6   | Required `index I;` declared without `pub(bind)` (bare → V=private)                                     | A4 (with A3) forbids the state — required forces bindable; an ordinary bindable declaration must be exported          | **V002**                                                              |
| 7   | `pub` declaration whose annotation names a private symbol                                               | A9 directly                                                                                                           | **V003**                                                              |
| 8   | Library uses `100 jpy`; importer wants different rate; library follows §5.2 idiom                       | importer binds `jpy_in_usd`; A8 trivially satisfied                                                                   | **OK**                                                                |
| 9   | Library uses `100 jpy`; library defines `jpy` directly with literal rate; importer wants different rate | A5 says `unit` is `fixed`; no override possible                                                                       | **rejected at include site** — library must refactor to §5.2 idiom    |
| 10  | Library declares `pub(bind) index I = {a,b}` and `pub const node y: D[I] = {I::a:1, I::b:2}`            | A10 case (c): `const node` is non-bindable, body has `I::v` literals → forbidden at declaration                       | **V004 at library compile time** (foreclosed design point — see §6.1) |
| 11  | Library declares `pub(bind) index I = {a,b}` and `param y: D[I] = {I::a:1, I::b:2}`                     | A10 case (a) trivially satisfied (a param input port is bindable). Importer overrides `I` and forgets to re-bind `y`. | **V005 at importer**                                                  |

## 8. What the axioms intentionally don't decide

- **Re-export semantics.** Resolved by the selective-only policy in §4.1:
  use-sites are private and only `{ pub items }` bindings are re-exported.
  V006 checks those explicitly selected signatures.
- **Scoped visibility.** A1 / A9 generalise by replacing `exported` with a
  lattice of export scopes while retaining input ports as a distinct role.
  `pub(bind)` syntax composes as
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
3. ~~**Required-ness for `index` / `type` / `dimension`.**~~ Resolved
   in §3.1: `dim D;` is added, `type T;` is repurposed (unit type moves
   to `type T {}`), and `base unit U: Dim;` is added (with `unit_decl`
   tightened). "No body" then means `R = required` uniformly across
   the three annotation-controlled bindable kinds and param input ports.
4. **Validation of the per-kind "nominally tied" table (§1).** Index
   (variant literals), dim (none — total algebraic substitution), and
   param (none) are settled. The `type` entry — constructors, field
   accesses, variant patterns — should be validated against the actual
   TIR / lowering passes before implementation; minor per-feature
   adjustments are still possible.
5. **V005 step ordering.** Issue #444 §7 originally staged V005 in three
   steps. Under A8 + A10, the staging is largely redundant; V005 can
   land as a single change keyed off A8.
6. **Explicit leaf-private attribute (deferred).** A future opt-in
   attribute (working name: `#[leaf_private]`) could let an author
   declare a private node / const as never reachable from any `pub`
   surface in the file. The compiler would enforce that no `pub`
   declaration depends on it, locally at the attributed declaration's
   site. With such an attribute, A10's body-of-`node`/`const` rule
   could be relaxed to allow nominally-tied mentions — analogous to
   the sink-kind carve-out, but opt-in rather than by-construction.
   Not part of the initial design; revisit if real demand appears.

## 10. Cross-reference to issue #444

| Issue #444 section       | Axiom(s) covering it                                 |
| ------------------------ | ---------------------------------------------------- |
| §3 two-axis model        | A1, A2, A3                                           |
| §4 per-kind matrix       | A5                                                   |
| §5 nominal typing        | A6                                                   |
| §6 V001–V006 restatement | derived rules in §6 above                            |
| §7 V005 spec             | A8 (and A10 for the originally-staged Step 3)        |
| §8 Rust analogues        | informs A7, A9                                       |
| §9 syntax options        | §4 above                                             |
| §10 required-ness        | A4                                                   |
| §11 non-goals            | A7 (no `impl Trait`); flat visibility (A1 unchanged) |
| §12 open questions       | §9 above                                             |

## 11. Settled design choices

These are the load-bearing choices the axioms encode. Each is the
outcome of an explicit design decision rather than a derivation from
something more primitive.

- **Bindable operations target param input ports plus exported bindable
  `{index, type, dimension}` declarations.** `unit` is deliberately excluded
  in favour of the §5.2 canonical idiom
  (rate-as-`param`, unit derives via `@`). `const unit` marks the
  runtime-independent case.
- **Surface syntax is `pub(bind)`** (Option B in #444 §9). Bare for
  private, `pub` for exported-only, `pub(bind)` for exported + bindable.
  Import/include use-sites carry no leading visibility; re-exports use only
  per-item `{ pub item }` selectors.
- **A8 covers both flavours.** Value-override (`param`) and nominal-
  rebind (`index`, `type`, `dimension`) reconcile under the same
  axiom; the resolver dispatches on the bound symbol's kind.
- **Library validity is standalone (P1, §2.5).** A library file is
  checkable in isolation. A10 is the axiomatic enforcement point; A8
  only fires for importer mistakes, never for library bugs. P1 is a
  derived property of A1–A10, not an additional axiom.
- **Indexes are nominal (A6).** Two indexes with identical variant names
  are distinct types. Override binding renames the index identifier;
  variants are never silently re-keyed across distinct indexes.
- **Bindability is contagious through nominal mention.** Marking a
  symbol `pub(bind)` commits every non-sink in-library declaration that
  names its nominal substructure (variants, constructors, base-dim
  refs, …) to also be part of the bindable external API surface.
  Authors who want to keep helpers private must either keep them
  variant-free or downgrade `s` from `pub(bind)`. See the A10 note in
  §2 for the full statement.
- **Sink kinds get a private-only carve-out.** Private `plot` /
  `assert` / `figure` / `layer` may mention nominally-tied names of
  `pub(bind)` symbols freely; they are pruned from the merged IR when
  the file is used as a library. Public sink kinds must abstract over
  the bindable symbol so importers can reuse the plot/assert logic
  under any override. No reachability-based carve-out is granted to
  `node` / `const` — the explicit-attribute path (§9) is the deferred
  alternative.
- **Required forms uniformised across bindable kinds.** "No body"
  means `R = required` for `param`, `index`, `dim`, and `type`. Three
  grammar adjustments make this consistent: add `dim D;`, repurpose
  `type T;` (unit type moves to `type T {}`), and add `base unit U: Dim;`
  while tightening `unit_decl` to require bodies on non-base units.
  See §3.1.
- **`param` is an annotation-free named input port.** It is not an ordinary
  declaration with implicit `pub` visibility, but its effective bound/default
  value is externally readable and projectable. `pub param` and
  `pub(bind) param` are illegal because the declaration kind already encodes
  the only meaningful input-port state. Internal computed/fixed values use
  `node` / `const node`; internal parameterization uses a private DAG or module
  boundary. See §4.0.
- **A10 inspects body/default only, not type annotations.** Sufficient
  because the language enforces explicit construction: any value of a
  generic / indexed type whose annotation mentions a `pub(bind)` symbol
  must be built via a turbofish-bearing constructor, a literal that
  names variants/fields, or a plain `@`-ref. The first two land
  nominal mentions in the body where A10 catches them; the third is
  not nominally tied (per §1). A "signature-only" mention is therefore
  structurally rare-to-impossible, and extending A10 to type
  annotations would catch zero new bugs.
- **Include consumes bindability — transitive includes are concrete.**
  An include statement resolves all bindings at that site (overrides
  supplied or defaults applied). The result is concrete from any
  further consumer's perspective. To expose a dependency's bindable
  surface to a transitive consumer, an intermediate file must
  re-declare its own bindable proxies and forward them in its include.
  See §2 elaboration for details.
- **Bindable kinds split into two camps (§1).** `index` and `type` have
  partial substitution under override (variants / constructors can fail
  to exist), so A8 and A10 fire on nominally-tied mentions. `dim` and
  `param` have total substitution (algebraic / value), so A8 and A10
  are vacuous for their references — bindability functions like
  reactive value-override. This is why `pub(bind) dim D1` followed by
  `dim D2 = Length * D1` requires no further constraint on D2: under
  override of D1, D2 just becomes `Length * NewD1` automatically.
- **Include consumes bindability.** When file A includes file B with
  some set of bindings (possibly empty, in which case B's defaults
  apply), B's bindable surface is _consumed_ at that include statement.
  The result of A's include is a concrete instantiation of B from any
  further consumer's perspective. If file C then includes A, C cannot
  reach into B's slot to re-override anything — B's bindings are
  resolved at A's site, period. Concretely: `include "./A.gcl"(b.Phase: …)`
  is not legal syntax; there is no path-qualified binding form.

  If A wants to expose B's bindable surface to C (so C can pick the
  values that B will see), A must do it explicitly: declare its own
  `param`s / `pub(bind) index`es / etc. at A's level, and forward them
  to B in A's `include` statement. This is the manual re-declaration
  pattern. It is verbose but legible, and matches the language's
  "explicit over implicit" stance — the bindable surface nameable at
  any include site is exactly what the importer can read from the
  file's declarations.

  Consequence for A8's "in the same include": there is at most one
  include statement involved in any override, and A8's re-bind
  obligation lives there.
