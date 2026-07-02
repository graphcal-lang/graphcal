use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::MulDivOp;
use crate::dimension::{Dimension, Rational};
use crate::nat::{Monomial, NatPolyForm};
use crate::registry::declared_type::IndexTypeRef;
use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;
use crate::syntax::index_name::IndexName;
use crate::syntax::span::Span;
use crate::syntax::type_name::GenericParamName;

use super::{ResolvedDimTerm, ResolvedIndex, ResolvedTypeExpr};

// ---------------------------------------------------------------------------
// Conversion to DeclaredType
// ---------------------------------------------------------------------------

/// Convert a non-generic [`ResolvedTypeExpr`] to a `DeclaredType`.
///
/// This is used by downstream stages (`dim_check`, `eval`) that work with concrete
/// types. Generic variants (`GenericDimParam`, `GenericDimExpr`, generic indexes)
/// cannot be converted and will return an error.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if the resolved type contains unresolved generic
/// parameters.
pub fn resolved_to_declared_type(
    resolved: &ResolvedTypeExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::registry::declared_type::DeclaredType, GraphcalError> {
    use crate::registry::declared_type::{DeclaredType, StructTypeRef};

    match resolved {
        ResolvedTypeExpr::Dimensionless => Ok(DeclaredType::Scalar(Dimension::dimensionless())),
        ResolvedTypeExpr::Bool => Ok(DeclaredType::Bool),
        ResolvedTypeExpr::Int => Ok(DeclaredType::Int),
        ResolvedTypeExpr::Datetime(scale) => Ok(DeclaredType::Datetime(*scale)),
        ResolvedTypeExpr::IndexArg(index) => Err(GraphcalError::EvalError {
            message: format!(
                "index `{}` cannot be used as a value type",
                index.format_for_diagnostic()
            ),
            src: src.clone(),
            span: resolved_type_expr_span(resolved).into(),
        }),
        ResolvedTypeExpr::Scalar(dim) => Ok(DeclaredType::Scalar(dim.clone())),
        ResolvedTypeExpr::Struct(name, _) => Ok(DeclaredType::Struct(
            StructTypeRef::from_resolved(name.clone()),
            vec![],
        )),
        ResolvedTypeExpr::GenericStruct {
            name, type_args, ..
        } => {
            let mut declared_args = Vec::with_capacity(type_args.len());
            for arg in type_args {
                declared_args.push(resolved_type_arg_to_declared_type(arg, src)?);
            }
            Ok(DeclaredType::Struct(
                StructTypeRef::from_resolved(name.clone()),
                declared_args,
            ))
        }
        ResolvedTypeExpr::GenericDimParam(name, span) => Err(GraphcalError::EvalError {
            message: format!("cannot use generic dimension parameter `{name}` as a concrete type"),
            src: src.clone(),
            span: (*span).into(),
        }),
        ResolvedTypeExpr::GenericTypeParam(name, span) => Err(GraphcalError::EvalError {
            message: format!("cannot use generic type parameter `{name}` as a concrete type"),
            src: src.clone(),
            span: (*span).into(),
        }),
        ResolvedTypeExpr::GenericDimExpr { span, .. } => Err(GraphcalError::EvalError {
            message: "cannot use generic dimension expression as a concrete type".to_string(),
            src: src.clone(),
            span: (*span).into(),
        }),
        ResolvedTypeExpr::Indexed { base, indexes } => {
            let mut result = resolved_to_declared_type(base, src)?;
            for idx in indexes.iter().rev() {
                match idx {
                    ResolvedIndex::Concrete(name, _) => {
                        result = DeclaredType::Indexed {
                            element: Box::new(result),
                            index: IndexTypeRef::from_resolved(name.clone()),
                        };
                    }
                    ResolvedIndex::NatExpr(form, span) => {
                        if !form.is_constant() {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "cannot use generic nat expression `{}` as a concrete type",
                                    form.format()
                                ),
                                src: src.clone(),
                                span: (*span).into(),
                            });
                        }
                        let nat_range =
                            crate::registry::types::NatRangeIndex::try_from_u64(form.constant())
                                .map_err(|err| GraphcalError::EvalError {
                                    message: err.to_string(),
                                    src: src.clone(),
                                    span: (*span).into(),
                                })?;
                        result = DeclaredType::Indexed {
                            element: Box::new(result),
                            index: IndexTypeRef::from_nat_range(nat_range),
                        };
                    }
                    ResolvedIndex::GenericParam(name, span) => {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "cannot use generic index parameter `{name}` as a concrete type"
                            ),
                            src: src.clone(),
                            span: (*span).into(),
                        });
                    }
                }
            }
            Ok(result)
        }
    }
}

