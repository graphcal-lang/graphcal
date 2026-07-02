# Graphcal Language Design Review — Inconsistency Findings

*Review date: 2026-07-02. Reviewed at commit `16762f4` on `main`.*

## Scope and method

This report reviews Graphcal **as a programming language design**: the language as specified by the user-facing docs (`docs/`), the formal grammar (`grammar.ebnf`, the stated source of truth), and the language as actually implemented (`crates/graphcal-compiler`, `crates/graphcal-eval`). Three passes were made:

1. **Docs cross-read** — every page of `docs/language/`, `docs/tutorial/`, and `README.md`, checked against each other and against the stated design philosophy (*safety over usability, explicitness over implicitness; no implicit conversion / inference / broadcasting*).
2. **Grammar diff** — `grammar.ebnf` compared production-by-production against the lexer/parser (`crates/graphcal-compiler/src/syntax/`) and the syntax shown in docs.
3. **Semantics check** — the type checker (`tir/dim_check/`), evaluator (`graphcal-eval/src/eval_expr/`), prelude (`registry/prelude.rs`), and test suites, checked for divergence from docs and for internal asymmetries.

Every finding cites both sides of the inconsistency with file paths (and line numbers as of the reviewed commit). Severity legend:

- **Major** — a contradiction that makes documented programs fail, breaks a stated core principle, or makes the grammar unable to describe the language.
- **Moderate** — a real design or spec/implementation divergence that will confuse users or tooling authors.
- **Minor** — naming, wording, or convention inconsistencies; paper cuts.

### Headline findings

| # | Finding | Severity |
|---|---------|----------|
| 2.1 | The flagship examples (README, docs index, tutorial) redefine prelude dimensions/units and are rejected by the compiler | Major |
| 1.1 | "No implicit broadcasting" is enforced for arithmetic but comparisons broadcast implicitly — including scalar→indexed | Major |
| 3.1 | String literals are used throughout the language but do not exist in the formal grammar | Major |
| 3.2 | The grammar declares `scan`/`unfold`/`linspace`/`step` to be contextual keywords; the lexer reserves them as hard keywords | Major |
| 2.2 | Multi-axis aggregation type-checks but always fails at runtime, contradicting the documented signature | Moderate |

---

## 1. Violations of the stated design philosophy

### 1.1 Comparisons implicitly broadcast over indexed values — arithmetic doesn't (Major)

The type system documentation presents "No Implicit Broadcasting" as a core safety decision:

> *"Arithmetic on indexed values requires explicit `for`. This is a deliberate safety decision: … This prevents the class of silent broadcasting bugs common in NumPy and Excel, where mismatched shapes are silently resolved."* — `docs/language/type-system.md` §"No Implicit Broadcasting" (lines 430–444)

The implementation enforces this for `+ - * / %`: both operands of a binary arithmetic op go through `expect_scalar`, which rejects `Indexed` types at compile time (`crates/graphcal-compiler/src/tir/dim_check/infer/rules.rs:293-320`, error text in `infer/helpers.rs:216`) and again at runtime (`crates/graphcal-eval/src/eval_expr/hir_eval.rs:307-313`).

But **comparison operators broadcast implicitly** — element-wise over two indexed operands, and a bare scalar is even splatted across an indexed operand:

```rust
// crates/graphcal-compiler/src/tir/dim_check/infer/rules.rs:55-82 (comparison_axes)
let axes = match (lhs_axes.is_empty(), rhs_axes.is_empty()) {
    (_, true) => lhs_axes,
    (true, false) => rhs_axes,   // scalar side broadcasts to the indexed side
    ...
```

with the matching runtime broadcast in `crates/graphcal-eval/src/eval_expr/arithmetic.rs:93-188` (`broadcast_comparison`, including a dedicated indexed-vs-scalar arm). This is documented as intended behavior:

> *"Comparisons broadcast element-wise over indexed operands: `T[I] op T[I]` zips the two collections per key, and `T[I] op scalar` applies the scalar to every key — both return `Bool[I]`."* — `docs/language/expressions.md:61-64`

**Why it's inconsistent:** `@a + @b` on two `Velocity[Maneuver]` values is a compile error that demands an explicit `for`, while `@a < @b` — and even `@a < 3.0 km/s` — silently broadcasts. The same operand shapes get the opposite policy, and the broadcast comparison reintroduces exactly the bug class ("mismatched shapes silently resolved") that `type-system.md` names as the reason the feature is forbidden. Either broadcasting is a footgun (then comparisons shouldn't do it) or it's fine (then arithmetic's restriction loses its stated rationale).

### 1.2 `sign(x)` documents a NaN result, violating the no-NaN guarantee (Moderate)

The language repeatedly promises NaN can never appear as a value:

> *"Non-finite literals, invalid unit scales, empty indexes, and out-of-range numeric conversions are rejected instead of silently producing `NaN`, `inf`, or saturated integers."* — `README.md:39`
> *"Scalar operations that would create `NaN` or `inf` are surfaced as errors instead of producing a runtime value."* — `docs/language/expressions.md:45-46`

Yet the built-in reference documents:

> *"`sign(x)` | `D -> Dimensionless` | Sign of value (1.0, -1.0, or NaN)"* — `docs/language/built-ins.md:29`

**Why it's inconsistent:** a built-in whose *documented* codomain includes `NaN` contradicts the safety invariant stated in three places. It is also internally impossible: since non-finite values are rejected everywhere upstream, `sign` can never receive the NaN input that would produce a NaN output (and `sign(0.0)` is `0.0` in IEEE semantics, which the doc doesn't mention either). The signature line is wrong on both counts.

### 1.3 "Inference" as a headline feature vs. "no implicit type inference" (Minor)

The project philosophy states there is *no implicit type inference*, and indeed all declarations require type annotations. But the reference docs headline two sections with the word: **"Generic Type Inference"** (`docs/language/type-system.md:772`, unification of generic parameters from argument types) and **"Dimension Inference"** (`docs/language/dimensions-and-units.md:191-204`, result-dimension computation for operators). `docs/language/assertions.md` similarly says comparisons *"infer `Bool[I]`"*.

