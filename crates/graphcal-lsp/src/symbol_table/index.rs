//! The symbol index: definitions, references, and their lookup structures.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::BindableVisibility;
use graphcal_compiler::resolved_name::{
    ResolvedDeclName, ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName, ResolvedUnitName,
};
use graphcal_compiler::syntax::span::Span;

use tower_lsp::lsp_types::Position;

use crate::convert::LineIndex;
use crate::symbol_identity::{ReferenceTarget, SymbolId, UnresolvedSymbol};

/// A source declaration identity whose syntactic kind and canonical namespace
/// agree by construction.
///
/// This private builder type prevents callers from pairing, for example, a
/// unit key with `SymbolCategory::Const`. The presentation category and
/// semantic ID are derived together in [`TopLevelDefinition::into_parts`].
pub(super) enum TopLevelDefinition {
    Param(ResolvedDeclName),
    Node(ResolvedDeclName),
    Const(ResolvedDeclName),
    Dimension(ResolvedDimName),
    Unit(ResolvedUnitName),
    StructType(ResolvedStructTypeName),
    Index(ResolvedIndexName),
    Assert(ResolvedDeclName),
    Plot(ResolvedDeclName),
    Figure(ResolvedDeclName),
    Layer(ResolvedDeclName),
    Dag(ResolvedDeclName),
}

impl TopLevelDefinition {
    fn into_parts(self) -> (SymbolId, SymbolCategory, String) {
        match self {
            Self::Param(name) => {
                definition_parts(SymbolId::Declaration(name), SymbolCategory::Param)
            }
            Self::Node(name) => definition_parts(SymbolId::Declaration(name), SymbolCategory::Node),
            Self::Const(name) => {
                definition_parts(SymbolId::Declaration(name), SymbolCategory::Const)
            }
            Self::Dimension(name) => {
                definition_parts(SymbolId::Dimension(name), SymbolCategory::Dimension)
            }
            Self::Unit(name) => definition_parts(SymbolId::Unit(name), SymbolCategory::Unit),
            Self::StructType(name) => {
                definition_parts(SymbolId::StructType(name), SymbolCategory::StructType)
            }
            Self::Index(name) => definition_parts(SymbolId::Index(name), SymbolCategory::Index),
            Self::Assert(name) => {
                definition_parts(SymbolId::Declaration(name), SymbolCategory::Assert)
            }
            Self::Plot(name) => definition_parts(SymbolId::Declaration(name), SymbolCategory::Plot),
            Self::Figure(name) => {
                definition_parts(SymbolId::Declaration(name), SymbolCategory::Figure)
            }
            Self::Layer(name) => {
                definition_parts(SymbolId::Declaration(name), SymbolCategory::Layer)
            }
            Self::Dag(name) => definition_parts(SymbolId::Declaration(name), SymbolCategory::Dag),
        }
    }
}

fn definition_parts(key: SymbolId, category: SymbolCategory) -> (SymbolId, SymbolCategory, String) {
    let name = key.leaf_name().to_string();
    (key, category, name)
}

/// The category of a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolCategory {
    Param,
    Node,
    Const,
    Dimension,
    Unit,
    StructType,
    Constructor,
    Index,
    IndexVariant,
    Field,
    GenericParam,
    LocalVar,
    BuiltinFn,
    /// Extern function declared by an `import plugin` block.
    ExternFn,
    BuiltinConst,
    Assert,
    Plot,
    Figure,
    Layer,
    Dag,
}

/// Information about a symbol definition.
#[derive(Debug, Clone)]
pub struct DefinitionInfo {
    pub name: String,
    pub category: SymbolCategory,
    /// Span of just the name token.
    pub name_span: Span,
    /// Span of the full declaration.
    pub decl_span: Span,
    /// Human-readable type/signature for hover (populated from TIR).
    pub type_description: Option<String>,
    /// Additional detail for hover.
    pub detail: Option<String>,
    /// Visibility / bindability of the declaration.
    ///
    /// `None` for builtins, fields, and local variables (concepts that have
    /// no surface annotation).
    pub visibility: Option<BindableVisibility>,
    /// The `///` doc block preceding the declaration, rendered in hover.
    pub doc: Option<String>,
}

