//! Canonical evaluation of dimension, unit, and index definitions.
//!
//! Every name written inside a `dim`, `unit`, or `index` declaration is
//! resolved through the [`ModuleResolver`] in the declaring module, with the
//! implicit prelude as fallback — exactly like HIR type lowering. Each
//! canonical identity is evaluated once, on demand, so a definition may refer
//! to a dimension or unit of any other module (an imported module, an inline
//! DAG of the same file, or the file its inline DAG self-imports from)
//! regardless of lowering order. There is no source-name keyed registry and no
//! merge of one module's tables into another's.

use crate::semantic::dimension_table::DimensionSpelling;
use crate::semantic_error::dimension::BaseUnitRejection;
use crate::semantic_error::dimension::UnitScaleSite;
use crate::semantic_error::index::CoordinateArgumentDimensions;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::dag_id::DagId;
use crate::desugar::desugared_ast::{
    self as ast, DeclKind, Declaration, DimDecl, DimExpr, IndexDecl, IndexDeclKind, UnitDecl,
    UnitExpr,
};
use crate::dimension::{BaseDimId, Dimension};
use crate::hir::const_expr::{ConstExprError, CoordinateAxisError, CoordinateAxisExpr};
use crate::hir::const_lower::{
    UnitScaleSource, classify_unit_scale, lower_coordinate_expr, lower_static_nat_expr,
};
use crate::ir::module_definitions::StaticDefinitions;
use crate::ir::prelude_definitions::PreludeDefinitionError;
use crate::resolve::ModuleResolver;
use crate::resolve::prelude::PreludeTypeScope;
use crate::resolved_name::{ResolvedDimName, ResolvedIndexName, ResolvedUnitName};
use crate::semantic::dimension_table::{BaseDimensionInfo, DimensionFormattingRegistry};
use crate::semantic::index_def::{
    ConcreteIndexKind, FiniteIndex, IndexBindingTarget, IndexDef, IndexKind, RequiredIndexKind,
};
use crate::semantic::unit_scale::{
    PositiveFiniteScale, PositiveFiniteScaleError, UnitInfo, UnitResolveError, UnitScale,
    resolve_unit_expr_with,
};
use crate::semantic_error::SemanticError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::index::IndexError;
use crate::semantic_error::name::NameError;
use crate::source_id::SourceId;
use crate::syntax::ast::{BindableVisibility, UnitConstness};
use crate::syntax::dimension::{DimName, DimRef, UnitName, UnitRef};
use crate::syntax::index_name::IndexName;
use crate::syntax::non_empty::{DuplicateItemError, NonEmptyUnique};
use crate::syntax::span::{Span, Spanned};

use super::entry::{DynamicUnitScaleEntry, InScope, Syntax};

/// The declarations and diagnostic source of one module.
#[derive(Debug, Clone, Copy)]
pub struct DefinitionSource<'a> {
    pub declarations: &'a [Declaration],
    pub src: SourceId,
}

/// A dimension expression that could not be evaluated.
#[derive(Debug)]
pub enum DimExprFailure {
    /// A written reference names no visible dimension.
    Unknown(DimRef),
    /// Exponent arithmetic overflowed.
    Overflow,
    /// A referenced dimension's own definition is invalid.
    Definition(Box<SemanticError>),
}

/// One module's evaluated Static definitions together with the runtime unit
/// scales its body must lower.
#[derive(Debug, Clone)]
pub(crate) struct ModuleStaticDefinitions {
    pub(crate) definitions: StaticDefinitions,
    pub(super) dynamic_unit_scales: Vec<DynamicUnitScaleEntry<Syntax>>,
}

/// A declaration being evaluated, reported when one of its references closes
/// a cycle.
#[derive(Debug, Clone, Copy)]
enum CycleSite<'a> {
    Dimension {
        name: &'a Spanned<DimName>,
        src: SourceId,
    },
    Unit {
        name: &'a Spanned<UnitName>,
        src: SourceId,
    },
}

impl CycleSite<'_> {
    fn error(self) -> SemanticError {
        match self {
            Self::Dimension { name, src } => SemanticError::located(
                src,
                name.span,
                DimensionError::CyclicDimension {
                    name: name.value.clone(),
                },
            ),
            Self::Unit { name, src } => SemanticError::located(
                src,
                name.span,
                DimensionError::CyclicUnit {
                    name: name.value.clone(),
                },
            ),
        }
    }
}

/// How the defaulted bindable dimension ports (`pub(bind) dim Q = Length;`)
/// of a module are seen while evaluating a dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PortView {
    /// Every port has its default definition.
    Default,
    /// The ports of this module stay opaque base dimensions, so a dimension
    /// defined over them is symbolic in them. This is the view an include
    /// binding (or the template-closure check) substitutes into.
    Generic(DagId),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum StaticItem {
    Dimension(ResolvedDimName),
    Unit(ResolvedUnitName),
    Index(ResolvedIndexName),
}

/// The source declaration of one dimension.
#[derive(Debug, Clone, Copy)]
enum DimensionSource<'a> {
    /// `base dim Foo;` or a required `dim D;`: an opaque base dimension.
    Base(&'a Spanned<DimName>),
    /// `dim Foo = <expr>;`
    Derived(&'a DimDecl, &'a DimExpr),
}

/// Declarations of one module indexed by the name they introduce.
#[derive(Debug)]
struct ModuleSource<'a> {
    src: SourceId,
    declarations: &'a [Declaration],
    dimensions: HashMap<DimName, DimensionSource<'a>>,
    units: HashMap<UnitName, &'a UnitDecl>,
    indexes: HashMap<IndexName, (&'a IndexDecl, Span)>,
    types: HashMap<crate::syntax::type_name::StructTypeName, &'a ast::TypeDecl>,
}

impl<'a> ModuleSource<'a> {
    fn new(source: DefinitionSource<'a>) -> Self {
        let mut module = Self {
            src: source.src,
            declarations: source.declarations,
            dimensions: HashMap::new(),
            units: HashMap::new(),
            indexes: HashMap::new(),
            types: HashMap::new(),
        };
        for declaration in source.declarations {
            match &declaration.kind {
                DeclKind::BaseDimension(dimension) => {
                    module
                        .dimensions
                        .entry(dimension.name.value.clone())
                        .or_insert(DimensionSource::Base(&dimension.name));
                }
                DeclKind::Dimension(dimension) => {
                    let definition = dimension
                        .definition
                        .as_ref()
                        .map_or(DimensionSource::Base(&dimension.name), |definition| {
                            DimensionSource::Derived(dimension, definition)
                        });
                    module
                        .dimensions
                        .entry(dimension.name.value.clone())
                        .or_insert(definition);
                }
                DeclKind::Unit(unit) => {
                    module.units.entry(unit.name.value.clone()).or_insert(unit);
                }
                DeclKind::Index(index) => {
                    module
                        .indexes
                        .entry(index.name.value.clone())
                        .or_insert((index, declaration.span));
                }
                DeclKind::Type(type_decl) => {
                    module
                        .types
                        .entry(type_decl.name.value.clone())
                        .or_insert(type_decl);
                }
                DeclKind::Param(_)
                | DeclKind::Node(_)
                | DeclKind::ConstNode(_)
                | DeclKind::Assert(_)
                | DeclKind::Plot(_)
                | DeclKind::Figure(_)
                | DeclKind::Layer(_)
                | DeclKind::Import(_)
                | DeclKind::PluginImport(_)
                | DeclKind::Include(_)
                | DeclKind::Dag(_) => {}
                #[expect(
                    clippy::uninhabited_references,
                    reason = "Sugar(Infallible) proves this arm unreachable"
                )]
                DeclKind::Sugar(sugar) => crate::syntax::phase::never(*sugar),
            }
        }
        module
    }
}