**Why it's inconsistent:** these are result-type *propagation*, not declaration inference, so the behavior itself is arguably compatible with the philosophy — but branding them "inference" makes the documentation read as contradicting the language's own selling point. A wording pass ("result dimension computation", "generic parameter unification") would reconcile it.

---

## 2. Documentation vs. implementation divergence

### 2.1 The flagship examples don't compile: prelude dimensions and units are redefined everywhere (Major)

The prelude defines not only the 8 base dimensions but also the derived dimensions:

```rust
// crates/graphcal-compiler/src/registry/prelude.rs:15-33
pub const PRELUDE_DIMENSION_NAMES: &[&str] = &[
    "Length", "Time", "Mass", "Temperature", "ElectricCurrent", "Amount",
    "LuminousIntensity", "Angle",
    "Velocity", "Acceleration", "Force", "Energy", "Power", "Frequency",
    "Pressure", "Area", "Volume",
];
```

Redefining any of these is a hard compile error — the resolver rejects shadowing of prelude names (`crates/graphcal-compiler/src/ir/resolve/mod.rs:89-127`, `GraphcalError::BuiltinNameShadowed`), and the test suite pins this behavior explicitly:

```rust
// crates/graphcal-compiler/src/ir/resolve/tests.rs:132-141
fn resolve_rejects_builtin_dimension_shadowing() {
    let err = parse_and_resolve("dim Velocity = Length / Time;").unwrap_err();
    assert!(matches!(err, GraphcalError::BuiltinNameShadowed { name, .. } if name == "Velocity"));
}
fn resolve_rejects_builtin_unit_shadowing() {
    let err = parse_and_resolve("unit m: Length = 1.0 m;").unwrap_err();
    ...
```

Yet `dim Velocity = Length / Time;` — the exact string the test asserts is an error — is the *opening example* of the language:

- `README.md:12-13` — `dim Velocity = Length / Time; dim Acceleration = Length / Time^2;`
- `docs/index.md` — same rocket example.
- `docs/language/dimensions-and-units.md:34-40` — the "Derived Dimensions" section itself teaches `dim Velocity = …; dim Force = …; dim Energy = …;` as user code.
- `docs/language/functions.md:18`, `docs/tutorial/step2-dimensions-and-units.md`, `docs/tutorial/step3-structs-and-blocks.md`, `docs/tutorial/step5-multi-file-projects.md:68` — all redeclare `Velocity` and friends.
- `docs/tutorial/step2-dimensions-and-units.md:88-90` additionally defines `const unit hour: Time = 3600.0 s;` — but `hour` is a prelude unit (`registry/prelude.rs:39-42`), so this is rejected by the same rule.

Meanwhile `docs/language/built-ins.md:163-175` ("Prelude Derived Dimensions") correctly documents that `Velocity`, `Acceleration`, `Force`, `Energy`, `Power`, `Pressure`, `Frequency`, `Area`, `Volume` are already provided.

**Why it's inconsistent:** the docs disagree with each other (built-ins.md says these dimensions exist in the prelude; dimensions-and-units.md presents defining them as the normal workflow), and the most prominent examples in the project — README, docs landing page, tutorial — are programs the compiler rejects. Per `docs/language/type-system.md` "Name Universes" (one leaf name per universe per scope) and the shadowing rule, there isn't even a permissive reading. Either the prelude should not pre-define derived dimensions, or every example needs the `dim`/`unit` redefinitions removed.

### 2.2 Multi-axis aggregation type-checks, then always fails at runtime (Moderate)

The checker peels exactly one index layer off an aggregation argument and returns the element type unconditionally:

```rust
// crates/graphcal-compiler/src/tir/dim_check/infer/hir.rs:601-608
if let InferredType::Indexed { element, .. } = arg_type {
    return Ok(match kind {
        AggregationFn::Count => InferredType::Scalar(Dimension::dimensionless()),
        AggregationFn::Sum | AggregationFn::Min | AggregationFn::Max
        | AggregationFn::Mean => *element,
    });
}
```

For `sum(@grid)` with `grid: Velocity[R, C]`, `*element` is `Velocity[C]` — still indexed — and the program type-checks with result type `Velocity[C]`. But the evaluator requires every entry to be a scalar (`crates/graphcal-eval/src/eval_expr/aggregations.rs:37-52`, `scalar_entry` → `expect_scalar("sum element")`), so evaluation always fails. The documentation admits only the scalar form:

> *"`sum(...)` | `D[I] -> D`"* — `docs/language/built-ins.md:143-147`

**Why it's inconsistent:** a three-way divergence — the checker accepts a shape the evaluator can never execute and the docs say doesn't exist. In a language whose pitch is "fails at compile time, not in flight" (`README.md:8`), a well-typed program with a guaranteed runtime type error is a design hole. Either the checker should require a scalar element (or recursively aggregate), or the evaluator/docs should support axis-peeling aggregation.

### 2.3 Fractional literal exponents: docs say any numeric literal works; only `0.5` does — and runtime exponents are *more* permissive (Moderate)

The docs state the exponent rule as:

> *"The exponent in `^` must be compile-time-known … In practice that means a numeric literal (integer or float, optionally with a leading unary `-`)"* — `docs/language/expressions.md:43`

The implementation accepts integer-valued float literals and exactly `0.5`; every other fractional literal (`0.25`, `1.5`, …) is rejected with — of all things — `NonLiteralExponent`:

```rust
// crates/graphcal-compiler/src/tir/dim_check/infer/rules.rs:407-425
if n == 0.5 { /* dimension exponent halved */ }
else { Err(GraphcalError::NonLiteralExponent { ... }) }   // e.g. x ^ 0.25
```

Meanwhile, a **non-literal, runtime-valued** exponent is accepted whenever base and exponent are dimensionless (`rules.rs:443-452`), and the evaluator computes `l.powf(r)` (`crates/graphcal-eval/src/eval_expr/arithmetic.rs:397`).

