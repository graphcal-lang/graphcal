use std::sync::Arc;

use miette::NamedSource;

use crate::dimension::Dimension;
use crate::hir::NominalTypeDef;
use crate::registry::error::GraphcalError;
use crate::registry::types::FormattingRegistry;

use super::{DeclaredType, InferredGenericArg, InferredIndex, InferredStructType, InferredType};
use crate::registry::declared_type::DeclaredGenericArg;
use crate::tir::typed::{ResolvedDimArg, ResolvedGenericArg, ResolvedIndex, ResolvedTypeExpr};

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
        (DeclaredType::Quantity(d), inferred) => inferred.quantity_dimension() == Some(d),
        (DeclaredType::Complex(d), InferredType::Complex(inferred)) => d == inferred,
        (DeclaredType::Bool, InferredType::Bool) | (DeclaredType::Int, InferredType::Int) => true,
        (DeclaredType::Datetime(d), InferredType::Datetime(i)) => d == i,
        (DeclaredType::IndexArg(d), InferredType::IndexArg(i))
        | (DeclaredType::Key(d), InferredType::Key(i)) => i.matches_ref(d),
        (DeclaredType::Struct(d, d_args), InferredType::Struct(i, i_args)) => {
            i.matches_ref(d)
                && d_args.len() == i_args.len()
                && d_args
                    .iter()
                    .zip(i_args)
                    .all(|(declared, inferred)| declared_generic_arg_matches(declared, inferred))
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

fn declared_generic_arg_matches(
    declared: &DeclaredGenericArg,
    inferred: &InferredGenericArg,
) -> bool {
    match (declared, inferred) {
        (DeclaredGenericArg::Dim(declared), InferredGenericArg::Dim(inferred)) => {
            declared == inferred
        }
        (DeclaredGenericArg::Index(declared), InferredGenericArg::Index(inferred)) => {
            inferred.matches_ref(declared)
        }
        (DeclaredGenericArg::Nat(declared), InferredGenericArg::Nat(inferred)) => {
            declared == inferred
        }
        (DeclaredGenericArg::Type(declared), InferredGenericArg::Type(inferred)) => {
            types_match(declared, inferred)
        }
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
            .quantity_dimension()
            .is_some_and(Dimension::is_dimensionless),
        (ResolvedTypeExpr::Bool, InferredType::Bool)
        | (ResolvedTypeExpr::Int, InferredType::Int) => true,
        (ResolvedTypeExpr::Datetime(expected), InferredType::Datetime(actual)) => {
            expected == actual
        }
        (ResolvedTypeExpr::Quantity(expected), inferred) => {
            inferred.quantity_dimension() == Some(expected)
        }
        (
            ResolvedTypeExpr::Complex {
                dimension: expected,
                ..
            },
            InferredType::Complex(actual),
        ) => match expected {
            ResolvedDimArg::Dimensionless => actual.is_dimensionless(),
            ResolvedDimArg::Concrete(expected) => expected == actual,
            ResolvedDimArg::GenericParam(_, _) | ResolvedDimArg::Expr { .. } => false,
        },
        (ResolvedTypeExpr::IndexArg(expected), InferredType::IndexArg(actual)) => {
            resolved_index_matches_inferred(expected, actual)
        }
        (ResolvedTypeExpr::Key { index, .. }, InferredType::Key(actual)) => {
            resolved_index_matches_inferred(index, actual)
        }
        (ResolvedTypeExpr::Struct(expected, _), InferredType::Struct(actual, args)) => {
            actual.matches_resolved(expected) && args.is_empty()
        }
        (
            ResolvedTypeExpr::GenericStruct {
                name, generic_args, ..
            },
            InferredType::Struct(actual, actual_args),
        ) => {
            actual.matches_resolved(name)
                && generic_args.len() == actual_args.len()
                && generic_args
                    .iter()
                    .zip(actual_args)
                    .all(|(expected, actual)| resolved_generic_arg_matches(expected, actual))
        }
        (ResolvedTypeExpr::Indexed { base, indexes }, _) => {
            resolved_indexed_type_matches_inferred(base, indexes, inferred)
        }
        _ => false,
    }
}

fn resolved_generic_arg_matches(
    resolved: &ResolvedGenericArg,
    inferred: &InferredGenericArg,
) -> bool {
    match (resolved, inferred) {
        (ResolvedGenericArg::Dim(resolved), InferredGenericArg::Dim(inferred)) => match resolved {
            ResolvedDimArg::Dimensionless => inferred.is_dimensionless(),
            ResolvedDimArg::Concrete(dimension) => dimension == inferred,
            ResolvedDimArg::GenericParam(_, _) | ResolvedDimArg::Expr { .. } => false,
        },
        (ResolvedGenericArg::Index(resolved), InferredGenericArg::Index(inferred)) => {
            resolved_index_matches_inferred(resolved, inferred)
        }
        (ResolvedGenericArg::Nat(resolved, _), InferredGenericArg::Nat(inferred)) => {
            resolved == inferred
        }
        (ResolvedGenericArg::Type(resolved), InferredGenericArg::Type(inferred)) => {
            resolved_type_matches_inferred(resolved, inferred)
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
        ResolvedIndex::Finite(form, _) => actual
            .finite_index_form()
            .is_some_and(|actual_form| actual_form == *form),
        // An unbound generic index parameter never reaches this comparison:
        // DAG declaration types and inline-DAG param types resolve with no
        // generic params in scope, and HIR inference only constructs
        // `InferredIndex` from concrete (resolved or finite-index) identities —
        // the syntax engine's leaf-name fallback that could fabricate a
        // generic-named index is gone (#765). No display-name comparison can
        // therefore be meaningful here.
        ResolvedIndex::GenericParam(_, _) => false,
    }
}

/// Format a declared type for display in diagnostics.
pub(super) fn format_declared_type(dt: &DeclaredType, registry: &FormattingRegistry) -> String {
    dt.format(&registry.dimensions)
}

/// Look up the definition for an inferred struct identity.
///
/// This lookup is canonical-owner based only. Falling back from a resolved
/// identity to a bare leaf would make diamond imports with same-named types
/// nondeterministic.
pub(super) fn struct_type_def_for_inferred<'a>(
    ty: &InferredStructType,
    dag: Option<&'a crate::tir::typed::DagTIR>,
    _registry: &'a FormattingRegistry,
) -> Option<&'a NominalTypeDef> {
    dag.map(|dag| &dag.semantic.type_defs)
        .and_then(|defs| defs.struct_types.get(ty.resolved()))
        .map(AsRef::as_ref)
}

