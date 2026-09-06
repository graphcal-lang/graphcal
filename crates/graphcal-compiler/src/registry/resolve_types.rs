//! Data types used by the declaration-collection layer.
//!
//! Source collection shells and attribute policy stay here; execution consumers
//! use the independent declaration-category and assertion-expectation contracts.

use std::collections::{HashMap, HashSet};

use crate::assertion_expectation::{ExpectedFail, ExpectedFailKey};
use crate::declaration_category::DeclCategory;
use crate::desugar::desugared_ast::{AssertBody, DeclKind, Expr, FigureDecl, LayerDecl, PlotDecl};
use crate::syntax::attribute::AttributeName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::span::Span;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Semantic category of a source declaration.
///
/// This broad category is for diagnostics and declaration-level policy. It is
/// distinct from [`DeclCategory`], whose variants are intentionally limited to
/// declarations that can appear in evaluation source order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeclarationKind {
    ConstNode,
    Param,
    Node,
    Assert,
    Plot,
    Figure,
    Layer,
    Dimension,
    Unit,
    Type,
    Index,
    Import,
    Include,
    Dag,
}

impl DeclarationKind {
    /// Classify one post-desugar declaration.
    #[must_use]
    pub fn from_decl_kind(kind: &DeclKind) -> Self {
        match kind {
            DeclKind::ConstNode(_) => Self::ConstNode,
            DeclKind::Param(_) => Self::Param,
            DeclKind::Node(_) => Self::Node,
            DeclKind::Assert(_) => Self::Assert,
            DeclKind::Plot(_) => Self::Plot,
            DeclKind::Figure(_) => Self::Figure,
            DeclKind::Layer(_) => Self::Layer,
            DeclKind::BaseDimension(_) | DeclKind::Dimension(_) => Self::Dimension,
            DeclKind::Unit(_) => Self::Unit,
            DeclKind::Type(_) => Self::Type,
            DeclKind::Index(_) => Self::Index,
            DeclKind::Import(_) | DeclKind::PluginImport(_) => Self::Import,
            DeclKind::Include(_) => Self::Include,
            DeclKind::Dag(_) => Self::Dag,
            DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
        }
    }
}

impl std::fmt::Display for DeclarationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ConstNode => "const node",
            Self::Param => "param",
            Self::Node => "node",
            Self::Assert => "assert",
            Self::Plot => "plot",
            Self::Figure => "figure",
            Self::Layer => "layer",
            Self::Dimension => "dim",
            Self::Unit => "unit",
            Self::Type => "type",
            Self::Index => "cat/range",
            Self::Import => "import",
            Self::Include => "include",
            Self::Dag => "dag",
        })
    }
}

/// Source context in which an attribute is attached.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AttributeTarget {
    Declaration(DeclarationKind),
    IncludeItem {
        /// Producer category when the include item names an assertion or plot.
        producer: Option<DeclarationKind>,
        /// Producer spelling written on the include item.
        name: NameAtom,
    },
}

impl AttributeTarget {
    #[must_use]
    pub const fn declaration(kind: DeclarationKind) -> Self {
        Self::Declaration(kind)
    }

    #[must_use]
    pub const fn include_item(producer: Option<DeclarationKind>, name: NameAtom) -> Self {
        Self::IncludeItem { producer, name }
    }

    /// Whether this target accepts the attribute's semantic role.
    #[must_use]
    pub const fn accepts(&self, attribute: AttributeName) -> bool {
        match (self, attribute) {
            (
                &Self::Declaration(DeclarationKind::Param | DeclarationKind::Node),
                AttributeName::Assumes,
            )
            | (
                &Self::Declaration(DeclarationKind::Assert)
                | &Self::IncludeItem {
                    producer: Some(DeclarationKind::Assert),
                    ..
                },
                AttributeName::ExpectedFail,
            )
            | (
                &Self::Declaration(DeclarationKind::Plot)
                | &Self::IncludeItem {
                    producer: Some(DeclarationKind::Plot),
                    ..
                },
                AttributeName::Hidden,
            ) => true,
            (
                _,
                AttributeName::Assumes
                | AttributeName::ExpectedFail
                | AttributeName::Hidden
                | AttributeName::Lazy,
            ) => false,
        }
    }
}

impl std::fmt::Display for AttributeTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declaration(kind) => kind.fmt(f),
            Self::IncludeItem { .. } => f.write_str("include/import item"),
        }
    }
}