**Why it's inconsistent:** three problems compound. (a) The docs promise any float literal; the checker accepts two shapes of float literal. (b) A *more precisely known* exponent is *less legal*: `x ^ 0.25` (literal) is a compile error while `x ^ @quarter` (runtime value 0.25) compiles and runs — inverted from the "compile-time-known" rationale. (c) The error name calls a literal "non-literal". Since dimensions already support arbitrary rational exponents (`Length^(1/2)` syntax exists, and `Dimension` stores `Rational` exponents), the 0.5-only carve-out looks arbitrary; for dimensionless bases there is no dimension-resolution obstacle at all.

### 2.4 Tolerance operand: docs promise full arithmetic; grammar/parser allow only a unary expression (Moderate)

`docs/language/assertions.md:83-85` says of `assert name = actual ~= expected +/- tolerance;`:

> *"All three operands are arbitrary expressions. They can reference `@param`, `@node`, constants, call functions, and use arithmetic -- anything valid in a `node` expression."*

But the grammar restricts the third operand to `unary_expr` (`grammar.ebnf:493-495`: `expr, "~=", expr, "+/-", unary_expr, [ "%" ]`), and the parser matches (`crates/graphcal-compiler/src/syntax/parser/decl/value.rs:96-97`: *"Parse tolerance as a unary expr (not full expr) so `%` isn't consumed as modulo"*).

**Why it's inconsistent:** `+/- @expected * 0.05` does not parse as documented (the `* 0.05` is outside the tolerance operand), while `actual` and `expected` really are full expressions — an asymmetry among the "three operands" the docs claim are equal. The `%`-ambiguity motivation is real, but the docs don't disclose the restriction.

---

## 3. Grammar vs. lexer/parser divergence

`grammar.ebnf` is documented as *"the source of truth referenced by tree-sitter and TextMate grammars"* (`CLAUDE.md`, `README.md:124`), which makes these divergences consequential for external tooling.

### 3.1 String literals do not exist in the grammar (Major)

The lexer defines a string token (`crates/graphcal-compiler/src/syntax/token.rs:89-90`, `#[regex(r#""[^"]*""#)] StringLiteral`), the parser produces string expressions (`syntax/parser/expr.rs:329-335`) and string conversion targets (`expr.rs:66-79`), and the docs rely on strings pervasively: plot `title`/`x_label`/`color` (`docs/language/plots.md:168`, `:78`), `datetime("2024-11-05T12:00:00Z")` and `epoch("…", TT)` (`docs/language/built-ins.md:87-89`), timezone display `@meeting -> "America/New_York"` (`built-ins.md:131-135`).

`grammar.ebnf` contains **no string literal token and no production that uses one** — the lexical section (lines 25–125) never mentions `"`, and `atom` (lines 723–735) has no string alternative.

**Why it's inconsistent:** the formal grammar cannot describe plot declarations, datetime construction, or timezone display — all documented core features. Any tree-sitter/TextMate grammar generated faithfully from the EBNF would fail on most real files.

### 3.2 `scan`, `unfold`, `linspace`, `step` are hard keywords, but the grammar promises they're contextual (Major)

The grammar (lines 55–60) states:

> *"The following identifiers are contextual keywords — they are parsed as keywords only in specific syntactic positions and are otherwise valid identifiers: `scan`, `unfold`, `step`, `linspace`, `range`, `mark`, `encode`, …"*

But the lexer reserves four of them as dedicated tokens (`token.rs:77-84`: `#[token("scan")] Scan`, `#[token("unfold")] Unfold`, `#[token("linspace")] Linspace`, `#[token("step")] Step`), so `param step: Time = 1.0 s;` or `node scan = …;` cannot lex as identifiers. The *other* names in the same list (`range`, `mark`, `encode`, `min`, `max`, …) genuinely are contextual — matched by text, not tokenized (`syntax/parser/compound.rs:183`, `type_expr.rs:155-157`).

**Why it's inconsistent:** the grammar makes an explicit reservation promise that holds for most of the list but is false for exactly these four. `step` is a particularly likely engineering identifier (time step, step size), so the divergence will be hit in practice.

### 3.3 The `-> "Timezone"` conversion form is absent from the grammar (Moderate)

`convert_expr = conditional_expr, [ "->", unit_expr ];` (`grammar.ebnf:677-678`) admits only a unit target. The parser also accepts a string target producing `DisplayTimezone` (`syntax/parser/expr.rs:66-79`), and the docs document it (`built-ins.md:131-135`, `dimensions-and-units.md:183`). Distinct from 3.1: even with a string token added, `convert_expr` would still need a second alternative.

### 3.4 Match-arm separators: mandatory in the grammar, optional in the parser (Moderate)

Grammar: `match_expr = "match", expr, "{", [ match_arm, { ",", match_arm }, [ "," ] ], "}"` (`grammar.ebnf:903-904`) — arms are comma-separated. Parser: `parse_match_arm_list` consumes a comma only *if present* (`syntax/parser/compound.rs:29-42`, comment "Optional comma between arms"), so `A => 1.0 B => 2.0` parses. The implementation accepts programs the source-of-truth grammar rejects; a formatter or external parser following the EBNF would disagree with the compiler.

### 3.5 Trailing commas: `T[I,]` accepted against the grammar, while the equivalent `table[I,]` is rejected (Moderate)

- Grammar `index_expr_list = index_expr, { ",", index_expr };` (`grammar.ebnf:602-603`) — no trailing comma. But the parser routes it through `parse_comma_separated` (`syntax/parser/type_expr.rs:124`), which explicitly *"supports trailing commas"* (`syntax/parser/mod.rs:578-600`). So `Velocity[Maneuver,]` parses, contrary to the grammar.
- The table literal's index list (`index_list`, `grammar.ebnf:865-866`) is hand-parsed with a loop that requires an item after every comma (`syntax/parser/table.rs:39-46`) — trailing comma rejected.

