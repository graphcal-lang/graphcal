mod arithmetic;
mod collections;
mod control;
mod functions;

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_compiler::desugar::resolved_ast::{Expr, ExprKind, MulDivOp, UnitExpr};
use graphcal_compiler::syntax::names::{FieldName, ScopedName, StructTypeName};

use graphcal_compiler::registry::builtins::BuiltinFunction;
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::types::{Registry, UnitScale};
use graphcal_compiler::tir::typed::ResolvedDomainConstraint;

pub use graphcal_compiler::registry::runtime_value::RuntimeValue;

/// Immutable evaluation environment shared across all expression evaluations.
///
/// Bundles built-in constants, built-in functions, the type/unit registry,
/// and source information for diagnostics, plus optional unfold context
/// for evaluating `unfold(...)` expressions inline.
pub struct EvalContext<'a> {
    pub builtin_consts: &'a HashMap<&'a str, f64>,
    pub builtin_fns: &'a HashMap<&'a str, BuiltinFunction>,
    pub registry: &'a Registry,
    pub src: &'a NamedSource<Arc<String>>,
    /// When set, enables inline evaluation of `ExprKind::Unfold` expressions.
    /// Contains the name of the node being evaluated and the declared types map.
    pub unfold_context: Option<UnfoldContext<'a>>,
    /// The enclosing file's full TIR.
    ///
    /// Used by [`eval_inline_dag_call`] to translate `@dag(args)::out` /
    /// `@alias.dag(args)::out` paths to canonical
    /// [`DagId`](graphcal_compiler::syntax::dag_id::DagId)s via
    /// [`graphcal_compiler::tir::typed::TIR::lookup_call_target`] and to
    /// reach the file's flat per-DAG body map. Shared across nested inline
    /// calls so a dag body invoking another dag can still resolve it.
    pub tir: &'a graphcal_compiler::tir::typed::TIR,
    /// Resolved domain constraints declared on struct/union member fields,
    /// keyed by `(struct type name, field name)`. Looked up at every
    /// `ExprKind::StructConstruction` to validate field values immediately.
    /// `None` means "skip the check" (used by paths that haven't resolved
    /// constraints yet, e.g. const-bound evaluation inside `exec_plan`).
    pub struct_field_constraints:
        Option<&'a HashMap<(StructTypeName, FieldName), ResolvedDomainConstraint>>,
}

/// Context required to evaluate an `unfold(...)` expression inline.
///
/// Provides the self-referencing node name and declared types needed
/// to look up the range index for iterative evaluation.
pub struct UnfoldContext<'a> {
    pub self_name: &'a str,
    pub declared_types: &'a HashMap<ScopedName, DeclaredType>,
}

impl EvalContext<'_> {
    /// Build a `GraphcalError::EvalError` using this context's source.
    pub fn eval_error(
        &self,
        message: impl Into<String>,
        span: graphcal_compiler::syntax::span::Span,
    ) -> GraphcalError {
        GraphcalError::EvalError {
            message: message.into(),
            src: self.src.clone(),
            span: span.into(),
        }
    }

    /// Build a `GraphcalError::InternalError` using this context's source.
    pub fn internal_error(
        &self,
        message: impl Into<String>,
        span: graphcal_compiler::syntax::span::Span,
    ) -> GraphcalError {
        GraphcalError::InternalError {
            message: message.into(),
            src: self.src.clone(),
            span: span.into(),
        }
    }
}

