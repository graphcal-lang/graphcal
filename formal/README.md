# Graphcal formal specifications

This Lake package contains the machine-checked normative specifications used by
Graphcal's verification-guided development workflow.

The first specification is
[`Graphcal/Static/RequiredBindability.lean`](Graphcal/Static/RequiredBindability.lean).
It formalizes validation rule V002: required indexes, types, and dimensions must
be bindable, while parameters are inherently bindable input ports.

## Build

Install [Elan](https://lean-lang.org/install/) and run:

```sh
cd formal
lake build --wfail
```

The checked-in `lean-toolchain` pins the Lean version. The package intentionally
has no third-party Lean dependencies for this initial specification.