**Why it's inconsistent:** two spellings of the same concept — "a list of index axes" — have different trailing-comma rules in the parser, and one of them also contradicts the grammar. Trailing-comma policy across the language is otherwise deliberate (`map_literal`, `type_application`, constructor lists allow it; `index_list` doesn't), but here the implementation doesn't follow its own spec.

### 3.6 `NUMBER` exponent: `_` separator allowed by the grammar, rejected by the lexer (Minor)

`NUMBER = DIGIT_SEQ, [ ".", DIGIT_SEQ ], [ ( "e" | "E" ), [ "+" | "-" ], DIGIT_SEQ ];` with `DIGIT_SEQ = DIGIT, { DIGIT | "_" };` (`grammar.ebnf:98-102`) — so `1e1_000` is grammatical. The lexer's regex ends `([eE][+-]?[0-9]+)?` (`token.rs:173`) — no `_` in the exponent. Mantissa and exponent get different separator rules, and grammar/lexer disagree.

### 3.7 Parser skips `/* … */` although the grammar says block comments don't exist (Minor)

`grammar.ebnf:25-27`: *"There are no block comments."* The lexer indeed defines only `//` comments (`token.rs:27`). Yet the turbofish lookahead scanner explicitly skips `/* … */` sequences (`syntax/parser/expr.rs:765-772`). Dead or latent logic that contradicts the stated comment design — if block comments are ever added the grammar is wrong, and until then the lookahead disagrees with the lexer it models.

### 3.8 Nat generic arguments are expressible in call position but not in type annotations (Moderate)

`generic_arg = type_expr | NUMBER;` (`grammar.ebnf:823-825`) — constructor calls and turbofish accept a Nat literal (`FixedVec<3>(…)`; parser: `syntax/parser/expr.rs:914-923`). But `type_application = ident_path, "<", type_expr, { ",", type_expr }, [ "," ], ">";` (`grammar.ebnf:598-599`) — type annotations accept only `type_expr`, with the parser matching (`syntax/parser/type_expr.rs:612-617`). `Nat` is a declared generic constraint (`grammar.ebnf:362-365`).

**Why it's inconsistent:** the *same* `<…>` argument list means different things in expression vs. type position: a type generic over `N: Nat` can be instantiated in a constructor call but the corresponding annotation `param v: FixedVec<3>` has no grammar/parser path (nat-range *indexes* `T[3]` are separate and unaffected). One of the two positions is missing a production.

### 3.9 `for` tuple-key sugar is implemented but never documented (Minor)

`for_expr` includes an optional `"(", IDENT, { ",", IDENT }, ")", "=>"` prefix before the body (`grammar.ebnf:930-933`), implemented in `syntax/parser/compound.rs:126-165`. Neither `docs/language/indexes.md` nor `type-system.md` ever shows this form — comprehension docs only show `for m: Maneuver { … }`. A grammar-level feature with zero documentation.

### 3.10 `Datetime<Scale>` is not represented in `type_expr_base` (Minor)

`type_expr_base` lists bare `"Datetime"` only (`grammar.ebnf:581-587`); the parameterized form used throughout the docs (`Datetime<TT>`, `type-system.md:118`, `built-ins.md:89`) is only derivable by accident through the generic `type_application` rule, while the parser gives it a dedicated `DatetimeApplication` variant (`syntax/parser/type_expr.rs:54-74`). The grammar doesn't acknowledge the built-in type argument the parser and docs treat as fundamental.

### 3.11 The formal grammar permits `pub`/attributes on multi-decls; its own prose and the parser forbid them (Minor)

`declaration = { attribute }, [ "pub", [ "(", "bind", ")" ] ], declaration_kind;` and `declaration_kind` includes `multi_decl` (`grammar.ebnf:141-142, 172-173`), so the formal rules attach attributes/visibility to multi-decls. The grammar's prose comment says the opposite (*"Attributes and `pub` / `pub(bind)` are forbidden on multi-decls"*, `grammar.ebnf:203`), and the parser rejects them (`syntax/parser/decl/mod.rs:318-328`). The formal productions and their own commentary disagree.

---

## 4. Documentation-internal contradictions

### 4.1 `floor`/`ceil`/`round`: `D -> D` on one page, `Dimensionless -> Dimensionless` on another (Moderate)

- `docs/language/built-ins.md:30-33`: `round(x)`, `trunc(x)`, `floor(x)`, `ceil(x)` — all `D -> D` ("any dimension").
- `docs/language/type-system.md:510`: `floor(x)`, `ceil(x)`, `round(x)` — argument `Dimensionless`, result `Dimensionless`.

Directly opposed signatures for the same functions; whether `round(@altitude)` is legal depends on which page you read. (type-system.md also omits `trunc` entirely.)

### 4.2 `atan2`: dimensionless-only on one page, dimension-polymorphic on another (Moderate)

- `docs/language/type-system.md:506` groups `atan2(y, x)` with `asin`/`acos`: argument `Dimensionless`, result `Angle`.
- `docs/language/built-ins.md:54`: `atan2(y, x)` is `(D, D) -> Angle` — any matching dimension.

The `(D, D)` signature is the physically useful one (`atan2(@y_pos, @x_pos)` on lengths); the type-system table contradicts it.

### 4.3 The scalar primitive is named both `Float` and `Scalar`; the primitive list itself varies (Moderate)

- `docs/language/type-system.md:16` models it as `Scalar(Dim)` and types `3.14` as `Dimensionless` (line 541); the stratification lists four primitives: `Scalar(Dim) | Int | Bool | Datetime(TimeScale)`.
- `docs/language/index.md:15-18` says the base types are *"`Float`, `Int`, `Bool`"* — a different name and a three-element list omitting `Datetime`.
- `docs/language/expressions.md:172` types `3.14` as *"`Float` (Dimensionless)"*; `README.md:38` says *"Every `Float` carries a physical dimension."*

One primitive, two names, and two different primitive inventories across reference pages. Note also the notation drift `Datetime(TimeScale)` (stratification) vs. `Datetime<TT>` (surface syntax) on the same page.

