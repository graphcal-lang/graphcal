//! Canonical semantic identities and source-visible bindings for editor symbols.
//!
//! A declaration's identity is independent of how one source file spells it.
//! Imports may expose one identity under several aliases, while distinct
//! compiler namespaces may intentionally reuse one leaf spelling. Keeping
//! semantic identity and source spelling in separate types prevents either
//! case from being collapsed by a `HashMap<String, _>`.

use graphcal_compiler::builtin::{BuiltinConst, BuiltinFnName};
use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::hir;
use graphcal_compiler::registry::time_scale::TimeScale;
use graphcal_compiler::syntax::decl_name::ResolvedDeclName;
use graphcal_compiler::syntax::dimension::{ResolvedDimName, ResolvedUnitName};
use graphcal_compiler::syntax::function_name::FnName;
use graphcal_compiler::syntax::index_name::{
    IndexVariantName, ResolvedIndexName, ResolvedIndexVariant,
};
use graphcal_compiler::syntax::module_name::ModuleAliasName;
use graphcal_compiler::syntax::names::{NameAtom, NamePath};
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::{
    FieldName, GenericParamName, ResolvedConstructorName, ResolvedStructTypeName,
};

/// Canonical identity of an index variant.
///
/// The compiler's resolved variant type is intentionally decomposed only once
/// at this boundary. Its resolved index owner remains structural identity; the
/// variant leaf cannot be confused with a variant of another index.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IndexVariantId {
    index: ResolvedIndexName,
    variant: IndexVariantName,
}

impl IndexVariantId {
    #[must_use]
    pub fn from_resolved(variant: &ResolvedIndexVariant) -> Self {
        Self {
            index: variant.index().clone(),
            variant: variant.variant().clone(),
        }
    }

    #[must_use]
    pub const fn new(index: ResolvedIndexName, variant: IndexVariantName) -> Self {
        Self { index, variant }
    }

    #[must_use]
    pub const fn index(&self) -> &ResolvedIndexName {
        &self.index
    }

    #[must_use]
    pub const fn variant(&self) -> &IndexVariantName {
        &self.variant
    }
}

/// Canonical identity of a constructor field.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FieldId {
    owner: ResolvedConstructorName,
    field: FieldName,
}

impl FieldId {
    #[must_use]
    pub const fn new(owner: ResolvedConstructorName, field: FieldName) -> Self {
        Self { owner, field }
    }

    #[must_use]
    pub const fn owner(&self) -> &ResolvedConstructorName {
        &self.owner
    }

    #[must_use]
    pub const fn field(&self) -> &FieldName {
        &self.field
    }
}

/// Canonical identity of a generic parameter within one nominal type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericParamId {
    owner: ResolvedStructTypeName,
    parameter: GenericParamName,
}

impl GenericParamId {
    #[must_use]
    pub const fn new(owner: ResolvedStructTypeName, parameter: GenericParamName) -> Self {
        Self { owner, parameter }
    }

    #[must_use]
    pub const fn owner(&self) -> &ResolvedStructTypeName {
        &self.owner
    }

    #[must_use]
    pub const fn parameter(&self) -> &GenericParamName {
        &self.parameter
    }
}

/// Identity of one tolerant-HIR expression body.
///
/// HIR local IDs are unique only inside a lowering context. Pairing the ID
/// with its canonical DAG owner and body span makes cross-body aliasing
/// unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LocalSymbolId {
    owner: DagId,
    body_span: Span,
    local: hir::LocalId,
}

impl LocalSymbolId {
    #[must_use]
    pub const fn new(owner: DagId, body_span: Span, local: hir::LocalId) -> Self {
        Self {
            owner,
            body_span,
            local,
        }
    }
}

/// Canonical identity of an extern function declared by one plugin import.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExternFunctionId {
    owner: DagId,
    plugin: ModuleAliasName,
    function: FnName,
}

impl ExternFunctionId {
    #[must_use]
    pub const fn new(owner: DagId, plugin: ModuleAliasName, function: FnName) -> Self {
        Self {
            owner,
            plugin,
            function,
        }
    }
}

