# Linear Algebra — Current Support and Gaps

This document surveys what a Graphcal user can express with vectors and
matrices today and what is missing for practical linear algebra. It is a
capability survey, not a design proposal: it records the verified state of
the language so that the design discussion can start from facts.

Companion design discussions live in GitHub issues — primarily
[#890](https://github.com/graphcal-lang/graphcal/issues/890) (linear-algebra
builtins over indexed matrices) and
[#99](https://github.com/graphcal-lang/graphcal/issues/99) (standard-library
checklist, which lists linear algebra and attitude quaternions). Adjacent:
[#895](https://github.com/graphcal-lang/graphcal/issues/895) (`rss`
aggregation) and the closed
[#315](https://github.com/graphcal-lang/graphcal/issues/315) (`Fin(N)` loop
variables, which delivered the static bounds checking that positional
vector/matrix code relies on).

The original "works" claims below were verified by evaluating probe programs
with the CLI at commit `bdb597a` (v0.0.1-alpha.17); expected rejections were
verified to produce their documented diagnostics. Implementation-status notes
in Section 4 track the follow-up work prompted by this survey.

## 1. Summary

Graphcal now has an axis-safe core linear-algebra layer: `dot`, `matmul`,
`transpose`, `trace`, `norm`, `cross`, and `outer` operate directly on indexed
quantities with dimension bookkeeping and typed-axis contractions. The data
model remains indexed values rather than a dedicated matrix type. Algorithmic
operations — `solve(A, b)`, general inverse, factorizations, and spectra —
remain the hard boundary described below until the next implementation slice,
and the WASM plugin escape hatch still cannot accept a matrix argument.

## 2. Representation

There is no dedicated vector or matrix type. Two building blocks exist:

**Indexed values.** `T[I]` and `T[I, J]` are total maps over finite axes.
The structural axis `Fin(N)` gives positional collections, and `table[...]`
literals give matrix-shaped input syntax:

```gcl
param v: Dimensionless[Fin(3)] = table[Fin(3)] { 1.0; 2.0; 3.0; };

param A: Length[Fin(2), Fin(3)] = table[Fin(2), Fin(3)] {
    1.0 m, 2.0 m, 3.0 m;
    4.0 m, 5.0 m, 6.0 m;
};
```

Axis order is part of the type (`T[I, J]` and `T[J, I]` differ), axes may
also be named or coordinate indexes, and `Fin` loop variables are statically
bounds-checked integers (`@v[i + 1]` on a `Fin(3)` loop over a `Fin(4)`
value checks at compile time).

**User-defined algebraic types.** The docs use `Vec3<D: Dim, Frame: Type>`
as the frame-safe 3-vector pattern. These get no arithmetic: operators apply
only to quantities, so `Vec3 + Vec3` is rejected and everything is
constructed field-by-field. There is no operator overloading.

Two properties of this model constrain linear algebra directly:

- **Uniform element dimension.** An indexed value has exactly one element
  type, so every entry of a matrix shares one dimension.
  Heterogeneous-unit matrices — a Kalman covariance over a
  `[position, velocity]` state, a Jacobian of a mixed-dimension state —
  cannot be a single value and must be split into per-block nodes.
- **No nesting.** `DeclType` is `ValueType[I₁, …, Iₘ]`; an element type
  cannot itself be indexed (`Dimensionless[Fin(2)][Step]` is a parse
  error), and constructor payload fields cannot be indexed either.

## 3. What works today (verified)

Aggregation over one axis composes with `for` comprehensions, outer loop
variables are visible inside inner `sum(for …)`, and `sqrt` halves
dimension exponents — so all contraction-style operations are expressible
and unit-correct:

| Operation | Spelling | Verified result |
|---|---|---|
| Dot product | `sum(for i: Fin(3) { @a[i] * @b[i] })` | `32` for `[1,2,3]·[4,5,6]` |
| Matrix–vector product | `for i { sum(for j { @A[i,j] * @x[j] }) }` | correct |
| Matrix–matrix product | `for i, k { sum(for j { @A[i,j] * @B[j,k] }) }` | `[[58,64],[139,154]]` for the 2×3 · 3×2 example |
| Transpose | `for j, i { @A[i, j] }` | correct |
| Trace | `sum(for i { @M[i, i] })` | same loop variable on both axes works |
| Euclidean norm with units | `sqrt(sum(for i { @r[i] * @r[i] }))` | `Length[Fin(3)]` of `(3, 4, 12) m` → `13 m` |
| Cross product | literal indexing + cofactor formulas | `r × r = (0, 0, 0) m^2` |
| 2×2 determinant | `@M[0,0]*@M[1,1] - @M[0,1]*@M[1,0]` | `-2 m^2` for a `Length` matrix — dimension `Area` comes out right |
| 2×2 inverse (closed form) | cofactor formulas over literal indices | `[[-2,1],[1.5,-0.5]]` for `[[1,2],[3,4]]` |
| Identity / structured matrices | `for i, j { if i == j { 1.0 } else { 0.0 } }` | `Fin` equality comparison works |
| Frobenius-style reductions | nested `sum(for i { sum(for j { … }) })` | multi-axis aggregation is rejected (`D021`), nesting required |