fn resolved_type_arg_to_declared_type(
    resolved: &ResolvedTypeExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::registry::declared_type::DeclaredType, GraphcalError> {
    match resolved {
        ResolvedTypeExpr::IndexArg(index) => resolved_index_to_declared_arg(index, src),
        _ => resolved_to_declared_type(resolved, src),
    }
}

fn resolved_type_expr_span(resolved: &ResolvedTypeExpr) -> Span {
    match resolved {
        ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::Bool
        | ResolvedTypeExpr::Int
        | ResolvedTypeExpr::Datetime(_)
        | ResolvedTypeExpr::Scalar(_) => Span::new(0, 0),
        ResolvedTypeExpr::IndexArg(index) => resolved_index_span(index),
        ResolvedTypeExpr::Struct(_, span)
        | ResolvedTypeExpr::GenericDimParam(_, span)
        | ResolvedTypeExpr::GenericTypeParam(_, span)
        | ResolvedTypeExpr::GenericDimExpr { span, .. }
        | ResolvedTypeExpr::GenericStruct { span, .. } => *span,
        ResolvedTypeExpr::Indexed { base, .. } => resolved_type_expr_span(base),
    }
}

const fn resolved_index_span(index: &ResolvedIndex) -> Span {
    match index {
        ResolvedIndex::Concrete(_, span)
        | ResolvedIndex::GenericParam(_, span)
        | ResolvedIndex::NatExpr(_, span) => *span,
    }
}

fn resolved_index_to_declared_arg(
    index: &ResolvedIndex,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::registry::declared_type::DeclaredType, GraphcalError> {
    let reference = match index {
        ResolvedIndex::Concrete(name, _) => IndexTypeRef::from_resolved(name.clone()),
        ResolvedIndex::NatExpr(form, span) => IndexTypeRef::from_nat_range_form(form.clone())
            .map_err(|err| GraphcalError::EvalError {
                message: err.to_string(),
                src: src.clone(),
                span: (*span).into(),
            })?,
        ResolvedIndex::GenericParam(name, span) => {
            return Err(GraphcalError::EvalError {
                message: format!("generic index parameter `{name}` is not bound"),
                src: src.clone(),
                span: (*span).into(),
            });
        }
    };
    Ok(crate::registry::declared_type::DeclaredType::IndexArg(
        reference,
    ))
}

fn resolved_index_to_inferred(
    index: &ResolvedIndex,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::tir::dim_check::InferredIndex, GraphcalError> {
    let reference = match index {
        ResolvedIndex::Concrete(name, _) => IndexTypeRef::from_resolved(name.clone()),
        ResolvedIndex::NatExpr(form, span) => IndexTypeRef::from_nat_range_form(form.clone())
            .map_err(|err| GraphcalError::EvalError {
                message: err.to_string(),
                src: src.clone(),
                span: (*span).into(),
            })?,
        ResolvedIndex::GenericParam(name, span) => {
            return Err(GraphcalError::EvalError {
                message: format!("generic index parameter `{name}` is not bound"),
                src: src.clone(),
                span: (*span).into(),
            });
        }
    };
    Ok(crate::tir::dim_check::InferredIndex::from_ref(reference))
}

fn resolved_index_matches_inferred(
    expected: &ResolvedIndex,
    actual: &crate::tir::dim_check::InferredIndex,
) -> bool {
    match expected {
        ResolvedIndex::Concrete(name, _) => actual.matches_resolved(name),
        ResolvedIndex::GenericParam(_, _) => false,
        ResolvedIndex::NatExpr(form, _) => actual.nat_range_form().as_ref() == Some(form),
    }
}

fn resolved_index_display_name(index: &ResolvedIndex) -> IndexName {
    match index {
        ResolvedIndex::Concrete(name, _) => name.to_unowned_def_name(),
        ResolvedIndex::GenericParam(name, _) => IndexName::from_atom(name.atom().clone()),
        ResolvedIndex::NatExpr(form, _) => {
            IndexName::expect_valid(format!("range({})", form.format()))
        }
    }
}

// ---------------------------------------------------------------------------
// Nat polynomial form unification
// ---------------------------------------------------------------------------

/// Solve a polynomial equation `form = target` for Nat generic params.
///
/// Substitutes already-bound variables, then:
/// - If no unbound vars remain: checks evaluated form == target.
/// - If exactly one unbound var appears only linearly (degree 1): solves the linear equation.
/// - Otherwise: returns an error (ambiguous or non-linear in unbound vars).
pub(in crate::tir::typed) fn unify_nat_poly_form(
    form: &NatPolyForm,
    target: u64,
    nat_sub: &mut HashMap<GenericParamName, u64>,
    actual_idx: &IndexName,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), GraphcalError> {
    // Substitute already-bound variables in each monomial, collecting
    // a reduced polynomial in only unbound variables + a constant part.
    let mut reduced_constant: u64 = 0;
    // (reduced_monomial, coefficient) pairs for terms with unbound variables
    let mut reduced_terms: BTreeMap<Monomial, u64> = BTreeMap::new();

    // Shared "this form cannot match the actual index" error, used both for
    // genuine mismatches and for arithmetic overflow during reduction (an
    // overflowing form cannot match any concrete index size).
    let form_mismatch = || GraphcalError::IndexMismatch {
        expected: IndexName::expect_valid(format!("range({})", form.format())),
        found: actual_idx.clone(),
        src: src.clone(),
        span: span.into(),
    };

    for (mono, coeff) in &form.terms {
        let (remaining_mono, factor) = mono.substitute(nat_sub).ok_or_else(form_mismatch)?;
        let term_value = coeff.checked_mul(factor).ok_or_else(form_mismatch)?;
        if remaining_mono.is_constant() {
            reduced_constant = reduced_constant
                .checked_add(term_value)
                .ok_or_else(form_mismatch)?;
        } else {
            let entry = reduced_terms.entry(remaining_mono).or_insert(0);
            *entry = entry.checked_add(term_value).ok_or_else(form_mismatch)?;
        }
    }
    // Remove zero terms
    reduced_terms.retain(|_, c| *c != 0);

    if reduced_terms.is_empty() {
        // All variables bound — check equality
        if reduced_constant != target {
            let expected = match form.evaluate(nat_sub) {
                Some(n) => crate::registry::types::NatRangeIndex::try_from_u64(n)
                    .map_err(|err| GraphcalError::EvalError {
                        message: err.to_string(),
                        src: src.clone(),
                        span: span.into(),
                    })?
                    .display_name(),
                None => IndexName::expect_valid(format!("range({})", form.format())),
            };
            return Err(GraphcalError::IndexMismatch {
                expected,
                found: actual_idx.clone(),
                src: src.clone(),
                span: span.into(),
            });
        }
        return Ok(());
    }

    // Check if exactly one unbound variable appears, only at degree 1
    let mut unbound_vars = std::collections::BTreeSet::new();
    for mono in reduced_terms.keys() {
        for var in mono.0.keys() {
            unbound_vars.insert(var.clone());
        }
    }

    if let [var] = unbound_vars.iter().collect::<Vec<_>>().as_slice() {
        let var = (*var).clone();
        // Check all remaining monomials are linear in this variable
        let all_linear = reduced_terms
            .keys()
            .all(|m| m.0.len() == 1 && m.0.get(&var) == Some(&1));

        if all_linear {
            // Solve: coeff * var + reduced_constant = target
            let total_coeff = reduced_terms
                .values()
                .try_fold(0u64, |acc, c| acc.checked_add(*c))
                .ok_or_else(form_mismatch)?;
            if target < reduced_constant {
                return Err(form_mismatch());
            }
            let remainder = target - reduced_constant;
            if total_coeff == 0 || !remainder.is_multiple_of(total_coeff) {
                return Err(form_mismatch());
            }
            let value = remainder / total_coeff;
            bind_or_check(nat_sub, var, value, |prev, _| {
                match crate::registry::types::NatRangeIndex::try_from_u64(*prev) {
                    Ok(index) => GraphcalError::IndexMismatch {
                        expected: index.display_name(),
                        found: actual_idx.clone(),
                        src: src.clone(),
                        span: span.into(),
                    },
                    Err(err) => GraphcalError::EvalError {
                        message: err.to_string(),
                        src: src.clone(),
                        span: span.into(),
                    },
                }
            })?;
            return Ok(());
        }
    }

    // Multiple unbound variables or non-linear — ambiguous
    let var_names: Vec<&str> = unbound_vars.iter().map(GenericParamName::as_str).collect();
    Err(GraphcalError::EvalError {
        message: format!(
            "cannot infer Nat parameters [{}] from a single index — \
             provide more arguments or use explicit type annotations",
            var_names.join(", ")
        ),
        src: src.clone(),
        span: span.into(),
    })
}

// ---------------------------------------------------------------------------
// Unification
// ---------------------------------------------------------------------------

/// Bind a generic parameter in a substitution map, or check consistency if already bound.
///
/// If `key` is not yet in `sub`, inserts `(key, value)`. If `key` is already bound
/// to a value equal to `value`, succeeds. Otherwise, calls `on_conflict` with the
/// previously bound value and the new value to produce an error.
fn bind_or_check<K, V, E>(
    sub: &mut HashMap<K, V>,
    key: K,
    value: V,
    on_conflict: impl FnOnce(&V, &V) -> E,
) -> Result<(), E>
where
    K: Eq + std::hash::Hash,
    V: PartialEq,
{
    if let Some(prev) = sub.get(&key) {
        if *prev != value {
            return Err(on_conflict(prev, &value));
        }
    } else {
        sub.insert(key, value);
    }
    Ok(())
}

/// Unify a resolved type expression against an actual inferred type,
/// binding generic dimension and index parameters.
///
/// For example, if `resolved` is `GenericDimParam("D")` and `actual` is
/// `Scalar(Length)`, binds `D = Length` in `dim_sub`.
///
/// # Errors
///
/// Returns a [`GraphcalError`] on type mismatch or conflicting bindings.
#[expect(
    clippy::too_many_lines,
    reason = "complex generic unification requires many match arms"
)]
#[expect(
    clippy::implicit_hasher,
    reason = "always called with standard HashMap"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "unification needs all substitution maps, registry, and source context"
)]
pub fn unify_resolved_type(
    resolved: &ResolvedTypeExpr,
    actual: &crate::tir::dim_check::InferredType,
    dim_sub: &mut HashMap<GenericParamName, Dimension>,
    index_sub: &mut HashMap<GenericParamName, IndexTypeRef>,
    nat_sub: &mut HashMap<GenericParamName, u64>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), GraphcalError> {
    use crate::tir::dim_check::InferredType;

    match resolved {
        ResolvedTypeExpr::Indexed { base, indexes } => {
            // Peel off index layers from actual type, binding index generics.
            // Iterate forward: first index in the list is the outermost Indexed layer.
            let mut current = actual;
            for idx in indexes {
                let InferredType::Indexed {
                    element,
                    index: actual_idx,
                } = current
                else {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "indexed type".to_string(),
                        found: crate::tir::dim_check::format_inferred_type(current, registry),
                        help: "expected an indexed value".to_string(),
                        src: src.clone(),
                        span: span.into(),
                    });
                };
                match idx {
                    ResolvedIndex::GenericParam(gp, _) => {
                        bind_or_check(
                            index_sub,
                            gp.clone(),
                            actual_idx.type_ref().clone(),
                            |prev, _| GraphcalError::IndexMismatch {
                                expected: prev.display_name(),
                                found: actual_idx.name(),
                                src: src.clone(),
                                span: span.into(),
                            },
                        )?;
                    }
                    ResolvedIndex::Concrete(name, _) => {
                        if !actual_idx.matches_resolved(name) {
                            return Err(GraphcalError::IndexMismatch {
                                expected: name.to_unowned_def_name(),
                                found: actual_idx.name(),
                                src: src.clone(),
                                span: span.into(),
                            });
                        }
                    }
                    ResolvedIndex::NatExpr(form, _) => {
                        // Extract the concrete nat value from the typed actual Nat-range identity.
                        let actual_nat = actual_idx
                            .nat_range_form()
                            .filter(NatPolyForm::is_constant)
                            .map(|actual_form| actual_form.constant())
                            .ok_or_else(|| GraphcalError::IndexMismatch {
                                expected: IndexName::expect_valid(format!(
                                    "range({})",
                                    form.format()
                                )),
                                found: actual_idx.name(),
                                src: src.clone(),
                                span: span.into(),
                            })?;
                        // Solve the polynomial equation: form = actual_nat
                        let actual_idx_name = actual_idx.name();
                        unify_nat_poly_form(
                            form,
                            actual_nat,
                            nat_sub,
                            &actual_idx_name,
                            src,
                            span,
                        )?;
                    }
                }
                current = element;
            }
            unify_resolved_type(
                base, current, dim_sub, index_sub, nat_sub, registry, src, span,
            )
        }

        ResolvedTypeExpr::Bool => {
            if *actual != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: "expected Bool argument".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Int => {
            if !actual.is_int_like() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Int".to_string(),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: "expected Int argument".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Datetime(expected_scale) => {
            if *actual != InferredType::Datetime(*expected_scale) {
                let expected_str = if expected_scale.is_utc() {
                    "Datetime".to_string()
                } else {
                    format!("Datetime<{expected_scale}>")
                };
                return Err(GraphcalError::DimensionMismatch {
                    expected: expected_str,
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: "expected Datetime argument".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::IndexArg(expected_index) => {
            let InferredType::NamedIndex(actual_index) = actual else {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format!("index {}", expected_index.format_for_diagnostic()),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: "expected an index generic argument".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            };
            if !resolved_index_matches_inferred(expected_index, actual_index) {
                return Err(GraphcalError::IndexMismatch {
                    expected: resolved_index_display_name(expected_index),
                    found: actual_index.name(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Dimensionless => {
            let actual_dim = crate::tir::dim_check::expect_scalar(actual, registry, src, span)?;
            if !actual_dim.is_dimensionless() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Dimensionless".to_string(),
                    found: registry.dimensions.format_dimension(&actual_dim),
                    help: "expected Dimensionless argument".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Scalar(expected_dim) => {
            let actual_dim = crate::tir::dim_check::expect_scalar(actual, registry, src, span)?;
            if *expected_dim != actual_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(expected_dim),
                    found: registry.dimensions.format_dimension(&actual_dim),
                    help: "dimension mismatch in function argument".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::GenericStruct {
            name, type_args, ..
        } => {
            // Unify the struct identity AND its type arguments: skipping the
            // args would let `Vec3<Length>` silently unify with `Vec3<Mass>`.
            let InferredType::Struct(actual_name, actual_args) = actual else {
                return Err(GraphcalError::DimensionMismatch {
                    expected: name.as_str().to_string(),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: format!("expected struct type `{}`", name.as_str()),
                    src: src.clone(),
                    span: span.into(),
                });
            };
            if actual_name.resolved() != name {
                return Err(GraphcalError::DimensionMismatch {
                    expected: name.as_str().to_string(),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: format!("expected struct type `{}`", name.as_str()),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            if type_args.len() != actual_args.len() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format!(
                        "{} with {} generic argument(s)",
                        name.as_str(),
                        type_args.len()
                    ),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: "generic struct argument count must match exactly".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            // Recursively unify each declared type argument against the
            // actual one now that both sides are known to carry the same
            // number of arguments.
            for (declared_arg, actual_arg) in type_args.iter().zip(actual_args) {
                unify_resolved_type(
                    declared_arg,
                    actual_arg,
                    dim_sub,
                    index_sub,
                    nat_sub,
                    registry,
                    src,
                    span,
                )?;
            }
            Ok(())
        }

        ResolvedTypeExpr::Struct(name, _) => {
            // When both sides carry canonical struct identities, compare
            // owners as well.
            let InferredType::Struct(actual_name, _) = actual else {
                return Err(GraphcalError::DimensionMismatch {
                    expected: name.as_str().to_string(),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: format!("expected struct type `{}`", name.as_str()),
                    src: src.clone(),
                    span: span.into(),
                });
            };
            if actual_name.resolved() != name {
                return Err(GraphcalError::DimensionMismatch {
                    expected: name.as_str().to_string(),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: format!("expected struct type `{}`", name.as_str()),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::GenericDimParam(gp, _) => {
            let actual_dim = crate::tir::dim_check::expect_scalar(actual, registry, src, span)?;
            bind_or_check(dim_sub, gp.clone(), actual_dim, |prev, new| {
                GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(prev),
                    found: registry.dimensions.format_dimension(new),
                    help: format!(
                        "generic `{gp}` was bound to {} but this argument requires {}",
                        registry.dimensions.format_dimension(prev),
                        registry.dimensions.format_dimension(new),
                    ),
                    src: src.clone(),
                    span: span.into(),
                }
            })
        }

        ResolvedTypeExpr::GenericTypeParam(gp, gp_span) => Err(GraphcalError::EvalError {
            message: format!(
                "cannot infer unconstrained generic type parameter `{gp}` in this position yet"
            ),
            src: src.clone(),
            span: (*gp_span).into(),
        }),

        ResolvedTypeExpr::GenericDimExpr { terms, .. } => {
            let actual_dim = crate::tir::dim_check::expect_scalar(actual, registry, src, span)?;

            // Single generic term with power: D^n means D = actual^(1/n)
            if terms.len() == 1
                && let ResolvedDimTerm::GenericParam {
                    name: gp,
                    power,
                    op: MulDivOp::Mul,
                    ..
                } = &terms[0]
            {
                let bound_dim = if *power == Rational::ONE {
                    actual_dim
                } else {
                    // D^(p/q) bound against `actual` means D = actual^(q/p).
                    let exponent = Rational::try_new(power.den(), power.num()).map_err(|_| {
                        GraphcalError::InternalError {
                            message: format!("generic dimension parameter `{gp}` has zero power"),
                            src: src.clone(),
                            span: span.into(),
                        }
                    })?;
                    actual_dim
                        .pow(exponent)
                        .map_err(|_| GraphcalError::DimensionOverflow {
                            src: src.clone(),
                            span: span.into(),
                        })?
                };
                bind_or_check(dim_sub, gp.clone(), bound_dim, |prev, new| {
                    GraphcalError::DimensionMismatch {
                        expected: registry.dimensions.format_dimension(prev),
                        found: registry.dimensions.format_dimension(new),
                        help: format!(
                            "generic `{gp}` was bound to {} but this argument requires {}",
                            registry.dimensions.format_dimension(prev),
                            registry.dimensions.format_dimension(new),
                        ),
                        src: src.clone(),
                        span: span.into(),
                    }
                })?;
                return Ok(());
            }

            // General case: compute expected dimension from already-bound generics + concrete terms
            let mut expected_dim = Dimension::dimensionless();
            for term in terms {
                let overflow_err = || GraphcalError::DimensionOverflow {
                    src: src.clone(),
                    span: span.into(),
                };
                let term_dim = match term {
                    ResolvedDimTerm::Concrete { dim, power, .. } => {
                        dim.pow(*power).map_err(|_| overflow_err())?
                    }
                    ResolvedDimTerm::GenericParam {
                        name: gp, power, ..
                    } => {
                        if let Some(prev) = dim_sub.get(gp) {
                            prev.pow(*power).map_err(|_| overflow_err())?
                        } else {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format!("generic `{gp}` (unresolved)"),
                                found: registry.dimensions.format_dimension(&actual_dim),
                                help: format!(
                                    "generic `{gp}` could not be inferred from this argument"
                                ),
                                src: src.clone(),
                                span: span.into(),
                            });
                        }
                    }
                };
                expected_dim = match term.op() {
                    MulDivOp::Mul => (expected_dim * term_dim).map_err(|_| overflow_err())?,
                    MulDivOp::Div => (expected_dim / term_dim).map_err(|_| overflow_err())?,
                };
            }

            if expected_dim != actual_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&expected_dim),
                    found: registry.dimensions.format_dimension(&actual_dim),
                    help: "dimension mismatch in function argument".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Substitution
// ---------------------------------------------------------------------------

/// Substitute generic parameters in a resolved type, producing an `InferredType`.
///
/// This replaces `resolve_type_with_substitution()` from `dim_check.rs`.
#[expect(
    clippy::implicit_hasher,
    reason = "always called with standard HashMap"
)]
pub fn substitute_resolved_type(
    resolved: &ResolvedTypeExpr,
    dim_sub: &HashMap<GenericParamName, Dimension>,
    index_sub: &HashMap<GenericParamName, IndexTypeRef>,
    nat_sub: &HashMap<GenericParamName, u64>,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::tir::dim_check::InferredType, GraphcalError> {
    let no_type_sub = HashMap::new();
    substitute_resolved_type_with_types(resolved, dim_sub, index_sub, nat_sub, &no_type_sub, src)
}

/// Like [`substitute_resolved_type`], but with generic *type* parameters.
///
/// Unconstrained generic type parameters are substituted from `type_sub`
/// (used by HIR constructor-call inference, which binds them from
/// call-site arguments).
#[expect(
    clippy::implicit_hasher,
    reason = "always called with standard HashMap"
)]
#[expect(
    clippy::too_many_lines,
    reason = "single dispatch over ResolvedTypeExpr variants with per-variant generic-substitution + dimension-arithmetic overflow handling"
)]
pub fn substitute_resolved_type_with_types(
    resolved: &ResolvedTypeExpr,
    dim_sub: &HashMap<GenericParamName, Dimension>,
    index_sub: &HashMap<GenericParamName, IndexTypeRef>,
    nat_sub: &HashMap<GenericParamName, u64>,
    type_sub: &HashMap<GenericParamName, crate::tir::dim_check::InferredType>,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::tir::dim_check::InferredType, GraphcalError> {
    use crate::tir::dim_check::InferredType;

    match resolved {
        ResolvedTypeExpr::Dimensionless => Ok(InferredType::Scalar(Dimension::dimensionless())),
        ResolvedTypeExpr::Bool => Ok(InferredType::Bool),
        ResolvedTypeExpr::Int => Ok(InferredType::Int),
        ResolvedTypeExpr::Datetime(scale) => Ok(InferredType::Datetime(*scale)),
        ResolvedTypeExpr::IndexArg(index) => {
            resolved_index_to_inferred(index, src).map(InferredType::NamedIndex)
        }
        ResolvedTypeExpr::Scalar(dim) => Ok(InferredType::Scalar(dim.clone())),
        ResolvedTypeExpr::Struct(name, _) => Ok(InferredType::Struct(
            crate::tir::dim_check::InferredStructType::from_resolved(name.clone()),
            vec![],
        )),
        ResolvedTypeExpr::GenericStruct {
            name, type_args, ..
        } => {
            let mut inferred_args = Vec::with_capacity(type_args.len());
            for arg in type_args {
                inferred_args.push(substitute_resolved_type_with_types(
                    arg, dim_sub, index_sub, nat_sub, type_sub, src,
                )?);
            }
            Ok(InferredType::Struct(
                crate::tir::dim_check::InferredStructType::from_resolved(name.clone()),
                inferred_args,
            ))
        }

        ResolvedTypeExpr::GenericDimParam(gp, span) => dim_sub.get(gp).map_or_else(
            || {
                Err(GraphcalError::EvalError {
                    message: format!("generic `{gp}` not bound during substitution"),
                    src: src.clone(),
                    span: (*span).into(),
                })
            },
            |dim| Ok(InferredType::Scalar(dim.clone())),
        ),

        ResolvedTypeExpr::GenericTypeParam(gp, span) => type_sub.get(gp).map_or_else(
            || {
                Err(GraphcalError::EvalError {
                    message: format!("generic type parameter `{gp}` not bound during substitution"),
                    src: src.clone(),
                    span: (*span).into(),
                })
            },
            |ty| Ok(ty.clone()),
        ),

        ResolvedTypeExpr::GenericDimExpr { terms, span } => {
            let overflow_err = || GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: (*span).into(),
            };
            let mut result = Dimension::dimensionless();
            for term in terms {
                let term_dim = match term {
                    ResolvedDimTerm::Concrete { dim, power, .. } => {
                        dim.pow(*power).map_err(|_| overflow_err())?
                    }
                    ResolvedDimTerm::GenericParam {
                        name: gp,
                        power,
                        span: term_span,
                        ..
                    } => {
                        let base = dim_sub.get(gp).ok_or_else(|| GraphcalError::EvalError {
                            message: format!("generic `{gp}` not bound during substitution"),
                            src: src.clone(),
                            span: (*term_span).into(),
                        })?;
                        base.pow(*power).map_err(|_| overflow_err())?
                    }
                };
                result = match term.op() {
                    MulDivOp::Mul => (result * term_dim).map_err(|_| overflow_err())?,
                    MulDivOp::Div => (result / term_dim).map_err(|_| overflow_err())?,
                };
            }
            Ok(InferredType::Scalar(result))
        }

        ResolvedTypeExpr::Indexed { base, indexes } => {
            let mut result = substitute_resolved_type_with_types(
                base, dim_sub, index_sub, nat_sub, type_sub, src,
            )?;
            for idx in indexes.iter().rev() {
                let resolved_idx = match idx {
                    ResolvedIndex::Concrete(name, _) => {
                        result = InferredType::Indexed {
                            element: Box::new(result),
                            index: crate::tir::dim_check::InferredIndex::from_resolved(
                                name.clone(),
                            ),
                        };
                        continue;
                    }
                    ResolvedIndex::GenericParam(gp, span) => {
                        crate::tir::dim_check::InferredIndex::from_ref(
                            index_sub
                                .get(gp)
                                .cloned()
                                .ok_or_else(|| GraphcalError::EvalError {
                                    message: format!(
                                        "generic index `{gp}` not bound during substitution"
                                    ),
                                    src: src.clone(),
                                    span: (*span).into(),
                                })?,
                        )
                    }
                    ResolvedIndex::NatExpr(form, span) => {
                        let n = form.evaluate(nat_sub).ok_or_else(|| {
                            let vars = form.variables();
                            let unbound: Vec<&str> = vars
                                .iter()
                                .filter(|k| !nat_sub.contains_key(*k))
                                .map(GenericParamName::as_str)
                                .collect();
                            GraphcalError::EvalError {
                                message: format!(
                                    "generic nat parameter(s) [{}] not bound during substitution",
                                    unbound.join(", ")
                                ),
                                src: src.clone(),
                                span: (*span).into(),
                            }
                        })?;
                        crate::tir::dim_check::InferredIndex::from_nat_range_form(
                            NatPolyForm::from_constant(n),
                        )
                        .map_err(|err| GraphcalError::EvalError {
                            message: err.to_string(),
                            src: src.clone(),
                            span: (*span).into(),
                        })?
                    }
                };
                result = InferredType::Indexed {
                    element: Box::new(result),
                    index: resolved_idx,
                };
            }
            Ok(result)
        }
    }
}

// ---------------------------------------------------------------------------
