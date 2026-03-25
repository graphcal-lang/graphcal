# 23 — Automatic Differentiation & GPU/TPU Batch Evaluation

**Status:** Exploration / RFC
**Date:** 2026-03-25

## Motivation

Engineering calculations frequently require:

1. **Sensitivity analysis** — How does output Y change when parameter X varies? (∂Y/∂X)
2. **Optimization** — Find parameter values that minimize/maximize an objective (gradient-based).
3. **Uncertainty propagation** — Propagate parameter uncertainties through the DAG (linear approximation via Jacobian).
4. **Batch evaluation** — Evaluate the same graph for thousands/millions of parameter combinations (Monte Carlo, Sobol sequences, parameter sweeps).

Graphcal's DAG-based computation model is a natural fit for automatic differentiation (AD) — the graph *is* the computation trace. Combined with batch evaluation on GPU/TPU, this would make graphcal a compelling platform for engineering design exploration.

## Background: AD Approaches

### Forward-Mode AD (Dual Numbers)

**Idea:** Augment every scalar value with its derivative. Instead of computing `f(x) = y`, compute `f(x + εẋ) = y + εẏ` where ε² = 0.

**Implementation:** Replace `f64` with a dual number `(value: f64, derivative: f64)`. Apply chain rule at each operation:
- `(a, a') + (b, b') = (a+b, a'+b')`
- `(a, a') * (b, b') = (a*b, a'*b + a*b')`
- `sin(a, a') = (sin(a), a' * cos(a))`

**Tradeoffs:**
- ✅ Simple to implement — just change the number type
- ✅ No tape/memory overhead
- ✅ Efficient for few inputs, many outputs (∂y₁/∂x, ∂y₂/∂x, ... in one pass)
- ❌ Cost scales with number of *input* parameters — one forward pass per parameter
- ❌ For N params → N outputs, need N forward passes (vs 1 reverse pass)

**Best for graphcal:** Sensitivity of many outputs w.r.t. a single parameter (common in engineering — "what if mass increases by 1%?").

### Reverse-Mode AD (Backpropagation)

**Idea:** Record a "tape" of operations during forward evaluation, then propagate adjoints backward from outputs to inputs.

**Implementation:** During forward pass, record each operation and its inputs. Then, starting from output, propagate `∂output/∂node` backward using the chain rule.

**Tradeoffs:**
- ✅ Efficient for many inputs, few outputs (∂y/∂x₁, ∂y/∂x₂, ... in one pass)
- ✅ Cost scales with number of *outputs*, not inputs
- ❌ Requires storing the entire computation tape (memory)
- ❌ More complex implementation

**Best for graphcal:** Gradient of a single objective w.r.t. all parameters (optimization).

### Source-to-Source Transformation (Enzyme / JAX-style)

**Idea:** Transform the program/IR itself to produce a differentiated version. Operates on the compiler IR rather than runtime values.

**Implementation:** At compile time (or JIT time), analyze the computation graph and generate a new graph that computes derivatives alongside or instead of the original computation.

**Tradeoffs:**
- ✅ Best performance — compiler can optimize the derivative code
- ✅ No runtime overhead from tape recording
- ✅ Can differentiate through control flow, loops, etc.
- ❌ Most complex to implement
- ❌ Requires deep compiler integration

**Best for graphcal:** Long-term optimal solution, especially since graphcal already has a well-structured IR pipeline (AST → IR → TIR → ExecPlan).

## Proposed Design for Graphcal

### Option A: Dual-Number Forward-Mode (Recommended for Phase 1)

This is the simplest approach and fits graphcal's architecture naturally.

#### Core Idea

Extend `RuntimeValue::Scalar(f64)` to optionally carry derivative information. The existing evaluation pipeline remains unchanged — we just swap the number type.

#### Language-Level Syntax

```graphcal
# Compute partial derivative of node w.r.t. a param
node sensitivity = diff(@delta_v, @spacecraft_mass)

# Gradient: derivatives w.r.t. all params (returns a struct)
node gradient = grad(@total_cost)

# Jacobian of indexed node w.r.t. indexed param
node J = jacobian(@outputs, @inputs)
```

#### Implementation Sketch

**Step 1: Generic number type**

Replace `f64` with a trait:

```rust
trait DiffScalar: Copy + Add + Mul + ... {
    fn constant(v: f64) -> Self;
    fn variable(v: f64, index: usize) -> Self;
    fn value(&self) -> f64;
    fn derivative(&self, index: usize) -> f64;
}

// Concrete implementations
struct F64Value(f64);  // no-AD fast path
struct DualNumber { val: f64, der: f64 }  // single derivative
struct MultiDual { val: f64, der: Vec<f64> }  // multiple derivatives
```

**Step 2: Differentiation rules for built-ins**

Every built-in function already has a known derivative:

| Function | Derivative |
|----------|-----------|
| `sqrt(x)` | `1 / (2 * sqrt(x))` |
| `sin(x)` | `cos(x)` |
| `cos(x)` | `-sin(x)` |
| `exp(x)` | `exp(x)` |
| `ln(x)` | `1/x` |
| `abs(x)` | `sign(x)` (discontinuous at 0) |
| `atan2(y,x)` | `(x*dy - y*dx) / (x² + y²)` |
| `min(a,b)` | subgradient (derivative of the active branch) |
| `max(a,b)` | subgradient (derivative of the active branch) |
| `clamp(x,lo,hi)` | `dx` if `lo < x < hi`, else `0` |

**Step 3: Dimension checking for derivatives**

A derivative `∂Y/∂X` has dimension `dim(Y) / dim(X)`. The type system must verify this:

```graphcal
param mass: Scalar<Mass> = 100 kg
node force: Scalar<Force> = @mass * 9.81 m/s^2

# diff(@force, @mass) has dimension Force/Mass = Acceleration ✓
node sensitivity: Scalar<Acceleration> = diff(@force, @mass)
```

This is naturally handled by graphcal's dimension algebra.

**Step 4: Handling control flow**

`if/else` and `match` expressions: Differentiate through the taken branch. The derivative is discontinuous at branch boundaries — this is standard for AD and acceptable for engineering calculations (sensitivity at a point, not global smoothness).

`scan`/`unfold`: Forward-mode AD naturally composes through sequential iteration — each step's dual numbers carry derivatives forward.

#### What This Doesn't Cover

- Second-order derivatives (Hessians) — need nested dual numbers or hyper-dual numbers
- Differentiation through discrete operations (`count`, label comparisons)
- Stochastic/sampling operations

### Option B: Tape-Based Reverse-Mode (Phase 2)

For optimization use cases where we need gradients of a scalar objective w.r.t. many parameters.

#### Core Idea

During forward evaluation, record operations to a tape. Then replay backward to compute all partial derivatives in one pass.

```rust
struct Tape {
    operations: Vec<TapeOp>,
}

enum TapeOp {
    Add { result: NodeId, lhs: NodeId, rhs: NodeId },
    Mul { result: NodeId, lhs: NodeId, rhs: NodeId },
    Sin { result: NodeId, input: NodeId },
    // ... one variant per primitive operation
}
```

#### Graphcal Advantage

Graphcal's DAG is *already* an explicit computation graph with topological ordering. We don't need to *record* a tape at runtime — the graph *is* the tape. Reverse-mode AD is simply a reverse topological traversal of the existing DAG:

```
Forward:  param₁ → node_a → node_b → node_c (output)
Backward: param₁ ← node_a ← node_b ← node_c (adjoint = 1.0)
```

This makes reverse-mode significantly simpler for graphcal than for general-purpose languages.

#### Language-Level Syntax

```graphcal
# Reverse-mode gradient of scalar output w.r.t. all params
node cost_gradient = grad(@total_mission_cost)
# Returns a struct with one field per param: { mass: ..., thrust: ..., ... }
```

### Option C: IR-Level Source Transformation (Phase 3 / Long-Term)

Transform the TIR to generate derivative computation nodes alongside the original nodes. This produces the most efficient code and integrates with potential future compilation targets (LLVM, GPU kernels).

## GPU/TPU Batch Evaluation

### Motivation

Parameter sweeps, Monte Carlo analysis, and Sobol sensitivity analysis require evaluating the same graph for many input combinations. This is embarrassingly parallel and maps perfectly to GPU execution.

### Architecture Options

#### Option 1: Compile to Burn Framework (Recommended)