/// A symbol identity that has passed namespace-aware module resolution.
///
/// Every variant carries the compiler type for its namespace. There is no
/// variant that can represent an unresolved spelling, so the definitions map
/// cannot accidentally contain a source alias or an unknown name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SymbolId {
    Declaration(ResolvedDeclName),
    Dimension(ResolvedDimName),
    Unit(ResolvedUnitName),
    StructType(ResolvedStructTypeName),
    Constructor(ResolvedConstructorName),
    Index(ResolvedIndexName),
    IndexVariant(IndexVariantId),
    Field(FieldId),
    GenericParam(GenericParamId),
    Local(LocalSymbolId),
    ExternFunction(ExternFunctionId),
    BuiltinFunction(BuiltinFnName),
    BuiltinConstant(BuiltinConst),
    TimeScale(TimeScale),
}

impl SymbolId {
    /// Canonical owner for source-defined identities.
    #[must_use]
    pub const fn owner(&self) -> Option<&DagId> {
        match self {
            Self::Declaration(name) => Some(name.owner()),
            Self::Dimension(name) => Some(name.owner()),
            Self::Unit(name) => Some(name.owner()),
            Self::StructType(name) => Some(name.owner()),
            Self::Constructor(name) => Some(name.owner()),
            Self::Index(name) => Some(name.owner()),
            Self::IndexVariant(id) => Some(id.index().owner()),
            Self::Field(id) => Some(id.owner().owner()),
            Self::GenericParam(id) => Some(id.owner().owner()),
            Self::Local(id) => Some(&id.owner),
            Self::ExternFunction(id) => Some(&id.owner),
            Self::BuiltinFunction(_) | Self::BuiltinConstant(_) | Self::TimeScale(_) => None,
        }
    }

    /// Whether two identities occupy the same compiler namespace.
    #[must_use]
    pub const fn same_namespace(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Declaration(_), Self::Declaration(_))
                | (Self::Dimension(_), Self::Dimension(_))
                | (Self::Unit(_), Self::Unit(_))
                | (Self::StructType(_), Self::StructType(_))
                | (Self::Constructor(_), Self::Constructor(_))
                | (Self::Index(_), Self::Index(_))
                | (Self::IndexVariant(_), Self::IndexVariant(_))
                | (Self::Field(_), Self::Field(_))
                | (Self::GenericParam(_), Self::GenericParam(_))
                | (Self::Local(_), Self::Local(_))
                | (Self::ExternFunction(_), Self::ExternFunction(_))
                | (Self::BuiltinFunction(_), Self::BuiltinFunction(_))
                | (Self::BuiltinConstant(_), Self::BuiltinConstant(_))
                | (Self::TimeScale(_), Self::TimeScale(_))
        )
    }

    /// Semantic leaf spelling, used only at editor presentation boundaries.
    #[must_use]
    pub fn leaf_name(&self) -> &str {
        match self {
            Self::Declaration(name) => name.as_str(),
            Self::Dimension(name) => name.as_str(),
            Self::Unit(name) => name.as_str(),
            Self::StructType(name) => name.as_str(),
            Self::Constructor(name) => name.as_str(),
            Self::Index(name) => name.as_str(),
            Self::IndexVariant(id) => id.variant().as_str(),
            Self::Field(id) => id.field().as_str(),
            Self::GenericParam(id) => id.parameter().as_str(),
            Self::BuiltinFunction(name) => name.as_str(),
            Self::BuiltinConstant(name) => name.as_str(),
            Self::TimeScale(scale) => match scale {
                TimeScale::UTC => "UTC",
                TimeScale::TAI => "TAI",
                TimeScale::TT => "TT",
                TimeScale::TDB => "TDB",
                TimeScale::ET => "ET",
                TimeScale::GPST => "GPST",
                TimeScale::GST => "GST",
                TimeScale::BDT => "BDT",
                TimeScale::QZSST => "QZSST",
            },
            Self::Local(_) | Self::ExternFunction(_) => "",
        }
    }
}

/// A structured source spelling before semantic resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceSymbolPath {
    qualifier: Vec<NameAtom>,
    leaf: NameAtom,
}

impl SourceSymbolPath {
    #[must_use]
    pub fn from_name_path(path: &NamePath) -> Self {
        let qualifier = path
            .qualifier_and_leaf()
            .map_or_else(Vec::new, |(qualifier, _)| qualifier.to_vec());
        Self {
            qualifier,
            leaf: path.leaf().clone(),
        }
    }

    #[must_use]
    pub const fn local(leaf: NameAtom) -> Self {
        Self {
            qualifier: Vec::new(),
            leaf,
        }
    }

    #[must_use]
    pub const fn qualified(qualifier: Vec<NameAtom>, leaf: NameAtom) -> Self {
        Self { qualifier, leaf }
    }

