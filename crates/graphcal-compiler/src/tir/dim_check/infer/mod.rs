//! Type inference for expressions.
//!
//! Contains the main `infer_type` function that walks the AST and determines
//! the type (dimension, Bool, Int, struct, or indexed) of each expression.
//! Complex match arms are delegated to submodules.

mod collections;
mod control;
mod functions;
mod scalar;

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::resolved_ast::{Expr, ExprKind};
use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;
use crate::syntax::dimension::Dimension;
use crate::syntax::names::{DeclName, ScopedName, UnitName};

use super::{DeclaredType, InferredType};

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
            // Validate index exists
            let idx_def = registry
                .indexes
                .get_index(index.value.as_str())
                .ok_or_else(|| GraphcalError::UnknownIndex {
                    name: index.value.clone(),
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
                    index_name: index.value.clone(),
                    variant_name: variant.value.clone(),
                    src: src.clone(),
                    span: variant.span.into(),
                });
            }
            Ok(InferredType::Label(index.value.clone()))
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
            let dt =
                declared_types
                    .get(&ident.value)
                    .ok_or_else(|| GraphcalError::UnknownConstRef {
                        name: DeclName::new(ident.value.to_string()),
                        src: src.clone(),
                        span: ident.span.into(),
                    })?;
            Ok(InferredType::from(dt))
        }

        ExprKind::GraphRef(ident) => {
            let dt =
                declared_types
                    .get(&ident.value)
                    .ok_or_else(|| GraphcalError::UnknownGraphRef {
                        name: DeclName::new(ident.value.to_string()),
                        src: src.clone(),
                        span: ident.span.into(),
                    })?;
            Ok(InferredType::from(dt))
        }

        ExprKind::LocalRef(ident) => {
            local_types
                .get(&ident.name)
                .cloned()
                .ok_or_else(|| GraphcalError::UnknownLocalRef {
                    name: ident.name.clone(),
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
            tir,
            registry,
            builtin_fns,
            src,
        ),

        // --- Function calls ---
        ExprKind::FnCall { name, args, .. } => functions::infer_fn_call(
            name,
            args,
            declared_types,
            local_types,
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
            tir,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::StructConstruction {
            type_name,
            type_args: constructor_type_args,
            fields,
        } => collections::infer_struct_construction(
            expr,
            type_name,
            constructor_type_args,
            fields,
            declared_types,
            local_types,
            tir,
            registry,
            builtin_fns,
            src,
        ),

        // TupleMatch is desugared before dim-checking.
        #[expect(
            clippy::unreachable,
            reason = "invariant: desugared before dim-checking"
        )]
        ExprKind::TupleMatch { .. } => {
            unreachable!("TupleMatch should be desugared before dim-checking")
        }

        ExprKind::InlineDagRef { path, args, output } => infer_inline_dag_ref(
            expr,
            path,
            args,
            output,
            declared_types,
            local_types,
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

/// Infer the type of an inline DAG invocation `@<path>(args).<out>`.
///
/// Looks up the called dag via `tir`, indexed by [`DagKey`]: the bare
/// DAG name for same-file calls (`@dag(args).out`) or a `(module-alias,
/// dag-name)` pair for cross-file qualified calls (`@module.dag(args).out`,
/// matching the key inserted by the cross-file dag-merge step). Both
/// variants share the same compiled-TIR resolution path: param types come
/// from `dag_tir.resolved_decl_types`, and `dag_tir.pub_nodes` gates
/// projection of non-`pub` outputs.
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
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let display_path = path.display_path();
    let dag_tir = tir
        .lookup_call_target(path)
        .ok_or_else(|| GraphcalError::UnknownDag {
            name: display_path.clone(),
            src: src.clone(),
            span: path.span.into(),
        })?;

    // Map param/node member names to their pre-resolved declared types.
    // Using the compiled TIR (not the raw AST) means the same code path
    // serves same-file and cross-file calls — the dep's TIR has already
    // resolved its type annotations in its own registry's scope.
    //
    // Locals only here: param/node binding names at a call site are bare
    // identifiers, so a `String`-keyed table is the right shape.
    let mut param_decl_types: HashMap<String, DeclaredType> = HashMap::new();
    for p in &dag_tir.params {
        if let Some(resolved) = dag_tir.resolved_decl_types.get(&p.name) {
            let dt = crate::tir::typed::resolved_to_declared_type(resolved, src)?;
            param_decl_types.insert(p.name.member().to_string(), dt);
        }
    }
    let mut node_decl_types: HashMap<String, DeclaredType> = HashMap::new();
    for n in &dag_tir.nodes {
        if let Some(resolved) = dag_tir.resolved_decl_types.get(&n.name) {
            let dt = crate::tir::typed::resolved_to_declared_type(resolved, src)?;
            node_decl_types.insert(n.name.member().to_string(), dt);
        }
    }

    // Check each binding names a real param and type-matches its annotation.
    let mut bound_names: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(args.len());
    for binding in args {
        let binding_name = binding.name.name.as_str();
        let expected = param_decl_types.get(binding_name).ok_or_else(|| {
            GraphcalError::UnknownInlineDagParam {
                name: binding_name.to_string(),
                dag_name: display_path.clone(),
                src: src.clone(),
                span: binding.name.span.into(),
            }
        })?;
        let found = infer_type(
            &binding.value,
            declared_types,
            local_types,
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
        bound_names.insert(binding_name.to_string());
    }

    // Every param declared in the dag must be bound at the call site.
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

    // Project the output node; reject projection of a non-`pub` node with
    // the same shape as the `include lib_dag(args) { private_result }` form.
    let output_decl = node_decl_types.get(output.value.as_str()).ok_or_else(|| {
        GraphcalError::UnknownInlineDagOutput {
            name: output.value.to_string(),
            dag_name: display_path.clone(),
            src: src.clone(),
            span: output.span.into(),
        }
    })?;
    if !dag_tir.pub_nodes.contains(output.value.as_str()) {
        return Err(GraphcalError::ImportPrivateItem {
            name: output.value.to_string(),
            file_path: display_path,
            src: src.clone(),
            span: output.span.into(),
        });
    }
    Ok(InferredType::from(output_decl))
}
