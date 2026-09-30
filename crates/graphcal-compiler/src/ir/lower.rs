//! Per-DAG HIR construction from a desugared syntax tree.
//!
//! `lower()` combines declaration collection (`resolve`), canonical
//! evaluation of the module's dimensions, units, and indexes, and nominal
//! type collection into one [`HirDag`]. Reference resolution happens at
//! [`UnfrozenIR::freeze`], which lowers every assembled declaration body to
//! HIR — a frozen DAG carries no syntax-AST expression.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::{DeclKind, Expr, File, TypeExpr};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::dimension::Dimension;
use crate::ir::instance::InstanceRecord;
use crate::ir::module_interface::ModuleInterface;
use crate::ir::resolve::{CollectedFile, ImportedValueNames, resolve_with_imported_values};
use crate::plot_visibility::PlotVisibility;
use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::ExternalDeclSurface;
use crate::registry::resolve_types::ParsedExpectedFail;
use crate::resolved_name::{ResolvedDeclName, ResolvedUnitName};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{DimRef, UnitName, UnitRef};
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;

use super::decl_table::DeclTable;
use super::entry::{self, BodyPhase, Syntax};
#[cfg(test)]
use super::extern_fns::resolve_extern_struct_return;
pub use super::extern_fns::{ExternFunctionEntry, ExternStructResult};
pub use super::include::{IncludeOverrideReconciliations, SemanticInstanceInput};
use super::module_definitions::{ModuleDefinitions, StaticDefinitions};

use super::static_definitions::StaticDefinitionEvaluator;

// ---------------------------------------------------------------------------
// Entry types for IR declarations
// ---------------------------------------------------------------------------

/// One plot declaration's expressions lowered to HIR, in source order.
#[derive(Debug, Clone, Default)]
pub struct LoweredPlotBody {
    /// Encoding channel expressions (`x: ...`, `y: ...`).
    pub encodings: Vec<(crate::syntax::ast::EncodingChannel, crate::hir::CheckedExpr)>,
    /// Mark property expressions (`stroke_width: ...`).
    pub mark_properties: Vec<LoweredPlotField>,
    /// Plot-level property expressions (`title: ...`).
    pub properties: Vec<LoweredPlotField>,
}

/// Typed classification of a plot, mark, or composition property name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoweredPlotProperty {
    Mark(crate::plot_props::MarkProperty),
    Plot(crate::plot_props::PlotProperty),
    Composition(crate::plot_props::CompositionProperty),
    Unknown(crate::syntax::ast::PlotPropertyName),
}

impl LoweredPlotProperty {
    pub(super) fn mark(name: crate::syntax::ast::PlotPropertyName) -> Self {
        crate::plot_props::MarkProperty::from_name(name.as_str())
            .map(Self::Mark)
            .unwrap_or(Self::Unknown(name))
    }

    pub(super) fn plot(name: crate::syntax::ast::PlotPropertyName) -> Self {
        crate::plot_props::PlotProperty::from_name(name.as_str())
            .map(Self::Plot)
            .unwrap_or(Self::Unknown(name))
    }

    pub(super) fn composition(name: crate::syntax::ast::PlotPropertyName) -> Self {
        crate::plot_props::CompositionProperty::from_name(name.as_str())
            .map(Self::Composition)
            .unwrap_or(Self::Unknown(name))
    }

    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Mark(property) => property.name(),
            Self::Plot(property) => property.name(),
            Self::Composition(property) => property.name(),
            Self::Unknown(name) => name.as_str(),
        }
    }
}

/// A typed plot/figure/layer field expression lowered to HIR.
#[derive(Debug, Clone)]
pub struct LoweredPlotField {
    pub property: LoweredPlotProperty,
    /// Span of the property name in the source, for validation diagnostics.
    pub(crate) name_span: crate::syntax::span::Span,
    pub value: crate::hir::CheckedExpr,
}

/// Unresolved expected-fail metadata with the scope that authored it.
///
/// Include assembly may rename the assertion key, but the attribute's index
/// paths and spans remain owned by the file where the attribute was written.
#[derive(Debug, Clone)]
pub(crate) struct ParsedExpectedFailMetadata {
    pub(crate) expected: ParsedExpectedFail,
    pub(crate) resolution_owner: crate::dag_id::DagId,
    pub(crate) attribute_span: Span,
}

impl ParsedExpectedFailMetadata {
    /// Resolve every named index key in the scope that authored the attribute.
    pub(super) fn resolve(
        self,
        resolver: &crate::resolve::ModuleResolver,
        src: &NamedSource<Arc<String>>,
    ) -> Result<ResolvedExpectedFailMetadata, GraphcalError> {
        use crate::assertion_expectation::{ExpectedFail, ExpectedFailKeyPart};

        let Self {
            expected,
            resolution_owner,
            attribute_span,
        } = self;
        let expected = match expected {
            ExpectedFail::All => ExpectedFail::All,
            ExpectedFail::Variants(keys) => ExpectedFail::Variants(keys.try_map(|key| {
                key.into_iter()
                    .map(|part| match part {
                        ExpectedFailKeyPart::Named {
                            index,
                            variant,
                            span,
                        } => resolver
                            .resolve_index_variant_parts(&resolution_owner, &index, &variant)
                            .map(|resolved| ExpectedFailKeyPart::resolved(resolved, span))
                            .map_err(|err| GraphcalError::EvalError {
                                message: err.to_string(),
                                src: src.clone(),
                                span: span.into(),
                            }),
                        ExpectedFailKeyPart::FinitePosition { position, span } => {
                            Ok(ExpectedFailKeyPart::FinitePosition { position, span })
                        }
                    })
                    .collect::<Result<_, GraphcalError>>()
            })?),
        };
        Ok(ResolvedExpectedFailMetadata {
            expected,
            attribute_span,
        })
    }
}

