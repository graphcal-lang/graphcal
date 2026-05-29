mod arithmetic;
mod collections;
mod control;
mod functions;

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_compiler::desugar::resolved_ast::{Expr, ExprKind, MulDivOp, UnitExpr};
use graphcal_compiler::syntax::names::{
    ConstructorName, FieldName, IndexName, NamePath, ResolvedIndexVariant, ResolvedName,
    ScopedName, namespace,
};

use graphcal_compiler::registry::builtins::BuiltinFunction;
use graphcal_compiler::registry::declared_type::{DeclaredType, IndexTypeRef, StructTypeRef};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::types::{Registry, TypeDef, UnitScale};
use graphcal_compiler::tir::typed::{
    DagTIR, ResolvedDagDependencies, ResolvedDomainConstraint, ResolvedInlineDagCall,
    StructFieldConstraintKey,
};

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
    /// Used by [`eval_inline_dag_call`] to reach the file's flat per-DAG body
    /// map. Module-aware TIRs route calls through [`current_dag`]'s resolved
    /// inline-DAG sidecar; legacy standalone TIRs fall back to source-path
    /// lookup on this TIR.
    pub tir: &'a graphcal_compiler::tir::typed::TIR,
    /// DAG whose expression is currently being evaluated. When present, inline
    /// DAG calls can use HIR-derived canonical `DagId` / `ResolvedName<Decl>`
    /// identities instead of resolving source paths again at eval time.
    pub current_dag: Option<&'a graphcal_compiler::tir::typed::DagTIR>,
    /// Root-file values visible to nested inline DAG calls. This lets DAG-body
    /// self-imports route by their canonical source `DagId` rather than by a
    /// same-leaf name in the immediate caller's local value map.
    pub root_values: Option<&'a HashMap<ScopedName, RuntimeValue>>,
    /// Resolved domain constraints declared on struct/union member fields,
    /// keyed by owner-qualified struct/constructor/field identity. Looked up at
    /// every `ExprKind::ConstructorCall` to validate field values immediately.
    /// `None` means "skip the check" (used by paths that haven't resolved
    /// constraints yet, e.g. const-bound evaluation inside `exec_plan`).
    pub struct_field_constraints:
        Option<&'a HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>>,
}

/// Context required to evaluate an `unfold(...)` expression inline.
///
/// Provides the self-referencing node name and declared types needed
/// to look up the range index for iterative evaluation.
pub struct UnfoldContext<'a> {
    pub self_name: &'a str,
    pub declared_types: &'a HashMap<ScopedName, DeclaredType>,
}

fn legacy_index_name_from_path(path: &NamePath) -> IndexName {
    IndexName::from(path.leaf().clone())
}

fn resolved_value_index_variant<'a>(
    ctx: &'a EvalContext<'_>,
    span: graphcal_compiler::syntax::span::Span,
) -> Option<&'a ResolvedIndexVariant> {
    ctx.current_dag
        .and_then(|dag| dag.resolved_collection_refs.as_ref())
        .and_then(|refs| refs.variant_literals.get(&span))
}

fn runtime_label_from_resolved_variant(resolved: &ResolvedIndexVariant) -> RuntimeValue {
    RuntimeValue::resolved_label(resolved)
}

pub(super) fn index_ref_matches_resolved_or_legacy(
    actual: &IndexTypeRef,
    expected: &ResolvedName<namespace::Index>,
) -> bool {
    match actual.resolved() {
        Some(actual) => actual == expected,
        None => actual.name().as_str() == expected.as_str(),
    }
}

fn constructor_call_target<'a>(
    ctx: &'a EvalContext<'_>,
    span: graphcal_compiler::syntax::span::Span,
) -> Option<&'a graphcal_compiler::tir::typed::ResolvedConstructorTarget> {
    ctx.current_dag
        .and_then(|dag| dag.resolved_constructor_refs.as_ref())
        .and_then(|refs| refs.constructor_calls.get(&span))
}

fn runtime_struct_type_def<'a>(
    type_name: &StructTypeRef,
    ctx: &'a EvalContext<'_>,
) -> Option<&'a TypeDef> {
    match type_name.resolved() {
        Some(resolved) => ctx
            .tir
            .dags
            .values()
            .find_map(|dag| dag.resolved_type_defs.as_ref()?.struct_types.get(resolved)),
        None => ctx.registry.types.get_type(type_name.as_str()),
    }
}

