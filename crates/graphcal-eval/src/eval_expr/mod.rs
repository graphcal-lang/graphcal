mod arithmetic;
mod collections;
mod control;
mod conversions;
mod functions;
mod hir_eval;

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

use crate::decl_key::RuntimeDeclKey;

pub use graphcal_compiler::registry::runtime_value::RuntimeValue;
pub use hir_eval::{HirLocalValueMap, eval_hir_expr};
pub type RuntimeValueMap = HashMap<RuntimeDeclKey, RuntimeValue>;

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
    /// map after semantic call metadata has selected a canonical DAG id.
    pub tir: &'a graphcal_compiler::tir::typed::TIR,
    /// DAG whose expression is currently being evaluated. When present, inline
    /// DAG calls can use HIR-derived canonical `DagId` / `ResolvedName<Decl>`
    /// identities instead of resolving source paths again at eval time.
    pub current_dag: Option<&'a graphcal_compiler::tir::typed::DagTIR>,
    /// Root-file values visible to nested inline DAG calls. This lets DAG-body
    /// self-imports route by their canonical source `DagId` rather than by a
    /// same-leaf name in the immediate caller's local value map.
    pub root_values: Option<&'a RuntimeValueMap>,
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

/// Collapse a syntactic index path to a leaf-only name for eval diagnostics.
///
/// Module-aware variant literals must use HIR/resolved collection refs.
fn standalone_index_name_from_path(path: &NamePath) -> IndexName {
    IndexName::from(path.leaf().clone())
}

fn eval_owner(ctx: &EvalContext<'_>) -> graphcal_compiler::dag_id::DagId {
    ctx.current_dag
        .map_or_else(|| ctx.tir.root_dag_id.clone(), |dag| dag.dag_id.clone())
}

fn index_ref_with_eval_owner(ctx: &EvalContext<'_>, name: IndexName) -> IndexTypeRef {
    if name.as_str().starts_with("__nat_range_") {
        IndexTypeRef::from_resolved(
            graphcal_compiler::registry::types::nat_range_resolved_index_name(name),
        )
    } else {
        IndexTypeRef::with_owner(eval_owner(ctx), name)
    }
}

fn index_ref_from_path(ctx: &EvalContext<'_>, path: &NamePath) -> IndexTypeRef {
    index_ref_with_eval_owner(ctx, standalone_index_name_from_path(path))
}

fn resolved_value_index_variant<'a>(
    ctx: &'a EvalContext<'_>,
    span: graphcal_compiler::syntax::span::Span,
) -> Option<&'a ResolvedIndexVariant> {
    ctx.current_dag
        .map(|dag| &dag.semantic.collection_refs)
        .and_then(|refs| refs.variant_literals.get(&span))
}

fn runtime_label_from_resolved_variant(resolved: &ResolvedIndexVariant) -> RuntimeValue {
    RuntimeValue::resolved_label(resolved)
}

fn resolved_graph_ref_target<'a>(
    ctx: &'a EvalContext<'_>,
    span: graphcal_compiler::syntax::span::Span,
) -> Option<&'a ResolvedName<namespace::Decl>> {
    ctx.current_dag
        .map(|dag| &dag.semantic.dependencies)
        .and_then(|deps| deps.graph_ref_targets.get(&span))
}

fn resolved_const_ref_target<'a>(
    ctx: &'a EvalContext<'_>,
    span: graphcal_compiler::syntax::span::Span,
) -> Option<&'a ResolvedName<namespace::Decl>> {
    ctx.current_dag
        .map(|dag| &dag.semantic.dependencies)
        .and_then(|deps| deps.const_ref_targets.get(&span))
}

fn visible_decl_runtime_key(ctx: &EvalContext<'_>, scoped: &ScopedName) -> Option<RuntimeDeclKey> {
    let dag = ctx.current_dag?;
    dag.semantic
        .decl_bindings
        .get(scoped)
        .cloned()
        .map(RuntimeDeclKey::resolved)
        .or_else(|| {
            dag.resolved_decl_key_for_local(scoped)
                .map(RuntimeDeclKey::resolved)
        })
}

fn graph_ref_runtime_key(
    ctx: &EvalContext<'_>,
    span: graphcal_compiler::syntax::span::Span,
    scoped: &ScopedName,
) -> Result<RuntimeDeclKey, GraphcalError> {
    resolved_graph_ref_target(ctx, span)
        .and_then(|target| imported_source_runtime_key(ctx, scoped, target))
        .or_else(|| scoped_visible_runtime_key(scoped, ctx))
        .or_else(|| {
            resolved_graph_ref_target(ctx, span)
                .cloned()
                .map(RuntimeDeclKey::resolved)
        })
        .or_else(|| visible_decl_runtime_key(ctx, scoped))
        .ok_or_else(|| {
            ctx.internal_error(
                format!("semantic graph-reference metadata missing target for `@{scoped}`"),
                span,
            )
        })
}