/// Expected-fail metadata whose index keys were resolved at the freeze
/// boundary in the scope that authored the attribute.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedExpectedFailMetadata {
    pub(crate) expected: crate::assertion_expectation::ExpectedFail,
    pub(crate) attribute_span: Span,
}

/// Frozen phase: every body is strictly lowered HIR with canonical references.
#[derive(Debug, Clone, Copy)]
pub enum Lowered {}

impl BodyPhase for Lowered {
    type Expr = crate::hir::CheckedExpr;
    type TypeAnnotation = crate::hir::TypeAnnotation;
    type NodeDefinition = crate::hir::node_definition::NodeDefinition;
    type AssertBody = crate::hir::CheckedAssertBody;
    type PlotBody = LoweredPlotBody;
    type CompositionFields = Vec<LoweredPlotField>;
    type UnitIdentity = ResolvedUnitName;
}

/// A lowered value, assertion, or visualization declaration.
pub type HirDecl = entry::Decl<Lowered>;
/// A const declaration with type annotation and lowered body.
pub type ConstEntry = entry::ConstEntry<Lowered>;
/// A param declaration with type annotation and an atomic lowered default.
pub type ParamEntry = entry::ParamEntry<Lowered>;
/// A node declaration with type annotation and lowered body.
pub type NodeEntry = entry::NodeEntry<Lowered>;
/// An assert declaration with lowered body.
pub type AssertEntry = entry::AssertEntry<Lowered>;
/// A plot declaration with lowered body.
pub type PlotEntry = entry::PlotEntry<Lowered>;
/// A figure declaration with lowered fields.
pub type FigureEntry = entry::FigureEntry<Lowered>;
/// A layer declaration with lowered fields.
pub type LayerEntry = entry::LayerEntry<Lowered>;
/// A validated dynamic unit scale lowered to HIR.
pub type DynamicUnitScaleEntry = entry::DynamicUnitScaleEntry<Lowered>;

/// A plot alias brought into this DAG by an include brace list (#847).
///
/// The plot itself is evaluated in its owning instance; this entry only
/// makes the alias known to the DAG so figures/layers can reference it and
/// duplicate-name checks see it.
#[derive(Debug, Clone)]
pub struct IncludedPlotEntry {
    /// The local alias the plot is visible under.
    pub name: ScopedName,
}

/// A plot requested by an include brace list item (#847).
#[derive(Debug, Clone)]
pub struct RequestedPlot {
    /// The local alias the plot enters the root namespace under.
    pub alias: DeclName,
    /// Composition-only when the include item carried `#[hidden]`.
    pub visibility: PlotVisibility,
}

/// Intermediate Representation produced by [`lower`].
///
/// Contains everything downstream stages need:
/// - The module's owner-qualified dimension, unit, index, and nominal definitions
/// - Declarations (consts, params, nodes) with their expressions
/// - Dependency graphs for const and runtime evaluation ordering
/// - Source-order tracking for deterministic output
#[derive(Debug)]
pub struct HirDag {
    /// Canonical identity carried by the body itself, so storage and consumers
    /// cannot pair this HIR with a different DAG key.
    pub(super) dag_id: crate::dag_id::DagId,
    /// Every dimension, unit, index, and nominal definition this DAG owns,
    /// keyed by canonical identity. Syntax-backed nominal definitions are
    /// unrepresentable; its nominal types carry canonical HIR signatures.
    pub(super) definitions: ModuleDefinitions,
    /// Dimension spellings visible to this DAG, which diagnostics prefer over
    /// a base-dimension expansion.
    pub(crate) display_dimensions: Vec<(DimRef, Dimension)>,
    /// Value, assertion, and visualization declarations keyed by canonical
    /// identity, in source order.
    pub(crate) decls: DeclTable<Lowered>,
    /// Plot aliases from include brace lists (#847).
    pub(crate) included_plots: Vec<IncludedPlotEntry>,
    /// Runtime-interface-relevant declarations authored directly in this DAG,
    /// excluding declarations merged from includes.
    pub(super) source_declarations: Vec<crate::hir::SourceDeclaration>,
    /// Typed Static ports authored directly in this reusable DAG.
    pub(crate) static_ports: Vec<crate::hir::StaticPort>,
    /// Mapping from each assertion to the declarations that assume it.
    pub(crate) assumes_map: HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>,
    /// Expected-fail configurations with resolved index keys, per assertion.
    pub(crate) expected_fail: HashMap<ResolvedDeclName, ResolvedExpectedFailMetadata>,
    /// Strictly lowered, source-qualified dynamic unit scale definitions.
    pub(crate) dynamic_unit_scales: Vec<DynamicUnitScaleEntry>,
    /// Imported declarations keyed by their source-visible lexical binding.
    ///
    /// HIR retains only canonical targets. Checked types and optional values
    /// are attached atomically when this IR becomes TIR.
    pub(crate) imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    /// Resolved extern function signatures declared by `import plugin`
    /// blocks, keyed by canonical plugin identity plus function name.
    pub(crate) extern_functions: HashMap<crate::plugin_identity::ExternFnKey, ExternFunctionEntry>,
    /// Explicit exports and annotation-free `param` input ports, kept in
    /// distinct roles for downstream boundary checks.
    pub external_surface: ExternalDeclSurface,
    /// Semantic instance edges with importer-context value bindings.
    pub(crate) semantic_instances: Vec<crate::ir::instance::HirInstanceRecord>,
}