### 4.4 `type-system.md`'s own generic example uses a bare-field record form the language doesn't have (Moderate)

The ADT model is explicit that records are single-variant unions spelled with a constructor:

> *"Every `type` declaration in graphcal is an n-variant tagged union — record-shaped types are simply single-variant unions whose sole constructor's name matches the type's name."* — `docs/language/type-system.md:141-147`, with examples `type Orbit { Orbit(sma: …), }` and `type Vec3<D: Dim, Frame: Type> { Vec3(x: D, …), }` (lines 156–163)

and the grammar has no bare-field form (`type_decl_body = ";" | "{", constructor_list, "}"`, `grammar.ebnf:328-330`). Yet the "Default Type Parameters" section on the same page (lines 760–770) writes:

```
type Vec3<D: Dim, F: Type = Unframed> {
    x: D,
    y: D,
    z: D,
}
```

— bare fields, no constructor: a syntax the page itself (and the grammar) says doesn't exist.

### 4.5 "Use `if`-`then`-`else`" — the language has no `then` (Minor)

`docs/language/expressions.md:79`: *"Use `if`-`then`-`else` when you need conditional evaluation."* The actual syntax, shown 64 lines later in the same file (line 143) and everywhere else, is `if cond { a } else { b }` — there is no `then` keyword (nor in `grammar.ebnf`).

### 4.6 The authoritative "what `import` may bring" table omits `assert` (Moderate)

- `docs/language/multi-file.md:136-145` — the reference table lists importable kinds: `const node`, `dim`, `unit`, `type`, `index`, `dag`. No `assert` row.
- `docs/language/functions.md:136-138` — *"`import` brings compile-time names into scope: `dim`, `unit`, `type`, `index`, `const node`, `dag`, `assert`."*
- `docs/language/assertions.md` §`#[assumes]` depends on importing asserts by name.

The page that owns the import model is missing a kind the rest of the docs depend on.

### 4.7 "8 base dimensions" vs. "7 built-in base dimensions … plus Angle" (Minor)

`docs/language/dimensions-and-units.md:15`: *"The prelude provides 8 base dimensions"* (table includes `Angle`). `docs/tutorial/step2-dimensions-and-units.md:44`: *"Graphcal has 7 built-in base dimensions: … plus `Angle`."* Same set, contradictory counting convention.

### 4.8 Error codes `A015`/`A016` are used in prose but missing from the error-code table (Minor)

`docs/language/assertions.md` references `A015` (line 90, negative tolerance) and `A016` (line 325, out-of-range `#N` key) in running text, but its "Error Codes" table (lines 374–390) ends at `A014`.

### 4.9 `unfold` closure parameters are named differently on different pages (Minor)

`docs/language/type-system.md:706` specifies `unfold(init, |prev, curr| body)`; `docs/language/indexes.md:490` writes `unfold(@x0, |prev_t, t| …)`. Also a structural asymmetry worth noting: `scan` obtains its index from its explicit `source` argument, while `unfold` gets it *implicitly* from the enclosing declaration's type annotation (*"`I` is the index from context"*, `type-system.md:713`) — two sibling recurrence operators with different index-provenance models, in a language that prefers explicitness.

### 4.10 Tutorial import paths contradict the absolute-path rule (Moderate)

`docs/language/multi-file.md` establishes that every module path is absolute from the package root with the package name as first segment. The tutorial's own `main.gcl` follows it (`import rocket_project.constants.{g0};`, `step5-multi-file-projects.md:65`). But the same page then shows bare sibling-file imports: `import constants;` (line 106), `import file_a.{velocity as velocity_a}; import file_b.{…};` (lines 119-120) — first segments that are file names, not the `rocket_project` package. Reference and tutorial cannot both be right.

### 4.11 Tutorial inline-DAG call drops the mandatory `@` and output projection (Moderate)

`docs/tutorial/step5-multi-file-projects.md:128`: `node y: Length = p.helper(...);`. Both `docs/language/expressions.md:154-162` (*"The projected output after `.` is mandatory"*; form `@scale(factor: …).result`) and `docs/language/multi-file.md` require `@module.dag(args).out` — with the `@` sigil and a projected output. The tutorial line has neither and would not parse.

### 4.12 Plot examples map `x` and `y` to the same expression (Minor)

`docs/language/plots.md:162-169` (line chart) encodes `x: for t: Time { @altitude[t] -> km }` and `y: for t: Time { @altitude[t] -> km }` — identical; the bar chart (lines 179–186) does the same with `@power[m] -> W`. As written both charts plot a value against itself; the `x` channels were presumably meant to be the axis (`t` / mode). Copy-paste doc bugs in the only two basic chart examples.

### 4.13 README "Vision" lists `scan` as future work; the reference documents it as shipped (Minor)

`README.md:106`: *"Graphcal is heading toward: **Dynamic simulation** — `scan` over a time axis for system dynamics."* But `scan` is a documented, implemented operator (`docs/language/indexes.md:94-104`, `type-system.md:692-700`), and range indexes over `Time` already exist (`linspace`). If the vision item means something more than the shipped `scan` (e.g., stateful integration), the README doesn't distinguish it from the existing feature of the same name.

### 4.14 Versioned feature markers ("v1"/"v2") appear in supposedly version-free docs (Minor)

`docs/language/indexes.md` says *"In v2, at most one slot may carry an extra axis…"* (line 329) and *"…not allowed on a multi-declaration or its slots in v1"* (line 362) — two different version labels within one feature's description, in a project that (per `CLAUDE.md`) is unpublished and does not maintain backward compatibility. No other doc page defines what v1/v2 refer to.

### 4.15 `stroke_width` used as the example of a rejected *plot property* while being a documented *mark property* (Minor)

`docs/language/plots.md:48-51` rejects dimensioned values with the example `stroke_width: 2.0 m` under the explanation *"plot properties are raw rendering quantities"*, but `stroke_width` belongs to the mark-property table (`plots.md:73-79`). Wording nit; the example blurs the plot-level/mark-level distinction the page otherwise draws.