/// Pre-evaluated value bindings imported from already-evaluated dependency files.
///
/// These names carry no parallel AST-expression import route. Canonical
/// targets cross the HIR boundary separately as typed imported bindings.
#[derive(Debug, Default, Clone)]
pub struct ImportedValueNames {
    /// Imported const names (for scope checking only — actual values are in the exec plan).
    pub const_names: Vec<(ScopedName, Span)>,
    /// Imported param names.
    pub param_names: Vec<(ScopedName, Span)>,
    /// Imported node names.
    pub node_names: Vec<(ScopedName, Span)>,
    /// Imported assert names (for `#[assumes]` validation).
    pub assert_names: Vec<(DeclName, Span)>,
    /// Plot aliases requested by include brace lists (#847). Registered in
    /// the value namespace for collision checking and recorded on the DAG so
    /// figures/layers can reference them.
    pub plot_names: Vec<(ScopedName, Span)>,
}

// ---------------------------------------------------------------------------
// Entry types for collected declaration shells
// ---------------------------------------------------------------------------

/// A collected const declaration awaiting type and HIR lowering.
#[derive(Debug)]
pub struct CollectedConstEntry {
    pub(crate) name: DeclName,
    pub(crate) expr: Expr,
    pub(crate) span: Span,
}

/// A collected parameter declaration awaiting type and HIR lowering.
#[derive(Debug)]
pub struct CollectedParamEntry {
    pub(crate) name: DeclName,
    pub(crate) default_expr: Option<Expr>,
    pub(crate) span: Span,
}

/// A collected node declaration awaiting type and HIR lowering.
#[derive(Debug)]
pub struct CollectedNodeEntry {
    pub(crate) name: DeclName,
    pub(crate) expr: Expr,
    pub(crate) span: Span,
}

/// A collected assertion declaration awaiting HIR lowering.
#[derive(Debug)]
pub struct CollectedAssertEntry {
    pub(crate) name: DeclName,
    pub(crate) body: AssertBody,
    pub(crate) span: Span,
}

/// A collected plot declaration awaiting HIR lowering.
#[derive(Debug)]
pub struct CollectedPlotEntry {
    pub(crate) name: DeclName,
    pub(crate) decl: PlotDecl,
    pub(crate) span: Span,
}

/// A collected figure declaration awaiting HIR lowering.
#[derive(Debug)]
pub struct CollectedFigureEntry {
    pub name: DeclName,
    pub decl: FigureDecl,
}

/// A collected layer declaration awaiting HIR lowering.
#[derive(Debug)]
pub struct CollectedLayerEntry {
    pub name: DeclName,
    pub decl: LayerDecl,
}

pub(crate) type ParsedExpectedFailKey = ExpectedFailKey<NamePath>;
pub type ParsedExpectedFail = ExpectedFail<NamePath>;

/// Source-level expected-fail configuration retained by declaration collection.
///
/// The attribute span is kept separately from key-part spans because the
/// blanket form has no keys but may still need a diagnostic at a later phase.
#[derive(Debug, Clone)]
pub(crate) struct CollectedExpectedFail {
    pub(crate) expected: ParsedExpectedFail,
    pub(crate) attribute_span: Span,
}

/// Roles that form a file or DAG's externally addressable declaration surface.
///
/// An explicit `pub`/`pub(bind)` export and an annotation-free `param` input
/// port have different provenance and capabilities. Both are externally
/// readable and may be selected as effective outputs, while only input ports
/// accept value bindings. Keeping the roles distinct prevents output selection
/// from erasing binding semantics or making a param look explicitly `pub`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalDeclRole {
    ExplicitExport,
    InputPort,
}

/// Namespace component of one typed external-surface slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExternalNamespace {
    Static,
    Term,
    Unit,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ExternalDeclSlot {
    namespace: ExternalNamespace,
    name: NameAtom,
}

#[derive(Debug, Clone, Default)]
pub struct ExternalDeclSurface {
    roles: HashMap<ExternalDeclSlot, ExternalDeclRole>,
}

impl ExternalDeclSurface {
    /// Record an explicitly exported flat Term.
    pub fn insert_explicit_export(&mut self, name: DeclName) {
        self.insert(
            ExternalNamespace::Term,
            name.into_atom(),
            ExternalDeclRole::ExplicitExport,
        );
    }

    /// Record an explicitly exported Static entity.
    pub fn insert_static_export(&mut self, name: NameAtom) {
        self.insert(
            ExternalNamespace::Static,
            name,
            ExternalDeclRole::ExplicitExport,
        );
    }