fn const_ref_runtime_key(
    ctx: &EvalContext<'_>,
    span: graphcal_compiler::syntax::span::Span,
    scoped: &ScopedName,
) -> Option<RuntimeDeclKey> {
    resolved_const_ref_target(ctx, span)
        .and_then(|target| imported_source_runtime_key(ctx, scoped, target))
        .or_else(|| scoped_visible_runtime_key(scoped, ctx))
        .or_else(|| {
            resolved_const_ref_target(ctx, span)
                .cloned()
                .map(RuntimeDeclKey::resolved)
        })
        .or_else(|| visible_decl_runtime_key(ctx, scoped))
}

fn imported_source_runtime_key(
    ctx: &EvalContext<'_>,
    scoped: &ScopedName,
    target: &ResolvedName<namespace::Decl>,
) -> Option<RuntimeDeclKey> {
    let dag = ctx.current_dag?;
    let source = dag.imported_value_sources.get(scoped)?;
    (source.dag_id == *target.owner() && source.source_name.as_str() == target.as_str())
        .then(|| visible_decl_runtime_key(ctx, scoped))
        .flatten()
}

fn scoped_visible_runtime_key(
    scoped: &ScopedName,
    ctx: &EvalContext<'_>,
) -> Option<RuntimeDeclKey> {
    let dag = ctx.current_dag?;
    scoped
        .is_qualified()
        .then(|| {
            dag.resolved_decl_key_for_local(scoped)
                .map(RuntimeDeclKey::resolved)
        })
        .flatten()
}

pub fn index_ref_matches_resolved_or_leaf(
    actual: &IndexTypeRef,
    expected: &ResolvedName<namespace::Index>,
) -> bool {
    actual.resolved() == expected
}

fn constructor_call_target<'a>(
    ctx: &'a EvalContext<'_>,
    span: graphcal_compiler::syntax::span::Span,
) -> Option<&'a graphcal_compiler::tir::typed::ResolvedConstructorTarget> {
    ctx.current_dag
        .map(|dag| &dag.semantic.constructor_refs)
        .and_then(|refs| refs.constructor_calls.get(&span))
}