/// Demand-driven evaluator of every canonical dimension, unit, and index of a
/// project.
#[derive(Debug)]
pub struct StaticDefinitionEvaluator<'a> {
    resolver: &'a ModuleResolver,
    prelude: &'static PreludeTypeScope,
    prelude_dimensions: Vec<(DimRef, ResolvedDimName)>,
    modules: HashMap<DagId, ModuleSource<'a>>,
    dimensions: HashMap<ResolvedDimName, Dimension>,
    /// Port-generic values ([`PortView::Generic`] of the owning module).
    port_generic_dimensions: HashMap<ResolvedDimName, Dimension>,
    units: HashMap<ResolvedUnitName, UnitInfo>,
    indexes: HashMap<ResolvedIndexName, IndexDef>,
    base_dimensions: BTreeMap<BaseDimId, BaseDimensionInfo>,
    /// Base dimensions each module declares or assigns a canonical unit to.
    base_annotations: HashMap<DagId, Vec<BaseDimId>>,
    dynamic_unit_scales: HashMap<DagId, Vec<DynamicUnitScaleEntry<Syntax>>>,
    in_progress: HashSet<StaticItem>,
    /// Source an invariant violation is reported against when the offending
    /// definition has no declaring module (the entry file).
    fallback_src: SourceId,
}

impl<'a> StaticDefinitionEvaluator<'a> {
    /// Create an evaluator over every module the resolver knows.
    ///
    /// # Errors
    ///
    /// Returns an error only if the built-in prelude is inconsistent.
    pub fn new(
        resolver: &'a ModuleResolver,
        sources: impl IntoIterator<Item = (DagId, DefinitionSource<'a>)>,
        fallback_src: SourceId,
    ) -> Result<Self, PreludeDefinitionError> {
        let prelude_definitions = crate::ir::prelude_definitions::prelude_definitions()?;
        let prelude_dimensions = prelude_definitions
            .dimensions()
            .map(|(identity, _)| {
                (
                    DimRef::local(identity.to_unowned_def_name()),
                    identity.clone(),
                )
            })
            .collect();
        Ok(Self {
            resolver,
            prelude: crate::resolve::prelude::prelude_type_scope(),
            prelude_dimensions,
            modules: sources
                .into_iter()
                .map(|(owner, source)| (owner, ModuleSource::new(source)))
                .collect(),
            dimensions: prelude_definitions
                .dimensions()
                .map(|(identity, dimension)| (identity.clone(), dimension.clone()))
                .collect(),
            port_generic_dimensions: HashMap::new(),
            units: prelude_definitions
                .units()
                .map(|(identity, info)| (identity.clone(), info.clone()))
                .collect(),
            indexes: HashMap::new(),
            base_dimensions: prelude_definitions
                .base_dimensions()
                .map(|(id, info)| (id.clone(), info.clone()))
                .collect(),
            base_annotations: HashMap::new(),
            dynamic_unit_scales: HashMap::new(),
            in_progress: HashSet::new(),
            fallback_src,
        })
    }

    /// The resolver every written name is resolved through.
    #[must_use]
    pub const fn resolver(&self) -> &'a ModuleResolver {
        self.resolver
    }

    /// Metadata of every base dimension evaluated so far.
    #[must_use]
    pub const fn base_dimensions(&self) -> &BTreeMap<BaseDimId, BaseDimensionInfo> {
        &self.base_dimensions
    }

    /// The `type` declaration of a module-owned nominal type, with the
    /// declaring module's diagnostic source.
    #[must_use]
    pub fn type_declaration(
        &self,
        identity: &crate::resolved_name::ResolvedStructTypeName,
    ) -> Option<(&'a ast::TypeDecl, SourceId)> {
        let module = self.modules.get(identity.owner())?;
        module
            .types
            .get(&identity.to_unowned_def_name())
            .map(|declaration| (*declaration, module.src))
    }

    /// Resolve a dimension reference written in `owner`.
    ///
    /// Only a declaration of a source module (or the prelude) has a
    /// definition; a member of an included instance's namespace is not a
    /// Static name of the including module.
    #[must_use]
    pub fn resolve_dimension(&self, owner: &DagId, reference: &DimRef) -> Option<ResolvedDimName> {
        let path = reference.to_name_path();
        self.resolver
            .resolve_dimension_path(owner, &path)
            .map_or_else(
                |_| self.prelude.resolve_dimension_path(&path),
                |symbol| {
                    Some(symbol.into_resolved())
                        .filter(|identity| self.has_source(identity.owner()))
                },
            )
    }

    /// Whether `identity` has a definition this evaluator can produce: a
    /// declaration of a source module or a prelude dimension. An
    /// instance-owned identity has none.
    #[must_use]
    pub fn defines_dimension(&self, identity: &ResolvedDimName) -> bool {
        self.dimensions.contains_key(identity) || self.has_source(identity.owner())
    }

    fn has_source(&self, owner: &DagId) -> bool {
        self.modules.contains_key(owner)
    }

    /// Resolve a unit reference written in `owner`.
    ///
    /// A runtime unit never crosses a pure `import` boundary: spelled
    /// through an imported module alias, it resolves to nothing.
    #[must_use]
    pub fn resolve_unit(&self, owner: &DagId, reference: &UnitRef) -> Option<ResolvedUnitName> {
        let through_import = reference.owner().is_some_and(|qualifier| {
            self.resolver.module_alias_role(
                owner,
                &crate::syntax::module_name::ModuleAliasName::classify(qualifier.first().clone()),
            ) == Some(crate::resolve::scope::ModuleAliasRole::ImportedDag)
        });
        self.resolver
            .resolve_unit_path(owner, &reference.to_name_path())
            .map_or_else(
                |_| self.prelude.resolve_unit_ref(reference),
                |symbol| {
                    (self.has_source(symbol.resolved().owner())
                        && (!through_import || symbol.kind().is_const()))
                    .then(|| symbol.into_resolved())
                },
            )
    }

    /// Evaluate one canonical dimension.
    ///
    /// # Errors
    ///
    /// Returns the diagnostic of an invalid definition or a definition cycle.
    pub fn dimension(&mut self, identity: &ResolvedDimName) -> Result<Dimension, SemanticError> {
        self.dimension_from(identity, None)
    }

    /// Evaluate one canonical unit.
    ///
    /// # Errors
    ///
    /// Returns the diagnostic of an invalid definition or a definition cycle.
    pub fn unit(&mut self, identity: &ResolvedUnitName) -> Result<UnitInfo, SemanticError> {
        self.unit_from(identity, None)
    }

    /// Evaluate one canonical index.
    ///
    /// # Errors
    ///
    /// Returns the diagnostic of an invalid definition.
    pub fn index(&mut self, identity: &ResolvedIndexName) -> Result<IndexDef, SemanticError> {
        if let Some(definition) = self.indexes.get(identity) {
            return Ok(definition.clone());
        }
        let owner = identity.owner();
        let finite = self
            .resolver
            .symbols(owner)
            .and_then(|symbols| symbols.finite_index_projection(&identity.to_unowned_def_name()));
        let definition = if let Some(finite) = finite {
            IndexDef::finite(finite)
        } else {
            let (declaration, span, src) = self.index_declaration(identity)?;
            self.in_progress.insert(StaticItem::Index(identity.clone()));
            let definition = self.index_definition(owner, declaration, span, src);
            self.in_progress
                .remove(&StaticItem::Index(identity.clone()));
            definition?
        };
        self.indexes.insert(identity.clone(), definition.clone());
        Ok(definition)
    }

    /// Evaluate a dimension expression written in `owner`, replacing every
    /// reference that resolves to an `overrides` key with its value; a `None`
    /// override makes the reference unknown.
    ///
    /// # Errors
    ///
    /// Returns the unknown reference, an exponent overflow, or the
    /// diagnostic of an invalid referenced definition.
    pub fn evaluate_dim_expr_with_overrides(
        &mut self,
        owner: &DagId,
        expr: &DimExpr,
        overrides: &HashMap<ResolvedDimName, Option<Dimension>>,
    ) -> Result<Dimension, DimExprFailure> {
        self.evaluate_dim_expr(owner, expr, overrides, &PortView::Default, None)
    }