    /// Record an explicitly exported Unit entity.
    pub fn insert_unit_export(&mut self, name: NameAtom) {
        self.insert(
            ExternalNamespace::Unit,
            name,
            ExternalDeclRole::ExplicitExport,
        );
    }

    /// Record a `param` as a named Term input port.
    pub fn insert_input_port(&mut self, name: DeclName) {
        self.insert(
            ExternalNamespace::Term,
            name.into_atom(),
            ExternalDeclRole::InputPort,
        );
    }

    fn insert(&mut self, namespace: ExternalNamespace, name: NameAtom, role: ExternalDeclRole) {
        let slot = ExternalDeclSlot { namespace, name };
        match self.roles.entry(slot) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(role);
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                debug_assert_eq!(*entry.get(), role, "one external slot has one role");
            }
        }
    }

    fn role(&self, namespace: ExternalNamespace, name: &NameAtom) -> Option<ExternalDeclRole> {
        self.roles
            .get(&ExternalDeclSlot {
                namespace,
                name: name.clone(),
            })
            .copied()
    }

    /// Whether a flat Term carries an explicit export.
    #[must_use]
    pub fn is_explicit_export(&self, name: &DeclName) -> bool {
        self.role(ExternalNamespace::Term, name.atom()) == Some(ExternalDeclRole::ExplicitExport)
    }

    /// Whether a Static entity carries an explicit export.
    #[must_use]
    pub fn is_static_explicit_export(&self, name: &NameAtom) -> bool {
        self.role(ExternalNamespace::Static, name) == Some(ExternalDeclRole::ExplicitExport)
    }

    /// Whether a Unit carries an explicit export.
    #[must_use]
    pub fn is_unit_explicit_export(&self, name: &NameAtom) -> bool {
        self.role(ExternalNamespace::Unit, name) == Some(ExternalDeclRole::ExplicitExport)
    }

    /// Whether the Term is a named `param` input port.
    #[must_use]
    pub fn is_input_port(&self, name: &DeclName) -> bool {
        self.role(ExternalNamespace::Term, name.atom()) == Some(ExternalDeclRole::InputPort)
    }

    /// Whether external Term syntax can resolve the declaration in either role.
    #[must_use]
    pub fn is_externally_nameable(&self, name: &DeclName) -> bool {
        self.role(ExternalNamespace::Term, name.atom()).is_some()
    }

    /// Whether a Term may be selected from an instance output surface.
    #[must_use]
    pub fn can_select_output(&self, name: &DeclName) -> bool {
        matches!(
            self.role(ExternalNamespace::Term, name.atom()),
            Some(ExternalDeclRole::ExplicitExport | ExternalDeclRole::InputPort)
        )
    }
}

/// The result of declaration collection: declarations separated by category.
#[derive(Debug)]
pub(crate) struct CollectedFile {
    /// Const declarations in source order.
    pub(crate) consts: Vec<CollectedConstEntry>,
    /// Param declarations in source order.
    pub(crate) params: Vec<CollectedParamEntry>,
    /// Node declarations in source order.
    pub(crate) nodes: Vec<CollectedNodeEntry>,
    /// Assert declarations in source order.
    pub(crate) asserts: Vec<CollectedAssertEntry>,
    /// Plot declarations in source order.
    pub(crate) plots: Vec<CollectedPlotEntry>,
    /// Figure declarations in source order.
    pub(crate) figures: Vec<CollectedFigureEntry>,
    /// Layer declarations in source order.
    pub(crate) layers: Vec<CollectedLayerEntry>,
    /// All declaration names in source order with their category.
    pub(crate) source_order: Vec<(DeclName, DeclCategory)>,
    /// Mapping from assert name to the list of declarations that assume it.
    /// Built from `#[assumes(...)]` attributes.
    pub(crate) assumes_map: HashMap<DeclName, Vec<DeclName>>,
    /// Mapping from assert name to its expected-fail configuration.
    /// Built from `#[expected_fail]` / `#[expected_fail(...)]` attributes.
    pub(crate) expected_fail: HashMap<DeclName, CollectedExpectedFail>,
    /// Plot names carrying `#[hidden]`: evaluated and referenceable from
    /// figures/layers, but excluded from standalone output (#847).
    pub(crate) hidden_plots: HashSet<DeclName>,
    /// Explicit exports and annotation-free `param` input ports, classified by role.
    pub(crate) external_surface: ExternalDeclSurface,
}
