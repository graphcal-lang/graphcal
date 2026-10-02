//! The Static bindings of one include: the authored bindings once they bind
//! every required Static port of the template, and their canonical form.
//!
//! [`IncludeStaticBindings`] is built only from [`CoveredStaticBindings`], so
//! the bindings an include instance carries bind every required port of its
//! template by construction.

use std::collections::{BTreeMap, HashMap};

use graphcal_compiler::ir::module_interface::ModuleInterface;
use graphcal_compiler::ir::static_substitution::{InstanceIndexBindingTarget, StaticSubstitution};
use graphcal_compiler::resolved_name::{
    ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName,
};
use graphcal_compiler::semantic::index_def::IndexBindingTarget;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::index::IndexError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::static_interface::StaticInputKind;
use graphcal_compiler::syntax::dimension::DimName;
use graphcal_compiler::syntax::index_name::IndexName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::StructTypeName;

use crate::compile_error::PipelineError;

/// One include's authored Static bindings, keyed by template-side name, that
/// bind every required Static port of the template.
#[derive(Debug)]
pub(super) struct CoveredStaticBindings {
    indexes: HashMap<IndexName, IndexBindingTarget>,
    index_spans: HashMap<IndexName, Span>,
    types: HashMap<StructTypeName, StructTypeName>,
    dims: HashMap<DimName, DimName>,
}

impl CoveredStaticBindings {
    /// Accept authored bindings that bind every required Static port `dep`
    /// declares.
    ///
    /// # Errors
    ///
    /// Returns I010 at `include_span` for the first unbound required port
    /// (by kind, then name).
    pub(super) fn check(
        dep: &ModuleInterface,
        indexes: HashMap<IndexName, IndexBindingTarget>,
        index_spans: HashMap<IndexName, Span>,
        types: HashMap<StructTypeName, StructTypeName>,
        dims: HashMap<DimName, DimName>,
        file_src: SourceId,
        include_span: Span,
    ) -> Result<Self, PipelineError> {
        validate_required_static_bindings(dep, &types, &dims, &indexes, file_src, include_span)?;
        Ok(Self {
            indexes,
            index_spans,
            types,
            dims,
        })
    }

    /// Resolve every binding canonically with `resolution`.
    ///
    /// An index binding without its own site is reported at `include_span`.
    ///
    /// # Errors
    ///
    /// Returns the first error of `resolution`.
    pub(super) fn resolve(
        self,
        resolution: &impl StaticBindingResolution,
        include_span: Span,
    ) -> Result<IncludeStaticBindings, PipelineError> {
        let Self {
            indexes,
            index_spans,
            types,
            dims,
        } = self;
        let indexes = indexes
            .into_iter()
            .map(|(port, authored)| {
                let span = index_spans.get(&port).copied().unwrap_or(include_span);
                let (identity, target) = resolution.index(&port, &authored, span)?;
                Ok((
                    identity,
                    BoundIndexPort {
                        target,
                        authored,
                        span,
                    },
                ))
            })
            .collect::<Result<_, PipelineError>>()?;
        let types = types
            .iter()
            .map(|(port, target)| resolution.struct_type(port, target))
            .collect::<Result<_, _>>()?;
        let dimensions = dims
            .iter()
            .map(|(port, target)| resolution.dimension(port, target))
            .collect::<Result<_, _>>()?;
        Ok(IncludeStaticBindings {
            indexes,
            types,
            dimensions,
        })
    }
}

/// How one include site names the canonical template port and importer-side
/// target of each authored Static binding.
pub(super) trait StaticBindingResolution {
    /// The index port `port` and the target `authored` binds it to, bound at
    /// `span`.
    ///
    /// # Errors
    ///
    /// Returns the diagnostic of a target that names no index.
    fn index(
        &self,
        port: &IndexName,
        authored: &IndexBindingTarget,
        span: Span,
    ) -> Result<(ResolvedIndexName, InstanceIndexBindingTarget), PipelineError>;