/// Format an inferred type for display in diagnostics.
#[must_use]
pub fn format_inferred_type(it: &InferredType, registry: &FormattingRegistry) -> String {
    DeclaredType::from(it).format(&registry.dimensions)
}

/// Format unequal inferred types without emitting a self-contradictory
/// leaf-only diagnostic such as `expected Foo, found Foo`.
pub(super) fn format_distinct_inferred_types(
    expected: &InferredType,
    found: &InferredType,
    registry: &FormattingRegistry,
) -> (String, String) {
    let expected_type = DeclaredType::from(expected);
    let found_type = DeclaredType::from(found);
    let expected_display = expected_type.format(&registry.dimensions);
    let found_display = found_type.format(&registry.dimensions);
    if expected_display != found_display {
        return (expected_display, found_display);
    }
    (
        expected_type.format_owner_qualified(&registry.dimensions),
        found_type.format_owner_qualified(&registry.dimensions),
    )
}

impl From<&InferredType> for DeclaredType {
    fn from(it: &InferredType) -> Self {
        match it {
            InferredType::Quantity(d) => Self::Quantity(d.clone()),
            InferredType::Complex(d) => Self::Complex(d.clone()),
            InferredType::Bool => Self::Bool,
            InferredType::Int => Self::Int,
            InferredType::Datetime(scale) => Self::Datetime(*scale),
            InferredType::IndexArg(index) => Self::IndexArg(index.type_ref().clone()),
            InferredType::Key(index) => Self::Key(index.type_ref().clone()),
            InferredType::Struct(n, args) => Self::Struct(
                n.type_ref().clone(),
                args.iter().map(DeclaredGenericArg::from).collect(),
            ),
            InferredType::Indexed { element, index } => Self::Indexed {
                element: Box::new(Self::from(element.as_ref())),
                index: index.type_ref().clone(),
            },
        }
    }
}