---

## 5. Syntax-design inconsistencies

### 5.1 Brace-bodied declarations split into two arbitrary surface families (Moderate)

Six declaration kinds have `{ … }` bodies, but they disagree on `=` and the trailing `;`:

| Form | Example | `=`? | Trailing `;`? |
|------|---------|------|---------------|
| `type T { … }` | `grammar.ebnf:328-330` | no | no |
| `dag d { … }` | `grammar.ebnf:486-487` | no | no |
| `index I = { … };` | `docs/language/indexes.md:14` | yes | yes |
| `plot p = { … };` | `grammar.ebnf:504-506` | yes | yes |
| `figure f = { … };` | `grammar.ebnf:533-535` | yes | yes |
| `layer l = { … };` | `grammar.ebnf:550-552` | yes | yes |

There is a post-hoc rationale available (the `= { … };` family assigns a *value-like* body, `type`/`dag` declare *scopes*), but the docs never state it, and users must memorize which of two conventions each keyword takes; the formatter and external parsers must special-case them.

### 5.2 `table` rows are `;`-terminated while the map literal it desugars to is `,`-separated (Moderate)

`map_literal = "{", map_entry, { ",", map_entry }, [ "," ], "}";` (`grammar.ebnf:840-841`) — comma-separated, trailing comma allowed. `table_data_row = [ IDENT, ":" ], expr, { ",", expr }, ";";` (`grammar.ebnf:884-889`) — semicolon-terminated rows, comma-separated cells, no trailing separator. The grammar itself notes the table form desugars to `map_literal`. Two surface syntaxes for the same data structure use opposite separator/terminator conventions and opposite trailing-separator rules.

### 5.3 Label qualification flips between map literals and table bodies (Minor)

Map-literal keys must be qualified (`Maneuver.Departure: …`, `docs/language/type-system.md:357-361`); table-body row labels are bare (*"Labels in the table body are unqualified (`Departure` instead of `Maneuver.Departure`)…"*, `docs/language/indexes.md:211`) — while table *slice* labels are qualified again (`[Time.T1]`, `indexes.md:246`). Three qualification rules across two spellings of the same construction.

### 5.4 Datetime construction: two function names, two argument conventions (Moderate)

`docs/language/built-ins.md:87-89`: parsing a datetime is `datetime("…")` for UTC and `datetime("…", "Asia/Tokyo")` with a **string** timezone — but `epoch("…", TT)` (a different function) with a **bare identifier** time scale. Display conversion likewise takes a string for timezones (`@meeting -> "America/New_York"`) but an identifier type argument for scales (`Datetime<TT>`). The same concept — "how to interpret this instant" — is spelled as a string in the timezone axis and as an identifier in the time-scale axis, split across two differently-named constructors.

### 5.5 Keyed initialization uses parens for constructors, braces for maps (Minor)

Struct construction is `Ctor(field: expr, …)` (parens); indexed-value construction is `{ Key: expr, … }` (braces). Both are exhaustive `name: value` forms; no rationale is given for the delimiter split. Compounding 5.2: three exhaustive keyed-literal syntaxes (constructor, map, table) with three delimiter/separator conventions.

### 5.6 `linspace` is a step-based constructor, not a linear-space constructor (Minor)

`index TimeStep = linspace(0.0 s, 1.0 s, step: 0.5 s);` (`docs/language/indexes.md:369`). In NumPy/Matlab, `linspace` takes a *point count*; the step-based constructor is `arange`/`colon`. Borrowing the well-known name with different semantics invites off-by-one/endpoint mistakes from the exact audience (engineers) the language targets — and it coexists with the count-based `range(N)`, so the two index constructors use opposite parameterizations.

---

## 6. Semantic asymmetries

### 6.1 Negative literal exponents: rejected on `Int` bases, accepted on `Scalar` bases (Moderate)

