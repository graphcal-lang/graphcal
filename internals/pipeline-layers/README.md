# Graphcal pipeline-layer guard

This standalone tool is deliberately outside the product workspace. Both
`just lint` and `just test` run it; `just pipeline-layers` runs its tests and
checks the current dependency ratchet. From the repository root:

```sh
cargo run --locked --manifest-path internals/pipeline-layers/Cargo.toml -- report .
cargo run --locked --manifest-path internals/pipeline-layers/Cargo.toml -- check .
cargo run --locked --manifest-path internals/pipeline-layers/Cargo.toml -- prune .
```

## Policy and maintenance

`role-map.toml` explicitly assigns every discovered compiler/evaluator module,
including inline tests and literal-`#[path]` modules. Missing, stale, unreachable,
and undeclared entries fail. Roles describe boundaries, not SCC membership:

- **Contracts**: shared data, semantic primitives, and foundational syntax utilities.
- **Checking**: lowering, checking, specialization, and plan construction.
- **Interpreter**: expression execution, numerical kernels, and host invocation.
- **Loading**: project/source acquisition.
- **Facade**: orchestration and public adapters.

Contracts may consume only contracts; interpreter modules may consume contracts
and interpreter modules. Checking cannot consume loading or facade modules;
loading and facade modules are shells allowed to compose the other roles.
Checking may use provisional interpretation; capability/API tests separately
ensure that checking cannot invoke native hosts. Production and test-only edges
are distinct keys, with the same role policy and no blanket test exemption.

Selected HIR/TIR and project data modules retain the contracts role even where
builders or checker calls are currently colocated. Their upward dependencies
are debt to split, not a reason to relabel the entire module permissively.
Public result records, materialized-shape facts, imported-binding records, and
desugared AST aliases are data rather than their neighboring producers.

`baseline.toml` records exact `(source, target, test-only)` exceptions with
bootstrap file/line/form evidence and proposed deletion seams. These are
existing source couplings, not proof that the proposed splits are complete.
`check` rejects both new and stale exceptions. After removing a dependency,
`prune` deletes stale entries **only**, preserving surviving reviewed reasons;
it refuses to write if any new forbidden edge exists. Role changes and any new
exception require explicit architectural review. Neither SCC-wide exceptions
nor routine baseline regeneration are acceptable.

`init-role-map` and `init-baseline` are bootstrap helpers, not lint recipes.
`init-baseline` refuses to overwrite an existing baseline. Bootstrap evidence
is historical; use `report` for current source locations.

## Resolution and coverage limits

The analyzer indexes modules, declarations, imports, and re-exports before
scanning uses. Module identities keep separate package/path components. Explicit
imports, globs, relative aliases, `super`, lexical block-local imports, and
literal `$crate` paths resolve in their defining scopes. Re-export consumers
retain **both provider-boundary and final-producer edges**. Glob lookup preserves
export names; associated suffixes do not walk into unrelated sibling modules.
Visibility paths such as `pub(crate)` specify access scope, not consumption.

Separate `cfg` attributes are conjoined; `any`/`all`/`not` are evaluated as
Boolean cofactors, including empty-operator identities. Unknown feature/target
predicates remain conservatively production-capable unless a test restriction
excludes production. Conditional imports retain all possible targets.
Block-local modules, conditional/nonliteral `#[path]`, missing source files,
undeclared Rust files, and `include!` fail closed rather than being omitted.

This is **syntax-aware analysis, not rustc name resolution or a capability
proof**. It does not expand procedural/declarative macros, infer hygiene beyond
literal `$crate`, inspect build-script-generated sources, or infer method/UFCS
dependencies whose producer is not syntactically named. Macro token scanning is
conservative and can miss interpolated or concatenated paths. External crates
other than the two analyzed Graphcal crates are outside the role map. Negative
compile-time API assertions and native/Wasm semantic tests remain necessary.