impl DefinitionInfo {
    /// The label editors show for this definition under `key`: its name, or
    /// `alias::fn` for an extern function, whose alias is a typed part of
    /// the key rather than of the name.
    #[must_use]
    pub fn label(&self, key: &SymbolId) -> std::borrow::Cow<'_, str> {
        match key {
            SymbolId::ExternFunction(function) => {
                std::borrow::Cow::Owned(format!("{}::{}", function.plugin(), function.function()))
            }
            _ => std::borrow::Cow::Borrowed(self.name.as_str()),
        }
    }

    /// Returns `true` if this is a built-in function or constant.
    pub const fn is_builtin(&self) -> bool {
        matches!(
            self.category,
            SymbolCategory::BuiltinFn | SymbolCategory::BuiltinConst
        )
    }

    /// Returns `true` if this definition has a navigable source location.
    ///
    /// Builtins and definitions with empty name spans have no source to navigate to.
    pub const fn is_navigable(&self) -> bool {
        !self.is_builtin() && !self.name_span.is_empty()
    }
}

/// A reference occurrence: a name that refers to a definition.
#[derive(Debug, Clone)]
pub struct ReferenceInfo {
    /// Byte-offset span of this reference in the current file.
    pub span: Span,
    /// Canonical target, or a namespace-aware unresolved spelling retained
    /// while the user is editing incomplete code.
    pub target: ReferenceTarget,
}

/// A precomputed inlay-hint candidate: a declaration that could produce a hint.
///
/// Populated at [`SymbolTable::finalize`] time so the inlay-hint handler
/// avoids walking every definition (including builtins/imports) and avoids
/// O(source-length) scans to turn each name-span into an LSP `Position` on
/// every request.
#[derive(Debug, Clone)]
pub struct InlayHintEntry {
    /// Key into [`SymbolTable::definitions`] for this declaration. Shared
    /// via `Arc` with [`SymbolTable::defs_by_name_span`] so each key is
    /// allocated once and refcounted across the two sorted indices.
    pub key: Arc<SymbolId>,
    /// LSP position of the declaration's name start.
    pub name_start: Position,
    /// LSP position of the declaration's name end (where the hint is placed).
    pub name_end: Position,
}

/// A duplicate canonical definition rejected by [`DefinitionIndex`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("duplicate editor definition for {key:?}")]
pub struct DuplicateDefinition {
    pub key: SymbolId,
    pub first_span: Span,
    pub duplicate_span: Span,
}

/// Invariant-preserving definition map.
///
/// Mutation is private and fallible: a canonical ID can never silently replace
/// another definition. Consumers receive only read APIs.
#[derive(Debug, Default)]
pub struct DefinitionIndex {
    entries: HashMap<SymbolId, DefinitionInfo>,
}

impl DefinitionIndex {
    fn insert(
        &mut self,
        key: SymbolId,
        definition: DefinitionInfo,
    ) -> Result<(), Box<DuplicateDefinition>> {
        use std::collections::hash_map::Entry;
        match self.entries.entry(key.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(definition);
                Ok(())
            }
            Entry::Occupied(entry) => Err(Box::new(DuplicateDefinition {
                key,
                first_span: entry.get().name_span,
                duplicate_span: definition.name_span,
            })),
        }
    }

    pub fn get(&self, key: &SymbolId) -> Option<&DefinitionInfo> {
        self.entries.get(key)
    }

    pub(super) fn get_mut(&mut self, key: &SymbolId) -> Option<&mut DefinitionInfo> {
        self.entries.get_mut(key)
    }

    pub(super) fn values_mut(
        &mut self,
    ) -> std::collections::hash_map::ValuesMut<'_, SymbolId, DefinitionInfo> {
        self.entries.values_mut()
    }

    pub fn contains_key(&self, key: &SymbolId) -> bool {
        self.entries.contains_key(key)
    }

    pub fn values(&self) -> std::collections::hash_map::Values<'_, SymbolId, DefinitionInfo> {
        self.entries.values()
    }

    pub fn iter(&self) -> std::collections::hash_map::Iter<'_, SymbolId, DefinitionInfo> {
        self.entries.iter()
    }

    pub fn keys(&self) -> std::collections::hash_map::Keys<'_, SymbolId, DefinitionInfo> {
        self.entries.keys()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