Element-wise arithmetic requires explicit `for` — `@a + @b` on indexed
values is rejected (`D001`/`D019`), by design (no implicit broadcasting).

### 3.1 Matrix–vector recurrences work — incidentally

The state-space pattern `x_{k+1} = A·x_k` evaluates correctly through
`unfold` with an indexed value as recurrence state:

```gcl
node x_hist: Dimensionless[Step, Fin(2)] = unfold(
    Step,
    @x0,
    |prev, prev_t, t| for i: Fin(2) { sum(for j: Fin(2) { @A[i, j] * prev[j] }) }
);
```

Verified: with `A = [[0.9, 0.1], [0, 1]]` and `x0 = [1, 0]`, the history is
`[1, 0.9, 0.81]` in component 0. The result flattens to `T[Step, Fin(2)]`.

Caveat: no documentation, fixture, or unit test covers indexed
`unfold`/`scan` state. The docs describe the accumulator as a single type
`T`, and the type-system stratification (`ValueType[I…]`, no nesting) does
not obviously admit it. This capability should either be blessed with tests
and docs or rejected deliberately — right now it is load-bearing for
time-marching models but incidental.

### 3.2 The plugin escape hatch works for rank-1 arrays

Extern (WASM/host) functions can take and return arrays over an index
variable, and a `Fin(N)` axis does bind an `I: Index` variable at the call
site — verified with the built-in demo plugin:

```gcl
import plugin "graphcal:demo" as demo {
    fn normalize<D: Dim, I: Index>(xs: D[I]) -> Dimensionless[I];
}

param v: Length[Fin(3)] = table[Fin(3)] { 1.0 m; 2.0 m; 5.0 m; };
node v_unit: Dimensionless[Fin(3)] = demo.normalize(@v);   // (0.125, 0.25, 0.625)
```

(The extern-functions doc says concrete indexes "cannot cross the plugin
boundary" — that restriction applies to the *signature*, not to which axes
can bind the variable at a call site.)

## 4. What is missing

### 4.1 The operations themselves — core contractions addressed

The first implementation slice adds native `dot`, `matmul`, `transpose`,
`trace`, `norm`, `cross`, and `outer` builtins. They preserve element-dimension
algebra, require rank-one/rank-two quantity inputs as appropriate, and match
contracted axes by typed identity rather than cardinality alone. Demonstration
and regression coverage lives in `tests/fixtures/valid/linear_algebra.gcl`.

The algorithmic remainder (`solve`, `inverse`, `det`, and decompositions) is
the subject of the next subsection and implementation slice.

### 4.2 Algorithmic linear algebra is inexpressible in-language

This is the hard boundary, and it is structural rather than a missing
feature:

- No recursion, no `while`, no convergence loops — `for` is a
  comprehension over a fixed finite axis.
- `scan` / `unfold` fold along one fixed axis; they cannot express
  pivoting or data-dependent iteration counts.
- Gaussian elimination, LU/QR, and iterative eigensolvers therefore cannot
  be hand-written for general N. Only fixed-size closed forms (2×2, 3×3
  cofactor inverses, quadratic formulas) are available.

Consequence: `solve` must arrive as a builtin or plugin capability; it
cannot be a userland library.

### 4.3 The plugin boundary stops at rank 1

The WASM plugin ABI would be the natural home for numeric kernels, but
today:

