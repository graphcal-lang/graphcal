# `graphcal check` false-accept findings

> [!WARNING]
> This content was written by an AI agent and must be verified by a human
> developer. After human verification, this alert may be removed.

Results of a negative-oracle fuzzing campaign against `graphcal check`
(see `../README.md` for the harness). The campaign generated ~3,400
high-entropy `.gcl` files per run across 185 probe families, each probe
embedding exactly one construct that the language documentation says must be
a compile-time error, paired with a minimally-different control file that
must pass. Probes that *pass* `check` are false accepts; every survivor
below was triaged by hand against the docs and the compiler source.

Final run (seed 20260818): 3,378 files checked — 1,922 expected-fail probes
(1,874 correctly rejected), 1,456 expected-pass controls (1,454 correctly
accepted). The 48 surviving probes reduce to the findings below.

Filed issues: [#1413](https://github.com/graphcal-lang/graphcal/issues/1413)
(Bug 1), [#1414](https://github.com/graphcal-lang/graphcal/issues/1414)
(Bug 2), [#1415](https://github.com/graphcal-lang/graphcal/issues/1415)
(minor), [#1416](https://github.com/graphcal-lang/graphcal/issues/1416) and
[#1417](https://github.com/graphcal-lang/graphcal/issues/1417) (divergences).

## Bug 1 — `include` accepts unbound required params, then eval hits an internal error

**Files:** `include_unbound_required_param.gcl`,
`include_unbound_partial.gcl`, `include_unbound_pkg/` (cross-file).

```gcl
dag scale {
    param factor: Dimensionless;      // required: no default
    node out: Dimensionless = @factor * 2.0;
}
include scale().{ out };              // factor never bound
```

- `graphcal check` → **passes**.
- `graphcal eval` → `X001 internal error: TIR runtime declaration missing
  for `…<include@…>.factor``.
- `docs/language/computation-model.md` (Parameters): “A param without a
  default is **required**. Leaving a required port unsatisfied is a compile
  error.”
- The equivalent expression-form call `@scale(factor: 3.0).out` **is**
  correctly rejected (`G004 missing required binding(s)`), raised in
  `crates/graphcal-compiler/src/tir/dim_check/infer/hir.rs` (~line 5215).
  The `include` declaration path never performs that completeness check; the
  hole survives to TIR, where
  `DeclarationIndexError::MissingBinding` is mapped to
  `GraphcalError::InternalError` in
  `crates/graphcal-compiler/src/tir/typed.rs` (~line 1890).

**Suggested fix:** validate include bindings against the target DAG's
required params exactly as the inline-call path does, reporting `G004` at
the include site. Applies to same-file `dag` includes and cross-file module
includes alike.

## Bug 2 — `base unit` registers on any dimension with no validation

**Files:** `base_unit_second_canonical.gcl`, `base_unit_on_prelude_dim.gcl`,
`base_unit_on_derived_dim.gcl`, `base_unit_on_temperature.gcl`.

`docs/language/dimensions-and-units.md` defines the form as: “A `base unit`
declaration … defines **the** canonical unit for a **user-defined base
dimension**.” The implementation
(`crates/graphcal-compiler/src/ir/lower.rs`, ~lines 3886–3908) registers any
definition-less `base unit` with scale 1.0 unconditionally:

1. **Second canonical unit** — `base dim Information; base unit bit: …;
   base unit nat: …;` both get scale 1.0, so `1.0 bit == 1.0 nat` evaluates
   `true` and `-> nat` is an identity relabel. Two distinct units silently
   identified — the exact silent-unit-aliasing failure mode the language
   exists to prevent.
2. **Prelude base dimension** — `base unit furlong: Length;` makes
   1 furlong ≡ 1 m (eval prints `5 furlong` → `5 m`).
3. **Derived dimension** — `base unit kt: Velocity;` makes 1 kt ≡ 1 m/s;
   the docs' “canonical unit” concept does not even apply to non-base
   dimensions.
4. **Temperature** — `const unit c: Temperature = 1.0 K;` is rejected with
   `D014` (affine-prone), but the guard in `ir/lower.rs` (~line 3815) is
   conditioned on `u.definition.is_some()`, so the definition-less
   `base unit celsius: Temperature;` bypasses it entirely.

**Suggested fix:** in `lower_unit_decl`, require that a definition-less
`base unit`'s dimension (a) resolves to a single user-declared base
dimension with exponent 1, and (b) does not already have a canonical unit
(prelude SI symbol or a previous `base unit`). That subsumes case 4.

## Minor — `+/- -0.0` literal tolerance evades A015

**File:** `tolerance_negative_zero.gcl`. The A015 check
(`crates/graphcal-compiler/src/tir/dim_check/mod.rs`, ~line 778) uses
`value < 0.0`, which is false for IEEE `-0.0`, so the negatively-signed
literal `-0.0 m` is accepted while `-0.1 m` is rejected. Semantically it
equals zero tolerance, so the impact is only that the documented
“literal negative tolerance is a compile error” has a sign-of-zero
exception.

## Doc/implementation divergences (not check false-accepts)

Found by the same campaign; the implementation disagrees with the docs but
the accepted programs are deliberate (fixtures/tests bless them), so these
are documentation bugs rather than missing validations:

1. **Param defaults may reference the graph.** `param b: Dimensionless =
   @a * 2.0;` passes check, evaluates, and re-evaluates reactively under
   `--set a=…`; cycles through defaults are caught by G001. Fixtures
   (`tests/fixtures/valid/indexed.gcl:66`, `power_budget.gcl:97`, …) and
   `tir/dim_check/tests.rs:230` rely on this. But
   `docs/language/computation-model.md`'s “Where `@` Is Allowed” table says
   `param` default values allow no `@`. The table (and the param-default
   policy in `tir/typed.rs` ~line 1338, which checks defaults as
   `BodyPhase::Runtime`) should be reconciled one way or the other.
2. **`%` rejects real quantities.** `docs/language/type-system.md` and
   `expressions.md` say `a % b` accepts “real quantities or integers” with
   matching dimensions, but `1.0 s % 2.0 s` is rejected with
   `D001 dimension mismatch: expected Int, found Time % Time`.

## Campaign notes

- Probe families with the largest correctly-rejected counts confirm the
  checker is strong across dimensions, numeric kinds, indexes, ADTs,
  generics, datetimes, complex numbers, conversions, includes, and
  attributes; the survivors cluster in exactly two validation gaps plus one
  sign-of-zero edge.
- Controls caught several over-strict-vs-docs behaviors worth knowing:
  visibility rules require `pub` for params' signature types/indexes (V003)
  and for expression-form DAG-call projections (V001) — while `include`
  projections of same-file private nodes are allowed; and the P015 nesting
  cap rejects expressions nested deeper than ~50 parens.