impl HirDag {
    /// Canonical identity of this HIR body.
    #[must_use]
    pub const fn dag_id(&self) -> &crate::dag_id::DagId {
        &self.dag_id
    }

    /// Value, assertion, and visualization declarations of this DAG.
    #[must_use]
    pub const fn decls(&self) -> &DeclTable<Lowered> {
        &self.decls
    }

    /// Nominal definitions canonically owned by this DAG.
    #[must_use]
    pub const fn nominal_types(&self) -> &crate::hir::NominalTypeRegistry {
        self.definitions.nominal_types()
    }

    /// Every definition canonically owned by this DAG.
    #[must_use]
    pub const fn definitions(&self) -> &ModuleDefinitions {
        &self.definitions
    }

    /// Diagnostic formatting services for this DAG, given the project's
    /// base-dimension metadata.
    #[must_use]
    pub fn formatting(
        &self,
        base_dimensions: std::collections::BTreeMap<
            crate::dimension::BaseDimId,
            crate::registry::types::BaseDimensionInfo,
        >,
    ) -> crate::registry::types::FormattingRegistry {
        crate::registry::types::FormattingRegistry::new(
            base_dimensions,
            self.display_dimensions.iter().cloned(),
        )
    }

    /// Borrow canonical lexical import targets resolved during HIR lowering.
    #[must_use]
    pub const fn imported_bindings(&self) -> &HashMap<ScopedName, ResolvedDeclName> {
        &self.imported_bindings
    }

    /// Runtime unit definitions after concrete include ownership has been
    /// assigned.
    #[must_use]
    pub fn dynamic_unit_scales(&self) -> &[DynamicUnitScaleEntry] {
        &self.dynamic_unit_scales
    }

    /// Declarations authored directly in this DAG that define its runtime
    /// parameter, output, and required-index interface.
    #[must_use]
    pub fn source_declarations(&self) -> &[crate::hir::SourceDeclaration] {
        &self.source_declarations
    }

    /// Typed Static interface authored directly in this DAG.
    #[must_use]
    pub fn static_ports(&self) -> &[crate::hir::StaticPort] {
        &self.static_ports
    }

    /// Extern signatures declared by this DAG's own `import plugin` blocks,
    /// keyed by canonical plugin identity plus function name.
    #[must_use]
    pub const fn extern_functions(
        &self,
    ) -> &HashMap<crate::plugin_identity::ExternFnKey, ExternFunctionEntry> {
        &self.extern_functions
    }
}

/// Lower an AST into a [`HirDag`].
///
/// This combines:
/// 1. Name resolution (`resolve`) — checks duplicates, extracts deps
/// 2. Registry construction — registers dimensions, units, indexes, structs from declarations
/// 3. Function registration — registers user-defined functions into the registry
///
/// # Errors
///
/// Returns a [`GraphcalError`] if declaration collection or registry construction fails
/// (e.g., unknown dimension in a type annotation, duplicate names, etc.).
pub fn lower(ast: &File, src: &NamedSource<Arc<String>>) -> Result<HirDag, GraphcalError> {
    let dag_id = crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new(src.name()))
        .map_err(|error| {
            GraphcalError::internal_error(
                format!("invalid source name `{}`: {error}", src.name()),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
    // Declaration collection reports duplicate names before the resolver's
    // own tables are built.
    let interface = ModuleInterface::new(&ast.declarations);
    resolve_with_imported_values(
        ast,
        interface.declared_surface(),
        src,
        &ImportedValueNames::default(),
    )?;
    let resolver = single_module_resolver(ast, &dag_id, src)?;
    let mut definitions = definition_evaluator(
        &resolver,
        [(
            dag_id.clone(),
            super::static_definitions::DefinitionSource {
                declarations: &ast.declarations,
                src,
            },
        )],
        src,
    )?;
    let unresolved = lower_module_with_imported_bindings(
        ast,
        src,
        &ImportedValueNames::default(),
        HashMap::new(),
        &dag_id,
        &mut definitions,
    )?;
    unresolved.freeze(&dag_id, &mut definitions, src)
}

/// Create a definition evaluator over `sources`, reporting a broken prelude
/// at `src`.
///
/// # Errors
///
/// Returns an internal error only if the built-in prelude is inconsistent.
pub fn definition_evaluator<'a>(
    resolver: &'a crate::resolve::ModuleResolver,
    sources: impl IntoIterator<
        Item = (
            crate::dag_id::DagId,
            super::static_definitions::DefinitionSource<'a>,
        ),
    >,
    src: &NamedSource<Arc<String>>,
) -> Result<StaticDefinitionEvaluator<'a>, GraphcalError> {
    StaticDefinitionEvaluator::new(resolver, sources).map_err(|error| {
        GraphcalError::internal_error(
            format!("prelude failed to load: {error}"),
            src,
            DiagnosticAnchor::Builtin,
        )
    })
}

/// A file root and its inline DAG bodies lowered with one resolver, for
/// compiler-side tests without the project loader.
#[cfg(test)]
pub(crate) struct LoweredTestFile {
    pub(crate) root: HirDag,
    pub(crate) inline_dags: Vec<HirDag>,
    pub(crate) resolver: crate::resolve::ModuleResolver,
}