`2 ^ -1` is a compile error (*"integer power requires a non-negative exponent"*, `tir/dim_check/infer/rules.rs:362-372`; mirrored at runtime `eval_expr/arithmetic.rs:356-363`), while `2.0 ^ -1` succeeds and yields `0.5` (`rules.rs:428-442` — the Int-literal exponent path for scalar bases has no sign check). Defensible (integers can't represent reciprocals) but undocumented: `docs/language/expressions.md:39-43` documents `Int ^ Int` support and the literal-exponent rule without mentioning the non-negativity requirement.

### 6.2 The five aggregations disagree on empty input (Minor, latent)

`crates/graphcal-eval/src/eval_expr/aggregations.rs`: on an empty collection, `sum` returns `0.0` (fold from `0.0`), `count` returns `0`, `mean` raises a purpose-built `EmptyMean` (*"mean() over an empty Indexed value is undefined"*), and `min`/`max` fold from `±INFINITY` and then fail the finiteness check with a misleading *"min() produced infinite result"*. Three different behaviors (identity value / domain error / spurious infinity error) for one condition. Currently unreachable — indexes are non-empty by construction (`range(0)` rejected, `registry/index.rs:165`; named indexes require ≥1 variant, `syntax/parser/decl/index.rs:62-65`) — but the divergent policy is baked in and will surface the moment any empty-collection path is added.

### 6.3 `Datetime` is excluded from domain constraints despite being ordered (Moderate)

Domain constraints (`min:`/`max:`) are allowed on any scalar dimension and on `Int`, but *"not valid on `Bool`, `Datetime`, struct types, or union types"* (`docs/language/type-system.md:219-225`). Yet `Datetime` supports full ordering (`<`, `<=`, …, `type-system.md:127-131`), so `param launch: Datetime(min: datetime("2026-01-01T00:00:00Z"))` is a natural, well-defined capability that is denied while the equally-ordered `Int`/`Scalar` get it. The docs group `Datetime` with `Bool` (unordered) without justification.

### 6.4 `param` visibility is a special case that inverts the annotation rules (Moderate)

Per `docs/language/multi-file.md:642-660`: `param` may **never** carry `pub`/`pub(bind)` (parse error), is *implicitly* bindable, and *cannot be made private* (the docs suggest "naming/module boundaries" instead). Meanwhile required `index`/`type`/`dim` **must** be explicitly annotated `pub(bind)` (error `V002`, `multi-file.md:672-687`). So the two kinds of "required bindable interface" have opposite annotation policies — one forbids the annotation, the other mandates it — and `param` is the only declaration kind exempt from the "private by default" rule. This is an acknowledged axiom (A5) rather than an accident, but it is an implicit-visibility default in a language whose philosophy is explicitness, and the "can't have private params" gap has no workaround beyond naming conventions — exactly the kind of convention-over-types design the project forbids in its own compiler (`CLAUDE.md`).

### 6.5 `count` returns `Dimensionless` (float) rather than `Int` (Minor)

`count(...)` is typed `D[I] -> Dimensionless` (`docs/language/built-ins.md:147`; implementation `tir/dim_check/infer/hir.rs:603`). Everything else discrete in the language is `Int` — `range(N)` loop variables, datetime extractors (`year`, `month`, …, all `Datetime -> Int`). A cardinality typed as a float is the lone exception, in a language with no implicit `Int`→`Float` conversion where the mismatch forces `to_float`/`to_int` churn on whichever side the user needs.

### 6.6 Datetime extractors mix 0-based and 1-based conventions (Minor)

`weekday(x)` is 0-based (*"0=Monday, 6=Sunday"*, `docs/language/built-ins.md:126`) while `month`, `day`, `day_of_year` are 1-based. Within one family of extractors, the indexing convention flips.

### 6.7 Datetime arithmetic operand-order asymmetries are undocumented in the operator tables (Minor, by design)

`Scalar(Time) + Datetime` is accepted but `Scalar - Datetime` is an error, and `Datetime - Datetime` is `Time` while `Datetime + Datetime` is an error (`tir/dim_check/infer/rules.rs:226-292`). This is standard affine point/vector semantics and checker/evaluator agree — but `docs/language/expressions.md`'s operator tables describe `+`/`-` only in terms of dimensions and never mention that operand *order* is significant for `Datetime`, while `type-system.md:127-133`'s Datetime table omits the commuted forms. Listed for completeness.

### 6.8 `Nat` arithmetic supports addition but not subtraction (Minor, documented)

Type-level `Nat` expressions normalize to linear forms and support `+` (and documented multiplication in `indexes.md:404-417`) but not subtraction — *"instead, express the larger side with addition"* (`docs/language/type-system.md:802`). A deliberate, documented operational asymmetry; noted as a designed non-orthogonality rather than a defect.

---

## 7. Naming inconsistencies

### 7.1 `min`/`max` name three different constructs (Minor)

Binary scalar functions `min(a, b)`/`max(a, b)` (`docs/language/built-ins.md:71-72`), aggregations `min(...)`/`max(...)` over indexed values (`built-ins.md:144-145`), and domain-constraint keys `Mass(min: …, max: …)` (`type-system.md:196`). The overload is also uneven: `sum`/`mean`/`count` exist only as aggregations, with no binary counterparts. In a compiler codebase that bans string-matched dispatch, the surface language dispatches `min` by arity/context.

### 7.2 Prelude unit names collide with builtin function names (Minor)

`min` is a prelude Time unit (60 s) *and* a builtin function; `hour` is a prelude unit *and* `hour(x)` is a datetime extractor (`docs/language/built-ins.md:123-124` vs. `:190-194`). The universes are formally disjoint (units resolve only in unit syntax, `type-system.md:60-63`), so there's no ambiguity for the compiler — but `1.0 min` vs `min(a, b)` reads as the same identifier meaning two things, and `60.0 s -> min` vs `minute(x)` invites confusion between the unit `min` and the extractor `minute`.

### 7.3 Prelude unit abbreviation policy is inconsistent (Minor)

`docs/language/dimensions-and-units.md:84-96`: Time units are `s`, `min`, `hour` — two abbreviated, one spelled out (SI would be `h`). Meanwhile every other prelude unit uses the SI symbol (`m`, `kg`, `K`, `Pa`, `Hz`, …). `hour` is the lone word-form unit in the prelude.

### 7.4 The `to_` prefix spans three unrelated conversion families (Minor)

`to_int`/`to_float` (primitive conversion), `to_utc`/`to_tt`/… (time-scale conversion, value-preserving relabel), and `to_jd`/`to_unix` (datetime → number encoding) all share the prefix (`docs/language/type-system.md:448-458`, `built-ins.md`). Three semantically distinct operations — lossy numeric cast, lossless relabel, epoch encoding — are spelled identically, while the fourth conversion in the language (unit conversion) is an operator (`->`) instead of a function.

### 7.5 `const node` is, by the language's own table, not a node (Minor)

`docs/language/computation-model.md:13-17` defines the declaration kinds with an "In DAG?" column: `param` Yes, `node` Yes, `const node` **No**. The keyword literally asserts DAG-node-hood for the one value kind the model excludes from the DAG. (`assert` avoids this by not being called `assert node`.) A `const`-only keyword would state the actual semantics.

---

## Summary of findings

| # | Severity | Finding |
|---|----------|---------|
| 1.1 | Major | Comparisons broadcast implicitly (incl. scalar→indexed) while arithmetic forbids broadcasting as a safety principle |
| 1.2 | Moderate | `sign(x)` documents a NaN result, contradicting the no-NaN guarantee |
| 1.3 | Minor | "Inference" section titles vs. "no implicit type inference" philosophy |
| 2.1 | Major | Prelude pre-defines `Velocity`/`Force`/…/`hour`; README, docs, and tutorial examples redefine them and are rejected (`BuiltinNameShadowed`) |
| 2.2 | Moderate | Multi-axis aggregation type-checks but always fails at runtime; docs say `D[I] -> D` only |
| 2.3 | Moderate | Only `0.5` accepted among fractional literal exponents (docs say any literal); runtime exponents more permissive than literals |
| 2.4 | Moderate | Assert tolerance restricted to `unary_expr`; docs promise "arbitrary expressions… arithmetic" for all three operands |
| 3.1 | Major | String literals absent from `grammar.ebnf` despite pervasive use (plots, datetime, timezones) |
| 3.2 | Major | `scan`/`unfold`/`linspace`/`step` are hard lexer keywords; grammar declares them contextual |
| 3.3 | Moderate | `-> "Timezone"` conversion form missing from the grammar |
| 3.4 | Moderate | Match-arm commas mandatory in grammar, optional in parser |
| 3.5 | Moderate | `T[I,]` parses against the grammar; equivalent `table[I,]` rejected |
| 3.6 | Minor | `NUMBER` exponent: `_` allowed by grammar, rejected by lexer |
| 3.7 | Minor | Parser lookahead handles `/* */` though grammar says no block comments |
| 3.8 | Moderate | Nat generic args allowed in calls (`generic_arg`) but not in type annotations (`type_application`) |
| 3.9 | Minor | `for` tuple-key sugar `(a, b) =>` in grammar/parser, never documented |
| 3.10 | Minor | `Datetime<Scale>` not represented in `type_expr_base` |
| 3.11 | Minor | Grammar productions permit `pub`/attrs on multi-decls; grammar prose and parser forbid them |
| 4.1 | Moderate | `floor`/`ceil`/`round`: `D -> D` vs `Dimensionless -> Dimensionless` across pages |
| 4.2 | Moderate | `atan2`: `(D, D) -> Angle` vs dimensionless-only across pages |
| 4.3 | Moderate | Scalar primitive named `Float` and `Scalar` inconsistently; primitive inventory differs across pages |
| 4.4 | Moderate | type-system.md's own generic example uses a bare-field record form the language doesn't have |
| 4.5 | Minor | "Use `if`-`then`-`else`" — no `then` in the language |
| 4.6 | Moderate | multi-file.md import-kinds table omits `assert` |
| 4.7 | Minor | "8 base dimensions" vs "7 … plus Angle" |
| 4.8 | Minor | `A015`/`A016` used in prose, missing from the error-code table |
| 4.9 | Minor | `unfold` closure param names drift; `scan`/`unfold` index-provenance asymmetry |
| 4.10 | Moderate | Tutorial import paths contradict the absolute-path rule |
| 4.11 | Moderate | Tutorial inline-DAG call missing mandatory `@` and `.out` projection |
| 4.12 | Minor | Plot examples encode `x` and `y` identically |
| 4.13 | Minor | README Vision lists `scan` as future; reference documents it as shipped |
| 4.14 | Minor | Unexplained "v1"/"v2" feature markers in indexes.md |
| 4.15 | Minor | `stroke_width` used as a plot-property example but documented as a mark property |
| 5.1 | Moderate | `type`/`dag` use `{ }` while `index`/`plot`/`figure`/`layer` use `= { };` |
| 5.2 | Moderate | Table rows `;`-terminated vs map entries `,`-separated for the same data structure |
| 5.3 | Minor | Label qualification: qualified map keys, bare table rows, qualified table slices |
| 5.4 | Moderate | `datetime("…", "tz")` (string) vs `epoch("…", Scale)` (identifier) split constructor API |
| 5.5 | Minor | Constructor parens vs map braces for keyed exhaustive initialization |
| 5.6 | Minor | `linspace` has `arange` semantics (step-based, not count-based) |
| 6.1 | Moderate | `2 ^ -1` rejected, `2.0 ^ -1` accepted; restriction undocumented |
| 6.2 | Minor | Aggregations disagree on empty input (latent: indexes are non-empty by construction) |
| 6.3 | Moderate | `Datetime` excluded from domain constraints despite supporting ordering |
| 6.4 | Moderate | `param` visibility: annotation forbidden + implicit bindability + cannot be private, vs mandatory `pub(bind)` on other required kinds |
| 6.5 | Minor | `count` returns `Dimensionless` float, not `Int` |
| 6.6 | Minor | `weekday` 0-based; `month`/`day`/`day_of_year` 1-based |
| 6.7 | Minor | Datetime operand-order asymmetries absent from operator docs |
| 6.8 | Minor | `Nat` addition without subtraction (documented design choice) |
| 7.1 | Minor | `min`/`max` overloaded across binary fn / aggregation / constraint key |
| 7.2 | Minor | Unit `min`/`hour` vs function `min()`/`hour()` name collisions |
| 7.3 | Minor | Prelude abbreviates `s`/`min` but spells out `hour` (not `h`) |
| 7.4 | Minor | `to_` prefix spans cast, relabel, and epoch-encoding conversions |
| 7.5 | Minor | `const node` is defined as not being a DAG node |

## Closing observations

Recurring themes rather than isolated slips:

1. **The safety story has one systematic exception path.** Broadcasting (1.1), aggregation shape-checking (2.2), and the exponent rules (2.3, 6.1) all show the checker/evaluator pair drifting exactly where indexed values or literals meet operators — the places the docs advertise the strongest guarantees.
2. **`grammar.ebnf` is not currently the source of truth it claims to be.** Findings 3.1–3.11 mean tree-sitter/TextMate grammars generated from it would reject valid programs (strings, optional match commas) and accept invalid ones (`step` as identifier). Snapshot-testing the parser against the grammar (or generating one from the other) would prevent the whole class.
3. **The docs lack a single owner per fact.** Function signatures live in two tables (4.1, 4.2), the primitive list in three pages (4.3), import kinds in two (4.6). Each duplicated fact has drifted. The same applies to examples: nothing currently compiles the README/tutorial snippets, which is how 2.1 — every headline example failing on prelude shadowing — went unnoticed. Compiling all doc examples in CI would catch the worst category here.
4. **Three keyed-literal syntaxes, three conventions** (5.1, 5.2, 5.3, 5.5): declaration bodies, map literals, and tables each chose separators, terminators, and label qualification independently.
