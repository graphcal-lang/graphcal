use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::syntax::ast::Expr;
use crate::syntax::dimension::Dimension;
use crate::syntax::names::{IndexName, StructTypeName};

use crate::registry::builtins::builtin_functions;
use crate::registry::error::GraphcalError;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::Registry;
use crate::tir::typed::NatLinearForm;

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
    /// A bounded natural number `Fin(N)`: the type of loop variables over `range(N)`.
    ///
    /// A value of type `Fin(N)` satisfies `0 <= value < N`. This enables compile-time
    /// bounds checking: `v[i]` is valid when `i : Fin(N)` and `v : T[M]` with `N <= M`.
    ///
    /// `Fin(N)` is not a user-declarable type — it only arises as the type of loop
    /// variables in `for i: range(N) { ... }`.
    Fin(NatLinearForm),
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

impl InferredType {
    /// Returns `true` if this type is `Int` or `Fin(N)` (integer-like).
    #[must_use]
    pub const fn is_int_like(&self) -> bool {
        matches!(self, Self::Int | Self::Fin(_))
    }
}

/// Check that a declaration's expression type matches its declared type annotation.
#[expect(clippy::too_many_arguments, reason = "passes compilation context")]
fn check_decl_expr_type(
    expr: &Expr,
    name: &crate::registry::resolve_types::ScopedName,
    type_ann_span: &crate::syntax::span::Span,
    declared_types: &HashMap<String, DeclaredType>,
    empty_locals: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let name_str = name.to_string();
    let declared = &declared_types[name_str.as_str()];
    let inferred = infer_type_with_owner(
        expr,
        Some(name_str.as_str()),
        declared_types,
        empty_locals,
        registry,
        builtin_fns,
        src,
    )?;
    if !types_match(declared, &inferred, registry) {
        return Err(GraphcalError::DimensionMismatchInAnnotation {
            declared: format_declared_type(declared, registry),
            inferred: format_inferred_type(&inferred, registry),
            src: src.clone(),
            span: (*type_ann_span).into(),
        });
    }
    Ok(())
}

/// Check dimension consistency of an assert body.
///
/// For expression asserts, verifies the body is boolean. For tolerance asserts,
/// verifies actual/expected have matching dimensions and tolerance is compatible.
fn check_assert_body(
    body: &crate::syntax::ast::AssertBody,
    span: crate::syntax::span::Span,
    declared_types: &HashMap<String, DeclaredType>,
    empty_locals: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match body {
        crate::syntax::ast::AssertBody::Expr(body_expr) => {
            let inferred = infer_type(
                body_expr,
                declared_types,
                empty_locals,
                registry,
                builtin_fns,
                src,
            )?;
            if !is_bool_type(&inferred) {
                return Err(GraphcalError::AssertBodyNotBool {
                    found: format_inferred_type(&inferred, registry),
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
                declared_types,
                empty_locals,
                registry,
                builtin_fns,
                src,
            )?;
            let expected_type = infer_type(
                expected,
                declared_types,
                empty_locals,
                registry,
                builtin_fns,
                src,
            )?;
            let tolerance_type = infer_type(
                tolerance,
                declared_types,
                empty_locals,
                registry,
                builtin_fns,
                src,
            )?;

            // actual and expected must have the same dimension
            let actual_dim = expect_scalar(&actual_type, registry, src, actual.span)?;
            let expected_dim = expect_scalar(&expected_type, registry, src, expected.span)?;
            if actual_dim != expected_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&actual_dim),
                    found: registry.dimensions.format_dimension(&expected_dim),
                    help: "actual and expected in tolerance assertion must have the same dimension"
                        .to_string(),
                    src: src.clone(),
                    span: expected.span.into(),
                });
            }

            // tolerance: same dimension (absolute) or dimensionless/Int (relative %)
            let tolerance_ok = if *is_relative {
                tolerance_type.is_int_like()
                    || matches!(&tolerance_type, InferredType::Scalar(d) if d.is_dimensionless())
            } else {
                let tolerance_dim = expect_scalar(&tolerance_type, registry, src, tolerance.span)?;
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
                        registry.dimensions.format_dimension(&actual_dim),
                        "absolute tolerance must have the same dimension as actual/expected"
                            .to_string(),
                    )
                };
                return Err(GraphcalError::DimensionMismatch {
                    expected: expected_str,
                    found: format_inferred_type(&tolerance_type, registry),
                    help: help_str,
                    src: src.clone(),
                    span: tolerance.span.into(),
                });
            }
        }
    }
    Ok(())
}

/// Check dimensions for all declarations in a file.
///
/// For each const/param/node, infers the dimension of the RHS expression
/// and verifies it matches the declared type annotation. Uses
/// `tir.build_declared_types()` (derived from `resolved_decl_types`) to validate
/// that every RHS expression matches its declared type annotation.
///
/// This is a pure validation step — returns `()` on success.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if dimensions are inconsistent.
pub fn check_dimensions_tir(
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let builtin_fns = builtin_functions();
    let declared_types = tir.build_declared_types(src)?;

    // Validate expressions against declared types
    let empty_locals: HashMap<String, InferredType> = HashMap::new();

    // Check consts, nodes, and params against their declared types
    for entry in &tir.consts {
        check_decl_expr_type(
            &entry.expr,
            &entry.name,
            &entry.type_ann.span,
            &declared_types,
            &empty_locals,
            &tir.registry,
            builtin_fns,
            src,
        )?;
    }
    for entry in &tir.nodes {
        check_decl_expr_type(
            &entry.expr,
            &entry.name,
            &entry.type_ann.span,
            &declared_types,
            &empty_locals,
            &tir.registry,
            builtin_fns,
            src,
        )?;
    }
    for entry in &tir.params {
        let Some(ref value_expr) = entry.default_expr else {
            continue;
        };
        check_decl_expr_type(
            value_expr,
            &entry.name,
            &entry.type_ann.span,
            &declared_types,
            &empty_locals,
            &tir.registry,
            builtin_fns,
            src,
        )?;
    }

    // Validate assert bodies
    for entry in &tir.asserts {
        check_assert_body(
            &entry.body,
            entry.span,
            &declared_types,
            &empty_locals,
            &tir.registry,
            builtin_fns,
            src,
        )?;
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
        src,
    )?;

    if !types_match(declared, &inferred, registry) {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_declared_type(declared, registry),
            found: format_inferred_type(&inferred, registry),
            help: format!(
                "override for `{param_name}` must have dimension {}",
                format_declared_type(declared, registry)
            ),
            src: src.clone(),
            span: expr.span.into(),
        });
    }
    Ok(())
}
