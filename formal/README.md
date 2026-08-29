# Graphcal formal specifications

This Lake package contains the machine-checked normative specifications used by Graphcal's verification-guided development workflow.

The package currently contains two reviewed specification slices:

- [`Graphcal/Static/RequiredBindability.lean`](Graphcal/Static/RequiredBindability.lean) formalizes validation rule V002: required indexes, types, and dimensions must be bindable, while parameters are inherently bindable input ports.
- [`Graphcal/Static/NamespaceResolution.lean`](Graphcal/Static/NamespaceResolution.lean) formalizes the Static, Term, and Unit namespace kernel.
  It models typed collision slots, owner-scoped index labels, structured DAG paths, `::` member selection, `#` label selection, capability checks, no-shadowing, canonical alias targets, and call resolution independent of argument shape.

The namespace slice is split into an independent declarative specification and executable reference implementation:

```text
Graphcal/Static/NamespaceResolution/
├── Model.lean
├── Spec.lean
├── Reference.lean
├── Proofs.lean
├── Proofs/
│   ├── Lookup.lean
│   ├── Scope.lean
│   ├── Path.lean
│   ├── InputBinding.lean
│   └── Resolution.lean
└── Examples.lean
```

The proofs establish exact agreement between the reference resolver and the normative relation, deterministic and order-independent lookup, scope-builder correctness, namespace noninterference, canonical member identity through DAG aliases, and the approved positive and negative design examples.
Table syntax sugar is deliberately outside this initial kernel; its contextual row/column labels are reconstructed as owner-qualified keys before namespace resolution.

## Build and conformance check

Install [Elan](https://lean-lang.org/install/) and run from the repository root:

```sh
just formal
just formal-conformance
```

`just formal` checks the Lean library and rejects proof placeholders or custom axioms.
`just formal-conformance` runs two differential checks:

- the required-bindability oracle's complete 20-case matrix against Graphcal's parser, desugaring, and V002 production pass;
- the namespace-resolution oracle's 72 reviewed scenarios against the production single-file and project pipeline.
  The matrix covers visible and `::` member lookups for every Static/Term/Unit occupancy, duplicate slots, every DAG-input selector/target category pair, and owner-qualified labels.

The namespace oracle is emitted by [`Graphcal/Static/NamespaceResolutionOracle.lean`](Graphcal/Static/NamespaceResolutionOracle.lean).
It evaluates the proven executable reference resolver; Rust independently renders and compiles each typed scenario, so production acceptance cannot silently diverge from the Lean model.

The checked-in `lean-toolchain` pins the Lean version.
The package intentionally has no third-party Lean dependencies for these specification slices.