impl<'a> IntoIterator for &'a DefinitionIndex {
    type Item = (&'a SymbolId, &'a DefinitionInfo);
    type IntoIter = std::collections::hash_map::Iter<'a, SymbolId, DefinitionInfo>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

/// The complete symbol table for one file.
#[derive(Debug)]
pub struct SymbolTable {
    /// Canonical module owning source definitions in this table.
    owner: DagId,
    /// All symbol definitions keyed by a typed `SymbolId`.
    pub(crate) definitions: DefinitionIndex,
    /// Duplicate IDs rejected while tolerantly indexing invalid source.
    definition_conflicts: Vec<DuplicateDefinition>,
    /// All reference occurrences sorted by span offset.
    pub(super) references: Vec<ReferenceInfo>,
    /// Secondary index: name-span byte offset → `SymbolId`.
    ///
    /// Populated alongside `definitions` so that `find_definition_key` is O(1)
    /// instead of a linear reverse scan.
    name_span_to_key: HashMap<usize, SymbolId>,
    /// Definition name-spans sorted by offset, each paired with the entry's
    /// `SymbolId` (refcounted — shared with [`Self::inlay_hint_entries`] so
    /// the keys are allocated once at finalize time). Used by
    /// `find_definition_at` for O(log n) lookup instead of scanning every
    /// definition on every hover/goto/rename request.
    ///
    /// Populated by [`SymbolTable::finalize`]; definitions added after
    /// `finalize` is called will not appear here until it is called again.
    defs_by_name_span: Vec<(Span, Arc<SymbolId>)>,
    /// Precomputed inlay-hint candidates, sorted by name-start line/column.
    ///
    /// Only `Param`, `Node`, and `Const` declarations with non-empty name
    /// spans appear here — builtins, imports, and other categories are
    /// filtered out up front so the request path does zero work for them.
    ///
    /// Populated by [`SymbolTable::finalize`].
    inlay_hint_entries: Vec<InlayHintEntry>,
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new(DagId::root_in_package("__graphcal_lsp__", "empty"))
    }
}

impl SymbolTable {
    pub(super) fn new(owner: DagId) -> Self {
        Self {
            owner,
            definitions: DefinitionIndex::default(),
            definition_conflicts: Vec::new(),
            references: Vec::new(),
            name_span_to_key: HashMap::new(),
            defs_by_name_span: Vec::new(),
            inlay_hint_entries: Vec::new(),
        }
    }

    pub(crate) const fn owner(&self) -> &DagId {
        &self.owner
    }

    /// Whether a source-defined identity can be referenced outside this file.
    /// Attached members inherit their nominal declaration's visibility, while
    /// symbols inside a private enclosing DAG remain file-local even when the
    /// port itself is bindable.
    #[must_use]
    pub fn is_externally_visible(&self, key: &SymbolId) -> bool {
        if matches!(
            key,
            SymbolId::Local(_)
                | SymbolId::ExternFunction(_)
                | SymbolId::BuiltinFunction(_)
                | SymbolId::BuiltinConstant(_)
                | SymbolId::TimeScale(_)
        ) {
            return false;
        }
        let Some(definition) = self.definitions.get(key) else {
            return true;
        };
        if definition.visibility == Some(BindableVisibility::Private) {
            return false;
        }

        let mut owner = key.owner().cloned();
        while let Some(current) = owner {
            if current == self.owner {
                return true;
            }
            let Some(parent) = current.parent() else {
                return true;
            };
            let Some(dag_declaration) = dag_declaration_name(&current) else {
                return true;
            };
            match self
                .definitions
                .get(&SymbolId::Declaration(dag_declaration))
            {
                Some(dag) if dag.visibility == Some(BindableVisibility::Private) => return false,
                Some(_) => owner = Some(parent),
                None => return true,
            }
        }
        true
    }

