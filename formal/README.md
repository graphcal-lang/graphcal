# Graphcal formal specifications

This Lake package contains the machine-checked normative specifications used by Graphcal's verification-guided development workflow.

The package currently contains three specification slices:

- [`Graphcal/Static/RequiredBindability.lean`](Graphcal/Static/RequiredBindability.lean) formalizes validation rule V002: required indexes, types, and dimensions must be bindable, while parameters are inherently bindable input ports.
- [`Graphcal/Static/NamespaceResolution.lean`](Graphcal/Static/NamespaceResolution.lean) formalizes the Static, Term, and Unit namespace kernel.
  It models typed collision slots, owner-scoped index labels, structured DAG paths, `::` member selection, `#` label selection, capability checks, no-shadowing, canonical alias targets, and call resolution independent of argument shape.
- [`Graphcal/Static/ExternalSurface.lean`](Graphcal/Static/ExternalSurface.lean) formalizes import capability and include projection after namespace/visibility resolution.
  It rejects required Static holes and declarations with unresolved transitive Static dependencies from direct imports, distinguishes fixed, optional-default, and required Static roles, projects supplied bindings to their effective targets, gives Static declarations applicative specialization identities, and keeps runtime Terms and plain-unit scales instance-specific.

[`Graphcal/Static/Interface.lean`](Graphcal/Static/Interface.lean) is the shared upstream vocabulary for Static input categories, visibility, and requirement state. [`Graphcal/Static/IncludeProjection.lean`](Graphcal/Static/IncludeProjection.lean) is the shared include-capability table: constructors follow their nominal owner while reusable DAG blueprints are deliberately not instance projections. Namespace resolution and external-surface composition reuse these definitions rather than maintaining separate category inventories.

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

The external-surface slice follows the same model/spec/reference/proofs split:

```text
Graphcal/Static/ExternalSurface/
├── Model.lean
├── Spec.lean
├── Reference.lean
├── Proofs.lean
└── Examples.lean
```

Its central equivalences prove that executable import validation accepts exactly the normative `Importable` predicate, direct import and unconfigured include projection share one identity for importable declarations, required and optional Static binding lookup preserves exact categories, normalized Static substitutions agree with the declarative relation, and executable include projection accepts exactly `Projects`. The slice consumes an upstream-resolved deterministic transitive Static dependency closure; constructing and certifying that closure from source declarations remains a separate environment-construction obligation.

## Build and conformance check

Install [Elan](https://lean-lang.org/install/) and run from the repository root:

```sh
just formal
just formal-conformance
```

`just formal` checks the Lean library and rejects proof placeholders or custom axioms.
`just formal-conformance` runs three differential checks:

- the required-bindability oracle's complete 20-case matrix against Graphcal's parser, desugaring, and V002 production pass;
- the namespace-resolution oracle's 72 reviewed scenarios against the production single-file and project pipeline;
- the external-surface oracle's 55 import, typed Static binding, effective projection-identity, constructor-owner, and DAG-blueprint states against Rust's shared semantic core.

The namespace matrix covers visible and `::` member lookups for every Static/Term/Unit occupancy, duplicate slots, every DAG-input selector/target category pair, and owner-qualified labels. The external-surface matrix exhaustively combines fixed, optional-input, and required-input roles with dependency, target-role, and exact-category choices, then checks constructor owner rebinding and non-projectable DAG blueprints explicitly.

The namespace and external-surface oracles are emitted by [`Graphcal/Static/NamespaceResolutionOracle.lean`](Graphcal/Static/NamespaceResolutionOracle.lean) and [`Graphcal/Static/ExternalSurfaceOracle.lean`](Graphcal/Static/ExternalSurfaceOracle.lean). They evaluate the proven executable references; Rust independently renders or classifies each typed scenario, so production acceptance cannot silently diverge from the Lean model.

The checked-in `lean-toolchain` pins the Lean version.
The package intentionally has no third-party Lean dependencies for these specification slices.
