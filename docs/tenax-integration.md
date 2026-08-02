---
icon: material/transit-connection-variant
---

# Tenax Integration

Graphcal can serve a checked engineering calculation as a persistent
[Tenax](https://github.com/shunichironomura/tenax) model. The process compiles
once, receives Arrow record batches on stdin, evaluates each row through the
same typed DAG used by `graphcal eval`, and writes Arrow results on stdout.

The initial interface is intentionally strict: Tenax stdio protocol version 1
and Arrow schema version 2. Unsupported Graphcal values are rejected during
startup rather than flattened, renamed, or converted without an explicit type.

## 1. Define a model

Declare finite sampling domains on every numeric input, use a concrete named
index for each categorical input, and make selected Boolean outputs public:

```graphcal title="reliability.gcl"
pub index Mode = { Nominal, Degraded };

param load: Force(min: 0.0 N, max: 10_000.0 N);
param cycles: Int(min: 0, max: 100_000);
param mode: Key<Mode>;

pub node failure: Bool =
    @load > 8_000.0 N
    && @cycles > 50_000
    && @mode == Mode.Degraded;
```

The interface above discovers as:

- `load`: continuous `Float64`, bounds `0..10000`, unit `N`;
- `cycles`: integer `Int64`, bounds `0..100000`;
- `mode`: `Dictionary<Int32, Utf8>`, categories `Degraded`, `Nominal` in lexical order;
- `failure`: Boolean output.

Dictionary codes are not semantic identities. Graphcal always resolves a
categorical request by its dictionary **value**, so a sender may assign codes
in any valid order.

## 2. Start the persistent server

```bash
graphcal model serve reliability.gcl --output failure
```

Repeat `--output` to select multiple public Boolean nodes. Discovery preserves
source declaration order even when flags are given in another order.

Do not run this command as an interactive text program. Stdout consists of two
concatenated Arrow IPC streams and must be connected to Tenax or another
protocol-compatible client. Logs and errors use stderr.

## 3. Spawn it from Tenax

A Rust Tenax client can spawn the command, inspect discovery, and reuse the same
child for multiple requests:

```rust
use std::process::Command;
use tenax::{
    EvalRequest, EvaluationId, Evaluator, Feature, InputChunk, StdioEvaluator,
};

let mut command = Command::new("graphcal");
command.args([
    "model", "serve", "reliability.gcl",
    "--output", "failure",
]);
let evaluator = StdioEvaluator::spawn(command)?;
let schema = evaluator.schema();

let inputs = InputChunk::new(
    schema,
    vec![
        Feature::continuous("load", vec![2_000.0, 9_000.0])?,
        Feature::integer("cycles", vec![10_000, 80_000])?,
        Feature::categorical(
            "mode",
            vec!["Nominal".to_owned(), "Degraded".to_owned()],
        )?,
    ],
)?;
let request = EvalRequest::new(EvaluationId::new(1), 42, inputs);
let result = evaluator.evaluate(vec![request]).next().unwrap()?;

// More evaluate(...) calls reuse the compiled Graphcal process.
let status = evaluator.shutdown()?;
assert!(status.success());
# Ok::<(), Box<dyn std::error::Error>>(())
```

Graphcal models are deterministic, so the request seed is validated for
protocol consistency and otherwise ignored.

## 4. Sample and run PRIM

Tenax sampling and analysis use the discovered domains directly:

```rust
use tenax::{
    Evaluator, Objective, Prim, PrimConfig, evaluation_to_dataset,
    sample_latin_hypercube,
};

let request = sample_latin_hypercube(evaluator.schema(), 5_000, 0x5eed, 1)?;
let retained = request.clone();
let result = evaluator.evaluate(vec![request]).next().unwrap()?;
let failure = evaluator.schema().output_position("failure")?;
let dataset = evaluation_to_dataset(
    evaluator.schema(), retained, result, failure,
)?;
let config = PrimConfig::new(0.05, 0.05, 0.05, Objective::Lenient1)?;
let discovered = Prim::new(&dataset, config).find_box();
# Ok::<(), Box<dyn std::error::Error>>(())
```

Always call `shutdown()` when finished. Dropping an incomplete result iterator
makes stream reuse ambiguous, so Tenax terminates that child.

## Supported interface

### Inputs

| Graphcal type | Requirement | Arrow type |
|---------------|-------------|------------|
| Quantity / `Dimensionless` | Finite closed `min` and `max`; finite width | `Float64` |
| `Int` | Closed `i64` `min` and `max` | `Int64` |
| `Key<I>` | Concrete non-empty named index | `Dictionary<Int32, Utf8>` |

Dimensioned quantities are exposed in a canonical scale-1 SI unit expression;
bounds and request values use that same unit. `Dimensionless` omits unit
metadata. Parameters with defaults remain inputs because every Tenax v2 request
contains every discovered input.

### Outputs

Each selected value must be a directly declared, explicitly public, scalar
`Bool` node. Graphcal preserves source order and rejects duplicate names across
the complete input/output interface.

Graphcal's prepared evaluator itself also supports Boolean, datetime, complex,
algebraic, key, and recursively indexed bindings and outputs. Those richer
families are not silently forced through schema v2; they require the planned
recursive Arrow protocol.

## Failure behavior

One valid request batch produces exactly one result batch with the same row
count, ID, and row order.

- Success: status `0`, every output non-null, no failure message.
- Ordinary Graphcal runtime/domain/assertion failure: status `1`, every output
  null, diagnostic present.
- Defensive binding rejection beyond the shared protocol checks: status `4`.

Malformed IPC, an incompatible request schema, null input/context values,
non-finite or out-of-domain numeric data, unknown categorical values, invalid
dictionary keys, inconsistent IDs/seeds, duplicate IDs, and internal evaluator
invariants are process errors. They do not become synthetic failed rows.

## Protocol lifecycle

Startup order is fixed to avoid pipe deadlocks:

1. Graphcal loads, checks, and prepares the project.
2. Stdout receives a schema-only discovery stream and its EOS marker.
3. Stdout receives the result-stream schema.
4. Graphcal opens and reads the stdin request stream.
5. Each non-empty request batch is evaluated sequentially.
6. Request EOS finishes the result stream and exits successfully.

A reader for the first stdout stream must not buffer bytes past its EOS unless
it can return those bytes for the second stream. Arrow's unbuffered stream
reader is suitable.

## Startup troubleshooting

`graphcal model serve` reports configuration failures before writing stdout.
Common errors include:

- a numeric parameter has no `min`, no `max`, or a one-sided/non-finite range;
- an input is `Bool`, datetime, complex, algebraic, indexed, a coordinate key,
  or a finite key, none of which schema v2 can represent;
- the selected output is unknown, private, duplicated, or not scalar `Bool`;
- an input and output share a field name;
- project dependencies or WASM plugins fail normal Graphcal loading checks.

Use `graphcal check reliability.gcl` first for source diagnostics, then inspect
`graphcal model serve ...` stderr for interface-capability errors. Never decode
stdout as text.