    /// The owner-qualified definitions of every dimension, unit, and index
    /// symbol of `owner`.
    ///
    /// Declarations are evaluated in phases (named indexes, derived
    /// dimensions, required coordinate indexes, units, coordinate indexes) so
    /// the first reported error follows declaration order within a phase.
    pub(crate) fn module_definitions(
        &mut self,
        owner: &DagId,
    ) -> Result<ModuleStaticDefinitions, SemanticError> {
        for item in self.evaluation_order(owner) {
            match &item {
                StaticItem::Dimension(identity) => self.dimension(identity).map(drop),
                StaticItem::Unit(identity) => self.unit(identity).map(drop),
                StaticItem::Index(identity) => self.index(identity).map(drop),
            }?;
        }
        if let Some(module) = self.modules.get(owner) {
            validate_structural_finite_indexes(module.declarations, module.src)?;
        }

        let mut definitions = StaticDefinitions::new(owner.clone());
        let fallback_src = self.fallback_src;
        let foreign = |error: crate::ir::module_definitions::ForeignDefinitionError| {
            missing_definition_error(&error.to_string(), fallback_src)
        };
        if let Some(symbols) = self.resolver.symbols(owner) {
            for symbol in symbols.dimensions().values() {
                let dimension = self.dimension(symbol.resolved())?;
                let generic = self.port_generic_dimension(symbol.resolved())?;
                if generic != dimension {
                    definitions
                        .insert_port_generic_dimension(symbol.resolved().clone(), generic)
                        .map_err(foreign)?;
                }
                definitions
                    .insert_dimension(symbol.resolved().clone(), dimension)
                    .map_err(foreign)?;
            }
            for symbol in symbols.units().values() {
                let info = self.unit(symbol.resolved())?;
                definitions
                    .insert_unit(symbol.resolved().clone(), info)
                    .map_err(foreign)?;
            }
            for symbol in symbols.indexes().values() {
                let index = self.index(symbol.resolved())?;
                definitions
                    .insert_index(symbol.resolved().clone(), index)
                    .map_err(foreign)?;
            }
        }
        for id in self.base_annotations.get(owner).into_iter().flatten() {
            if let Some(info) = self.base_dimensions.get(id) {
                definitions.insert_base_dimension(id.clone(), info.clone());
            }
        }
        // A module without runtime-scaled units records no entry.
        let dynamic_unit_scales = self
            .dynamic_unit_scales
            .get(owner)
            .map_or_else(Vec::new, Clone::clone);
        Ok(ModuleStaticDefinitions {
            definitions,
            dynamic_unit_scales,
        })
    }