/// Evaluate an expression given a set of resolved values and built-in functions.
/// Returns a `RuntimeValue` (scalar or struct).
///
/// # Errors
///
/// Returns a [`GraphcalError`] if the expression references an undefined variable,
/// constant, or function.
#[expect(clippy::too_many_lines, reason = "large match on ExprKind variants")]
pub fn eval_expr(
    expr: &Expr,
    values: &HashMap<ScopedName, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match &expr.kind {
        ExprKind::Number(n) => Ok(RuntimeValue::Scalar(*n)),
        ExprKind::Integer(n) => Ok(RuntimeValue::Int(*n)),
        ExprKind::StringLiteral(_) => {
            Err(ctx.eval_error("unexpected string literal in evaluation context", expr.span))
        }
        ExprKind::UnitLiteral { value, unit } => {
            let scale = resolve_unit_scale(unit, values, local_values, ctx)?;
            Ok(RuntimeValue::Scalar(*value * scale))
        }
        ExprKind::Bool(b) => Ok(RuntimeValue::Bool(*b)),
        ExprKind::VariantLiteral { index, variant } => Ok(RuntimeValue::Label {
            index_name: index.value.clone(),
            variant: variant.value.clone(),
        }),
        ExprKind::GraphRef(ident) => values.get(&ident.value).cloned().ok_or_else(|| {
            ctx.eval_error(
                format!("undefined graph reference `@{}`", ident.value),
                expr.span,
            )
        }),
        ExprKind::ConstRef(ident) => values
            .get(&ident.value)
            .cloned()
            .or_else(|| {
                ctx.builtin_consts
                    .get(ident.value.member())
                    .map(|v| RuntimeValue::Scalar(*v))
            })
            .or_else(|| {
                // Nat generic params are stored as local values (e.g., `N` from `N: Nat`)
                // and may be referenced in expression position as ConstRef (uppercase).
                // Locals are always bare names (no module qualification).
                if ident.value.is_qualified() {
                    None
                } else {
                    local_values.get(ident.value.member()).cloned()
                }
            })
            .ok_or_else(|| {
                ctx.eval_error(format!("undefined constant `{}`", ident.value), expr.span)
            }),
        ExprKind::LocalRef(ident) => {
            local_values
                .get(ident.name.as_str())
                .cloned()
                .ok_or_else(|| {
                    ctx.eval_error(
                        format!("undefined local variable `{}`", ident.name),
                        expr.span,
                    )
                })
        }

        // --- Arithmetic (delegated) ---
        ExprKind::BinOp { op, lhs, rhs } => {
            arithmetic::eval_binop_expr(expr, *op, lhs, rhs, values, local_values, ctx)
        }
        ExprKind::UnaryOp { op, operand } => {
            arithmetic::eval_unaryop_expr(expr, *op, operand, values, local_values, ctx)
        }

        // --- Function calls (delegated) ---
        ExprKind::FnCall { name, args, .. } => {
            functions::eval_fn_call(expr, name, args, values, local_values, ctx)
        }

        // --- Control flow (delegated) ---
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => control::eval_if(
            expr,
            condition,
            then_branch,
            else_branch,
            values,
            local_values,
            ctx,
        ),
        ExprKind::Match { scrutinee, arms } => {
            control::eval_match(expr, scrutinee, arms, values, local_values, ctx)
        }

        // --- Collections (delegated) ---
        ExprKind::MapLiteral { entries } => {
            collections::eval_map_literal(entries, values, local_values, ctx)
        }
        ExprKind::ForComp { bindings, body } => {
            collections::eval_for_comp(bindings, body, values, local_values, ctx)
        }
        ExprKind::IndexAccess { expr: inner, args } => {
            collections::eval_index_access(expr, inner, args, values, local_values, ctx)
        }
        ExprKind::Scan {
            source,
            init,
            acc_name,
            val_name,
            body,
        } => collections::eval_scan(
            source,
            init,
            acc_name,
            val_name,
            body,
            values,
            local_values,
            ctx,
        ),

        // --- Passthrough (unit/display annotations are handled at the type level) ---
        ExprKind::Convert { expr: inner, .. } | ExprKind::DisplayTimezone { expr: inner, .. } => {
            eval_expr(inner, values, local_values, ctx)
        }

        // --- Field access ---
        ExprKind::FieldAccess { expr: inner, field } => {
            let inner_val = eval_expr(inner, values, local_values, ctx)?;
            match inner_val {
                RuntimeValue::Struct { fields, .. } => {
                    fields.get(field.value.as_str()).cloned().ok_or_else(|| {
                        ctx.eval_error(format!("no field `{}` on struct", field.value), field.span)
                    })
                }
                _ => Err(ctx.eval_error("field access on non-struct value", inner.span)),
            }
        }

        // --- Struct construction ---
        ExprKind::StructConstruction {
            type_name, fields, ..
        } => {
            let mut field_map = IndexMap::new();
            for field_init in fields {
                let val = if let Some(value_expr) = &field_init.value {
                    eval_expr(value_expr, values, local_values, ctx)?
                } else {
                    // Shorthand: look up name in local scope, then graph scope.
                    // Shorthand field names are always bare locals.
                    let bare = field_init.name.value.as_str();
                    local_values
                        .get(bare)
                        .or_else(|| values.get(&ScopedName::local(bare)))
                        .cloned()
                        .ok_or_else(|| {
                            ctx.eval_error(
                                format!(
                                    "undefined variable `{}` for shorthand field",
                                    field_init.name.value
                                ),
                                field_init.name.span,
                            )
                        })?
                };
                // Validate against any field-level domain constraint declared
                // on `<type_name>.<field_name>`. The check fires at the field's
                // span so the diagnostic points at the offending value.
                if let Some(field_constraints) = ctx.struct_field_constraints
                    && let Some(constraint) = field_constraints
                        .get(&(type_name.value.clone(), field_init.name.value.clone()))
                    && let Err(violation) =
                        crate::domain_check::check_domain_constraint(&val, constraint)
                {
                    let span = field_init
                        .value
                        .as_ref()
                        .map_or(field_init.name.span, |e| e.span);
                    return Err(ctx.eval_error(
                        format!(
                            "field `{}.{}` {}",
                            type_name.value, field_init.name.value, violation.message
                        ),
                        span,
                    ));
                }
                field_map.insert(field_init.name.value.clone(), val);
            }
            Ok(RuntimeValue::Struct {
                type_name: type_name.value.clone(),
                fields: field_map,
            })
        }

        // --- Unfold ---
        ExprKind::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } => collections::eval_unfold(expr, init, prev_name, curr_name, body, values, ctx),

        // TupleMatch is desugared to If/BinOp(Eq) chains before evaluation.
        #[expect(clippy::unreachable, reason = "invariant: desugared before eval")]
        ExprKind::TupleMatch { .. } => unreachable!("TupleMatch should be desugared before eval"),

        ExprKind::InlineDagRef { path, args, output } => {
            eval_inline_dag_call(expr, path, args, output, values, local_values, ctx)
        }

        // `Sugar` and `UnresolvedRef` payloads are `Infallible` in `Resolved`
        // — both arms are statically unreachable.
        #[expect(
            clippy::uninhabited_references,
            reason = "Sugar/UnresolvedRef(Infallible) — proof of unreachability"
        )]
        ExprKind::Sugar(s) | ExprKind::UnresolvedRef(s) => match *s {},
    }
}

