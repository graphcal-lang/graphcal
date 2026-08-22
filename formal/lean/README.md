# Graphcal static semantics in Lean

This Lake package is the initial machine-checked model of the type-level
ontology documented in [`docs/language/type-system.md`](../../docs/language/type-system.md).
It formalizes Graphcal concepts as an object language inside Lean; Graphcal's
`Type` kind is represented by `Kind.valueType` and is not Lean's `Type`
universe.

## Established properties

The current model encodes and checks the following boundaries:

- `Dim`, semantic `TimeScale`, `Type`, `Index`, and `Nat` are distinct kinds.
- `TimeScale` is absent from the kinds legal for generic parameters.
- Resolved semantic entities have a unique kind (`HasKind.unique`).
- Untyped type-former expressions have unique declarative kinds
  (`HasTypeLevelKind.unique`).
- Malformed former applications such as `Complex(Fin(3))`, `Key(3)`, and
  `Fin(0)` have no declarative kind.
- Every semantic kind has a concrete inhabitant (`semantic_kind_model_nonempty`).
- `Quantity`, `Complex`, and `Key` have kind-correct semantic signatures.
- `Fin` accepts only a `PositiveNat`; `Fin(0)` is rejected during elaboration.
- Nominal generic arguments are indexed by their declared kind sequence, so a
  cross-kind application cannot be constructed.
- Generic substitution preserves the result kind by construction.
- Bounds can be constructed only for quantities, integers, and datetimes.
- Indexed declaration types carry a structurally nonempty axis list.
- In a value-type position, dimension syntax elaborates to `Quantity(D)`.
- The same dimension syntax remains a dimension in a `Dim` generic position
  and becomes `Quantity(D)` in a `Type` generic position.
- Value-type and generic-argument elaboration are deterministic.
- The extensional dimension model satisfies identity, associativity,
  commutativity, cancellation, and basic power laws.

Representative accepted and rejected cases are checked in
[`Graphcal/Static/Examples.lean`](Graphcal/Static/Examples.lean).
Completed declarations use neither `sorry` nor Graphcal-specific axioms. The
full initial trust boundary is recorded in [`ASSUMPTIONS.md`](ASSUMPTIONS.md).

## Deliberate scope

This is the first static-ontology milestone, not a proof of the Rust compiler.
It currently does **not** establish:

- Correctness of Graphcal parsing, name resolution, or the production checker
- Dimension or Nat normalization soundness and completeness
- Decidable equality for the extensional dimension model
- Registry coherence for owner-qualified nominal declarations
- Surface elaboration of constrained bounds or nominal generic applications
- Declarative term typing, checker soundness/completeness, or runtime type safety
- Correspondence between Lean and production Rust

`Dimension` is initially modelled extensionally as a function from canonical
base-dimension identities to rational exponents. Production Graphcal uses a
canonical finite sparse map. A later milestone should add a finite normalized
representation and prove its interpretation sound and complete.

## Layout

The modules are ordered by dependency direction:

```text
Graphcal/Static/
├── Kind.lean
├── TimeScale.lean
├── Dimension.lean
├── Nat.lean
├── Index.lean
├── ValueType.lean
├── ConstrainedType.lean
├── DeclType.lean
├── Kinding.lean
├── TypeFormer.lean
├── SurfaceSyntax.lean
├── Elaboration.lean
├── Generic.lean
├── Metatheory.lean
└── Examples.lean
```

Core semantic domains precede source elaboration, and the metatheory consumes
rather than defines those domains.

## Building

The Lean toolchain is pinned in [`lean-toolchain`](lean-toolchain).
From the Graphcal repository root, run:

```sh
lake -d formal/lean build
```

The package intentionally has no third-party Lean dependency in this initial
milestone.

## Assurance boundary

Lean checks these results relative to Lean's kernel and foundations. The model
itself may still fail to express Graphcal's intended design, and none of the
proofs automatically applies to the Rust implementation. Changes to the model
must therefore be reviewed against the language reference, and later work must
connect it to Rust using structured differential testing.
