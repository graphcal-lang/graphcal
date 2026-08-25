# Graphcal formal specifications

This Lake package contains the machine-checked normative specifications used by
Graphcal's verification-guided development workflow.

The first specification is
[`Graphcal/Static/RequiredBindability.lean`](Graphcal/Static/RequiredBindability.lean).
It formalizes validation rule V002: required indexes, types, and dimensions must
be bindable, while parameters are inherently bindable input ports.

## Build and conformance check

Install [Elan](https://lean-lang.org/install/) and run from the repository root:

```sh
just formal
just formal-conformance
```

`just formal` checks the Lean library and rejects proof placeholders or custom
axioms. `just formal-conformance` then asks the Lean executable oracle for its
complete 20-case state matrix and compares every decision with Graphcal's real
parser, desugaring, and V002 production pass.

The checked-in `lean-toolchain` pins the Lean version. The package intentionally
has no third-party Lean dependencies for this initial specification.
