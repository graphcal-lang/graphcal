use std::sync::Arc;

use miette::NamedSource;

use crate::dimension::Dimension;
use crate::registry::error::GraphcalError;
use crate::registry::types::{Registry, TypeDef};

use super::{DeclaredType, InferredIndex, InferredStructType, InferredType};
use crate::tir::typed::{ResolvedIndex, ResolvedTypeExpr};

pub(super) fn is_bool_type(ty: &InferredType) -> bool {
    match ty {
        InferredType::Bool => true,
        InferredType::Indexed { element, .. } => is_bool_type(element),
        _ => false,
    }
}

/// Check if a declared type matches an inferred type.
///
/// Under the n-variant-union model, the inferred type of a constructor
/// expression is *already* the owning union — there is no per-variant
/// type and therefore no widening/subtyping at the type level. Struct
/// equality is by name and type-argument list only.
pub(super) fn types_match(declared: &DeclaredType, inferred: &InferredType) -> bool {
    match (declared, inferred) {
        (DeclaredType::Scalar(d), inferred) => inferred.scalar_dimension() == Some(d),
        (DeclaredType::Bool, InferredType::Bool) => true,
        (DeclaredType::Int, inferred) if inferred.is_int_like() => true,
        (DeclaredType::Datetime(d), InferredType::Datetime(i)) => d == i,
        (DeclaredType::IndexArg(d), InferredType::NamedIndex(i)) => i.matches_ref(d),
        (DeclaredType::Struct(d, d_args), InferredType::Struct(i, i_args)) => {
            i.matches_ref(d)
                && d_args.len() == i_args.len()
                && d_args
                    .iter()
                    .zip(i_args)
                    .all(|(da, ia)| types_match(da, ia))
        }
        (
            DeclaredType::Indexed {
                element: d_elem,
                index: d_idx,
            },
            InferredType::Indexed {
                element: i_elem,
                index: i_idx,
            },
        ) => i_idx.matches_ref(d_idx) && types_match(d_elem, i_elem),
        _ => false,
    }
}

/// Check if a resolved declaration type matches an inferred expression type,
/// preserving canonical index identity when both sides carry it.
pub(super) fn resolved_type_matches_inferred(
    resolved: &ResolvedTypeExpr,
    inferred: &InferredType,
) -> bool {
    match (resolved, inferred) {
        (ResolvedTypeExpr::Dimensionless, inferred) => inferred
            .scalar_dimension()
            .is_some_and(Dimension::is_dimensionless),
        (ResolvedTypeExpr::Bool, InferredType::Bool) => true,
        (ResolvedTypeExpr::Int, inferred) => inferred.is_int_like(),
        (ResolvedTypeExpr::Datetime(expected), InferredType::Datetime(actual)) => {
            expected == actual
        }
        (ResolvedTypeExpr::Scalar(expected), inferred) => {
            inferred.scalar_dimension() == Some(expected)
        }
        (ResolvedTypeExpr::IndexArg(expected), InferredType::NamedIndex(actual)) => {
            resolved_index_matches_inferred(expected, actual)
        }
        (ResolvedTypeExpr::Struct(expected, _), InferredType::Struct(actual, args)) => {
            actual.matches_resolved(expected) && args.is_empty()
        }
        (
            ResolvedTypeExpr::GenericStruct {
                name, type_args, ..
            },
            InferredType::Struct(actual, actual_args),
        ) => {
            actual.matches_resolved(name)
                && type_args.len() == actual_args.len()
                && type_args
                    .iter()
                    .zip(actual_args)
                    .all(|(expected, actual)| resolved_type_matches_inferred(expected, actual))
        }
        (ResolvedTypeExpr::Indexed { base, indexes }, _) => {
            resolved_indexed_type_matches_inferred(base, indexes, inferred)
        }
        _ => false,
    }
}

fn resolved_indexed_type_matches_inferred(
    base: &ResolvedTypeExpr,
    indexes: &[ResolvedIndex],
    inferred: &InferredType,
) -> bool {
    let mut current = inferred;
    for index in indexes {
        let InferredType::Indexed {
            element,
            index: actual,
        } = current
        else {
            return false;
        };
        if !resolved_index_matches_inferred(index, actual) {
            return false;
        }
        current = element;
    }
    resolved_type_matches_inferred(base, current)
}

fn resolved_index_matches_inferred(index: &ResolvedIndex, actual: &InferredIndex) -> bool {
    match index {
        ResolvedIndex::Concrete(expected, _) => actual.matches_resolved(expected),
        ResolvedIndex::NatExpr(form, _) => actual
            .nat_range_form()
            .is_some_and(|actual_form| actual_form == *form),
        // An unbound generic index parameter never reaches this comparison:
        // DAG declaration types and inline-DAG param types resolve with no
        // generic params in scope, and HIR inference only constructs
        // `InferredIndex` from concrete (resolved or Nat-range) identities —
        // the syntax engine's leaf-name fallback that could fabricate a
        // generic-named index is gone (#765). No display-name comparison can
        // therefore be meaningful here.
        ResolvedIndex::GenericParam(_, _) => false,
    }
}

