use std::collections::{BTreeMap, HashMap};

use thiserror::Error;

use crate::desugar::desugared_ast::{DimExpr, MulDivOp, TypeExpr, TypeExprKind};
use crate::dimension::{BaseDimId, Dimension, MissingBaseDimensionName, RationalError};
use crate::syntax::dimension::{DimName, DimRef};

/// Error returned when resolving a `DimExpr` to a concrete [`Dimension`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DimensionResolveError {
    /// A referenced dimension is not visible under its (possibly
    /// module-qualified) source reference.
    #[error("unknown dimension `{name}`")]
    UnknownDimension { name: DimRef },
    /// Dimension exponent arithmetic overflowed.
    #[error(transparent)]
    Overflow(#[from] RationalError),
}

/// Resolve a `DimExpr` by looking up each term's typed (possibly
/// module-qualified) reference with `lookup`.
///
/// This is the single term-folding implementation; registries supply their
/// alias-aware scope lookup, while boundary code that spans two scopes (for
/// example a dependency declaration re-read under include bindings) supplies
/// a lookup that routes each reference to its owning scope.
///
/// # Errors
///
/// Returns [`DimensionResolveError::UnknownDimension`] with the full source
/// reference when `lookup` misses, or an overflow error from exponent
/// arithmetic.
pub fn resolve_dim_expr_with<'a>(
    expr: &DimExpr,
    mut lookup: impl FnMut(&DimRef) -> Option<&'a Dimension>,
) -> Result<Dimension, DimensionResolveError> {
    expr.terms
        .iter()
        .try_fold(Dimension::dimensionless(), |acc, item| {
            let reference = DimRef::from_name_path(item.term.name.value.clone());
            let Some(base) = lookup(&reference) else {
                return Err(DimensionResolveError::UnknownDimension { name: reference });
            };
            let powered = base.pow(item.term.effective_power())?;
            match item.op {
                MulDivOp::Mul => acc * powered,
                MulDivOp::Div => acc / powered,
            }
            .map_err(DimensionResolveError::from)
        })
}

/// Borrowed view of a flat source-visible dimension scope.
///
/// Every dimension-name lookup of a registry — direct `get_dimension` calls
/// and `DimExpr` resolution alike — goes through [`Self::lookup`], so module
/// qualifiers and source-visible aliases are honoured uniformly.
#[derive(Clone, Copy)]
pub(crate) struct DimensionScope<'a> {
    dimensions: &'a HashMap<DimRef, Dimension>,
    aliases: &'a HashMap<DimRef, DimRef>,
}

impl<'a> DimensionScope<'a> {
    pub(crate) const fn new(
        dimensions: &'a HashMap<DimRef, Dimension>,
        aliases: &'a HashMap<DimRef, DimRef>,
    ) -> Self {
        Self {
            dimensions,
            aliases,
        }
    }

    /// Look up a dimension reference, following alias edges. The alias walk
    /// is bounded by the number of aliases so a cyclic chain cannot loop.
    pub(crate) fn lookup(self, reference: &DimRef) -> Option<&'a Dimension> {
        let mut current = reference;
        for _ in 0..=self.aliases.len() {
            if let Some(dimension) = self.dimensions.get(current) {
                return Some(dimension);
            }
            current = self.aliases.get(current)?;
        }
        None
    }

    /// Resolve a `DimExpr` to a concrete `Dimension`, returning `Ok(None)`
    /// when a referenced dimension is unknown.
    pub(crate) fn resolve_dim_expr(
        self,
        expr: &DimExpr,
    ) -> Result<Option<Dimension>, RationalError> {
        match self.resolve_dim_expr_detailed(expr) {
            Ok(dim) => Ok(Some(dim)),
            Err(DimensionResolveError::UnknownDimension { .. }) => Ok(None),
            Err(DimensionResolveError::Overflow(err)) => Err(err),
        }
    }

    /// Resolve a `DimExpr` while preserving the failing (qualified) reference.
    pub(crate) fn resolve_dim_expr_detailed(
        self,
        expr: &DimExpr,
    ) -> Result<Dimension, DimensionResolveError> {
        resolve_dim_expr_with(expr, |reference| self.lookup(reference))
    }

    /// Resolve a `TypeExpr` to a concrete `Dimension`.
    pub(crate) fn resolve_type_expr(
        self,
        type_expr: &TypeExpr,
    ) -> Result<Option<Dimension>, RationalError> {
        match &type_expr.kind {
            TypeExprKind::Dimensionless => Ok(Some(Dimension::dimensionless())),
            TypeExprKind::IndexLabel { .. }
            | TypeExprKind::Bool
            | TypeExprKind::Int
            | TypeExprKind::Datetime
            | TypeExprKind::TypeApplication { .. }
            | TypeExprKind::DatetimeApplication { .. }
            | TypeExprKind::ComplexApplication { .. }
            | TypeExprKind::KeyApplication { .. } => Ok(None),
            TypeExprKind::DimExpr(dim_expr) => self.resolve_dim_expr(dim_expr),
            TypeExprKind::Indexed { base, .. } => self.resolve_type_expr(base),
        }
    }
}

