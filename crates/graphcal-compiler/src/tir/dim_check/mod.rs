use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::syntax::ast::Expr;
use crate::syntax::dimension::Dimension;
use crate::syntax::names::{FnName, IndexName, StructTypeName};

use crate::registry::time_scale::TimeScale;

use crate::registry::builtins::builtin_functions;
use crate::registry::error::GraphcalError;
use crate::registry::registry::Registry;
use crate::tir::tir::ResolvedFnSig;

pub(crate) use helpers::format_inferred_type;
use helpers::{expect_scalar, format_declared_type, is_bool_type, types_match};
use infer::{infer_type, infer_type_with_owner};

mod builtins;
mod helpers;
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::trivially_copy_pass_by_ref,
    clippy::doc_markdown,
    reason = "inference functions pass compilation context through many parameters; \
              large match on ExprKind variants is inherently long"
)]
mod infer;
#[cfg(test)]
mod tests;

pub use crate::registry::declared_type::DeclaredType;

/// The inferred type of an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferredType {
    Scalar(Dimension),
    Bool,
    Int,
    /// A datetime instant in a specific time scale.
    Datetime(TimeScale),
    /// A label of a named index (e.g., `Maneuver::Departure` has type `Label(Maneuver)`).
    Label(IndexName),
    /// A struct type, optionally with concrete type arguments for generic structs.
    Struct(StructTypeName, Vec<Self>),
    Indexed {
        element: Box<Self>,
        index: IndexName,
    },
}