/// Lower a file root and each inline DAG body (without self-import
/// preprocessing) against one project-wide resolver.
#[cfg(test)]
pub(crate) fn lower_file_with_inline_dags_for_test(
    ast: &File,
    src: &NamedSource<Arc<String>>,
) -> Result<LoweredTestFile, GraphcalError> {
    let dag_id = crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new(src.name()))
        .map_err(|error| {
            GraphcalError::internal_error(
                format!("invalid source name `{}`: {error}", src.name()),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
    let interface = ModuleInterface::new(&ast.declarations);
    resolve_with_imported_values(
        ast,
        interface.declared_surface(),
        src,
        &ImportedValueNames::default(),
    )?;
    let dag_bodies = ast
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            DeclKind::Dag(dag) => Some((
                dag_id.inline_dag_child(dag.name.value.clone()),
                File {
                    declarations: dag.body.clone(),
                },
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut modules = crate::resolve::builder::TestModules::default();
    modules.add(dag_id.clone(), &ast.declarations);
    for (owner, body) in &dag_bodies {
        modules.add(owner.clone(), &body.declarations);
        for declaration in &body.declarations {
            if let DeclKind::Import(import) = &declaration.kind {
                modules.import(owner, import, &dag_id);
            }
        }
    }
    let resolver = modules.build().map_err(|error| {
        GraphcalError::internal_error(
            format!("test module resolver failed: {error}"),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    let (root, inline_dags) = {
        let mut definitions = definition_evaluator(
            &resolver,
            std::iter::once((
                dag_id.clone(),
                super::static_definitions::DefinitionSource {
                    declarations: &ast.declarations,
                    src,
                },
            ))
            .chain(dag_bodies.iter().map(|(owner, body)| {
                (
                    owner.clone(),
                    super::static_definitions::DefinitionSource {
                        declarations: &body.declarations,
                        src,
                    },
                )
            })),
            src,
        )?;
        let unresolved = lower_module_with_imported_bindings(
            ast,
            src,
            &ImportedValueNames::default(),
            HashMap::new(),
            &dag_id,
            &mut definitions,
        )?;
        let root = unresolved.freeze(&dag_id, &mut definitions, src)?;
        let inline_dags = dag_bodies
            .iter()
            .map(|(owner, body)| {
                let unresolved = lower_dag_module_with_imported_bindings(
                    body,
                    &ImportedValueNames::default(),
                    HashMap::new(),
                    src,
                    owner,
                    &mut definitions,
                )?;
                unresolved.freeze(owner, &mut definitions, src)
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;
        (root, inline_dags)
    };
    Ok(LoweredTestFile {
        root,
        inline_dags,
        resolver,
    })
}

/// Build a resolver covering only this file's own module.
///
/// Single-file lowering has no project loader, so imported modules are not
/// resolvable; bodies that reference them fail at the freeze boundary just
/// as they previously failed during type resolution.
fn single_module_resolver(
    ast: &File,
    dag_id: &crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::resolve::ModuleResolver, GraphcalError> {
    let mut tables = crate::resolve::builder::SymbolTables::default();
    tables
        .add_file(dag_id.clone(), &ast.declarations)
        .and_then(|()| {
            tables
                .scopes(&crate::resolve::builder::NoModuleTargets)?
                .freeze()
        })
        .map_err(|error| {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })
}

/// The Static ports `owner` declares, with the identities the resolver
/// declared for them.
fn collect_static_ports(
    ast: &File,
    owner: &crate::dag_id::DagId,
    resolver: &crate::resolve::ModuleResolver,
) -> Result<Vec<crate::hir::StaticPort>, crate::resolve::error::ModuleResolveError> {
    use crate::hir::StaticPortIdentity;
    ast.declarations
        .iter()
        .filter_map(|declaration| {
            let interface = crate::static_interface::static_interface(&declaration.kind)?;
            let identity = match &declaration.kind {
                DeclKind::Type(type_decl) => resolver
                    .declaration(owner, &type_decl.name.value)
                    .map(|symbol| StaticPortIdentity::Type(symbol.into_resolved())),
                DeclKind::BaseDimension(dimension) => resolver
                    .declaration(owner, &dimension.name.value)
                    .map(|symbol| StaticPortIdentity::Dimension(symbol.into_resolved())),
                DeclKind::Dimension(dimension) => resolver
                    .declaration(owner, &dimension.name.value)
                    .map(|symbol| StaticPortIdentity::Dimension(symbol.into_resolved())),
                DeclKind::Index(index) => resolver
                    .declaration(owner, &index.name.value)
                    .map(|symbol| StaticPortIdentity::Index(symbol.into_resolved())),
                _ => return None,
            };
            Some(identity.map(|identity| crate::hir::StaticPort {
                identity,
                role: interface.role(),
                span: declaration.span,
            }))
        })
        .collect()
}

fn collect_source_declarations(ast: &File) -> Vec<crate::hir::SourceDeclaration> {
    ast.declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            DeclKind::Param(param) => Some(crate::hir::SourceDeclaration::Parameter {
                name: param.name.value.clone(),
                span: declaration.span,
            }),
            DeclKind::Node(node) => Some(crate::hir::SourceDeclaration::Node {
                name: node.name.value.clone(),
                span: declaration.span,
            }),
            DeclKind::Index(index) => Some(crate::hir::SourceDeclaration::Index {
                name: index.name.value.clone(),
                span: declaration.span,
            }),
            _ => None,
        })
        .collect()
}

/// Lower an AST with imported value bindings into an [`UnfrozenIR`], which
/// include elaboration may still extend before freezing.
///
/// Imported lexical names are added to the resolution scope without injecting
/// parallel AST expressions. Canonical targets remain attached to those names
/// in `imported_bindings`.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if declaration collection or registry construction fails.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_module_with_imported_bindings(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    dag_id: &crate::dag_id::DagId,
    definitions: &mut StaticDefinitionEvaluator<'_>,
) -> Result<UnfrozenIR, GraphcalError> {
    lower_module_with_imported_bindings_and_cancellation(
        ModuleBody {
            ast,
            interface: &ModuleInterface::new(&ast.declarations),
        },
        src,
        imported_names,
        imported_bindings,
        dag_id,
        definitions,
        &crate::cancellation::CancellationToken::unbounded(),
    )
}

/// Lower an AST with imported bindings and cooperative cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for invalid source or cancellation.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_module_with_imported_bindings_and_cancellation(
    module: ModuleBody<'_>,
    src: &NamedSource<Arc<String>>,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    dag_id: &crate::dag_id::DagId,
    definitions: &mut StaticDefinitionEvaluator<'_>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<UnfrozenIR, GraphcalError> {
    cancellation.checkpoint()?;
    let ModuleBody { ast, interface } = module;
    let resolved =
        resolve_with_imported_values(ast, interface.declared_surface(), src, imported_names)?;
    let mut unfrozen = build_ir_from_resolved(
        ast,
        src,
        resolved,
        imported_bindings,
        dag_id,
        definitions,
        cancellation,
    )?;

    // Plot aliases from include brace lists become known to this DAG so
    // figures/layers can reference them (#847).
    unfrozen.included_plots = imported_names
        .plot_names
        .iter()
        .map(|(name, _span)| IncludedPlotEntry { name: name.clone() })
        .collect();

    Ok(unfrozen)
}

/// Lower a `dag { ... }` body as if it were a standalone file.
///
/// The dag body is a virtual [`File`] with its own lexical scope. Cross-scope
/// values must be passed through params or explicit imports; every imported
/// lexical name maps to its canonical [`ResolvedDeclName`] target.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if declaration collection or type-system construction
/// fails for the dag body.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_dag_module_with_imported_bindings(
    dag_body: &File,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::dag_id::DagId,
    definitions: &mut StaticDefinitionEvaluator<'_>,
) -> Result<UnfrozenIR, GraphcalError> {
    lower_dag_module_with_imported_bindings_and_cancellation(
        ModuleBody {
            ast: dag_body,
            interface: &ModuleInterface::new(&dag_body.declarations),
        },
        imported_names,
        imported_bindings,
        src,
        dag_id,
        definitions,
        &crate::cancellation::CancellationToken::unbounded(),
    )
}

/// Lower an inline DAG module with cooperative cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for invalid source or cancellation.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_dag_module_with_imported_bindings_and_cancellation(
    module: ModuleBody<'_>,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::dag_id::DagId,
    definitions: &mut StaticDefinitionEvaluator<'_>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<UnfrozenIR, GraphcalError> {
    cancellation.checkpoint()?;
    let ModuleBody {
        ast: dag_body,
        interface,
    } = module;
    let resolved =
        resolve_with_imported_values(dag_body, interface.declared_surface(), src, imported_names)?;

    build_ir_from_resolved(
        dag_body,
        src,
        resolved,
        imported_bindings,
        dag_id,
        definitions,
        cancellation,
    )
}

/// One module body to lower together with the declared interface of the
/// module it belongs to.
///
/// The interface's declared surface (explicit exports and `param` input ports
/// of the module's own declarations) is the single classification of the
/// module's external boundary; lowering does not re-derive it. An inline DAG's
/// `ast` may have its self-imports stripped, which never changes the declared
/// surface.
#[derive(Debug, Clone, Copy)]
pub struct ModuleBody<'a> {
    pub ast: &'a File,
    pub interface: &'a ModuleInterface,
}

/// Result of `preprocess_dag_body_self_imports`: imported names, canonical
/// bindings, and the body with self-import declarations stripped.
pub struct DagBodySelfImports {
    pub names: ImportedValueNames,
    pub bindings: HashMap<ScopedName, ResolvedDeclName>,
    pub stripped_body: Vec<crate::desugar::desugared_ast::Declaration>,
}

/// Shared implementation for local and imported-binding lowering.
///
/// Evaluates the module's canonical Static definitions and constructs the
/// `UnfrozenIR` from the collected declaration entries.
fn build_ir_from_resolved(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    resolved: CollectedFile,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    dag_id: &crate::dag_id::DagId,
    definitions: &mut StaticDefinitionEvaluator<'_>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<UnfrozenIR, GraphcalError> {
    cancellation.checkpoint()?;
    // Dimensions, units, and indexes are evaluated canonically through the
    // module resolver; nothing is registered under a source spelling.
    let module_statics = definitions.module_definitions(dag_id)?;
    cancellation.checkpoint()?;
    // The resolver was built from these same declarations, so it declared
    // every identity the entries and Static ports need.
    let module_resolver = definitions.resolver();
    let (decls, static_ports) = super::resolve::declaration_entries(
        ast,
        &resolved.plot_visibilities,
        module_resolver,
        dag_id,
    )
    .and_then(|decls| Ok((decls, collect_static_ports(ast, dag_id, module_resolver)?)))
    .map_err(|error| {
        GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
    })?;

    let unfrozen = UnfrozenIR {
        decls,
        included_plots: Vec::new(),
        source_declarations: collect_source_declarations(ast),
        static_ports,
        assumes_map: resolved
            .assumes_map
            .into_iter()
            .map(|(k, v)| {
                (
                    ScopedName::local(k),
                    v.into_iter().map(ScopedName::local).collect(),
                )
            })
            .collect(),
        expected_fail: resolved
            .expected_fail
            .into_iter()
            .map(|(name, collected)| {
                (
                    ScopedName::local(name),
                    ParsedExpectedFailMetadata {
                        expected: collected.expected,
                        resolution_owner: dag_id.clone(),
                        attribute_span: collected.attribute_span,
                    },
                )
            })
            .collect(),
        dynamic_unit_scales: module_statics.dynamic_unit_scales,
        static_definitions: module_statics.definitions,
        unit_bindings: HashMap::new(),
        imported_bindings,
        external_surface: resolved.external_surface,
        plugin_imports: ast
            .declarations
            .iter()
            .filter_map(|decl| match &decl.kind {
                DeclKind::PluginImport(plugin) => Some(plugin.clone()),
                _ => None,
            })
            .collect(),
        semantic_instances: Vec::new(),
    };

    Ok(unfrozen)
}

/// Value declaration metadata needed to create a selective include alias.
#[derive(Debug, Clone)]
pub struct IncludeAliasDeclaration {
    /// Producer-owned type annotation before importer-side Static substitution.
    pub type_ann: TypeExpr,
    /// Whether the alias must remain in the compile-time constant category.
    pub is_const: bool,
}

/// An IR whose bodies still hold syntax, awaiting a call to [`freeze`](Self::freeze).
#[derive(Debug, Clone)]
pub struct UnfrozenIR {
    /// Value, assertion, and visualization declarations in source order,
    /// including selective include aliases appended during assembly.
    pub(super) decls: Vec<entry::Decl<Syntax>>,
    /// Plot aliases from include brace lists (#847).
    pub(super) included_plots: Vec<IncludedPlotEntry>,
    /// Direct source declarations are immutable provenance. Include merging
    /// extends `decls` but never this entry-interface subset.
    pub(super) source_declarations: Vec<crate::hir::SourceDeclaration>,
    /// Static interface provenance is authored only by this DAG template.
    pub(super) static_ports: Vec<crate::hir::StaticPort>,
    // Key-lookup only, order irrelevant.
    pub(super) assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    // Key-lookup only, order irrelevant. Each value retains authored scope/source.
    pub(super) expected_fail: HashMap<ScopedName, ParsedExpectedFailMetadata>,
    // Dynamic unit scales declared by this source body.
    pub(super) dynamic_unit_scales: Vec<entry::DynamicUnitScaleEntry<Syntax>>,
    // Canonical dimensions, units, and indexes this DAG owns.
    pub(super) static_definitions: StaticDefinitions,
    // Source-visible projected units mapped to concrete instance identities.
    pub(super) unit_bindings: HashMap<UnitRef, ResolvedUnitName>,
    // Lexical binding lookup only; each value carries one canonical target.
    pub(super) imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    // Explicit exports and named `param` input ports used by downstream
    // import/include boundary checks.
    pub(super) external_surface: ExternalDeclSurface,
    /// Plugin-import declarations, awaiting signature resolution in the
    /// declaring module's scope in [`UnfrozenIR::freeze`].
    pub(super) plugin_imports: Vec<crate::desugar::desugared_ast::PluginImportDecl>,
    /// Semantic instance edges awaiting importer-context HIR lowering.
    pub(super) semantic_instances: Vec<UnfrozenSemanticInstance>,
}

/// One semantic include edge before importer-context value expressions become HIR.
#[derive(Debug, Clone)]
pub struct UnfrozenSemanticInstance {
    pub(crate) instance: InstanceRecord,
    pub(crate) value_bindings: HashMap<ResolvedDeclName, Expr>,
    pub(crate) runtime_unit_names: HashSet<UnitName>,
    pub(crate) output_projections: Vec<crate::ir::instance::InstanceValueProjection>,
    pub(crate) assertion_projections: Vec<crate::ir::instance::InstanceAssertionProjection>,
    pub(crate) plot_projections: Vec<crate::ir::instance::InstancePlotProjection>,
    pub(crate) override_reconciliations:
        HashMap<ResolvedDeclName, Vec<crate::ir::override_reconciliation::OverrideReconciliation>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::names::{NameAtom, NamePath};
    use crate::syntax::parser::Parser;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test.gcl", Arc::new(source.to_string()))
    }

    fn parse_and_lower(source: &str) -> Result<HirDag, GraphcalError> {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let desugared = crate::desugar::desugared_ast::File::from(raw_file);
        let file = desugared;
        lower(&file, &make_src(source))
    }

    #[test]
    fn lower_rocket() {
        let source = include_str!("../../../../tests/fixtures/valid/rocket.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert_eq!(ir.decls().consts().count(), 1); // G0
        assert_eq!(ir.decls().params().count(), 3); // dry_mass, fuel_mass, isp
        assert_eq!(ir.decls().nodes().count(), 3); // v_exhaust, mass_ratio, delta_v
        // Prelude names are resolved canonically; the module owns no copy.
        assert_eq!(ir.definitions().statics().dimensions().count(), 0);
        assert!(ir.display_dimensions.iter().any(|(name, _)| name
            == &crate::syntax::dimension::DimRef::local(
                crate::syntax::dimension::DimName::expect_valid("Length")
            )));
    }

    #[test]
    fn lower_constants() {
        let source = include_str!("../../../../tests/fixtures/valid/constants.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert_eq!(ir.decls().consts().count(), 4);
        assert_eq!(ir.decls().params().count(), 1);
        assert_eq!(ir.decls().nodes().count(), 2);
    }

    #[test]
    fn lower_indexed() {
        let source = include_str!("../../../../tests/fixtures/valid/indexed.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert!(
            ir.definitions()
                .statics()
                .indexes()
                .any(|(identity, _)| identity.as_str() == "Maneuver")
        );
    }

    #[test]
    fn hir_retains_the_direct_runtime_interface_in_source_order() {
        let hir = parse_and_lower(
            "const node ignored: Dimensionless = 1.0;\n\
             pub(bind) index Phase;\n\
             param input: Dimensionless = 1.0;\n\
             node private: Dimensionless = @input;\n\
             pub node output: Dimensionless = @private;\n",
        )
        .unwrap();

        assert!(matches!(
            hir.source_declarations(),
            [
                crate::hir::SourceDeclaration::Index { name, .. },
                crate::hir::SourceDeclaration::Parameter { name: input, .. },
                crate::hir::SourceDeclaration::Node { name: private, .. },
                crate::hir::SourceDeclaration::Node { name: output, .. },
            ] if name.as_str() == "Phase"
                && input.as_str() == "input"
                && private.as_str() == "private"
                && output.as_str() == "output"
        ));
    }

    #[test]
    fn nominal_signatures_are_canonical_hir() {
        let hir = parse_and_lower(
            "type Marker { Marker }\n\
             type Box<T: Type = Marker> { Box(value: T) }\n",
        )
        .unwrap();
        let identity = crate::resolved_name::ResolvedStructTypeName::for_test(
            hir.dag_id().clone(),
            crate::syntax::type_name::StructTypeName::expect_valid("Box"),
        );
        let marker = crate::resolved_name::ResolvedStructTypeName::for_test(
            hir.dag_id().clone(),
            crate::syntax::type_name::StructTypeName::expect_valid("Marker"),
        );
        assert!(
            hir.nominal_types().get(&identity).is_some(),
            "canonical nominal definitions must cross into HIR"
        );
        let definition = hir.nominal_types().get(&identity).unwrap();
        let [parameter] = definition.generic_params() else {
            panic!("Box should retain exactly one generic parameter");
        };
        let Some(crate::hir::GenericArg::Type(default)) = parameter.default() else {
            panic!("Box#T should have a HIR type default");
        };
        assert!(matches!(
            &default.kind,
            crate::hir::ValueTypeKind::Struct(name) if name.value == marker
        ));
        let [constructor] = definition.union_members().unwrap() else {
            panic!("Box should retain exactly one constructor");
        };
        let [field] = constructor.fields() else {
            panic!("Box should retain exactly one field");
        };
        assert!(matches!(
            &field.type_annotation().decl_type,
            crate::hir::DeclType::Value(crate::hir::ValueType {
                kind: crate::hir::ValueTypeKind::GenericTypeParam(field_param),
                ..
            }) if &field_param.value == parameter.id()
        ));
        assert_eq!(definition.source().name(), "test.gcl");
    }

    #[test]
    fn lower_hohmann() {
        // hohmann.gcl uses DAG+include. The full project pipeline accepts
        // it (see the CLI tests), but single-file IR lowering rejects it at
        // the freeze boundary: include expansion is a higher-phase concern,
        // so `@transfer` (the include's projected node) cannot resolve.
        let source = include_str!("../../../../tests/fixtures/valid/hohmann.gcl");
        let err = parse_and_lower(source).unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownGraphRef { .. }));
    }

    #[test]
    fn lower_duplicate_name_error() {
        let err = parse_and_lower("param x: Dimensionless = 1.0;\nnode x: Dimensionless = 2.0;")
            .unwrap_err();
        assert!(matches!(err, GraphcalError::DuplicateName { .. }));
    }

    #[test]
    fn duplicate_constructor_field_declarations_are_rejected() {
        let err =
            parse_and_lower("pub type Pair { Pair(value: Length, other: Bool, value: Time) }")
                .unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::DuplicateConstructorField {
                type_name,
                constructor,
                field,
                ..
            } if type_name.as_str() == "Pair"
                && constructor.as_str() == "Pair"
                && field.as_str() == "value"
        ));
    }

    #[test]
    fn same_field_name_in_distinct_constructors_is_valid() {
        parse_and_lower("pub type Choice { Left(value: Length), Right(value: Time) }").unwrap();
    }

    #[test]
    fn unknown_unit_dimension_reports_referenced_dimension() {
        let err = parse_and_lower("unit foo: Blah = 1.0 m;").unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::UnknownDimension { name, .. } if name.to_string() == "Blah"
        ));
    }

    #[test]
    fn unknown_derived_dimension_term_reports_referenced_dimension() {
        let err = parse_and_lower("dim Foo = Bar * Baz;").unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::UnknownDimension { name, .. } if name.to_string() == "Bar"
        ));
    }

    #[test]
    fn extern_struct_results_merge_only_for_the_same_record_type() {
        let declarations = |second_result: &str| {
            format!(
                "type Bounds {{ Bounds(lo: Length, hi: Length), }}\n\
                 type Range {{ Range(lo: Length, hi: Length), }}\n\
                 import plugin \"graphcal:demo\" as a {{ fn bounds(x: Length) -> Bounds; }}\n\
                 import plugin \"graphcal:demo\" as b {{ fn bounds(y: Length) -> {second_result}; }}\n"
            )
        };

        let merged = parse_and_lower(&declarations("Bounds")).unwrap();
        assert_eq!(merged.extern_functions().len(), 1);

        let err = parse_and_lower(&declarations("Range")).unwrap_err();
        assert!(
            matches!(
                &err,
                GraphcalError::InvalidExternSignature { message, .. }
                    if message.contains("different result type")
            ),
            "{err:?}"
        );
    }

    #[test]
    fn unknown_qualified_extern_dimension_preserves_its_path() {
        let path = NamePath::qualified(
            crate::syntax::non_empty::NonEmpty::singleton(NameAtom::parse("missing").unwrap()),
            NameAtom::parse("Dimension").unwrap(),
        );
        let owner =
            crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("test.gcl"))
                .unwrap();
        let resolver = crate::resolve::ModuleResolver::default();
        let source = make_src("missing.Dimension");
        let mut definitions = definition_evaluator(&resolver, [], &source).unwrap();
        let nominal_types = crate::hir::NominalTypeRegistry::default();

        let error = resolve_extern_struct_return(
            &path,
            Span::new(0, source.inner().len()),
            &mut super::super::extern_fns::ExternSignatureScope {
                owner: &owner,
                nominal_types: &nominal_types,
                definitions: &mut definitions,
            },
            &source,
        )
        .unwrap_err();

        assert!(
            matches!(
                &error,
                GraphcalError::UnknownDimension { name, .. }
                    if name.qualifier().iter().map(NameAtom::as_str).eq(["missing"])
                        && name.leaf().as_str() == "Dimension"
            ),
            "{error:?}"
        );
    }

    #[test]
    fn static_unit_definition_must_match_declared_dimension() {
        let err = parse_and_lower("const unit wrong: Length = 1.0 h;").unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::UnitDefinitionDimensionMismatch {
                name,
                declared,
                definition,
                ..
            } if name.as_str() == "wrong"
                && declared == "Length"
                && definition == "Time"
        ));
    }

    #[test]
    fn dynamic_unit_definition_must_match_declared_dimension() {
        let err = parse_and_lower(
            "param factor: Dimensionless = 2.0;\nunit wrong: Length = (@factor) h;",
        )
        .unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::UnitDefinitionDimensionMismatch { name, .. }
                if name.as_str() == "wrong"
        ));
    }

    #[test]
    fn compound_unit_definition_with_matching_dimension_is_accepted() {
        parse_and_lower("const unit cruise: Velocity = 0.5 km/h;").unwrap();
    }

    #[test]
    fn failed_unit_definition_is_not_memoized() {
        let source = "const unit wrong: Length = 1.0 h;";
        let src = make_src(source);
        let raw_file = Parser::new(source).parse_file().unwrap();
        let file = crate::desugar::desugared_ast::File::from(raw_file);
        let owner = crate::dag_id::DagId::root_in_package("test", "main");
        let resolver = single_module_resolver(&file, &owner, &src).unwrap();
        let mut definitions = definition_evaluator(
            &resolver,
            [(
                owner.clone(),
                super::super::static_definitions::DefinitionSource {
                    declarations: &file.declarations,
                    src: &src,
                },
            )],
            &src,
        )
        .unwrap();
        let wrong = resolver
            .resolve_unit_path(&owner, &NamePath::expect_local("wrong"))
            .unwrap()
            .into_resolved();

        for _ in 0..2 {
            assert!(matches!(
                definitions.unit(&wrong),
                Err(GraphcalError::UnitDefinitionDimensionMismatch { .. })
            ));
        }
        assert!(matches!(
            definitions.module_definitions(&owner),
            Err(GraphcalError::UnitDefinitionDimensionMismatch { .. })
        ));
    }

    #[test]
    fn freeze_resolves_attribute_targets_to_identities() {
        let hir = parse_and_lower(
            "#[expected_fail]\nassert ok = true;\n\
             #[assumes(ok)]\nparam p: Dimensionless = 1.0;\n\
             #[assumes(ok)]\nnode n: Dimensionless = @p;",
        )
        .unwrap();
        let identity = |spelling: &str| {
            hir.decls()
                .lookup(&DeclName::expect_valid(spelling))
                .unwrap()
                .clone()
        };
        let ok = identity("ok");
        assert_eq!(ok.owner(), hir.dag_id());
        let mut assumers = hir.assumes_map.get(&ok).unwrap().clone();
        assumers.sort_by_key(ToString::to_string);
        assert_eq!(assumers, [identity("n"), identity("p")]);
        assert!(matches!(
            hir.expected_fail
                .get(&ok)
                .map(|metadata| &metadata.expected),
            Some(crate::assertion_expectation::ExpectedFail::All)
        ));
        assert_eq!(hir.expected_fail.len(), 1);
    }

    #[test]
    fn lower_source_order_preserved() {
        let ir = parse_and_lower(
            "param b: Dimensionless = 2.0;\nparam a: Dimensionless = 1.0;\nnode z: Dimensionless = @a + @b;",
        )
        .unwrap();
        let names: Vec<String> = ir.decls().iter().map(|d| d.name().to_string()).collect();
        assert_eq!(names, vec!["b", "a", "z"]);
    }
}