    #[must_use]
    pub fn qualifier(&self) -> &[NameAtom] {
        &self.qualifier
    }

    #[must_use]
    pub const fn leaf(&self) -> &NameAtom {
        &self.leaf
    }

    #[must_use]
    pub const fn is_local(&self) -> bool {
        self.qualifier.is_empty()
    }
}

impl std::fmt::Display for SourceSymbolPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for segment in &self.qualifier {
            write!(f, "{segment}.")?;
        }
        self.leaf.fmt(f)
    }
}

/// Namespace-aware source spelling retained only when tolerant lowering could
/// not produce a canonical identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UnresolvedSymbol {
    /// A selective-import term, which may resolve to a declaration or a
    /// constructor according to the compiler's term namespace.
    Term(SourceSymbolPath),
    Declaration(SourceSymbolPath),
    /// A bare type annotation path, resolved by the compiler as either a
    /// dimension or a nominal type.
    TypeExpression(SourceSymbolPath),
    Dimension(SourceSymbolPath),
    Unit(SourceSymbolPath),
    StructType(SourceSymbolPath),
    Index(SourceSymbolPath),
    Field(FieldName),
    Function(SourceSymbolPath),
    /// Generic syntax whose expected sort is determined only after resolving
    /// the generic parameter declaration.
    GenericArgument(SourceSymbolPath),
}

impl UnresolvedSymbol {
    #[must_use]
    pub const fn path(&self) -> Option<&SourceSymbolPath> {
        match self {
            Self::Term(path)
            | Self::Declaration(path)
            | Self::TypeExpression(path)
            | Self::Dimension(path)
            | Self::Unit(path)
            | Self::StructType(path)
            | Self::Index(path)
            | Self::Function(path)
            | Self::GenericArgument(path) => Some(path),
            Self::Field(_) => None,
        }
    }

    /// Whether a canonical identity belongs to this unresolved namespace.
    #[must_use]
    pub const fn accepts(&self, target: &SymbolId) -> bool {
        match self {
            Self::Term(_) => matches!(target, SymbolId::Declaration(_) | SymbolId::Constructor(_)),
            Self::Declaration(_) => matches!(target, SymbolId::Declaration(_)),
            Self::TypeExpression(_) => {
                matches!(target, SymbolId::Dimension(_) | SymbolId::StructType(_))
            }
            Self::Dimension(_) => matches!(target, SymbolId::Dimension(_)),
            Self::Unit(_) => matches!(target, SymbolId::Unit(_)),
            Self::StructType(_) => matches!(target, SymbolId::StructType(_)),
            Self::Index(_) => matches!(target, SymbolId::Index(_)),
            Self::Field(_) => matches!(target, SymbolId::Field(_)),
            Self::Function(_) => matches!(
                target,
                SymbolId::BuiltinFunction(_) | SymbolId::ExternFunction(_)
            ),
            Self::GenericArgument(_) => matches!(
                target,
                SymbolId::Dimension(_)
                    | SymbolId::StructType(_)
                    | SymbolId::Index(_)
                    | SymbolId::GenericParam(_)
            ),
        }
    }
}

/// Target of one source occurrence.
///
/// Definitions use [`SymbolId`] directly. Only references may be unresolved,
/// making the invalid state “unresolved definition” impossible to construct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReferenceTarget {
    Resolved(SymbolId),
    Unresolved(UnresolvedSymbol),
}

impl From<SymbolId> for ReferenceTarget {
    fn from(value: SymbolId) -> Self {
        Self::Resolved(value)
    }
}

/// One source-visible import binding for a canonical target.
///
/// Multiple values may point to the same target, which represents multiple
/// aliases without selecting one spelling as canonical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleBinding {
    target: SymbolId,
    spelling: SourceSymbolPath,
}

impl VisibleBinding {
    #[must_use]
    pub const fn new(target: SymbolId, spelling: SourceSymbolPath) -> Self {
        Self { target, spelling }
    }

    #[must_use]
    pub const fn target(&self) -> &SymbolId {
        &self.target
    }

    #[must_use]
    pub const fn spelling(&self) -> &SourceSymbolPath {
        &self.spelling
    }

    #[must_use]
    pub fn resolves(&self, unresolved: &UnresolvedSymbol) -> bool {
        unresolved.accepts(&self.target)
            && match unresolved {
                UnresolvedSymbol::Field(field) => self.target.leaf_name() == field.as_str(),
                _ => unresolved.path().is_some_and(|path| path == &self.spelling),
            }
    }
}