/// Format a declared type for display in diagnostics.
pub(super) fn format_declared_type(dt: &DeclaredType, registry: &Registry) -> String {
    dt.format(&registry.dimensions)
}

/// Look up the definition for an inferred struct identity.
///
/// Prefer canonical semantic TIR type definitions, then consult the leaf-keyed
/// registry for boundary-created synthetic owners.
pub(super) fn struct_type_def_for_inferred<'a>(
    ty: &InferredStructType,
    dag: Option<&'a crate::tir::typed::DagTIR>,
    registry: &'a Registry,
) -> Option<&'a TypeDef> {
    dag.map(|dag| &dag.semantic.type_defs)
        .and_then(|defs| defs.struct_types.get(ty.resolved()))
        .or_else(|| registry.types.get_type(ty.name().as_str()))
}

/// Format an inferred type for display in diagnostics.
#[must_use]
pub fn format_inferred_type(it: &InferredType, registry: &Registry) -> String {
    if let InferredType::Fin(bound) = it {
        return format!("Fin({})", bound.format());
    }
    DeclaredType::from(it).format(&registry.dimensions)
}

impl From<&InferredType> for DeclaredType {
    fn from(it: &InferredType) -> Self {
        match it {
            InferredType::Scalar(d) | InferredType::RangeIndexLabel { dimension: d, .. } => {
                Self::Scalar(d.clone())
            }
            InferredType::Bool => Self::Bool,
            InferredType::Int | InferredType::Fin(_) => Self::Int,
            InferredType::Datetime(scale) => Self::Datetime(*scale),
            InferredType::NamedIndex(index) => Self::IndexArg(index.type_ref().clone()),
            InferredType::Struct(n, args) => {
                Self::Struct(n.type_ref().clone(), args.iter().map(Self::from).collect())
            }
            InferredType::Indexed { element, index } => Self::Indexed {
                element: Box::new(Self::from(element.as_ref())),
                index: index.type_ref().clone(),
            },
        }
    }
}

impl From<&DeclaredType> for InferredType {
    fn from(dt: &DeclaredType) -> Self {
        match dt {
            DeclaredType::Scalar(d) => Self::Scalar(d.clone()),
            DeclaredType::Bool => Self::Bool,
            DeclaredType::Int => Self::Int,
            DeclaredType::Datetime(scale) => Self::Datetime(*scale),
            DeclaredType::IndexArg(index) => {
                Self::NamedIndex(InferredIndex::from_ref(index.clone()))
            }
            DeclaredType::Struct(n, args) => Self::Struct(
                InferredStructType::from_ref(n.clone()),
                args.iter().map(Self::from).collect(),
            ),
            DeclaredType::Indexed { element, index } => Self::Indexed {
                element: Box::new(Self::from(element.as_ref())),
                index: InferredIndex::from_ref(index.clone()),
            },
        }
    }
}

pub fn expect_scalar(
    inferred: &InferredType,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    span: crate::syntax::span::Span,
) -> Result<Dimension, GraphcalError> {
    let found_kind = match inferred {
        InferredType::Scalar(d) | InferredType::RangeIndexLabel { dimension: d, .. } => {
            return Ok(d.clone());
        }
        InferredType::Bool => "a Bool value",
        InferredType::Int | InferredType::Fin(_) => "an Int value",
        InferredType::Datetime(_) => "a Datetime value",
        InferredType::NamedIndex(_) => "a named-index loop variable",
        InferredType::Struct(..) => "a struct",
        InferredType::Indexed { .. } => "an indexed value",
    };
    Err(GraphcalError::DimensionMismatch {
        expected: "scalar dimension".to_string(),
        found: format_inferred_type(inferred, registry),
        help: format!("expected a scalar value, not {found_kind}"),
        src: src.clone(),
        span: span.into(),
    })
}

/// Build the Cartesian product of variant-key slices across multiple axes.
pub(super) fn cartesian_product<T: Clone + Eq + std::hash::Hash>(
    axes: &[Vec<T>],
    current: &mut Vec<T>,
    result: &mut std::collections::HashSet<Vec<T>>,
) {
    if current.len() == axes.len() {
        result.insert(current.clone());
        return;
    }
    let axis_idx = current.len();
    for variant in &axes[axis_idx] {
        current.push(variant.clone());
        cartesian_product(axes, current, result);
        current.pop();
    }
}