    /// The declared dimensions, units, and indexes of `owner` whose
    /// definitions are evaluated, in phase order and declaration order
    /// within a phase.
    fn evaluation_order(&self, owner: &DagId) -> Vec<StaticItem> {
        let (Some(symbols), Some(module)) = (self.resolver.symbols(owner), self.modules.get(owner))
        else {
            return Vec::new();
        };
        let mut items = module
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.kind {
                DeclKind::Index(index) => {
                    let phase = match index.kind {
                        IndexDeclKind::Named { .. } | IndexDeclKind::RequiredNamed => 0,
                        IndexDeclKind::RequiredCoordinate { .. } => 2,
                        IndexDeclKind::Range { .. } | IndexDeclKind::Linspace { .. } => 4,
                    };
                    symbols
                        .indexes()
                        .get(&index.name.value)
                        .map(|symbol| (phase, StaticItem::Index(symbol.resolved().clone())))
                }
                DeclKind::Dimension(dimension) if dimension.definition.is_some() => symbols
                    .dimensions()
                    .get(&dimension.name.value)
                    .map(|symbol| (1, StaticItem::Dimension(symbol.resolved().clone()))),
                DeclKind::Unit(unit) => symbols
                    .units()
                    .get(&unit.name.value)
                    .map(|symbol| (3, StaticItem::Unit(symbol.resolved().clone()))),
                _ => None,
            })
            .collect::<Vec<_>>();
        items.sort_by_key(|(phase, _)| *phase);
        items.into_iter().map(|(_, item)| item).collect()
    }

    /// Every dimension spelling visible in `owner` with its value, for
    /// diagnostics that prefer a named dimension over its base-dimension
    /// expansion.
    ///
    /// # Errors
    ///
    /// Returns the diagnostic of an invalid visible definition.
    pub(crate) fn display_dimensions(
        &mut self,
        owner: &DagId,
    ) -> Result<Vec<(DimRef, Dimension)>, SemanticError> {
        self.display_spellings(owner)
            .into_iter()
            .map(|(spelling, identity)| Ok((spelling, self.dimension(&identity)?)))
            .collect()
    }

    fn display_spellings(&self, owner: &DagId) -> Vec<(DimRef, ResolvedDimName)> {
        self.prelude_dimensions
            .iter()
            .cloned()
            .chain(
                self.resolver
                    .visible_dimension_spellings(owner)
                    .into_iter()
                    .filter(|(_, identity)| self.has_source(identity.owner()))
                    .map(|(spelling, identity)| (spelling, identity.clone())),
            )
            .collect()
    }

    /// Format a dimension for a diagnostic in `owner`, using only dimensions
    /// already evaluated.
    #[must_use]
    pub fn format_dimension(&self, owner: &DagId, dimension: &Dimension) -> String {
        self.display_registry(owner).format_dimension(dimension)
    }

    /// Spell a dimension for a diagnostic in `owner`, using only dimensions
    /// already evaluated.
    #[must_use]
    pub fn dimension_spelling(&self, owner: &DagId, dimension: &Dimension) -> DimensionSpelling {
        self.display_registry(owner).dimension_spelling(dimension)
    }

    fn display_registry(&self, owner: &DagId) -> DimensionFormattingRegistry {
        DimensionFormattingRegistry::new(
            self.base_dimensions.clone(),
            self.display_spellings(owner)
                .into_iter()
                .filter_map(|(spelling, identity)| {
                    self.dimensions
                        .get(&identity)
                        .map(|dimension| (spelling, dimension.clone()))
                }),
        )
    }

    fn annotate_base(&mut self, owner: &DagId, id: BaseDimId) {
        self.base_annotations
            .entry(owner.clone())
            .or_default()
            .push(id);
    }

    fn dimension_from(
        &mut self,
        identity: &ResolvedDimName,
        site: Option<CycleSite<'a>>,
    ) -> Result<Dimension, SemanticError> {
        if let Some(dimension) = self.dimensions.get(identity) {
            return Ok(dimension.clone());
        }
        let item = StaticItem::Dimension(identity.clone());
        if self.in_progress.contains(&item) {
            return Err(site.map_or_else(|| self.dimension_cycle_at(identity), CycleSite::error));
        }
        let owner = identity.owner();
        let projection = self.resolver.symbols(owner).and_then(|symbols| {
            symbols
                .dimension_projection(&identity.to_unowned_def_name())
                .cloned()
        });
        self.in_progress.insert(item.clone());
        let dimension = match projection {
            Some(projection) => self.projected_dimension(owner, &projection, &PortView::Default),
            None => self.declared_dimension(identity),
        };
        self.in_progress.remove(&item);
        let dimension = dimension?;
        self.dimensions.insert(identity.clone(), dimension.clone());
        Ok(dimension)
    }

    /// Evaluate a dimension with its module's defaulted bindable dimension
    /// ports kept opaque ([`PortView::Generic`]).
    ///
    /// # Errors
    ///
    /// Returns the diagnostic of an invalid definition or a definition cycle.
    pub(crate) fn port_generic_dimension(
        &mut self,
        identity: &ResolvedDimName,
    ) -> Result<Dimension, SemanticError> {
        let view = PortView::Generic(identity.owner().clone());
        self.dimension_in(identity, &view, None)
    }

    fn dimension_in(
        &mut self,
        identity: &ResolvedDimName,
        view: &PortView,
        site: Option<CycleSite<'a>>,
    ) -> Result<Dimension, SemanticError> {
        // The default evaluation validates the definition and rejects cycles
        // for the whole reference closure the generic view walks again.
        let default = self.dimension_from(identity, site)?;
        let PortView::Generic(module) = view else {
            return Ok(default);
        };
        if identity.owner() != module {
            return Ok(default);
        }
        if let Some(generic) = self.port_generic_dimensions.get(identity) {
            return Ok(generic.clone());
        }
        let owner = identity.owner();
        let projection = self.resolver.symbols(owner).and_then(|symbols| {
            symbols
                .dimension_projection(&identity.to_unowned_def_name())
                .cloned()
        });
        let source = self.modules.get(owner).and_then(|module| {
            module
                .dimensions
                .get(&identity.to_unowned_def_name())
                .map(|source| (*source, module.src))
        });
        let generic = match (projection, source) {
            (Some(projection), _) => self.projected_dimension(owner, &projection, view)?,
            (None, Some((DimensionSource::Derived(declaration, _), _)))
                if declaration.visibility == BindableVisibility::PublicBind =>
            {
                Dimension::base(BaseDimId::UserDefined(identity.clone()))
            }
            (None, Some((DimensionSource::Derived(_, definition), src))) => self
                .evaluate_dim_expr(owner, definition, &HashMap::new(), view, site)
                .map_err(|failure| dim_expr_error(failure, src, definition.span))?,
            (None, Some((DimensionSource::Base(_), _)) | None) => default,
        };
        self.port_generic_dimensions
            .insert(identity.clone(), generic.clone());
        Ok(generic)
    }

    /// Whether `identity` is a defaulted bindable dimension port of its module.
    fn is_defaulted_dimension_port(&self, identity: &ResolvedDimName) -> bool {
        self.modules
            .get(identity.owner())
            .and_then(|module| module.dimensions.get(&identity.to_unowned_def_name()))
            .is_some_and(|source| {
                matches!(
                    source,
                    DimensionSource::Derived(declaration, _)
                        if declaration.visibility == BindableVisibility::PublicBind
                )
            })
    }

    fn dimension_cycle_at(&self, identity: &ResolvedDimName) -> SemanticError {
        let source = self.modules.get(identity.owner()).and_then(|module| {
            module
                .dimensions
                .get(&identity.to_unowned_def_name())
                .map(|source| (source, module.src))
        });
        match source {
            Some((DimensionSource::Base(name), src)) => CycleSite::Dimension { name, src }.error(),
            Some((DimensionSource::Derived(declaration, _), src)) => CycleSite::Dimension {
                name: &declaration.name,
                src,
            }
            .error(),
            None => missing_definition_error(&identity.to_string(), self.fallback_src),
        }
    }

    fn declared_dimension(
        &mut self,
        identity: &ResolvedDimName,
    ) -> Result<Dimension, SemanticError> {
        let owner = identity.owner();
        let Some((source, src)) = self.modules.get(owner).and_then(|module| {
            module
                .dimensions
                .get(&identity.to_unowned_def_name())
                .map(|source| (*source, module.src))
        }) else {
            return Err(missing_definition_error(
                &identity.to_string(),
                self.fallback_src,
            ));
        };
        match source {
            DimensionSource::Base(_) => {
                let id = BaseDimId::UserDefined(identity.clone());
                self.base_dimensions.entry(id.clone()).or_default();
                self.annotate_base(owner, id.clone());
                Ok(Dimension::base(id))
            }
            DimensionSource::Derived(declaration, definition) => self
                .evaluate_dim_expr(
                    owner,
                    definition,
                    &HashMap::new(),
                    &PortView::Default,
                    Some(CycleSite::Dimension {
                        name: &declaration.name,
                        src,
                    }),
                )
                .map_err(|failure| dim_expr_error(failure, src, definition.span)),
        }
    }

    /// Specialize a template dimension through an include's dimension
    /// bindings.
    ///
    /// The template's dimension is taken in its port-generic view, so a
    /// dimension defined over a port (`QR = Q / Time`) keeps that port as an
    /// opaque base whether the port is required (`pub(bind) dim Q;`) or
    /// defaulted (`pub(bind) dim Q = Length;`). Each port base bound by the
    /// projecting include is replaced with the importer's bound dimension
    /// (`dim Q: Mass` makes `QR` `Mass / Time`), evaluated in `view`; an
    /// unbound defaulted port takes its default. An unknown binding target is
    /// left opaque here; the include's binding validation reports it.
    fn projected_dimension(
        &mut self,
        owner: &DagId,
        projection: &crate::resolve::symbols::DimensionProjection,
        view: &PortView,
    ) -> Result<Dimension, SemanticError> {
        let template = self.port_generic_dimension(projection.template())?;
        let template_owner = projection.template().owner().clone();
        let mut bound = HashMap::new();
        for port in projection.ports() {
            let Some(target) =
                self.resolve_dimension(owner, &port.target().clone().classify_leaf())
            else {
                continue;
            };
            let value = self.dimension_in(&target, view, None)?;
            bound.insert(port.port().clone(), value);
        }
        let mut factors = Vec::new();
        for (base, exponent) in template.iter() {
            let replacement = match base {
                BaseDimId::UserDefined(port) if port.owner() == &template_owner => {
                    match bound.get(&port.to_unowned_def_name()) {
                        Some(value) => Some(value.clone()),
                        None if self.is_defaulted_dimension_port(port) => {
                            Some(self.dimension(port)?)
                        }
                        None => None,
                    }
                }
                BaseDimId::UserDefined(_) | BaseDimId::Prelude(_) => None,
            };
            factors.push((
                replacement.unwrap_or_else(|| Dimension::base(base.clone())),
                *exponent,
            ));
        }
        let src = self
            .modules
            .get(owner)
            .map_or(self.fallback_src, |module| module.src);
        factors
            .into_iter()
            .try_fold(Dimension::dimensionless(), |acc, (factor, exponent)| {
                factor
                    .pow(exponent)
                    .and_then(|factor| acc.checked_mul(&factor))
            })
            .map_err(|_| {
                SemanticError::located(src, src.whole_span(), DimensionError::DimensionOverflow)
            })
    }

    fn evaluate_dim_expr(
        &mut self,
        owner: &DagId,
        expr: &DimExpr,
        overrides: &HashMap<ResolvedDimName, Option<Dimension>>,
        view: &PortView,
        site: Option<CycleSite<'a>>,
    ) -> Result<Dimension, DimExprFailure> {
        let mut factors = Vec::with_capacity(expr.terms.len());
        for item in &expr.terms {
            let reference: DimRef = item.term.name.value.clone().classify_leaf();
            let Some(identity) = self.resolve_dimension(owner, &reference) else {
                return Err(DimExprFailure::Unknown(reference));
            };
            let value = match overrides.get(&identity) {
                Some(Some(value)) => value.clone(),
                Some(None) => return Err(DimExprFailure::Unknown(reference)),
                None => self
                    .dimension_in(&identity, view, site)
                    .map_err(|error| DimExprFailure::Definition(Box::new(error)))?,
            };
            factors.push((value, item.term.effective_power(), item.op));
        }
        factors
            .into_iter()
            .try_fold(Dimension::dimensionless(), |acc, (value, power, op)| {
                let powered = value.pow(power)?;
                match op {
                    ast::MulDivOp::Mul => acc * powered,
                    ast::MulDivOp::Div => acc / powered,
                }
            })
            .map_err(|_| DimExprFailure::Overflow)
    }

    fn unit_from(
        &mut self,
        identity: &ResolvedUnitName,
        site: Option<CycleSite<'a>>,
    ) -> Result<UnitInfo, SemanticError> {
        if let Some(info) = self.units.get(identity) {
            return Ok(info.clone());
        }
        let item = StaticItem::Unit(identity.clone());
        let owner = identity.owner();
        let Some((declaration, src)) = self.modules.get(owner).and_then(|module| {
            module
                .units
                .get(&identity.to_unowned_def_name())
                .map(|declaration| (*declaration, module.src))
        }) else {
            return Err(missing_definition_error(
                &identity.to_string(),
                self.fallback_src,
            ));
        };
        if self.in_progress.contains(&item) {
            return Err(site
                .unwrap_or(CycleSite::Unit {
                    name: &declaration.name,
                    src,
                })
                .error());
        }
        self.in_progress.insert(item.clone());
        let info = self.declared_unit(identity, declaration, src);
        self.in_progress.remove(&item);
        let info = info?;
        self.units.insert(identity.clone(), info.clone());
        Ok(info)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one unit declaration's validation and scale evaluation in source order"
    )]
    fn declared_unit(
        &mut self,
        identity: &ResolvedUnitName,
        unit: &'a UnitDecl,
        src: SourceId,
    ) -> Result<UnitInfo, SemanticError> {
        let owner = identity.owner();
        let site = CycleSite::Unit {
            name: &unit.name,
            src,
        };
        let dim = self
            .evaluate_dim_expr(
                owner,
                &unit.dim_type,
                &HashMap::new(),
                &PortView::Default,
                None,
            )
            .map_err(|failure| dim_expr_error(failure, src, unit.dim_type.span))?;
        let Some(def) = &unit.definition else {
            let base = dim.base_dimension_id().cloned();
            let registered = base.as_ref().map(|base| {
                self.base_dimensions
                    .entry(base.clone())
                    .or_default()
                    .register_canonical_unit(unit.name.value.clone())
            });
            let reason = match registered {
                Some(Ok(())) => None,
                Some(Err(error)) => Some(BaseUnitRejection::CanonicalUnitTaken {
                    existing: error.existing,
                }),
                None => Some(BaseUnitRejection::NotBaseDimension),
            };
            if let Some(rejection) = reason {
                return Err(SemanticError::located(
                    src,
                    unit.name.span,
                    DimensionError::InvalidBaseUnitDeclaration {
                        name: unit.name.value.clone(),
                        dim: self.dimension_spelling(owner, &dim),
                        rejection,
                    },
                ));
            }
            if let Some(base) = base {
                self.annotate_base(owner, base);
            }
            return Ok(UnitInfo {
                dimension: dim,
                scale: UnitScale::Const(PositiveFiniteScale::ONE),
            });
        };

        if dim
            .base_dimension_id()
            .and_then(|id| self.base_dimensions.get(id))
            .is_some_and(BaseDimensionInfo::is_affine_prone)
        {
            return Err(SemanticError::located(
                src,
                unit.name.span,
                DimensionError::AffineProneUnitDefinition {
                    dim: self.dimension_spelling(owner, &dim),
                },
            ));
        }
        let scale_source = classify_unit_scale(&def.scale_expr);
        if unit.constness.is_const() {
            if let UnitScaleSource::Dynamic { first_graph_ref } = &scale_source {
                return Err(SemanticError::located(
                    src,
                    first_graph_ref.span,
                    DimensionError::GraphRefInConstUnit {
                        name: first_graph_ref.value.clone(),
                    },
                ));
            }
            for term in &def.unit_expr.terms {
                let info = match self.resolve_unit(owner, &term.name.value) {
                    Some(target) => Some(self.unit_from(&target, Some(site))?),
                    None => None,
                };
                if info.is_some_and(|info| !info.scale.constness().is_const()) {
                    return Err(SemanticError::located(
                        src,
                        term.name.span,
                        DimensionError::NonConstUnitInConst {
                            name: term.name.value.clone(),
                        },
                    ));
                }
            }
        }
        let (base_unit_dimension, base_scale) = self
            .resolve_unit_expr(owner, &def.unit_expr, Some(site))?
            .map_err(|error| unit_resolve_error(error, src, def.unit_expr.span))?;
        if base_unit_dimension != dim {
            return Err(SemanticError::located(
                src,
                def.unit_expr.span,
                DimensionError::UnitDefinitionDimensionMismatch {
                    name: unit.name.value.clone(),
                    declared: self.dimension_spelling(owner, &dim),
                    definition: self.dimension_spelling(owner, &base_unit_dimension),
                },
            ));
        }
        let scale = match scale_source {
            UnitScaleSource::Dynamic { .. } => {
                // Preserve the validated definition so strict HIR lowering,
                // policy checking, dependency collection, and type checking all
                // consume the same source-qualified semantic entry.
                self.dynamic_unit_scales
                    .entry(owner.clone())
                    .or_default()
                    .push(DynamicUnitScaleEntry {
                        unit: owner.clone(),
                        spelling: UnitRef::local(unit.name.value.clone()),
                        expr: InScope::new(def.scale_expr.clone(), owner.clone()),
                        span: def.scale_expr.span,
                        src,
                    });
                UnitScale::Dynamic {
                    base_unit_scale: base_scale,
                }
            }
            UnitScaleSource::Static(source) => {
                // A plain `unit` with no `@` still remains a runtime unit for
                // const-context policy; `const unit` is the surface marker
                // that makes it available to `const node`.
                let scale_expr = source
                    .lower()
                    .and_then(|expr| expr.evaluate())
                    .map_err(|error| const_expr_error(error, src))?;
                let scale = scale_expr
                    .checked_mul(base_scale)
                    .map_err(|err| scale_error(UnitScaleSite::Definition, err, src, def.span))?;
                match unit.constness {
                    UnitConstness::Const => UnitScale::Const(scale),
                    UnitConstness::Dynamic => UnitScale::Runtime(scale),
                }
            }
        };
        Ok(UnitInfo {
            dimension: dim,
            scale,
        })
    }

    /// Resolve a unit expression written in `owner` to its dimension and
    /// static scale.
    ///
    /// The outer error is an invalid referenced unit definition; the inner
    /// one is a failure of the expression itself.
    fn resolve_unit_expr(
        &mut self,
        owner: &DagId,
        expr: &UnitExpr,
        site: Option<CycleSite<'a>>,
    ) -> Result<Result<(Dimension, PositiveFiniteScale), UnitResolveError>, SemanticError> {
        let mut infos = HashMap::new();
        for term in &expr.terms {
            if let Some(target) = self.resolve_unit(owner, &term.name.value) {
                let info = self.unit_from(&target, site)?;
                infos.insert(term.name.value.clone(), info);
            }
        }
        Ok(resolve_unit_expr_with(expr, |reference| {
            infos.get(reference)
        }))
    }

    fn index_declaration(
        &self,
        identity: &ResolvedIndexName,
    ) -> Result<(&'a IndexDecl, Span, SourceId), SemanticError> {
        self.modules
            .get(identity.owner())
            .and_then(|module| {
                module
                    .indexes
                    .get(&identity.to_unowned_def_name())
                    .map(|(declaration, span)| (*declaration, *span, module.src))
            })
            .ok_or_else(|| missing_definition_error(&identity.to_string(), self.fallback_src))
    }

    fn index_definition(
        &mut self,
        owner: &DagId,
        index: &'a IndexDecl,
        decl_span: Span,
        src: SourceId,
    ) -> Result<IndexDef, SemanticError> {
        let kind = match &index.kind {
            IndexDeclKind::Named { variants } => {
                let unique =
                    NonEmptyUnique::try_from_non_empty(variants.map_ref(|v| v.value.clone()))
                        .map_err(|DuplicateItemError { first, duplicate }| {
                            SemanticError::located(
                                src,
                                variants[duplicate].span,
                                NameError::DuplicateName {
                                    name: variants[duplicate]
                                        .value
                                        .qualified_by(&index.name.value)
                                        .to_string(),
                                    first: variants[first].span,
                                },
                            )
                        })?;
                IndexKind::Concrete(ConcreteIndexKind::Named { variants: unique })
            }
            IndexDeclKind::Range { start, end, step } => {
                let units = self.coordinate_units(owner, [&**start, &**end, &**step])?;
                let resolve = |unit: &UnitExpr| resolve_unit_expr_with(unit, |r| units.get(r));
                let axis = lower_coordinate_expr(start, &resolve)
                    .and_then(|start| {
                        Ok(CoordinateAxisExpr::Range {
                            start,
                            end: lower_coordinate_expr(end, &resolve)?,
                            step: lower_coordinate_expr(step, &resolve)?,
                        })
                    })
                    .map_err(|error| const_expr_error(error, src))?;
                self.coordinate_index(owner, &index.name.value, &axis, src, decl_span)?
            }
            IndexDeclKind::Linspace { start, end, points } => {
                let units = self.coordinate_units(owner, [&**start, &**end])?;
                let resolve = |unit: &UnitExpr| resolve_unit_expr_with(unit, |r| units.get(r));
                let axis = lower_coordinate_expr(start, &resolve)
                    .and_then(|start| {
                        Ok(CoordinateAxisExpr::Linspace {
                            start,
                            end: lower_coordinate_expr(end, &resolve)?,
                            points: lower_static_nat_expr(points)?,
                        })
                    })
                    .map_err(|error| const_expr_error(error, src))?;
                self.coordinate_index(owner, &index.name.value, &axis, src, decl_span)?
            }
            IndexDeclKind::RequiredNamed => IndexKind::Required(RequiredIndexKind::Named),
            IndexDeclKind::RequiredCoordinate { dimension } => {
                let dim = self
                    .evaluate_dim_expr(owner, dimension, &HashMap::new(), &PortView::Default, None)
                    .map_err(|failure| dim_expr_error(failure, src, dimension.span))?;
                IndexKind::Required(RequiredIndexKind::Coordinate { dimension: dim })
            }
        };
        Ok(IndexDef {
            name: IndexBindingTarget::Declared(index.name.value.clone()),
            kind,
        })
    }

    /// Evaluate every unit written in the quantity literals of coordinate
    /// bounds, keyed by its written reference.
    fn coordinate_units<const N: usize>(
        &mut self,
        owner: &DagId,
        exprs: [&ast::Expr; N],
    ) -> Result<HashMap<UnitRef, UnitInfo>, SemanticError> {
        fn unit_exprs<'e>(expr: &'e ast::Expr, found: &mut Vec<&'e UnitExpr>) {
            match &expr.kind {
                ast::ExprKind::QuantityLiteral { unit, .. } => found.push(unit),
                ast::ExprKind::UnaryOp { operand, .. } => unit_exprs(operand, found),
                ast::ExprKind::BinOp { lhs, rhs, .. } => {
                    unit_exprs(lhs, found);
                    unit_exprs(rhs, found);
                }
                _ => {}
            }
        }

        let mut found = Vec::new();
        for expr in exprs {
            unit_exprs(expr, &mut found);
        }
        let mut infos = HashMap::new();
        for term in found.into_iter().flat_map(|unit| &unit.terms) {
            if let Some(target) = self.resolve_unit(owner, &term.name.value) {
                infos.insert(term.name.value.clone(), self.unit(&target)?);
            }
        }
        Ok(infos)
    }

    /// Evaluate a lowered coordinate axis into an index kind.
    fn coordinate_index(
        &self,
        owner: &DagId,
        name: &IndexName,
        axis: &CoordinateAxisExpr,
        src: SourceId,
        decl_span: Span,
    ) -> Result<IndexKind, SemanticError> {
        let dimension_mismatch = |mismatch: CoordinateArgumentDimensions| {
            SemanticError::located(
                src,
                decl_span,
                IndexError::CoordinateIndexDimensionMismatch {
                    name: name.clone(),
                    mismatch,
                },
            )
        };
        axis.evaluate()
            .map(|data| IndexKind::Concrete(ConcreteIndexKind::Coordinate(data)))
            .map_err(|error| match error {
                CoordinateAxisError::Expr(error) => const_expr_error(error, src),
                CoordinateAxisError::RangeDimensionMismatch { start, end, step } => {
                    dimension_mismatch(CoordinateArgumentDimensions::Range {
                        start: self.dimension_spelling(owner, &start),
                        end: self.dimension_spelling(owner, &end),
                        step: self.dimension_spelling(owner, &step),
                    })
                }
                CoordinateAxisError::LinspaceDimensionMismatch { start, end } => {
                    dimension_mismatch(CoordinateArgumentDimensions::Linspace {
                        start: self.dimension_spelling(owner, &start),
                        end: self.dimension_spelling(owner, &end),
                    })
                }
                CoordinateAxisError::Invalid { error, point_count } => SemanticError::located(
                    src,
                    point_count.unwrap_or(decl_span),
                    IndexError::CoordinateIndexInvalid {
                        name: name.clone(),
                        error,
                    },
                ),
            })
    }
}

