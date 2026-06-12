# `->` operator stress test, round 2

Follow-up to the first screening in issue #648. Same scope (the `->`
conversion/display operator), new attack surface: runtime scale resolution,
formatter round-trips, module scoping, exponent/nesting limits, JSON output,
and timezone display. Verified at commit `aa19c30` (release build,
`graphcal eval` unless noted).

## TL;DR

The implementation is unchanged in character since #648: `Convert` is an
evaluator passthrough (`crates/graphcal-eval/src/eval_expr/hir_eval.rs:116`)
and only `attach_display_units` (`crates/graphcal-eval/src/eval/display.rs:14`)
gives `->` an observable effect. All #648 findings still reproduce at HEAD.
This round found **four new bugs** — the worst being a class of *silent
display-unit fallbacks* when target-scale resolution fails at runtime, and a
**formatter internal error** on parser-accepted input — plus several places
where the defenses held up well.

## New findings

### N1. Display unit silently dropped when the target's scale fails to resolve at runtime

`resolve_display_unit_scale` discards resolution errors
(`crates/graphcal-eval/src/eval/display.rs:124` — `resolve_unit_scale(...).ok()`),
so any conversion target whose *dimension* checks out but whose *scale*
cannot be computed silently renders in the base unit, exit code 0.

Two reproductions:

**(a) Dynamic unit with non-positive scale.**

```gcl
pub base dim Money;
pub base unit USD: Money;
param rate: Dimensionless = 1.08;
unit EUR: Money = (@rate) USD;
param price: Money = 100.0 USD;
node price_eur: Money = @price -> EUR;
```

```
$ graphcal eval f.gcl --set rate=0.0   # exit 0
price_eur = 100 USD                    # -> EUR silently ignored
```

The asymmetry is the damning part: if the same program contains a `1.0 EUR`
*literal*, the identical `rate=0.0` override fails loudly
(`ERROR: dynamic unit scale must be greater than zero, got 0`, exit 1).
Whether an invalid unit is a hard error or a silent no-op depends on whether
it appears left or right of `->`.

**(b) Static compound target whose scale overflows.**

```gcl
unit big: Length = 1.0e200 m;
param a: Length = 1.0 m;
node b: Length = @a -> big*big/m;   // dimension Length — passes D006
```

`big*big/m` has scale `1e400 = inf`; `checked_positive_finite_unit_scale`
rejects it, `.ok()` swallows the rejection, and the output is `b = 1 m` with
no diagnostic. `graphcal check` is also clean. In JSON output the value
quietly loses its `display_value` field while keeping `"unit": "m"`.

For a language whose stated bar is "errors over silence" this is the same
class as #648's B1, but triggered by runtime data rather than AST position.
Suggested fix: propagate the error (it already carries a span) instead of
`.ok()`, or at minimum surface a runtime warning in the output.

### N2. Formatter internal error on parser-accepted `(expr -> u) -> v`

#648's B2 (parenthesized chaining bypasses the non-chaining rule) compounds
with the formatter: `format_expr` for `Convert`
(`crates/graphcal-fmt/src/format/expr.rs:66`) prints the inner expression
without parens, producing the illegal chain `@a -> m -> cm`, which then fails
the formatter's own re-parse validation:

```
$ graphcal format f.gcl
error: f.gcl: internal formatter error: formatted output did not re-parse:
unexpected token `->`
```

The safety net works (the file is left untouched, exit 1), but `format` is
hard-broken on input the parser accepts. Fixing B2 by rejecting nested
`Convert` in the dim-checker would resolve this for free; otherwise the
formatter must parenthesize a `Convert` inner operand.

### N3. `-> u^0` is accepted and renders an empty unit label

```gcl
param a: Dimensionless = 5.0;
node b: Dimensionless = @a -> m^0;
```

`m^0` dim-checks as `Dimensionless`, so the conversion is accepted; the
canonical label formatter then drops the zero-power term entirely:

```
b = 5␣        # trailing space, empty label
```

JSON output emits `"unit": ""`. On a non-dimensionless operand the `^0` is
caught by D006, so the fix is narrow: reject zero exponents in `unit_term`
at parse/desugar time (a unit raised to 0 is never meaningful as a target).

### N4. Canonical display labels are not round-trippable as source

```gcl
node fm: Frequency = @f -> min^-1;
```

prints `fm = 120 1/min` — but `1/min` is exactly the syntax #648's U2
documents as a parse error in target position. The pretty-printer
(`format_unit_expr_canonical`) speaks a richer unit grammar than the parser
accepts, so copying a displayed unit back into source fails. Either accept
`1/unit` in `unit_expr` (closing U2) or stop emitting it.

### N5. Timezone display has the same access-site drop as B1

```gcl
param t: Datetime = datetime("2026-01-15T12:00:00Z");
node a: Datetime = @t -> "Asia/Tokyo";
node b: Datetime = @a;
```

```
a = 2026-01-15T21:00:00+09:00[Asia/Tokyo]
b = 2026-01-15T12:00:00 UTC
```

#648's B1 was demonstrated for scalars; `display_tz` follows the identical
construction-site-only pattern, so any B1 fix should cover `Datetime` too.

### N6. Module alias does not namespace units; qualified units unparseable after `->`