    /// The type port `port` and its target `target`.
    ///
    /// # Errors
    ///
    /// Returns the diagnostic of a target that names no type.
    fn struct_type(
        &self,
        port: &StructTypeName,
        target: &StructTypeName,
    ) -> Result<(ResolvedStructTypeName, ResolvedStructTypeName), PipelineError>;

    /// The dimension port `port` and its target `target`.
    ///
    /// # Errors
    ///
    /// Returns the diagnostic of a target that names no dimension.
    fn dimension(
        &self,
        port: &DimName,
        target: &DimName,
    ) -> Result<(ResolvedDimName, ResolvedDimName), PipelineError>;
}

/// Check that authored bindings bind every required Static port `dep`
/// declares.
///
/// # Errors
///
/// Returns I010 at `include_span` for the first unbound required port (by
/// kind, then name).
pub(super) fn validate_required_static_bindings(
    dep: &ModuleInterface,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
    index_bindings: &HashMap<IndexName, IndexBindingTarget>,
    file_src: SourceId,
    include_span: Span,
) -> Result<(), PipelineError> {
    let mut missing = dep
        .static_declarations()
        .filter(|(kind, name, role)| {
            role.is_required()
                && !match kind {
                    StaticInputKind::Type => {
                        type_bindings.contains_key(&StructTypeName::classify((*name).clone()))
                    }
                    StaticInputKind::Dimension => {
                        dim_bindings.contains_key(&DimName::classify((*name).clone()))
                    }
                    StaticInputKind::Index => {
                        index_bindings.contains_key(&IndexName::classify((*name).clone()))
                    }
                }
        })
        .map(|(kind, name, _)| (kind, name.clone()))
        .collect::<Vec<_>>();
    missing.sort_by(|(first_kind, first_name), (second_kind, second_name)| {
        first_kind
            .marker()
            .cmp(second_kind.marker())
            .then_with(|| first_name.cmp(second_name))
    });
    let Some((kind, name)) = missing.into_iter().next() else {
        return Ok(());
    };
    Err(PipelineError::Semantic(SemanticError::located(
        file_src,
        include_span,
        IndexError::RequiredStaticInputNotBound { kind, name },
    )))
}

/// One include's Static bindings, resolved once at the include site.
///
/// Every bound template port is its canonical identity and every target is
/// the canonical importer-side definition it names; no consumer resolves an
/// authored name again. Built only from [`CoveredStaticBindings`], so every
/// required Static port of the template is bound.
#[derive(Debug)]
pub(super) struct IncludeStaticBindings {
    /// Each bound index port, with its target and binding site in one record.
    indexes: BTreeMap<ResolvedIndexName, BoundIndexPort>,
    types: BTreeMap<ResolvedStructTypeName, ResolvedStructTypeName>,
    dimensions: BTreeMap<ResolvedDimName, ResolvedDimName>,
}

/// One bound index port: the canonical target, and the authored spelling and
/// source site of the binding for diagnostics of its binding contract.
#[derive(Debug)]
pub(super) struct BoundIndexPort {
    pub(super) target: InstanceIndexBindingTarget,
    pub(super) authored: IndexBindingTarget,
    pub(super) span: Span,
}

impl IncludeStaticBindings {
    /// Each bound index port, with its target and binding site.
    pub(super) const fn indexes(&self) -> &BTreeMap<ResolvedIndexName, BoundIndexPort> {
        &self.indexes
    }

    /// Each bound dimension port with its importer-side target.
    pub(super) const fn dimensions(&self) -> &BTreeMap<ResolvedDimName, ResolvedDimName> {
        &self.dimensions
    }

    /// The Static substitution these bindings apply to the template.
    pub(super) fn substitution(&self) -> StaticSubstitution {
        StaticSubstitution {
            indexes: self
                .indexes
                .iter()
                .map(|(port, bound)| (port.clone(), bound.target.clone()))
                .collect(),
            types: self.types.clone(),
            dimensions: self.dimensions.clone(),
        }
    }
}