/// Evaluate an inline DAG invocation `@<path>(args).<out>`.
///
/// Semantics (from issue #451):
/// - Each call site is a fresh DAG instantiation; every evaluation produces a
///   fresh sub-graph bound to this call's argument values.
/// - Arguments are evaluated in the *caller's* scope, so expressions may
///   reference loop variables from an enclosing `for`, `scan`, `unfold`, or
///   match-binding — a deliberate divergence from top-level `include`.
/// - The dag body executes in its compiled topological order (not textual
///   order), so forward references across body nodes resolve correctly.
/// - The projected output's value is returned.
///
/// The dag body is evaluated against the pre-compiled [`TIR`] stored on the
/// enclosing file's TIR and carried in [`EvalContext::compiled_dags`]. The
/// dag's own registry is used for all nested lookups so sibling dag calls
/// from inside the body resolve through the same pipeline.
///
/// `ctx.compiled_dags` is keyed by [`DagKey`](graphcal_compiler::tir::typed::DagKey):
/// a single-segment key for same-file calls (`@dag(args).out`) and a two-
/// segment `(module-alias, dag-name)` key for cross-file qualified calls
/// (`@module.dag(args).out`) brought into scope via `import path as module;`
/// or `import path;`.
fn eval_inline_dag_call(
    _call_expr: &Expr,
    path: &graphcal_compiler::syntax::ast::ModulePath,
    args: &[graphcal_compiler::desugar::resolved_ast::ParamBinding],
    output: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::DeclName>,
    caller_values: &HashMap<ScopedName, RuntimeValue>,
    caller_locals: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let display_path = path.display_path();
    let dag_tir = ctx.tir.lookup_call_target(path).ok_or_else(|| {
        ctx.internal_error(
            format!(
                "dag `{display_path}` has no compiled TIR (should have been caught by dim-check)"
            ),
            path.span,
        )
    })?;

    // Evaluate argument expressions in the caller's scope so loop variables
    // and other enclosing bindings resolve correctly. Param-binding names are
    // always bare locals.
    let mut dag_values: HashMap<ScopedName, RuntimeValue> = HashMap::new();
    for binding in args {
        let value = eval_expr(&binding.value, caller_values, caller_locals, ctx)?;
        dag_values.insert(ScopedName::local(binding.name.name.clone()), value);
    }

    // Resolve explicit DAG-body imports. Cross-file qualified calls receive
    // concrete imported values when the dependency dag TIR is cloned into the
    // caller; same-file calls resolve through the source binding map against
    // the caller's current values.
    let own_names: std::collections::HashSet<&str> = dag_tir
        .consts
        .iter()
        .map(|e| e.name.member())
        .chain(dag_tir.params.iter().map(|e| e.name.member()))
        .chain(dag_tir.nodes.iter().map(|e| e.name.member()))
        .collect();
    let outer_scope_keys: std::collections::HashSet<&ScopedName> = dag_tir
        .imported_values
        .keys()
        .chain(dag_tir.imported_value_sources.keys())
        .collect();
    for scoped in outer_scope_keys {
        let member = scoped.member();
        let local_key = ScopedName::local(member);
        if own_names.contains(member) || dag_values.contains_key(&local_key) {
            continue;
        }
        let value = dag_tir
            .imported_values
            .get(scoped)
            .map(|(v, _)| v)
            .or_else(|| {
                dag_tir
                    .imported_value_sources
                    .get(scoped)
                    .and_then(|source| {
                        caller_values.get(&ScopedName::local(source.source_name.as_str()))
                    })
            });
        if let Some(value) = value {
            dag_values.insert(local_key, value.clone());
        }
    }

    // Inside the dag body the visible registry is the file's shared one
    // (every DAG in the file resolves through the same registry); `tir`
    // continues to point at the file-level map so nested inline calls
    // resolve.
    let dag_ctx = EvalContext {
        builtin_consts: ctx.builtin_consts,
        builtin_fns: ctx.builtin_fns,
        registry: ctx.registry,
        src: ctx.src,
        unfold_context: None,
        tir: ctx.tir,
        struct_field_constraints: ctx.struct_field_constraints,
    };

    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();

    // Evaluate consts and nodes in topological order derived from
    // `runtime_deps` ∪ `const_deps`. Params are leaves — they arrive via
    // `args` and never execute body code.
    let topo = topo_order_for_dag_body(dag_tir);
    for name in topo {
        let local_key = ScopedName::local(name.member());
        if dag_values.contains_key(&local_key) {
            continue;
        }
        let expr = lookup_dag_body_expr(dag_tir, &name);
        let Some(expr) = expr else {
            continue;
        };
        let value = eval_expr(expr, &dag_values, &empty_locals, &dag_ctx)?;
        dag_values.insert(local_key, value);
    }

    dag_values
        .get(&ScopedName::local(output.value.as_str()))
        .cloned()
        .ok_or_else(|| {
            ctx.internal_error(
                format!(
                    "dag `{display_path}` has no node `{}` after evaluation (should have been caught by dim-check)",
                    output.value,
                ),
                output.span,
            )
        })
}