    /// Find the reference at a given byte offset, if any.
    pub fn find_reference_at(&self, offset: usize) -> Option<&ReferenceInfo> {
        // Binary search for a reference whose span contains the offset.
        let idx = self
            .references
            .binary_search_by(|r| {
                if offset < r.span.offset() {
                    std::cmp::Ordering::Greater
                } else if offset >= r.span.offset() + r.span.len() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;
        Some(&self.references[idx])
    }

    /// Find the definition whose name span contains the given byte offset, if any.
    ///
    /// O(log n) via [`SymbolTable::defs_by_name_span`], which is populated by
    /// [`SymbolTable::finalize`].
    pub fn find_definition_at(&self, offset: usize) -> Option<&DefinitionInfo> {
        let idx = self
            .defs_by_name_span
            .binary_search_by(|(span, _)| {
                if offset < span.offset() {
                    std::cmp::Ordering::Greater
                } else if offset >= span.offset() + span.len() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;
        let key = &self.defs_by_name_span[idx].1;
        self.definitions.get(key.as_ref())
    }

    /// Build the sorted `defs_by_name_span` index and precompute inlay-hint
    /// candidate positions. Called once at the end of `build_from_ast`.
    ///
    /// `source` is used to turn byte offsets into LSP `Position`s ahead of
    /// time, so the inlay-hint request path avoids O(source) scans per
    /// declaration.
    pub(super) fn finalize(&mut self, source: &str) {
        // Build sorted-by-name-span and inlay-hint indices in one pass,
        // sharing each key via `Arc` so a definition's `SymbolId` is
        // allocated once and refcounted across the two sorted views.
        // `Arc` (not `Rc`) because `AnalysisResult` lives in an
        // `Arc<RwLock<…>>` and must be `Send + Sync` for the LSP runtime.
        let lines = LineIndex::new(source);
        let mut span_index: Vec<(Span, Arc<SymbolId>)> = Vec::new();
        let mut hint_entries: Vec<InlayHintEntry> = Vec::new();
        for (key, def) in &self.definitions {
            if def.name_span.is_empty() {
                continue;
            }
            let shared = Arc::new(key.clone());
            span_index.push((def.name_span, Arc::clone(&shared)));
            if matches!(
                def.category,
                SymbolCategory::Param | SymbolCategory::Node | SymbolCategory::Const
            ) {
                let start = def.name_span.offset();
                let end = start + def.name_span.len();
                hint_entries.push(InlayHintEntry {
                    key: shared,
                    name_start: lines.position(start),
                    name_end: lines.position(end),
                });
            }
        }
        span_index.sort_by_key(|(span, _)| span.offset());
        self.defs_by_name_span = span_index;
        self.inlay_hint_entries = hint_entries;
        self.inlay_hint_entries
            .sort_by_key(|e| (e.name_start.line, e.name_start.character));
    }

    /// Returns the precomputed inlay-hint candidates.
    ///
    /// Only `Param`/`Node`/`Const` declarations with non-empty name spans
    /// are included, sorted by name-start position. The inlay-hint request
    /// handler iterates this directly instead of filtering every definition.
    pub fn inlay_hint_entries(&self) -> &[InlayHintEntry] {
        &self.inlay_hint_entries
    }

    /// Declaration identities some inlay hint can display a value for.
    #[must_use]
    pub fn inlay_hint_declarations(&self) -> HashSet<&ResolvedDeclName> {
        self.inlay_hint_entries
            .iter()
            .filter_map(|entry| match entry.key.as_ref() {
                SymbolId::Declaration(identity) => Some(identity),
                _ => None,
            })
            .collect()
    }

    /// Look up the key for a definition by its name-span byte offset.
    ///
    /// Returns `None` if the definition has an empty name-span (builtins,
    /// fields) or if the definition did not come from this table's
    /// `insert_definition` path.
    ///
    /// O(1) via the `name_span_to_key` secondary index — no linear scan needed.
    pub fn find_definition_key(&self, definition: &DefinitionInfo) -> Option<SymbolId> {
        self.name_span_to_key
            .get(&definition.name_span.offset())
            .cloned()
    }

    /// Insert a definition and update the secondary `name_span_to_key` index.
    ///
    /// The operation is atomic: duplicate semantic IDs return a typed error and
    /// leave both indices unchanged.
    fn try_insert_definition(
        &mut self,
        key: SymbolId,
        definition: DefinitionInfo,
    ) -> Result<(), Box<DuplicateDefinition>> {
        let name_span = definition.name_span;
        self.definitions.insert(key.clone(), definition)?;
        if !name_span.is_empty() {
            self.name_span_to_key.insert(name_span.offset(), key);
        }
        Ok(())
    }

    /// Tolerant builder boundary: preserve the first valid definition and
    /// retain every typed duplicate error for diagnostics/debugging.
    pub(super) fn insert_definition(&mut self, key: SymbolId, definition: DefinitionInfo) {
        if let Err(conflict) = self.try_insert_definition(key, definition) {
            self.definition_conflicts.push(*conflict);
        }
    }

    #[cfg(test)]
    pub(super) fn definition_conflicts(&self) -> &[DuplicateDefinition] {
        &self.definition_conflicts
    }

    /// Register a source definition under its canonical semantic identity.
    pub(super) fn register_definition(
        &mut self,
        identity: TopLevelDefinition,
        name_span: Span,
        decl_span: Span,
        type_description: Option<String>,
        detail: Option<String>,
        visibility: BindableVisibility,
    ) {
        let (key, category, name) = identity.into_parts();
        self.insert_definition(
            key,
            DefinitionInfo {
                doc: None,
                name,
                category,
                name_span,
                decl_span,
                type_description,
                detail,
                visibility: Some(visibility),
            },
        );
    }

    /// Every indexed reference occurrence in source order.
    #[must_use]
    pub fn references(&self) -> &[ReferenceInfo] {
        &self.references
    }

    /// Resolve a reference target against definitions declared in this table.
    /// Qualified unresolved spellings belong to an import binding and are
    /// intentionally left for the workspace analysis layer.
    pub fn resolve_local_target(&self, target: &ReferenceTarget) -> Option<SymbolId> {
        match target {
            ReferenceTarget::Resolved(target) => Some(target.clone()),
            ReferenceTarget::Unresolved(unresolved) => {
                let leaf = if let UnresolvedSymbol::Field(field) = unresolved {
                    field.as_str()
                } else {
                    let path = unresolved.path()?;
                    if !path.is_local() {
                        return None;
                    }
                    path.leaf().as_str()
                };
                let mut candidates = self.definitions.keys().filter(|candidate| {
                    candidate.leaf_name() == leaf && unresolved.accepts(candidate)
                });
                let candidate = candidates.next()?;
                candidates.next().is_none().then(|| candidate.clone())
            }
        }
    }

    /// Whether a source occurrence has the target's spelling and namespace but
    /// could not be assigned a unique canonical identity. Rename must refuse
    /// such a target rather than silently omit a possible occurrence.
    pub fn has_ambiguous_reference_to(&self, target: &SymbolId) -> bool {
        self.references.iter().any(|reference| {
            let ReferenceTarget::Unresolved(unresolved) = &reference.target else {
                return false;
            };
            let same_local_spelling = match unresolved {
                UnresolvedSymbol::Field(field) => field.as_str() == target.leaf_name(),
                _ => unresolved.path().is_some_and(|path| {
                    path.is_local() && path.leaf().as_str() == target.leaf_name()
                }),
            };
            same_local_spelling
                && unresolved.accepts(target)
                && self.resolve_local_target(&reference.target).is_none()
        })
    }

    /// Find all references that point to the given target key.
    pub fn find_all_references(&self, target: &SymbolId) -> Vec<&ReferenceInfo> {
        self.references
            .iter()
            .filter(|reference| {
                self.resolve_local_target(&reference.target).as_ref() == Some(target)
            })
            .collect()
    }

    /// Every source declaration spelled `name`, across all of the file's
    /// scopes (independent DAGs may each declare one), excluding members,
    /// locals and built-ins.
    pub fn declarations_named(&self, name: &str) -> Vec<&DefinitionInfo> {
        self.definitions
            .values()
            .filter(|definition| {
                definition.name == name
                    && !definition.name_span.is_empty()
                    && !matches!(
                        definition.category,
                        SymbolCategory::Constructor
                            | SymbolCategory::Field
                            | SymbolCategory::GenericParam
                            | SymbolCategory::LocalVar
                            | SymbolCategory::BuiltinFn
                            | SymbolCategory::BuiltinConst
                            | SymbolCategory::ExternFn
                    )
            })
            .collect()
    }
}

/// The declaration that names `dag` inside its parent module, when `dag` is a
/// spelled child of another DAG. Root modules and anonymous include instances
/// have no such declaration.
pub(super) fn dag_declaration_name(dag: &DagId) -> Option<ResolvedDeclName> {
    ResolvedDeclName::naming_inline_dag(dag)
}