/// A resolver symbol has no valid definition source: the evaluator was not
/// given the declaring module, or the symbol belongs to another module.
fn missing_definition_error(detail: &str, src: SourceId) -> SemanticError {
    SemanticError::internal_error(
        format!("resolved definition has no owned source declaration: {detail}"),
        src,
        crate::diagnostic_anchor::DiagnosticAnchor::WholeFile,
    )
}

/// Render a dimension-expression failure at its declaration.
fn dim_expr_error(failure: DimExprFailure, src: SourceId, span: Span) -> SemanticError {
    match failure {
        DimExprFailure::Unknown(name) => SemanticError::located(
            src,
            span,
            DimensionError::UnknownDimension {
                name: name.to_name_path(),
            },
        ),
        DimExprFailure::Overflow => {
            SemanticError::located(src, span, DimensionError::DimensionOverflow)
        }
        DimExprFailure::Definition(error) => *error,
    }
}

fn scale_error(
    site: UnitScaleSite,
    error: PositiveFiniteScaleError,
    src: SourceId,
    span: Span,
) -> SemanticError {
    SemanticError::located(src, span, DimensionError::InvalidUnitScale { site, error })
}

/// Convert a typed unit-resolution failure into a spanned diagnostic.
fn unit_resolve_error(err: UnitResolveError, src: SourceId, span: Span) -> SemanticError {
    match err {
        UnitResolveError::UnknownUnit(name) => {
            SemanticError::located(src, span, DimensionError::UnknownUnit { name })
        }
        UnitResolveError::DynamicScale(name) => SemanticError::located(
            src,
            span,
            DimensionError::DynamicUnitScaleNotAllowed { name },
        ),
        UnitResolveError::InvalidScale(err) => scale_error(UnitScaleSite::Compound, err, src, span),
        UnitResolveError::Overflow(_) => {
            SemanticError::located(src, span, DimensionError::DimensionOverflow)
        }
    }
}

