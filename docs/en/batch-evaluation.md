---
icon: material/transit-connection-variant
---

# Batch Evaluation

Sampling studies, scenario discovery, and optimization evaluate one model many
thousands of times. Graphcal serves a checked model to such analyses: the model
compiles once, and a client sends batches of parameter values and receives one
outcome per row, computed by the same typed DAG as `graphcal eval`.

The workflow has three steps:

1. Describe the model's interface once.
2. Start a session that varies selected parameters and reads selected outputs.
3. Send request batches and read the results.

## 1. Describe the model

Any entry file works. Domains (`min`, `max`) are published so that samplers
and optimizers know the search space:

```graphcal title="reliability.gcl"
pub index Mode = { Nominal, Degraded };

param load: Force(min: 0.0 N, max: 10_000.0 N);
param cycles: Int(min: 0, max: 100_000) = 10_000;
param mode: Key<Mode>;

assert load_positive = @load > 0.0 N;

pub node failure: Bool =
    @load > 8_000.0 N && @cycles > 50_000 && @mode == Mode#Degraded;
pub node margin: Force = 10_000.0 N - @load;
```

```bash
graphcal model describe reliability.gcl
```

The command prints a JSON document listing every parameter and every public
node:

```json
{
  "protocol_version": 2,
  "graphcal_version": "0.0.1-alpha.37",
  "interface_fingerprint": "sha256:…",
  "interface": {
    "parameters": [
      {
        "name": "load",
        "graphcal_type": "Force",
        "wire": { "kind": "quantity", "dimension": { "Length": 1, "Mass": 1, "Time": -2 }, "unit": "m*kg/s^2" },
        "domain": { "kind": "quantity", "min": 0.0, "max": 10000.0 },
        "has_default": false
      },
      …
    ],
    "outputs": [ … ]
  }
}
```

- `wire` is how values travel (see the table below). A type the protocol
  cannot carry, such as a recursive type, is listed with
  `"kind": "unsupported"` and a reason, never silently omitted.
- `unit` is the port's canonical SI unit. Values travel in it unless a
  session chooses another unit for the port (see below).
- `domain` is in the same unit. It bounds every element of an indexed
  parameter. A value outside it fails only its own row.

| Graphcal type | `kind` | Value |
|---------------|--------|-------|
| Quantity | `quantity` | A number in `unit` |
| `Complex<D>` | `complex` | Real and imaginary parts in `unit` |
| `Int`, `Bool` | `int`, `bool` | An integer or Boolean |
| `Key<I>` | `key` | A variant name, a `Fin` position counted from 0, or a coordinate in SI |
| `Datetime<S>` | `datetime` | Seconds since J2000 (2000-01-01T12:00:00), read in `S` |
| `T[I]` | `indexed` | One `T` per entry of `I`, in declaration order |
| Struct or union type | `algebraic` | One constructor and its fields |

- `interface_fingerprint` identifies this interface. Formulas may change
  without changing it; adding, removing, or retyping a port changes it.

## 2. Start a session

A session names the parameters it varies (`--input`) and the outputs it reads
(`--output`). Every parameter it does not vary keeps its default, so a
parameter without a default must be varied:

```bash
graphcal model serve reliability.gcl \
    --input load --input mode --output failure --output margin \
    --expect-fingerprint sha256:…
```

`--expect-fingerprint` makes the server refuse to start if the model changed
since it was described. Pass `--skip-fingerprint-check` instead when you start
a server by hand. One of the two is required.

To send or receive a quantity in another unit, add `--unit NAME=UNIT` for that
port. The unit is a unit expression resolved in the served file, as if written
after `->` there. It must measure the port's dimension and have a fixed scale:
base, `const`, and prelude units qualify, runtime units do not. The session's
domains are converted into the chosen unit:

```bash
graphcal model serve reliability.gcl \
    --input load --input mode --output failure --output margin \
    --unit load=kN --unit margin=kN \
    --expect-fingerprint sha256:…
```

The server reads request batches on stdin and writes results on stdout as
Arrow IPC streams, so it is driven by a client rather than a terminal.

## 3. Evaluate from Rust

The `graphcal-model-client` crate runs both commands and keeps Arrow
internal. Handles are typed (`Quantity`, `Complex`, `Int`, `Bool`, `Key`,
`Datetime`, `Indexed<K>`, or `Algebraic`), so each column takes values of the
port's kind:

```rust
use graphcal_model_client::{Bool, GraphcalCommand, Key, Model, Quantity};
use graphcal_model_protocol::{RowOutcome, UnitSpelling};

let model = Model::describe(GraphcalCommand::new("reliability.gcl"))?;
let load = model.input::<Quantity>("load")?;
let mode = model.input::<Key>("mode")?;
let failure = model.output::<Bool>("failure")?;

let mut session = model
    .session()
    .vary_in(&load, UnitSpelling::new("kN")?)
    .vary(&mode)
    .output(&failure)
    .spawn()?;
let degraded = mode.variant("Degraded")?;
let request = session
    .batch()
    .column(&load, [2.0, 9.0])?
    .column(&mode, [degraded.clone(), degraded])?
    .build()?;
let result = session.evaluate(&request)?;
for outcome in result.outcomes() {
    match outcome {
        RowOutcome::Success(row) => println!("failure: {}", row.get(&failure)?),
        RowOutcome::Failure(failure) => println!("failed: {}", failure.message()),
    }
}
session.shutdown()?;
```

[Tenax](https://github.com/shunichironomura/tenax) uses this client to sample
Graphcal models and run PRIM scenario discovery.

## Fix parameters with a wrapper file

To hold some parameters at values other than their defaults, write a small
wrapper that includes the model, and serve the wrapper instead. An `include`
of another file needs a package (see
[Multi-File Projects](language/multi-file.md)); here the model is
`src/plant/reliability.gcl` in a package named `plant`:

```graphcal title="src/plant/aging.gcl"
import plant.reliability::{ index Mode };

param load: Force(min: 0.0 N, max: 10_000.0 N);

include plant.reliability(load: @load, cycles: 80_000, mode: Mode#Degraded)
    ::{ pub failure, pub margin };
```

The fixed values are ordinary Graphcal expressions, so their units and types
are checked like any other binding. Marking an included output `pub` in the
brace list re-exports it, and a re-exported node is an output of the wrapper
just like a `pub node` it declares itself.

## Row outcomes

Every request row produces exactly one outcome, in request order:

| Outcome | When | Outputs |
|---------|------|---------|
| Success | The row evaluated | Every selected output |
| Assertion failed | An `assert` did not hold; optimizers can treat the row as infeasible | None |
| Input out of domain | A value lies outside its parameter's `min`/`max` | None |
| Evaluation error | Any other runtime failure, such as a division by zero | None |

Malformed streams, a request schema that differs from the session, NaN or
infinite values, and keys outside their index are protocol errors: the server stops
with exit code `1` instead of reporting a row.

## Writing another client

Clients in other languages talk to `graphcal model serve` directly:

1. Run `graphcal model describe` and select ports.
2. Start `graphcal model serve` with `--input`, `--output`, any `--unit`
   choices, and `--expect-fingerprint`.
3. Read the result-stream schema from stdout before writing to stdin. Its
   schema-level metadata lists `si_scales`: the size of each chosen unit in
   the canonical SI unit. The request schema carries the same metadata, and
   its domains are converted into the chosen units.
4. Write one request stream on stdin. For each request batch, read one result
   batch.
5. Close the request stream; the server finishes the result stream and exits
   with code `0`.

Schemas carry one `graphcal` metadata key whose value is JSON. The request
stream has one non-nullable field per varied parameter, in declaration order.
The result stream has two struct fields:

| Field | Children |
|-------|----------|
| `outputs` | One nullable field per read output, in declaration order; null exactly in failed rows |
| `outcome` | `status` (`UInt8`: `0` success, `1` assertion failed, `2` input out of domain, `3` evaluation error), `subject` (the failed assertion or parameter), `message` |

Quantities and datetimes are `Float64`, `Int` is `Int64`, and `Bool` is
`Boolean`. A key over a named index is a `Dictionary<Int32, Utf8>` resolved by
its dictionary value, never by its code; a key over `Fin(N)` is its `UInt32`
position; a key over a coordinate index is its `Float64` coordinate in SI,
which must equal one of the index's coordinates exactly. A complex value is a struct of
`re` and `im`, and an indexed value a `FixedSizeList` with one item per axis
entry. A struct type is a struct of its fields; a union is a struct of a
`constructor` key and a `variants` struct with one nullable child per
constructor, of which exactly the row's constructor is non-null.
