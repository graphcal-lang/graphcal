//! Type inference for expressions.
//!
//! Contains the main `infer_type` function that walks the AST and determines
//! the type (dimension, Bool, Int, struct, or indexed) of each expression.
//! Complex match arms are delegated to submodules.

mod collections;
mod control;
mod functions;
pub(super) mod hir;
mod scalar;

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::resolved_ast::{Expr, ExprKind};
use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;
use crate::syntax::dimension::Dimension;
use crate::syntax::names::{
    GenericParamName, IndexName, NamePath, ResolvedIndexVariant, ScopedName, UnitName,
};

use super::{DeclaredType, InferredIndex, InferredType};

fn legacy_index_name_from_path(path: &NamePath) -> IndexName {
    IndexName::from(path.leaf().clone())
}

fn resolved_value_index_variant(
    dag: Option<&crate::tir::typed::DagTIR>,
    span: crate::syntax::span::Span,
) -> Option<&ResolvedIndexVariant> {
    dag.and_then(|dag| dag.resolved_collection_refs.as_ref())
        .and_then(|refs| refs.variant_literals.get(&span))
}

fn infer_resolved_index_variant_label(resolved_variant: &ResolvedIndexVariant) -> InferredType {
    InferredType::Label(InferredIndex::from_resolved(
        resolved_variant.index().clone(),
    ))
}

/// Infer the type (dimension or struct) of an expression.
///
/// `owner_decl_name` is the name of the top-level declaration (node/const/param)
/// that contains this expression. It is threaded through to `infer_unfold` so
/// the unfold can look up the owning declaration's range index precisely.
/// Pass `None` when the owner is not known (e.g., in override dimension checks).
pub(super) fn infer_type(
    expr: &Expr,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    infer_type_with_owner(
        expr,
        None,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )
}

