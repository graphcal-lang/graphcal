# Typed semantic generation

`graphcal-test-support` is a non-published workspace crate for generated
Graphcal programs. It keeps names, module ownership, dimensions, indexes,
declarations, references, and expected results in typed structures. Source
strings and paths are created only by `GeneratedProject::render` at the test or
fuzz boundary.

The initial generator provides:

- bounded dimensionless expression trees with an independent integer oracle;
- valid single-file projects containing a user base dimension, derived
  dimension, scaled unit, and named index;
- explicitly presented quantities with typed expected SI value, display label,
  display scale, and projected value;
- multi-file projects with two canonical owners exporting the same leaf name;
- typed import-alias renaming and module-storage permutation transforms;
- a controlled result-dimension mutation with an expected semantic error class;
- Proptest strategies with structural shrinking;
- a deterministic byte adapter reused by the `generated_project` fuzz target.

The evaluator properties require valid generated projects to reach TIR and
runtime evaluation, compare semantic and presentation output with independent
oracles, and verify owner-order and alias-renaming invariance. Formatter
properties require
idempotence for the same generated single-file programs. The fuzz target maps
arbitrary bytes to valid bounded models and treats a typed `InternalError` as a
finding.

Expand the typed model rather than adding source-template generators elsewhere.
Future presentation, struct, and multi-axis cases should add variants and
transforms here so Proptest and libFuzzer continue to share one semantic model.
