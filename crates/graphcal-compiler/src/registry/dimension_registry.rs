use std::collections::{BTreeMap, HashMap};

use thiserror::Error;

use crate::desugar::desugared_ast::{DimExpr, MulDivOp, TypeExpr, TypeExprKind};
use crate::dimension::{BaseDimId, Dimension, MissingBaseDimensionName, Rational, RationalError};
use crate::syntax::dimension::DimName;

/// Shared implementation for resolving a `DimExpr` to a concrete `Dimension`.
pub(crate) fn resolve_dim_expr_impl(
    dimensions: &HashMap<DimName, Dimension>,
    expr: &DimExpr,
) -> Result<Option<Dimension>, RationalError> {
    expr.terms
        .iter()
        .try_fold(Some(Dimension::dimensionless()), |acc, item| {
            let Some(acc) = acc else {
                return Ok(None);
            };
            let Some(atom) = item.term.name.value.as_bare() else {
                return Ok(None);
            };
            let Some(base) = dimensions.get(atom.as_str()) else {
                return Ok(None);
            };
            let exp = item.term.power.unwrap_or(Rational::ONE);
            let powered = base.pow(exp)?;
            match item.op {
                MulDivOp::Mul => acc * powered,
                MulDivOp::Div => acc / powered,
            }
            .map(Some)
        })
}

/// Shared implementation for resolving a `TypeExpr` to a concrete `Dimension`.
pub(crate) fn resolve_type_expr_impl(
    dimensions: &HashMap<DimName, Dimension>,
    type_expr: &TypeExpr,
) -> Result<Option<Dimension>, RationalError> {
    match &type_expr.kind {
        TypeExprKind::Dimensionless => Ok(Some(Dimension::dimensionless())),
        TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime
        | TypeExprKind::TypeApplication { .. }
        | TypeExprKind::DatetimeApplication { .. } => Ok(None),
        TypeExprKind::DimExpr(dim_expr) => resolve_dim_expr_impl(dimensions, dim_expr),
        TypeExprKind::Indexed { base, .. } => resolve_type_expr_impl(dimensions, base),
    }
}

/// Format a dimension, preferring a registered named alias for compound forms.
///
/// A pure base dimension (`Length`) or `Dimensionless` keeps its canonical
/// rendering. A compound dimension (`Length^2 * Mass / Time^2`) is replaced by
/// a matching named dimension (`Energy`) when one is registered; if several
/// names match, the lexicographically smallest is chosen for determinism.
fn format_dimension_preferring_alias(
    dimensions: &HashMap<DimName, Dimension>,
    base_dim_names: &BTreeMap<BaseDimId, String>,
    dim: &Dimension,
) -> Result<String, MissingBaseDimensionName> {
    let canonical = dim.try_format_with(base_dim_names)?;
    // Base dimensions and Dimensionless render as a single bare name already;
    // only compound renderings benefit from an alias.
    let is_compound = canonical.contains([' ', '^', '*', '/']);
    if is_compound
        && let Some(alias) = dimensions
            .iter()
            .filter(|(_, d)| *d == dim)
            .map(|(name, _)| name)
            .min()
    {
        return Ok(alias.to_string());
    }
    Ok(canonical)
}

#[expect(
    clippy::unreachable,
    reason = "RegistryBuilder::try_build validates base-dimension display metadata before Registry construction"
)]
pub(crate) fn format_dimension_preferring_alias_after_validation(
    dimensions: &HashMap<DimName, Dimension>,
    base_dim_names: &BTreeMap<BaseDimId, String>,
    dim: &Dimension,
) -> String {
    match format_dimension_preferring_alias(dimensions, base_dim_names, dim) {
        Ok(formatted) => formatted,
        Err(err) => unreachable!("validated registry lost base dimension display metadata: {err}"),
    }
}

pub(crate) fn assert_base_dim_names_cover(
    base_dim_names: &BTreeMap<BaseDimId, String>,
    dim: &Dimension,
    context: impl Into<String>,
) -> Result<(), RegistryBuildError> {
    let context = context.into();
    for (id, _) in dim.iter() {
        if !base_dim_names.contains_key(id) {
            return Err(RegistryBuildError::MissingBaseDimensionName {
                context,
                id: id.clone(),
            });
        }
    }
    Ok(())
}

/// Error returned when freezing a [`crate::registry::types::RegistryBuilder`] would violate registry invariants.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistryBuildError {
    /// A dimension or unit references a base dimension whose display name was
    /// not registered.
    #[error(
        "registry invariant violation: {context} references base dimension {id:?} without a registered display name"
    )]
    MissingBaseDimensionName { context: String, id: BaseDimId },
}

/// Dimension registry: maps dimension names to `Dimension` values and tracks
/// base dimension metadata (ID assignment, names, default unit symbols).
#[derive(Debug, Clone)]
pub struct DimensionRegistry {
    /// Base dimension ID → dimension name (for display).
    pub(crate) base_dim_names: BTreeMap<BaseDimId, String>,
    /// Base dimension ID → default unit symbol for runtime display.
    pub(crate) base_dim_symbols: BTreeMap<BaseDimId, String>,
    pub(crate) dimensions: HashMap<DimName, Dimension>,
}

impl DimensionRegistry {
    /// Look up a dimension by name.
    #[must_use]
    pub fn get_dimension(&self, name: &str) -> Option<&Dimension> {
        self.dimensions.get(name)
    }

    /// Iterate over all named dimensions.
    pub fn all_dimensions(&self) -> impl Iterator<Item = (&DimName, &Dimension)> {
        self.dimensions.iter()
    }

    /// Get the base dimension names map (for display purposes).
    #[must_use]
    pub const fn base_dim_names(&self) -> &BTreeMap<BaseDimId, String> {
        &self.base_dim_names
    }

    /// Get the base dimension symbols map for runtime display.
    #[must_use]
    pub const fn base_dim_symbols(&self) -> &BTreeMap<BaseDimId, String> {
        &self.base_dim_symbols
    }

    /// Format a dimension as a human-readable string using registered base dimension names.
    ///
    /// Returns `"Dimensionless"` for dimensionless, or names like `"Length / Time"`.
    /// When a compound dimension matches a named dimension alias (e.g. `Energy`
    /// for `Length^2 * Mass / Time^2`), the alias is preferred so diagnostics
    /// speak the user's vocabulary.
    #[must_use]
    pub fn format_dimension(&self, dim: &Dimension) -> String {
        format_dimension_preferring_alias_after_validation(
            &self.dimensions,
            &self.base_dim_names,
            dim,
        )
    }

    /// Resolve a `DimExpr` AST node to a concrete `Dimension`.
    ///
    /// Returns `Ok(None)` if any dimension name is unknown, and `Err` if
    /// dimension exponent arithmetic overflows `i32`.
    pub fn resolve_dim_expr(&self, expr: &DimExpr) -> Result<Option<Dimension>, RationalError> {
        resolve_dim_expr_impl(&self.dimensions, expr)
    }

    /// Resolve a `TypeExpr` to a concrete `Dimension`.
    ///
    /// Returns `Ok(None)` if the type references unknown dimensions, and
    /// `Err` if dimension exponent arithmetic overflows `i32`.
    pub fn resolve_type_expr(
        &self,
        type_expr: &TypeExpr,
    ) -> Result<Option<Dimension>, RationalError> {
        resolve_type_expr_impl(&self.dimensions, type_expr)
    }
}