/// Kahn-style topological sort over a dag body's combined dep graph.
///
/// Produces an order in which each const/param/node appears after every one
/// of its runtime and const dependencies. Cycles are impossible in a
/// well-typed dag body because compile-time dep collection rejects them.
fn topo_order_for_dag_body(dag_tir: &graphcal_compiler::tir::typed::DagTIR) -> Vec<ScopedName> {
    use std::collections::BTreeSet;

    // All declaration names in a stable order.
    let mut names: Vec<ScopedName> = dag_tir
        .source_order
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();

    // Incoming-edge counts and reverse adjacency keyed by name.
    let mut incoming: HashMap<ScopedName, usize> = HashMap::new();
    let mut outgoing: HashMap<ScopedName, Vec<ScopedName>> = HashMap::new();
    for name in &names {
        incoming.insert(name.clone(), 0);
        outgoing.insert(name.clone(), Vec::new());
    }

    let add_edge = |from: &ScopedName,
                    to: &ScopedName,
                    incoming: &mut HashMap<ScopedName, usize>,
                    outgoing: &mut HashMap<ScopedName, Vec<ScopedName>>| {
        if let (Some(out), Some(deg)) = (outgoing.get_mut(from), incoming.get_mut(to)) {
            out.push(to.clone());
            *deg += 1;
        }
    };

    for (name, deps) in &dag_tir.runtime_deps {
        for dep in deps {
            add_edge(dep, name, &mut incoming, &mut outgoing);
        }
    }
    for (name, deps) in &dag_tir.const_deps {
        for dep in deps {
            add_edge(dep, name, &mut incoming, &mut outgoing);
        }
    }

    // Kahn with a sorted ready set for deterministic tie-breaking.
    let mut ready: BTreeSet<ScopedName> = incoming
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(n, _)| n.clone())
        .collect();
    let mut order = Vec::with_capacity(names.len());
    while let Some(n) = ready.iter().next().cloned() {
        ready.remove(&n);
        order.push(n.clone());
        if let Some(succs) = outgoing.remove(&n) {
            for s in succs {
                if let Some(deg) = incoming.get_mut(&s) {
                    *deg -= 1;
                    if *deg == 0 {
                        ready.insert(s);
                    }
                }
            }
        }
    }
    order
}