fn constructor_fields_for_runtime_struct<'a>(
    type_def: &'a TypeDef,
    type_name: &StructTypeRef,
) -> Option<&'a [graphcal_compiler::registry::types::StructField]> {
    type_def.union_members()?.iter().find_map(|member| {
        (member.name.as_str() == type_name.as_str()).then_some(member.fields.as_slice())
    })
}

fn find_struct_field_constraint<'a>(
    constraints: &'a HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    owning_type: Option<&StructTypeRef>,
    constructor: &ConstructorName,
    field: &FieldName,
) -> Option<&'a ResolvedDomainConstraint> {
    owning_type
        .and_then(|owning_type| {
            constraints.get(&StructFieldConstraintKey::new(
                owning_type.clone(),
                constructor.clone(),
                field.clone(),
            ))
        })
        .or_else(|| {
            constraints
                .iter()
                .find(|(key, _)| {
                    key.constructor == *constructor
                        && key.field == *field
                        && owning_type
                            .is_none_or(|owning_type| key.owning_type.matches_ref(owning_type))
                })
                .map(|(_, constraint)| constraint)
        })
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
        ExprKind::TypeSystemRef(name) => Err(ctx.eval_error(
            format!(
                "unexpected type-system name `{}` in evaluation context",
                name.value
            ),
            name.span,
        )),
        ExprKind::UnitLiteral { value, unit } => {
            let scale = resolve_unit_scale(unit, values, local_values, ctx)?;
            Ok(RuntimeValue::Scalar(*value * scale))
        }
        ExprKind::Bool(b) => Ok(RuntimeValue::Bool(*b)),
        ExprKind::VariantLiteral { index, variant } => {
            let full_span = index.span.merge(variant.span);
            Ok(resolved_value_index_variant(ctx, full_span)
                .map(runtime_label_from_resolved_variant)
                .unwrap_or_else(|| {
                    RuntimeValue::legacy_label(
                        legacy_index_name_from_path(&index.value),
                        variant.value.clone(),
                    )
                }))
        }
        ExprKind::GraphRef(ident) => values.get(&ident.value).cloned().ok_or_else(|| {
            ctx.eval_error(
                format!("undefined graph reference `@{}`", ident.value),
                expr.span,
            )
        }),
        ExprKind::ConstRef(ident) => {
            if let Some(resolved_variant) = resolved_value_index_variant(ctx, ident.span) {
                return Ok(runtime_label_from_resolved_variant(resolved_variant));
            }

            values
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
                })
        }
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
        ExprKind::FnCall { callee, args, .. } => {
            functions::eval_fn_call(expr, callee, args, values, local_values, ctx)
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
                RuntimeValue::Struct { type_name, fields } => {
                    if let Some(type_def) = runtime_struct_type_def(&type_name, ctx) {
                        let constructor_fields =
                            constructor_fields_for_runtime_struct(type_def, &type_name)
                                .ok_or_else(|| {
                                    ctx.eval_error(
                                        format!(
                                            "constructor `{}` is not a member of struct `{}`",
                                            type_name.name(),
                                            type_def.name
                                        ),
                                        inner.span,
                                    )
                                })?;
                        if !constructor_fields
                            .iter()
                            .any(|field_def| field_def.name == field.value)
                        {
                            return Err(ctx.eval_error(
                                format!("no field `{}` on struct `{type_name}`", field.value),
                                field.span,
                            ));
                        }
                    }
                    fields.get(field.value.as_str()).cloned().ok_or_else(|| {
                        ctx.eval_error(format!("no field `{}` on struct", field.value), field.span)
                    })
                }
                _ => Err(ctx.eval_error("field access on non-struct value", inner.span)),
            }
        }

        // --- Constructor call ---
        ExprKind::ConstructorCall { callee, fields, .. } => {
            let resolved_constructor = constructor_call_target(ctx, callee.span());
            let (constructor_name, owning_type) = match resolved_constructor {
                Some(target) => (
                    target.variant.name.clone(),
                    Some(StructTypeRef::from_resolved(target.owning_type.clone())),
                ),
                None => {
                    let Some(constructor) = callee.as_bare() else {
                        return Err(ctx.eval_error(
                            format!("unknown constructor `{}`", callee.display_path()),
                            callee.span(),
                        ));
                    };
                    (ConstructorName::from_atom(constructor.name.clone()), None)
                }
            };
            let mut field_map = IndexMap::new();
            for field_init in fields {
                let val = eval_expr(&field_init.value, values, local_values, ctx)?;
                // Validate against any field-level domain constraint declared
                // on `<constructor>.<field_name>`. Prefer the HIR-resolved owning
                // struct identity when available; runtime values still store the
                // constructor leaf for display/variant identity.
                if let Some(field_constraints) = ctx.struct_field_constraints
                    && let Some(constraint) = find_struct_field_constraint(
                        field_constraints,
                        owning_type.as_ref(),
                        &constructor_name,
                        &field_init.name.value,
                    )
                    && let Err(violation) =
                        crate::domain_check::check_domain_constraint(&val, constraint)
                {
                    let span = field_init.value.span;
                    return Err(ctx.eval_error(
                        format!(
                            "field `{}.{}` {}",
                            constructor_name, field_init.name.value, violation.message
                        ),
                        span,
                    ));
                }
                field_map.insert(field_init.name.value.clone(), val);
            }
            let type_name = resolved_constructor.map_or_else(
                || {
                    StructTypeRef::legacy(
                        graphcal_compiler::syntax::names::StructTypeName::from_atom(
                            constructor_name.atom().clone(),
                        ),
                    )
                },
                |target| {
                    StructTypeRef::new(
                        graphcal_compiler::syntax::names::StructTypeName::from_atom(
                            target.variant.name.atom().clone(),
                        ),
                        Some(target.owning_type.clone()),
                    )
                },
            );
            Ok(RuntimeValue::Struct {
                type_name,
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

fn resolved_inline_call<'a>(
    expr: &Expr,
    ctx: &'a EvalContext<'_>,
) -> Option<&'a ResolvedInlineDagCall> {
    ctx.current_dag
        .and_then(|dag| dag.resolved_inline_dag_refs.as_ref())
        .and_then(|refs| refs.calls.get(&expr.span))
}

fn resolved_decl_local_key(name: &ResolvedName<namespace::Decl>) -> ScopedName {
    ScopedName::local(name.as_str())
}

fn imported_value_source_value<'a>(
    source: &graphcal_compiler::ir::lower::ImportedValueSource,
    caller_values: &'a HashMap<ScopedName, RuntimeValue>,
    ctx: &'a EvalContext<'_>,
) -> Option<&'a RuntimeValue> {
    let source_key = ScopedName::local(source.source_name.as_str());
    match ctx.current_dag {
        Some(caller_dag) if source.dag_id.eq(&caller_dag.dag_id) => caller_values.get(&source_key),
        _ if source.dag_id.eq(&ctx.tir.root_dag_id) => {
            ctx.root_values.and_then(|values| values.get(&source_key))
        }
        _ => None,
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
/// The dag body is evaluated against the pre-compiled [`TIR`] carried in
/// [`EvalContext::tir`]. Module-aware callers use the current DAG's
/// HIR-derived sidecar to route directly to a canonical
/// [`DagId`](graphcal_compiler::dag_id::DagId); legacy callers fall back to
/// source-path lookup through the enclosing TIR. The dag's own registry is used
/// for all nested lookups so sibling dag calls from inside the body resolve
/// through the same pipeline.
fn eval_inline_dag_call(
    _call_expr: &Expr,
    path: &graphcal_compiler::syntax::ast::ModulePath,
    args: &[graphcal_compiler::desugar::resolved_ast::ParamBinding],
    output: &graphcal_compiler::syntax::span::Spanned<graphcal_compiler::syntax::names::DeclName>,
    caller_values: &HashMap<ScopedName, RuntimeValue>,
    caller_locals: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let display_path = path.display_path();
    let resolved_call = resolved_inline_call(_call_expr, ctx);
    let dag_tir = match resolved_call {
        Some(call) => ctx.tir.dags.get(&call.target).ok_or_else(|| {
            ctx.internal_error(
                format!(
                    "dag `{}` has no compiled TIR (should have been caught by dim-check)",
                    call.target
                ),
                path.span,
            )
        })?,
        None => ctx.tir.lookup_call_target(path).ok_or_else(|| {
            ctx.internal_error(
                format!(
                    "dag `{display_path}` has no compiled TIR (should have been caught by dim-check)"
                ),
                path.span,
            )
        })?,
    };

    // Evaluate argument expressions in the caller's scope so loop variables
    // and other enclosing bindings resolve correctly. Param-binding names are
    // always bare locals.
    let mut dag_values: HashMap<ScopedName, RuntimeValue> = HashMap::new();
    for binding in args {
        let value = eval_expr(&binding.value, caller_values, caller_locals, ctx)?;
        let target_name = resolved_call
            .and_then(|call| call.arg_targets.get(&binding.name.span))
            .map_or_else(
                || ScopedName::local(binding.name.name.as_str()),
                resolved_decl_local_key,
            );
        dag_values.insert(target_name, value);
    }

    // Resolve explicit DAG-body imports. Cross-file qualified calls receive
    // concrete imported values when the dependency dag TIR is cloned into the
    // caller; same-file calls route source bindings by their canonical source
    // DAG so nested inline calls do not accidentally capture same-leaf values
    // from the immediate caller.
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
                    .and_then(|source| imported_value_source_value(source, caller_values, ctx))
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
        current_dag: Some(dag_tir),
        root_values: ctx.root_values,
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

    let output_key = resolved_call.map_or_else(
        || ScopedName::local(output.value.as_str()),
        |call| resolved_decl_local_key(&call.output.value),
    );
    dag_values.get(&output_key).cloned().ok_or_else(|| {
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
fn topo_order_for_dag_body(dag_tir: &DagTIR) -> Vec<ScopedName> {
    match &dag_tir.resolved_deps {
        Some(deps) => topo_order_for_dag_body_resolved(dag_tir, deps),
        None => topo_order_for_dag_body_legacy(dag_tir),
    }
}

type ResolvedDeclKey = ResolvedName<namespace::Decl>;

fn topo_order_for_dag_body_resolved(
    dag_tir: &DagTIR,
    deps: &ResolvedDagDependencies,
) -> Vec<ScopedName> {
    use std::collections::BTreeSet;

    let mut names: Vec<ScopedName> = dag_tir
        .source_order
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();

    let mut key_by_name: HashMap<ScopedName, ResolvedDeclKey> = HashMap::new();
    let mut name_by_key: HashMap<ResolvedDeclKey, ScopedName> = HashMap::new();
    for name in &names {
        let Some(key) = dag_tir.resolved_decl_key_for_local(name) else {
            return topo_order_for_dag_body_legacy(dag_tir);
        };
        key_by_name.insert(name.clone(), key.clone());
        name_by_key.insert(key, name.clone());
    }

    let mut incoming: HashMap<ResolvedDeclKey, usize> = HashMap::new();
    let mut outgoing: HashMap<ResolvedDeclKey, Vec<ResolvedDeclKey>> = HashMap::new();
    for key in key_by_name.values() {
        incoming.insert(key.clone(), 0);
        outgoing.insert(key.clone(), Vec::new());
    }

    let add_edge =
        |from: &ResolvedDeclKey,
         to: &ResolvedDeclKey,
         incoming: &mut HashMap<ResolvedDeclKey, usize>,
         outgoing: &mut HashMap<ResolvedDeclKey, Vec<ResolvedDeclKey>>| {
            if let (Some(out), Some(deg)) = (outgoing.get_mut(from), incoming.get_mut(to)) {
                out.push(to.clone());
                *deg += 1;
            }
        };

    for (name, dep_set) in deps.runtime_deps.iter().chain(deps.const_deps.iter()) {
        for dep in dep_set {
            add_edge(dep, name, &mut incoming, &mut outgoing);
        }
    }

    let mut ready: BTreeSet<ResolvedDeclKey> = incoming
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(name, _)| name.clone())
        .collect();
    let mut order = Vec::with_capacity(names.len());
    while let Some(key) = ready.iter().next().cloned() {
        ready.remove(&key);
        if let Some(name) = name_by_key.get(&key) {
            order.push(name.clone());
        }
        if let Some(succs) = outgoing.remove(&key) {
            for succ in succs {
                if let Some(deg) = incoming.get_mut(&succ) {
                    *deg -= 1;
                    if *deg == 0 {
                        ready.insert(succ);
                    }
                }
            }
        }
    }
    order
}

fn topo_order_for_dag_body_legacy(dag_tir: &DagTIR) -> Vec<ScopedName> {
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
