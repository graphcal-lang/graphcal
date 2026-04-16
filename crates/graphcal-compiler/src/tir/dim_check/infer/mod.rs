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

use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;
use crate::syntax::ast::{Expr, ExprKind};
use crate::syntax::dimension::Dimension;
use crate::syntax::names::UnitName;

use super::{DeclaredType, InferredType};

/// Infer the type (dimension or struct) of an expression.
///
/// `owner_decl_name` is the name of the top-level declaration (node/const/param)
/// that contains this expression. It is threaded through to `infer_unfold` so
/// the unfold can look up the owning declaration's range index precisely.
/// Pass `None` when the owner is not known (e.g., in override dimension checks).
pub(super) fn infer_type(
    expr: &Expr,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    infer_type_with_owner(
        expr,
        None,
        declared_types,
        local_types,
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
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
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
            let dim = registry.units.resolve_unit_dimension(unit).ok_or_else(|| {
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

        ExprKind::ConstRef(ident) | ExprKind::QualifiedConstRef { name: ident, .. } => {
            let dt = declared_types.get(ident.value.as_str()).ok_or_else(|| {
                GraphcalError::UnknownConstRef {
                    name: ident.value.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                }
            })?;
            Ok(InferredType::from(dt))
        }

        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => {
            let dt = declared_types.get(ident.value.as_str()).ok_or_else(|| {
                GraphcalError::UnknownGraphRef {
                    name: ident.value.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                }
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
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::UnaryOp { op, operand } => scalar::infer_unary(
            op,
            operand,
            declared_types,
            local_types,
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
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::AsCast {
            expr: inner,
            target_type,
        } => scalar::infer_as_cast(
            expr,
            inner,
            target_type,
            declared_types,
            local_types,
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
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            collections::infer_map_or_table_literal(
                expr,
                entries,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                src,
            )
        }

        ExprKind::IndexAccess { expr: inner, args } => collections::infer_index_access(
            expr,
            inner,
            args,
            declared_types,
            local_types,
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
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::FieldAccess { expr: inner, field } => collections::infer_field_access(
            inner,
            field,
            declared_types,
            local_types,
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

        // NameRef/QualifiedNameRef are resolved before dim-checking.
        #[expect(
            clippy::unreachable,
            reason = "invariant: resolved before dim-checking"
        )]
        ExprKind::NameRef(_) | ExprKind::QualifiedNameRef { .. } => {
            unreachable!("NameRef/QualifiedNameRef should be resolved before dim-checking")
        }
    }
}