With `import app.units as u;` (where `units.gcl` declares `pub unit mile`),
the conversion target `-> mile` resolves *bare*, ignoring the alias, while
the alias-consistent `-> u.mile` is a parse error (`unit_term` is IDENT-only,
no `ident_path` like `dim_term` has — #648's U6 again). Node references made
through the same import require `@u.…`. Units are the only imported category
that escapes its module alias; that inconsistency will bite the first project
with two `mile` definitions.

### N7 (cosmetic). Misleading D001 help text for non-scalar, non-indexed operands

`@n -> km` on `Int`/`Bool` reports "expected a scalar value, not an indexed
value or struct" — the operand is neither. The help string is hard-wired to
the indexed/struct case.

## #648 findings — status at HEAD

All reproduce unchanged: **B1** (display unit lost at access sites: DAG
projection, struct field, index access — also `const node` access, same
shape), **B2** (paren chaining accepted; now also breaks `format`, see N2),
**B3** (Convert silently inert in arithmetic / `if` / `match` / aggregation
argument position), **B4** (D006 prints `Length^2 * Mass / Time^2` instead
of `Energy`), **U1** (no element-wise `->` on indexed values), **U2**
(`1/min` rejected — now with the N4 asymmetry), **U3** (no fractional
exponents), **U4** (affine `unit C: Temperature = 1.0 K` still silently
wrong: `300.0 K -> C` prints `300 C`), **U5** (LSP completion has no
after-`->` context).

Partial progress since #648: `attach_display_units` now distributes
conversions through map-literal entries (per-entry, mixed targets work),
for-comprehension bodies, and `scan`/`unfold` inits — so the *construction*
side of indexed display is in good shape; only the operator's own indexed
form (U1) is missing.

## What held up (negative results)

- **Exponent abuse**: `m^2147483648`, `m^-2147483649`, 26-digit exponents,
  `m^1.5` → all clean P003 "expected integer"; `m^(2)`, `m^2^3` → P001.
- **Nesting/size**: 5 000 nested parens in a target → P015 depth limit
  (though the diagnostic echoes the entire 5 000-char line); a 100 000-term
  `m * s / s * …` target parses and evaluates in ~80 ms.
- **Token confusion**: `-> 5`, `-> -m`, `-> dag` (keyword) → clean P001s.
- **Dimension zero-power**: `Length -> m^0` correctly D006.
- **Top-level positions**: `if`/`match`/`sum(...)`/`max(...)` followed by
  `-> u` all convert correctly (workaround for B3 exists: lift the convert
  to top level); `@a + 500.0 m -> km` correctly wraps the whole sum.
- **Dimensionless ratios**: `@x -> km/m` on a `Dimensionless` value legally
  displays `0.005 km/m` — surprising but consistent and correct.
- **Equivalent targets normalize**: `m/s/s`, `m/(s*s)`, `m*s^-2` all label
  as `m/s^2` with identical values.
- **Precision**: `0.1 km -> m`, `3.0 inch -> inch` round-trips display clean
  (no `100.00000000000001`-style noise; `format_number` rounding holds).
- **CLI integration**: `--set 'a=(200.0 m -> km)'` works including display;
  assertions with `->` operands pass/fail correctly (values stay SI).
- **Misc**: unit shadowing of builtins → N009; unknown timezone strings →
  compile-time D007 (`Etc/GMT-14` and other valid-but-odd IANA names fine);
  non-ASCII unit identifiers rejected explicitly (P006); forward references
  to units declared later in the file work; `format` leaves files untouched
  when its re-parse check fails.

## Numbat comparison notes

`->` originates in Numbat, but the two have diverged in kind, and several
graphcal pain points trace to that divergence:

1. **Numbat's `->`/`to` is a true value conversion** — the result *is* a
   quantity in the target unit, so conversion is meaningful in any
   expression position and chaining is coherent. Graphcal's display-only
   semantics is a legitimate, arguably safer choice (values never leave SI),
   but it *creates* the positional-silence class (B1/B3/N1): an operator
   that only sometimes does anything. If display-only stays, the missing
   piece is a diagnostic — "conversion has no display effect here" — for
   Converts in discarded positions, mirroring how the language already
   refuses bare chaining.
2. **Numbat refuses to model temperatures as multiplicative units**; Celsius/
   Fahrenheit exist only as functions. That is direct support for
   U4 option (b): reject linear `unit … : Temperature` definitions (or any
   dimension flagged as affine) instead of silently computing nonsense.
3. **Numbat's conversion target is a full expression** (any quantity of
   matching dimension, including `1/min`-style reciprocals), with one
   grammar shared between declarations and targets. Graphcal's separate
   IDENT-only `unit_expr` is the common root of U2, U6, N4, and N6 — unify
   the target grammar with `dim_expr`'s shape (ident paths, parens,
   reciprocals) and four findings collapse into one fix.
4. **Numbat treats the `->` RHS as a general "display directive"** (units,
   but also `bin`/`hex`/`oct`). Graphcal already has two RHS kinds (unit
   expr, timezone string) implemented as two unrelated AST nodes behind one
   operator. Naming that concept — a `DisplayDirective` enum — would give
   N5's fix and future directives (significant figures?) one home.

## Suggested priority

1. **N1** — replace `.ok()` at `display.rs:124` with error propagation; it
   is the only finding where wrong-looking output ships with exit code 0.
2. **N2 + B2** — enforce non-chaining semantically (reject nested `Convert`
   in `infer_hir_convert`); the formatter crash disappears with it.
3. **B1 + N5** — persist `display_unit`/`display_tz` through value reads.
4. **N3/N4/N6** — unit-grammar cleanups, foldable into the U6 parser
   unification.