/// Look up the evaluation expression for a declaration in a dag body.
///
/// Returns `None` for params (they receive their value from the call-site
/// argument binding, not from a body-local expression).
fn lookup_dag_body_expr<'a>(
    dag_tir: &'a graphcal_compiler::tir::typed::DagTIR,
    name: &ScopedName,
) -> Option<&'a Expr> {
    if let Some(c) = dag_tir.consts.iter().find(|c| &c.name == name) {
        return Some(&c.expr);
    }
    if let Some(n) = dag_tir.nodes.iter().find(|n| &n.name == name) {
        return Some(&n.expr);
    }
    None
}

/// Resolve a `UnitExpr` to its compound scale factor at runtime.
///
/// For static units, this is equivalent to `registry.units.resolve_unit_expr()`.
/// For dynamic units, the scale expression is evaluated using the current `values`
/// and `local_values` maps, then multiplied by the base unit's static scale.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a unit is unknown or a dynamic scale expression
/// fails to evaluate to a scalar.
pub fn resolve_unit_scale(
    unit: &UnitExpr,
    values: &HashMap<ScopedName, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<f64, GraphcalError> {
    let mut compound_scale = 1.0;
    for item in &unit.terms {
        let info = ctx
            .registry
            .units
            .get_unit(item.name.value.as_str())
            .ok_or_else(|| {
                ctx.eval_error(
                    format!("unknown unit `{}`", item.name.value),
                    item.name.span,
                )
            })?;
        let unit_scale = match &info.scale {
            UnitScale::Static(s) => *s,
            UnitScale::Dynamic {
                scale_expr,
                base_unit_scale,
            } => {
                let scale_val = eval_expr(scale_expr, values, local_values, ctx)?;
                let RuntimeValue::Scalar(scale_f64) = scale_val else {
                    return Err(ctx.eval_error(
                        "dynamic unit scale expression must evaluate to a scalar",
                        scale_expr.span,
                    ));
                };
                scale_f64 * base_unit_scale
            }
        };
        let exp = item.power.unwrap_or(1);
        let powered_scale = unit_scale.powi(exp);
        match item.op {
            MulDivOp::Mul => compound_scale *= powered_scale,
            MulDivOp::Div => compound_scale /= powered_scale,
        }
    }
    Ok(compound_scale)
}