impl From<&InferredGenericArg> for DeclaredGenericArg {
    fn from(arg: &InferredGenericArg) -> Self {
        match arg {
            InferredGenericArg::Dim(dimension) => Self::Dim(dimension.clone()),
            InferredGenericArg::Index(index) => Self::Index(index.type_ref().clone()),
            InferredGenericArg::Nat(form) => Self::Nat(form.clone()),
            InferredGenericArg::Type(type_expr) => Self::Type(DeclaredType::from(type_expr)),
        }
    }
}

impl From<&DeclaredGenericArg> for InferredGenericArg {
    fn from(arg: &DeclaredGenericArg) -> Self {
        match arg {
            DeclaredGenericArg::Dim(dimension) => Self::Dim(dimension.clone()),
            DeclaredGenericArg::Index(index) => Self::Index(InferredIndex::from_ref(index.clone())),
            DeclaredGenericArg::Nat(form) => Self::Nat(form.clone()),
            DeclaredGenericArg::Type(type_expr) => Self::Type(InferredType::from(type_expr)),
        }
    }
}

impl From<&DeclaredType> for InferredType {
    fn from(dt: &DeclaredType) -> Self {
        match dt {
            DeclaredType::Quantity(d) => Self::Quantity(d.clone()),
            DeclaredType::Complex(d) => Self::Complex(d.clone()),
            DeclaredType::Bool => Self::Bool,
            DeclaredType::Int => Self::Int,
            DeclaredType::Datetime(scale) => Self::Datetime(*scale),
            DeclaredType::IndexArg(index) => Self::IndexArg(InferredIndex::from_ref(index.clone())),
            DeclaredType::Key(index) => Self::Key(InferredIndex::from_ref(index.clone())),
            DeclaredType::Struct(n, args) => Self::Struct(
                InferredStructType::from_ref(n.clone()),
                args.iter().map(InferredGenericArg::from).collect(),
            ),
            DeclaredType::Indexed { element, index } => Self::Indexed {
                element: Box::new(Self::from(element.as_ref())),
                index: InferredIndex::from_ref(index.clone()),
            },
        }
    }
}

pub fn expect_quantity(
    inferred: &InferredType,
    registry: &FormattingRegistry,
    src: &NamedSource<Arc<String>>,
    span: crate::syntax::span::Span,
) -> Result<Dimension, GraphcalError> {
    let found_kind = match inferred {
        InferredType::Quantity(d) => return Ok(d.clone()),
        InferredType::Complex(_) => "a Complex value",
        InferredType::Bool => "a Bool value",
        InferredType::Int => "an Int value",
        InferredType::Datetime(_) => "a Datetime value",
        InferredType::Key(_) => "an index-key value",
        InferredType::IndexArg(_) => "an Index argument",
        InferredType::Struct(..) => "a struct",
        InferredType::Indexed { .. } => "an indexed value",
    };
    Err(GraphcalError::DimensionMismatch {
        expected: "quantity type".to_string(),
        found: format_inferred_type(inferred, registry),
        help: format!("expected a quantity value, not {found_kind}"),
        src: src.clone(),
        span: span.into(),
    })
}
