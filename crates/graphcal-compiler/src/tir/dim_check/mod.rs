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

    // Validate domain constraint bound expression dimensions
    check_domain_constraint_dimensions(tir, &declared_types, &empty_locals, builtin_fns, src)?;

    // Recursively dim-check every compiled inline-dag body.
    //
    // Each dag body was compiled as a virtual file in `type_resolve`, so its
    // own registry already contains the enclosing file's types plus any
    // sibling dags. Checking it here catches dimension errors in dag body
    // expressions at compile time, rather than letting them slip through to
    // runtime on the first call site.
    for dag_tir in tir.dags.values() {
        check_dimensions_tir(dag_tir, src)?;
    }

    Ok(())
}

/// What a domain bound expression must infer to for a given target type.
enum ExpectedBound {
    /// Bound must be `Scalar(d)`. `Int` is also accepted when `d` is dimensionless.
    Scalar(Dimension),
    /// Bound must be unitless: `Int`, or `Scalar` with the dimensionless dimension.
    Int,
}

/// Check that domain constraint bound expressions have the correct type.
///
/// For each param/node with `(min: ..., max: ...)` constraints whose target type
/// is `Scalar(d)`, `Dimensionless`, or `Int`, infers the type of each bound
/// expression using the regular type checker and verifies it matches:
/// - `Scalar(d)` target: bound must be `Scalar(d)` (or `Int` if `d` is dimensionless).
/// - `Dimensionless` target: bound must be `Scalar(dimensionless)` or `Int`.
/// - `Int` target: bound must be `Int` or `Scalar(dimensionless)` — units forbidden.
///
/// Other targets (e.g., `Bool`) are skipped here and handled by
/// `validate_constraint_target` in `exec_plan` (which raises `InvalidDomainTarget`).
fn check_domain_constraint_dimensions(
    tir: &crate::tir::typed::TIR,
    declared_types: &HashMap<String, DeclaredType>,
    empty_locals: &HashMap<String, InferredType>,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let decl_iter = tir
        .consts
        .iter()
        .map(|e| (&e.name, &e.type_ann))
        .chain(tir.params.iter().map(|e| (&e.name, &e.type_ann)))
        .chain(tir.nodes.iter().map(|e| (&e.name, &e.type_ann)));

    for (name, type_ann) in decl_iter {
        let bounds = extract_domain_bounds(type_ann);
        if bounds.is_empty() {
            continue;
        }

        let resolved = tir.resolved_decl_types.get(name);
        let base_resolved = resolved.map(strip_indexed);
        let expected = match base_resolved {
            Some(crate::tir::typed::ResolvedTypeExpr::Scalar(dim)) => {
                ExpectedBound::Scalar(dim.clone())
            }
            Some(crate::tir::typed::ResolvedTypeExpr::Dimensionless) => {
                ExpectedBound::Scalar(Dimension::dimensionless())
            }
            Some(crate::tir::typed::ResolvedTypeExpr::Int) => ExpectedBound::Int,
            _ => continue,
        };

        for bound in bounds {
            let inferred = infer_type(
                &bound.value,
                declared_types,
                empty_locals,
                &tir.registry,
                builtin_fns,
                src,
            )?;
            check_one_bound(name, bound, &inferred, &expected, &tir.registry, src)?;
        }
    }

    Ok(())
}

fn check_one_bound(
    name: &crate::registry::resolve_types::ScopedName,
    bound: &crate::syntax::ast::DomainBound,
    inferred: &InferredType,
    expected: &ExpectedBound,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match expected {
        ExpectedBound::Scalar(target_dim) => {
            let ok = match inferred {
                InferredType::Scalar(d) => d == target_dim,
                InferredType::Int => target_dim.is_dimensionless(),
                _ => false,
            };
            if ok {
                return Ok(());
            }
            let bound_dim_str = match inferred {
                InferredType::Scalar(d) => registry.dimensions.format_dimension(d),
                other => format_inferred_type(other, registry),
            };
            Err(GraphcalError::DomainDimensionMismatch {
                name: name.to_string(),
                type_dim: registry.dimensions.format_dimension(target_dim),
                bound_name: bound.kind.to_string(),
                bound_dim: bound_dim_str,
                src: src.clone(),
                span: bound.span.into(),
            })
        }
        ExpectedBound::Int => {
            let ok = match inferred {
                InferredType::Int => true,
                InferredType::Scalar(d) => d.is_dimensionless(),
                _ => false,
            };
            if ok {
                return Ok(());
            }
            Err(GraphcalError::IntDomainBoundNotUnitless {
                name: name.to_string(),
                bound_name: bound.kind.to_string(),
                bound_type: format_inferred_type(inferred, registry),
                src: src.clone(),
                span: bound.span.into(),
            })
        }
    }
}

/// Extract `DomainBound`s from a `TypeExpr`, handling indexed types.
///
/// For `Velocity(min: 0)[Maneuver]`, the constraints are on the base `Velocity`,
/// not on the outer `Indexed` wrapper.
fn extract_domain_bounds(
    type_ann: &crate::syntax::ast::TypeExpr,
) -> &[crate::syntax::ast::DomainBound] {
    if !type_ann.constraints.is_empty() {
        return &type_ann.constraints;
    }
    if let crate::syntax::ast::TypeExprKind::Indexed { base, .. } = &type_ann.kind {
        return &base.constraints;
    }
    &[]
}

/// Strip `Indexed` wrappers to get the base resolved type.
fn strip_indexed(
    resolved: &crate::tir::typed::ResolvedTypeExpr,
) -> &crate::tir::typed::ResolvedTypeExpr {
    match resolved {
        crate::tir::typed::ResolvedTypeExpr::Indexed { base, .. } => strip_indexed(base),
        other => other,
    }
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
