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
aggregation, addressed by the Section 4.5 follow-up) and the closed
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
model remains indexed values rather than a dedicated matrix type. Native
scaled-pivoting LU now provides general `solve(A, b)`, `inverse`, and `det`,
closing the highest-value algorithmic gap without adding unbounded language
iteration. WASM ABI v4 carries shaped multi-axis arrays, required type-level DAG
ports package dimension/axis-polymorphic kernels, and `product`/`rss` cover the
remaining common reduction ergonomics.

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

### 3.1 Matrix–vector recurrences preserve indexed state

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

Indexed recurrence state is specified and covered by the compiler, evaluator,
formatter, and LSP through `tests/fixtures/valid/indexed_state_recurrence.gcl`
(#889). The recurrence axis is prepended to the fixed state axes without
introducing nested source type syntax: `T[Element]` becomes
`T[Step, Element]`. `scan` likewise permits indexed accumulator state while
requiring its input source to be rank one, so it never chooses a source axis
implicitly.

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

The algorithmic remainder (`solve`, `inverse`, and `det`) is implemented by
the next subsection's native-kernel slice. Public factorization objects and
spectral decompositions remain future extensions rather than prerequisites for
practical linear systems.

### 4.2 Algorithmic linear algebra — addressed with native kernels

The structural finding remains true: Graphcal intentionally has no recursion,
`while`, convergence loops, or data-dependent pivoting in user expressions.
Instead of weakening the finite reactive model, native `solve`, `inverse`, and
`det` builtins use one shared scaled partial-pivoting LU implementation.
`solve(A, b)` preserves `(D2 / D1)[I]`, `inverse(A)` preserves `D^-1[I, I]`,
and `det(A)` computes `D^N` from the concrete axis cardinality. Solve-like
operations reject singular or numerically ill-conditioned matrices and verify
residuals before exposing results; determinant remains defined as zero for a
singular matrix.

This closes the practical in-language boundary at the builtin layer while
keeping arbitrary iterative algorithms out of the source language. The valid
fixture demonstrates all three operations, and
`tests/fixtures/runtime_error/linear_algebra_singular.gcl` locks in explicit
singularity failure.

### 4.3 Multi-axis plugin arrays — addressed with ABI v4

Extern signatures and the WASM boundary now carry quantity arrays of any
non-zero rank: `A: D[I, J]` is legal, each array value carries ordered shape
metadata, and flattened values use explicit row-major order. ABI v4 lowers an
array parameter to a pointer followed by one `i32` extent per axis. The SDK
exposes borrowed `ArrayView` inputs and validated owned `Array` results, while
the host/evaluator preserve every concrete typed axis and its keys.

Result axes still must reuse index variables bound by parameters, so plugins
cannot invent dynamic extents. They can reorder or repeat bound axes; for
example `matrix_transpose<D, I, J>(D[I, J]) -> D[J, I]` returns the reversed
shape and rebuilt Graphcal axes. `tests/fixtures/valid/plugin_extern.gcl`
demonstrates that path, and the ABI, macro, CLI, evaluator, and real-WASM host
tests cover shape validation and row-major conversion.

Record-shaped results still allow only concrete-dimension fields; a
dimension-generic `Vec3`-like record result remains a separate extension.

### 4.4 Reusable linear-algebra DAGs — addressed with type-level ports

Graphcal already has an explicit abstraction mechanism that better matches its
no-inference policy than generic DAG headers: required `pub(bind)` dimensions,
types, and indexes inside a `dag`. They are type-level input ports supplied by
name at each `include`, alongside ordinary value params. A single body can
therefore use `ElementDim[Axis]`, dimension algebra such as `ElementDim^2`, and
linear-algebra builtins, then be instantiated over unrelated dimensions and
axes with fully checked substitutions.

Inline-DAG module resolution now registers body-local type-system declarations
under the child DAG identity, fixing required indexes (and other local type
members) that previously resolved syntactically but were absent from the
module-aware type registry. `tests/fixtures/valid/linear_algebra_reusable_dag.gcl`
instantiates one magnitude DAG over both `Length[SpatialAxis]` and
`Time[Samples]` and verifies both results.

A separate `<N: Nat>` DAG-generic system with symbolic size arithmetic remains
issue #354. It is not needed for ordinary size polymorphism here: a required
`Index` carries its concrete cardinality and identity explicitly at each
instantiation.

### 4.5 Aggregation ergonomics and nominal geometry — addressed

`product(values)` now reduces an explicit rank-one selection and computes the
unit-correct result dimension `D^|I|`; nested `product(for …)` expressions make
chained multi-axis products readable without adding implicit flattening or
broadcasting. `rss(values)` implements the #895 root-sum-square operation with
scaled accumulation, preserving the element dimension while avoiding avoidable
intermediate overflow and underflow. The valid ergonomics fixture includes a
mean/sigma budget and a nested product chain.

A universal built-in `Vec3`/rotation/quaternion nominal layer would have to
invent globally meaningful frame names, which conflicts with project-owned
engineering vocabulary. The documented standard pattern therefore keeps frames
as project-defined nominal `Type` arguments (`Vec3<D, Frame>`,
`Rotation<From, To>`, `Quaternion<From, To>`) and delegates numeric kernels to
the axis-safe array operations. `tests/fixtures/valid/linear_algebra_ergonomics.gcl`
checks that pattern together with `product` and `rss`; reusable numeric
component operations can be packaged through Section 4.4's type-level DAG inputs.

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