[Burn](https://burn.dev) is a Rust deep-learning framework with multiple backends:
- **wgpu** — WebGPU, works on all platforms (including browser via WASM)
- **candle** — Hugging Face's Rust ML framework
- **tch** — LibTorch/PyTorch C++ bindings
- **ndarray** — CPU fallback
- **CUDA** (via candle/tch)

**Why Burn:**
- Pure Rust, type-safe tensor operations
- Backend-agnostic — write once, run on CPU/GPU/TPU
- Already supports autodiff as a backend decorator (`Autodiff<Backend>`)
- Active development, growing ecosystem

**Compilation strategy:**

```
graphcal source
    ↓ parse
AST → IR → TIR
    ↓ compile
Burn tensor graph (batch dimension added automatically)
    ↓ execute
wgpu/CUDA/CPU backend
```

Each `param` becomes a 1-D tensor (batch dimension), each `node` becomes a tensor operation. The graph structure is preserved — we just vectorize across the batch dimension.

```rust
// Single evaluation (current)
let mass: f64 = 100.0;
let force: f64 = mass * 9.81;

// Batch evaluation (compiled to Burn)
let mass: Tensor<B, 1> = Tensor::from_floats([100.0, 110.0, 120.0, ...]);
let force: Tensor<B, 1> = mass.clone() * 9.81;
// All 1000 evaluations happen in parallel on GPU
```

**Burn's autodiff backend** gives us reverse-mode AD for free:

```rust
type MyBackend = Autodiff<Wgpu>;
// Tensors on this backend automatically track gradients
let mass = Tensor::<MyBackend, 1>::from_floats([100.0, ...]).require_grad();
let cost = compute_graph(&mass, &thrust, ...);
let grads = cost.backward();  // reverse-mode AD through entire graph
let dcost_dmass = mass.grad(&grads);  // ∂cost/∂mass for each batch element
```

#### Option 2: Compile to wgpu Compute Shaders Directly

Lower the graphcal DAG to WGSL (WebGPU Shading Language) compute shaders.

**Pros:**
- Maximum control, minimal dependencies
- Can target WebGPU in browser (future graphcal playground)
- No ML framework overhead

**Cons:**
- Must implement AD ourselves in WGSL
- No automatic kernel fusion/optimization
- Significant implementation effort

#### Option 3: Compile to XLA / StableHLO (for TPU)

[StableHLO](https://github.com/openxla/stablehlo) is the portable IR for XLA (the compiler behind JAX/TensorFlow on TPU).

**Pros:**
- Direct TPU access
- XLA's optimizer handles fusion, layout, etc.
- JAX-like `vmap` semantics for batching

**Cons:**
- C++ dependency (StableHLO/XLA)
- Complex build setup
- Overkill unless TPU is a primary target

#### Option 4: Generate JAX/Python Code (Pragmatic Bridge)

Since Phase 13 plans Python interop anyway, we could:
1. Compile graphcal → JAX Python code
2. Use JAX's `jit`, `grad`, `vmap` for batch + AD
3. Return results to graphcal

**Pros:**
- Leverage JAX's mature AD + batching + XLA compilation
- Works on GPU + TPU immediately
- Minimal Rust-side implementation

**Cons:**
- Python dependency
- Serialization overhead
- Not self-contained

### Recommended Batch Evaluation Architecture

```
                    ┌─────────────────────────────────────┐
                    │         graphcal compiler            │
                    │  AST → IR → TIR → ExecPlan          │
                    └──────────────┬──────────────────────┘
                                   │
                    ┌──────────────┴──────────────────────┐
                    │         Execution Backends           │
                    │                                      │
          ┌─────────┤                                      │
          │         │                                      │
          ▼         ▼                                      ▼
   ┌──────────┐  ┌───────────────┐              ┌──────────────────┐
   │ Interpret │  │  Burn Backend │              │  JAX/Python      │
   │ (current) │  │               │              │  Backend         │
   │           │  │ - wgpu (GPU)  │              │                  │
   │ - f64     │  │ - CUDA        │              │ - jit + vmap     │
   │ - single  │  │ - CPU         │              │ - GPU/TPU        │
   │ - no AD   │  │ - autodiff    │              │ - autodiff       │
   │           │  │ - batch eval  │              │ - batch eval     │
   └──────────┘  └───────────────┘              └──────────────────┘
```

## Phased Implementation Plan

### Phase A: Forward-Mode AD via Dual Numbers (Minimal)

**Scope:** `diff(@output, @param)` for scalar nodes.

1. Add `DualNumber` type alongside `f64` in evaluation
2. Implement derivative rules for all built-in functions
3. Add `diff(node_ref, param_ref)` built-in function
4. Dimension inference for derivatives (`dim(Y)/dim(X)`)
5. Handle control flow (differentiate taken branch)

**Effort:** ~2-3 weeks. Requires:
- New `DualNumber` struct in `graphcal-registry`
- Parameterize `eval_expr` over number type (or add a parallel eval path)
- Derivative rules table in `builtins.rs`
- New `diff` expression kind in AST/IR/TIR

### Phase B: Reverse-Mode AD via DAG Traversal

**Scope:** `grad(@scalar_output)` returning derivatives w.r.t. all params.

1. Reverse topological traversal of the DAG
2. Accumulate adjoint values at each node
3. Return gradient struct (one field per param)
4. Combine with forward-mode for mixed-mode AD

**Effort:** ~2-3 weeks. Builds on Phase A's derivative rules.

### Phase C: Batch Evaluation via Burn

**Scope:** `graphcal batch eval --sweep params.json` evaluating 10K+ points on GPU.

1. Add `graphcal-backend-burn` crate
2. Compile TIR → Burn tensor operations
3. Map `param` → input tensor (batch dim), `node` → tensor ops
4. Support parameter sweep specifications (Cartesian, Sobol, LHS)
5. Output results as Arrow/Parquet

**Effort:** ~4-6 weeks. Requires:
- New `graphcal-backend-burn` crate
- TIR → Burn compilation pass
- Sweep specification format
- Output serialization

### Phase D: Batch AD on GPU

**Scope:** Combine Phases B and C — compute gradients for all batch points on GPU.

1. Use Burn's `Autodiff<Wgpu>` backend
2. Batch gradient computation
3. Sobol-based global sensitivity analysis (built on batch gradients)

**Effort:** ~2 weeks if Phases B and C are complete.

## Integration with Existing Features

### With Indexed Values

Indexed nodes expand to per-label DAG nodes. AD operates on the expanded graph:

```graphcal
cat Maneuver { Departure, Arrival }
param delta_v[Maneuver]: Scalar<Velocity> = { Departure: 3.0 km/s, Arrival: 2.0 km/s }
node fuel_mass[Maneuver] = @spacecraft_mass * (exp(@delta_v[m] / @exhaust_velocity) - 1)

# Derivative of each fuel_mass w.r.t. each delta_v — a 2×2 Jacobian
node J = jacobian(@fuel_mass, @delta_v)
```

### With System Dynamics (scan/unfold)

Forward-mode AD naturally threads through `scan`/`unfold` iterations. Each time step carries dual numbers forward, accumulating sensitivity through the entire simulation:

```graphcal
range TimeStep(0.0 s, 100.0 s, step: 0.1 s)
node position[TimeStep] = unfold 0.0 m with prev, curr {
    @position[prev] + @velocity[prev] * (curr - prev)
}

# Sensitivity of final position to initial velocity
node sensitivity = diff(@position[TimeStep::last], @initial_velocity)
```

### With Dimensions & Units

Derivatives have well-defined dimensions: `dim(∂Y/∂X) = dim(Y) / dim(X)`. This integrates seamlessly with graphcal's dimension algebra — the type checker can verify derivative dimensions automatically.

### With Python Interop (Phase 13)

Batch evaluation results and gradients can be exposed via PyO3:

```python
import graphcal

model = graphcal.load("rocket.gcl")
results = model.batch_eval(
    params={"mass": np.linspace(50, 200, 1000)},
    backend="gpu"
)
gradients = model.grad("total_cost", backend="gpu")
```

## Open Questions

1. **Syntax for `diff`/`grad`**: Should these be built-in functions, expression-level operators, or declaration attributes?

2. **Non-differentiable operations**: How to handle `floor`, `round`, `count`, `match` on labels? Options:
   - Error at compile time if diff encounters non-differentiable op
   - Use subgradients / straight-through estimators
   - Mark non-differentiable paths explicitly

3. **Higher-order derivatives**: Support `diff(diff(@y, @x), @x)` for second derivatives? Requires nested dual numbers (hyper-dual numbers).

4. **Struct derivatives**: If a node returns a struct, what does `diff(@struct_node, @param)` return? A struct of derivatives? A Jacobian?

5. **Batch semantics**: Should batch evaluation be a CLI mode only, or should it have language-level support (e.g., `param mass: Scalar<Mass>[1000] = sweep(50 kg, 200 kg)`)?

6. **Backend selection**: Should the backend be a CLI flag (`--backend gpu`), a project config, or auto-detected?

7. **Memory model for GPU batch**: For large graphs with many intermediate nodes, GPU memory may be a constraint. Strategies:
   - Checkpoint/recompute (trade compute for memory)
   - Stream batches (evaluate in chunks)
   - Fuse operations (eliminate intermediate tensors)

## Rust Ecosystem Status (as of March 2026)

| Capability | Best Current Option | Maturity |
|-----------|-------------------|----------|
| Forward-mode AD | `ad-trait` crate, `autodiff` crate | Usable |
| Reverse-mode AD | Burn (`burn-autodiff`), `ad-trait` | Usable |
| Compiler-level AD | `#[autodiff]` (Enzyme in rustc nightly) | Experimental (RFC pending, nightly-only, cross-crate broken) |
| Cross-platform GPU compute | wgpu | Mature |
| NVIDIA GPU compute | cudarc (v0.19) | Mature |
| Write GPU shaders in Rust | rust-gpu (SPIR-V codegen) | Experimental (all major GPU backends demonstrated July 2025) |
| Full DL framework | Burn (v0.20) | Active development, CubeCL replacing candle backend |
| Automatic batching (vmap-style) | None in Rust | Not available |
| Composable transforms (JAX-like) | None in Rust | Not available |

### Notable Details

- **Rust `#[autodiff]`** (tracking issue `rust-lang/rust#124509`): Uses Enzyme at LLVM IR level. Available on `rustc 1.96.0-nightly` with `-Zautodiff=Enable`. Batching support merged early 2025. However: not shipped in standard nightly distribution, doesn't work across crate boundaries, and fails on `impl` blocks. Rust's aliasing guarantees (`&` vs `&mut`) give Enzyme performance advantages over C++ — this is a [Rust project goal](https://rust-lang.github.io/rust-project-goals/2024h2/Rust-for-SciComp.html) for scientific computing.

- **`ad-trait`** (April 2025, [arXiv:2504.15976](https://arxiv.org/abs/2504.15976)): New crate supporting both forward and reverse mode via operator overloading. Integrates with nalgebra/ndarray.

- **Burn v0.20**: Most active Rust DL framework. Backends: wgpu (cross-platform), CUDA/ROCm (via CubeCL), LibTorch, ndarray (CPU, supports `no_std`). Supports WebAssembly via wgpu/WebGPU. Autodiff is a composable backend layer (`Autodiff<Wgpu>`). Candle backend deprecated in favor of CubeCL.

- **rust-gpu**: Compiles Rust to SPIR-V via `rustc_codegen_spirv`. Major milestone in July 2025: all major GPU backends demonstrated from single Rust codebase. SPIR-V consumed by wgpu via naga translation.

- **No vmap equivalent in Rust**: JAX's `vmap` (automatic batching) has no Rust equivalent. Batching must be done manually via tensor operations or by compiling to a framework that supports it (Burn tensors, or generating JAX code).

## References

- **Rust `#[autodiff]`**: Nightly feature using Enzyme for LLVM-level AD. Tracks `rust-lang/rust#124509`. Currently experimental but promising for source-level AD in Rust.
- **ad-trait**: Fast forward+reverse AD via operator overloading. https://arxiv.org/abs/2504.15976
- **Burn framework**: https://burn.dev — Multi-backend tensor framework in Rust with built-in autodiff (v0.20).
- **JAX**: https://jax.readthedocs.io — `grad`, `vmap`, `jit` composable transformations. Gold standard for batch+AD on GPU/TPU.
- **Enzyme**: https://enzyme.mit.edu — LLVM-level AD. Powers Rust's `#[autodiff]`.
- **rust-gpu**: https://rust-gpu.github.io — Compile Rust to SPIR-V for GPU shaders.
- **wgpu**: https://wgpu.rs — WebGPU implementation in Rust. Portable GPU compute.
- **cudarc**: https://docs.rs/cudarc — Safe Rust bindings for CUDA driver API.
- **Dual numbers**: Clifford (1873). Modern treatment in "Automatic Differentiation in Machine Learning: a Survey" (Baydin et al., 2018).
