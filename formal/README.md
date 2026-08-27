# Graphcal formal specifications

This Lake package contains the machine-checked normative specifications used by
Graphcal's verification-guided development workflow.

The package currently contains two reviewed specification slices:

- [`Graphcal/Static/RequiredBindability.lean`](Graphcal/Static/RequiredBindability.lean)
  formalizes validation rule V002: required indexes, types, and dimensions must
  be bindable, while parameters are inherently bindable input ports.
- [`Graphcal/Static/NamespaceResolution.lean`](Graphcal/Static/NamespaceResolution.lean)
  formalizes the Static, Term, and Unit namespace kernel. It models typed
  collision slots, owner-scoped index labels, structured DAG paths, `::` member
  selection, `#` label selection, capability checks, no-shadowing, canonical
  alias targets, and call resolution independent of argument shape.

The namespace slice is split into an independent declarative specification and
executable reference implementation:

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

The proofs establish exact agreement between the reference resolver and the
normative relation, deterministic and order-independent lookup, scope-builder
correctness, namespace noninterference, canonical member identity through DAG
aliases, and the approved positive and negative design examples. Table syntax
sugar is deliberately outside this initial kernel; its contextual row/column
labels are reconstructed as owner-qualified keys before namespace resolution.

## Build and conformance check

Install [Elan](https://lean-lang.org/install/) and run from the repository root:

```sh
just formal
just formal-conformance
```

`just formal` checks the Lean library and rejects proof placeholders or custom
axioms. `just formal-conformance` then asks the required-bindability Lean
executable oracle for its complete 20-case state matrix and compares every
decision with Graphcal's real parser, desugaring, and V002 production pass.

Namespace-resolution conformance is intentionally deferred until the approved
namespace syntax and production resolver are implemented. Its executable Lean
reference is already suitable for a future structured-project oracle; it should
not be compared against the fallback-based resolver being replaced.

The checked-in `lean-toolchain` pins the Lean version. The package intentionally
has no third-party Lean dependencies for these specification slices.
