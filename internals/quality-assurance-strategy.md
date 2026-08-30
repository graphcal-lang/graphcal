# Quality Assurance Strategy for Graphcal

## Use of AI

We do fast design-implement-use iterations, especially in the alpha phase of Graphcal development, to validate the design of the language core and settle on a well-designed one.
We use AI coding agents extensively, with minimal review, to enable fast iterations.
Reviewing all the code changes made by AI while maintaining fast iteration is virtually impossible, as AI code generation is an order of magnitude faster than human review.\
At the same time, we track human review coverage using [git-vet](https://github.com/shunichironomura/git-vet "git-vet: Git-based vetting gate for tracked file contents.")
in the `default` channel.

## Verification-Guided Development

While AI coding agents can fix bugs and implement features very quickly, we find that they rarely proactively identify the underlying design choices that give rise to such bugs.
This makes it all the more important to get the design right from the start.
That is why we are formalizing Graphcal’s language design in Lean.\
The Lean code in `/formal` directory is composed of multiple independent modules, each of which proves some properties of Graphcal.
One module typically consists of four submodules: `Model`, `Spec`, `Reference`, and `Proofs`.
The `Spec` module defines the set of properties we expect Graphcal to have and represents our design for Graphcal; the `Reference` module is the Lean implementation of these specifications; the `Model` module defines the underlying data models; and the `Proofs` module proves that the `Reference` module's implementation satisfies `Spec`.
We also check that the Graphcal compiler is consistent with the `Reference` module by comparing their outputs for various inputs.\
We track human review coverage of the formalization code using [git-vet](https://github.com/shunichironomura/git-vet "git-vet: Git-based vetting gate for tracked file contents.")
in the `formal` channel.