- Multi-axis arrays cannot cross — `A: D[I, J]` is not a legal extern
  parameter ("array elements are quantities in this phase;
  `Bool[I]`/`Int[I]` and multi-axis arrays are not supported"). No plugin
  can implement `solve` or `inverse`.
- Result arrays must reuse an index variable bound by some parameter — a
  plugin can never invent its output length. (This is fine for `solve`:
  `x` shares `b`'s axis. It rules out factorizations whose output shapes
  differ from inputs unless those axes appear in the signature.)
- Record-shaped results allow only concrete-dimension fields — a
  dimension-generic `Vec3`-like result cannot cross yet.

### 4.4 No reusable abstractions to package linear algebra as a library

`dag` blocks take no generic parameters (`dag_decl = "dag", IDENT, "{" …`
in `grammar.ebnf`) — no `<N: Nat>`, `<D: Dim>`, or `<I: Index>`. A user
cannot write `cross` or `solve3` once and reuse it across dimensions; every
dimension/size instantiation is spelled out by hand. `pub(bind) index`
gives per-file axis genericity for named axes in multi-file projects, but
nothing covers `Dim`-generic math.

### 4.5 Ergonomics and standard-library gaps

- Chained products are deeply nested comprehensions; there is no
  pipeline-friendly form.
- No product aggregation; no `rss` (proposed separately in #895).
- No built-in `Vec3` / rotation-matrix / quaternion layer. The docs'
  frame-tagged `Vec3<D, Frame>` is a per-project pattern with hand-rolled
  component math; attitude quaternions sit unchecked on the #99 stdlib
  list.

## 5. Design questions a linear-algebra layer must answer

Worth settling on #890 before implementation.

**Dimension rules per operation.** Contractions fit the existing
rational-exponent dimension algebra cleanly; determinants entangle it with
`Nat` generics:

| Operation | Element dimension rule | Note |
|---|---|---|
| `matmul(A: D1[I,J], B: D2[J,K])` | `(D1 * D2)[I, K]` | shared axis `J` is the contraction — explicit in the types |
| `solve(A: D1[I,I], b: D2[I])` | `(D2 / D1)[I]` | consistent with `x` satisfying `A·x = b` |
| `inverse(A: D[I,I])` | `D^-1[I, I]` | |
| `det(A: D[Fin(N),Fin(N)])` | `D^N` | typeable only for concrete `N` — the exponent depends on the axis cardinality |
| `norm(v: D[I])` | `D` | already works via `sqrt` |
| `cross(a: D1[…], b: D2[…])` | `D1 * D2` | needs a fixed-3 axis story |

**Uniform element dimension.** Fine for frame vectors and rotation
matrices (`Dimensionless` DCMs); blocking for covariance/Jacobian
workflows. Either document block-matrix idioms as the answer or design a
heterogeneous form.

**Axis identity for contraction.** With named axes, which axis pairs up is
visible in the types — a natural fit for the language's
no-implicit-broadcasting stance. For `Fin` axes, `matmul(A, A)` on
`D[Fin(2), Fin(2)]` has an axis-pairing ambiguity that hand-written `for`
never has. Named-axis contraction (issue #890 asks for "named, checked
axes") sidesteps this; positional `Fin`-only signatures reintroduce it.

**Square-ness.** `solve`/`inverse`/`det` need `A: D[I, I]` — the same axis
twice — which the type grammar already permits. Whether the checker should
also accept `D[Fin(N), Fin(N)]` with equal-cardinality-but-distinct axes is
a design choice (recommendation: no; same axis literally, keeping
row/column identity meaningful).

## Appendix: probe program

The main probe (all nodes type-check and evaluate at `bdb597a`;
abbreviated):

```gcl
param a: Dimensionless[Fin(3)] = table[Fin(3)] { 1.0; 2.0; 3.0; };
param b: Dimensionless[Fin(3)] = table[Fin(3)] { 4.0; 5.0; 6.0; };
param A: Dimensionless[Fin(2), Fin(3)] = table[Fin(2), Fin(3)] {
    1.0, 2.0, 3.0;
    4.0, 5.0, 6.0;
};
param B: Dimensionless[Fin(3), Fin(2)] = table[Fin(3), Fin(2)] {
    7.0,  8.0;
    9.0,  10.0;
    11.0, 12.0;
};
param r: Length[Fin(3)] = table[Fin(3)] { 3.0 m; 4.0 m; 12.0 m; };
param M2: Length[Fin(2), Fin(2)] = table[Fin(2), Fin(2)] {
    1.0 m, 2.0 m;
    3.0 m, 4.0 m;
};

node dot_ab: Dimensionless = sum(for i: Fin(3) { @a[i] * @b[i] });   // 32
node Ab: Dimensionless[Fin(2)] = for i: Fin(2) {
    sum(for j: Fin(3) { @A[i, j] * @a[j] })                          // (14, 32)
};
node AB: Dimensionless[Fin(2), Fin(2)] = for i: Fin(2), k: Fin(2) {
    sum(for j: Fin(3) { @A[i, j] * @B[j, k] })                       // [[58,64],[139,154]]
};
node At: Dimensionless[Fin(3), Fin(2)] = for j: Fin(3), i: Fin(2) { @A[i, j] };
node r_norm: Length = sqrt(sum(for i: Fin(3) { @r[i] * @r[i] }));    // 13 m
node det_m2: Area = @M2[0, 0] * @M2[1, 1] - @M2[0, 1] * @M2[1, 0];   // -2 m^2
node trace_m2: Length = sum(for i: Fin(2) { @M2[i, i] });            // 5 m
node eye3: Dimensionless[Fin(3), Fin(3)] = for i: Fin(3), j: Fin(3) {
    if i == j { 1.0 } else { 0.0 }
};
```

Negative probes, confirming documented rejections:

```gcl
node c: Dimensionless[Fin(3)] = @a + @b;
// D001: dimension mismatch: expected quantity type, found Dimensionless[Fin(3)]
// help: expected a quantity value, not an indexed value

node x: Dimensionless[Fin(2)][Step] = ...;
// P001: unexpected token `[` — nested indexed types do not exist
```
