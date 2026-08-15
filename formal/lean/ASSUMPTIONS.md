# Assumptions and trusted boundary

The current formalization is checked by Lean 4.33.0 and makes no
Graphcal-specific `axiom` declarations. Its conclusions are nevertheless
relative to the following boundary.

## Trusted foundation

- Lean's kernel, logic, and standard-library definitions are trusted.
- The pinned Lean distribution and the machine executing it are trusted.
- CI additionally requests Lean's bundled environment checker, but that check
  does not make the model itself a correct description of Graphcal.

## Model assumptions

- Input to `ValueTypeSyntax` and `GenericArgumentSyntax` is already parsed and
  name-resolved. Parser, namespace, import, and registry correctness are out of
  scope.
- Owner-qualified identities are represented by typed natural-number IDs.
  Their allocation and correspondence to Rust identities are assumed.
- A `NominalTypeConstructor` carries its authoritative generic signature. The
  initial model does not prove that two occurrences of one identity carry the
  same registry signature.
- Dimensions are interpreted extensionally as total functions from canonical
  base identities to rational exponents. Expressions have finite support, but
  finiteness and canonical sparse normalization are not yet represented.
- Lean's rational exponents are unbounded for this model. Production Rust uses
  bounded exact rationals and can report overflow; that correspondence is not
  yet formalized.
- `NatExpr` currently models only closed literals, addition, and
  multiplication. Generic Nat variables and production polynomial
  normalization remain future work.
- Concrete index constructors carry positive cardinalities. Required index
  obligations and the term-level checking of coordinate-index constructors are
  not yet modelled.
- Bound payloads are an idealized static representation. Constant evaluation,
  ordering, finiteness, and production domain checking are out of scope.

## Claims intentionally not made

The model does not yet prove that:

- The prose specification is free from every design error.
- The Rust compiler implements these definitions.
- Every valid Graphcal type is represented by the initial model.
- Type checking or evaluation is sound.
- Graphcal programs cannot fail at runtime.

Those claims require later normalization, declarative term typing, evaluation,
and Rust correspondence milestones.