/// Render a constant-expression failure at the definition boundary.
fn const_expr_error(error: ConstExprError, src: SourceId) -> SemanticError {
    match error {
        ConstExprError::Unit { error, span } => unit_resolve_error(error, src, span),
        ConstExprError::DimensionOverflow { span } => {
            SemanticError::located(src, span, DimensionError::DimensionOverflow)
        }
        error => SemanticError::located(
            src,
            error.span(),
            DimensionError::InvalidConstantExpression { error },
        ),
    }
}

/// Reject every invalid concrete `Fin(N)` cardinality written in a type
/// position or finite comprehension of one module.
fn validate_structural_finite_indexes(
    declarations: &[Declaration],
    src: SourceId,
) -> Result<(), SemanticError> {
    for decl in declarations {
        match &decl.kind {
            DeclKind::Param(d) => {
                validate_type_expr_finite_indexes(&d.type_ann, src)?;
                if let Some(value) = &d.value {
                    validate_expr_finite_indexes(value, src)?;
                }
            }
            DeclKind::Node(d) => {
                validate_type_expr_finite_indexes(&d.type_ann, src)?;
                d.definition.formula().map_or(Ok(()), |expression| {
                    validate_expr_finite_indexes(expression, src)
                })?;
            }
            DeclKind::ConstNode(d) => {
                validate_type_expr_finite_indexes(&d.type_ann, src)?;
                validate_expr_finite_indexes(&d.value, src)?;
            }
            DeclKind::Type(d) => {
                for default in d
                    .generic_params
                    .iter()
                    .filter_map(|param| param.default.as_ref())
                {
                    validate_generic_arg_finite_indexes(default, src)?;
                }
                if let ast::TypeDeclBody::Constructors(members) = &d.body {
                    for field in members
                        .iter()
                        .filter_map(|member| member.payload.as_ref())
                        .flatten()
                    {
                        validate_type_expr_finite_indexes(&field.type_ann, src)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn concrete_nat_value(expr: &ast::NatExpr, src: SourceId) -> Result<Option<u64>, SemanticError> {
    match expr {
        ast::NatExpr::Literal(value, _) => Ok(Some(*value)),
        ast::NatExpr::Var(_) => Ok(None),
        ast::NatExpr::Add(operands, span) => {
            operands.iter().try_fold(Some(0_u64), |sum, operand| {
                match (sum, concrete_nat_value(operand, src)?) {
                    (Some(sum), Some(value)) => sum.checked_add(value).map(Some).ok_or_else(|| {
                        SemanticError::located(
                            src,
                            *span,
                            IndexError::FinCardinalityAdditionOverflow,
                        )
                    }),
                    _ => Ok(None),
                }
            })
        }
        ast::NatExpr::Mul(operands, span) => {
            operands.iter().try_fold(Some(1_u64), |product, operand| {
                match (product, concrete_nat_value(operand, src)?) {
                    (Some(product), Some(value)) => {
                        product.checked_mul(value).map(Some).ok_or_else(|| {
                            SemanticError::located(
                                src,
                                *span,
                                IndexError::FinCardinalityMultiplicationOverflow,
                            )
                        })
                    }
                    _ => Ok(None),
                }
            })
        }
    }
}

fn validate_finite_cardinality(
    cardinality: u64,
    span: Span,
    src: SourceId,
) -> Result<(), SemanticError> {
    FiniteIndex::try_from_u64(cardinality)
        .map(|_| ())
        .map_err(|error| {
            SemanticError::located(
                src,
                span,
                IndexError::InvalidFiniteIndexCardinality { error },
            )
        })
}

fn validate_index_expr_finite_indexes(
    index: &ast::IndexExpr,
    src: SourceId,
) -> Result<(), SemanticError> {
    match index {
        ast::IndexExpr::Finite { cardinality, span } => {
            if let Some(value) = concrete_nat_value(cardinality, src)? {
                validate_finite_cardinality(value, *span, src)?;
            }
        }
        ast::IndexExpr::Name(_) | ast::IndexExpr::BareNat(_) => {}
    }
    Ok(())
}

fn validate_generic_arg_finite_indexes(
    arg: &ast::GenericArg,
    src: SourceId,
) -> Result<(), SemanticError> {
    match arg {
        ast::GenericArg::Type(type_expr) => validate_type_expr_finite_indexes(type_expr, src),
        ast::GenericArg::Index(index) => validate_index_expr_finite_indexes(index, src),
        ast::GenericArg::Nat(_) | ast::GenericArg::Ambiguous(_) => Ok(()),
    }
}

fn validate_type_expr_finite_indexes(
    type_expr: &ast::TypeExpr,
    src: SourceId,
) -> Result<(), SemanticError> {
    match &type_expr.kind {
        ast::TypeExprKind::Indexed { base, indexes } => {
            validate_type_expr_finite_indexes(base, src)?;
            for index in indexes {
                validate_index_expr_finite_indexes(index, src)?;
            }
        }
        ast::TypeExprKind::TypeApplication { generic_args, .. } => {
            for arg in generic_args {
                validate_generic_arg_finite_indexes(arg, src)?;
            }
        }
        ast::TypeExprKind::DatetimeApplication { type_args } => {
            for arg in type_args {
                validate_type_expr_finite_indexes(arg, src)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Reject invalid concrete `Fin(N)` identities used by comprehensions and tables.
fn validate_expr_finite_indexes(expr: &ast::Expr, src: SourceId) -> Result<(), SemanticError> {
    use crate::syntax::visitor::ExprVisitor;

    struct FiniteValidator {
        src: SourceId,
    }

    impl ExprVisitor<crate::syntax::phase::Desugared> for FiniteValidator {
        type Error = SemanticError;

        fn visit_expr(&mut self, expr: &ast::Expr) -> Result<(), SemanticError> {
            match &expr.kind {
                ast::ExprKind::ForComp { bindings, .. } => {
                    for binding in bindings {
                        if let ast::ForBindingIndex::Finite { cardinality, span } = &binding.index
                            && let Some(value) = concrete_nat_value(cardinality, self.src)?
                        {
                            validate_finite_cardinality(value, *span, self.src)?;
                        }
                    }
                }
                ast::ExprKind::MapLiteral { entries } => {
                    for entry in entries {
                        for key in &entry.keys {
                            if let crate::syntax::ast::MapEntryKey::Finite {
                                axis_span,
                                position,
                            } = key
                            {
                                validate_finite_cardinality(
                                    position.value.cardinality(),
                                    *axis_span,
                                    self.src,
                                )?;
                            }
                        }
                    }
                }
                _ => {}
            }
            self.dispatch(expr)
        }
    }

    FiniteValidator { src }.visit_expr(expr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::PreludeBaseDimension;
    use crate::resolve::builder::TestModules;
    use crate::semantic_error::SemanticErrorKind;
    use crate::syntax::names::NamePath;
    use crate::syntax::parser::Parser;

    fn parse(source: &str) -> ast::File {
        ast::File::from(Parser::new(source).parse_file().unwrap())
    }

    fn src(name: &str, source: &str) -> SourceId {
        crate::source_registry::SourceRegistry::new()
            .register(name, std::sync::Arc::new(source.to_string()))
    }

    fn base(dimension: PreludeBaseDimension) -> Dimension {
        Dimension::base(BaseDimId::Prelude(dimension))
    }

    /// Modules of one test project: identity, parsed body, and source.
    struct Project {
        modules: Vec<(DagId, ast::File, SourceId)>,
    }

    impl Project {
        fn new(modules: &[(&str, &str)]) -> Self {
            Self {
                modules: modules
                    .iter()
                    .map(|(name, source)| {
                        (
                            DagId::root_in_package("test", *name),
                            parse(source),
                            src(name, source),
                        )
                    })
                    .collect(),
            }
        }

        fn id(name: &str) -> DagId {
            DagId::root_in_package("test", name)
        }

        /// Resolve every import/include path by its last segment.
        fn resolver(&self) -> ModuleResolver {
            let mut modules = TestModules::default();
            for (owner, file, _) in &self.modules {
                modules.add(owner.clone(), &file.declarations);
                for declaration in &file.declarations {
                    let path = match &declaration.kind {
                        DeclKind::Import(import) => import.path(),
                        DeclKind::Include(include) => &include.path,
                        _ => continue,
                    };
                    let target = Self::id(path.leaf().name.as_str());
                    modules.edge(owner, path, &target);
                }
            }
            modules.build().unwrap()
        }

        fn evaluator<'a>(&'a self, resolver: &'a ModuleResolver) -> StaticDefinitionEvaluator<'a> {
            StaticDefinitionEvaluator::new(
                resolver,
                self.modules.iter().map(|(owner, file, src)| {
                    (
                        owner.clone(),
                        DefinitionSource {
                            declarations: &file.declarations,
                            src: *src,
                        },
                    )
                }),
                self.modules.first().map_or_else(
                    || {
                        crate::source_registry::SourceRegistry::new()
                            .register("<empty>", std::sync::Arc::new(String::new()))
                    },
                    |(_, _, src)| *src,
                ),
            )
            .unwrap()
        }
    }

    fn dimension_of(
        evaluator: &mut StaticDefinitionEvaluator<'_>,
        owner: &DagId,
        name: &str,
    ) -> Result<Dimension, SemanticError> {
        let identity = evaluator
            .resolve_dimension(owner, &DimRef::local(DimName::expect_valid(name)))
            .unwrap_or_else(|| panic!("`{name}` must resolve"));
        evaluator.dimension(&identity)
    }

    #[test]
    fn definitions_resolve_imported_names_through_the_resolver() {
        let project = Project::new(&[
            (
                "lib",
                "pub dim Rate = Length / Time;\npub const unit furl: Length = 200.0 m;",
            ),
            (
                "main",
                "import lib::{dim Rate as R, unit furl};\n\
                 dim Twice = R * Mass;\n\
                 const unit two: Length = 2.0 furl;",
            ),
        ]);
        let resolver = project.resolver();
        let mut evaluator = project.evaluator(&resolver);
        let main = Project::id("main");

        let rate = (base(PreludeBaseDimension::Length) / base(PreludeBaseDimension::Time)).unwrap();
        let twice = (&rate * &base(PreludeBaseDimension::Mass)).unwrap();
        assert_eq!(
            dimension_of(&mut evaluator, &main, "Twice").ok(),
            Some(twice)
        );
        let definitions = evaluator.module_definitions(&main).unwrap().definitions;
        let (_, two) = definitions
            .units()
            .find(|(identity, _)| identity.as_str() == "two")
            .unwrap();
        assert_eq!(
            two.scale.static_scale().map(PositiveFiniteScale::get),
            Some(400.0)
        );
        // Only the module's own declarations are its definitions.
        assert_eq!(definitions.dimensions().count(), 1);
    }

    #[test]
    fn a_reference_closing_a_cycle_names_the_declaration_being_evaluated() {
        let project = Project::new(&[(
            "main",
            "dim Foo = Length;\ndim Bar = Foo * Baz;\ndim Baz = Bar;",
        )]);
        let resolver = project.resolver();
        let mut evaluator = project.evaluator(&resolver);

        let error = evaluator
            .module_definitions(&Project::id("main"))
            .unwrap_err();
        assert!(
            matches!(&error, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::CyclicDimension { name, .. }), .. }) if name.as_str() == "Baz"),
            "{error:?}"
        );
    }

    #[test]
    fn unknown_references_are_reported_at_the_declaration() {
        let project = Project::new(&[("main", "dim Foo = Length * Missing;")]);
        let resolver = project.resolver();
        let mut evaluator = project.evaluator(&resolver);

        let error = evaluator
            .module_definitions(&Project::id("main"))
            .unwrap_err();
        assert!(
            matches!(&error, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::UnknownDimension { name, .. }), .. }) if name.to_string() == "Missing"),
            "{error:?}"
        );
    }

    #[test]
    fn projected_dimension_substitutes_bound_ports_only() {
        let project = Project::new(&[
            (
                "lib",
                "pub(bind) dim Q;\npub(bind) dim P;\npub dim QR = Q * P / Time;\nparam q: Q;",
            ),
            (
                "main",
                "include lib(dim Q: Mass, q: 1.0 kg)::{dim QR as R};",
            ),
        ]);
        let resolver = project.resolver();
        let mut evaluator = project.evaluator(&resolver);
        let lib = Project::id("lib");

        let projected = dimension_of(&mut evaluator, &Project::id("main"), "R").unwrap();
        let unbound_port = dimension_of(&mut evaluator, &lib, "P").unwrap();
        let expected = (&(&base(PreludeBaseDimension::Mass) * &unbound_port).unwrap()
            / &base(PreludeBaseDimension::Time))
            .unwrap();
        assert_eq!(projected, expected);
    }

    #[test]
    fn defaulted_ports_are_opaque_only_in_their_module_port_generic_view() {
        let project = Project::new(&[
            (
                "lib",
                "pub(bind) dim Q = Length;\npub(bind) dim P = Mass;\n\
                 pub dim QR = Q * P / Time;\npub dim Fixed = Length;\nparam q: Q;",
            ),
            ("main", "include lib(dim Q: Time, q: 1.0 s)::{dim QR as R};"),
        ]);
        let resolver = project.resolver();
        let mut evaluator = project.evaluator(&resolver);
        let lib = Project::id("lib");
        let identity = |evaluator: &StaticDefinitionEvaluator<'_>, name: &str| {
            evaluator
                .resolve_dimension(&lib, &DimRef::local(DimName::expect_valid(name)))
                .unwrap()
        };
        let q = identity(&evaluator, "Q");
        let p = identity(&evaluator, "P");
        let qr = identity(&evaluator, "QR");
        let fixed = identity(&evaluator, "Fixed");
        let opaque =
            |identity: &ResolvedDimName| Dimension::base(BaseDimId::UserDefined(identity.clone()));

        // The default view expands every port.
        let length = base(PreludeBaseDimension::Length);
        let mass = base(PreludeBaseDimension::Mass);
        let time = base(PreludeBaseDimension::Time);
        assert_eq!(
            evaluator.dimension(&qr).unwrap(),
            (&(&length * &mass).unwrap() / &time).unwrap()
        );
        // The port-generic view keeps both ports opaque.
        assert_eq!(evaluator.port_generic_dimension(&q).unwrap(), opaque(&q));
        assert_eq!(
            evaluator.port_generic_dimension(&qr).unwrap(),
            (&(&opaque(&q) * &opaque(&p)).unwrap() / &time).unwrap()
        );
        assert_eq!(evaluator.port_generic_dimension(&fixed).unwrap(), length);
        // A projection substitutes the bound port and defaults the other.
        assert_eq!(
            dimension_of(&mut evaluator, &Project::id("main"), "R").unwrap(),
            mass
        );
        // Only differing values are recorded, ports as their own bases.
        let definitions = evaluator.module_definitions(&lib).unwrap().definitions;
        let recorded = definitions
            .port_generic_dimensions()
            .map(|(identity, _)| identity.clone())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(recorded, [q, p, qr].into_iter().collect());
    }

    #[test]
    fn dimension_overrides_replace_or_hide_resolved_references() {
        let project = Project::new(&[("lib", "pub(bind) dim Q;\nindex Axis: Q;")]);
        let resolver = project.resolver();
        let mut evaluator = project.evaluator(&resolver);
        let lib = Project::id("lib");
        let q = evaluator
            .resolve_dimension(&lib, &DimRef::local(DimName::expect_valid("Q")))
            .unwrap();
        let expr = DimExpr {
            terms: vec![ast::DimExprItem {
                op: ast::MulDivOp::Mul,
                term: ast::DimTerm {
                    name: Spanned::new(NamePath::expect_local("Q"), Span::new(0, 1)),
                    power: None,
                    span: Span::new(0, 1),
                },
            }],
            span: Span::new(0, 1),
        };

        let length = base(PreludeBaseDimension::Length);
        let bound = HashMap::from([(q.clone(), Some(length.clone()))]);
        assert_eq!(
            evaluator
                .evaluate_dim_expr_with_overrides(&lib, &expr, &bound)
                .ok(),
            Some(length)
        );
        let hidden = HashMap::from([(q, None)]);
        assert!(matches!(
            evaluator.evaluate_dim_expr_with_overrides(&lib, &expr, &hidden),
            Err(DimExprFailure::Unknown(name)) if name.to_string() == "Q"
        ));
    }

    #[test]
    fn runtime_units_do_not_cross_an_import_alias() {
        let project = Project::new(&[
            (
                "lib",
                "pub const unit furl: Length = 200.0 m;\n\
                 param factor: Dimensionless = 2.0;\n\
                 pub unit dyn: Length = (@factor) m;",
            ),
            ("main", "import lib as l;"),
        ]);
        let resolver = project.resolver();
        let evaluator = project.evaluator(&resolver);
        let main = Project::id("main");
        let qualified = |name: &str| {
            UnitRef::qualified(
                crate::syntax::non_empty::NonEmpty::singleton(
                    crate::syntax::names::NameAtom::parse("l").unwrap(),
                ),
                UnitName::expect_valid(name),
            )
        };

        assert!(evaluator.resolve_unit(&main, &qualified("furl")).is_some());
        assert!(evaluator.resolve_unit(&main, &qualified("dyn")).is_none());
    }

    #[test]
    fn base_units_register_canonical_units_and_reject_a_second_one() {
        let project = Project::new(&[
            ("lib", "pub base dim Money;\npub base unit USD: Money;"),
            ("main", "import lib::{dim Money};\nbase unit EUR: Money;"),
        ]);
        let resolver = project.resolver();
        let mut evaluator = project.evaluator(&resolver);

        let lib_definitions = evaluator
            .module_definitions(&Project::id("lib"))
            .unwrap()
            .definitions;
        assert!(
            lib_definitions
                .base_dimensions()
                .any(|(_, info)| info.canonical_unit().map(UnitName::as_str) == Some("USD"))
        );
        let error = evaluator
            .module_definitions(&Project::id("main"))
            .unwrap_err();
        assert!(
            matches!(&error, SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::InvalidBaseUnitDeclaration { rejection: BaseUnitRejection::CanonicalUnitTaken { existing }, .. }), .. }) if existing.as_str() == "USD"),
            "{error:?}"
        );
    }

    #[test]
    fn display_dimensions_include_alias_qualified_exports_and_the_prelude() {
        let project = Project::new(&[
            ("lib", "pub dim Rate = Length / Time;\ndim Hidden = Mass;"),
            ("main", "import lib as l;\ndim Own = Mass;"),
        ]);
        let resolver = project.resolver();
        let mut evaluator = project.evaluator(&resolver);
        let spellings = evaluator
            .display_dimensions(&Project::id("main"))
            .unwrap()
            .into_iter()
            .map(|(spelling, _)| spelling.to_string())
            .collect::<HashSet<_>>();

        assert!(spellings.contains("l::Rate"));
        assert!(spellings.contains("Own"));
        assert!(spellings.contains("Velocity"));
        assert!(!spellings.iter().any(|spelling| spelling.contains("Hidden")));
    }

    #[test]
    fn dynamic_unit_scales_are_recorded_for_their_module() {
        let project = Project::new(&[(
            "main",
            "param factor: Dimensionless = 2.0;\nunit dyn: Length = (@factor) m;",
        )]);
        let resolver = project.resolver();
        let mut evaluator = project.evaluator(&resolver);

        let module = evaluator.module_definitions(&Project::id("main")).unwrap();
        assert_eq!(module.dynamic_unit_scales.len(), 1);
        assert!(matches!(
            module
                .definitions
                .units()
                .next()
                .map(|(_, info)| &info.scale),
            Some(UnitScale::Dynamic { .. })
        ));
    }

    #[test]
    fn finite_index_cardinalities_are_validated() {
        let project = Project::new(&[("main", "param v: Dimensionless[Fin(0)];")]);
        let resolver = project.resolver();
        let mut evaluator = project.evaluator(&resolver);

        assert!(matches!(
            evaluator.module_definitions(&Project::id("main")),
            Err(SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::InvalidFiniteIndexCardinality { .. }),
                ..
            }))
        ));
    }
}
