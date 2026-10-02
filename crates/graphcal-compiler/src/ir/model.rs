//! Per-DAG IR model: the lowered body phase, frozen [`HirDag`]s, and the
//! [`UnfrozenIR`] that include assembly extends before the freeze boundary.

use std::collections::{HashMap, HashSet};

use crate::desugar::desugared_ast::{Expr, TypeExpr};
use crate::dimension::Dimension;
use crate::ir::instance::InstanceRecord;
use crate::ir::resolve::collected::ExternalDeclSurface;
use crate::ir::resolve::collected::ParsedExpectedFail;
use crate::plot_visibility::PlotVisibility;
use crate::resolved_name::{ResolvedDeclName, ResolvedUnitName};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{DimRef, UnitName, UnitRef};
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;

use super::decl_table::DeclTable;
use super::entry::{self, BodyPhase, Syntax};
use super::extern_function::ExternFunctionEntry;
use super::module_definitions::{ModuleDefinitions, StaticDefinitions};

// ---------------------------------------------------------------------------
// Entry types for IR declarations
// ---------------------------------------------------------------------------

/// One plot declaration's expressions lowered to HIR, in source order.
#[derive(Debug, Clone, Default)]
pub struct LoweredPlotBody {
    /// Encoding channel expressions (`x: ...`, `y: ...`).
    pub encodings: Vec<(
        crate::syntax::ast::EncodingChannel,
        crate::hir::expr::CheckedExpr,
    )>,
    /// Mark property expressions (`stroke_width: ...`).
    pub mark_properties: Vec<LoweredPlotField<crate::plot_props::MarkProperty>>,
    /// Plot-level property expressions (`title: ...`).
    pub properties: Vec<LoweredPlotField<crate::plot_props::PlotProperty>>,
}

impl LoweredPlotBody {
    /// Every property value expression, mark properties first.
    pub fn property_values(&self) -> impl Iterator<Item = &crate::hir::expr::CheckedExpr> {
        self.mark_properties
            .iter()
            .map(|field| &field.value)
            .chain(self.properties.iter().map(|field| &field.value))
    }
}

/// A plot, mark, figure, or layer field expression lowered to HIR.
///
/// Its property is classified for the block that holds it when the
/// declaration is lowered, so a name that is not a property of that block is
/// rejected there and never reaches checking or evaluation.
#[derive(Debug, Clone)]
pub struct LoweredPlotField<P> {
    pub property: P,
    pub value: crate::hir::expr::CheckedExpr,
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
    type Expr = crate::hir::expr::CheckedExpr;
    type TypeAnnotation = crate::hir::type_annotation::TypeAnnotation;
    type NodeDefinition = crate::hir::node_definition::NodeDefinition;
    type AssertBody = crate::hir::expr::CheckedAssertBody;
    type PlotBody = LoweredPlotBody;
    type CompositionFields = Vec<LoweredPlotField<crate::plot_props::CompositionProperty>>;
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

/// Intermediate Representation produced by [`lower`](super::lower::lower).
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
    /// The handle of this DAG in the resolver that lowered it.
    pub(super) module: crate::resolve::module_table::ModuleHandle,
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
    pub(super) source_declarations: Vec<crate::hir::source_interface::SourceDeclaration>,
    /// Typed Static ports authored directly in this reusable DAG.
    pub(crate) static_ports: Vec<crate::hir::source_interface::StaticPort>,
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

    /// The handle of this DAG in the resolver that lowered it.
    #[must_use]
    pub const fn module(&self) -> crate::resolve::module_table::ModuleHandle {
        self.module
    }

    /// Value, assertion, and visualization declarations of this DAG.
    #[must_use]
    pub const fn decls(&self) -> &DeclTable<Lowered> {
        &self.decls
    }

    /// Nominal definitions canonically owned by this DAG.
    #[must_use]
    pub const fn nominal_types(&self) -> &crate::hir::nominal::NominalTypeRegistry {
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
            crate::semantic::dimension_table::BaseDimensionInfo,
        >,
    ) -> crate::display::formatting_registry::FormattingRegistry {
        crate::display::formatting_registry::FormattingRegistry::new(
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
    pub fn source_declarations(&self) -> &[crate::hir::source_interface::SourceDeclaration] {
        &self.source_declarations
    }

    /// Typed Static interface authored directly in this DAG.
    #[must_use]
    pub fn static_ports(&self) -> &[crate::hir::source_interface::StaticPort] {
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
    pub(super) source_declarations: Vec<crate::hir::source_interface::SourceDeclaration>,
    /// Static interface provenance is authored only by this DAG template.
    pub(super) static_ports: Vec<crate::hir::source_interface::StaticPort>,
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