fn runtime_struct_type_def<'a>(
    type_name: &StructTypeRef,
    ctx: &'a EvalContext<'_>,
) -> Option<&'a TypeDef> {
    ctx.tir
        .dags
        .values()
        .find_map(|dag| {
            dag.semantic
                .type_defs
                .struct_types
                .get(type_name.resolved())
        })
        .or_else(|| ctx.registry.types.get_type(type_name.as_str()))
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
    owning_type.and_then(|owning_type| {
        constraints.get(&StructFieldConstraintKey::new(
            owning_type.clone(),
            constructor.clone(),
            field.clone(),
        ))
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

fn checked_finite_scalar(
    value: f64,
    context: &str,
    span: graphcal_compiler::syntax::span::Span,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    if value.is_finite() {
        Ok(RuntimeValue::Scalar(value))
    } else {
        Err(ctx.eval_error(format!("{context} must be finite, got {value}"), span))
    }
}

fn checked_positive_finite_unit_scale(
    value: f64,
    context: &str,
    span: graphcal_compiler::syntax::span::Span,
    ctx: &EvalContext<'_>,
) -> Result<f64, GraphcalError> {
    if !value.is_finite() {
        Err(ctx.eval_error(format!("{context} must be finite, got {value}"), span))
    } else if value <= 0.0 {
        Err(ctx.eval_error(
            format!("{context} must be greater than zero, got {value}"),
            span,
        ))
    } else {
        Ok(value)
    }
}

fn checked_unit_scaled_value(
    value: f64,
    scale: f64,
    span: graphcal_compiler::syntax::span::Span,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    checked_finite_scalar(value * scale, "unit literal value", span, ctx)
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
    values: &RuntimeValueMap,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match &expr.kind {
        ExprKind::Number(n) => checked_finite_scalar(*n, "numeric literal", expr.span, ctx),
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
            checked_unit_scaled_value(*value, scale, expr.span, ctx)
        }
        ExprKind::Bool(b) => Ok(RuntimeValue::Bool(*b)),
        ExprKind::VariantLiteral { index, variant } => {
            let full_span = index.span.merge(variant.span);
            Ok(resolved_value_index_variant(ctx, full_span).map_or_else(
                || {
                    RuntimeValue::label_with_owner(
                        eval_owner(ctx),
                        standalone_index_name_from_path(&index.value),
                        variant.value.clone(),
                    )
                },
                runtime_label_from_resolved_variant,
            ))
        }
        ExprKind::GraphRef(ident) => {
            let key = graph_ref_runtime_key(ctx, ident.span, &ident.value)?;
            values.get(&key).cloned().ok_or_else(|| {
                ctx.eval_error(
                    format!("undefined graph reference `@{}`", ident.value),
                    expr.span,
                )
            })
        }
        ExprKind::ConstRef(ident) => {
            if let Some(resolved_variant) = resolved_value_index_variant(ctx, ident.span) {
                return Ok(runtime_label_from_resolved_variant(resolved_variant));
            }

            if let Some(key) = const_ref_runtime_key(ctx, ident.span, &ident.value)
                && let Some(value) = values.get(&key)
            {
                return Ok(value.clone());
            }

            if let Some(value) = ctx.builtin_consts.get(ident.value.member()) {
                return checked_finite_scalar(*value, "built-in constant", expr.span, ctx);
            }

            // Nat generic params are stored as local values (e.g., `N` from `N: Nat`)
            // and may be referenced in expression position as ConstRef (uppercase).
            // Locals are always bare names (no module qualification).
            if !ident.value.is_qualified()
                && let Some(value) = local_values.get(ident.value.member())
            {
                return Ok(value.clone());
            }

            Err(ctx.eval_error(format!("undefined constant `{}`", ident.value), expr.span))
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
            let (constructor_name, owning_type) = if let Some(target) = resolved_constructor {
                (
                    target.variant.name.clone(),
                    Some(StructTypeRef::from_resolved(target.owning_type.clone())),
                )
            } else {
                let Some(constructor) = callee.as_bare() else {
                    return Err(ctx.eval_error(
                        format!("unknown constructor `{}`", callee.display_path()),
                        callee.span(),
                    ));
                };
                (ConstructorName::from_atom(constructor.name.clone()), None)
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
                    StructTypeRef::with_owner(
                        eval_owner(ctx),
                        graphcal_compiler::syntax::names::StructTypeName::from_atom(
                            constructor_name.atom().clone(),
                        ),
                    )
                },
                |target| {
                    StructTypeRef::with_display_leaf(
                        graphcal_compiler::syntax::names::StructTypeName::from_atom(
                            target.variant.name.atom().clone(),
                        ),
                        target.owning_type.clone(),
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
        .map(|dag| &dag.semantic.inline_dag_refs)
        .and_then(|refs| refs.calls.get(&expr.span))
}

fn dag_decl_runtime_key(dag_tir: &DagTIR, name: &ResolvedName<namespace::Decl>) -> RuntimeDeclKey {
    let _ = dag_tir;
    RuntimeDeclKey::resolved(name.clone())
}

fn imported_value_source_key(
    source: &graphcal_compiler::ir::lower::ImportedValueSource,
    source_dag: &DagTIR,
) -> RuntimeDeclKey {
    let _ = source_dag;
    RuntimeDeclKey::resolved(ResolvedName::from_def(
        source.dag_id.clone(),
        source.source_name.clone(),
    ))
}

fn imported_value_source_value<'a>(
    source: &graphcal_compiler::ir::lower::ImportedValueSource,
    caller_values: &'a RuntimeValueMap,
    ctx: &'a EvalContext<'_>,
) -> Option<&'a RuntimeValue> {
    match ctx.current_dag {
        Some(caller_dag) if source.dag_id.eq(&caller_dag.dag_id) => {
            caller_values.get(&imported_value_source_key(source, caller_dag))
        }
        _ if source.dag_id.eq(&ctx.tir.root_dag_id) => {
            let root_dag = ctx.tir.dags.get(&ctx.tir.root_dag_id)?;
            ctx.root_values
                .and_then(|values| values.get(&imported_value_source_key(source, root_dag)))
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
/// [`EvalContext::tir`]. Calls route through canonical semantic call metadata;
/// the dag's own registry is used for all nested lookups so sibling dag calls
/// from inside the body resolve through the same pipeline.
#[expect(
    clippy::too_many_lines,
    reason = "inline DAG evaluation validates bindings and executes selected body"
)]
fn eval_inline_dag_call(
    call_expr: &Expr,
    path: &graphcal_compiler::syntax::ast::ModulePath,
    args: &[graphcal_compiler::desugar::resolved_ast::ParamBinding],
    output: &graphcal_compiler::syntax::span::Spanned<graphcal_compiler::syntax::names::DeclName>,
    caller_values: &RuntimeValueMap,
    caller_locals: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let display_path = path.display_path();
    let resolved_call = resolved_inline_call(call_expr, ctx).ok_or_else(|| {
        ctx.internal_error(
            format!("semantic TIR missing inline-DAG call metadata for `{display_path}`"),
            call_expr.span,
        )
    })?;
    let dag_tir = ctx.tir.dags.get(&resolved_call.target).ok_or_else(|| {
        ctx.internal_error(
            format!(
                "dag `{}` has no compiled TIR (should have been caught by dim-check)",
                resolved_call.target
            ),
            path.span,
        )
    })?;

    // Evaluate argument expressions in the caller's scope so loop variables
    // and other enclosing bindings resolve correctly. Param-binding names are
    // always bare locals.
    let mut dag_values: RuntimeValueMap = HashMap::new();
    for binding in args {
        let value = eval_expr(&binding.value, caller_values, caller_locals, ctx)?;
        let target_key = resolved_call
            .arg_targets
            .get(&binding.name.span)
            .map(|target| dag_decl_runtime_key(dag_tir, target))
            .ok_or_else(|| {
                ctx.internal_error(
                    format!(
                        "semantic TIR missing inline-DAG arg target for `{}`",
                        binding.name.name
                    ),
                    binding.name.span,
                )
            })?;
        dag_values.insert(target_key, value);
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
        let visible_key = RuntimeDeclKey::for_visible_name(dag_tir, scoped);
        let unresolved_local_import =
            !dag_tir.semantic.decl_bindings.contains_key(scoped) && own_names.contains(member);
        if unresolved_local_import || dag_values.contains_key(&visible_key) {
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
            dag_values.insert(visible_key, value.clone());
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

    let empty_hir_locals = HirLocalValueMap::new();

    // Evaluate consts and nodes in topological order derived from
    // `runtime_deps` ∪ `const_deps`. Params are leaves — they arrive via
    // `args` and never execute body code.
    let topo = topo_order_for_dag_body(dag_tir);
    for name in topo {
        let local_key = RuntimeDeclKey::for_local_decl(dag_tir, &name);
        if dag_values.contains_key(&local_key) {
            continue;
        }
        let key = dag_tir.resolved_decl_key_for_local(&name).ok_or_else(|| {
            ctx.internal_error(format!("no semantic key for `{name}`"), path.span)
        })?;
        let hir_expr = dag_tir
            .semantic
            .expressions
            .consts
            .get(&key)
            .or_else(|| dag_tir.semantic.expressions.runtime_expr(&key))
            .ok_or_else(|| {
                ctx.internal_error(
                    format!(
                        "semantic TIR missing HIR expression for DAG body declaration `{name}`"
                    ),
                    path.span,
                )
            })?;
        let value = eval_hir_expr(hir_expr, &dag_values, &empty_hir_locals, &dag_ctx)?;
        dag_values.insert(local_key, value);
    }

    let output_key = dag_decl_runtime_key(dag_tir, &resolved_call.output.value);
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
    topo_order_for_dag_body_resolved(dag_tir, &dag_tir.semantic.dependencies)
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
            continue;
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
    values: &RuntimeValueMap,
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
            UnitScale::Static(s) => {
                checked_positive_finite_unit_scale(s.get(), "unit scale", item.name.span, ctx)?
            }
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
                let dynamic_scale = checked_positive_finite_unit_scale(
                    scale_f64,
                    "dynamic unit scale",
                    scale_expr.span,
                    ctx,
                )?;
                let base_scale = checked_positive_finite_unit_scale(
                    base_unit_scale.get(),
                    "base unit scale",
                    item.name.span,
                    ctx,
                )?;
                checked_positive_finite_unit_scale(
                    dynamic_scale * base_scale,
                    "dynamic unit scale",
                    scale_expr.span,
                    ctx,
                )?
            }
        };
        let exp = item.power.unwrap_or(1);
        let powered_scale = checked_positive_finite_unit_scale(
            unit_scale.powi(exp),
            "unit scale exponentiation",
            item.name.span,
            ctx,
        )?;
        compound_scale = match item.op {
            MulDivOp::Mul => compound_scale * powered_scale,
            MulDivOp::Div => compound_scale / powered_scale,
        };
        compound_scale = checked_positive_finite_unit_scale(
            compound_scale,
            "compound unit scale",
            unit.span,
            ctx,
        )?;
    }
    Ok(compound_scale)
}