/// Format a dimension, preferring a registered named alias for compound forms.
///
/// A pure base dimension (`Length`) or `Dimensionless` keeps its canonical
/// rendering. A compound dimension (`Length^2 * Mass / Time^2`) is replaced by
/// a matching named dimension (`Energy`) when one is registered; if several
/// names match, the lexicographically smallest is chosen for determinism.
fn format_dimension_preferring_alias(
    dimensions: &HashMap<DimRef, Dimension>,
    base_dim_names: &BTreeMap<BaseDimId, String>,
    dim: &Dimension,
) -> Result<String, MissingBaseDimensionName> {
    let canonical = dim.try_format_with(base_dim_names)?;
    // Base dimensions and Dimensionless render as a single bare name already;
    // only compound dimensions benefit from an alias.
    if dim.is_compound()
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
    dimensions: &HashMap<DimRef, Dimension>,
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

/// Dimension formatting data retained after semantic name resolution.
///
/// Named dimensions are display aliases only: this type deliberately exposes
/// no source-name lookup or AST resolution API.
#[derive(Debug, Clone)]
pub struct DimensionFormattingRegistry {
    base_dim_names: BTreeMap<BaseDimId, String>,
    base_dim_symbols: BTreeMap<BaseDimId, String>,
    display_aliases: HashMap<DimRef, Dimension>,
}

impl DimensionFormattingRegistry {
    /// Get base dimension names used by diagnostics and display adapters.
    #[must_use]
    pub const fn base_dim_names(&self) -> &BTreeMap<BaseDimId, String> {
        &self.base_dim_names
    }

    /// Get default base-unit symbols used by runtime display adapters.
    #[must_use]
    pub const fn base_dim_symbols(&self) -> &BTreeMap<BaseDimId, String> {
        &self.base_dim_symbols
    }

    /// Format a dimension without providing any semantic name-resolution API.
    #[must_use]
    pub fn format_dimension(&self, dim: &Dimension) -> String {
        format_dimension_preferring_alias_after_validation(
            &self.display_aliases,
            &self.base_dim_names,
            dim,
        )
    }

    /// Add diagnostic formatting for one synthetic rigid template dimension.
    pub(crate) fn register_rigid_dimension(
        &mut self,
        name: &crate::syntax::dimension::ResolvedDimName,
    ) {
        let base = BaseDimId::UserDefined(name.clone());
        self.base_dim_names
            .insert(base.clone(), name.as_str().to_string());
        self.base_dim_symbols
            .insert(base.clone(), name.as_str().to_string());
        self.display_aliases.insert(
            DimRef::local(DimName::from_atom(name.atom().clone())),
            Dimension::base(base),
        );
    }

    /// Format user-defined base dimensions with their canonical owner.
    #[must_use]
    #[expect(
        clippy::unreachable,
        reason = "RegistryBuilder validates complete base-dimension metadata before construction"
    )]
    pub(crate) fn format_dimension_owner_qualified(&self, dim: &Dimension) -> String {
        let names = self
            .base_dim_names
            .iter()
            .map(|(id, name)| {
                let display = match id {
                    BaseDimId::Prelude(_) => name.clone(),
                    BaseDimId::UserDefined(resolved) => resolved.to_string(),
                };
                (id.clone(), display)
            })
            .collect();
        match dim.try_format_with(&names) {
            Ok(formatted) => formatted,
            Err(err) => unreachable!(
                "validated registry lost owner-qualified base dimension metadata: {err}"
            ),
        }
    }
}

/// Dimension registry: maps dimension names to `Dimension` values and tracks
/// base dimension metadata (ID assignment, names, default unit symbols).
#[derive(Debug, Clone)]
pub struct DimensionRegistry {
    /// Base dimension ID → dimension name (for display).
    pub(crate) base_dim_names: BTreeMap<BaseDimId, String>,
    /// Base dimension ID → default unit symbol for runtime display.
    pub(crate) base_dim_symbols: BTreeMap<BaseDimId, String>,
    pub(crate) dimensions: HashMap<DimRef, Dimension>,
    pub(crate) aliases: HashMap<DimRef, DimRef>,
}

impl DimensionRegistry {
    pub(crate) fn into_formatting(self) -> DimensionFormattingRegistry {
        DimensionFormattingRegistry {
            base_dim_names: self.base_dim_names,
            base_dim_symbols: self.base_dim_symbols,
            display_aliases: self.dimensions,
        }
    }

    const fn scope(&self) -> DimensionScope<'_> {
        DimensionScope::new(&self.dimensions, &self.aliases)
    }

    /// Look up an unqualified (local, selectively imported, or prelude)
    /// dimension by name.
    #[must_use]
    pub fn get_dimension(&self, name: &str) -> Option<&Dimension> {
        self.get_dimension_ref(&DimRef::local(DimName::try_new(name).ok()?))
    }

    /// Look up a possibly module-qualified dimension reference.
    #[must_use]
    pub fn get_dimension_ref(&self, reference: &DimRef) -> Option<&Dimension> {
        self.scope().lookup(reference)
    }

    /// Iterate over all named dimensions.
    pub fn all_dimensions(&self) -> impl Iterator<Item = (&DimRef, &Dimension)> {
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
    #[cfg(test)]
    pub(crate) fn resolve_dim_expr(
        &self,
        expr: &DimExpr,
    ) -> Result<Option<Dimension>, RationalError> {
        self.scope().resolve_dim_expr(expr)
    }

    /// Resolve a `DimExpr` AST node to a concrete `Dimension`, preserving the
    /// unknown referenced dimension name in the error.
    pub fn resolve_dim_expr_detailed(
        &self,
        expr: &DimExpr,
    ) -> Result<Dimension, DimensionResolveError> {
        self.scope().resolve_dim_expr_detailed(expr)
    }

    /// Resolve a `TypeExpr` to a concrete `Dimension`.
    ///
    /// Returns `Ok(None)` if the type references unknown dimensions, and
    /// `Err` if dimension exponent arithmetic overflows `i32`.
    pub fn resolve_type_expr(
        &self,
        type_expr: &TypeExpr,
    ) -> Result<Option<Dimension>, RationalError> {
        self.scope().resolve_type_expr(type_expr)
    }
}