/// Infer the type of an expression, with the owning declaration name for
/// precise unfold range-index lookup.
pub(super) fn infer_type_with_owner(
    expr: &Expr,
    owner_decl_name: Option<&str>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    match &expr.kind {
        ExprKind::Number(_) => Ok(InferredType::Scalar(Dimension::dimensionless())),
        ExprKind::Integer(_) => Ok(InferredType::Int),
        ExprKind::Bool(_) => Ok(InferredType::Bool),
        ExprKind::StringLiteral(_) => Err(GraphcalError::DimensionMismatch {
            expected: "a numeric or boolean expression".to_string(),
            found: "string literal".to_string(),
            help: "string literals can only be used as arguments to datetime() or epoch()"
                .to_string(),
            src: src.clone(),
            span: expr.span.into(),
        }),
        ExprKind::TypeSystemRef(name) => Err(GraphcalError::DimensionMismatch {
            expected: "a value expression".to_string(),
            found: format!("type-system name `{}`", name.value),
            help: "type-system names can only be used in include/import bindings".to_string(),
            src: src.clone(),
            span: name.span.into(),
        }),

        ExprKind::VariantLiteral { index, variant } => {
            let full_span = index.span.merge(variant.span);
            if let Some(resolved_variant) = resolved_value_index_variant(dag, full_span) {
                return Ok(infer_resolved_index_variant_label(resolved_variant));
            }

            let index_name = legacy_index_name_from_path(&index.value);
            // Validate index exists
            let idx_def = registry
                .indexes
                .get_index(index_name.as_str())
                .ok_or_else(|| GraphcalError::UnknownIndex {
                    name: index_name.clone(),
                    src: src.clone(),
                    span: index.span.into(),
                })?;
            // Validate variant exists in this index
            if !idx_def
                .variants()
                .iter()
                .any(|v| v.as_str() == variant.value.as_str())
            {
                return Err(GraphcalError::UnknownVariant {
                    index_name: index_name.clone(),
                    variant_name: variant.value.clone(),
                    src: src.clone(),
                    span: variant.span.into(),
                });
            }
            Ok(InferredType::Label(InferredIndex::legacy(index_name)))
        }

        ExprKind::UnitLiteral { unit, .. } => {
            let dim = registry
                .units
                .resolve_unit_dimension(unit)
                .map_err(|_| GraphcalError::DimensionOverflow {
                    src: src.clone(),
                    span: unit.span.into(),
                })?
                .ok_or_else(|| {
                    for item in &unit.terms {
                        if registry.units.get_unit(item.name.value.as_str()).is_none() {
                            return GraphcalError::UnknownUnit {
                                name: item.name.value.clone(),
                                src: src.clone(),
                                span: item.name.span.into(),
                            };
                        }
                    }
                    GraphcalError::UnknownUnit {
                        name: UnitName::new("unknown"),
                        src: src.clone(),
                        span: unit.span.into(),
                    }
                })?;
            Ok(InferredType::Scalar(dim))
        }

        ExprKind::ConstRef(ident) => {
            if let Some(resolved_variant) = resolved_value_index_variant(dag, ident.span) {
                return Ok(infer_resolved_index_variant_label(resolved_variant));
            }

            infer_decl_ref_type(&ident.value, ident.span, declared_types, dag, src).map_err(|err| {
                match err {
                    GraphcalError::UnknownGraphRef { name, src, span } => {
                        GraphcalError::UnknownConstRef { name, src, span }
                    }
                    err => err,
                }
            })
        }

        ExprKind::GraphRef(ident) => {
            infer_decl_ref_type(&ident.value, ident.span, declared_types, dag, src)
        }

        ExprKind::LocalRef(ident) => {
            local_types
                .get(ident.name.as_str())
                .cloned()
                .ok_or_else(|| GraphcalError::UnknownLocalRef {
                    name: ident.name.to_string(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
        }

        // --- Scalar operations ---
        ExprKind::BinOp { op, lhs, rhs } => scalar::infer_binop(
            expr,
            op,
            lhs,
            rhs,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::UnaryOp { op, operand } => scalar::infer_unary(
            op,
            operand,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::Convert {
            expr: inner,
            target,
        } => scalar::infer_convert(
            inner,
            target,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::DisplayTimezone {
            expr: inner,
            timezone,
        } => scalar::infer_display_timezone(
            expr,
            inner,
            timezone,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        // --- Function calls ---
        ExprKind::FnCall { callee, args, .. } => functions::infer_fn_call(
            callee,
            args,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        // --- Control flow ---
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => control::infer_if(
            condition,
            then_branch,
            else_branch,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::Match {
            scrutinee, arms, ..
        } => control::infer_match(
            expr,
            scrutinee,
            arms,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        // --- Collections / indexed expressions ---
        ExprKind::ForComp { bindings, body } => collections::infer_for_comp(
            bindings,
            body,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::MapLiteral { entries } => collections::infer_map_or_table_literal(
            expr,
            entries,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::IndexAccess { expr: inner, args } => collections::infer_index_access(
            expr,
            inner,
            args,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::Scan {
            source,
            init,
            acc_name,
            val_name,
            body,
        } => collections::infer_scan(
            source,
            init,
            acc_name,
            val_name,
            body,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } => collections::infer_unfold(
            init,
            prev_name,
            curr_name,
            body,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::FieldAccess { expr: inner, field } => collections::infer_field_access(
            inner,
            field,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::ConstructorCall {
            callee,
            generic_args: constructor_generic_args,
            fields,
        } => collections::infer_constructor_call(
            expr,
            callee,
            constructor_generic_args,
            fields,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::InlineDagRef { path, args, output } => infer_inline_dag_ref(
            expr,
            path,
            args,
            output,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        // `Sugar` and `UnresolvedRef` payloads are `Infallible` in `Resolved`
        // — both arms are statically unreachable.
        #[expect(
            clippy::uninhabited_references,
            reason = "Sugar/UnresolvedRef(Infallible) — proof of unreachability"
        )]
        ExprKind::Sugar(s) | ExprKind::UnresolvedRef(s) => match *s {},
    }
}

fn infer_decl_ref_type(
    name: &ScopedName,
    span: crate::syntax::span::Span,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    if let Some(resolved) = dag.and_then(|dag| dag.resolved_decl_types.get(name)) {
        let dim_sub = HashMap::new();
        let index_sub = HashMap::<GenericParamName, IndexName>::new();
        let nat_sub = HashMap::new();
        return crate::tir::typed::substitute_resolved_type(
            resolved, &dim_sub, &index_sub, &nat_sub, src,
        );
    }

    declared_types
        .get(name)
        .map(InferredType::from)
        .ok_or_else(|| GraphcalError::UnknownGraphRef {
            name: name.clone(),
            src: src.clone(),
            span: span.into(),
        })
}

/// Infer the type of an inline DAG invocation `@<path>(args).<out>`.
///
/// Module-aware TIRs carry HIR-derived [`ResolvedInlineDagCall`] sidecars for
/// call routing. When present, those canonical `DagId` / `ResolvedName<Decl>`
/// identities are authoritative; standalone TIRs retain the legacy source-path
/// lookup by `path` and leaf param/output names.
#[expect(
    clippy::too_many_arguments,
    reason = "passes inference context through"
)]
fn infer_inline_dag_ref(
    expr: &Expr,
    path: &crate::syntax::ast::ModulePath,
    args: &[crate::desugar::resolved_ast::ParamBinding],
    output: &crate::syntax::span::Spanned<crate::syntax::names::DeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    use crate::syntax::names::{ResolvedName, namespace};

    type ResolvedDeclKey = ResolvedName<namespace::Decl>;

    let display_path = path.display_path();
    let resolved_call = dag
        .and_then(|dag| dag.resolved_inline_dag_refs.as_ref())
        .and_then(|refs| refs.calls.get(&expr.span));
    let dag_tir = match resolved_call {
        Some(call) => tir
            .dags
            .get(&call.target)
            .ok_or_else(|| GraphcalError::UnknownDag {
                name: call.target.to_string(),
                src: src.clone(),
                span: path.span.into(),
            })?,
        None => tir
            .lookup_call_target(path)
            .ok_or_else(|| GraphcalError::UnknownDag {
                name: display_path.clone(),
                src: src.clone(),
                span: path.span.into(),
            })?,
    };

    let mut param_decl_types: HashMap<String, DeclaredType> = HashMap::new();
    let mut param_decl_types_by_key: HashMap<ResolvedDeclKey, DeclaredType> = HashMap::new();
    for p in &dag_tir.params {
        if let Some(resolved) = dag_tir.resolved_decl_types.get(&p.name) {
            let dt = crate::tir::typed::resolved_to_declared_type(resolved, src)?;
            param_decl_types.insert(p.name.member().to_string(), dt.clone());
            if let Some(key) = dag_tir.resolved_decl_key_for_local(&p.name) {
                param_decl_types_by_key.insert(key, dt);
            }
        }
    }
    let mut node_decl_types: HashMap<String, DeclaredType> = HashMap::new();
    let mut node_decl_types_by_key: HashMap<ResolvedDeclKey, DeclaredType> = HashMap::new();
    for n in &dag_tir.nodes {
        if let Some(resolved) = dag_tir.resolved_decl_types.get(&n.name) {
            let dt = crate::tir::typed::resolved_to_declared_type(resolved, src)?;
            node_decl_types.insert(n.name.member().to_string(), dt.clone());
            if let Some(key) = dag_tir.resolved_decl_key_for_local(&n.name) {
                node_decl_types_by_key.insert(key, dt);
            }
        }
    }

    let mut bound_names: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(args.len());
    let mut bound_resolved_names: std::collections::HashSet<ResolvedDeclKey> =
        std::collections::HashSet::with_capacity(args.len());
    for binding in args {
        let binding_name = binding.name.name.as_str();
        let expected = match resolved_call {
            Some(call) => {
                let target = call.arg_targets.get(&binding.name.span).ok_or_else(|| {
                    GraphcalError::InternalError {
                        message: format!(
                            "resolved inline-DAG sidecar has no arg target for binding `{binding_name}`"
                        ),
                        src: src.clone(),
                        span: binding.name.span.into(),
                    }
                })?;
                bound_resolved_names.insert(target.clone());
                param_decl_types_by_key.get(target).ok_or_else(|| {
                    GraphcalError::UnknownInlineDagParam {
                        name: target.as_str().to_string(),
                        dag_name: display_path.clone(),
                        src: src.clone(),
                        span: binding.name.span.into(),
                    }
                })?
            }
            None => {
                bound_names.insert(binding_name.to_string());
                param_decl_types.get(binding_name).ok_or_else(|| {
                    GraphcalError::UnknownInlineDagParam {
                        name: binding_name.to_string(),
                        dag_name: display_path.clone(),
                        src: src.clone(),
                        span: binding.name.span.into(),
                    }
                })?
            }
        };
        let found = infer_type(
            &binding.value,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?;
        if !super::helpers::types_match(expected, &found) {
            return Err(GraphcalError::InlineDagArgDimensionMismatch {
                param_name: binding_name.to_string(),
                expected: super::helpers::format_declared_type(expected, registry),
                found: super::helpers::format_inferred_type(&found, registry),
                src: src.clone(),
                span: binding.value.span.into(),
            });
        }
    }

    match resolved_call {
        Some(_) => {
            let mut missing: Vec<String> = param_decl_types_by_key
                .keys()
                .filter(|p| !bound_resolved_names.contains(*p))
                .map(|p| p.as_str().to_string())
                .collect();
            if !missing.is_empty() {
                missing.sort();
                return Err(GraphcalError::MissingInlineDagBindings {
                    missing,
                    dag_name: display_path.clone(),
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
        }
        None => {
            let mut missing: Vec<String> = param_decl_types
                .keys()
                .filter(|p| !bound_names.contains(p.as_str()))
                .cloned()
                .collect();
            if !missing.is_empty() {
                missing.sort();
                return Err(GraphcalError::MissingInlineDagBindings {
                    missing,
                    dag_name: display_path.clone(),
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
        }
    }

    let (output_name, output_decl) = match resolved_call {
        Some(call) => {
            let output_key = &call.output.value;
            let output_decl = node_decl_types_by_key.get(output_key).ok_or_else(|| {
                GraphcalError::UnknownInlineDagOutput {
                    name: output_key.as_str().to_string(),
                    dag_name: display_path.clone(),
                    src: src.clone(),
                    span: output.span.into(),
                }
            })?;
            (output_key.as_str(), output_decl)
        }
        None => {
            let output_decl = node_decl_types.get(output.value.as_str()).ok_or_else(|| {
                GraphcalError::UnknownInlineDagOutput {
                    name: output.value.to_string(),
                    dag_name: display_path.clone(),
                    src: src.clone(),
                    span: output.span.into(),
                }
            })?;
            (output.value.as_str(), output_decl)
        }
    };
    if !dag_tir.pub_nodes.contains(output_name) {
        return Err(GraphcalError::ImportPrivateItem {
            name: output_name.to_string(),
            file_path: display_path,
            src: src.clone(),
            span: output.span.into(),
        });
    }
    Ok(InferredType::from(output_decl))
}