/// Check dimensions for all declarations in a file.
///
/// For each const/param/node, infers the dimension of the RHS expression
/// and verifies it matches the declared type annotation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if dimensions are inconsistent.
/// Check dimensions for all declarations using a TIR.
///
/// Uses `tir.build_declared_types()` (derived from `resolved_decl_types`) to validate that
/// every RHS expression matches its declared type annotation.
///
/// This is a pure validation step — returns `()` on success.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if dimensions are inconsistent.
#[expect(
    clippy::too_many_lines,
    reason = "dimension checking for all declaration kinds including assert tolerance"
)]
pub fn check_dimensions_tir(
    tir: &crate::tir::tir::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let builtin_fns = builtin_functions();
    let declared_types = tir.build_declared_types(src)?;

    // Validate expressions against declared types
    let empty_locals: HashMap<String, InferredType> = HashMap::new();

    // Check consts (always have expressions)
    for entry in &tir.consts {
        let name_str = entry.name.to_string();
        let declared = &declared_types[name_str.as_str()];
        let inferred = infer_type_with_owner(
            &entry.expr,
            Some(name_str.as_str()),
            &declared_types,
            &empty_locals,
            &tir.registry,
            builtin_fns,
            &tir.resolved_fn_sigs,
            src,
        )?;

        if !types_match(declared, &inferred, &tir.registry) {
            return Err(GraphcalError::DimensionMismatchInAnnotation {
                declared: format_declared_type(declared, &tir.registry),
                inferred: format_inferred_type(&inferred, &tir.registry),
                src: src.clone(),
                span: entry.type_ann.span.into(),
            });
        }
    }

    // Check nodes (always have expressions)
    for entry in &tir.nodes {
        let name_str = entry.name.to_string();
        let declared = &declared_types[name_str.as_str()];
        let inferred = infer_type_with_owner(
            &entry.expr,
            Some(name_str.as_str()),
            &declared_types,
            &empty_locals,
            &tir.registry,
            builtin_fns,
            &tir.resolved_fn_sigs,
            src,
        )?;

        if !types_match(declared, &inferred, &tir.registry) {
            return Err(GraphcalError::DimensionMismatchInAnnotation {
                declared: format_declared_type(declared, &tir.registry),
                inferred: format_inferred_type(&inferred, &tir.registry),
                src: src.clone(),
                span: entry.type_ann.span.into(),
            });
        }
    }

    // Check params (may be required with no default expression to check)
    for entry in &tir.params {
        let Some(ref value_expr) = entry.default_expr else {
            continue;
        };
        let name_str = entry.name.to_string();
        let declared = &declared_types[name_str.as_str()];
        let inferred = infer_type_with_owner(
            value_expr,
            Some(name_str.as_str()),
            &declared_types,
            &empty_locals,
            &tir.registry,
            builtin_fns,
            &tir.resolved_fn_sigs,
            src,
        )?;

        if !types_match(declared, &inferred, &tir.registry) {
            return Err(GraphcalError::DimensionMismatchInAnnotation {
                declared: format_declared_type(declared, &tir.registry),
                inferred: format_inferred_type(&inferred, &tir.registry),
                src: src.clone(),
                span: entry.type_ann.span.into(),
            });
        }
    }

    // Validate assert bodies
    for entry in &tir.asserts {
        let body = &entry.body;
        let span = entry.span;
        match body {
            crate::syntax::ast::AssertBody::Expr(body_expr) => {
                let inferred = infer_type(
                    body_expr,
                    &declared_types,
                    &empty_locals,
                    &tir.registry,
                    builtin_fns,
                    &tir.resolved_fn_sigs,
                    src,
                )?;
                let is_bool = is_bool_type(&inferred);
                if !is_bool {
                    return Err(GraphcalError::AssertBodyNotBool {
                        found: format_inferred_type(&inferred, &tir.registry),
                        src: src.clone(),
                        span: span.into(),
                    });
                }
            }
            crate::syntax::ast::AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
                is_relative,
            } => {
                let actual_type = infer_type(
                    actual,
                    &declared_types,
                    &empty_locals,
                    &tir.registry,
                    builtin_fns,
                    &tir.resolved_fn_sigs,
                    src,
                )?;
                let expected_type = infer_type(
                    expected,
                    &declared_types,
                    &empty_locals,
                    &tir.registry,
                    builtin_fns,
                    &tir.resolved_fn_sigs,
                    src,
                )?;
                let tolerance_type = infer_type(
                    tolerance,
                    &declared_types,
                    &empty_locals,
                    &tir.registry,
                    builtin_fns,
                    &tir.resolved_fn_sigs,
                    src,
                )?;

                // actual and expected must have the same dimension
                let actual_dim = expect_scalar(&actual_type, &tir.registry, src, actual.span)?;
                let expected_dim =
                    expect_scalar(&expected_type, &tir.registry, src, expected.span)?;
                if actual_dim != expected_dim {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: tir.registry.dimensions.format_dimension(&actual_dim),
                        found: tir.registry.dimensions.format_dimension(&expected_dim),
                        src: src.clone(),
                        span: expected.span.into(),
                        help: "actual and expected in tolerance assertion must have the same dimension".to_string(),
                    });
                }

                // tolerance: same dimension (absolute) or dimensionless/Int (relative %)
                let tolerance_ok = if *is_relative {
                    // Relative tolerance: accept Int or Dimensionless scalar
                    matches!(tolerance_type, InferredType::Int)
                        || matches!(&tolerance_type, InferredType::Scalar(d) if d.is_dimensionless())
                } else {
                    let tolerance_dim =
                        expect_scalar(&tolerance_type, &tir.registry, src, tolerance.span)?;
                    tolerance_dim == actual_dim
                };
                if !tolerance_ok {
                    let (expected_str, help_str) = if *is_relative {
                        (
                            "Dimensionless".to_string(),
                            "relative tolerance (%) must be dimensionless".to_string(),
                        )
                    } else {
                        (
                            tir.registry.dimensions.format_dimension(&actual_dim),
                            "absolute tolerance must have the same dimension as actual/expected"
                                .to_string(),
                        )
                    };
                    return Err(GraphcalError::DimensionMismatch {
                        expected: expected_str,
                        found: format_inferred_type(&tolerance_type, &tir.registry),
                        src: src.clone(),
                        span: tolerance.span.into(),
                        help: help_str,
                    });
                }
            }
        }
    }

    Ok(())
}

/// Check that an override expression has the correct dimension for the given param.
///
/// # Errors
///
/// Returns a [`GraphcalError::DimensionMismatch`] if the expression's inferred
/// dimension does not match the declared type of the param.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn check_override_dimension(
    expr: &Expr,
    param_name: &str,
    declared_types: &HashMap<String, DeclaredType>,
    registry: &Registry,
    resolved_fn_sigs: &HashMap<FnName, ResolvedFnSig>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let builtin_fns = builtin_functions();
    let empty_locals: HashMap<String, InferredType> = HashMap::new();

    let declared = &declared_types[param_name];
    let inferred = infer_type(
        expr,
        declared_types,
        &empty_locals,
        registry,
        builtin_fns,
        resolved_fn_sigs,
        src,
    )?;

    if !types_match(declared, &inferred, registry) {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_declared_type(declared, registry),
            found: format_inferred_type(&inferred, registry),
            src: src.clone(),
            span: expr.span.into(),
            help: format!(
                "override for `{param_name}` must have dimension {}",
                format_declared_type(declared, registry)
            ),
        });
    }
    Ok(())
}
